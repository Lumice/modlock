#pragma once
#include <QObject>
#include <QByteArray>
#include <QJsonObject>
#include <QJsonValue>
#include <QHash>
#include <QQueue>
#include <QSharedPointer>
#include <QFile>
#include <QNetworkAccessManager>
#include <QPointer>
#include <QTimer>
#include <QElapsedTimer>
#include <QUrl>
class QNetworkReply;
class QSocketNotifier;

// QCoreApplication thread owns network, file handles, timers and both pipe ends.
class NetworkTransport final : public QObject {
    Q_OBJECT
public:
    explicit NetworkTransport(const QString &directory, QObject *parent = nullptr, int ownerFd = -1);
    ~NetworkTransport() override;
    bool valid() const { return m_valid; }
private:
    struct Request {
        QString key, digest, url, phase = QStringLiteral("queued"), token;
        QPointer<QNetworkReply> reply;
        QSharedPointer<QFile> file;
        QPointer<QTimer> deadline, progressTimer;
        qint64 progressBytes = 0;
        qint64 bytes = 0;
        quint64 device = 0, inode = 0;
        int redirects = 0;
        QJsonValue httpStatus, finalUrl, retryAfter, error;
        bool slot = false;
        QUrl pendingUrl;
    };
    void readInput();
    void writeOutput();
    void dispatch(const QByteArray &line);
    void respond(const QJsonValue &id, const QJsonObject &result);
    void reject(const QJsonValue &id, int code, const char *kind, const char *message);
    void enqueue(const QJsonObject &frame);
    void pump();
    void start(const QSharedPointer<Request> &request, const QUrl &url);
    void startReady();
    bool writeInventory();
    void consume(const QSharedPointer<Request> &request);
    void finished(const QSharedPointer<Request> &request);
    void terminate(const QSharedPointer<Request> &request, const char *kind, const char *message);
    void removeBody(const QSharedPointer<Request> &request);
    void shutdown(bool replyPending = false);
    QJsonObject status(const Request &request) const;
    QJsonObject base(const QString &key = {}) const;
    QString m_directory, m_session;
    QFile m_directoryHandle, m_namespaceLease;
    QSharedPointer<QFile> m_inventoryHandle;
    QJsonObject m_inventory;
    quint64 m_inventorySequence = 0;
    QNetworkAccessManager m_network;
    QHash<QString, QSharedPointer<Request>> m_requests;
    QQueue<QString> m_queue, m_httpQueue;
    QQueue<qint64> m_httpStarts;
    QElapsedTimer m_clock;
    QTimer m_startTimer;
    QByteArray m_input, m_output;
    QSocketNotifier *m_reader = nullptr, *m_writer = nullptr;
    int m_slots = 0, m_slotLimit = 0;
    bool m_valid = false, m_closing = false, m_oversize = false, m_storageBlocked = false;
};
