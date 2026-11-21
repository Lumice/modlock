#pragma once
#include "audiodecoder.h"
#include "networktransport/remoteuri.h"
#include <QAudioFormat>
#include <QObject>
#include <QPointer>
#include <QSet>
#include <QTimer>
#include <functional>
#include <memory>
#include <vector>

class QNetworkAccessManager;
class QNetworkReply;

// Browse sound previews: checked bounded fetch -> memory decoder -> PCM ->
// lazily created output. No URL or file name ever reaches a media API.
namespace AudioPreview {

constexpr qint64 BodyLimit = 8LL * 1024 * 1024;
constexpr int RedirectLimit = 3;
constexpr int TransferTimeoutMs = 15000;
constexpr int FetchDeadlineMs = 45000;
constexpr qsizetype ChunkFrames = 4096;        // largest PCM block decoded at once
constexpr qsizetype FramesPerPump = 16384;     // decode work per event-loop step
constexpr int PumpIntervalMs = 20;
constexpr int MaximumSeconds = 20 * 60;        // decoded-output ceiling per clip
constexpr qreal SessionVolume = 0.5;

// One GameBanana-only HTTPS GET with manual, re-checked redirects.
class Fetch final : public QObject {
    Q_OBJECT
public:
    explicit Fetch(QNetworkAccessManager *network, QObject *parent = nullptr);
    ~Fetch() override;
    // Returns an error kind when the URL is refused before any request.
    QString start(const QString &url);
    void cancel();
    int redirects() const { return m_redirects; }
signals:
    void finished(const QByteArray &body, const QByteArray &mime);
    void failed(const QString &kind);
private:
    void request(const RemoteWireUri &uri);
    bool admitHeaders();
    void readBody();
    void done();
    void fail(const QString &kind);
    void drop();
    QNetworkAccessManager *m_network;
    QPointer<QNetworkReply> m_reply;
    RemoteWireUri m_current;
    QByteArray m_body, m_mime;
    QTimer m_deadline;
    qint64 m_announced = -1;
    int m_redirects = 0;
    bool m_admitted = false, m_closed = false;
};

class Output {
public:
    virtual ~Output() = default;
    // Returns an error kind ("output" or "outputFormat"), empty on success.
    virtual QString start(const QAudioFormat &format) = 0;
    virtual qsizetype bytesFree() const = 0;
    virtual qsizetype write(const char *data, qsizetype size) = 0;
    virtual void setPaused(bool paused) = 0;
    virtual bool drained() const = 0;
    virtual bool failed() const = 0;
};
using OutputFactory = std::function<std::unique_ptr<Output>()>;
// The only production factory: creates a QAudioSink on the default device.
std::unique_ptr<Output> createSystemOutput();

}

class AudioPreviewController final : public QObject {
    Q_OBJECT
    Q_PROPERTY(QString currentIdentity READ currentIdentity NOTIFY changed)
    Q_PROPERTY(QString state READ state NOTIFY changed)
    Q_PROPERTY(QString errorKind READ errorKind NOTIFY changed)
    Q_PROPERTY(QString message READ message NOTIFY changed)
    Q_PROPERTY(QStringList revealedIdentities READ revealedIdentities NOTIFY changed)
public:
    explicit AudioPreviewController(QObject *parent = nullptr);
    ~AudioPreviewController() override;
    QString currentIdentity() const { return m_identity; }
    QString state() const { return m_state; }
    QString errorKind() const { return m_errorKind; }
    QString message() const { return describe(m_errorKind); }
    QStringList revealedIdentities() const { return QStringList(m_revealed.cbegin(), m_revealed.cend()); }
    // A user's click: start, pause, resume, cancel loading or retry. A
    // concealed (mature or unrated) clip plays only after reveal(identity).
    Q_INVOKABLE void toggle(const QString &identity, const QString &url, bool concealed);
    Q_INVOKABLE void reveal(const QString &identity);
    Q_INVOKABLE void stop();
    // Route, query, page, results, library or maturity change: stop and forget reveals.
    Q_INVOKABLE void invalidate();
    static QString describe(const QString &kind);

    // Test seams (C++ only). The network is not owned.
    void setNetworkAccessManager(QNetworkAccessManager *network);
    void setOutputFactory(AudioPreview::OutputFactory factory);
    void setDecoderLimits(const AudioPreview::DecoderLimits &limits) { m_limits = limits; }
    bool outputCreated() const { return !!m_output; }
    qint64 decodedFrames() const { return m_frames; }
    qsizetype largestChunkBytes() const { return m_largestChunk; }
    qsizetype largestPumpFrames() const { return m_largestPump; }
    qsizetype decoderWorkingBytes() const { return m_decoder ? m_decoder->workingBytes() : 0; }
    AudioPreview::Fetch *activeFetch() const { return m_fetch; }
    void pumpNow() { pump(); }
signals:
    void changed();
private:
    void begin(const QString &identity, const QString &url);
    void fetched(quint64 generation, const QByteArray &body, const QByteArray &mime);
    void fail(const QString &kind);
    void pump();
    void release();
    void setPhase(const QString &state, const QString &errorKind = {});
    QNetworkAccessManager *network();
    quint64 m_generation = 0;
    QString m_identity, m_url, m_state = QStringLiteral("idle"), m_errorKind;
    QSet<QString> m_revealed;
    QNetworkAccessManager *m_network = nullptr;
    std::unique_ptr<QNetworkAccessManager> m_ownNetwork;
    AudioPreview::OutputFactory m_factory;
    AudioPreview::DecoderLimits m_limits;
    AudioPreview::Fetch *m_fetch = nullptr;
    std::unique_ptr<AudioPreview::PcmDecoder> m_decoder;
    std::unique_ptr<AudioPreview::Output> m_output;
    QTimer m_pump;
    QByteArray m_pending;
    std::vector<qint16> m_chunk;
    qint64 m_frames = 0, m_frameLimit = 0;
    qsizetype m_largestChunk = 0, m_largestPump = 0;
    int m_frameBytes = 0;
    bool m_decoderEnded = false;
};
