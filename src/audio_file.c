// music_rust —— FFmpeg + ALSA 音频文件播放器
// Copyright (C) 2026 FuturePioneer-3
// SPDX-License-Identifier: GPL-3.0-or-later

#include "audio_file.h"
#include <libavformat/avformat.h>
#include <libavcodec/avcodec.h>
#include <libavutil/avutil.h>
#include <libswresample/swresample.h>
#include <alsa/asoundlib.h>
#include <pthread.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <math.h>

struct music_audio {
    int16_t *pcm;
    int64_t frames;
    int sample_rate;
    int channels;
    int64_t cursor;
    float volume;
    int playing;
    int finished;
    int stop;
    int thread_started;
    uint64_t generation;
    pthread_mutex_t lock;
    pthread_cond_t wake;
    pthread_t thread;
};

static void set_error(char *out, int len, const char *message) {
    if (out && len > 0) snprintf(out, (size_t)len, "%s", message);
}

static void *audio_thread(void *opaque) {
    music_audio *p = (music_audio *)opaque;
    snd_pcm_t *pcm = NULL;
    if (snd_pcm_open(&pcm, "default", SND_PCM_STREAM_PLAYBACK, 0) < 0 ||
        snd_pcm_set_params(pcm, SND_PCM_FORMAT_S16_LE, SND_PCM_ACCESS_RW_INTERLEAVED,
                            (unsigned)p->channels, (unsigned)p->sample_rate, 1, 50000) < 0) {
        pthread_mutex_lock(&p->lock); p->finished = 1; p->stop = 1; pthread_mutex_unlock(&p->lock);
        if (pcm) snd_pcm_close(pcm);
        return NULL;
    }
    uint64_t played_generation = 0;
    while (1) {
        pthread_mutex_lock(&p->lock);
        while (!p->playing && !p->stop) pthread_cond_wait(&p->wake, &p->lock);
        if (p->stop) { pthread_mutex_unlock(&p->lock); break; }
        int64_t left = p->frames - p->cursor;
        int64_t start = p->cursor;
        float volume = p->volume;
        uint64_t generation = p->generation;
        pthread_mutex_unlock(&p->lock);
        if (generation != played_generation) {
            snd_pcm_drop(pcm);
            snd_pcm_prepare(pcm);
            played_generation = generation;
            continue;
        }
        if (left <= 0) {
            pthread_mutex_lock(&p->lock); p->finished = 1; p->playing = 0; pthread_mutex_unlock(&p->lock);
            continue;
        }
        int64_t count = left > 512 ? 512 : left;
        int16_t *buffer = malloc((size_t)count * (size_t)p->channels * sizeof(int16_t));
        if (!buffer) break;
        for (int64_t i = 0; i < count * p->channels; i++) {
            int value = (int)((float)p->pcm[start * p->channels + i] * volume);
            buffer[i] = (int16_t)(value > 32767 ? 32767 : (value < -32768 ? -32768 : value));
        }
        pthread_mutex_lock(&p->lock);
        int changed = generation != p->generation || start != p->cursor;
        pthread_mutex_unlock(&p->lock);
        if (changed) { free(buffer); continue; }
        snd_pcm_sframes_t written = snd_pcm_writei(pcm, buffer, (snd_pcm_uframes_t)count);
        free(buffer);
        if (written < 0) { snd_pcm_recover(pcm, (int)written, 1); continue; }
        pthread_mutex_lock(&p->lock);
        if (generation == p->generation && start == p->cursor) p->cursor += written;
        pthread_mutex_unlock(&p->lock);
    }
    snd_pcm_drain(pcm); snd_pcm_close(pcm);
    return NULL;
}

music_audio *music_audio_open(const char *path, char *error, int error_len) {
    AVFormatContext *format = NULL;
    if (avformat_open_input(&format, path, NULL, NULL) < 0 || avformat_find_stream_info(format, NULL) < 0) {
        set_error(error, error_len, "FFmpeg 无法打开音频文件"); if (format) avformat_close_input(&format); return NULL;
    }
    int stream = av_find_best_stream(format, AVMEDIA_TYPE_AUDIO, -1, -1, NULL, 0);
    if (stream < 0) { set_error(error, error_len, "文件中没有音频流"); avformat_close_input(&format); return NULL; }
    AVCodecParameters *params = format->streams[stream]->codecpar;
    const AVCodec *codec = avcodec_find_decoder(params->codec_id);
    AVCodecContext *ctx = codec ? avcodec_alloc_context3(codec) : NULL;
    if (!ctx || avcodec_parameters_to_context(ctx, params) < 0 || avcodec_open2(ctx, codec, NULL) < 0) {
        set_error(error, error_len, "FFmpeg 无法初始化音频解码器"); if (ctx) avcodec_free_context(&ctx); avformat_close_input(&format); return NULL;
    }
    int rate = ctx->sample_rate > 0 ? ctx->sample_rate : 48000;
    int channels = 2;
    AVChannelLayout output_layout = AV_CHANNEL_LAYOUT_STEREO;
    SwrContext *swr = NULL;
    if (swr_alloc_set_opts2(&swr, &output_layout, AV_SAMPLE_FMT_S16, rate,
                            &ctx->ch_layout, ctx->sample_fmt, ctx->sample_rate, 0, NULL) < 0 ||
        swr_init(swr) < 0) {
        set_error(error, error_len, "FFmpeg 音频重采样初始化失败"); if (swr) swr_free(&swr); avcodec_free_context(&ctx); avformat_close_input(&format); return NULL;
    }
    music_audio *p = calloc(1, sizeof(*p)); p->sample_rate = rate; p->channels = channels; p->volume = 0.8f;
    pthread_mutex_init(&p->lock, NULL); pthread_cond_init(&p->wake, NULL);
    AVPacket *packet = av_packet_alloc(); AVFrame *frame = av_frame_alloc();
    while (packet && frame && av_read_frame(format, packet) >= 0) {
        if (packet->stream_index == stream && avcodec_send_packet(ctx, packet) >= 0) {
            while (avcodec_receive_frame(ctx, frame) >= 0) {
                int out_count = swr_get_out_samples(swr, frame->nb_samples);
                int16_t *out = malloc((size_t)out_count * channels * sizeof(int16_t));
                uint8_t *out_data[] = { (uint8_t *)out };
                int converted = out ? swr_convert(swr, out_data, out_count, (const uint8_t **)frame->extended_data, frame->nb_samples) : 0;
                if (converted > 0) { p->pcm = realloc(p->pcm, (size_t)(p->frames + converted) * channels * sizeof(int16_t)); memcpy(p->pcm + p->frames * channels, out, (size_t)converted * channels * sizeof(int16_t)); p->frames += converted; }
                free(out);
            }
        }
        av_packet_unref(packet);
    }
    av_packet_free(&packet); av_frame_free(&frame); swr_free(&swr); avcodec_free_context(&ctx); avformat_close_input(&format);
    if (!p->pcm || p->frames == 0) { set_error(error, error_len, "音频文件没有可播放数据"); music_audio_close(p); return NULL; }
    if (pthread_create(&p->thread, NULL, audio_thread, p) != 0) { set_error(error, error_len, "无法创建音频播放线程"); music_audio_close(p); return NULL; }
    p->thread_started = 1;
    return p;
}

int music_audio_play(music_audio *p) { if (!p) return -1; pthread_mutex_lock(&p->lock); p->playing = 1; p->finished = 0; pthread_cond_signal(&p->wake); pthread_mutex_unlock(&p->lock); return 0; }
int music_audio_pause(music_audio *p) { if (!p) return -1; pthread_mutex_lock(&p->lock); p->playing = 0; pthread_mutex_unlock(&p->lock); return 0; }
int music_audio_seek(music_audio *p, int64_t ms) { if (!p) return -1; pthread_mutex_lock(&p->lock); p->cursor = (ms < 0 ? 0 : ms > p->frames * 1000 / p->sample_rate ? p->frames : ms * p->sample_rate / 1000); p->generation++; p->finished = 0; pthread_cond_signal(&p->wake); pthread_mutex_unlock(&p->lock); return 0; }
int64_t music_audio_position_ms(music_audio *p) { if (!p) return 0; pthread_mutex_lock(&p->lock); int64_t v = p->cursor * 1000 / p->sample_rate; pthread_mutex_unlock(&p->lock); return v; }
int64_t music_audio_duration_ms(music_audio *p) { return p ? p->frames * 1000 / p->sample_rate : 0; }
int music_audio_finished(music_audio *p) { if (!p) return 1; pthread_mutex_lock(&p->lock); int v = p->finished; pthread_mutex_unlock(&p->lock); return v; }
void music_audio_set_volume(music_audio *p, float v) { if (!p) return; pthread_mutex_lock(&p->lock); p->volume = v < 0.0f ? 0.0f : (v > 5.0f ? 5.0f : v); pthread_mutex_unlock(&p->lock); }
float music_audio_volume(music_audio *p) { if (!p) return 0.8f; pthread_mutex_lock(&p->lock); float v = p->volume; pthread_mutex_unlock(&p->lock); return v; }
void music_audio_spectrum(music_audio *p, uint8_t levels[16]) {
    if (!levels) return;
    memset(levels, 0, 16);
    if (!p) return;
    pthread_mutex_lock(&p->lock);
    int64_t start = p->cursor;
    int64_t count = p->frames - start;
    if (count > 1024) count = 1024;
    if (count < 32) { pthread_mutex_unlock(&p->lock); return; }
    for (int band = 0; band < 16; band++) {
        // 16 个对数中心频率严格覆盖 20Hz 到 10kHz。
        float frequency = 20.0f * powf(500.0f, (float)band / 15.0f);
        float coeff = 2.0f * cosf(2.0f * 3.14159265f * frequency / p->sample_rate);
        float q0 = 0.0f, q1 = 0.0f, q2 = 0.0f;
        for (int64_t i = 0; i < count; i++) {
            float sample = (float)p->pcm[(start + i) * p->channels] / 32768.0f;
            q0 = coeff * q1 - q2 + sample;
            q2 = q1;
            q1 = q0;
        }
        float magnitude = sqrtf(q1 * q1 + q2 * q2 - coeff * q1 * q2) / count;
        int level = (int)(log10f(1.0f + magnitude * 550.0f) * 4.0f);
        levels[band] = (uint8_t)(level > 7 ? 7 : level);
    }
    pthread_mutex_unlock(&p->lock);
}
void music_audio_close(music_audio *p) { if (!p) return; if (p->thread_started) { pthread_mutex_lock(&p->lock); p->stop = 1; p->playing = 1; pthread_cond_signal(&p->wake); pthread_mutex_unlock(&p->lock); pthread_join(p->thread, NULL); } free(p->pcm); pthread_cond_destroy(&p->wake); pthread_mutex_destroy(&p->lock); free(p); }
