// music_rust —— WAV fast-path assembly interface
// Copyright (C) 2026 FuturePioneer-3
// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef MUSIC_AUDIO_WAV_H
#define MUSIC_AUDIO_WAV_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct music_wav_meta {
    uint16_t format;          /* 1 = PCM integer, 3 = IEEE float */
    uint16_t channels;
    uint32_t sample_rate;
    uint16_t block_align;
    uint16_t bits_per_sample;
    uint32_t data_offset;
    uint32_t data_size;
} music_wav_meta;

/* Parse a complete RIFF/WAVE file image. Returns 1 on supported PCM/float WAV. */
int audio_wav_parse(const uint8_t *file, size_t len, music_wav_meta *out);

/* Convert interleaved WAV samples into the player's native signed 16-bit PCM. */
int audio_wav_to_s16(const uint8_t *src, int16_t *dst, size_t samples,
                     uint16_t format, uint16_t bits_per_sample);

#ifdef __cplusplus
}
#endif

#endif /* MUSIC_AUDIO_WAV_H */
