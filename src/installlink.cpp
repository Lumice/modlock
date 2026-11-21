#include "installlink.h"
#include "networktransport/remoteuri.h"
#include <QRegularExpression>
#include <limits>
bool parseInstallLink(const QString &raw, QVariantMap &submission, QString &fileId) {
    submission.clear(); fileId.clear();
    if (raw.size()>8192) return false;
    const auto colon=raw.indexOf(':');
    if (!QStringList{"dlmm","deadlock-mod-manager","deadlock-modmanager"}.contains(raw.left(colon))) return false;
    auto payload=raw.mid(colon+1);
    if (payload.startsWith("//")) payload.remove(0,2);
    const auto parts=payload.split(',');
    if(parts.size()!=3) return false;
    RemoteWireUri uri;
    if(!remoteWireUri(parts[0].toLatin1(),uri) || QString::fromLatin1(parts[0].toLatin1())!=parts[0])return false;
    static const QRegularExpression filePattern(QStringLiteral("^/mmdl/([1-9][0-9]*)(?:/)?(?:\\?.*)?$"));
    const auto match=filePattern.match(QString::fromLatin1(uri.target));
    const auto type=parts[1].toLower();
    static const QRegularExpression idPattern(QStringLiteral("^[1-9][0-9]*\\z"));
    bool ok=false; const auto id=parts[2].toULongLong(&ok);
    if(!match.hasMatch() || !QStringList{"mod","sound"}.contains(type) || !idPattern.match(parts[2]).hasMatch()
        || !ok || id>std::numeric_limits<quint32>::max())return false;
    bool fileOk=false;match.captured(1).toULongLong(&fileOk);if(!fileOk)return false;
    submission={{"provider","gamebanana"},{"submissionType",type},{"submissionId",parts[2]}};
    fileId=match.captured(1);return true;
}
