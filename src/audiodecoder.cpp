#include "audiodecoder.h"
#include <cstdlib>
#include <cstring>

// Vendored, unmodified single-file decoders (see third_party/*/ORIGIN.md).
// Both are compiled only here, from memory, without stdio or push APIs.
#define DR_MP3_IMPLEMENTATION
#define DR_MP3_NO_STDIO
#include "../third_party/dr_mp3/dr_mp3.h"

#define STB_VORBIS_NO_STDIO
#define STB_VORBIS_NO_PUSHDATA_API
#define STB_VORBIS_NO_COMMENTS
#define STB_VORBIS_MAX_CHANNELS 2
#include "../third_party/stb_vorbis/stb_vorbis.c"

namespace AudioPreview {
namespace {

bool mpegFrameSync(const unsigned char *p) {
    // 11-bit sync, version not reserved (01), layer not reserved (00),
    // bitrate index not 1111, sample-rate index not 11.
    return p[0] == 0xFF && (p[1] & 0xE0) == 0xE0 && ((p[1] >> 3) & 3) != 1 && ((p[1] >> 1) & 3) != 0
        && (p[2] >> 4) != 0xF && ((p[2] >> 2) & 3) != 3;
}

// dr_mp3 allocation callbacks over a counted budget. Each block carries its
// size in a 16-byte header so realloc/free can keep the count exact.
struct Budget {
    qsizetype limit = 0, used = 0, peak = 0;
};
constexpr size_t Header = 16;
void *budgetMalloc(size_t size, void *user) {
    auto *budget = static_cast<Budget *>(user);
    if (size > size_t(budget->limit - budget->used)) return nullptr;
    auto *base = static_cast<char *>(std::malloc(size + Header));
    if (!base) return nullptr;
    std::memcpy(base, &size, sizeof size);
    budget->used += qsizetype(size);
    budget->peak = qMax(budget->peak, budget->used);
    return base + Header;
}
void budgetFree(void *pointer, void *user) {
    if (!pointer) return;
    auto *base = static_cast<char *>(pointer) - Header;
    size_t size = 0;
    std::memcpy(&size, base, sizeof size);
    static_cast<Budget *>(user)->used -= qsizetype(size);
    std::free(base);
}
void *budgetRealloc(void *pointer, size_t size, void *user) {
    if (!pointer) return budgetMalloc(size, user);
    auto *budget = static_cast<Budget *>(user);
    auto *base = static_cast<char *>(pointer) - Header;
    size_t old = 0;
    std::memcpy(&old, base, sizeof old);
    if (size > old && size - old > size_t(budget->limit - budget->used)) return nullptr;
    auto *moved = static_cast<char *>(std::realloc(base, size + Header));
    if (!moved) return nullptr;
    std::memcpy(moved, &size, sizeof size);
    budget->used += qsizetype(size) - qsizetype(old);
    budget->peak = qMax(budget->peak, budget->used);
    return moved + Header;
}

bool supportedShape(int channels, int rate) {
    return channels >= 1 && channels <= MaximumChannels && rate >= MinimumRate && rate <= MaximumRate;
}

class Mp3Decoder final : public PcmDecoder {
public:
    Mp3Decoder(QByteArray bytes, qsizetype budget) : m_bytes(std::move(bytes)) { m_budget.limit = budget; }
    ~Mp3Decoder() override { if (m_open) drmp3_uninit(m_mp3.get()); }
    QString open() {
        const drmp3_allocation_callbacks callbacks{&m_budget, budgetMalloc, budgetRealloc, budgetFree};
        m_mp3 = std::make_unique<drmp3>();
        if (!drmp3_init_memory(m_mp3.get(), m_bytes.constData(), size_t(m_bytes.size()), &callbacks))
            return QStringLiteral("damaged");
        m_open = true;
        m_channels = int(m_mp3->channels);
        m_rate = int(m_mp3->sampleRate);
        return supportedShape(m_channels, m_rate) ? QString() : QStringLiteral("unsupported");
    }
    qsizetype read(qint16 *out, qsizetype maxFrames) override {
        if (maxFrames <= 0) return 0;
        return qsizetype(drmp3_read_pcm_frames_s16(m_mp3.get(), drmp3_uint64(maxFrames), out));
    }
    qsizetype workingBytes() const override { return m_budget.peak; }
private:
    QByteArray m_bytes;
    Budget m_budget;
    std::unique_ptr<drmp3> m_mp3;
    bool m_open = false;
};

class VorbisDecoder final : public PcmDecoder {
public:
    VorbisDecoder(QByteArray bytes, int arena) : m_bytes(std::move(bytes)), m_arenaBytes(arena), m_arena(new char[size_t(arena)]) {}
    ~VorbisDecoder() override { if (m_vorbis) stb_vorbis_close(m_vorbis); }
    QString open() {
        stb_vorbis_alloc arena{m_arena.get(), m_arenaBytes};
        int error = 0;
        m_vorbis = stb_vorbis_open_memory(reinterpret_cast<const unsigned char *>(m_bytes.constData()), int(m_bytes.size()), &error, &arena);
        if (!m_vorbis)
            return error == VORBIS_too_many_channels ? QStringLiteral("unsupported") : QStringLiteral("damaged");
        const auto info = stb_vorbis_get_info(m_vorbis);
        m_channels = info.channels;
        m_rate = int(info.sample_rate);
        return supportedShape(m_channels, m_rate) ? QString() : QStringLiteral("unsupported");
    }
    qsizetype read(qint16 *out, qsizetype maxFrames) override {
        if (maxFrames <= 0) return 0;
        const auto shorts = int(qMin<qsizetype>(maxFrames, 1 << 20) * m_channels);
        const auto frames = stb_vorbis_get_samples_short_interleaved(m_vorbis, m_channels, out, shorts);
        // An error can accompany a partial block; do not send that block to
        // output or disguise the failure as ordinary end-of-stream.
        return stb_vorbis_get_error(m_vorbis) == VORBIS__no_error ? qsizetype(frames) : -1;
    }
    qsizetype workingBytes() const override { return m_arenaBytes; }
private:
    QByteArray m_bytes;
    int m_arenaBytes = 0;
    std::unique_ptr<char[]> m_arena;
    stb_vorbis *m_vorbis = nullptr;
};

}

Container sniff(const QByteArray &bytes) {
    const auto *p = reinterpret_cast<const unsigned char *>(bytes.constData());
    const auto size = bytes.size();
    if (size >= 10 && std::memcmp(p, "ID3", 3) == 0 && p[3] != 0xFF && p[4] != 0xFF
        && !((p[6] | p[7] | p[8] | p[9]) & 0x80)) return Container::Mp3;
    if (size >= 4 && mpegFrameSync(p)) return Container::Mp3;
    if (size >= 28 && std::memcmp(p, "OggS", 4) == 0 && p[4] == 0 && (p[5] & 0x02)) {
        const qsizetype payload = 27 + qsizetype(p[26]);
        if (size >= payload + 7 && std::memcmp(p + payload, "\x01vorbis", 7) == 0) return Container::Vorbis;
        if (size >= payload + 8 && std::memcmp(p + payload, "OpusHead", 8) == 0) return Container::Opus;
        return Container::OtherOgg;
    }
    return Container::Unknown;
}

PcmDecoder::Opened PcmDecoder::open(const QByteArray &bytes, const DecoderLimits &limits) {
    switch (sniff(bytes)) {
    case Container::Mp3: {
        auto decoder = std::make_unique<Mp3Decoder>(bytes, limits.mp3AllocationBytes);
        const auto error = decoder->open();
        if (!error.isEmpty()) return {nullptr, error};
        return {std::move(decoder), {}};
    }
    case Container::Vorbis: {
        if (limits.vorbisArenaBytes <= 0) return {nullptr, QStringLiteral("damaged")};
        auto decoder = std::make_unique<VorbisDecoder>(bytes, limits.vorbisArenaBytes);
        const auto error = decoder->open();
        if (!error.isEmpty()) return {nullptr, error};
        return {std::move(decoder), {}};
    }
    default:
        return {nullptr, QStringLiteral("unsupported")};
    }
}

}
