#pragma once
#include "remoteuri.h"
#include "remoteworker.h"
#include <QElapsedTimer>
#include <QJsonArray>
#include <QJsonObject>
#include <QObject>
#include <QSet>
#include <QSocketNotifier>
#include <QTimer>
#include <memory>

class QSslSocket;
class RemoteTransfer final : public QObject {
public:
    explicit RemoteTransfer(QObject *parent=nullptr);
    ~RemoteTransfer() override;
private:
    enum class Phase { Start, Prefix, Connecting, Headers, Decision, Body, Checkpoint, Finishing, Terminal };
    enum class Framing { Length, Chunked, Close };
    enum class Chunk { Size, Data, DataCrlf, Trailers, Complete };
    void inputReady();
    void outputReady();
    bool enqueueFrame(const char *kind,const QJsonObject &data,bool advisory=false);
    void receive(const QJsonObject &frame);
    bool start(const QJsonObject &frame);
    bool command(const QJsonObject &frame);
    bool validateFds(const QJsonObject &identity,quint64 offset);
    bool redirectTargetAllowed(const QByteArray &fromHost,const QByteArray &toHost) const;
    bool targetAllowed(const RemoteWireUri &uri) const;
    void startRequest(const RemoteWireUri &uri,bool ranged);
    void networkPump();
    bool readMore();
    int line(QByteArray &result,bool control);
    bool statusLine(const QByteArray &line);
    bool fieldLine(const QByteArray &line,bool trailer);
    bool headersComplete();
    bool chunkSize(const QByteArray &line,quint64 &size);
    bool remainingPayload(quint64 &remaining) const;
    void bodyComplete();
    void beginCheckpoint();
    void workerWake();
    void tick();
    void fail(const char *kind);
    void stopQuietly();
    void closeSocket(bool abort);
    void maybeQuit();
    void schedulePump();
    QByteArray input_,output_,network_;
    qsizetype outputPos_=0;
    quint64 inputTotal_=0,outputTotal_=0,chunkOverhead_=0;
    int controlBytes_=0,fieldCount_=0,redirects_=0,status_=0;
    QSocketNotifier inputNotifier_,outputNotifier_;
    QTimer timer_;
    QElapsedTimer absolute_,activity_,checkpointClock_,progressClock_;
    Phase phase_=Phase::Start;
    Framing framing_=Framing::Close;
    Chunk chunk_=Chunk::Size;
    QSslSocket *socket_=nullptr;
    std::unique_ptr<RemoteTransferWorker> worker_;
    QSet<QByteArray> allowedHosts_,deniedHosts_,critical_;
    QJsonArray headers_;
    RemoteWireUri current_,redirect_;
    QByteArray epoch_,ifRange_,uriDigest_,etag_,location_,checkpointHash_,canonicalRedirectTarget_;
    quint64 initialOffset_=0,byteCap_=0,admitted_=0,checkpointOffset_=0,acknowledgedOffset_=0,bodyBytes_=0;
    quint64 device_=0,inode_=0,uid_=0;
    quint64 contentLength_=0,rangeEnd_=0,total_=0,chunkRemaining_=0,lastActivityCount_=0,lastProgress_=0;
    bool ranged_=false,hasLength_=false,hasTotal_=false,hasTransfer_=false,framed_=false,shortRange_=false;
    bool sawStatus_=false,sawCloseNotify_=false,networkEof_=false,terminalSent_=false,pumpScheduled_=false,quiet_=false;
};
