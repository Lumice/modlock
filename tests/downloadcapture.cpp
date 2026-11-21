#include "downloadcapture.h"
#include "librarymodel.h"
#include "remotecontroller.h"
#include "progressaccessibility.h"
#include <QQmlComponent>
#include <QQmlEngine>
#include <QQuickItem>
#include <QQuickWindow>
#include <QTimer>
#include <QCoreApplication>
#include <QDir>
#include <QImage>
#include <QSet>
#include <cstdio>
#include <memory>
#include <functional>

// The same private fixture access as the model-check executable.
class ModelCheck {
public:
    static void seed(RemoteController *remote, int stage) {
        remote->m_jobs.clear();remote->m_statuses.clear();remote->m_error.clear();
        remote->m_installStages.clear();remote->m_installErrors.clear();remote->m_installMetadata.clear();
        if(stage==0 && qEnvironmentVariableIntValue("DOWNLOADS_ERROR"))remote->m_error=qEnvironmentVariableIntValue("DOWNLOADS_ERROR")==1
            ? "Switch to this download's profile to add it to your library." : "The provider check did not complete. Try again later.";
        const QStringList states=stage==0?(qEnvironmentVariableIntValue("DOWNLOADS_RETAINED")?QStringList{"downloaded"}:QStringList{"transferring","paused","preparing","failed"}):QStringList{"installed","cancelled","abandoned"};
        int n=0;
        for(const auto &state:states) {
            const auto id=QString(31,'a')+QString::number(++n);
            remote->m_jobs.append(QVariantMap{{"jobId",id},{"state",state},{"sequence","1"},{"cleanupPending",false},{"progress",QVariantMap{{"bytesWritten",40},{"total",100}}}});
            if(qEnvironmentVariableIntValue("DOWNLOADS_RETAINED")){remote->m_installStages[id]="stopped";remote->m_installErrors[id]="Select this download's profile and continue manually.";}
            remote->m_installMetadata[id]={{"name",n==1?"Sakuya Izayoi as Paradox — a long download name":"Download "+QString::number(n)}};
        }
        emit remote->changed();
    }
    static void seedTransition(RemoteController *remote, int phase) {
        remote->m_jobs.clear();remote->m_statuses.clear();remote->m_error.clear();
        remote->m_installStages.clear();remote->m_installErrors.clear();remote->m_installMetadata.clear();
        const QString id(32,'a');
        if(phase==0) {
            remote->m_jobs.append(QVariantMap{{"jobId",id},{"state","transferring"},{"sequence","1"},{"cleanupPending",false},{"progress",QVariantMap{{"bytesWritten","40"},{"total","100"}}}});
            remote->m_installStages[id]="download";
            remote->m_installMetadata[id]={{"name","Controlled Transition Mod"}};
        } else if(phase==1) {
            remote->m_jobs.append(QVariantMap{{"jobId",id},{"state","committing"},{"sequence","2"},{"cleanupPending",false}});
            remote->m_installStages[id]="commit";
            remote->m_installMetadata[id]={{"name","Controlled Transition Mod"}};
        } else if(phase==2) {
            remote->m_jobs.append(QVariantMap{{"jobId",id},{"state","installed"},{"sequence","3"},{"cleanupPending",true}});
            remote->m_installStages[id]="done";
            remote->m_installMetadata[id]={{"name","Controlled Transition Mod"}};
        } else {
            const QString id2(32,'b');
            remote->m_jobs.append(QVariantMap{{"jobId",id},{"state","resolving"},{"sequence","4"},{"cleanupPending",false}});
            remote->m_installStages[id]="download";
            remote->m_installMetadata[id]={{"name","Unknown Transfer Size Mod"}};
            remote->m_jobs.append(QVariantMap{{"jobId",id2},{"state","paused"},{"sequence","5"},{"cleanupPending",false}});
            remote->m_installStages[id2]="stopped";
            remote->m_installMetadata[id2]={{"name","Normal Paused Mod"}};
        }
        emit remote->changed();
    }
};
static QList<QQuickItem *> findItems(QQuickItem *root, const QString &name) {
    QList<QQuickItem *> found;
    if(!root) return found;
    if(root->objectName()==name) found.append(root);
    for(auto *child:root->childItems()) found.append(findItems(child, name));
    return found;
}
static bool hasActiveSweep(QObject *root) {
    if(!root) return false;
    if(root->objectName()=="downloadTicketSweep" || root->objectName()=="downloadBubbleSweep") {
        auto *item=qobject_cast<QQuickItem *>(root);
        if(!item || item->isVisible()) return true;
    }
    // QML animations inherit QObject, not QAbstractAnimation. The settled
    // progress fixture permits BusyIndicator's RotationAnimator, but no running
    // NumberAnimation (including an unnamed implicit "on x" animation).
    if(root->inherits("QQuickNumberAnimation") && root->property("running").toBool()
        && !root->property("paused").toBool()) return true;
    for(auto *child:root->children()) if(hasActiveSweep(child)) return true;
    return false;
}
void startDownloadCapture(QQuickWindow *window, LibraryModel *model) {
    QQmlComponent sweepControl(qmlEngine(window));
    sweepControl.setData(R"(import QtQuick
Item {
    id: sweepFixture
    property bool animate: true
    Rectangle {
        NumberAnimation on x { from: 0; to: 100; duration: 1000; loops: Animation.Infinite; running: sweepFixture.animate }
    }
})", QUrl());
    std::unique_ptr<QObject> sweep(sweepControl.create());
    if(!sweep || !hasActiveSweep(sweep.get())) {
        std::fprintf(stderr,"Sweep detector missed an unnamed running QML NumberAnimation\n");
        QTimer::singleShot(0,window,[]{QCoreApplication::exit(1);});return;
    }
    sweep->setProperty("animate",false);
    if(hasActiveSweep(sweep.get())) {
        std::fprintf(stderr,"Sweep detector reported a stopped QML NumberAnimation\n");
        QTimer::singleShot(0,window,[]{QCoreApplication::exit(1);});return;
    }
    std::puts("PASS sweep detector positive/negative QML controls");
    auto *component=new QQmlComponent(qmlEngine(window),window);
    component->setData(R"(import QtQuick
import Deadlock.Native
Rectangle {
    required property LibraryModel library
    property bool showBubbles: false
    color: Theme.window
    DownloadHistory {
        objectName: "captureHistory"
        anchors.left: parent.left; anchors.top: parent.top; anchors.bottom: parent.bottom; anchors.right: parent.right
        anchors.margins: 16
        anchors.rightMargin: parent.showBubbles ? 352 : 16
        library: parent.library
    }
    DownloadBubbles {
        objectName: "captureBubbles"
        visible: parent.showBubbles && activeCount > 0
        anchors.right: parent.right; anchors.bottom: parent.bottom; anchors.margins: 16
        library: parent.library
    }
})",QUrl("qrc:/qt/qml/Deadlock/Native/qml/DownloadCapture.qml"));
    auto *item=qobject_cast<QQuickItem *>(component->createWithInitialProperties({{"library",QVariant::fromValue(model)}}));
    if(!item){qWarning()<<component->errors();QCoreApplication::exit(1);return;}
    item->setParentItem(window->contentItem());item->setZ(100000);
    window->setMinimumSize({320,400});
    auto *history=findItems(item,"captureHistory").value(0);
    auto *bubbles=findItems(item,"captureBubbles").value(0);
    auto *timer=new QTimer(window);timer->setInterval(400);
    auto step=std::make_shared<int>(0);
    const bool runTransition=!qEnvironmentVariableIntValue("DOWNLOADS_RETAINED") && !qEnvironmentVariableIntValue("DOWNLOADS_ERROR");
    QObject::connect(timer,&QTimer::timeout,window,[=]{
        auto fail=[&](const QString &msg){
            std::fprintf(stderr,"%s\n",qPrintable(msg));
            QCoreApplication::exit(1);timer->stop();
        };
        if(*step==0 || *step==4) {
            ModelCheck::seed(model->remote(),*step==0?0:1);
            if(*step==4 && !model->remote()->jobs().isEmpty()){return fail("Finished stage jobs should filter out");}
        }
        if(*step>=8 && (*step % 2 == 0)) {
            item->setProperty("showBubbles",true);
            ModelCheck::seedTransition(model->remote(),(*step-8)/2);
            const QSize size(960,520);
            window->resize(size);item->setSize(size);++*step;return;
        }
        if(*step<8 && (*step % 2 == 0)) {
            const QSize size=(*step==2 || *step==6)?QSize(960,760):QSize(420,760);
            window->resize(size);item->setSize(size);++*step;return;
        }
        if(*step==1) {
            QSet<QString> actions;
            int progressControls=0;
            QString progressError;
            for(auto *object:findItems(history,"downloadProgress")) {
                if(object->isVisible()) {
                    ++progressControls;
                    if(progressError.isEmpty())progressAccessibilityMatches(object,progressError);
                }
            }
            std::function<void(QQuickItem *)> visit=[&](QQuickItem *object) {
                if(object->objectName().startsWith("downloadAction-"))actions.insert(object->objectName().mid(15));
                for(auto *child:object->childItems())visit(child);
            };
            visit(history);
            if(progressControls==0 || !progressError.isEmpty()){
                return fail("Accessible download progress: "+progressError);
            }
            if(actions!=(qEnvironmentVariableIntValue("DOWNLOADS_RETAINED")?QSet<QString>{"install","cancel"}:QSet<QString>{"pause","resume","cancel","retry","dismiss"})){
                return fail("Downloads actions: "+QStringList(actions.values()).join(","));
            }
        }
        if(*step<8 && qEnvironmentVariableIntValue("DOWNLOADS_ERROR")) {
            QQuickItem *error=findItems(history,"downloadsError").value(0);
            const bool expected=*step<4;
            const auto text=qEnvironmentVariableIntValue("DOWNLOADS_ERROR")==1?QString("Switch to this download's profile to add it to your library."):QString("The provider check did not complete. Try again later.");
            if(!error || error->isVisible()!=expected || (expected && (error->property("text")!=text || error->property("textFormat").toInt()!=int(Qt::PlainText)))) {
                return fail("Downloads continuation error is missing or incorrectly displayed");
            }
        }
        if(*step<8) {
            auto *empty = findItems(history, "downloadsEmpty").value(0);
            if (!empty || empty->isVisible() != (*step >= 4)
                    || (*step >= 4 && empty->property("headline").toString() != "Nothing downloading")) {
                return fail("Downloads history did not show the appropriate empty state");
            }
            const QStringList names={"downloads-active-narrow","downloads-active-wide","downloads-empty-narrow","downloads-empty-wide"};
            const auto file=qEnvironmentVariable("UI_NATIVE_CAPTURES")+"/"+names[*step/2]+".png";
            if(!window->grabWindow().save(file)) return fail("Cannot save download capture "+file);
            ++*step;
            if(*step==8 && !runTransition){timer->stop();std::puts("PASS UI downloads active and finished filtering, narrow and wide");QCoreApplication::exit(0);}
            return;
        }
        const int phase=(*step-8)/2;
        QString err;
        auto checkProgress=[&](QQuickItem *scope, const QString &who, int idx, bool expectVisible, bool expectIndet, double expectValue) {
            const auto bars=findItems(scope,"downloadProgress");
            if(idx>=bars.size()){fail(who+" missing downloadProgress "+QString::number(idx));return false;}
            auto *bar=bars[idx];
            if(bar->isVisible()!=expectVisible){fail(who+" downloadProgress visibility mismatch");return false;}
            if(expectVisible) {
                if(bar->property("indeterminate").toBool()!=expectIndet){fail(who+" indeterminate mismatch");return false;}
                if(qAbs(bar->property("value").toDouble()-expectValue)>0.001){fail(who+" progress value mismatch: "+QString::number(bar->property("value").toDouble()));return false;}
                if(!progressAccessibilityMatches(bar,err)){fail(who+" accessibility: "+err);return false;}
            }
            return true;
        };
        if(phase==0) {
            if(!checkProgress(history,"history transferring",0,true,false,0.4)) return;
            if(!checkProgress(bubbles,"bubbles transferring",0,true,false,0.4)) return;
            auto *hp=findItems(history,"downloadPercent").value(0);
            auto *bp=findItems(bubbles,"downloadBubblePercent").value(0);
            if(!hp || !hp->isVisible() || hp->property("text")!="40%") return fail("history transferring percent mismatch");
            if(!bp || !bp->isVisible() || bp->property("text")!="40%") return fail("bubbles transferring percent mismatch");
            if(hasActiveSweep(history) || hasActiveSweep(bubbles)) return fail("sweep animation active during measured transferring");
        } else if(phase==1) {
            if(!checkProgress(history,"history post-transfer",0,true,false,1.0)) return;
            if(!checkProgress(bubbles,"bubbles post-transfer",0,true,false,1.0)) return;
            auto *hp=findItems(history,"downloadPercent").value(0);
            auto *bp=findItems(bubbles,"downloadBubblePercent").value(0);
            auto *hs=findItems(history,"downloadStatus").value(0);
            auto *bm=findItems(bubbles,"downloadBubbleMessage").value(0);
            auto *hb=findItems(history,"downloadBusyIndicator").value(0);
            auto *bb=findItems(bubbles,"downloadBusyIndicator").value(0);
            if(hp && hp->isVisible()) return fail("history post-transfer showed false ongoing percentage");
            if(bp && bp->isVisible()) return fail("bubbles post-transfer showed false ongoing percentage");
            if(!hs || !hs->property("text").toString().contains("Adding to library") || hs->property("text").toString().contains('%')) return fail("history post-transfer status wrong: "+hs->property("text").toString());
            if(!bm || !bm->property("text").toString().contains("Adding to library") || bm->property("text").toString().contains('%')) return fail("bubbles post-transfer status wrong: "+bm->property("text").toString());
            if(hasActiveSweep(history) || hasActiveSweep(bubbles)) return fail("misleading progress sweep active in post-transfer");
            if(!hb || !hb->isVisible() || !bb || !bb->isVisible()) return fail("post-transfer busy indicator missing");
        } else if(phase==2) {
            if(!checkProgress(history,"history installed cleanup",0,true,false,1.0)) return;
            auto *hp=findItems(history,"downloadPercent").value(0);
            auto *hs=findItems(history,"downloadStatus").value(0);
            auto *hb=findItems(history,"downloadBusyIndicator").value(0);
            if(hp && hp->isVisible()) return fail("installed cleanup showed false percentage");
            if(!hs || hs->property("text")!="Finishing up…") return fail("installed cleanup status wrong");
            if(hasActiveSweep(history)) return fail("misleading progress sweep active during installed cleanup");
            if(!hb || !hb->isVisible()) return fail("installed cleanup busy indicator missing");
            if(bubbles && bubbles->isVisible()) return fail("installed cleanup should stay out of bubbles");
        } else if(phase==3) {
            if(!checkProgress(history,"history resolving",0,true,true,0.0)) return;
            if(!checkProgress(history,"history paused",1,false,false,0.0)) return;
            if(!checkProgress(bubbles,"bubbles resolving",0,true,true,0.0)) return;
            if(!checkProgress(bubbles,"bubbles paused",1,false,false,0.0)) return;
            const auto hPercents=findItems(history,"downloadPercent");
            const auto bPercents=findItems(bubbles,"downloadBubblePercent");
            for(auto *p:hPercents) if(p->isVisible()) return fail("history resolving/paused showed false percentage");
            for(auto *p:bPercents) if(p->isVisible()) return fail("bubbles resolving/paused showed false percentage");
            if(hasActiveSweep(history) || hasActiveSweep(bubbles)) return fail("misleading progress sweep active during unknown work");
            const auto hBusy=findItems(history,"downloadBusyIndicator");
            const auto bBusy=findItems(bubbles,"downloadBusyIndicator");
            if(hBusy.size()<2 || !hBusy[0]->isVisible() || hBusy[1]->isVisible()) return fail("history resolving/paused busy indicator state wrong");
            if(bBusy.size()<2 || !bBusy[0]->isVisible() || bBusy[1]->isVisible()) return fail("bubbles resolving/paused busy indicator state wrong");
            const auto hStatus=findItems(history,"downloadStatus");
            const auto bMsgs=findItems(bubbles,"downloadBubbleMessage");
            if(hStatus.size()<2 || hStatus[0]->property("text")!="Downloading…" || hStatus[1]->property("text")!="Paused") return fail("history resolving/paused status wrong");
            if(bMsgs.size()<2 || bMsgs[0]->property("text")!="Downloading…" || bMsgs[1]->property("text")!="Paused") return fail("bubbles resolving/paused status wrong");
        }
        const QStringList tNames={"downloads-transition-transferring","downloads-transition-post-transfer","downloads-transition-cleanup","downloads-transition-unknown-paused"};
        const auto file=qEnvironmentVariable("UI_NATIVE_CAPTURES")+"/"+tNames[phase]+".png";
        if(!window->grabWindow().save(file)) return fail("Cannot save transition capture "+file);
        ++*step;
        if(*step==16){timer->stop();std::puts("PASS UI downloads active and finished filtering, narrow and wide");QCoreApplication::exit(0);}
    });timer->start();
}
