#pragma once
#include "catalogcontroller.h"
#include <QPointer>
#include <QScopeGuard>

// Empty, native-owned catalog reads must advance on admission replies rather
// than pay two periodic polling intervals for every serialized library row.
static void checkCatalogCacheScheduling(const QString &helper) {
    QTemporaryDir fixture;
    QVERIFY(fixture.isValid());
    const auto root = fixture.filePath("library");
    QProcess initialize;
    initialize.start(helper, {});
    QVERIFY(initialize.waitForStarted());
    initialize.write(QJsonDocument(QJsonObject{{"jsonrpc", "2.0"}, {"id", 1},
        {"method", "native.library.initialize"}, {"params", QJsonObject{
            {"version", 1}, {"dataDir", root}, {"operationId", QString(32, 'c')}, {"expectedRevision", "0"}}}}).toJson(QJsonDocument::Compact) + '\n');
    initialize.closeWriteChannel();
    QVERIFY(initialize.waitForFinished());
    QVERIFY(!QJsonDocument::fromJson(initialize.readAllStandardOutput()).object().contains("error"));
    BackendClient client;
    CatalogController catalog(&client);
    client.start(helper, {}, root, true);
    QTRY_VERIFY_WITH_TIMEOUT(catalog.available(), 5000);
    int completed = 0;
    QObject::connect(&catalog, &CatalogController::libraryMediaReady, &catalog,
        [&](const QString &, const QString &, const CatalogDecodedResult &result) {
            if (result.valid && result.phase == "succeeded") ++completed;
        });
    QElapsedTimer elapsed;
    elapsed.start();
    for (int i = 0; i < 9; ++i) {
        const QString key = QStringLiteral("cache-read-%1").arg(i);
        const SubmissionKey submission{QStringLiteral("mod"), QString::number(1000 + i)};
        bool admitted = false;
        QTRY_VERIFY_WITH_TIMEOUT(admitted || (admitted = catalog.requestLibraryMedia(key, submission, false)), 5000);
        QTRY_VERIFY_WITH_TIMEOUT(completed == i + 1, 5000);
    }
    QVERIFY2(elapsed.elapsed() < 10000, "Nine cached metadata reads paid serialized polling intervals");
    catalog.shutdown();
    client.stop();
    QTRY_VERIFY_WITH_TIMEOUT(!client.retiring(), 5000);
}


// A controlled wire peer preserves real helper identities and validation, but
// holds receipt status at running. No provider operation is sent to the helper.
static void checkCatalogPollCadence(const QString &helper, const QString &mode) {
    QTemporaryDir fixture;
    QVERIFY(fixture.isValid());
    const auto root = fixture.filePath("library");
    QProcess initialize;
    initialize.start(helper, {});
    QVERIFY(initialize.waitForStarted());
    initialize.write(QJsonDocument(QJsonObject{{"jsonrpc", "2.0"}, {"id", 1},
        {"method", "native.library.initialize"}, {"params", QJsonObject{
            {"version", 1}, {"dataDir", root}, {"operationId", QString(32, 'c')}, {"expectedRevision", "0"}}}}).toJson(QJsonDocument::Compact) + '\n');
    initialize.closeWriteChannel();
    QVERIFY(initialize.waitForFinished());
    const auto script = fixture.filePath("peer.py");
    QFile peer(script); QVERIFY(peer.open(QIODevice::WriteOnly));
    peer.write("#!/usr/bin/python3\nimport json\nhelper, trace, mode = " + QJsonDocument(QJsonArray{helper, fixture.filePath("trace.jsonl"), mode}).toJson(QJsonDocument::Compact) + "\n");
    peer.write(R"PY(import sys, subprocess, json, time
child = subprocess.Popen([helper], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
receipts = {}
for line in sys.stdin:
    req = json.loads(line); method = req['method']; p = req.get('params', {})
    key = p.get('requestKey')
    with open(trace, 'a') as f:
        f.write(json.dumps({'method': method, 'key': key, 'time': time.monotonic()})+'\n')
    if method == 'native.catalog.begin':
        # Synthesize admission, including online work, without contacting GameBanana.
        receipts[key] = p
        result = dict(version=1, root=p['dataDir'], catalogSession=p['catalogSession'],
                      requestKey=key, viewGeneration=p['viewGeneration'], inputDigest='a'*64, phase='running')
        response = dict(jsonrpc='2.0', id=req['id'], result=result)
    elif method == 'native.catalog.status' and key:
        if mode in ('catalogBusy', 'catalogRequestPending'):
            response = dict(jsonrpc='2.0', id=req['id'], error=dict(code=-32020, message='Controlled rejection', data=dict(kind=mode)))
        else:
            r = receipts[key]
            response = dict(jsonrpc='2.0', id=req['id'], result=dict(version=1, root=p['dataDir'],
                catalogSession=p['catalogSession'], requestKey=key, viewGeneration=r['viewGeneration'],
                inputDigest='a'*64, operation=r['operation'], phase='running', progress=None, result=None, error=None))
    elif method == 'native.catalog.cancel' or method == 'native.catalog.release':
        response = dict(jsonrpc='2.0', id=req['id'], result=dict(version=1, root=p['dataDir'],
            catalogSession=p['catalogSession'], requestKey=key, state='released'))
    else:
        child.stdin.write(line); child.stdin.flush()
        while True:
            response = json.loads(child.stdout.readline())
            if response.get('id') == req['id']: break
            print(json.dumps(response), flush=True)
    print(json.dumps(response), flush=True)
child.stdin.close(); child.wait()
)PY");
    peer.close();
    QVERIFY(peer.setPermissions(QFileDevice::ReadOwner|QFileDevice::WriteOwner|QFileDevice::ExeOwner));
    const auto tracePath = fixture.filePath("trace.jsonl");
    const auto trace = [&] {
        QFile f(tracePath); QList<QJsonObject> rows;
        if(f.open(QIODevice::ReadOnly))while(!f.atEnd())rows.append(QJsonDocument::fromJson(f.readLine()).object());
        return rows;
    };
    const auto polls = [&](const QString &key) {
        QList<double> times;
        for(const auto &r:trace())if(r["method"]==QJsonValue("native.catalog.status")&&r["key"]==QJsonValue(key))times.append(r["time"].toDouble());
        return times;
    };
    BackendClient client; CatalogController catalog(&client);
    client.start(script, {}, root, true);
    QTRY_VERIFY_WITH_TIMEOUT(catalog.available(), 5000);
    bool admitted=false;
    QTRY_VERIFY_WITH_TIMEOUT(admitted || (admitted=catalog.requestLibraryMedia("fast", {"mod","1000"}, false)),5000);
    QString fastKey;
    QTRY_VERIFY_WITH_TIMEOUT(([&]{for(const auto &r:trace())if(r["method"]==QJsonValue("native.catalog.begin")){fastKey=r["key"].toString();return true;}return false;})(),5000);
    QTRY_VERIFY_WITH_TIMEOUT(!polls(fastKey).isEmpty(),5000);
    if(mode=="overlap") {
        catalog.setActive(true); catalog.refresh();
        QTRY_VERIFY_WITH_TIMEOUT(([&]{int count=0;for(const auto &r:trace())if(r["method"]==QJsonValue("native.catalog.begin"))++count;return count>=2;})(),5000);
        waitEvents(1200);
        QHash<QString,double> last;
        for(const auto &r:trace())if(r["method"]==QJsonValue("native.catalog.status")) {
            const auto key=r["key"].toString(); if(key.isEmpty()||key==fastKey)continue;
            const double now=r["time"].toDouble();
            if(last.contains(key))QVERIFY2(now-last[key]>=0.245,"Online/query receipt polled faster than 250 ms");
            last[key]=now;
        }
        QVERIFY(!last.isEmpty());
    } else if(mode=="stall") {
        waitEvents(1700);
        const auto times=polls(fastKey); QVERIFY(times.size()>3);
        int slow=0;
        for(int i=1;i<times.size();++i)if(times[i-1]-times.first()>1.05){
            QVERIFY2(times[i]-times[i-1]>=0.245,"Stalled cache receipt did not back off"); ++slow;
        }
        QVERIFY(slow>=1);
        QVERIFY2(times.size()<=56,"Stalled cache read retained unbounded fast polling");
    } else if(mode!="shutdown") {
        QTRY_VERIFY_WITH_TIMEOUT(polls(fastKey).size()>=3,5000);
        const auto times=polls(fastKey);
        for(int i=1;i<times.size();++i)QVERIFY2(times[i]-times[i-1]>=0.245,"Busy/pending rejection retried at fast cadence");
    }
    catalog.shutdown();
    QTRY_VERIFY_WITH_TIMEOUT(catalog.shutdownReady(),5000);
    waitEvents(50); // drain already queued lifecycle notifications
    int changes=0;
    QObject::connect(&catalog,&CatalogController::changed,&catalog,[&]{++changes;});
    const auto requests=trace().size();
    waitEvents(600);
    QCOMPARE(changes,0); QCOMPARE(trace().size(),requests);
    client.stop(); QTRY_VERIFY_WITH_TIMEOUT(!client.retiring(),5000);
}

// A short bulk pause must wake passive Library demand even when no poll saw
// the pause. Removing the deferred-media wake-up strands this request forever.
static void checkCatalogDeferredMedia(const QString &helper) {
    QTemporaryDir fixture;
    QVERIFY(fixture.isValid());
    const auto root = fixture.filePath("library");
    QProcess initialize;
    initialize.start(helper, {});
    QVERIFY(initialize.waitForStarted());
    initialize.write(QJsonDocument(QJsonObject{{"jsonrpc", "2.0"}, {"id", 1},
        {"method", "native.library.initialize"}, {"params", QJsonObject{
            {"version", 1}, {"dataDir", root}, {"operationId", QString(32, 'd')}, {"expectedRevision", "0"}}}}).toJson(QJsonDocument::Compact) + '\n');
    initialize.closeWriteChannel();
    QVERIFY(initialize.waitForFinished());
    QVERIFY(!QJsonDocument::fromJson(initialize.readAllStandardOutput()).object().contains("error"));

    BackendClient client;
    CatalogController catalog(&client); // Home: Browse is inactive, no session yet.
    client.start(helper, {}, root, true);
    QTRY_VERIFY_WITH_TIMEOUT(catalog.available() && !client.catalogPaused(), 5000);
    QVERIFY(catalog.libraryMediaSession().isEmpty());
    QVERIFY(!catalog.active());

    // Suppress timer delivery until the real bulk reply is consumed. This makes
    // "pause ends before first poll" deterministic even on a heavily loaded CI
    // machine; socket/process events and real helper replies still run normally.
    struct TimerObserver : QObject {
        bool block = true;
        QPointer<QTimer> last;
        QHash<QObject *, int> ticks;
        bool eventFilter(QObject *object, QEvent *event) override {
            if (event->type() == QEvent::Timer) {
                if (auto *timer = qobject_cast<QTimer *>(object)) {
                    if (block) return true;
                    last = timer;
                    ++ticks[object];
                }
            }
            return false;
        }
    } observer;
    QCoreApplication::instance()->installEventFilter(&observer);
    const auto removeFilter = qScopeGuard([&] {
        QCoreApplication::instance()->removeEventFilter(&observer);
    });
    bool admitted = false, pumping = false;
    int completed = 0, wakeups = 0;
    QPointer<QTimer> poll;
    const QString key = "deferred-cache-read";
    QObject::connect(&catalog, &CatalogController::libraryMediaReady, &catalog,
        [&](const QString &readyKey, const QString &readyRoot, const CatalogDecodedResult &result) {
            if (readyKey == key && readyRoot == root && result.valid && result.phase == "succeeded")
                ++completed;
        });
    QObject::connect(&catalog, &CatalogController::changed, &catalog, [&] {
        if (observer.block || client.catalogPaused()) return;
        ++wakeups;
        // The first wake-up must come from the deferred poll, before opening a
        // session. Retain that timer to verify actual quiescence after delivery.
        if (!poll) poll = observer.last;
        if (!admitted && !pumping) {
            pumping = true;
            admitted = catalog.requestLibraryMedia(key, {"mod", "1000"}, false);
            pumping = false;
        }
    });

    client.requestSnapshot({}, {});
    QVERIFY(client.catalogPaused());
    QVERIFY(!catalog.requestLibraryMedia(key, {"mod", "1000"}, false));
    QTRY_VERIFY_WITH_TIMEOUT(!client.catalogPaused(), 5000);
    QCOMPARE(wakeups, 0);
    QVERIFY(catalog.libraryMediaSession().isEmpty());
    observer.block = false;
    // Retry ONLY on changed(), never from this wait: an unconditional retry
    // would conceal the missing wake-up in the old controller.
    QTRY_VERIFY_WITH_TIMEOUT(admitted && completed == 1, 5000);
    QVERIFY(poll);
    QTRY_VERIFY_WITH_TIMEOUT(!poll->isActive(), 5000);
    const auto ticks = observer.ticks.value(poll);
    const auto changes = wakeups;
    waitEvents(600);
    QVERIFY(!poll->isActive());
    QCOMPARE(observer.ticks.value(poll), ticks);
    QCOMPARE(wakeups, changes);
    QCOMPARE(completed, 1);
    QVERIFY(!client.catalogPaused());

    catalog.shutdown();
    QTRY_VERIFY_WITH_TIMEOUT(catalog.shutdownReady(), 5000);
    client.stop();
    QTRY_VERIFY_WITH_TIMEOUT(!client.retiring(), 5000);
}

// Real demand, normal timer delivery, and a controlled storage refusal at the
// process boundary. Removing retry recovery strands the original demand.
static void checkLibraryMediaOpenRetry(const QString &helper, const QString &mode) {
    QTemporaryDir fixture; QVERIFY(fixture.isValid());
    const auto root=fixture.filePath("library"), release=fixture.filePath("release"), trace=fixture.filePath("opens");
    QProcess initialize; initialize.start(helper,{}); QVERIFY(initialize.waitForStarted());
    initialize.write(QJsonDocument(QJsonObject{{"jsonrpc","2.0"},{"id",1},{"method","native.library.initialize"},
        {"params",QJsonObject{{"version",1},{"dataDir",root},{"operationId",QString(32,'e')},{"expectedRevision","0"}}}}).toJson(QJsonDocument::Compact)+'\n');
    initialize.closeWriteChannel(); QVERIFY(initialize.waitForFinished());
    QVERIFY(!QJsonDocument::fromJson(initialize.readAllStandardOutput()).object().contains("error"));
    const auto script=fixture.filePath("peer.py"); QFile peer(script); QVERIFY(peer.open(QIODevice::WriteOnly));
    peer.write("#!/usr/bin/python3\nimport json, os, sys, subprocess, time\nhelper, release, trace = "+QJsonDocument(QJsonArray{helper,release,trace}).toJson(QJsonDocument::Compact)+"\n");
    peer.write("mode = "+QJsonDocument(QJsonArray{mode}).toJson(QJsonDocument::Compact)+"[0]\n");
    peer.write(R"PY(child = subprocess.Popen([helper], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
failed_session = None
for line in sys.stdin:
    req = json.loads(line)
    with open(trace+'.wire', 'a') as f: f.write(json.dumps(req)+'\n')
    if req['method'] == 'native.catalog.open':
        with open(trace, 'a') as f: f.write(str(time.monotonic())+'\n')
        if not os.path.exists(release):
            failed_session = 'e'*32
            if mode == 'refusal':
                failed_session = None
                print(json.dumps(dict(jsonrpc='2.0', id=req['id'], error=dict(code=-32020, message='Controlled storage refusal', data=dict(kind='catalogRootBusy')))), flush=True)
                continue
            print(json.dumps(dict(jsonrpc='2.0', id=req['id'], result=dict(version=1, root=req['params']['dataDir'], catalogSession=failed_session, phase='failed', error=dict(kind='catalogStorage', message='Controlled storage refusal', retryAfterSeconds=None)))), flush=True)
            continue
    if failed_session and req['method'] == 'native.catalog.close':
        print(json.dumps(dict(jsonrpc='2.0', id=req['id'], result=dict(version=1, root=req['params']['dataDir'], catalogSession=failed_session, phase='closed'))), flush=True)
        failed_session = None
        continue
    child.stdin.write(line); child.stdin.flush()
    while True:
        response = json.loads(child.stdout.readline())
        print(json.dumps(response), flush=True)
        if response.get('id') == req['id']: break
child.stdin.close(); child.wait()
)PY");
    peer.close(); QVERIFY(peer.setPermissions(QFileDevice::ReadOwner|QFileDevice::WriteOwner|QFileDevice::ExeOwner));
    const auto evidence=qScopeGuard([&]{
        const auto dir=qEnvironmentVariable("CATALOG_RETRY_EVIDENCE");
        if(!dir.isEmpty()){QFile::remove(dir+"/"+mode+".wire");QFile::copy(trace+".wire",dir+"/"+mode+".wire");}
    });
    const auto opens=[&]{ QFile f(trace); QList<double> times; if(f.open(QIODevice::ReadOnly))while(!f.atEnd())times.append(f.readLine().trimmed().toDouble()); return times; };
    const auto allow=[&]{QFile f(release); QVERIFY(f.open(QIODevice::WriteOnly));};
    BackendClient client; CatalogController catalog(&client); LibraryMediaController media(&catalog);
    client.start(script,{},root,true);
    QTRY_VERIFY_WITH_TIMEOUT(catalog.available()&&!client.catalogPaused(),5000);
    int failures=0, completed=0, openFailures=0;
    QObject::connect(&client,&BackendClient::catalogReady,&catalog,[&](BackendClient::Call,const CatalogDecodedResult &r){
        if(r.key.isEmpty()&&r.phase=="failed")++openFailures;
    });
    QObject::connect(&client,&BackendClient::catalogRejected,&catalog,[&](BackendClient::Call call,const QString &,const QString &,const QString &){
        if(call==BackendClient::Call::CatalogOpen)++openFailures;
    });
    QObject::connect(&catalog,&CatalogController::libraryMediaReady,&media,[&](const QString &,const QString &,const CatalogDecodedResult &r){
        if(r.errorKind=="catalogStorage"||r.errorKind=="catalogRootBusy")++failures;
        if(r.valid&&r.phase=="succeeded")++completed;
    });
    // Shared Browse open must fail after Library has attached its demand.
    if(mode=="shared")catalog.setActive(true);
    media.setContext(root,1,true);
    if(mode=="deferred") { allow(); client.requestSnapshot({},{}); QVERIFY(client.catalogPaused()); }
    media.demand("selected","row",{"mod","1000"},true);
    if(mode=="shared")catalog.setActive(false);
    QTimer caller;
    if(mode=="cap"||mode=="refusal") {
        caller.setInterval(500);
        QObject::connect(&caller,&QTimer::timeout,&catalog,[&]{catalog.requestLibraryMedia("enrichment",{"mod","1001"},false);});
        caller.start();
    }
    if(mode!="deferred") {
        QTRY_VERIFY_WITH_TIMEOUT(failures>=1,5000);
        waitEvents(600); QCOMPARE(opens().size(),1);
        int synchronousChanges=0,synchronousResults=0;
        const auto changedConnection=QObject::connect(&catalog,&CatalogController::changed,&catalog,[&]{++synchronousChanges;});
        const auto resultConnection=QObject::connect(&catalog,&CatalogController::libraryMediaReady,&catalog,[&]{++synchronousResults;});
        QVERIFY(!catalog.requestLibraryMedia("gated",{"mod","1002"},false));
        QObject::disconnect(changedConnection);QObject::disconnect(resultConnection);
        QCOMPARE(synchronousChanges,0);QCOMPARE(synchronousResults,0);
        if(mode=="cap"||mode=="refusal") {
            // Five attempts check all four intervals, including the repeated cap.
            QTRY_VERIFY_WITH_TIMEOUT(opens().size()>=5,42000);
            const auto times=opens(); QCOMPARE(times.size(),5);
            const QList<double> minimum{2,5,15,15};
            for(int i=0;i<4;++i){
                std::fprintf(stderr,"open spacing %d: %.3f seconds\n",i+1,times[i+1]-times[i]);
                QVERIFY(times[i+1]-times[i]>=minimum[i]);
            }
            QTRY_VERIFY_WITH_TIMEOUT(openFailures>=5,3000);
            caller.stop();allow();
            // Explicit Browse retry bypasses the pending 15-second passive gate.
            catalog.setActive(true);catalog.retry();
            QTRY_VERIFY_WITH_TIMEOUT(!catalog.libraryMediaRoot().isEmpty(),5000);
            catalog.setActive(false);
        } else { allow(); }
    }
    QTRY_VERIFY_WITH_TIMEOUT(completed>=1,8000);
    if(mode=="cap") {
        // Retire the ready session without restarting the controller: ready must
        // reset the accumulated delay before the next storage failure.
        media.clear();catalog.shutdown();QTRY_VERIFY_WITH_TIMEOUT(catalog.shutdownReady(),5000);
        catalog.resumeAfterRetirement();QVERIFY(QFile::remove(release));
        const auto before=opens().size();
        media.setContext(root,2,true);media.demand("selected","row",{"mod","1000"},true);
        QTRY_VERIFY_WITH_TIMEOUT(opens().size()==before+1,3000);
        QTRY_VERIFY_WITH_TIMEOUT(opens().size()==before+2,3500);
        const auto times=opens();std::fprintf(stderr,"reset spacing: %.3f seconds\n",times.last()-times[times.size()-2]);
        QVERIFY(times.last()-times[times.size()-2]>=2);
        QVERIFY(times.last()-times[times.size()-2]<3.5);
    }
    media.setContext(root,2,false);
    catalog.shutdown(); QTRY_VERIFY_WITH_TIMEOUT(catalog.shutdownReady(),5000);
    for(auto *timer:catalog.findChildren<QTimer *>())QVERIFY(!timer->isActive());
    const auto attempts=opens().size(); waitEvents(2300); QCOMPARE(opens().size(),attempts);
    client.stop(); QTRY_VERIFY_WITH_TIMEOUT(!client.retiring(),5000);
}

// An open refused (catalogBusy) by a Library snapshot before any session is
// owned must not leave a stale failed phase that strands passive demand.
static void checkLibraryMediaBusyOpenRecovery(const QString &helper, bool browseFirst) {
    QTemporaryDir fixture; QVERIFY(fixture.isValid());
    const auto root=fixture.filePath("library");
    QProcess initialize; initialize.start(helper,{}); QVERIFY(initialize.waitForStarted());
    initialize.write(QJsonDocument(QJsonObject{{"jsonrpc","2.0"},{"id",1},{"method","native.library.initialize"},
        {"params",QJsonObject{{"version",1},{"dataDir",root},{"operationId",QString(32,'e')},{"expectedRevision","0"}}}}).toJson(QJsonDocument::Compact)+'\n');
    initialize.closeWriteChannel(); QVERIFY(initialize.waitForFinished());
    QVERIFY(!QJsonDocument::fromJson(initialize.readAllStandardOutput()).object().contains("error"));
    BackendClient client; CatalogController catalog(&client); LibraryMediaController media(&catalog);
    int rejects=0, completed=0;
    QObject::connect(&client,&BackendClient::catalogRejected,&catalog,[&](BackendClient::Call call,const QString &,const QString &,const QString &){
        if(call==BackendClient::Call::CatalogOpen)++rejects;
    });
    QObject::connect(&catalog,&CatalogController::libraryMediaReady,&catalog,[&](const QString &,const QString &,const CatalogDecodedResult &r){
        if(r.valid&&r.phase=="succeeded")++completed;
    });
    client.start(helper,{},root,true);
    QTRY_VERIFY_WITH_TIMEOUT(catalog.available()&&!client.catalogPaused(),5000);
    media.setContext(root,1,true);
    if(browseFirst){
        client.requestSnapshot({},{}); QVERIFY(client.catalogPaused());
        catalog.setActive(true); catalog.setActive(false);
        media.demand("selected","row",{"mod","1000"},true);
    }else{
        QVERIFY(!catalog.requestLibraryMedia("queued",{"mod","1000"},false));
        client.requestSnapshot({},{}); QVERIFY(client.catalogPaused());
        media.demand("selected","row",{"mod","1000"},true);
    }
    QTRY_VERIFY_WITH_TIMEOUT(rejects>0&&!client.catalogPaused(),5000);
    // Recovery must happen without entering Browse; allow the 2 s passive gate.
    QTRY_VERIFY_WITH_TIMEOUT(completed>=1,8000);
    QVERIFY(!catalog.active());
    media.clear(); catalog.shutdown(); QTRY_VERIFY_WITH_TIMEOUT(catalog.shutdownReady(),5000);
    client.stop(); QTRY_VERIFY_WITH_TIMEOUT(!client.retiring(),5000);
}

// The peer holds real catalog receipts at running until the test releases them.
// Assert wire admission/cancellation and row projection, never controller fields.
struct LibraryMediaPeer {
    QTemporaryDir fixture;
    QString root = fixture.filePath("library");
    LibraryMediaPeer() { QDir().mkpath(root); }
    void snapshot(const QString &id, const QString &at = {}) {
        const auto target=at.isEmpty()?root:at;
        auto value=QJsonDocument::fromJson(R"({"protocolVersion":1,"sourceKind":"native","storeVersion":1,"revision":"1",
            "activeProfileId":"default","selectedProfileId":"default","pendingChanges":false,"canRestore":false,
            "recoveryRequired":false,"gamePath":"","appliedProfileId":null,"warnings":[],
            "profiles":[{"id":"default","name":"Default","isActive":true,"modCount":2}],
            "mods":[{"id":"anchor","remoteId":"","title":"Anchor","author":"Fixture","hero":"Ivy","category":"Skins",
                "status":"installed","enabled":true,"appliedEnabled":false,"available":true,"unavailableReason":null,
                "installOrder":null,"installedVpks":[],"missingVpks":[]},
                {"id":"row","remoteId":"1000","title":"Gallery","author":"Fixture","hero":"Ivy","category":"Skins",
                "status":"installed","enabled":true,"appliedEnabled":false,"available":true,"unavailableReason":null,
                "installOrder":null,"installedVpks":[],"missingVpks":[]}]})").object();
        value["sourcePath"]=target+"/library.json";
        auto mods=value["mods"].toArray();auto row=mods[1].toObject();row["id"]=id;mods[1]=row;value["mods"]=mods;
        QFile f(target+"/snapshot.json");QVERIFY(f.open(QIODevice::WriteOnly));f.write(QJsonDocument(value).toJson());
    }
    QList<QJsonObject> trace(const QString &at = {}) const {
        QFile f((at.isEmpty() ? root : at) + "/trace.jsonl"); QList<QJsonObject> rows;
        if (f.open(QIODevice::ReadOnly)) while (!f.atEnd()) rows.append(QJsonDocument::fromJson(f.readLine()).object());
        return rows;
    }
    QList<QJsonObject> calls(const QString &method, const QString &at = {}) const {
        QList<QJsonObject> result;
        for (const auto &r : trace(at)) if (r["method"].toString() == method) result.append(r);
        return result;
    }
    void complete(const QString &id, const QString &url, const QString &at = {}) {
        QFile f((at.isEmpty() ? root : at) + "/complete-" + id);
        QVERIFY(f.open(QIODevice::WriteOnly)); f.write(url.toUtf8());
    }
};

static QString writeHomeCatalogPeerScript(const QString &path, const QString &helper) {
    QFile peer(path);
    QVERIFY(peer.open(QIODevice::WriteOnly));
    peer.write("#!/usr/bin/python3\nHELPER = " + QJsonDocument(QJsonArray{helper}).toJson(QJsonDocument::Compact) + "[0]\n");
    peer.write(R"PY(import hashlib, json, os, subprocess, sys, time
child = subprocess.Popen([HELPER], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
receipts = {}
released = set()

def cfg(root):
    p = os.path.join(root, 'home-peer.json')
    if os.path.exists(p):
        with open(p, 'r') as f:
            return json.load(f)
    return {}

def trace(root, obj):
    with open(os.path.join(root, 'home-trace.jsonl'), 'a') as f:
        f.write(json.dumps(obj) + '\n')

def make_summary(sid, title, downloads, hero='Ivy', nsfw=False):
    sub = dict(provider='gamebanana', submissionType='mod', submissionId=str(sid))
    return dict(
        submission=sub, name=title, description='Saved description for ' + title,
        author='PlayerOne', authorRemoteId='101',
        profileUrl='https://gamebanana.com/mods/' + str(sid),
        category='Skins', hero=hero, isAudio=False, isMap=False,
        isNsfw=nsfw, visibilityKnown=True, isObsolete=False, hasFiles=True,
        downloadCount=str(downloads), likes='12',
        images=['https://images.gamebanana.com/img/ss/mods/' + str(sid) + '.jpg'],
        thumbnailUrl='https://images.gamebanana.com/img/ss/mods/' + str(sid) + '_thumb.jpg',
        audioUrl=None, tags=['skin'], remoteAddedAt=1700000000,
        remoteUpdatedAt=1700000100, filesUpdatedAt=1700000200,
        developmentState=None, completionPercentage=None, hydrated=True,
    )

def make_prov(conf):
    return dict(
        source='cache', sourcePin='976637e2d63329ffef6f87eca40e2aea681acc94',
        fetchedAt=1700000300, lastSuccessfulSyncAt=1700000300,
        stale=bool(conf.get('stale', False)),
        incomplete=bool(conf.get('incomplete', False)),
        refreshError=None, searchAvailable=True, searchState='available',
        accounting=dict(databaseBytes='1024', sidecarBytes='512', spoolAndControlBytes='128', reserveBytes='1073741824', aggregateQuota=False),
    )

def make_result(op, conf):
    kind = op['kind']
    args = op['args']
    prefs_rev = '0' if kind in ('query', 'collection', 'favoritesGet', 'favoriteSet') else None
    ident = dict(kind=kind, args=args, catalogRevision='1', preferencesRevision=prefs_rev)
    prov = make_prov(conf)
    if kind == 'query':
        sort = args.get('sort', 'default')
        hide_nsfw = bool(args.get('hideNsfw', True))
        if sort == 'releaseDate':
            items = conf.get('latestItems')
            if items is None:
                items = [make_summary(201, 'Latest One', 50, 'Ivy', not hide_nsfw),
                         make_summary(202, 'Latest Two', 40, 'Haze', False),
                         make_summary(203, 'Latest Three', 30, 'Lash', False)]
        elif sort == 'downloadCount':
            items = conf.get('popularItems')
            if items is None:
                items = [make_summary(301, 'Popular One', 9000, 'Abrams', False),
                         make_summary(302, 'Popular Two', 5000, 'Seven', False)]
        else:
            items = [make_summary(101, 'Browse Default', 100, 'Warden', False)]
        favs = [dict(submission=i['submission'], favorite=False) for i in items]
        data = dict(items=items, total=str(len(items)), page=args['page'], pageSize=args['pageSize'], favorites=favs, preferencesRevision='0')
    elif kind == 'facets':
        data = [dict(category='Skins', hero='Ivy')]
    elif kind == 'sync':
        data = dict(mode='incremental', snapshot='b'*32, complete=True, startedAt=1700000300, memberMaxAgeSeconds=604800,
                    collections=dict(state='complete', snapshot='c'*32, headersComplete=True, membersComplete=False, memberMaxAgeSeconds=604800))
    else:
        return None
    return dict(identity=ident, catalogRevision='1', provenance=prov, data=data)

for line in sys.stdin:
    req = json.loads(line)
    method = req['method']
    p = req.get('params', {})
    root = p.get('dataDir', '')
    if root and method.startswith('native.catalog.'):
        trace(root, dict(method=method, key=p.get('requestKey'), op=p.get('operation'), time=time.monotonic()))
    if method == 'native.catalog.open':
        conf = cfg(root)
        sess = hashlib.sha256(root.encode()).hexdigest()[:32]
        if conf.get('failOpen'):
            res = dict(version=1, root=root, catalogSession=sess, phase='failed',
                       error=dict(kind='catalogUnavailable', message='Controlled unavailable open', retryAfterSeconds=None))
        else:
            res = dict(version=1, root=root, catalogSession=sess, phase='ready', error=None)
        print(json.dumps(dict(jsonrpc='2.0', id=req['id'], result=res)), flush=True)
        continue
    if method == 'native.catalog.close':
        print(json.dumps(dict(jsonrpc='2.0', id=req['id'], result=dict(version=1, root=root, catalogSession=p['catalogSession'], phase='closed'))), flush=True)
        continue
    if method == 'native.catalog.begin':
        conf = cfg(root)
        key = p['requestKey']
        receipts[key] = p
        op = p['operation']
        marker = os.path.join(root, 'home-exhausted-once')
        if op['kind'] == 'query' and conf.get('exhaustHomeOnce') and not os.path.exists(marker):
            open(marker, 'w').close()
            print(json.dumps(dict(jsonrpc='2.0', id=req['id'], error=dict(code=-32020, message='Controlled Home session recycle', data=dict(kind='catalogSessionExhausted')))), flush=True)
            continue
        if op['kind'] == 'authors' and conf.get('exhaustAuthors'):
            print(json.dumps(dict(jsonrpc='2.0', id=req['id'], error=dict(code=-32020, message='Controlled session recycle', data=dict(kind='catalogSessionExhausted')))), flush=True)
            continue
        if conf.get('holdSort') == op['args'].get('sort') and not os.path.exists(os.path.join(root, 'release-' + key)) and not os.path.exists(os.path.join(root, 'release-all')):
            phase = 'running'
        else:
            phase = 'running'
        res = dict(version=1, root=root, catalogSession=p['catalogSession'], requestKey=key,
                   viewGeneration=p['viewGeneration'], inputDigest='a'*64, phase=phase)
        print(json.dumps(dict(jsonrpc='2.0', id=req['id'], result=res)), flush=True)
        continue
    if method == 'native.catalog.status' and p.get('requestKey'):
        conf = cfg(root)
        key = p['requestKey']
        r = receipts[key]
        op = r['operation']
        args = op['args']
        held = (conf.get('holdSort') == args.get('sort') or (conf.get('holdNsfw') is not None and args.get('hideNsfw') == conf.get('holdNsfw')))
        released_flag = os.path.exists(os.path.join(root, 'release-' + key)) or os.path.exists(os.path.join(root, 'release-all'))
        late_succeed_flag = os.path.exists(os.path.join(root, 'late-succeed-cancelled'))
        if key in released and not (late_succeed_flag and conf.get('ignoreCancel')):
            res = dict(version=1, root=root, catalogSession=p['catalogSession'], requestKey=key,
                       viewGeneration=r['viewGeneration'], inputDigest='a'*64,
                       operation=dict(kind=op['kind'], args=None), phase='released', progress=None, result=None, error=None)
        elif held and not released_flag:
            res = dict(version=1, root=root, catalogSession=p['catalogSession'], requestKey=key,
                       viewGeneration=r['viewGeneration'], inputDigest='a'*64,
                       operation=op, phase='running', progress=None, result=None, error=None)
        elif conf.get('failSlot') == args.get('sort'):
            res = dict(version=1, root=root, catalogSession=p['catalogSession'], requestKey=key,
                       viewGeneration=r['viewGeneration'], inputDigest='a'*64,
                       operation=op, phase='failed', progress=None, result=None,
                       error=dict(kind='catalogUnavailable', message='Slot unavailable', retryAfterSeconds=None))
        else:
            res = dict(version=1, root=root, catalogSession=p['catalogSession'], requestKey=key,
                       viewGeneration=r['viewGeneration'], inputDigest='a'*64,
                       operation=op, phase='succeeded', progress=None, result=make_result(op, conf), error=None)
        print(json.dumps(dict(jsonrpc='2.0', id=req['id'], result=res)), flush=True)
        continue
    if method == 'native.catalog.cancel' and p.get('requestKey'):
        conf = cfg(root)
        key = p['requestKey']
        released.add(key)
        state = 'requested' if conf.get('ignoreCancel') else 'released'
        res = dict(version=1, root=root, catalogSession=p['catalogSession'], requestKey=key, state=state)
        print(json.dumps(dict(jsonrpc='2.0', id=req['id'], result=res)), flush=True)
        continue
    if method == 'native.catalog.release' and p.get('requestKey'):
        key = p['requestKey']
        released.add(key)
        res = dict(version=1, root=root, catalogSession=p['catalogSession'], requestKey=key, state='released')
        print(json.dumps(dict(jsonrpc='2.0', id=req['id'], result=res)), flush=True)
        continue
    child.stdin.write(line); child.stdin.flush()
    while True:
        response = json.loads(child.stdout.readline())
        if response.get('id') == req['id']: break
        print(json.dumps(response), flush=True)
    print(json.dumps(response), flush=True)
child.stdin.close(); child.wait()
)PY");
    peer.close();
    QVERIFY(peer.setPermissions(QFileDevice::ReadOwner|QFileDevice::WriteOwner|QFileDevice::ExeOwner));
    return path;
}

static QList<QJsonObject> readHomeTrace(const QString &root) {
    QFile f(root + "/home-trace.jsonl");
    QList<QJsonObject> rows;
    if (f.open(QIODevice::ReadOnly))
        while (!f.atEnd()) rows.append(QJsonDocument::fromJson(f.readLine()).object());
    return rows;
}

static void checkHomeFeedPreservesBrowseState(const QString &helper) {
    QTemporaryDir fixture; QVERIFY(fixture.isValid());
    const auto root = fixture.filePath("library");
    QVERIFY(QDir().mkpath(root));
    const auto script = writeHomeCatalogPeerScript(fixture.filePath("peer.py"), helper);
    BackendClient client;
    CatalogController catalog(&client);
    client.start(script, {}, root, true);
    QTRY_VERIFY_WITH_TIMEOUT(catalog.available(), 5000);
    catalog.setHomeActive(true);
    QTRY_VERIFY_WITH_TIMEOUT(!catalog.homeLoading() && catalog.homeLatest().size() == 3 && catalog.homePopular().size() == 2, 5000);
    QVERIFY(!catalog.active());
    QCOMPARE(catalog.route(), QString("browse"));
    QCOMPARE(catalog.model()->count(), 0);
    QCOMPARE(catalog.total(), QString("0"));
    QCOMPARE(catalog.viewGeneration(), 1ULL);
    QCOMPARE(catalog.queryState().value("sort").toString(), QString("default"));
    QVERIFY(catalog.error().isEmpty());
    QVERIFY(catalog.provenance().isEmpty());
    QVERIFY(!catalog.homeUnavailable());
    QCOMPARE(catalog.homeLatest().first().toMap().value("title").toString(), QString("Latest One"));
    QCOMPARE(catalog.homePopular().first().toMap().value("title").toString(), QString("Popular One"));

    const auto rows = readHomeTrace(root);
    QStringList sorts;
    for (const auto &r : rows) {
        if (r["method"].toString() == "native.catalog.begin") {
            const auto op = r["op"].toObject();
            QCOMPARE(op["kind"].toString(), QString("query"));
            const auto args = op["args"].toObject();
            QCOMPARE(args["page"].toInt(), 0);
            QCOMPARE(args["pageSize"].toInt(), 5);
            QCOMPARE(args["submissionType"].toString(), QString("mod"));
            sorts.append(args["sort"].toString());
        }
    }
    QCOMPARE(sorts, (QStringList{"releaseDate", "downloadCount"}));
    catalog.shutdown(); QTRY_VERIFY_WITH_TIMEOUT(catalog.shutdownReady(), 5000);
    client.stop(); QTRY_VERIFY_WITH_TIMEOUT(!client.retiring(), 5000);
}

static void checkHomeFeedYieldsToActiveBrowse(const QString &helper) {
    QTemporaryDir fixture; QVERIFY(fixture.isValid());
    const auto root = fixture.filePath("library");
    QVERIFY(QDir().mkpath(root));
    {
        QFile cfg(root + "/home-peer.json"); QVERIFY(cfg.open(QIODevice::WriteOnly));
        cfg.write(QJsonDocument(QJsonObject{{"holdSort", "releaseDate"}, {"ignoreCancel", true}}).toJson());
    }
    const auto script = writeHomeCatalogPeerScript(fixture.filePath("peer.py"), helper);
    BackendClient client;
    CatalogController catalog(&client);
    client.start(script, {}, root, true);
    QTRY_VERIFY_WITH_TIMEOUT(catalog.available(), 5000);
    catalog.setHomeActive(true);
    QString heldKey;
    QTRY_VERIFY_WITH_TIMEOUT(([&] {
        for (const auto &r : readHomeTrace(root)) {
            if (r["method"].toString() == "native.catalog.begin" && r["op"].toObject()["args"].toObject()["sort"].toString() == "releaseDate") {
                heldKey = r["key"].toString();
                return true;
            }
        }
        return false;
    })(), 5000);
    QVERIFY(catalog.homeLoading());
    catalog.setHomeActive(false);
    catalog.browse();
    QVERIFY(catalog.active());
    QTRY_VERIFY_WITH_TIMEOUT(([&] {
        for (const auto &r : readHomeTrace(root))
            if (r["method"].toString() == "native.catalog.cancel" && r["key"].toString() == heldKey) return true;
        return false;
    })(), 5000);
    {
        QFile f(root + "/late-succeed-cancelled"); QVERIFY(f.open(QIODevice::WriteOnly));
    }
    {
        QFile f(root + "/release-" + heldKey); QVERIFY(f.open(QIODevice::WriteOnly));
    }
    QTRY_VERIFY_WITH_TIMEOUT(catalog.model()->count() == 1, 5000);
    waitEvents(300);
    QVERIFY(catalog.homeLatest().isEmpty());
    QVERIFY(catalog.homePopular().isEmpty());
    QCOMPARE(catalog.queryState().value("sort").toString(), QString("default"));

    catalog.shutdown(); QTRY_VERIFY_WITH_TIMEOUT(catalog.shutdownReady(), 5000);
    client.stop(); QTRY_VERIFY_WITH_TIMEOUT(!client.retiring(), 5000);

    // Passive open failure must leave Browse error untouched and mark Home quietly unavailable.
    QTemporaryDir failFixture; QVERIFY(failFixture.isValid());
    const auto failRoot = failFixture.filePath("library"); QVERIFY(QDir().mkpath(failRoot));
    {
        QFile cfg(failRoot + "/home-peer.json"); QVERIFY(cfg.open(QIODevice::WriteOnly));
        cfg.write(QJsonDocument(QJsonObject{{"failOpen", true}}).toJson());
    }
    const auto failScript = writeHomeCatalogPeerScript(failFixture.filePath("peer.py"), helper);
    BackendClient failClient; CatalogController failCatalog(&failClient);
    failClient.start(failScript, {}, failRoot, true);
    QTRY_VERIFY_WITH_TIMEOUT(failCatalog.available(), 5000);
    failCatalog.setHomeActive(true);
    QTRY_VERIFY_WITH_TIMEOUT(failCatalog.homeUnavailable() && !failCatalog.homeLoading(), 5000);
    QVERIFY(failCatalog.error().isEmpty());
    failCatalog.shutdown(); QTRY_VERIFY_WITH_TIMEOUT(failCatalog.shutdownReady(), 5000);
    failClient.stop(); QTRY_VERIFY_WITH_TIMEOUT(!failClient.retiring(), 5000);
}

static void checkHomeFeedRejectsStaleRootNsfwInactiveResponses(const QString &helper) {
    QTemporaryDir fixture; QVERIFY(fixture.isValid());
    const auto root = fixture.filePath("library"), root2 = fixture.filePath("library2");
    QVERIFY(QDir().mkpath(root)); QVERIFY(QDir().mkpath(root2));
    {
        QFile cfg(root + "/home-peer.json"); QVERIFY(cfg.open(QIODevice::WriteOnly));
        cfg.write(QJsonDocument(QJsonObject{{"holdNsfw", true}}).toJson());
    }
    const auto script = writeHomeCatalogPeerScript(fixture.filePath("peer.py"), helper);
    BackendClient client;
    CatalogController catalog(&client);
    client.start(script, {}, root, true);
    QTRY_VERIFY_WITH_TIMEOUT(catalog.available(), 5000);
    catalog.setHomeActive(true);
    QString firstKey;
    QTRY_VERIFY_WITH_TIMEOUT(([&] {
        for (const auto &r : readHomeTrace(root))
            if (r["method"].toString() == "native.catalog.begin") { firstKey = r["key"].toString(); return true; }
        return false;
    })(), 5000);
    // Toggle hideNsfw while first request is still running: old response must not land.
    catalog.setHideNsfw(false);
    {
        QFile f(root + "/release-" + firstKey); QVERIFY(f.open(QIODevice::WriteOnly));
    }
    QTRY_VERIFY_WITH_TIMEOUT(!catalog.homeLoading() && catalog.homeLatest().size() == 3, 5000);
    // First item for hideNsfw == false has masked == true (isNsfw: !hideNsfw).
    QCOMPARE(catalog.homeLatest().first().toMap().value("masked").toBool(), true);

    // Leaving Home rejects any subsequent late arrivals.
    catalog.setHomeActive(false);
    QVERIFY(!catalog.homeLoading());
    {
        QFile cfg(root + "/home-peer.json"); QVERIFY(cfg.open(QIODevice::WriteOnly));
        cfg.write(QJsonDocument(QJsonObject{{"holdSort", "releaseDate"}}).toJson());
    }
    catalog.setHideNsfw(true);
    QVERIFY(catalog.homeLatest().isEmpty());
    catalog.setHomeActive(true);
    QString secondKey;
    QTRY_VERIFY_WITH_TIMEOUT(([&] {
        for (const auto &r : readHomeTrace(root))
            if (r["method"].toString() == "native.catalog.begin" && r["op"].toObject()["args"].toObject()["hideNsfw"].toBool() == true && r["key"].toString() != firstKey)
                secondKey = r["key"].toString();
        return !secondKey.isEmpty();
    })(), 5000);
    catalog.setHomeActive(false);
    {
        QFile f(root + "/release-" + secondKey); QVERIFY(f.open(QIODevice::WriteOnly));
    }
    QTRY_VERIFY_WITH_TIMEOUT(([&] {
        for (const auto &r : readHomeTrace(root))
            if (r["method"].toString() == "native.catalog.cancel" && r["key"].toString() == secondKey) return true;
        return false;
    })(), 5000);
    waitEvents(200);
    QVERIFY(catalog.homeLatest().isEmpty());

    // Re-populate on root, then switch to root2 (rootChanged clears stale Home data).
    {
        QFile cfg(root + "/home-peer.json"); QVERIFY(cfg.open(QIODevice::WriteOnly));
        cfg.write(QJsonDocument(QJsonObject{}).toJson());
    }
    catalog.setHomeActive(true);
    QTRY_VERIFY_WITH_TIMEOUT(!catalog.homeLoading() && catalog.homeLatest().size() == 3, 5000);
    catalog.shutdown(); QTRY_VERIFY_WITH_TIMEOUT(catalog.shutdownReady(), 5000);
    client.stop(); QTRY_VERIFY_WITH_TIMEOUT(!client.retiring(), 5000);
    catalog.resumeAfterRetirement();
    QVERIFY(catalog.homeLatest().isEmpty());
    QVERIFY(catalog.homePopular().isEmpty());
    QVERIFY(catalog.homeProvenance().isEmpty());
    QVERIFY(catalog.homeActive()); // Temporary retirement preserves the desired page.
    client.start(script, {}, root2, true);
    QTRY_VERIFY_WITH_TIMEOUT(catalog.available(), 5000);
    QTRY_VERIFY_WITH_TIMEOUT(!catalog.homeLoading() && !catalog.homeLatest().isEmpty(), 5000);
    QVERIFY(!readHomeTrace(root2).isEmpty());
    catalog.shutdown(); QTRY_VERIFY_WITH_TIMEOUT(catalog.shutdownReady(), 5000);
    client.stop(); QTRY_VERIFY_WITH_TIMEOUT(!client.retiring(), 5000);
}

static void checkHomeFeedReleasesCancelledReceiptsAndNeverAutoSyncs(const QString &helper) {
    QTemporaryDir fixture; QVERIFY(fixture.isValid());
    const auto root = fixture.filePath("library");
    QVERIFY(QDir().mkpath(root));
    {
        QFile cfg(root + "/home-peer.json"); QVERIFY(cfg.open(QIODevice::WriteOnly));
        cfg.write(QJsonDocument(QJsonObject{{"stale", true}, {"incomplete", true}}).toJson());
    }
    const auto script = writeHomeCatalogPeerScript(fixture.filePath("peer.py"), helper);
    BackendClient client;
    CatalogController catalog(&client);
    client.start(script, {}, root, true);
    QTRY_VERIFY_WITH_TIMEOUT(catalog.available(), 5000);
    catalog.setHomeActive(true);
    QTRY_VERIFY_WITH_TIMEOUT(!catalog.homeLoading() && !catalog.homeLatest().isEmpty() && !catalog.homePopular().isEmpty(), 5000);
    QVERIFY(catalog.homeProvenance().value("stale").toBool());
    QVERIFY(catalog.homeProvenance().value("incomplete").toBool());
    waitEvents(300);

    int begins = 0, releases = 0;
    for (const auto &r : readHomeTrace(root)) {
        if (r["method"].toString() == "native.catalog.begin") {
            ++begins;
            QVERIFY2(r["op"].toObject()["kind"].toString() != "sync", "Home triggered auto-sync on stale cache");
        }
        if (r["method"].toString() == "native.catalog.release") ++releases;
    }
    QCOMPARE(begins, 2);
    QTRY_VERIFY_WITH_TIMEOUT(([&] {
        int rel = 0;
        for (const auto &r : readHomeTrace(root)) if (r["method"].toString() == "native.catalog.release") ++rel;
        return rel == 2;
    })(), 5000);

    // Exhaustion of a Home query must reopen without requiring navigation.
    catalog.setHomeActive(false);
    {
        QFile cfg(root + "/home-peer.json"); QVERIFY(cfg.open(QIODevice::WriteOnly));
        cfg.write(QJsonDocument(QJsonObject{{"exhaustHomeOnce", true}}).toJson());
    }
    catalog.setHomeActive(true);
    QTRY_VERIFY_WITH_TIMEOUT(!catalog.homeLoading() && !catalog.homeUnavailable() && !catalog.homeLatest().isEmpty() && !catalog.homePopular().isEmpty(), 5000);
    QVERIFY(QFile::exists(root + "/home-exhausted-once"));

    // A non-Home request forces a normal session recycle while old shelves exist.
    {
        QFile cfg(root + "/home-peer.json"); QVERIFY(cfg.open(QIODevice::WriteOnly));
        cfg.write(QJsonDocument(QJsonObject{{"exhaustAuthors", true}, {"holdSort", "downloadCount"}, {"incomplete", false}}).toJson());
    }
    catalog.findAuthors("recycle");
    QTRY_VERIFY_WITH_TIMEOUT(([&] {
        int opens=0; for(const auto &r:readHomeTrace(root)) if(r["method"]=="native.catalog.open") ++opens;
        return opens>=3 && !catalog.homeLatest().isEmpty() && catalog.homeLoading();
    })(), 5000);
    QVERIFY(catalog.homePopular().isEmpty());
    QVERIFY(!catalog.homeProvenance().value("incomplete").toBool());
    { QFile f(root + "/release-all"); QVERIFY(f.open(QIODevice::WriteOnly)); }
    QTRY_VERIFY_WITH_TIMEOUT(!catalog.homeLoading(), 5000);
    QVERIFY(QFile::remove(root + "/release-all"));

    // A reread must not pair old Popular rankings with new Latest provenance.
    catalog.setHomeActive(false);
    {
        QFile cfg(root + "/home-peer.json"); QVERIFY(cfg.open(QIODevice::WriteOnly));
        cfg.write(QJsonDocument(QJsonObject{{"failSlot", "downloadCount"}, {"incomplete", false}}).toJson());
    }
    catalog.setHomeActive(true);
    QTRY_VERIFY_WITH_TIMEOUT(!catalog.homeLoading(), 5000);
    QVERIFY(catalog.homeLatest().isEmpty());
    QVERIFY(catalog.homePopular().isEmpty());
    QVERIFY(catalog.homeUnavailable());
    QVERIFY(catalog.homeProvenance().isEmpty());

    // Hold a Home query, leave Home, verify cancel sent and shutdown drains cleanly.
    {
        QFile cfg(root + "/home-peer.json"); QVERIFY(cfg.open(QIODevice::WriteOnly));
        cfg.write(QJsonDocument(QJsonObject{{"holdSort", "releaseDate"}}).toJson());
    }
    catalog.setHideNsfw(false);
    QString cancelledKey;
    QTRY_VERIFY_WITH_TIMEOUT(([&] {
        for (const auto &r : readHomeTrace(root)) {
            if (r["method"].toString() == "native.catalog.begin" && r["op"].toObject()["args"].toObject()["hideNsfw"].toBool() == false) {
                cancelledKey = r["key"].toString();
                return true;
            }
        }
        return false;
    })(), 5000);
    catalog.setHomeActive(false);
    QTRY_VERIFY_WITH_TIMEOUT(([&] {
        for (const auto &r : readHomeTrace(root))
            if (r["method"].toString() == "native.catalog.cancel" && r["key"].toString() == cancelledKey) return true;
        return false;
    })(), 5000);
    catalog.shutdown(); QTRY_VERIFY_WITH_TIMEOUT(catalog.shutdownReady(), 5000);
    client.stop(); QTRY_VERIFY_WITH_TIMEOUT(!client.retiring(), 5000);
}

