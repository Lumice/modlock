#include "catalogcontroller.h"
#include "librarymodel.h"
#include "libraryrows.h"
#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QQuickItem>
#include <QQuickWindow>
#include <QEventLoop>
#include <QElapsedTimer>
#include <functional>
#include <QTimer>
#include <cstdio>
#include <cstdlib>

static void check(bool ok, const char *message) {
    if (!ok) { std::fprintf(stderr, "FAIL: %s\n", message); std::exit(1); }
}

static void settle() { QEventLoop loop; QTimer::singleShot(500, &loop, &QEventLoop::quit); loop.exec(); }
static bool until(const std::function<bool()> &predicate, int ms = 30000) {
    QElapsedTimer clock; clock.start();
    while (!predicate() && clock.elapsed() < ms) {
        QEventLoop loop; QTimer::singleShot(50, &loop, &QEventLoop::quit); loop.exec();
    }
    return predicate();
}
static QQuickItem *item(QObject *root, const char *name) {
    auto *result = root->findChild<QQuickItem *>(QString::fromLatin1(name));
    check(result, name);
    return result;
}
static void inside(QQuickWindow *window, QQuickItem *control, const char *message) {
    const auto pos = control->mapToScene(QPointF());
    check(control->isVisible() && pos.x() >= -1 && pos.y() >= -1
          && pos.x() + control->width() <= window->width() + 1
          && pos.y() + control->height() <= window->height() + 1, message);
}

int main(int argc, char **argv) {
    QGuiApplication app(argc, argv);
    LibraryModel library;
    auto *rows = static_cast<LibraryRows *>(library.sourceModel());
    auto *catalog = library.catalog();
    const QVariantMap mod{{"provider", "gamebanana"}, {"submissionType", "mod"}, {"submissionId", "42"}};
    const QVariantMap sound{{"provider", "gamebanana"}, {"submissionType", "sound"}, {"submissionId", "42"}};
    check(catalog->libraryAction(mod).value("action") == "download", "missing mod should offer download");
    check(catalog->libraryAction({{"submissionId", "42"}}).value("action") == "unavailable", "invalid identity must be rejected");
    LibraryMod a; a.id = "a"; a.title = "A"; a.remoteSubmission = mod; a.selectionReady = true;
    LibraryMod b; b.id = "b"; b.title = "B"; b.remoteSubmission = sound; b.enabled = true;
    rows->replace({a, b});
    check(catalog->libraryAction(mod).value("action") == "enable", "disabled matching mod should offer enable");
    check(catalog->libraryAction(sound).value("action") == "disable", "type must distinguish equal provider IDs");
    library.setQuery("no matches");
    check(catalog->libraryAction(mod).value("id") == "a", "library filters must not hide the match");
    a.enabled = true; rows->replace({a, b});
    check(catalog->libraryAction(mod).value("action") == "disable", "enabled mod should offer disable");
    rows->setUpdates({QVariantMap{{"modId", "a"}, {"status", "available"}}});
    check(catalog->libraryAction(mod).value("action") == "update", "available update should take precedence");
    b.remoteSubmission = mod; rows->replace({a, b});
    check(catalog->libraryAction(mod).value("action") == "ambiguous", "duplicate matches must not choose a mod");
    check(!catalog->runLibraryAction(mod), "duplicate matches must not run an action");
    std::puts("PASS catalog identity and library actions");

    QQmlApplicationEngine engine;
    engine.rootContext()->setContextProperty("preferencesLocation", QString());
    engine.rootContext()->setContextProperty("libraryDataRoot", QString());
    engine.rootContext()->setContextProperty("buildIdentifier", QString());
    engine.rootContext()->setContextProperty("uiCheckFilePreviews", false);
    engine.loadFromModule("Deadlock.Native", "Main");
    check(!engine.rootObjects().isEmpty(), "main window failed to load");
    auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
    check(window, "main window missing");
    auto *uiLibrary = window->findChild<LibraryModel *>("libraryModel");
    check(uiLibrary, "library model missing");
    uiLibrary->loadDemoData(12);
    QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QString("library")));
    for (const QSize size : {QSize(1280, 800), QSize(720, 520)}) {
        window->resize(size); settle();
        auto *update = item(window, "updateAllButton");
        update->setVisible(true); settle();
        for (auto name : {"categoryChooser", "heroChooser", "enabledFilter", "needsAttentionFilter", "updatesFilter", "updateAllButton"})
            inside(window, item(window, name), name);
    }
    QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QString("browse")));
    for (const QSize size : {QSize(1280, 800), QSize(720, 520)}) {
        window->resize(size); settle();
        for (auto name : {"browseHeroFilter", "browseFavoritesQuick", "browseMoreFilters"})
            inside(window, item(window, name), name);
        item(window, "browseFilterFlow")->setEnabled(true);
        auto *more = item(window, "browseMoreFilters");
        check(QMetaObject::invokeMethod(more, "clicked"), "cannot open filters");
        settle();
        inside(window, item(window, "browseMatureResults"), "filter popup is clipped");
        check(QMetaObject::invokeMethod(more, "clicked"), "cannot close filters");
    }
    std::puts("PASS filter controls within both window sizes");

    if (qEnvironmentVariableIsSet("CATALOG_INSTALL_HELPER")) {
        LibraryModel nativeLibrary;
        nativeLibrary.loadNative(qEnvironmentVariable("CATALOG_INSTALL_HELPER"), qEnvironmentVariable("CATALOG_INSTALL_DATA"));
        if (!until([&]{ return nativeLibrary.remote()->canResolve(); }, 10000)) {
            std::fprintf(stderr, "fixture: error=%s remote=%s loading=%d native=%d\n", qPrintable(nativeLibrary.error()), qPrintable(nativeLibrary.remote()->error()), nativeLibrary.loading(), nativeLibrary.nativeMode());
            check(false, "fixture not ready");
        }
        auto *uiCatalog = nativeLibrary.catalog();
        check(uiCatalog->libraryAction(mod).value("action") == "download", "fresh entry did not offer download");
        check(nativeLibrary.remote()->resolveInstallLink(mod, "12"), "install link was not accepted");
        check(until([&]{ return uiCatalog->libraryAction(mod).value("action") == "progress"; }), "download progress not shown");
        check(until([&]{ return uiCatalog->libraryAction(mod).value("action") == "enable" && uiCatalog->libraryAction(mod).value("enabled").toBool(); }), "download did not reach Enable");
        check(uiCatalog->runLibraryAction(mod), "Enable was rejected");
        check(until([&]{ return uiCatalog->libraryAction(mod).value("action") == "disable"; }), "Enable did not reach Disable");
        std::puts("PASS install link: Download, progress, Enable, Disable");
    }
}
