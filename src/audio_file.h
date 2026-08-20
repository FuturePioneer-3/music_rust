// music_rust —— 音频文件播放 C 接口
// Copyright (C) 2026 FuturePioneer-3
// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef MUSIC_AUDIO_FILE_H
#define MUSIC_AUDIO_FILE_H

#include <stdint.h>

typedef struct music_audio music_audio;

music_audio *music_audio_open(const char *path, char *error, int error_len);
int music_audio_play(music_audio *player);
int music_audio_pause(music_audio *player);
int music_audio_seek(music_audio *player, int64_t position_ms);
int64_t music_audio_position_ms(music_audio *player);
int64_t music_audio_duration_ms(music_audio *player);
int music_audio_finished(music_audio *player);
void music_audio_set_volume(music_audio *player, float volume);
float music_audio_volume(music_audio *player);
void music_audio_spectrum(music_audio *player, uint8_t levels[16]);
const char *music_audio_metadata(music_audio *player, const char *key);
int music_audio_art(music_audio *player, const unsigned char **data, int *width, int *height);
void music_audio_close(music_audio *player);

// ---- 2.4.0：元数据与封面 ----
// 返回内部缓冲区指针（调用方不得释放）；字段不存在返回 NULL。
// key 取值：title / artist / album / composer / date / genre
const char *music_audio_metadata(music_audio *player, const char *key);
// 内嵌封面（MP3 APIC / FLAC PICTURE / M4A covr），已解码为 RGBA8 并缩放到 ≤96px。
// 存在返回 1 并回填 data/width/height，否则返回 0。
int music_audio_art(music_audio *player, const unsigned char **data, int *width, int *height);

#endif
