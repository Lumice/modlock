#pragma once
#include <QByteArray>
#include <QObject>
#include <QCryptographicHash>
#include <condition_variable>
#include <deque>
#include <functional>
#include <mutex>
#include <thread>

// The queue charge includes the chunk currently inside pwrite(). No payload
// bytes are carried by queued Qt events.
class RemoteTransferWorker {
public:
    struct State {
        quint64 written=0;
        quint64 activity=0;
        qsizetype charged=0;
        bool ready=false;
        bool checkpointReady=false;
        bool done=false;
        bool failed=false;
        bool identityFailed=false;
        QByteArray checkpointHash;
        QByteArray md5;
        QByteArray sha256;
    };
    RemoteTransferWorker(QObject *receiver, std::function<void()> wake, quint64 offset,
                         quint64 device, quint64 inode, quint64 uid);
    ~RemoteTransferWorker();
    bool enqueue(QByteArray bytes);
    void checkpoint();
    void acknowledge();
    void finish();
    void stop();
    State state();
private:
    void run();
    bool validateDescriptors();
    void notifyLocked();
    bool readPrefix(QCryptographicHash &hash, quint64 bytes);
    bool finalHash(quint64 bytes, const QByteArray &expected, QByteArray &md5, QByteArray &sha);
    QObject *receiver_;
    std::function<void()> wake_;
    std::thread thread_;
    std::mutex mutex_;
    std::condition_variable condition_;
    std::deque<QByteArray> queue_;
    State state_;
    quint64 initial_;
    quint64 device_,inode_,uid_;
    bool notified_=false;
    bool stop_=false;
    bool checkpoint_=false;
    bool waitingAck_=false;
    bool finish_=false;
};
