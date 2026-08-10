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
void music_audio_close(music_audio *player);

#endif
