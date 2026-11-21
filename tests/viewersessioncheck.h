#pragma once
#include <QQmlExpression>
#include <QThread>

// Exercise production hosts and dialogs. Only detail payloads, rows and PNGs
// are fixtures; route generations and collection readiness use the controller.
static void startViewerSessionCheck(QQuickWindow *window, bool dataChangeOnly) {
    QTimer::singleShot(0, window, [=] {
        int assertions = 0;
        try {
            const auto check = [&](bool ok, const char *message) { require(ok, message); ++assertions; };
            const auto wait = [&](const std::function<bool()> &condition, const char *message) {
                QElapsedTimer clock; clock.start();
                while (!condition() && clock.elapsed() < 4000) { QCoreApplication::processEvents(QEventLoop::AllEvents, 5); QCoreApplication::sendPostedEvents(nullptr, QEvent::DeferredDelete); QThread::msleep(1); }
                if(!condition()) for(auto *o:objects(window)) if(o->property("revealSeedSource").isValid())
                    std::fprintf(stderr,"DIALOG context=%s visible=%d active=%d opened=%d parent=%s\n",qPrintable(o->property("contextKey").toString()),o->property("visible").toBool(),o->property("active").toBool(),o->property("opened").toBool(),qPrintable(o->parent()?o->parent()->objectName():QString()));
                check(condition(), message);
            };
            const auto settle = [] { QEventLoop loop; QTimer::singleShot(80, &loop, &QEventLoop::quit); loop.exec(); QCoreApplication::sendPostedEvents(nullptr, QEvent::DeferredDelete); };
            auto *library = new LibraryModel(window);
            library->loadNative(qEnvironmentVariable("UI_VIEWER_PEER"), qEnvironmentVariable("UI_VIEWER_ROOT"));
            wait([&] { return !library->loading() && library->nativeMode(); }, "viewer fixture library failed to load");
            auto *focusLibrary = new LibraryModel(window);
            focusLibrary->loadDemoData(30);
            wait([&] { return focusLibrary->demoMode() && focusLibrary->count() == 30; }, "focus fixture failed to load");
            auto *client=new BackendClient(window);
            auto *catalog=new CatalogController(client,window);
            CatalogDecodedResult collectionReply, detailReply;
            QObject::connect(client,&BackendClient::catalogReady,window,[&](BackendClient::Call call,const CatalogDecodedResult &reply) {
                if(call!=BackendClient::Call::CatalogStatus || reply.phase!="running")return;
                if(reply.kind=="collection")collectionReply=reply;
                if(reply.kind=="detail")detailReply=reply;
            });
            client->start(qEnvironmentVariable("UI_VIEWER_PEER"),{},qEnvironmentVariable("UI_VIEWER_ROOT"),true);
            wait([&] { return client->readyForRequests(); },"viewer catalog client failed to load");
            catalog->openCollection("7"); catalog->setActive(true);
            wait([&] { return !collectionReply.key.isEmpty(); }, "collection fixture receipt was not admitted");
            QQmlComponent component(qmlEngine(window), QUrl::fromLocalFile(qEnvironmentVariable("UI_VIEWER_QML")));
            check(component.isReady(), qPrintable(component.errorString()));
            auto *fixture = component.createWithInitialProperties({{"library", QVariant::fromValue(library)}, {"focusLibrary", QVariant::fromValue(focusLibrary)}, {"catalogController",QVariant::fromValue(catalog)},
                {"firstImage", qEnvironmentVariable("UI_PREVIEW_FIRST")}, {"secondImage", qEnvironmentVariable("UI_PREVIEW_SECOND")},
                {"encodedImage", qEnvironmentVariable("UI_PREVIEW_ENCODED")}});
            check(fixture, qPrintable(component.errorString()));
            auto *item = qobject_cast<QQuickItem *>(fixture); item->setParent(window); item->setParentItem(window->contentItem());
            std::function<void()> applyPayload;
            const auto eval = [&](QObject *object, const QString &code) {
                auto *context=qmlContext(object);
                for (auto *child:objects(object)) {
                    auto *candidate=qmlContext(child);
                    if(candidate && candidate->objectForName("root")==object) { context=candidate; break; }
                }
                QQmlExpression expression(context, object, code);
                const auto result = expression.evaluate();
                check(!expression.hasError(), qPrintable(expression.error().toString()));
                if(applyPayload && object==fixture && (code.contains("remoteDetail=") || code.contains("collectionDetail=")))applyPayload();
                return result;
            };
            // Deliver file/image payloads at the decoded-response boundary.
            // The peer holds release replies so another update can exercise the
            // same route/generation without manufacturing a route invalidation.
            applyPayload=[&] {
                auto reply=catalog->route()=="collection"?collectionReply:detailReply;
                const auto payload=fixture->property(catalog->route()=="collection"?"collectionDetail":"remoteDetail");
                reply.detail=payload.value<QJSValue>().toVariant().toMap();
                reply.phase="succeeded";reply.state="ready";reply.preferencesRevision="1";
                reply.submission={"mod","100"};
                client->catalogReady(BackendClient::Call::CatalogStatus,reply);
            };
            applyPayload();
            check(catalog->contentReady(),"decoded collection payload was not published");
            const auto object = [&](QObject *host, const QString &code) { return eval(host, code).value<QObject *>(); };
            const auto popup = [&]() -> QObject * {
                for (auto *o : objects(window))
                    if (o->property("revealSeedSource").isValid() && o->property("visible").toBool()) return o;
                return nullptr;
            };
            const auto opened = [&](QObject *host) {
                wait([&] { auto *p=popup(); return host->property("viewerOpen").toBool() && p && p->property("opened").toBool(); }, "viewer did not open");
                wait([&] { return imageReady(named(window, "mediaViewerPreview")); }, "viewer PNG did not load");
            };
            const auto closed = [&](QObject *host) {
                wait([&] { return !host->property("viewerOpen").toBool() && !popup(); }, qPrintable(QString("invalidated viewer stayed open: host=%1 visible=%2 open=%3 popup=%4").arg(host->objectName()).arg(host->property("visible").toBool()).arg(host->property("viewerOpen").toBool()).arg(popup()!=nullptr))); settle();
            };
            const auto reviewCase = qEnvironmentVariable("UI_VIEWER_REVIEW_CASE");
            // Keep production LibraryPage/LibraryView wiring and a real proxy
            // model for role notifications, keyboard navigation and dialog focus.
            const auto reviewInteractions = [&](bool navigation) {
                fixture->setProperty("host", "library-page");
                auto *page = object(fixture, "libraryPage.item");
                check(page, "narrow LibraryPage did not load");
                auto *libraryView = object(page, "libraryView");
                auto *host = object(libraryView, "results");
                auto *rows = static_cast<LibraryRows *>(focusLibrary->sourceModel());
                int badge = 0;
                const auto notify = [&] {
                    const auto revision = host->property("modelRevision").toInt();
                    const auto structural = host->property("changeRevision").toInt();
                    rows->setUpdates({QVariantMap{{"modId", focusLibrary->idAt(20)}, {"status", ++badge % 2 ? "available" : "current"}}});
                    check(host->property("modelRevision").toInt() > revision, "badge did not refresh model snapshots");
                    check(host->property("modelCoherent").toBool() && host->property("changeRevision").toInt() == structural,
                          "badge unexpectedly changed model structure/coherence");
                };
                if (navigation) for (const auto &mode : {QStringLiteral("list"), QStringLiteral("grid")}) {
                    for (const bool beforeCapture : {true, false}) {
                        if (reviewCase == "pending-capture" && !beforeCapture) continue;
                        if (reviewCase == "captured-navigation" && beforeCapture) continue;
                        page->setProperty("presentationMode", mode);
                        wait([&] { return !host->property("restoring").toBool(); }, "navigation fixture did not settle");
                        auto *view = object(host, "view");
                        qobject_cast<QQuickItem *>(view)->forceActiveFocus();
                        const auto previous = view->property("currentIndex").toInt();
                        const auto selection = focusLibrary->selectedId();
                        host->setProperty("ready", false);
                        // A real geometry change queues a restore which cannot
                        // finish until the readiness gate reopens.
                        page->setProperty("height", page->property("height").toReal() - 1);
                        check(host->property("restoring").toBool(), "geometry did not leave a pending restore");
                        realKey(window, previous == focusLibrary->count()-1 ? Qt::Key_Up : Qt::Key_Down);
                        const auto navigated = view->property("currentIndex").toInt();
                        std::fprintf(stderr,"NAV mode=%s old=%d new=%d selection=%s focus=%d keynav=%d interactive=%d restoring=%d interrupted=%d count=%d\n",qPrintable(mode),previous,navigated,qPrintable(selection),view->property("activeFocus").toBool(),view->property("keyNavigationEnabled").toBool(),view->property("interactive").toBool(),host->property("restoring").toBool(),host->property("userInterrupted").toBool(),view->property("count").toInt());
                        check(navigated != previous && navigated >= 0, "keyboard did not navigate the pending view");
                        check(host->property("userInterrupted").toBool() && focusLibrary->selectedId() == selection,
                              "navigation was not held by the pending restore");
                        if (!beforeCapture) settle();
                        notify();
                        settle();
                        check(host->property("restoring").toBool(), "ready=false did not hold interrupted completion");
                        host->setProperty("ready", true);
                        wait([&] { return !host->property("restoring").toBool(); }, "interrupted restore did not finish");
                        check(view->property("currentIndex").toInt() == navigated && focusLibrary->selectedId() == focusLibrary->idAt(navigated),
                              "harmless badge update discarded pending keyboard navigation");
                        const auto identity = focusLibrary->selectedId();
                        auto *card = object(host, "view.currentItem");
                        check(card && card->property("anchorIdentity").toString() == identity && card->property("selected").toBool(),
                              "navigated card did not become the visible selection");
                        std::printf("PASS pending navigation %s/%s\n", qPrintable(mode), beforeCapture ? "before-capture" : "after-capture");
                    }
                } else {
                    for (const auto &change : {QStringLiteral("none"), QStringLiteral("badge"), QStringLiteral("epoch"), QStringLiteral("filter")}) {
                        // Each route starts with fresh focus scopes; an earlier
                        // successful return must not seed Qt's saved subfocus.
                        fixture->setProperty("host", "results"); settle();
                        fixture->setProperty("host", "library-page");
                        page = object(fixture, "libraryPage.item");
                        libraryView = object(page, "libraryView");
                        host = object(libraryView, "results");
                        wait([&] { return !host->property("restoring").toBool() && object(host,"view.itemAtIndex(0)"); }, "details fixture did not settle");
                        QPointer<QObject> card = object(host, "view.itemAtIndex(0)");
                        QPointer<QObject> title = named(card, "rowTitle");
                        // Double-click the card from search focus. Clicking the
                        // title itself lets Qt's saved subfocus hide a rejected
                        // return-to-title policy by restoring it automatically.
                        realClickItem(window, qobject_cast<QQuickItem *>(named(libraryView,"modSearch")));
                        check(!title->property("activeFocus").toBool(), "title already held focus before card activation");
                        auto *cardItem = qobject_cast<QQuickItem *>(card.data());
                        const auto point = cardItem->mapToScene(QPointF(cardItem->width() * .6, cardItem->height()-4));
                        for (int tap = 0; tap < 2; ++tap) {
                            QMouseEvent press(QEvent::MouseButtonPress,point,point,window->mapToGlobal(point.toPoint()),Qt::LeftButton,Qt::LeftButton,Qt::NoModifier);
                            QMouseEvent release(QEvent::MouseButtonRelease,point,point,window->mapToGlobal(point.toPoint()),Qt::LeftButton,Qt::NoButton,Qt::NoModifier);
                            press.setTimestamp(1000+tap*100); release.setTimestamp(1020+tap*100);
                            QCoreApplication::sendEvent(window,&press); QCoreApplication::sendEvent(window,&release);
                        }
                        wait([&] { return page->property("detailsOpen").toBool(); }, "card double-click did not open narrow details");
                        settle();
                        check(!title->property("activeFocus").toBool(), "details did not take focus from the title");
                        if (change == "badge") notify();
                        else if (change == "epoch") page->setProperty("mediaEpoch", page->property("mediaEpoch").toInt()+1);
                        else if (change == "filter") focusLibrary->setEnabledOnly(true);
                        settle();
                        realKey(window, Qt::Key_Escape);
                        wait([&] { return !page->property("detailsOpen").toBool(); }, "Escape did not close narrow details");
                        settle();
                        std::fprintf(stderr,"FOCUS change=%s title=%d view=%d active=%s\n",qPrintable(change),title && title->property("activeFocus").toBool(),object(host,"view")->property("activeFocus").toBool(),qPrintable(window->activeFocusItem()?window->activeFocusItem()->objectName():QString()));
                        if (change == "none" || change == "badge")
                            check(title && title->property("activeFocus").toBool(), "harmless badge update lost the invoking title focus");
                        else check(object(host,"view")->property("activeFocus").toBool() && (!title || !title->property("activeFocus").toBool()), "invalid details owner did not use results focus fallback");
                        std::printf("PASS narrow details focus %s\n", qPrintable(change));
                        focusLibrary->setEnabledOnly(false);
                    }
                }
                if (navigation) {
                    page->setProperty("presentationMode", "list");
                    for (const auto &change : {QStringLiteral("epoch"), QStringLiteral("filter")}) {
                        wait([&] { return !host->property("restoring").toBool(); }, "rejection fixture did not settle");
                        auto *view = object(host,"view");
                        qobject_cast<QQuickItem *>(view)->forceActiveFocus();
                        // Keep a selected enabled row through the real filter.
                        if (!focusLibrary->selectedMod().value("enabled").toBool()) { realKey(window,Qt::Key_Down); settle(); }
                        const auto identity = focusLibrary->selectedId();
                        const auto previous = view->property("currentIndex").toInt();
                        host->setProperty("ready",false);
                        page->setProperty("height",page->property("height").toReal()-1);
                        realKey(window,Qt::Key_Down);
                        check(view->property("currentIndex").toInt() != previous && host->property("userInterrupted").toBool(), "rejection fixture did not navigate while pending");
                        settle();
                        if (change == "epoch") page->setProperty("mediaEpoch",page->property("mediaEpoch").toInt()+1);
                        else focusLibrary->setEnabledOnly(true);
                        settle();
                        host->setProperty("ready",true);
                        wait([&] { return !host->property("restoring").toBool(); }, "rejected navigation did not settle");
                        check(focusLibrary->selectedId() == identity && view->property("currentIndex").toInt() == focusLibrary->rowForId(identity),
                              "stale navigation survived owner/filter invalidation");
                        std::printf("PASS pending navigation rejects %s\n",qPrintable(change));
                        focusLibrary->setEnabledOnly(false);
                    }
                }
                fixture->setProperty("host", "results");
            };
            if (reviewCase == "details-focus" || reviewCase == "pending-capture" || reviewCase == "captured-navigation") {
                reviewInteractions(reviewCase != "details-focus");
                QObject::disconnect(client,nullptr,window,nullptr); delete fixture;
                std::printf("PASS viewer review %s: %d assertions\n", qPrintable(reviewCase), assertions); QCoreApplication::exit(0); return;
            }
            auto *results = object(fixture, "result");
            if (dataChangeOnly) {
                wait([&] { return !results->property("restoring").toBool(); }, "results did not settle");
                auto *view = object(results, "view");
                view->setProperty("contentY",130.0);results->setProperty("selectedIdentity","row-2");settle();
                auto *card=object(results,"view.itemAtIndex(2)");
                auto *target=named(card,"rowTitle");
                check(QMetaObject::invokeMethod(results,"openViewer",Q_ARG(QVariant,QVariant::fromValue(card)),Q_ARG(QVariant,QVariant::fromValue(target))),"data-change viewer open failed");
                opened(results);
                const auto position = view->property("contentY").toReal();
                eval(results,"stateBank.put(scope, activeContext, {identity:'row-45',index:45,offset:0,focus:'search'})");
                eval(fixture,"rows.setProperty(20,'title','Unrelated changed title')");
                check(results->property("viewerOpen").toBool() && popup(),"nonstructural dataChanged closed an unchanged viewer");
                settle();
                check(std::abs(view->property("contentY").toReal()-position)<1,"dataChanged restored a stale scroll bookmark");
                realKey(window,Qt::Key_Escape);closed(results);
                check(target->property("activeFocus").toBool(),"dataChanged lost invoking-control focus");
                check(results->property("selectedIdentity").toString()=="row-2","dataChanged lost selection");
                // Content changes still invalidate a live viewer snapshot.
                check(QMetaObject::invokeMethod(results,"openViewer",Q_ARG(QVariant,QVariant::fromValue(card)),Q_ARG(QVariant,QVariant::fromValue(target))),"changed-snapshot viewer open failed");
                opened(results);
                eval(fixture,"rows.setProperty(2,'title','Changed viewer owner')");
                closed(results);
                reviewInteractions(true);
                std::printf("PASS viewer data change: %d assertions\n", assertions);
                QObject::disconnect(client,nullptr,window,nullptr); delete fixture; QCoreApplication::exit(0); return;
            }
            if (reviewCase.isEmpty()) reviewInteractions(false);
            if (!qEnvironmentVariableIsSet("UI_VIEWER_DETAIL_ONLY") && (qEnvironmentVariable("UI_VIEWER_HOST").isEmpty() || qEnvironmentVariable("UI_VIEWER_HOST")=="results")) for (const auto &scope : {QStringLiteral("library"), QStringLiteral("browse")}) for (const auto &mode : {QStringLiteral("list"), QStringLiteral("grid")}) {
                fixture->setProperty("scope", scope); fixture->setProperty("mode", mode);
                wait([&] { return !results->property("restoring").toBool() && eval(results,"view.itemAtIndex(0) !== null && view.itemAtIndex(0).preview.ready").toBool(); }, "result card not ready");
                auto *card=object(results,"view.itemAtIndex(0)");
                auto *target=named(card,"rowTitle");
                // Supply an actual visible invoking control in both presentations.
                const auto invoke=[&] {
                    check(QMetaObject::invokeMethod(results,"openViewer",Q_ARG(QVariant,QVariant::fromValue(card)),Q_ARG(QVariant,QVariant::fromValue(target))),"result open failed");
                };
                invoke(); opened(results); realKey(window,Qt::Key_Escape); closed(results);
                check(target->property("activeFocus").toBool(),"result Escape did not return invoking-control focus");
                invoke(); opened(results); click(window,"closeEnlargedPreview"); closed(results);
                check(target->property("activeFocus").toBool(),"result close button lost focus");
                for (const auto &change : {QStringLiteral("rows.setProperty(0, 'source', secondImage)"),
                     QStringLiteral("rows.setProperty(0, 'gallery', JSON.stringify([firstImage]))"),
                     QStringLiteral("rows.setProperty(0, 'hidden', true)"), QStringLiteral("rows.setProperty(0, 'identity', 'different-owner')"),
                     QStringLiteral("result.selectedIdentity = 'row-1'"), QStringLiteral("result.contextKey += 'x'"),
                     QStringLiteral("result.ownerEpoch++"), QStringLiteral("result.ready = false"), QStringLiteral("result.viewerAllowed = false"),
                     QStringLiteral("result.view.itemAtIndex(0).pooled = true"), QStringLiteral("result.view.itemAtIndex(0).mediaToken += 'x'"), QStringLiteral("rows.remove(0)")}) {
                    invoke(); opened(results); eval(fixture,change); closed(results);
                    if(change.contains("ready") || change.contains("viewerAllowed") || change.contains("pooled")) {
                        invoke();settle();check(!results->property("viewerOpen").toBool() && !popup(),"invalid result gate admitted viewer");
                    }
                    if(change.contains("mediaToken") || change.contains("pooled")) check(object(results,"view")->property("activeFocus").toBool(),"invalid result owner did not use list focus fallback");
                    eval(fixture,"result.ready=true; result.viewerAllowed=true; result.contextKey='fixture'; resetRows()");
                    wait([&] { return !results->property("restoring").toBool() && eval(results,"view.itemAtIndex(0) !== null && view.itemAtIndex(0).preview.ready").toBool(); },"result reset failed");
                    card=object(results,"view.itemAtIndex(0)"); target=named(card,"rowTitle");
                }
                invoke(); opened(results); eval(results,"view.contentY = view.contentHeight * .8"); closed(results);
                eval(results,"view.contentY = 0"); settle();
                std::printf("PASS viewer results %s/%s\n",qPrintable(scope),qPrintable(mode));
            }
            for (const auto &kind : {QStringLiteral("local"),QStringLiteral("remote"),QStringLiteral("header"),QStringLiteral("member")}) {
                if(!qEnvironmentVariable("UI_VIEWER_HOST").isEmpty() && qEnvironmentVariable("UI_VIEWER_HOST")!=kind)continue;
                std::fprintf(stderr,"VIEWER host %s starting\n",qPrintable(kind));
                const bool collection=kind=="header"||kind=="member";
                if (kind=="remote") {
                    catalog->openSubmission(SubmissionKey{"mod","100"});
                    wait([&] { return detailReply.generation==QString::number(catalog->viewGeneration()); },"detail receipt not admitted");
                    applyPayload();
                }
                else if (collection && catalog->route()!="collection") {
                    catalog->openCollection("7");
                    wait([&] { return collectionReply.generation==QString::number(catalog->viewGeneration()); },"collection reentry failed");
                    applyPayload();
                }
                QList<CatalogRow> memberRows;
                if (collection) {
                    for(int i=0;i<40;++i) {
                        CatalogRow row;row.identity="mod:"+QString::number(100+i);row.key={"mod",QString::number(100+i)};row.state="ready";row.title="Member";
                        row.thumbnail=qEnvironmentVariable("UI_PREVIEW_FIRST");row.masked=false;
                        row.metadata={{"identity",row.identity},{"title",row.title},{"thumbnail",row.thumbnail},{"masked",false},
                            {"images",QStringList{row.thumbnail,qEnvironmentVariable("UI_PREVIEW_SECOND")}}}; memberRows.append(row);
                    }
                    catalog->model()->replace(memberRows,{},{});
                }
                fixture->setProperty("host",collection?"collection":kind); settle();
                auto *host=object(fixture,"activeHost");
                const QString previewCode=kind=="local"||kind=="remote" ? "preview" : kind=="header" ? "headerPreview" : "members.itemAtIndex(0).preview";
                const QString targetCode=kind=="local"||kind=="remote" ? "largerButton" : kind=="header" ? "headerLarger" : "members.itemAtIndex(0).children[0].children[0].children[0]";
                const auto ready=[&] { return eval(host,kind=="member" ? "members.itemAtIndex(0) !== null && members.itemAtIndex(0).preview.ready" : previewCode+".ready").toBool(); };
                wait(ready,"detail/member preview did not become ready");
                QPointer<QObject> preview=object(host,previewCode);
                QPointer<QObject> target;
                const auto refreshControls=[&] {
                    preview=object(host,previewCode);
                    target=nullptr;
                if(kind=="member") {
                    auto *member=object(host,"members.itemAtIndex(0)");
                    for(auto *o:objects(member))if(o->property("text").toString()=="View larger")target=o;
                } else target=object(host,targetCode);
                };
                refreshControls();
                check(target && target->property("visible").toBool() && target->property("enabled").toBool(),"invoker is not usable");
                const auto invoke=[&] {
                    if(kind=="member") check(QMetaObject::invokeMethod(host,"openMemberViewer",Q_ARG(QVariant,eval(host,"members.itemAtIndex(0)")),Q_ARG(QVariant,QVariant::fromValue(target.data()))),"member open failed");
                    else check(QMetaObject::invokeMethod(host,kind=="header"?"openHeaderViewer":"openViewer",Q_ARG(QVariant,QVariant::fromValue(target.data()))),"detail open failed");
                };
                if (kind == "local") {
                    eval(fixture,"localMod=Object.assign({},localMod,{images:[encodedImage,secondImage],previewHidden:true})");
                    check(eval(host,"String(currentImage) !== String(images[galleryIndex])").toBool(), "encoded fixture did not exercise URL normalization");
                    check(preview->property("masked").toBool(), "encoded local preview was not masked");
                    click(preview,"revealPreview"); wait(ready,"encoded local file was not admitted/revealed");
                    invoke(); opened(host);
                    check(!named(window,"mediaViewerPreview")->property("masked").toBool(), "enlarged encoded file lost its reveal");
                    realKey(window,Qt::Key_Escape); closed(host);
                    check(target->property("activeFocus").toBool(), "encoded reveal close lost invoking focus");
                    eval(fixture,"localMod=Object.assign({},localMod,{images:[firstImage,secondImage],previewHidden:false})");
                    wait(ready,"local preview did not reset after encoded reveal");
                    std::puts("PASS encoded local-file reveal seed");
                }
                invoke(); opened(host); realKey(window,Qt::Key_Right); settle();
                check(host->property("viewerOpen").toBool() && popup()->property("currentIndex").toInt()==1,"viewer navigation invalidated the session");
                if(kind!="member")check(host->property("galleryIndex").toInt()==1,"viewer navigation did not update header/detail gallery");
                realKey(window,Qt::Key_Escape); closed(host); if(kind=="member")refreshControls();
                check(target->property("activeFocus").toBool(),"detail/member Escape lost invoking-control focus");
                if(kind!="member")host->setProperty("galleryIndex",0); wait(ready,"preview reset failed");
                invoke(); opened(host); click(window,"closeEnlargedPreview"); closed(host); if(kind=="member")refreshControls();
                check(target->property("activeFocus").toBool(),"detail/member Close lost invoking-control focus");
                // Invalidate on the mature flag, reject masked opens, and seed
                // only the explicitly revealed opening image. Navigation remasks.
                invoke(); opened(host);
                if(kind=="local")eval(fixture,"localMod=Object.assign({},localMod,{previewHidden:true})");
                else if(kind=="remote")eval(fixture,"remoteDetail={state:'ready',summary:Object.assign({},remoteDetail.summary,{masked:true})}");
                else if(kind=="header")eval(fixture,"collectionDetail={state:'ready',header:Object.assign({},collectionDetail.header,{isNsfw:true})}");
                else { memberRows[0].masked=true;memberRows[0].metadata["masked"]=true;catalog->model()->replace(memberRows,{},{}); }
                closed(host); if(kind=="member")refreshControls();
                check(preview->property("masked").toBool() && named(preview,"previewImage")->property("source").toUrl().isEmpty(),"masked detail requested pixels");
                invoke(); settle(); check(!host->property("viewerOpen").toBool(),"masked detail admitted viewer");
                if(kind=="member") {
                    QObject *show=nullptr;
                    for(auto *o:objects(object(host,"members.itemAtIndex(0)")))if(o->inherits("QQuickAbstractButton") && o->property("text").toString()=="Show preview" && o->property("visible").toBool() && o->property("enabled").toBool())show=o;
                    check(show && show->property("visible").toBool() && show->property("enabled").toBool(),"member reveal control unavailable");
                    check(QMetaObject::invokeMethod(show,"clicked"),"member reveal failed");
                } else click(preview,"revealPreview");
                wait(ready,"explicit reveal did not load PNG");
                invoke(); opened(host);
                realKey(window,Qt::Key_Right); settle();
                auto *enlarged=named(window,"mediaViewerPreview");
                check(enlarged->property("masked").toBool() && named(enlarged,"previewImage")->property("source").toUrl().isEmpty(),"reveal leaked to another gallery image");
                if(kind=="member")preview->setProperty("revealed",false);
                click(enlarged,"revealPreview"); wait([&] { return imageReady(enlarged); },"viewer reveal did not load PNG"); settle();
                if(kind=="member")check(preview->property("masked").toBool(),"nonopening member image revealed the card");
                else check(!preview->property("masked").toBool(),"current viewer reveal did not return to header/detail");
                realKey(window,Qt::Key_Left); settle();
                check(enlarged->property("masked").toBool(),"return navigation retained an earlier reveal");
                realKey(window,Qt::Key_Escape); closed(host); if(kind=="member")refreshControls();
                if(kind=="local")eval(fixture,"localMod=Object.assign({},localMod,{previewHidden:false})");
                else if(kind=="remote")eval(fixture,"remoteDetail={state:'ready',summary:Object.assign({},remoteDetail.summary,{masked:false})}");
                else if(kind=="header")eval(fixture,"collectionDetail={state:'ready',header:Object.assign({},collectionDetail.header,{isNsfw:false})}");
                else { memberRows[0].masked=false;memberRows[0].metadata["masked"]=false;catalog->model()->replace(memberRows,{},{}); }
                wait(ready,"unmasked detail did not recover");
                std::fprintf(stderr,"VIEWER host %s identity\n",qPrintable(kind));
                // Same URL under another owner must invalidate the old dialog.
                invoke(); opened(host);
                if(kind=="local")eval(fixture,"localMod=Object.assign({}, localMod, {id:'local-b'})");
                else if(kind=="remote")eval(fixture,"remoteDetail={state:'ready',summary:Object.assign({},remoteDetail.summary,{identity:'mod:101'})}");
                else if(kind=="header")eval(fixture,"collectionDetail={state:'ready',header:Object.assign({},collectionDetail.header,{collectionId:'8'})}");
                else eval(host,"members.itemAtIndex(0).mediaToken += 'changed'");
                closed(host); if(kind=="member")refreshControls(); wait(ready,"owner-change preview did not recover");
                std::fprintf(stderr,"VIEWER host %s gallery\n",qPrintable(kind));
                invoke(); opened(host);
                if(kind=="local")eval(fixture,"localMod=Object.assign({},localMod,{images:[secondImage]})");
                else if(kind=="remote")eval(fixture,"remoteDetail={state:'ready',summary:Object.assign({},remoteDetail.summary,{images:[secondImage]})}");
                else if(kind=="header")eval(fixture,"collectionDetail={state:'ready',header:Object.assign({},collectionDetail.header,{images:[secondImage]})}");
                else eval(host,"members.currentIndex=1");
                closed(host); if(kind=="member")refreshControls(); wait(ready,"gallery-change preview did not recover");
                std::fprintf(stderr,"VIEWER host %s route\n",qPrintable(kind));
                if(kind=="header") {
                    invoke(); opened(host); host->setProperty("galleryIndex",1); closed(host);
                    host->setProperty("galleryIndex",0); wait(ready,"header gallery reset failed");
                    invoke();opened(host);
                    const auto selection=catalog->selectedIdentity();
                    catalog->remember("mod:101",0,"results",catalog->viewGeneration());catalog->changed();closed(host);
                    catalog->remember(selection,0,"results",catalog->viewGeneration());catalog->changed();
                }
                if(kind=="member") {
                    invoke();opened(host);
                    catalog->model()->dataChanged(catalog->model()->index(0),catalog->model()->index(0),{CatalogModel::Metadata});
                    closed(host);refreshControls();wait(ready,"member revision did not rearm preview");
                }
                invoke(); opened(host);
                if(collection)catalog->setActive(false);
                else if(kind=="remote")catalog->browse();
                else fixture->setProperty("host","none");
                closed(host); if(kind=="member")refreshControls();
                invoke(); settle(); check(!host->property("viewerOpen").toBool(),"inactive host admitted a viewer");
                if(collection)catalog->setActive(true);
                std::printf("PASS viewer host %s\n",qPrintable(kind));
            }
            QObject::disconnect(client,nullptr,window,nullptr);
            delete fixture;
            std::printf("PASS viewer sessions: %d assertions\n",assertions); std::fflush(stdout); QCoreApplication::exit(0);
        } catch(const std::exception &e) {
            std::fprintf(stderr,"FAIL viewer sessions (%d assertions): %s\n",assertions,e.what()); QCoreApplication::exit(1);
        }
    });
}
