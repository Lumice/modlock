// Browse sound preview checks: production URL policy, AudioPreview::Fetch
// transitions (through fake QNetworkReply events), memory decoders and the
// controller state machine with a fake output. No QAudioSink, QMediaDevices,
// audio device, live network or provider content is used.
#include "audiofakes.h"
#include <QCoreApplication>
#include <QFile>
#include <cstdio>
#include <functional>
#include <stdexcept>

using namespace AudioPreview;

namespace {

void require(bool ok, const char *message) { if (!ok) throw std::runtime_error(message); }

QByteArray fixture(const char *name) {
    QFile file(QStringLiteral(AUDIO_FIXTURES "/") + QLatin1String(name));
    require(file.open(QIODevice::ReadOnly), "fixture missing");
    return file.readAll();
}

void settle() {
    QCoreApplication::processEvents();
    QCoreApplication::sendPostedEvents(nullptr, QEvent::DeferredDelete);
    QCoreApplication::processEvents();
}

const QString A = QStringLiteral("gamebanana:sound:101");
const QString B = QStringLiteral("gamebanana:sound:102");
const QString UrlA = QStringLiteral("https://files.gamebanana.com/preview/sounds/silence-a.mp3");
const QString UrlB = QStringLiteral("https://files.gamebanana.com/preview/sounds/silence-b.ogg");

struct Rig {
    FakeNetwork network;
    std::shared_ptr<FakeOutputState> output = std::make_shared<FakeOutputState>();
    AudioPreviewController controller;
    Rig() {
        controller.setNetworkAccessManager(&network);
        controller.setOutputFactory(fakeOutputFactory(output));
    }
    FakeReply *play(const QString &identity, const QString &url, bool concealed = false) {
        const auto before = network.requests.size();
        controller.toggle(identity, url, concealed);
        return network.requests.size() > before ? network.last() : nullptr;
    }
    void runToEnd(int guard = 100000) {
        while (controller.state() == "playing" && guard-- > 0) { output->drain(); controller.pumpNow(); }
        require(guard > 0, "clip never ended");
    }
};

// Builds the first page of an Ogg stream whose payload starts with magic.
QByteArray oggPage(const QByteArray &payload) {
    QByteArray page("OggS", 4);
    page += char(0); page += char(0x02);
    page += QByteArray(20, '\0');
    page += char(1); page += char(payload.size());
    return page + payload;
}

void policyRefusals() {
    for (const auto &url : {QStringLiteral("https://evil.example/a.mp3"), QStringLiteral("http://files.gamebanana.com/preview/sounds/a.mp3"),
                            QStringLiteral("https://user@files.gamebanana.com/a.mp3"), QStringLiteral("https://files.gamebanana.com:8443/a.mp3"),
                            QStringLiteral("https://gamebanana.com.evil.example/a.mp3"), QStringLiteral("file:///etc/passwd")}) {
        Rig rig;
        require(!rig.play(A, url), "refused initial URL still issued a request");
        require(rig.network.requests.isEmpty(), "refused initial URL reached the network");
        require(rig.controller.state() == "error" && rig.controller.errorKind() == "policy", "initial URL refusal not reported as policy");
        require(rig.output->created == 0, "refused URL created output");
    }
}

void requestHygiene() {
    Rig rig;
    auto *reply = rig.play(A, UrlA);
    require(reply, "allowed URL issued no request");
    require(reply->readBufferSize() == 64 * 1024, "Qt reply receive buffer is not bounded to 64 KiB");
    const auto &request = rig.network.requests.first();
    require(request.url() == QUrl(UrlA), "request URL changed");
    require(request.attribute(QNetworkRequest::RedirectPolicyAttribute).toInt() == QNetworkRequest::ManualRedirectPolicy, "redirects not manual");
    require(request.maximumRedirectsAllowed() == 0, "automatic redirects allowed");
    require(request.attribute(QNetworkRequest::CookieLoadControlAttribute).toInt() == QNetworkRequest::Manual
            && request.attribute(QNetworkRequest::CookieSaveControlAttribute).toInt() == QNetworkRequest::Manual, "cookies not disabled");
    require(request.attribute(QNetworkRequest::CacheLoadControlAttribute).toInt() == QNetworkRequest::AlwaysNetwork
            && !request.attribute(QNetworkRequest::CacheSaveControlAttribute).toBool(), "disk cache not disabled");
    require(request.attribute(QNetworkRequest::AuthenticationReuseAttribute).toInt() == QNetworkRequest::Manual, "credential reuse allowed");
    require(request.transferTimeoutAsDuration().count() == TransferTimeoutMs, "transfer timeout missing");
    require(request.rawHeader("Accept-Encoding") == "identity", "compressed bodies would bypass the received-byte count");
    require(!request.hasRawHeader("Cookie") && !request.hasRawHeader("Authorization") && !request.hasRawHeader("Referer"), "credentials or referer sent");
    require(rig.controller.state() == "loading" && rig.output->created == 0, "output created before decoding");
}

void redirects() {
    { // Off-host Location after an allowed first URL.
        Rig rig;
        auto *first = rig.play(A, UrlA);
        first->redirect(302, "https://evil.example/a.mp3");
        require(rig.network.requests.size() == 1, "off-host redirect was followed");
        require(rig.controller.errorKind() == "policy" && first->aborted, "off-host redirect not refused");
    }
    { // Plain HTTP Location.
        Rig rig;
        rig.play(A, UrlA)->redirect(301, "http://files.gamebanana.com/preview/sounds/a.mp3");
        require(rig.network.requests.size() == 1 && rig.controller.errorKind() == "policy", "HTTP redirect not refused");
    }
    { // Three GameBanana redirects (absolute, relative, protocol-relative) then success.
        Rig rig;
        rig.play(A, UrlA)->redirect(302, "https://gamebanana.com/preview/sounds/r1.mp3");
        require(rig.network.last()->readBufferSize() == 64 * 1024, "redirect reply receive buffer is not bounded");
        require(rig.network.requests.size() == 2 && rig.network.requests[1].url().host() == "gamebanana.com", "redirect 1 not followed");
        rig.network.last()->redirect(307, "/preview/sounds/r2.mp3");
        require(rig.network.requests[2].url().toString() == "https://gamebanana.com/preview/sounds/r2.mp3", "relative redirect not resolved");
        rig.network.last()->redirect(308, "//files.gamebanana.com/preview/sounds/r3.mp3");
        require(rig.network.requests.size() == 4, "redirect 3 not followed");
        rig.network.last()->serve(200, "audio/mpeg", fixture("silence-250ms.mp3"));
        require(rig.controller.state() == "playing", "three redirects then audio did not play");
    }
    { // A fourth redirect is refused without a fifth request.
        Rig rig;
        rig.play(A, UrlA);
        for (int i = 0; i < 4; ++i) rig.network.last()->redirect(302, "/preview/sounds/loop.mp3");
        require(rig.network.requests.size() == 4, "redirect limit not enforced");
        require(rig.controller.errorKind() == "redirects", "redirect limit not reported");
    }
}

void lengthsAndBody() {
    { // Announced length above the cap: refused before any body is read.
        Rig rig;
        auto *reply = rig.play(A, UrlA);
        reply->headers(200, {{"Content-Type", "audio/mpeg"}, {"Content-Length", QByteArray::number(BodyLimit + 1)}});
        require(rig.controller.errorKind() == "tooLarge" && reply->aborted, "oversized announced length accepted");
    }
    { // No length, streamed body above the cap: aborted once the cap is crossed.
        Rig rig;
        auto *reply = rig.play(A, UrlA);
        reply->headers(200, {{"Content-Type", "audio/mpeg"}});
        const QByteArray block(1 << 20, '\0');
        for (int i = 0; i < 12 && !reply->aborted; ++i) reply->chunk(block);
        require(rig.controller.errorKind() == "tooLarge" && reply->aborted, "oversized stream accepted");
        require(reply->delivered <= BodyLimit + (1 << 20), "stream read far past the cap");
    }
    { // A lying length that is too small: more bytes than announced.
        Rig rig;
        auto *reply = rig.play(A, UrlA);
        reply->headers(200, {{"Content-Type", "audio/mpeg"}, {"Content-Length", "100"}});
        reply->chunk(QByteArray(200, '\0'));
        require(rig.controller.errorKind() == "damaged" && reply->aborted, "body longer than Content-Length accepted");
    }
    { // A lying length that is too large: truncated transfer.
        Rig rig;
        auto *reply = rig.play(A, UrlA);
        const auto mp3 = fixture("silence-250ms.mp3");
        reply->headers(200, {{"Content-Type", "audio/mpeg"}, {"Content-Length", QByteArray::number(mp3.size() + 10)}});
        reply->chunk(mp3);
        reply->complete();
        require(rig.controller.errorKind() == "damaged", "truncated body accepted");
    }
    for (const QByteArray &bad : {QByteArray("12abc"), QByteArray("-5"), QByteArray("0"), QByteArray("100, 100"), QByteArray("99999999999999999999")}) {
        Rig rig;
        auto *reply = rig.play(A, UrlA);
        reply->headers(200, {{"Content-Type", "audio/mpeg"}, {"Content-Length", bad}});
        require(rig.controller.state() == "error" && reply->aborted, "invalid Content-Length accepted");
    }
    { // Missing length with a valid body plays.
        Rig rig;
        rig.play(A, UrlA)->serve(200, "audio/mpeg", fixture("silence-250ms.mp3"), false);
        require(rig.controller.state() == "playing", "valid body without Content-Length refused");
    }
    { // Non-200 status and compressed bodies.
        Rig rig;
        rig.play(A, UrlA)->serve(404, "text/html", "<html>missing</html>");
        require(rig.controller.errorKind() == "network", "404 not refused");
        auto *reply = rig.play(A, UrlA);
        reply->headers(200, {{"Content-Type", "audio/mpeg"}, {"Content-Encoding", "gzip"}});
        require(rig.controller.errorKind() == "unsupported" && reply->aborted, "compressed body accepted");
    }
}

void transportFailures() {
#if QT_CONFIG(ssl)
    {
        Rig rig;
        auto *reply = rig.play(A, UrlA);
        reply->tlsFailure();
        require(rig.controller.errorKind() == "network" && !reply->ignoredSsl, "TLS failure ignored or not reported");
    }
#endif
    {
        Rig rig;
        auto *reply = rig.play(A, UrlA);
        QAuthenticator authenticator;
        rig.network.challenge(reply, &authenticator);
        require(rig.controller.errorKind() == "network" && reply->aborted, "authentication challenge not refused");
        require(authenticator.user().isEmpty() && authenticator.password().isEmpty(), "credentials supplied");
    }
    {
        Rig rig;
        auto *reply = rig.play(A, UrlA);
        reply->chunk("early");
        require(rig.controller.errorKind() == "network" && reply->aborted, "body before status accepted");
    }
}

void formats() {
    const auto mp3 = fixture("silence-250ms.mp3");
    const auto ogg = fixture("silence-250ms.ogg");
    struct Case { const char *name; QByteArray mime, body; const char *kind; };
    const QList<Case> cases{
        {"html as audio", "audio/mpeg", "<!DOCTYPE html><html><body>Not audio</body></html>", "unsupported"},
        {"text type", "text/html", mp3, "unsupported"},
        {"missing type", "", mp3, "unsupported"},
        {"playlist", "audio/mpeg", "#EXTM3U\n#EXTINF:1,x\nhttps://files.gamebanana.com/preview/sounds/a.mp3\n", "unsupported"},
        {"opus", "application/octet-stream", oggPage(QByteArray("OpusHead") + QByteArray(11, '\1')), "unsupported"},
        {"other ogg", "audio/ogg", oggPage(QByteArray("\x80theora", 7) + QByteArray(12, '\0')), "unsupported"},
        {"wav", "application/octet-stream", QByteArray("RIFF\x24\0\0\0WAVEfmt ", 16) + QByteArray(64, '\0'), "unsupported"},
        {"ogg labelled mpeg", "audio/mpeg", ogg, "unsupported"},
        {"mpeg labelled ogg", "audio/ogg", mp3, "unsupported"},
        {"truncated vorbis", "audio/ogg", ogg.left(200), "damaged"},
        {"malformed mpeg", "audio/mpeg", QByteArray("\xFF\xFB\x50\x00", 4) + QByteArray(65536, '\0'), "damaged"},
    };
    for (const auto &item : cases) {
        Rig rig;
        rig.play(A, UrlA)->serve(200, item.mime, item.body);
        if (rig.controller.state() != "error" || rig.controller.errorKind() != item.kind) {
            std::fprintf(stderr, "case %s: state %s kind %s\n", item.name, qPrintable(rig.controller.state()), qPrintable(rig.controller.errorKind()));
            throw std::runtime_error("format admission wrong");
        }
        require(rig.output->created == 0, "rejected media created output");
    }
    require(sniff(QByteArray("ID3\x04\0\0\0\0\0\0", 10) + mp3) == Container::Mp3, "ID3-tagged MP3 not recognised");
    require(sniff(ogg) == Container::Vorbis && sniff(mp3) == Container::Mp3, "fixture containers misclassified");
}

void silentDecode() {
    for (const char *name : {"silence-250ms.mp3", "silence-250ms.ogg"}) {
        auto opened = PcmDecoder::open(fixture(name));
        require(opened.decoder && opened.errorKind.isEmpty(), "fixture did not open");
        auto &decoder = *opened.decoder;
        require(decoder.channels() == 2 && decoder.sampleRate() == 44100, "fixture shape wrong");
        std::vector<qint16> chunk(size_t(ChunkFrames * 2));
        qint64 frames = 0;
        int peak = 0;
        qsizetype got = 0;
        for (; (got = decoder.read(chunk.data(), ChunkFrames)) > 0;) {
            require(got <= ChunkFrames, "decoder exceeded the chunk");
            frames += got;
            for (qsizetype i = 0; i < got * 2; ++i) peak = qMax(peak, qAbs(int(chunk[size_t(i)])));
        }
        require(got == 0, "clean silent stream ended as a decoder error");
        std::printf("  %s: %lld frames, peak %d, working %lld bytes\n", name, static_cast<long long>(frames), peak, static_cast<long long>(decoder.workingBytes()));
        require(frames >= 11025 - 1152 && frames <= 11025 + 2304, "silent fixture decoded to the wrong length");
        require(peak == 0, "silent fixture decoded to non-silent PCM");
    }
    const DecoderLimits defaults;
    auto mp3 = PcmDecoder::open(fixture("silence-250ms.mp3"));
    require(mp3.decoder->workingBytes() <= defaults.mp3AllocationBytes, "MP3 decoder exceeded its heap budget");
    require(PcmDecoder::open(fixture("silence-250ms.ogg")).decoder->workingBytes() == defaults.vorbisArenaBytes, "Vorbis arena not fixed");
    // Starved limits fail plainly instead of allocating more.
    require(PcmDecoder::open(fixture("silence-250ms.ogg"), {defaults.mp3AllocationBytes, 4096}).errorKind == "damaged", "Vorbis arena overflow not refused");
}

void lateVorbisFailure() {
    auto damaged = fixture("silence-250ms.ogg");
    const auto lastPage = damaged.lastIndexOf("OggS");
    require(lastPage > 0 && (damaged[lastPage + 5] & 4), "late-error fixture lacks its final page");
    const auto ending = damaged.mid(lastPage);
    // Keep the valid silent packets, then supply a malformed next-page header.
    // The declared HTTP size matches these bytes, so transport truncation
    // checks cannot replace the decoder's own error result.
    damaged[lastPage + 5] = char(damaged[lastPage + 5] & ~4);
    damaged += QByteArray("Not an Ogg page header", 22);
    // stb_vorbis probes the final page for length on its first read. Retain
    // valid final-page metadata beyond the corrupt playback boundary so the
    // first block succeeds and the subsequent sequential read must fail.
    damaged += ending;
    Rig rig;
    rig.play(A, UrlB)->serve(200, "audio/ogg", damaged);
    require(rig.controller.state() == "playing" && rig.output->written > 0,
            "late-corruption fixture failed before producing PCM");
    rig.runToEnd();
    require(rig.controller.state() == "error" && rig.controller.errorKind() == "damaged",
            "late Vorbis error was treated as clean completion");
    require(rig.output->destroyed == 1, "damaged stream retained its output");
}

void playbackLifecycle() {
    Rig rig;
    rig.output->capacity = rig.output->free = 4000; // smaller than one decoded chunk
    require(!rig.controller.outputCreated() && rig.output->created == 0, "output exists before play");
    rig.play(A, UrlA)->serve(200, "application/octet-stream", fixture("silence-250ms.mp3"));
    require(rig.controller.state() == "playing" && rig.controller.currentIdentity() == A, "MP3 did not start");
    require(rig.output->created == 1 && rig.output->started == 1, "output not created exactly once");
    require(rig.output->format.sampleRate() == 44100 && rig.output->format.channelCount() == 2
            && rig.output->format.sampleFormat() == QAudioFormat::Int16, "output format wrong");
    require(rig.output->written <= 4000 && rig.output->largestWrite <= 4000, "wrote past the output's free space");
    // Pause: no writes while paused even when the output has room.
    rig.controller.toggle(A, UrlA, false);
    require(rig.controller.state() == "paused" && rig.output->paused, "pause not applied");
    const auto before = rig.output->written;
    rig.output->drain();
    rig.controller.pumpNow();
    require(rig.output->written == before && rig.output->writesWhilePaused == 0, "wrote while paused");
    rig.controller.toggle(A, UrlA, false);
    require(rig.controller.state() == "playing" && !rig.output->paused && rig.output->written > before, "resume did not continue");
    rig.runToEnd();
    require(rig.controller.state() == "idle" && rig.controller.currentIdentity().isEmpty(), "end did not return to idle");
    require(rig.output->destroyed == 1 && rig.output->nonZeroBytes == 0, "end did not release the output, or PCM was not silent");
    require(rig.controller.largestChunkBytes() <= ChunkFrames * 4 && rig.controller.largestPumpFrames() <= FramesPerPump, "decode work per step unbounded");
    require(rig.output->written % 4 == 0 && rig.output->written / 4 == rig.controller.decodedFrames(), "written PCM differs from decoded frames");

    // Vorbis through the same path.
    rig.play(B, UrlB)->serve(200, "audio/ogg", fixture("silence-250ms.ogg"));
    require(rig.controller.state() == "playing" && rig.output->created == 2, "Vorbis did not start");
    rig.runToEnd();
    require(rig.controller.state() == "idle" && rig.output->destroyed == 2, "Vorbis did not end cleanly");
}

void replacementAndStaleCompletion() {
    Rig rig;
    auto *a = rig.play(A, UrlA);
    QPointer<FakeReply> stale = a;
    auto *b = rig.play(B, UrlB);
    require(a != b && stale && stale->aborted, "starting B did not cancel A's fetch");
    require(rig.controller.currentIdentity() == B && rig.controller.state() == "loading", "B not current");
    // A late completion from A's reply cannot resurrect A.
    stale->serve(200, "audio/mpeg", fixture("silence-250ms.mp3"));
    require(rig.controller.currentIdentity() == B && rig.controller.state() == "loading" && rig.output->created == 0, "stale A completion restarted playback");
    b->serve(200, "audio/ogg", fixture("silence-250ms.ogg"));
    require(rig.controller.state() == "playing" && rig.controller.currentIdentity() == B, "B did not play");
    // Switching while playing replaces the output.
    rig.play(A, UrlA)->serve(200, "audio/mpeg", fixture("silence-250ms.mp3"));
    require(rig.controller.currentIdentity() == A && rig.output->created == 2 && rig.output->destroyed == 1, "A->B replacement leaked an output");
    settle();
}

void cancellationAndInvalidation() {
    Rig rig;
    auto *reply = rig.play(A, UrlA);
    QPointer<FakeReply> held = reply;
    rig.controller.toggle(A, UrlA, false); // a second click while loading cancels
    require(rig.controller.state() == "idle" && held && held->aborted, "cancel while loading failed");
    held->serve(200, "audio/mpeg", fixture("silence-250ms.mp3"));
    require(rig.controller.state() == "idle" && rig.output->created == 0, "cancelled fetch resurrected playback");

    rig.controller.reveal(A);
    rig.play(A, UrlA, true)->serve(200, "audio/mpeg", fixture("silence-250ms.mp3"));
    require(rig.controller.state() == "playing", "revealed clip did not play");
    rig.controller.invalidate();
    require(rig.controller.state() == "idle" && rig.output->destroyed == 1, "invalidate did not release the output");
    require(rig.controller.revealedIdentities().isEmpty(), "invalidate kept reveals");
    rig.controller.pumpNow();
    require(rig.output->destroyed == 1 && rig.output->created == 1, "pump after invalidate recreated output");

    rig.output->failed = false;
    rig.play(A, UrlA)->serve(200, "audio/mpeg", fixture("silence-250ms.mp3"));
    rig.output->failed = true;
    rig.controller.pumpNow();
    require(rig.controller.state() == "error" && rig.controller.errorKind() == "output", "output failure not reported");
    settle();
}

void ownerReveal() {
    Rig rig;
    require(!rig.play(A, UrlA, true), "concealed clip fetched without reveal");
    require(rig.controller.state() == "idle", "concealed refusal changed state");
    rig.controller.reveal(B);
    require(!rig.play(A, UrlA, true), "another owner's reveal unlocked this clip");
    rig.controller.reveal(A);
    require(rig.controller.state() == "idle" && rig.network.requests.isEmpty(), "reveal started playback by itself");
    require(rig.play(A, UrlA, true), "revealed clip did not fetch");
    rig.controller.invalidate();
    require(!rig.play(A, UrlA, true), "reveal survived invalidation");
}

void outputErrors() {
    {
        Rig rig;
        rig.output->startError = QStringLiteral("outputFormat");
        rig.play(A, UrlA)->serve(200, "audio/mpeg", fixture("silence-250ms.mp3"));
        require(rig.controller.state() == "error" && rig.controller.errorKind() == "outputFormat", "unsupported output format not reported");
        require(rig.output->destroyed == 1, "failed output not released");
        require(!AudioPreviewController::describe("outputFormat").isEmpty(), "output error has no plain message");
    }
    {
        Rig rig;
        rig.controller.setOutputFactory([] { return std::unique_ptr<Output>(); });
        rig.play(A, UrlA)->serve(200, "audio/mpeg", fixture("silence-250ms.mp3"));
        require(rig.controller.errorKind() == "output", "missing output not reported");
    }
    for (const auto *kind : {"policy", "redirects", "network", "tooLarge", "unsupported", "damaged", "output", "outputFormat"})
        require(!AudioPreviewController::describe(QLatin1String(kind)).isEmpty(), "error kind has no plain message");
}

}

int main(int argc, char **argv) {
    QCoreApplication app(argc, argv);
    const QList<QPair<const char *, std::function<void()>>> checks{
        {"policy refusals (initial URL, HTTP, userinfo, port, look-alike host, file)", policyRefusals},
        {"request hygiene (manual redirects, no cookies/cache/credentials, identity encoding)", requestHygiene},
        {"redirects (off-host, HTTP, three allowed, fourth refused)", redirects},
        {"lengths and body caps (announced, streamed, lying, invalid, missing)", lengthsAndBody},
        {"transport failures (TLS, authentication, body before status)", transportFailures},
        {"format admission (MIME hint, signatures, Opus, playlists, malformed, truncated)", formats},
        {"silent MP3/Vorbis decode and decoder limits", silentDecode},
        {"late Vorbis failure after PCM and clean-EOF distinction", lateVorbisFailure},
        {"playback lifecycle (lazy output, bounded writes, pause/resume/end)", playbackLifecycle},
        {"A->B replacement and stale A completion", replacementAndStaleCompletion},
        {"cancellation, invalidation and output failure", cancellationAndInvalidation},
        {"owner-scoped mature reveal", ownerReveal},
        {"output errors", outputErrors},
    };
    int failed = 0;
    for (const auto &check : checks) {
        try {
            check.second();
            settle();
            std::printf("PASS %s\n", check.first);
        } catch (const std::exception &error) {
            ++failed;
            std::printf("FAIL %s: %s\n", check.first, error.what());
        }
        std::fflush(stdout);
    }
    std::printf("%s audio-preview-check (%d failed)\n", failed ? "FAIL" : "PASS", failed);
    return failed ? 1 : 0;
}
