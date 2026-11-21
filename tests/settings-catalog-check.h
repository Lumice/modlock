#pragma once

static void checkSettingsCatalogPolicy() {
    BackendClient client;
    CatalogController catalog(&client);
    QVERIFY(catalog.setProperty("hideNsfw",false));
    QVERIFY(!catalog.queryState().value("hideNsfw").toBool());
    catalog.updateQuery({{"nsfwOnly",true}});
    QVERIFY(catalog.queryState().value("nsfwOnly").toBool());
    catalog.updateQuery({{"kind","collections"}});
    catalog.openCollection("123");
    QVERIFY(catalog.setProperty("hideNsfw",true));
    catalog.back();
    QVERIFY(catalog.queryState().value("hideNsfw").toBool());
    QVERIFY(!catalog.queryState().value("nsfwOnly").toBool());
    catalog.author("123");
    catalog.back();
    QVERIFY(catalog.queryState().value("hideNsfw").toBool());
    catalog.updateQuery({{"hideNsfw",false},{"search","test"}});
    QVERIFY(catalog.queryState().value("hideNsfw").toBool());
}

static void checkSettingsCatalogResetGate(const QString &helper) {
    BackendClient client;
    CatalogController catalog(&client);
    QCOMPARE(catalog.property("canClearCache").toBool(),false);
    QVERIFY(!catalog.property("clearCacheReason").toString().isEmpty());
    catalog.clearCacheAndResync();
    QVERIFY(!catalog.errorSummary().isEmpty());
    QTemporaryDir directory;
    const auto root=directory.filePath("library");
    QProcess initialize;initialize.start(helper,{});QVERIFY(initialize.waitForStarted());
    initialize.write(QJsonDocument(QJsonObject{{"jsonrpc","2.0"},{"id",1},{"method","native.library.initialize"},{"params",QJsonObject{{"version",1},{"dataDir",root},{"operationId",QString(32,'a')},{"expectedRevision","0"}}}}).toJson(QJsonDocument::Compact)+'\n');
    initialize.closeWriteChannel();QVERIFY(initialize.waitForFinished());
    client.start(helper,{},root,true);
    QTRY_VERIFY_WITH_TIMEOUT(catalog.available(),5000);
    const SubmissionKey submission{"mod","123"};
    catalog.requestLibraryMedia("reset-gate",submission,false);
    QElapsedTimer gateWait;gateWait.start();
    while(!catalog.property("canClearCache").toBool() && gateWait.elapsed()<5000){
        if(catalog.libraryMediaSession().isEmpty())catalog.requestLibraryMedia("reset-gate",submission,false);
        waitEvents(1);
    }
    QVERIFY2(catalog.property("canClearCache").toBool(),qPrintable(catalog.clearCacheReason()+"; "+catalog.status()+"; "+catalog.errorSummary()));
    QVERIFY(!catalog.contentReady());
    QVERIFY(catalog.property("clearCacheReason").toString().isEmpty());
    client.stop();
    QTRY_VERIFY_WITH_TIMEOUT(!catalog.property("canClearCache").toBool(),5000);
    QVERIFY(!catalog.property("clearCacheReason").toString().isEmpty());
}
