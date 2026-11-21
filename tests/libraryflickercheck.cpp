#include "libraryflickercheck.h"
#include "librarymodel.h"
#include <QAbstractItemModel>
#include <QChildEvent>
#include <QCoreApplication>
#include <QElapsedTimer>
#include <QJsonDocument>
#include <QJsonObject>
#include <QPointer>
#include <QQmlContext>
#include <QQmlExpression>
#include <QQuickItem>
#include <QQuickWindow>
#include <QSet>
#include <QTimer>
#include <cmath>

namespace {
struct Signals {
    int reset = 0, layout = 0, removed = 0, inserted = 0, changed = 0;
    QJsonObject json() const { return {{"modelReset",reset},{"layoutChanged",layout},{"rowsRemoved",removed},{"rowsInserted",inserted},{"dataChanged",changed}}; }
    bool stable() const { return !reset && !layout && !removed && !inserted; }
};
class FlickerCheck final : public QObject {
public:
    FlickerCheck(QQuickWindow *window, LibraryModel *model) : QObject(window), m_window(window), m_model(model) {
        observe(model, m_proxy); observe(model->sourceModel(), m_rows);
        qApp->installEventFilter(this);
        m_timer.setInterval(25);
        connect(&m_timer,&QTimer::timeout,this,[this]{ tick(); });
        m_total.start(); m_step.start(); m_timer.start();
        window->resize(1200,780);
    }
private:
    QQuickWindow *m_window;
    LibraryModel *m_model;
    QTimer m_timer;
    QElapsedTimer m_total, m_step;
    Signals m_proxy, m_rows;
    QSet<QObject *> m_hooked, m_delegates, m_watchedImages;
    QList<QPointer<QObject>> m_savedDelegates;
    int m_created=0, m_destroyed=0, m_previewResets=0, m_phase=0, m_mode=0;
    int m_previewSources=0, m_readyDepartures=0;
    bool m_failed=false;
    QString m_selected;
    qreal m_scroll=0;
    void observe(QAbstractItemModel *model, Signals &counts) {
        connect(model,&QAbstractItemModel::modelReset,this,[&counts]{++counts.reset;});
        connect(model,&QAbstractItemModel::layoutChanged,this,[&counts]{++counts.layout;});
        connect(model,&QAbstractItemModel::rowsRemoved,this,[&counts]{++counts.removed;});
        connect(model,&QAbstractItemModel::rowsInserted,this,[&counts]{++counts.inserted;});
        connect(model,&QAbstractItemModel::dataChanged,this,[&counts]{++counts.changed;});
    }
    void track(QObject *object) {
        if(object->objectName()!="modRow")return;
        auto *preview=object->property("preview").value<QObject *>();
        auto *image=preview?preview->findChild<QObject *>("previewImage"):nullptr;
        if(image && !m_watchedImages.contains(image)) {
            m_watchedImages.insert(image);
            auto *source=new QQmlExpression(qmlContext(image),image,"source",image);
            source->setNotifyOnValueChanged(true);source->evaluate();
            connect(source,&QQmlExpression::valueChanged,this,[this]{++m_previewSources;});
            auto *status=new QQmlExpression(qmlContext(image),image,"status",image);
            status->setNotifyOnValueChanged(true);
            const int initial=status->evaluate().toInt();
            connect(status,&QQmlExpression::valueChanged,this,[this,status,previous=initial]() mutable {
                const int current=status->evaluate().toInt();
                if(previous==1 && current!=1)++m_readyDepartures;
                previous=current;
            });
            connect(image,&QObject::destroyed,this,[this,image]{m_watchedImages.remove(image);});
        }
        if(m_delegates.contains(object))return;
        m_delegates.insert(object); ++m_created;
        if(preview) {
            auto *watch=new QQmlExpression(qmlContext(preview),preview,"requestRevision",object);
            watch->setNotifyOnValueChanged(true);watch->evaluate();
            connect(watch,&QQmlExpression::valueChanged,this,[this]{++m_previewResets;});
        }
        connect(object,&QObject::destroyed,this,[this,object]{m_delegates.remove(object);++m_destroyed;});
    }
    bool eventFilter(QObject *, QEvent *event) override {
        if(event->type()==QEvent::ChildAdded) {
            auto *child=static_cast<QChildEvent *>(event)->child();
            if(!m_hooked.contains(child)) {
                m_hooked.insert(child);
                connect(child,&QObject::objectNameChanged,this,[this,child]{track(child);});
                connect(child,&QObject::destroyed,this,[this,child]{m_hooked.remove(child);});
                track(child);
            }
        }
        return false;
    }
    void scan(QQuickItem *item) {
        track(item);
        for(auto *child:item->childItems())scan(child);
    }
    void reset() {
        m_proxy={};m_rows={};m_created=0;m_destroyed=0;m_previewResets=0;m_previewSources=0;m_readyDepartures=0;m_step.restart();
        m_savedDelegates.clear();for(auto *object:m_delegates)m_savedDelegates.append(object);
        m_selected=m_model->selectedId();m_scroll=m_model->viewScrollPosition();
    }
    void record(const char *phase) {
        bool pointers=true;for(const auto &object:m_savedDelegates)if(!object)pointers=false;
        const bool position=m_model->selectedId()==m_selected && std::abs(m_model->viewScrollPosition()-m_scroll)<1;
        const QJsonObject result{{"mode",m_mode?"grid":"list"},{"phase",phase},{"elapsedMs",m_step.elapsed()},
            {"proxy",m_proxy.json()},{"rows",m_rows.json()},{"delegateCreated",m_created},{"delegateDestroyed",m_destroyed},
            {"previewRequestChanges",m_previewResets},{"previewSourceChanges",m_previewSources},
            {"previewReadyDepartures",m_readyDepartures},{"watchedImages",m_watchedImages.size()},
            {"liveDelegates",m_delegates.size()},{"delegatePointersStable",pointers},{"selectionAndScrollStable",position},
            {"scrollBefore",m_scroll},{"scrollAfter",m_model->viewScrollPosition()},
            {"selectedBefore",m_selected},{"selectedAfter",m_model->selectedId()}};
        qInfo().noquote()<<"FLICKER"<<QJsonDocument(result).toJson(QJsonDocument::Compact);
        if(!m_proxy.stable() || !m_rows.stable() || m_created || m_destroyed || !pointers || !position
                || (QString::fromLatin1(phase)!="idle" && (m_previewResets || m_previewSources || m_readyDepartures))
                || m_watchedImages.isEmpty())m_failed=true;
    }
    void finish(const char *error=nullptr) {
        m_timer.stop();qApp->removeEventFilter(this);
        if(error)qCritical("FLICKER failure: %s",error);
        else qInfo("FLICKER regression %s",m_failed?"FAIL":"PASS");
        QCoreApplication::exit(error || m_failed?1:0);
    }
    bool checkViewer(const QString &script) {
        for (auto *card : m_delegates) {
            if (card->property("anchorIdentity").toString()!=m_model->selectedId()) continue;
            QQmlExpression expression(qmlContext(card), card, script);
            const bool passed=expression.evaluate().toBool() && !expression.hasError();
            qInfo()<<"FLICKER viewer regression"<<script<<passed<<expression.error();
            if (!passed) m_failed=true;
            return passed;
        }
        m_failed=true;
        return false;
    }
    void tick() {
        m_window->requestUpdate();
        scan(m_window->contentItem());
        if(m_total.elapsed()>105000){finish("timed out");return;}
        if(m_phase==0) {
            if(m_model->loading() || !m_model->nativeMode() || m_model->updatesChecking() || !m_model->lastUpdateCheck().isValid())return;
            if(m_model->count()!=7){finish("fixture did not load seven linked mods");return;}
            QMetaObject::invokeMethod(m_window,"screenshotSetup",Q_ARG(QVariant,QStringLiteral("library")));
            m_phase=1;m_step.restart();
        } else if(m_phase==1 && m_step.elapsed()>1800) {
            scan(m_window->contentItem());
            if(m_delegates.isEmpty()){finish("Library delegates not found");return;}
            m_model->setSelectedId(m_model->idAt(2));
            auto *results=m_window->findChild<QObject *>("modList");
            auto *view=results?results->property("view").value<QObject *>():nullptr;
            if(!view){finish("Library result view not found");return;}
            view->setProperty("contentY",80.0);
            m_phase=2;m_step.restart();
        } else if(m_phase==2 && m_step.elapsed()>400) {
            reset();m_phase=3;
        } else if(m_phase==3 && m_step.elapsed()>2200) {
            record("idle");reset();
            if(!m_model->checkForUpdates()){finish("update check refused");return;}
            m_phase=4;
        } else if(m_phase==4 && !m_model->updatesChecking() && m_step.elapsed()>250) {
            if(m_model->updateCount()!=1){finish("changed update badge was not applied");return;}
            record("updates");reset();m_model->refresh();
            checkViewer("(function() { openLarger(); return !resultHost.viewerOpen && !interactable && !preview.enabled; })()");
            m_phase=5;
        } else if(m_phase==5 && !m_model->loading() && m_step.elapsed()>1000) {
            record("identical-snapshot");
            checkViewer("(function() { openLarger(); const opened = resultHost.viewerOpen; Qt.callLater(() => resultHost.closeViewer()); return opened; })()");
            if(!m_mode) {
                m_mode=1;
                QMetaObject::invokeMethod(m_window,"screenshotSetup",Q_ARG(QVariant,QStringLiteral("grid")));
                m_phase=1;m_step.restart();
            } else { m_model->refresh(); m_phase=6; m_step.restart(); }
        } else if(m_phase==6 && !m_model->loading() && m_step.elapsed()>2000) {
            finish();
        }
    }
};
}
void startLibraryFlickerCheck(QQuickWindow *window, LibraryModel *model) { new FlickerCheck(window,model); }
