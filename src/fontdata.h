#pragma once
#include <QJsonObject>
#include <QHash>
#include <QVariantMap>
#include <QSet>

// Presentation codecs only. Source coordinates and fingerprints grant no I/O authority.
namespace FontData {
bool refs(const QJsonValue &value, QVariantList &out, double &bytes,
          QSet<QString> *archiveIds = nullptr, const QJsonObject &context = {});
bool candidates(const QJsonValue &value, bool diagnostics = false, const QJsonObject &context = {});
bool global(const QJsonValue &value, const QString &game, bool &canUndo,
            QHash<QString, QVariantList> &owners);
bool review(const QJsonValue &value, QVariantMap &out);
bool hex(const QJsonValue &value, int length);
bool safeId(const QJsonValue &value);
bool revision(const QJsonValue &value);
}
