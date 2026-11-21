#pragma once
#include <QQmlExpression>

// Real MediaCards and LibraryModel, with only catalog storage replaced by a
// controlled wire peer. A retiring card must not cancel another live owner.
static void startMediaDemandCheck(QQuickWindow *window) {
    auto *model=new LibraryModel(window);
    model->loadNative(qEnvironmentVariable("UI_DEMAND_HELPER"),qEnvironmentVariable("UI_DEMAND_ROOT1"));
    struct State {
        int stage=0, ticks=0, loadingSamples=0;
        QElapsedTimer step;
        QPointer<QObject> probe;
    };
    const auto state=std::make_shared<State>();
    auto *timer=new QTimer(window); timer->setInterval(25);
    QObject::connect(timer,&QTimer::timeout,window,[=] {
        try {
            require(++state->ticks<500,"media demand guard timed out");
            const auto calls=[&](const QString &method) {
                QFile f(model->dataDir()+"/trace.jsonl"); int count=0;
                if(f.open(QIODevice::ReadOnly))while(!f.atEnd())
                    if(QJsonDocument::fromJson(f.readLine()).object()["method"].toString()==method)++count;
                return count;
            };
            const auto script=[&](const QString &code) {
                QQmlExpression e(qmlContext(state->probe),state->probe,code);
                const auto result=e.evaluate(); require(!e.hasError(),qPrintable(e.error().toString())); return result;
            };
            if(state->probe) {
                require(script("hiddenCard.preview.masked && !hiddenCard.preview.ready && hiddenCard.preview.source.toString() !== ''").toBool(),"hidden card lost its mask");
                auto *hidden=state->probe->property("hiddenCard").value<QObject *>();
                auto *image=named(hidden,"previewImage");
                require(image && image->property("source").toUrl().isEmpty() && image->property("status").toInt()==0,"hidden card requested/decoded pixels during refresh");
                if(model->loading())++state->loadingSamples;
            }
            if(state->stage==0) {
                if(model->loading())return;
                require(model->nativeMode() && model->count()==2,"media demand fixture missing");
                model->setSelectedId("anchor");
                model->setLibraryMediaActive(true);
                QQmlComponent component(qmlEngine(window));
                component.setData(R"(import QtQuick
import Deadlock.Native
Item {
    id: probe
    required property var library
    required property string identity
    required property string image
    property var first
    property var second
    property var hiddenCard: hidden
    width: 1100; height: 600; z: 100000
    QtObject {
        id: host
        property string scope: "library"
        property string presentationMode: "list"
        property real width: 500
        property real cellWidth: 500
        property real listHeight: 100
        property bool ready: !probe.library.loading
        property bool modelCoherent: ready
        property int serial: 0
        function issueToken() { return "demand-guard:" + (++serial) }
        function inRange(item) { return true }
    }
    Component {
        id: card
        MediaCard {
            index: 1; anchorIdentity: probe.identity; heading: "Demand guard"
            resultHost: host
            ownerSnapshot: ({identity: anchorIdentity, source: probe.image, hidden: false})
            mediaDemandOwner: probe.library
            onMediaDemandRequested: (consumer, identity, wanted) => probe.library.demandModMedia(consumer, identity, wanted)
        }
    }
    MediaCard {
        id: hidden
        index: 0; anchorIdentity: "hidden"; heading: "Hidden"
        y: 300; resultHost: host
        ownerSnapshot: ({identity: "hidden", source: probe.image, hidden: true})
    }
    function addCard() { return card.createObject(probe) }
    Component.onCompleted: { first=addCard(); second=addCard() }
})",QUrl("qrc:/media-demand-guard.qml"));
                require(component.isReady(),qPrintable(component.errorString()));
                state->probe=component.createWithInitialProperties({{"library",QVariant::fromValue(model)},
                    {"identity",qEnvironmentVariable("UI_DEMAND_ID")},{"image",qEnvironmentVariable("UI_PREVIEW_FIRST")}});
                require(state->probe,qPrintable(component.errorString()));
                auto *item=qobject_cast<QQuickItem *>(state->probe.data());
                item->setParent(window); item->setParentItem(window->contentItem());
                state->stage=1;state->step.start();return;
            }
            if(state->stage==1) {
                if(state->step.elapsed()>2000)require(calls("native.catalog.begin")==1,"visible linked card did not request a gallery");
                if(!script("first.preview.ready && second.preview.ready").toBool() || calls("native.catalog.begin")!=1)return;
                require(script("first.preview.contextKey").toString().toUtf8().size()<=1024
                        && script("second.preview.contextKey").toString().toUtf8().size()<=1024,"long identity exceeded preview ownership bound");
                script("first.visible=false");state->stage=2;state->step.restart();return;
            }
            if(state->stage==2 && state->step.elapsed()>150) {
                require(calls("native.catalog.cancel")==0,"retiring card cancelled overlapping owner's gallery");
                require(script("second.mediaActive").toBool(),"overlapping card was not active");
                script("first.destroy(); first=null");
                QFile release(model->dataDir()+"/complete-1000");require(release.open(QIODevice::WriteOnly),"cannot release gallery");
                release.write("https://images.gamebanana.com/img/ss/mods/current.jpg");release.close();
                state->stage=3;return;
            }
            if(state->stage==3) {
                if(model->modDetails(qEnvironmentVariable("UI_DEMAND_ID"))["images"].toStringList()!=QStringList{"https://images.gamebanana.com/img/ss/mods/current.jpg"})return;
                require(model->modDetails(qEnvironmentVariable("UI_DEMAND_ID"))["previewHidden"].toBool(),"gallery lost mature mask");
                model->refresh();require(model->loading(),"refresh did not enter loading");
                state->stage=4;return;
            }
            if(state->stage==4) {
                if(model->loading())return;
                require(state->loadingSamples>0,"hidden mask was not sampled during loading");
                model->loadNative(qEnvironmentVariable("UI_DEMAND_HELPER"),qEnvironmentVariable("UI_DEMAND_ROOT2"));
                state->stage=5;return;
            }
            if(state->stage==5) {
                if(model->loading() || model->dataDir()!=qEnvironmentVariable("UI_DEMAND_ROOT2"))return;
                script("first=addCard()");state->stage=6;state->step.restart();return;
            }
            if(state->stage==6 && state->step.elapsed()>150) {
                if(calls("native.catalog.begin")!=1)return;
                script("second.visible=false; second.destroy(); second=null");
                state->stage=7;state->step.restart();return;
            }
            if(state->stage==7 && state->step.elapsed()>150) {
                require(calls("native.catalog.cancel")==0,"late old card cancelled same-ID demand in new root");
                script("first.anchorIdentity='anchor'");state->stage=8;return;
            }
            if(state->stage==8 && calls("native.catalog.cancel")==1) {
                script("first.anchorIdentity=identity");state->stage=9;return;
            }
            if(state->stage==9 && calls("native.catalog.begin")==2) {
                script("first.visible=false");state->stage=10;return;
            }
            if(state->stage==10 && calls("native.catalog.cancel")==2) {
                timer->stop();std::printf("PASS media demand: overlap, long identity, refresh mask (%d samples), same-ID roots, recycling, final withdrawal\n",state->loadingSamples);
                QCoreApplication::exit(0);
            }
        } catch(const std::exception &e) {
            timer->stop();std::fprintf(stderr,"FAIL media demand stage %d: %s\n",state->stage,e.what());QCoreApplication::exit(1);
        }
    });
    timer->start();
}
