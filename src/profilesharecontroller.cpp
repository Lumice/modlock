#include "profilesharecontroller.h"
#include "librarymodel.h"
#include "remotecontroller.h"
#include <QCoreApplication>
#include <QGuiApplication>
#include <QClipboard>
#include <QProcess>
#include <QJsonDocument>
#include <QJsonObject>
#include <QUuid>
#include <memory>
#include <QFile>
LibraryModel *ProfileShareController::library() const { return m_library; }
void ProfileShareController::setLibrary(LibraryModel *l) {
    if (m_library == l) return;
    if (m_library) disconnect(m_library, nullptr, this, nullptr);
    m_library = l;
    if (l) connect(l, &LibraryModel::nativeChanged, this, [this] {
        if (m_library && !m_reviewRevision.isEmpty() && m_reviewRevision != m_library->revision()) invalidate();
    });
    invalidate();
}
void ProfileShareController::invalidate() { m_result.clear(); m_reviewCode.clear(); m_reviewRevision.clear(); emit changed(); }
void ProfileShareController::exportProfile(const QString &id, bool names) { request("export", {{"profileId", id}, {"includeNames", names}}); }
void ProfileShareController::review(const QString &code) { m_reviewCode.clear(); request("review", {{"code", code}}); }
void ProfileShareController::create(const QString &code, const QString &name) {
    if (!m_library || !m_library->canEditProfiles() || code != m_reviewCode || m_reviewRevision != m_library->revision() || !m_result.value("canCreate").toBool()) return;
    request("create", {{"code", code}, {"name", name}, {"expectedRevision", m_reviewRevision}, {"operationId", QUuid::createUuid().toString(QUuid::Id128)}});
}
void ProfileShareController::copyCode() { QGuiApplication::clipboard()->setText(m_result.value("code").toString()); }
QString ProfileShareController::readFile(const QUrl &url) {
    QFile file(url.toLocalFile());
    if (!url.isLocalFile() || !file.open(QIODevice::ReadOnly) || file.size() > 16384) {
        m_error = tr("Choose a profile file of at most 16 KiB."); emit changed(); return {};
    }
    return QString::fromUtf8(file.read(16385));
}
bool ProfileShareController::download(const QVariantMap &s, const QString &file) { return m_library && !m_busy && m_library->remote()->resolveInstallLink(s, file == "latest" ? QString() : file); }
void ProfileShareController::request(const QString &action, QVariantMap params) {
    if (!m_library || m_busy || !m_library->nativeMode() || m_library->busy() || m_library->loading()) return;
    m_error.clear(); m_result.clear();
    if(!m_library->helperReady()){emit changed();return;}
    if (params.value("code").toString().toUtf8().size() > 16384) { m_error = tr("The shared profile exceeds 16 KiB."); emit changed(); return; }
    const auto root = m_library->dataDir(); const auto epoch = m_library->loadEpoch();
    const auto helper = m_library->helperPath();
    const auto revision = m_library->revision(); const auto code = params.value("code").toString();
    params.insert("dataDir", root);
    const auto wire = QJsonDocument(QJsonObject{{"jsonrpc", "2.0"}, {"id", 1}, {"method", "native.profileShare." + action}, {"params", QJsonObject::fromVariantMap(params)}}).toJson(QJsonDocument::Compact) + '\n';
    if (wire.size() > 65536) { m_error = tr("The shared profile is too large."); emit changed(); return; }
    m_busy = true; emit changed();
    // Application ownership keeps an admitted metadata transaction alive if its dialog closes.
    auto *process = new QProcess(QCoreApplication::instance());
    auto bytes = std::make_shared<QByteArray>();
    QPointer<ProfileShareController> self(this); QPointer<LibraryModel> library(m_library);
    connect(process, &QProcess::readyReadStandardOutput, process, [process, bytes] {
        const auto chunk = process->readAllStandardOutput();
        if (bytes->size() <= 32 * 1024 * 1024) bytes->append(chunk);
    });
    connect(process, &QProcess::readyReadStandardError, process, [process] { process->readAllStandardError(); });
    auto done = [self, library, process, bytes, root, epoch, revision, action, code] {
        process->deleteLater();
        const bool current = library && library->dataDir() == root && library->loadEpoch() == epoch;
        const auto response = QJsonDocument::fromJson(*bytes).object();
        const bool success = process->exitStatus() == QProcess::NormalExit && process->exitCode() == 0 && response.value("id").toInt() == 1 && response.contains("result") && bytes->size() <= 32 * 1024 * 1024;
        if (action == "create" && current) library->refresh();
        if (!self) return;
        self->m_busy = false;
        if (!current) { self->m_error = tr("The library changed. Open sharing again."); emit self->changed(); return; }
        if (!success) self->m_error = response.value("error").toObject().value("message").toString(tr("The helper did not return a result. Refresh the library before retrying."));
        else if (action != "create" && library->revision() != revision) self->m_error = tr("The library changed. Review again.");
        else { self->m_result = response.value("result").toObject().toVariantMap();
            self->m_reviewRevision = revision;
            if (action == "review") self->m_reviewCode = code;
        }
        emit self->changed();
        if (self && success && action == "create") emit self->created();
    };
    connect(process, qOverload<int, QProcess::ExitStatus>(&QProcess::finished), process, [done](int, QProcess::ExitStatus) { done(); });
    connect(process, &QProcess::errorOccurred, process, [done](QProcess::ProcessError e) { if (e == QProcess::FailedToStart) done(); });
    connect(process, &QProcess::started, process, [process, wire] { process->write(wire); process->closeWriteChannel(); });
    process->start(helper, {});
}
