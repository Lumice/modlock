#pragma once

#include "backendclient.h"
#include <QJsonObject>
#include <QVariantMap>

struct RemoteReply {
    QVariantMap value;
};

namespace RemoteData {
bool call(BackendClient::Call call);
QString method(BackendClient::Call call);
bool submission(const QJsonValue &value);
bool request(BackendClient::Call call, const QJsonObject &params);
bool marker(const QJsonObject &value, const QString &root);
bool replacementMarker(const QJsonObject &value, const QString &root);
QJsonObject replacementInput(const QJsonObject &marker);
bool recoveryInspection(const QJsonValue &value, const QJsonObject &request, QVariantMap &out);
bool decode(BackendClient::Call call, const QJsonValue &value, const QJsonObject &request, RemoteReply &out);
}
