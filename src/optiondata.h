#pragma once
#include "backendclient.h"
namespace OptionData {
bool optionsCall(BackendClient::Call call);
bool importCall(BackendClient::Call call);
bool localCall(BackendClient::Call call);
bool decodeReply(const QJsonValue &value, QVariantMap &out);
bool validPerformanceChanges(const QJsonValue &value);
bool decodeApplyReview(const QJsonValue &value, QVariantMap &out);
}
