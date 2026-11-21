#pragma once
#include <QByteArray>
#include <QJsonObject>
#include <QSet>

struct RemoteWireUri {
    QByteArray effective;
    QByteArray host;
    QByteArray authority;
    QByteArray target;
};
bool remoteWireUri(const QByteArray &input, RemoteWireUri &uri);
bool remoteResolveLocation(const RemoteWireUri &base, const QByteArray &location, RemoteWireUri &uri);
bool remoteCanonicalRedirectOrigin(const QByteArray &host);
bool remoteCanonicalCacheHost(const QByteArray &host);
bool remoteStrongEtag(const QByteArray &value);
bool remoteDecimal(const QJsonValue &value, quint64 &number);
bool remoteHex(const QByteArray &value, int length);
bool remoteParseFrame(const QByteArray &frame, QJsonObject &object);
bool remoteExactKeys(const QJsonObject &object, std::initializer_list<const char *> keys);
