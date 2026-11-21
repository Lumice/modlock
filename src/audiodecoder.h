#pragma once
#include <QByteArray>
#include <QString>
#include <memory>

// Memory-only decoders for Browse sound previews. Input bytes come from the
// bounded preview fetch; nothing here opens files, URLs or devices.
namespace AudioPreview {

enum class Container { Unknown, Mp3, Vorbis, Opus, OtherOgg };
// Classifies leading bytes only. Mp3 means an ID3v2 tag or an MPEG audio frame
// sync at offset 0; Vorbis means a first Ogg page carrying a Vorbis
// identification header. Decoding must still succeed before playback.
Container sniff(const QByteArray &bytes);

struct DecoderLimits {
    qsizetype mp3AllocationBytes = 1 << 20; // counted dr_mp3 heap budget
    int vorbisArenaBytes = 2 << 20;          // fixed stb_vorbis arena (all of its memory)
};

class PcmDecoder {
public:
    virtual ~PcmDecoder() = default;
    int channels() const { return m_channels; }
    int sampleRate() const { return m_rate; }
    // Decodes at most maxFrames interleaved signed 16-bit frames into out,
    // which must hold maxFrames * channels() samples. Returns frames written,
    // 0 at clean end, or -1 when the codec reports a decoding error. A codec
    // may be unable to distinguish some truncation from ordinary end-of-input.
    virtual qsizetype read(qint16 *out, qsizetype maxFrames) = 0;
    // Bytes of decoder working memory currently held (heap budget or arena).
    virtual qsizetype workingBytes() const = 0;

    struct Opened {
        std::unique_ptr<PcmDecoder> decoder;
        QString errorKind; // "unsupported" or "damaged" when decoder is null
    };
    // Keeps a shared reference to bytes for the decoder's lifetime.
    static Opened open(const QByteArray &bytes, const DecoderLimits &limits = {});

protected:
    int m_channels = 0, m_rate = 0;
};

constexpr int MinimumRate = 8000, MaximumRate = 48000, MaximumChannels = 2;

}
