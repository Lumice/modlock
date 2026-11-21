#pragma once
// Test doubles for Browse sound previews. FakeNetwork/FakeReply stand in for
// the transport only: the production AudioPreview::Fetch still issues the
// request and applies every header, redirect, length and body rule. FakeOutput
// replaces QAudioSink, so no audio device is ever opened.
#include "audiopreview.h"
#include <QAuthenticator>
#include <QNetworkAccessManager>
#include <QNetworkReply>
#include <QPointer>
#include <QSslError>
#include <cstring>
#include <memory>

class FakeReply final : public QNetworkReply {
public:
    FakeReply(const QNetworkRequest &request, QObject *parent) : QNetworkReply(parent) {
        setRequest(request);
        setUrl(request.url());
        setOperation(QNetworkAccessManager::GetOperation);
        open(QIODevice::ReadOnly | QIODevice::Unbuffered);
    }
    void headers(int status, const QList<QPair<QByteArray, QByteArray>> &raw) {
        setAttribute(QNetworkRequest::HttpStatusCodeAttribute, status);
        for (const auto &header : raw) setRawHeader(header.first, header.second);
        emit metaDataChanged();
    }
    void chunk(const QByteArray &bytes) { m_buffer += bytes; delivered += bytes.size(); emit readyRead(); }
    void complete(NetworkError error = NoError) {
        if (isFinished()) return;
        if (error != NoError) { setError(error, QStringLiteral("fake")); emit errorOccurred(error); }
        setFinished(true);
        emit finished();
    }
    void serve(int status, const QByteArray &mime, const QByteArray &body, bool announce = true) {
        QList<QPair<QByteArray, QByteArray>> raw{{"Content-Type", mime}};
        if (announce) raw.append({"Content-Length", QByteArray::number(body.size())});
        headers(status, raw);
        for (qsizetype at = 0; at < body.size() && !isFinished(); at += 65536) chunk(body.mid(at, 65536));
        complete();
    }
    void redirect(int status, const QByteArray &location) { headers(status, {{"Location", location}}); complete(); }
#if QT_CONFIG(ssl)
    void tlsFailure() { emit sslErrors({QSslError(QSslError::UnableToGetLocalIssuerCertificate)}); complete(SslHandshakeFailedError); }
#endif
    void abort() override {
        aborted = true;
        if (!isFinished()) { setError(OperationCanceledError, QStringLiteral("aborted")); setFinished(true); emit finished(); }
    }
    void ignoreSslErrors() override { ignoredSsl = true; }
    qint64 bytesAvailable() const override { return m_buffer.size() + QNetworkReply::bytesAvailable(); }
    bool isSequential() const override { return true; }
    bool aborted = false, ignoredSsl = false;
    qint64 delivered = 0;
protected:
    qint64 readData(char *data, qint64 max) override {
        const auto n = qMin<qint64>(max, m_buffer.size());
        std::memcpy(data, m_buffer.constData(), size_t(n));
        m_buffer.remove(0, n);
        return n;
    }
private:
    QByteArray m_buffer;
};

class FakeNetwork final : public QNetworkAccessManager {
public:
    using QNetworkAccessManager::QNetworkAccessManager;
    QList<QNetworkRequest> requests;
    QList<QPointer<FakeReply>> replies;
    FakeReply *last() const { return replies.isEmpty() ? nullptr : replies.last().data(); }
    void challenge(FakeReply *reply, QAuthenticator *authenticator) { emit authenticationRequired(reply, authenticator); }
protected:
    QNetworkReply *createRequest(Operation, const QNetworkRequest &request, QIODevice *) override {
        requests.append(request);
        auto *reply = new FakeReply(request, this);
        replies.append(reply);
        return reply;
    }
};

struct FakeOutputState {
    int created = 0, started = 0, destroyed = 0, writesWhilePaused = 0;
    QAudioFormat format;
    qsizetype capacity = 16384, free = 16384, largestWrite = 0;
    qint64 written = 0, nonZeroBytes = 0;
    bool paused = false, failed = false;
    QList<bool> pauseCalls;
    QString startError;
    void drain() { free = capacity; }
};

class FakeOutput final : public AudioPreview::Output {
public:
    explicit FakeOutput(std::shared_ptr<FakeOutputState> state) : m_state(std::move(state)) { ++m_state->created; }
    ~FakeOutput() override { ++m_state->destroyed; }
    QString start(const QAudioFormat &format) override { m_state->format = format; ++m_state->started; return m_state->startError; }
    qsizetype bytesFree() const override { return m_state->free; }
    qsizetype write(const char *data, qsizetype size) override {
        if (m_state->paused) ++m_state->writesWhilePaused;
        size = qMin(size, m_state->free);
        for (qsizetype i = 0; i < size; ++i) if (data[i]) ++m_state->nonZeroBytes;
        m_state->free -= size;
        m_state->written += size;
        m_state->largestWrite = qMax(m_state->largestWrite, size);
        return size;
    }
    void setPaused(bool paused) override { m_state->paused = paused; m_state->pauseCalls.append(paused); }
    bool drained() const override { return m_state->free == m_state->capacity; }
    bool failed() const override { return m_state->failed; }
private:
    std::shared_ptr<FakeOutputState> m_state;
};

inline AudioPreview::OutputFactory fakeOutputFactory(const std::shared_ptr<FakeOutputState> &state) {
    return [state] { return std::unique_ptr<AudioPreview::Output>(new FakeOutput(state)); };
}
