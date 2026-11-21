#pragma once
#include <QVariantMap>
#include <QString>
// GameBanana legacy one-click wire format, also used by upstream DMM (GPL-3.0).
bool parseInstallLink(const QString &raw, QVariantMap &submission, QString &fileId);
