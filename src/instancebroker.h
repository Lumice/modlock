#pragma once
#include <QObject>
#include <QLocalServer>
#include <QLockFile>
#include <memory>
class InstanceBroker final : public QObject {
    Q_OBJECT
public:
    using QObject::QObject;
    enum Result { Primary, Forwarded, Failed };
    Result start(const QString &library, const QStringList &links);
signals:
    void received(const QStringList &links);
private:
    QLocalServer m_server;
    std::unique_ptr<QLockFile> m_lock;
};
