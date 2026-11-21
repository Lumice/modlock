#include "crosshaircapture.h"
#include "gameconfigcontroller.h"
#include <QDir>
#include <QFile>
#include <QJsonDocument>
#include <QJSValue>
#include <QKeyEvent>
#include <QQuickItem>
#include <QQuickWindow>
#include <QTimer>
#include <QCoreApplication>
#include <cmath>
#include <cstdio>
#include <memory>
#include <stdexcept>

namespace {
QVariant plain(const QVariant &v) {
    return v.metaType().id() == qMetaTypeId<QJSValue>() ? v.value<QJSValue>().toVariant() : v;
}
void check(bool ok, const QString &message) {
    if (!ok) throw std::runtime_error(message.toStdString());
}
QVariant field(const QVariantMap &values, const QString &path) {
    QVariant result = values;
    for (const auto &part : path.split('.')) result = result.toMap().value(part);
    return result;
}
struct Step { QString hero, name; int zoom; qreal scroll; };
struct State {
    QVariantMap expected;
    QList<Step> steps;
    int index = 0, ticks = 0;
    bool settled = false, addedScrolls = false;
};
}

void startCrosshairCapture(QQuickWindow *window) {
    window->resize(1280, 900);
    QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QString("crosshair")));
    auto state = std::make_shared<State>();
    QFile input(qEnvironmentVariable("CROSSHAIR_EXPECTED"));
    check(input.open(QIODevice::ReadOnly), "crosshair expected values missing");
    state->expected = QJsonDocument::fromJson(input.readAll()).toVariant().toMap();
    for (const auto &hero : {QString(), QString("hero_inferno"), QString("hero_doorman")}) {
        const QString name = hero.isEmpty() ? "all-heroes" : hero == "hero_inferno" ? "infernus" : "doorman";
        for (int zoom : {1, 4}) state->steps.append({hero, name + QString("-%1x").arg(zoom), zoom, 0});
    }
    auto *timer = new QTimer(window);
    timer->setInterval(180);
    QObject::connect(timer, &QTimer::timeout, window, [window, timer, state] {
        try {
            check(++state->ticks < 180, "crosshair capture timed out");
            auto *panel = window->findChild<QQuickItem *>("crosshairPanel");
            if (!panel || !panel->isVisible()) return;
            auto *controller = qvariant_cast<GameConfigController *>(panel->property("controller"));
            if (!controller || !controller->available() || controller->busy() || controller->values().isEmpty()) return;
            auto *scroll = panel->parentItem();
            while (scroll && scroll->metaObject()->indexOfProperty("contentY") < 0) scroll = scroll->parentItem();
            check(scroll, "crosshair scroll area missing");
            if (state->index == 6 && !state->addedScrolls) {
                state->addedScrolls = true;
                const qreal extent = std::max(0., scroll->property("contentHeight").toReal() - scroll->height());
                for (const auto &hero : {QString(), QString("hero_inferno"), QString("hero_doorman")}) {
                    const QString name = hero.isEmpty() ? "all-heroes" : hero == "hero_inferno" ? "infernus" : "doorman";
                    int page = 1;
                    for (qreal y = 0;; y = std::min(extent, y + scroll->height() - 140)) {
                        state->steps.append({hero, name + QString("-settings-%1").arg(page++), 1, y});
                        if (y >= extent) break;
                    }
                }
            }
            if (state->index >= state->steps.size()) {
                std::puts("PASS UI crosshair values: all 21 saved fields match for All heroes, Infernus and Doorman at 1x and 4x; scrolled settings captured");
                timer->stop(); QCoreApplication::exit(0); return;
            }
            const auto step = state->steps[state->index];
            if (!state->settled) {
                auto *hero = window->findChild<QObject *>("crosshairHero");
                auto *zoom = window->findChild<QObject *>("crosshairZoom");
                check(hero && zoom, "crosshair selectors missing");
                const auto model = plain(hero->property("model")).toList();
                int heroIndex = -1;
                for (int i = 0; i < model.size(); ++i)
                    if (model[i].toMap().value("key").toString() == step.hero) { heroIndex = i; break; }
                check(heroIndex >= 0, "crosshair hero missing from selector");
                hero->setProperty("currentIndex", heroIndex);
                QMetaObject::invokeMethod(hero, "activated", Q_ARG(int, heroIndex));
                zoom->setProperty("currentIndex", step.zoom == 1 ? 0 : 2);
                QMetaObject::invokeMethod(zoom, "activated", Q_ARG(int, step.zoom == 1 ? 0 : 2));
                panel->setProperty("customColourExpanded", true);
                scroll->setProperty("contentY", step.scroll);
                state->settled = true;
                window->requestUpdate(); return;
            }
            check(panel->isVisible(), "crosshair panel is hidden during capture");
            check(panel->property("hero").toString() == step.hero && panel->property("zoom").toInt() == step.zoom, "selectors did not update preview");
            const auto effective = plain(panel->property("v")).toMap();
            const auto expected = state->expected.value(step.hero).toMap();
            check(expected.size() == 21, "expected 21 saved fields per hero");
            for (auto it = expected.cbegin(); it != expected.cend(); ++it) {
                const QString key = it.key();
                const auto actual = field(effective, key);
                if (it.value().metaType().id() == QMetaType::Bool) {
                    auto *control = panel->findChild<QObject *>("crosshairToggle." + key);
                    check(control && actual.toBool() == it.value().toBool() && control->property("checked").toBool() == it.value().toBool(), "toggle mismatch: " + key);
                } else {
                    auto *row = panel->findChild<QObject *>("crosshairValue." + key);
                    auto *number = panel->findChild<QObject *>("crosshairNumber." + key);
                    check(row && number, "missing displayed field: " + key);
                    for (const double n : {actual.toDouble(), row->property("value").toDouble(), number->property("text").toString().toDouble()})
                        check(std::abs(n - it.value().toDouble()) < 0.00001, "displayed value mismatch: " + key);
                }
            }
            auto *note = panel->findChild<QObject *>("crosshairPreviewNote");
            check(note && note->property("text").toString().contains("Shared dot/pip colours") && note->property("text").toString().contains("no themed artwork or weapon spread"), "preview limitations must describe shared colours and themed mode");
            const QString directory = qEnvironmentVariable("CROSSHAIR_CAPTURES");
            if (!directory.isEmpty()) check(window->grabWindow().save(QDir(directory).filePath(step.name + ".png")), "could not capture " + step.name);
            std::printf("PASS crosshair %s (%lld displayed values)\n", qPrintable(step.name), static_cast<long long>(expected.size()));
            ++state->index; state->settled = false;
        } catch (const std::exception &error) {
            std::fprintf(stderr, "FAIL crosshair values: %s\n", error.what());
            timer->stop(); QCoreApplication::exit(1);
        }
    });
    timer->start();
}

void startCrosshairRegression(QQuickWindow *window, const QString &test) {
    window->resize(1280,900);
    QMetaObject::invokeMethod(window,"screenshotSetup",Q_ARG(QVariant,QString("crosshair")));
    auto stage = std::make_shared<int>(0);
    auto ticks = std::make_shared<int>(0);
    auto before = std::make_shared<QVariantMap>();
    auto *timer = new QTimer(window); timer->setInterval(180);
    QObject::connect(timer,&QTimer::timeout,window,[=] {
        try {
            check(++*ticks < 120, "regression timed out: " + test);
            auto *panel=window->findChild<QQuickItem *>("crosshairPanel");
            if(!panel || !panel->isVisible()) return;
            auto *config=qvariant_cast<GameConfigController *>(panel->property("controller"));
            if(!config || !config->available() || config->busy() || config->values().isEmpty()) return;
            if(test=="picker") {
                auto *picker=window->findChild<QObject *>("crosshairHero");
                auto rows=plain(picker->property("model")).toList();
                bool atlas=false,unknown=false; QSet<QString> keys,names;
                for(const auto &entry:rows) {
                    auto row=entry.toMap(); auto key=row.value("key").toString(),name=row.value("name").toString();
                    check(!keys.contains(key) && !names.contains(name),"duplicate picker scope or label"); keys.insert(key); names.insert(name);
                    if(key=="hero_atlas") { atlas=name=="Abrams"; picker->setProperty("currentIndex",keys.size()-1); QMetaObject::invokeMethod(picker,"activated",Q_ARG(int,keys.size()-1)); }
                    unknown|=key=="hero_review_unknown" && name==key;
                }
                check(atlas && panel->property("hero").toString()=="hero_atlas","hero_atlas must be selectable as Abrams");
                check(unknown,"unknown saved hero must be selectable by raw scope");
            } else if(test=="legacy-import") {
                if(*stage==0) {
                    *before=config->values(); panel->setProperty("hero","hero_inferno");
                    check(config->importCode("6;3;12;0.5;true;true;0.4;0.3;12;34;56;Default"),"legacy import rejected");
                    ++*stage; return;
                }
                check(config->error().isEmpty(),"legacy import failed");
                auto base=config->values(); auto overrides=base.take("overrides").toMap();
                auto old=*before; old.remove("overrides");
                check(base==old,"hero legacy import changed the base");
                auto hero=overrides.value("hero_inferno").toMap();
                check(hero.value("gap").toInt()==6 && hero.value("color").toMap().value("r").toInt()==12 && hero.value("pipOutlineWidth").toInt()==1,"decoded values did not become hero override");
                check(hero.value("dotSize").toInt()==5 && hero.value("dotOutlineWidth").toInt()==2,"legacy import discarded newer hero settings");
            } else if(test=="legacy-export") {
                check(config->setValue("pipOutlineWidth",2),"outline edit rejected");
                check(config->exportCode().split(';').value(4)=="true","legacy export lost numeric pip outline");
                check(config->setValue("pipOutlineWidth",0),"outline reset rejected");
                check(config->exportCode().split(';').value(4)=="false","legacy export retained disabled outline");
            } else if (test == "hex-colour") {
                if (*stage == 0) {
                    panel->setProperty("customColourExpanded", true);
                    ++*stage;
                    return;
                }
                auto *hex = panel->findChild<QQuickItem *>("crosshairHex");
                check(hex && hex->isVisible() && hex->isEnabled(), "Hex colour field is unavailable");
                hex->forceActiveFocus(Qt::OtherFocusReason);
                check(hex->hasActiveFocus(), "Hex colour field did not take focus");
                const auto type = [window, hex](const QString &text) {
                    check(QMetaObject::invokeMethod(hex, "selectAll"), "Hex colour field could not select its text");
                    QKeyEvent key(QEvent::KeyPress, Qt::Key_unknown, Qt::NoModifier, text);
                    QCoreApplication::sendEvent(window, &key);
                };
                type("#12aBF0");
                const auto values = config->values();
                const auto rgb = values.value("color").toMap();
                check(rgb.value("r").toInt() == 18 && rgb.value("g").toInt() == 171 && rgb.value("b").toInt() == 240,
                      "Hex colour edit did not update all RGB channels");
                for (const auto &invalid : {QStringLiteral("#fff"), QStringLiteral("ffffff"), QStringLiteral("#gg0000")}) {
                    type(invalid);
                    check(config->values() == values, "Invalid hex colour changed the crosshair: " + invalid);
                }
            } else {
                if(*stage==0) {
                    for(const auto &p:QVariantMap{{"dotSize",4},{"dotOpacity",test=="dpi"?1:0},{"dotOutlineWidth",0},{"pipOpacity",test=="dpi"?0:1},{"pipOutlineWidth",0},{"width",2},{"height",10},{"gap",-5}}.asKeyValueRange())
                        check(config->setValue(p.first,p.second),"geometry fixture edit rejected: "+p.first);
                    ++*stage; window->requestUpdate(); return;
                }
                QQuickItem *canvas=nullptr;
                for(auto *item:panel->findChildren<QQuickItem *>()) if(item->inherits("QQuickCanvasItem")) { canvas=item; break; }
                check(canvas,"crosshair canvas missing");
                const QImage shot=window->grabWindow(); const qreal dpr=window->devicePixelRatio();
                const auto pos=canvas->mapToScene(QPointF())*dpr;
                int left=shot.width(),right=-1,top=shot.height(),bottom=-1;
                for(int y=qRound(pos.y());y<qRound(pos.y()+canvas->height()*dpr);++y)
                    for(int x=qRound(pos.x());x<qRound(pos.x()+canvas->width()*dpr);++x)
                        if(shot.pixelColor(x,y)==QColor(Qt::white)) { left=std::min(left,x);right=std::max(right,x);top=std::min(top,y);bottom=std::max(bottom,y); }
                const int expected=test=="dpi"?4:10;
                check(right-left+1==expected && bottom-top+1==expected,QString("%1: physical bounds %2x%3, expected %4 (DPR %5)").arg(test).arg(right-left+1).arg(bottom-top+1).arg(expected).arg(dpr));
            }
            std::printf("PASS UI crosshair regression %s\n",qPrintable(test)); timer->stop(); QCoreApplication::exit(0);
        } catch(const std::exception &error) { std::fprintf(stderr,"FAIL crosshair %s: %s\n",qPrintable(test),error.what()); timer->stop(); QCoreApplication::exit(1); }
    }); timer->start();
}
