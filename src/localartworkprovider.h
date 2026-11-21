#pragma once
#include <QJsonObject>
#include <QQuickAsyncImageProvider>
class LocalArtworkProvider final : public QQuickAsyncImageProvider {
  public:
    LocalArtworkProvider();
    QQuickImageResponse *requestImageResponse(const QString &id, const QSize &requestedSize) override;
    static bool validSource(const QJsonObject &value);
    static bool validOwned(const QJsonObject &value);
    static QString admitSource(const QJsonObject &value);
    static QString admitOwned(const QString &root, const QJsonObject &value);
};
