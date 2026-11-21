#pragma once
#include "backendclient.h"

namespace LaunchData {
bool call(BackendClient::Call call);
bool asynchronous(BackendClient::Call call);
bool admitsOwner(BackendClient::Call call);
QString method(BackendClient::Call call);
bool decimal(const QJsonValue &value, bool positive = false);
bool id(const QJsonValue &value);
bool hex(const QJsonValue &value, int length);
bool root(const QJsonValue &value);
bool arguments(const QString &value);
bool strictJson(const QByteArray &bytes);
QByteArray canonical(const QJsonValue &value);
QJsonObject settingsReceipt(const QString &additionalArguments,const QString &expectedRevision);
QString inputHash(BackendClient::Call call, QJsonObject params);
bool decode(BackendClient::Call call, const QJsonValue &value, const QJsonObject &params,
            const QJsonObject &binding, BackendClient::Call receiptCall, const QJsonObject &receiptParams, QVariantMap &out);
bool reviewSummary(const QJsonValue &value, bool changed, const QJsonObject &binding);
bool barrier(const QJsonValue &value, const QString &root);
bool marker(const QJsonObject &value, const QString &root);
}
