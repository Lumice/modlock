#pragma once
#include <functional>
#include "textutils.h"
#include <QAccessible>
#include <QFont>
#include <QHoverEvent>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QJSValue>
#include <QSet>
#include <QUuid>

namespace {

inline double decoChannel(double v) {
    return v <= 0.04045 ? v / 12.92 : std::pow((v + 0.055) / 1.055, 2.4);
}

inline double decoLuminance(const QColor &c) {
    return 0.2126 * decoChannel(c.redF()) + 0.7152 * decoChannel(c.greenF()) + 0.0722 * decoChannel(c.blueF());
}

inline double decoContrast(const QColor &a, const QColor &b) {
    const double l1 = decoLuminance(a), l2 = decoLuminance(b);
    return (std::max(l1, l2) + 0.05) / (std::min(l1, l2) + 0.05);
}

inline void applyDecoPalette(bool dark) {
    auto palette = QGuiApplication::palette();
    palette.setColor(QPalette::Window, QColor(dark ? "#0e0c15" : "#f1eff5"));
    palette.setColor(QPalette::Base, QColor(dark ? "#120f1b" : "#ffffff"));
    palette.setColor(QPalette::AlternateBase, QColor(dark ? "#14111d" : "#e9e6ef"));
    palette.setColor(QPalette::Button, QColor(dark ? "#1a1626" : "#fbfafd"));
    const QColor text(dark ? "#eee8dc" : "#1b1626");
    const QColor disabled(dark ? "#716a7b" : "#9b94a6");
    for (const auto role : {QPalette::WindowText, QPalette::Text, QPalette::ButtonText}) {
        palette.setColor(role, text);
        palette.setColor(QPalette::Disabled, role, disabled);
    }
    QGuiApplication::setPalette(palette);
}

inline void decoSettle(int steps = 4, int stepMs = 30) {
    for (int i = 0; i < steps; ++i) {
        QEventLoop loop;
        QTimer::singleShot(stepMs, &loop, &QEventLoop::quit);
        loop.exec();
    }
}

inline void saveDecoCapture(QQuickWindow *window, const QString &name, int width = 0, int height = 0) {
    const auto dir = qEnvironmentVariable("DECO_CAPTURE_DIR");
    if (dir.isEmpty()) return;
    QDir().mkpath(dir);
    if (width > 0 && height > 0) {
        window->resize(width, height);
        decoSettle(6, 30);
    } else {
        decoSettle(3, 25);
    }
    require(window->grabWindow().save(dir + "/" + name + ".png"), qPrintable("Deco capture failed: " + name));
}

inline void saveDecoSurfaceCaptures(QQuickWindow *window, const QString &name) {
    if (qEnvironmentVariableIsEmpty("DECO_CAPTURE_DIR")) return;
    const auto palette = QGuiApplication::palette();
    const auto size = window->size();
    for (const bool dark : {true, false}) {
        applyDecoPalette(dark);
        for (const int width : {1200, 720})
            saveDecoCapture(window, name + (dark ? "-dark-" : "-light-") + QString::number(width), width, 780);
    }
    QGuiApplication::setPalette(palette);
    window->resize(size);
    decoSettle();
}

inline bool decoWaitFor(const std::function<bool()> &pred, int timeoutMs = 4000) {
    QElapsedTimer t;
    t.start();
    while (t.elapsed() < timeoutMs) {
        if (pred()) return true;
        decoSettle(1, 20);
    }
    return pred();
}

inline void requireAccessibleIgnored(QObject *object, const char *what) {
    bool ignored = false;
    for (auto *child : object->children()) {
        const auto prop = child->property("ignored");
        if (prop.isValid() && prop.toBool()) {
            ignored = true;
            break;
        }
    }
    require(ignored, what);
}

inline void verifyLogoHalo(QQuickWindow *window, QObject *tokens) {
    auto *outer = qobject_cast<QQuickItem *>(named(window, "headerLogoHaloOuter"));
    auto *inner = qobject_cast<QQuickItem *>(named(window, "headerLogoHaloInner"));
    require(outer && outer->isVisible() && inner && inner->isVisible(), "Header logo static halos hidden");
    require(outer->width() >= 36 && inner->width() >= 28 && outer->width() > inner->width(), "Header logo halo sizes invalid");
    const QColor live = tokens->property("live").value<QColor>();
    const QColor outerColor = outer->property("color").value<QColor>();
    const QColor innerColor = inner->property("color").value<QColor>();
    require(outerColor.alpha() > 0 && innerColor.alpha() > outerColor.alpha(), "Header logo halo alpha hierarchy wrong");
    require(qAbs(outerColor.red() - live.red()) <= 2 && qAbs(outerColor.green() - live.green()) <= 2 && qAbs(outerColor.blue() - live.blue()) <= 2,
            "Header logo halo RGB must match static Theme.live");
    requireAccessibleIgnored(outer, "headerLogoHaloOuter must be ignored by accessibility");
    requireAccessibleIgnored(inner, "headerLogoHaloInner must be ignored by accessibility");
}

inline void verifyNavigationOrnament(QQuickWindow *window, const char *checkedTabName, const char *uncheckedTabName, QObject *tokens) {
    auto *checkedTab = qobject_cast<QQuickItem *>(named(window, checkedTabName));
    auto *uncheckedTab = qobject_cast<QQuickItem *>(named(window, uncheckedTabName));
    require(checkedTab && checkedTab->property("checked").toBool(), "Expected navigation tab not checked");
    require(uncheckedTab && !uncheckedTab->property("checked").toBool(), "Expected navigation tab still checked");
    require(checkedTab->width() >= 24 && checkedTab->height() >= 24, "NavigationTab hit target below 24px");

    auto *ornament = qobject_cast<QQuickItem *>(named(checkedTab, "navigationActiveOrnament"));
    auto *diamond = qobject_cast<QQuickItem *>(named(checkedTab, "navigationActiveDiamond"));
    auto *halo = qobject_cast<QQuickItem *>(named(checkedTab, "navigationActiveHalo"));
    require(ornament && ornament->isVisible() && ornament->opacity() == 1.0, "Checked tab active ornament missing");
    require(diamond && diamond->isVisible() && qAbs(diamond->rotation() - 45.0) < 0.1, "Active rail brass diamond missing");
    const QColor brass = tokens->property("brass").value<QColor>();
    require(diamond->property("color").value<QColor>() == brass, "Active rail diamond color mismatch");
    require(halo && halo->opacity() == 1.0, "Checked tab icon halo missing");
    requireAccessibleIgnored(ornament, "navigationActiveOrnament must be ignored by accessibility");
    requireAccessibleIgnored(halo, "navigationActiveHalo must be ignored by accessibility");

    auto *inactiveOrnament = qobject_cast<QQuickItem *>(named(uncheckedTab, "navigationActiveOrnament"));
    auto *inactiveHalo = qobject_cast<QQuickItem *>(named(uncheckedTab, "navigationActiveHalo"));
    require(inactiveOrnament && inactiveOrnament->opacity() == 0.0 && !inactiveOrnament->isVisible(), "Unchecked tab retained active ornament");
    require(inactiveHalo && inactiveHalo->opacity() == 0.0, "Unchecked tab retained icon halo");
}

inline int verifyVisibleSectionHeadings(QQuickWindow *window, QObject *tokens, const QStringList &expectedTexts) {
    QStringList seen;
    const QColor brass = tokens->property("brass").value<QColor>();
    const QString displayFamily = tokens->property("displayFamily").toString();
    for (auto *object : objects(window)) {
        if (object->objectName() != "sectionHeadingLabel") continue;
        auto *label = qobject_cast<QQuickItem *>(object);
        if (!label || !label->isVisible()) continue;
        auto *heading = label->parentItem();
        while (heading && !findNamed(heading, "sectionHeadingDiamond")) heading = heading->parentItem();
        require(heading != nullptr, "sectionHeadingLabel missing SectionHeading parent");
        auto *diamond = qobject_cast<QQuickItem *>(named(heading, "sectionHeadingDiamond"));
        auto *rule = qobject_cast<QQuickItem *>(named(heading, "sectionHeadingRule"));
        require(diamond->isVisible() && qAbs(diamond->rotation() - 45.0) < 0.1, "SectionHeading brass diamond missing");
        require(QQmlProperty(diamond, "border.color").read().value<QColor>() == brass, "SectionHeading diamond border is not Theme.brass");
        require(rule->isVisible() && rule->width() >= 16 && qAbs(rule->height() - 1.0) < 0.1, "SectionHeading fading rule missing");
        requireAccessibleIgnored(diamond, "sectionHeadingDiamond must be ignored by accessibility");
        requireAccessibleIgnored(rule, "sectionHeadingRule must be ignored by accessibility");
        require(label->property("textFormat").toInt() == Qt::PlainText, "SectionHeading text must be PlainText");
        if (!displayFamily.isEmpty()) {
            const auto font = label->property("font").value<QFont>();
            require(font.family() == displayFamily, "SectionHeading did not use Theme.displayFamily");
        }
        seen << label->property("text").toString();
    }
    for (const auto &expected : expectedTexts) {
        require(seen.contains(expected), qPrintable("Missing visible SectionHeading: " + expected + " (got: " + seen.join(", ") + ")"));
    }
    return seen.size();
}

inline void writePeerConfig(const QString &dataDir, const QJsonObject &conf) {
    QFile file(QDir(dataDir).filePath("peer-config.json"));
    require(file.open(QIODevice::WriteOnly | QIODevice::Truncate), "Cannot write deco peer-config.json");
    require(file.write(QJsonDocument(conf).toJson(QJsonDocument::Compact)) > 0, "Failed writing deco peer-config.json");
}

inline QJsonObject baseNativePeerConfig() {
    QJsonArray profiles{
        QJsonObject{{"id", "p1"}, {"name", "Default"}, {"isActive", true}, {"modCount", 1}},
        QJsonObject{{"id", "p2"}, {"name", "Extremely Long Modpack Profile Name For Layout Truncation Verification 12345"}, {"isActive", false}, {"modCount", 1}},
        QJsonObject{{"id", "p3"}, {"name", QString::fromUtf8("\xF0\x9F\xA6\x8A Моды")}, {"isActive", false}, {"modCount", 0}},
        QJsonObject{{"id", "p4"}, {"name", ""}, {"isActive", false}, {"modCount", 0}},
    };
    return QJsonObject{
        {"activeProfileId", "p1"},
        {"selectedProfileId", "p1"},
        {"pendingChanges", true},
        {"pendingStatus", "pending"},
        {"recoveryRequired", false},
        {"profiles", profiles}
    };
}

inline void startDecoChromeCheck(QQuickWindow *window, LibraryModel *model, const QString &scenario) {
    const QString themeEnv = qEnvironmentVariable("DECO_THEME", "dark");
    applyDecoPalette(themeEnv != "light");

    if (scenario == "deco-chrome-demo") {
        window->resize(1200, 780);
        auto *timer = new QTimer(window);
        timer->setInterval(80);
        QObject::connect(timer, &QTimer::timeout, window, [window, model, timer, themeEnv, stage = 0, ticks = 0]() mutable {
            try {
                require(++ticks < 150, "deco-chrome-demo timed out");
                if (model->loading() || !window->isVisible()) return;
                auto *tokens = qmlEngine(window)->singletonInstance<QObject *>("DeadlockStyle", "Tokens");
                require(tokens != nullptr, "Tokens singleton missing");

                if (stage == 0) {
                    verifyLogoHalo(window, tokens);
                    auto *chooser = qobject_cast<QQuickItem *>(named(window, "profileChooser"));
                    auto *bg = qvariant_cast<QQuickItem *>(chooser->property("background"));
                    require(chooser->width() >= 24 && chooser->height() >= 24, "profileChooser hit target too small");
                    require(decoContrast(QQmlProperty(bg, "border.color").read().value<QColor>(), bg->property("color").value<QColor>()) >= 3.0, "Profile pill boundary contrast below 3:1");
                    require(qAbs(bg->property("radius").toDouble() - chooser->height() / 2.0) <= 0.6, "profileChooser is not pill-shaped");
                    TextUtils textUtils;
                    require(textUtils.initial(QString::fromUtf8("e\u0301clair")) == QString::fromUtf8("E\u0301"), "Combining initial split");
                    require(textUtils.initial(QString::fromUtf8("👩‍💻 mods")) == QString::fromUtf8("👩‍💻"), "Joined emoji initial split");
                    require(textUtils.initial(QString::fromUtf8("🇪🇪 mods")) == QString::fromUtf8("🇪🇪"), "Flag initial split");
                    require(textUtils.initial("   ") == "P", "Empty initial fallback changed");
                    plain(window, "profileText", "Default");
                    require(named(window, "profileInitialText")->property("text").toString() == "D", "Default initial badge mismatch");
                    auto *badge = qobject_cast<QQuickItem *>(named(window, "profileInitialBadge"));
                    requireAccessibleIgnored(badge, "profileInitialBadge must be ignored by accessibility");
                    auto *manageItem = qobject_cast<QQuickItem *>(named(window, "manageProfilesMenuItem"));
                    require(!manageItem->isEnabled(), "Demo manageProfilesMenuItem must stay disabled");

                    verifyNavigationOrnament(window, "homeTab", "libraryTab", tokens);
                    // Hover opacity transition on inactive tab (no ColorAnimation)
                    auto *libTab = qobject_cast<QQuickItem *>(named(window, "libraryTab"));
                    libTab->setProperty("hoverEnabled", true);
                    QHoverEvent hover(QEvent::HoverEnter, QPointF(20, 20), QPointF(20, 20), QPointF(-1, -1));
                    QCoreApplication::sendEvent(libTab, &hover);
                    require(libTab->property("hovered").toBool(), "NavigationTab hover did not activate");
                    decoSettle(6, 30);
                    libTab->forceActiveFocus(Qt::TabFocusReason);
                    auto *hoverFill = qobject_cast<QQuickItem *>(named(libTab, "navigationHoverFill"));
                    require(libTab->property("visualFocus").toBool() && libTab->property("hovered").toBool(), "Combined rail focus and hover not reached");
                    require(hoverFill->x() >= 2 && hoverFill->y() >= 2
                        && hoverFill->x() + hoverFill->width() <= libTab->width() - 2
                        && hoverFill->y() + hoverFill->height() <= libTab->height() - 2, "Rail hover fill covers focus border");
                    saveDecoCapture(window, "rail-hover-focus-" + themeEnv, 1200, 780);
                    QHoverEvent leave(QEvent::HoverLeave, QPointF(-10, -10), QPointF(-10, -10), QPointF(20, 20));
                    QCoreApplication::sendEvent(libTab, &leave);

                    // Demo rows have no addedAt; the dated native fixture checks all shelves.
                    verifyVisibleSectionHeadings(window, tokens, {"Home"});
                    saveDecoCapture(window, "demo-home-" + themeEnv + "-1200", 1200, 780);
                    saveDecoCapture(window, "demo-home-" + themeEnv + "-720", 720, 780);
                    window->resize(1200, 780);
                    decoSettle(4, 30);

                    const QColor liveBefore = named(window, "headerLogoHaloOuter")->property("color").value<QColor>();
                    realClick(window, "libraryTab");
                    stage = 1;
                    // Verify live halo remains static across navigation
                    require(named(window, "headerLogoHaloOuter")->property("color").value<QColor>() == liveBefore,
                            "Header logo halo changed color on navigation (must be static)");
                    return;
                }
                if (stage == 1) {
                    auto *homeOrn = qobject_cast<QQuickItem *>(named(named(window, "homeTab"), "navigationActiveOrnament"));
                    auto *libOrn = qobject_cast<QQuickItem *>(named(named(window, "libraryTab"), "navigationActiveOrnament"));
                    auto *libHalo = qobject_cast<QQuickItem *>(named(named(window, "libraryTab"), "navigationActiveHalo"));
                    if (homeOrn->opacity() != 0.0 || libOrn->opacity() != 1.0 || libHalo->opacity() != 1.0) return;
                    verifyNavigationOrnament(window, "libraryTab", "homeTab", tokens);
                    auto *browseTab = qobject_cast<QQuickItem *>(named(window, "browseTab"));
                    auto *settingsTab = qobject_cast<QQuickItem *>(named(window, "settingsTab"));
                    browseTab->forceActiveFocus(Qt::TabFocusReason);
                    realKey(window, Qt::Key_Down);
                    require(settingsTab->hasActiveFocus(), "Down from Browse must reach Settings with native tabs hidden");
                    realKey(window, Qt::Key_Up);
                    require(browseTab->hasActiveFocus(), "Up from Settings must skip hidden native tabs");
                    realKey(window, Qt::Key_Down);
                    realKey(window, Qt::Key_Space);
                    auto *accessibleBrowse = QAccessible::queryAccessibleInterface(browseTab);
                    auto *accessibleSettings = QAccessible::queryAccessibleInterface(settingsTab);
                    require(accessibleBrowse && accessibleBrowse->parent() && accessibleSettings && accessibleSettings->role() == QAccessible::PageTab
                        && accessibleSettings->state().selected && accessibleSettings->parent()
                        && accessibleSettings->parent()->role() == QAccessible::PageTabList
                        && accessibleSettings->parent()->object() == accessibleBrowse->parent()->object(),
                        "Settings must be announced as the selected tab in the same navigation list as Browse");
                    stage = 2;
                    return;
                }
                if (stage == 2) {
                    auto *libOrn = qobject_cast<QQuickItem *>(named(named(window, "libraryTab"), "navigationActiveOrnament"));
                    auto *setOrn = qobject_cast<QQuickItem *>(named(named(window, "settingsTab"), "navigationActiveOrnament"));
                    auto *setHalo = qobject_cast<QQuickItem *>(named(named(window, "settingsTab"), "navigationActiveHalo"));
                    if (libOrn->opacity() != 0.0 || setOrn->opacity() != 1.0 || setHalo->opacity() != 1.0) return;
                    verifyNavigationOrnament(window, "settingsTab", "libraryTab", tokens);
                    verifyVisibleSectionHeadings(window, tokens, {"Settings"});
                    saveDecoCapture(window, "demo-settings-" + themeEnv + "-1200", 1200, 780);
                    saveDecoCapture(window, "demo-settings-" + themeEnv + "-720", 720, 780);
                    timer->stop();
                    std::puts("PASS deco-chrome-demo");
                    QCoreApplication::exit(0);
                }
            } catch (const std::exception &error) {
                timer->stop();
                std::fprintf(stderr, "FAIL deco-chrome-demo (stage %d): %s\n", stage, error.what());
                QCoreApplication::exit(1);
            }
        });
        timer->start();
        return;
    }

    if (scenario == "deco-chrome-native") {
        window->resize(1200, 780);
        auto *timer = new QTimer(window);
        timer->setInterval(80);
        QObject::connect(timer, &QTimer::timeout, window, [window, model, timer, stage = 0, ticks = 0]() mutable {
            try {
                require(++ticks < 220, "deco-chrome-native timed out");
                auto *catalog = model->catalog();
                if (model->loading() || catalog->homeLoading() || !window->isVisible()) return;
                if (!model->nativeMode() || model->profiles().isEmpty()) return;

                auto *tokens = qmlEngine(window)->singletonInstance<QObject *>("DeadlockStyle", "Tokens");
                auto *prefs = qmlEngine(window)->singletonInstance<QObject *>("Deadlock.Native", "Preferences");
                auto *themeObj = qmlEngine(window)->singletonInstance<QObject *>("Deadlock.Native", "Theme");
                require(tokens && prefs && themeObj, "Singletons missing");

                auto *playBtn = qobject_cast<QQuickItem *>(named(window, "launchActiveButton"));
                auto *optBtn = qobject_cast<QQuickItem *>(named(window, "launchMenuButton"));
                auto *playBg = qvariant_cast<QQuickItem *>(playBtn->property("background"));
                auto *optBg = qvariant_cast<QQuickItem *>(optBtn->property("background"));
                auto *sheenAnim = named(playBg, "playSheenAnimation");
                auto *sheenBar = qobject_cast<QQuickItem *>(named(playBg, "playSheenBar"));

                auto requireSheenReset = [&]() {
                    require(!sheenAnim->property("running").toBool(), "Sheen animation running when gated off");
                    require(qAbs(sheenBar->x() + sheenBar->width()) <= 0.5, "Sheen bar did not reset off left edge");
                    require(qAbs(sheenBar->opacity()) <= 0.001, "Sheen bar opacity did not reset to 0");
                };

                auto verifyGradientContrast = [&]() {
                    const QColor selText = themeObj->property("selectionText").value<QColor>();
                    const QColor topFill = playBg->property("topFill").value<QColor>();
                    const QColor bottomFill = playBg->property("bottomFill").value<QColor>();
                    const double topRatio = decoContrast(selText, topFill);
                    const double botRatio = decoContrast(selText, bottomFill);
                    require(topRatio >= 4.5 && botRatio >= 4.5, "DecoPlayPlaque gradient contrast below 4.5:1");
                    const auto gradient = sheenBar->property("gradient").value<QJSValue>().toQObject();
                    require(gradient != nullptr, "Sheen gradient missing");
                    int stops = 0;
                    double peakAlpha = 0;
                    for (auto *stop : gradient->children()) {
                        if (!stop->property("color").isValid()) continue;
                        const QColor peak = stop->property("color").value<QColor>();
                        ++stops;
                        peakAlpha = std::max(peakAlpha, double(peak.alphaF()));
                        for (const QColor base : {topFill, bottomFill}) {
                            const double a = peak.alphaF();
                            const QColor composite = QColor::fromRgbF(peak.redF()*a + base.redF()*(1-a), peak.greenF()*a + base.greenF()*(1-a), peak.blueF()*a + base.blueF()*(1-a));
                            require(decoContrast(selText, composite) >= 4.5, "Peak composited Play sheen contrast below 4.5:1");
                        }
                    }
                    require(stops == 3 && peakAlpha > 0, "Sheen contrast check did not inspect a visible peak");
                };

                if (stage == 0) {
                    // Check all Home shelves + headings + Play typography & plaque
                    if (!model->launch()->canLaunch()) {
                        std::puts("WAIT deco-chrome-native: launch settings not ready");
                        return;
                    }
                    verifyLogoHalo(window, tokens);
                    verifyVisibleSectionHeadings(window, tokens, {"Home", "Recently added", "Latest mods", "Most downloaded"});
                    require(named(window, "homePopularQualifier")->property("visible").toBool() == false,
                            "homePopularQualifier visible when catalog complete");

                    require(playBtn->isVisible() && optBtn->isVisible(), "Play/Options split buttons hidden in native mode");
                    require(playBtn->width() >= 24 && playBtn->height() >= 24 && optBtn->width() >= 24 && optBtn->height() >= 24,
                            "Split Play hit targets below 24px");
                    require(playBtn->property("text").toString() == "Play", "Play semantic text changed from 'Play'");
                    const auto playFont = playBtn->property("font").value<QFont>();
                    require(playFont.capitalization() == QFont::AllUppercase, "Play button must use Font.AllUppercase");
                    const QString dispFam = tokens->property("displayFamily").toString();
                    if (!dispFam.isEmpty()) require(playFont.family() == dispFam, "Play button did not use displayFamily");
                    require(playBg->property("chamferLeft").toBool() && !playBg->property("chamferRight").toBool(), "Play chamfer flags wrong");
                    require(!optBg->property("chamferLeft").toBool() && optBg->property("chamferRight").toBool(), "Options chamfer flags wrong");

                    // Contrast on both dark and light
                    verifyGradientContrast();
                    applyDecoPalette(false);
                    decoSettle(2, 20);
                    verifyGradientContrast();
                    saveDecoCapture(window, "native-pending-sheen-light-1200", 1200, 780);
                    saveDecoCapture(window, "native-pending-sheen-light-720", 720, 780);
                    applyDecoPalette(true);
                    decoSettle(2, 20);
                    saveDecoCapture(window, "native-pending-sheen-dark-720", 720, 780);
                    saveDecoCapture(window, "native-pending-sheen-dark-1200", 1200, 780);

                    // Positive runtime sheen assertion: animation must actually advance sheenBar.x!
                    require(sheenAnim->property("running").toBool(), "Sheen did not start for verified pendingChanges");
                    QElapsedTimer animWait;
                    animWait.start();
                    double firstX = sheenBar->x();
                    bool advanced = false;
                    while (animWait.elapsed() < 2500) {
                        decoSettle(1, 30);
                        const double curX = sheenBar->x();
                        if (curX > -sheenBar->width() + 1.0 && qAbs(curX - firstX) > 1.0) {
                            advanced = true;
                            break;
                        }
                        if (curX > -sheenBar->width() + 0.5 && firstX <= -sheenBar->width() + 0.5) {
                            firstX = curX;
                        }
                    }
                    require(advanced, "playSheenBar.x never advanced during positive sheen run");

                    // Negative 1: reduceMotion immediately stops & resets sheen
                    prefs->setProperty("reduceMotion", true);
                    decoSettle(1, 20);
                    requireSheenReset();
                    saveDecoCapture(window, "native-reduced-motion-1200", 1200, 780);
                    prefs->setProperty("reduceMotion", false);
                    decoSettle(2, 25);
                    require(sheenAnim->property("running").toBool(), "Sheen failed to resume after reduceMotion cleared");

                    // Negative 2: hidden window immediately stops & resets sheen
                    window->setVisible(false);
                    decoSettle(1, 20);
                    requireSheenReset();
                    window->setVisible(true);
                    decoSettle(2, 25);

                    // Negative 3 + profile long name / layout overlap test at 1200 and 720
                    model->viewProfile("p2");
                    require(decoWaitFor([&] { return !model->loading() && model->viewedProfileId() == "p2"; }), "Switching to profile p2 timed out");
                    requireSheenReset();
                    auto *chooser = qobject_cast<QQuickItem *>(named(window, "profileChooser"));
                    auto *profileText = qobject_cast<QQuickItem *>(named(window, "profileText"));
                    require(named(window, "profileInitialText")->property("text").toString() == "E", "Long profile initial mismatch");
                    plain(window, "profileText", "Extremely Long Modpack Profile Name For Layout Truncation Verification 12345");
                    for (const int w : {1200, 720}) {
                        window->resize(w, 780);
                        decoSettle(4, 25);
                        require(chooser->width() <= window->width() * 0.28 + 2.0, "profileChooser exceeded 28% width budget");
                        require(profileText->property("truncated").toBool(), "Long profile text failed to elide");
                        const auto chooserRect = chooser->mapRectToScene(chooser->boundingRect());
                        const auto playRect = playBtn->mapRectToScene(playBtn->boundingRect());
                        require(chooserRect.right() <= playRect.left() + 0.5, "profileChooser overlaps Play plaque");
                    }
                    saveDecoCapture(window, "native-long-profile-720", 720, 780);
                    saveDecoCapture(window, "native-long-profile-1200", 1200, 780);

                    // Emoji/non-ASCII and empty profile name initial badge checks
                    model->viewProfile("p3");
                    require(decoWaitFor([&] { return !model->loading() && model->viewedProfileId() == "p3"; }), "Switching to profile p3 timed out");
                    require(named(window, "profileInitialText")->property("text").toString() == QString::fromUtf8("\xF0\x9F\xA6\x8A"),
                            "Non-ASCII emoji initial codePointAt handling failed");
                    model->viewProfile("p4");
                    require(decoWaitFor([&] { return !model->loading() && model->viewedProfileId() == "p4"; }), "Switching to profile p4 timed out");
                    require(named(window, "profileInitialText")->property("text").toString() == "P",
                            "Empty profile initial fallback failed");

                    model->viewProfile("p1");
                    require(decoWaitFor([&] { return !model->loading() && model->viewedProfileId() == "p1"; }), "Switching back to profile p1 timed out");
                    // Negative 4: pendingChanges false -> sheen stopped & reset
                    auto conf = baseNativePeerConfig();
                    conf["pendingChanges"] = false;
                    conf["pendingStatus"] = "current";
                    writePeerConfig(model->dataDir(), conf);
                    model->refresh();
                    stage = 1;
                    return;
                }
                if (stage == 1) {
                    if (model->pendingChanges()) return;
                    requireSheenReset();

                    // Negative 5: externalGameinfoChange (unknown) -> sheen stopped & reset
                    auto conf = baseNativePeerConfig();
                    conf["pendingStatus"] = "unknown";
                    conf["pendingReason"] = "externalGameinfoChange";
                    writePeerConfig(model->dataDir(), conf);
                    model->refresh();
                    stage = 2;
                    return;
                }
                if (stage == 2) {
                    if (!model->externalGameinfoChange()) return;
                    requireSheenReset();

                    // Negative 6: disabled launch button (recoveryRequired) -> sheen stopped & reset + capture
                    auto conf = baseNativePeerConfig();
                    conf["recoveryRequired"] = true;
                    writePeerConfig(model->dataDir(), conf);
                    model->refresh();
                    stage = 3;
                    return;
                }
                if (stage == 3) {
                    if (playBtn->isEnabled()) return;
                    requireSheenReset();
                    saveDecoCapture(window, "native-disabled-play-1200", 1200, 780);

                    // Restore clean active state for navigation + keyboard focus checks
                    auto conf = baseNativePeerConfig();
                    conf["pendingChanges"] = false;
                    conf["pendingStatus"] = "current";
                    writePeerConfig(model->dataDir(), conf);
                    model->refresh();
                    stage = 4;
                    return;
                }
                if (stage == 4) {
                    if (!playBtn->isEnabled() || model->recoveryRequired() || !named(window, "downloadsTab")->property("enabled").toBool()) return;
                    // Real click Downloads & Tweaks tabs to verify their SectionHeadings
                    realClick(window, "downloadsTab");
                    require(decoWaitFor([&] {
                        return named(named(window, "downloadsTab"), "navigationActiveOrnament")->property("opacity").toDouble() == 1.0
                            && named(named(window, "downloadsTab"), "navigationActiveHalo")->property("opacity").toDouble() == 1.0
                            && named(named(window, "homeTab"), "navigationActiveOrnament")->property("opacity").toDouble() == 0.0;
                    }), "Downloads tab ornament settle timed out");
                    verifyNavigationOrnament(window, "downloadsTab", "homeTab", tokens);
                    verifyVisibleSectionHeadings(window, tokens, {"Downloads"});
                    saveDecoSurfaceCaptures(window, "native-downloads");

                    realClick(window, "tweaksTab");
                    require(decoWaitFor([&] {
                        return named(named(window, "tweaksTab"), "navigationActiveOrnament")->property("opacity").toDouble() == 1.0
                            && named(named(window, "tweaksTab"), "navigationActiveHalo")->property("opacity").toDouble() == 1.0
                            && named(named(window, "downloadsTab"), "navigationActiveOrnament")->property("opacity").toDouble() == 0.0;
                    }), "Tweaks tab ornament settle timed out");
                    verifyNavigationOrnament(window, "tweaksTab", "downloadsTab", tokens);
                    verifyVisibleSectionHeadings(window, tokens, {"Tweaks"});
                    saveDecoSurfaceCaptures(window, "native-tweaks");
                    auto *tweaksTab = qobject_cast<QQuickItem *>(named(window, "tweaksTab"));
                    auto *settingsTab = qobject_cast<QQuickItem *>(named(window, "settingsTab"));
                    tweaksTab->forceActiveFocus(Qt::TabFocusReason);
                    realKey(window, Qt::Key_Down);
                    require(settingsTab->hasActiveFocus(), "Down from Tweaks must reach Settings");
                    realKey(window, Qt::Key_Up);
                    require(tweaksTab->hasActiveFocus(), "Up from Settings must reach Tweaks when visible");

                    // Keyboard focus rings on split Play & Options buttons
                    realClick(window, "homeTab");
                    require(decoWaitFor([&] {
                        return named(named(window, "homeTab"), "navigationActiveOrnament")->property("opacity").toDouble() == 1.0;
                    }), "Home tab return timed out");
                    for (const auto *name : {"launchActiveButton", "launchMenuButton"}) {
                        auto *button = qobject_cast<QQuickItem *>(named(window, name));
                        button->forceActiveFocus(Qt::TabFocusReason);
                        decoSettle(2, 20);
                        auto *bg = qvariant_cast<QQuickItem *>(button->property("background"));
                        auto *outer = qobject_cast<QQuickItem *>(named(bg, "playOuterFocusRing"));
                        auto *inner = named(bg, "playFillFocusRing");
                        require(outer->isVisible() && inner->property("visible").toBool(), "Play/Options focus ring hidden");
                        require(outer->x() == -4 && outer->width() == bg->width() + 8, "Play focus ring offset geometry wrong");
                        const double fillRatio = decoContrast(QQmlProperty(inner, "border.color").read().value<QColor>(), tokens->property("accent").value<QColor>());
                        const double outerRatio = decoContrast(tokens->property("focus").value<QColor>(), tokens->property("window").value<QColor>());
                        require(fillRatio >= 3.0 && outerRatio >= 3.0, "Play focus border contrast below 3:1");
                    }
                    playBtn->forceActiveFocus(Qt::TabFocusReason);
                    saveDecoCapture(window, "native-keyboard-focus-1200", 1200, 780);

                    timer->stop();
                    std::puts("PASS deco-chrome-native");
                    QCoreApplication::exit(0);
                }
            } catch (const std::exception &error) {
                timer->stop();
                std::fprintf(stderr, "FAIL deco-chrome-native (stage %d): %s\n", stage, error.what());
                QCoreApplication::exit(1);
            }
        });
        timer->start();
        return;
    }
}

} // namespace
