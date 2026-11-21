#pragma once

#include <QDebug>
#include <QCryptographicHash>
#include <QJsonDocument>
#include <QJsonObject>
#include <sys/stat.h>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QStandardPaths>
#include <QLockFile>
#include <QTemporaryDir>
#include <QTemporaryFile>
#include <QUrl>
#include <QProcess>
#include <QCoreApplication>
#include <QSettings>
#include <QVariantMap>
#include <cerrno>
#include <unistd.h>

// Compatibility identifiers are used only to find existing preferences.
namespace AppPaths {
inline QString legacyOrganization() { return QStringLiteral("lumi"); }
inline QString legacyApplication() { return QStringLiteral("Deadlock Native"); }
// Resolve the config home once, just as preferences does. There is one state
// location, never a fallback or migration of recovery records.
inline QString configHome() {
    static const QString config = [] {
        const auto raw = QDir::cleanPath(QDir(QStandardPaths::writableLocation(QStandardPaths::GenericConfigLocation)).absolutePath());
        const auto canonical = QFileInfo(raw).canonicalFilePath();
        return canonical.isEmpty() ? raw : canonical;
    }();
    return config;
}
inline QString rootSelectionError;
inline QString dataRoot(const QString &selectedHelper = {}) {
    rootSelectionError.clear();
    // One implementation selects the app and helper default. Never reproduce
    // the directory heuristics here: an empty folder is not a library.
    QProcess helper;
    helper.start(selectedHelper.isEmpty() ? QDir(QCoreApplication::applicationDirPath()).filePath("modlock-helper") : selectedHelper, {"--data-root"});
    helper.closeWriteChannel();
    if (!helper.waitForFinished(5000)) {
        if(helper.state()!=QProcess::NotRunning) {helper.kill();helper.waitForFinished(1000);}
        rootSelectionError=QObject::tr("The selected helper couldn't discover the library folder. Pass --data-dir <folder> to choose it explicitly.");
        return {};
    }
    if(helper.exitStatus()!=QProcess::NormalExit || helper.exitCode()!=0) {
        if(helper.exitStatus()==QProcess::NormalExit && helper.exitCode()==3) {
            rootSelectionError=QObject::tr("Modlock can't read its settings folder (%1). Fix the permissions of this folder and everything inside it, then open Modlock again.")
                .arg(QDir(configHome()).filePath("dl-mod-manager"));
            return {};
        }
        rootSelectionError=QString::fromUtf8(helper.readAllStandardError()).trimmed();
        if(!rootSelectionError.isEmpty())rootSelectionError+='\n';
        rootSelectionError+=QObject::tr("Pass --data-dir <folder> to choose the library explicitly.");
        return {};
    }
    const auto root=QJsonDocument::fromJson(helper.readAllStandardOutput()).object().value("root").toString();
    if(root.isEmpty()) rootSelectionError=QObject::tr("The selected helper did not return a library folder. Pass --data-dir <folder> to choose it explicitly.");
    return root;
}
inline QString stateRoot() { return QDir(configHome()).filePath("dl-mod-manager/state"); }
inline QString settingsFolderError() {
    return QObject::tr("Modlock can't use its settings folder (~/.config/dl-mod-manager). Check that it is a normal folder you own.");
}
// lstat distinguishes a missing child from one hidden behind an inaccessible
// parent. Check each parent first, and never traverse a link below config home.
inline bool directoryChain(const QStringList &parts) {
    QString path=configHome();
    for(int i=0;i<=parts.size();++i) {
        if(i)path=QDir(path).filePath(parts[i-1]);
        struct stat st{};
        if(::lstat(QFile::encodeName(path).constData(),&st)!=0) return errno==ENOENT;
        if(!S_ISDIR(st.st_mode) || !(st.st_mode & (S_IXUSR|S_IXGRP|S_IXOTH))
            || ::access(QFile::encodeName(path).constData(),X_OK)!=0) return false;
    }
    return true;
}
// Admission is a process-wide startup decision. A blocked process never gains
// permission after a folder is repaired; restart after fixing the folder.
inline QString stateWriteError() {
    static const QString error = []() -> QString {
        for(const auto &leaf : {QString("pending"),QString("launch-health")})
            if(!directoryChain({"dl-mod-manager","state",leaf}))return settingsFolderError();
        return {};
    }();
    return error;
}
// Create missing state folders as 0700 so a 0002 umask (common on Fedora and
// Debian) doesn't produce group-writable folders the recovery checks reject.
inline bool prepareState() {
    if(!stateWriteError().isEmpty() || !QDir().mkpath(configHome())) return false;
    for(const auto &rel : {QString("dl-mod-manager"),QString("dl-mod-manager/state"),
                           QString("dl-mod-manager/state/pending"),QString("dl-mod-manager/state/launch-health")}) {
        if(::mkdir(QFile::encodeName(QDir(configHome()).filePath(rel)).constData(),0700)!=0 && errno!=EEXIST) return false;
    }
    return true;
}
inline QString qtCacheRoot() {
    // Qt's private shader cache uses CacheLocation (organisation/application).
    return QDir(QStandardPaths::writableLocation(QStandardPaths::GenericCacheLocation))
        .filePath("dl-mod-manager/Deadlock Native");
}
inline QString previewCacheRoot() {
    return QDir(QStandardPaths::writableLocation(QStandardPaths::GenericCacheLocation))
        .filePath("dl-mod-manager");
}
inline QString offlineStorageRoot() {
    // No LocalStorage consumers: reserve only disposable cache storage here.
    return QDir(QStandardPaths::writableLocation(QStandardPaths::GenericCacheLocation))
        .filePath("dl-mod-manager/QML/OfflineStorage");
}
inline QVariantMap preferencesDefaults;
inline QString preferencesSaveNotice;
inline QUrl preferencesLocation() {
    // The config home itself may be a link (dotfile managers do this on
    // purpose); resolve it once. Only the app's folders below it must be real.
    const auto rawConfig = QDir::cleanPath(QDir(QStandardPaths::writableLocation(QStandardPaths::GenericConfigLocation)).absolutePath());
    const auto canonicalConfig = QFileInfo(rawConfig).canonicalFilePath();
    const auto config = canonicalConfig.isEmpty() ? rawConfig : canonicalConfig;
    const auto destination = QDir(config).filePath("modlock/settings.conf");
    const auto previous = QDir(config).filePath("dl-mod-manager/settings.conf");
    const auto predecessorFolder=QDir(config).filePath("dl-mod-manager");
    struct stat predecessorStat{};
    const auto predecessorStatus=::lstat(QFile::encodeName(predecessorFolder).constData(),&predecessorStat);
    const bool predecessorUnavailable=predecessorStatus==0
        ? (!S_ISDIR(predecessorStat.st_mode) || ::access(QFile::encodeName(predecessorFolder).constData(),R_OK|X_OK)!=0)
        : errno!=ENOENT;
    const auto source = predecessorUnavailable || QFileInfo::exists(previous) || QFileInfo(previous).isSymLink() ? previous
        : QDir(config).filePath(legacyOrganization() + "/" + legacyApplication() + ".conf");
    // A preferences path is usable only if every folder from the config home
    // down is a real directory (no symlinks) and the file itself is either a
    // real regular file or absent. Qt would otherwise follow links out of the
    // config folders or block on FIFOs. Preferences are display choices only,
    // so anything unprovable falls back instead of stopping startup.
    const auto folderOk = [&](const QString &file) {
        const auto rel = QDir(config).relativeFilePath(QFileInfo(file).absolutePath());
        QString path = config;
        const QFileInfo base(config);
        // A folder we can't search hides what is inside it, so "absent" would be unproven.
        if (base.exists() && (!base.isDir() || !base.isExecutable())) return false; // absent is fine: created below
        for (const auto &part : rel.split('/', Qt::SkipEmptyParts)) {
            path = QDir(path).filePath(part);
            const QFileInfo f(path);
            if (f.isSymLink() || (f.exists() && (!f.isDir() || !f.isExecutable()))) return false;
        }
        return true;
    };
    // QSettings also opens "<file>.lock"; a FIFO or link there would hang or
    // escape. A leftover lock is fine only if Qt can read its owner (it then
    // clears a dead owner's lock at once); an empty or garbled one would make
    // every save wait about 30 seconds, so use a session file instead.
    const auto lockOk = [](const QString &p) {
        const QFileInfo l(p + ".lock");
        if (l.isSymLink()) return false;
        if (!l.exists()) return true;
        qint64 pid = 0; QString host, app;
        return l.isFile() && QLockFile(l.filePath()).getLockInfo(&pid, &host, &app) && pid > 0;
    };
    const auto regular = [&](const QString &p) { const QFileInfo f(p); return folderOk(p) && f.isFile() && !f.isSymLink() && lockOk(p); };
    const auto absent = [&](const QString &p) { const QFileInfo f(p); return folderOk(p) && !f.exists() && !f.isSymLink() && lockOk(p); };
    const auto session = [&](const char *why) {
        // QSettings cannot create a child of /dev/null, so the QML Settings
        // values remain in memory. The legacy file is read only for defaults.
        preferencesSaveNotice=QObject::tr("Settings couldn't be saved. Changes will last for this session only.");
        if (!predecessorUnavailable && regular(source)) {
            QSettings old(source,QSettings::IniFormat);
            for(const auto &key : {QString("showMaturePreviews"),QString("hideNsfwMods"),QString("automaticApply")})
                if(old.contains("display/"+key)) preferencesDefaults[key]=old.value("display/"+key).toBool();
        }
        qWarning("Settings couldn't be saved (%s). Changes will last for this session only.",why);
        return QUrl::fromLocalFile(QStringLiteral("/dev/null/settings.conf"));
    };
    if (predecessorUnavailable) return session("previous settings folder is inaccessible");
    if (regular(destination)) return QUrl::fromLocalFile(destination);
    if (!absent(destination)) return session("new location is not a plain file in a plain folder");
    if (!regular(source)) {
        // Start fresh only when there is provably nothing to migrate. Anything
        // else at the old path (FIFO, link, unreadable lock) keeps this run
        // session-only so old values are never hidden behind a new file.
        if (!absent(source)) return session("old preferences exist but can't be read safely");
        if (!QDir().mkpath(QFileInfo(destination).absolutePath()) || !absent(destination)) return session("new folder unavailable");
        return QUrl::fromLocalFile(destination);
    }
    // Preserve every INI value (including unknown keys) and the old file.
    QFile old(source);
    if (!old.open(QIODevice::ReadOnly)) return session("old file unreadable");
    const auto bytes = old.readAll();
    if (old.error() != QFileDevice::NoError) return session("old file unreadable");
    const auto folder = QFileInfo(destination).absolutePath();
    if (!QDir().mkpath(folder) || !absent(destination)) return session("new folder unavailable");
    // Write a private temporary file, then publish it with link(), which never
    // replaces a settings file another instance created in the meantime.
    QTemporaryFile staged(QDir(folder).filePath(".settings-XXXXXX.tmp"));
    if (!staged.open() || staged.write(bytes) != bytes.size() || !staged.flush() || ::fsync(staged.handle()) != 0)
        return session("could not write new file");
    if (::link(QFile::encodeName(staged.fileName()).constData(), QFile::encodeName(destination).constData()) != 0) {
        // EEXIST only proves the name exists; use it only if it is a real file.
        if (errno == EEXIST) return regular(destination) ? QUrl::fromLocalFile(destination) : session("new location is not a regular file");
        return session("could not publish new file");
    }
    return QUrl::fromLocalFile(destination); // the temporary name is removed by QTemporaryFile
}
}
