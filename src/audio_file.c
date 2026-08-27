// music_rust —— FFmpeg + ALSA 音频文件播放器
// Copyright (C) 2026 FuturePioneer-3
// SPDX-License-Identifier: GPL-3.0-or-later
//
// 2.4.0 新增：
//   - 热循环改用独立 AT&T 汇编（music_asm.S）：
//       * 音量饱和缩放  music_asm_apply_volume_s16（音频线程，8 样本/迭代 SSE2）
//       * 频谱 Goertzel  music_asm_goertzel4（16 频段分 4 组并行谐振器）
//   - 元数据提取（title/artist/album/composer/date/genre，如 MP3 ID3 TCOM → composer）
//   - 内嵌封面图提取（MP3 APIC / FLAC PICTURE / M4A covr），解码并缩放到 ≤96px RGBA，
//     供 TUI 以半块字符渲染。

#include "audio_file.h"
#include "audio_dsp.h"
#include "audio_wav.h"
#include "music_asm.h"
#include <libavformat/avformat.h>
#include <libavcodec/avcodec.h>
#include <libavutil/avutil.h>
#include <libavutil/dict.h>
#include <libswresample/swresample.h>
#include <libswscale/swscale.h>
#include <alsa/asoundlib.h>
#include <pthread.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <stdio.h>
#include <math.h>

static void *audio_thread(void *opaque);

static int is_wav_path(const char *path) {
    const char *dot = strrchr(path, '.');
    if (!dot) return 0;
    return strcasecmp(dot, ".wav") == 0;
}

#define MUSIC_META_LEN 256
/// 封面图最长边（像素）。TUI 再按终端尺寸二次缩放渲染。
#define MUSIC_ART_MAX_DIM 96

struct music_audio {
    int16_t *pcm;
    int64_t frames;
    int sample_rate;
    int channels;
    int64_t cursor;
    float volume;
    float ramp_gain;   /* 逐样本渐变增益（防爆音），由汇编 dsp_vol_s16 维护 */
    int playing;
    int paused;
    int finished;
    int stop;
    int thread_started;
    uint64_t generation;
    pthread_mutex_t lock;
    pthread_cond_t wake;
    pthread_t thread;

    // ---- 元数据与封面 ----
    char meta_title[MUSIC_META_LEN];
    char meta_artist[MUSIC_META_LEN];
    char meta_album[MUSIC_META_LEN];
    char meta_composer[MUSIC_META_LEN];
    char meta_date[MUSIC_META_LEN];
    char meta_genre[MUSIC_META_LEN];
    unsigned char *art;   // RGBA8，宽 art_w × 高 art_h
    int art_w;
    int art_h;
};

static void set_error(char *out, int len, const char *message) {
    if (out && len > 0) snprintf(out, (size_t)len, "%s", message);
}

static void free_audio_state(music_audio *p) {
    if (!p) return;
    free(p->pcm);
    free(p->art);
    free(p);
}

static music_audio *load_wav_direct(const char *path) {
    FILE *fp = fopen(path, "rb");
    if (!fp) return NULL;
    if (fseek(fp, 0, SEEK_END) != 0) {
        fclose(fp);
        return NULL;
    }
    long file_len = ftell(fp);
    if (file_len <= 0) {
        fclose(fp);
        return NULL;
    }
    if (fseek(fp, 0, SEEK_SET) != 0) {
        fclose(fp);
        return NULL;
    }
    uint8_t *file = malloc((size_t)file_len);
    if (!file) {
        fclose(fp);
        return NULL;
    }
    if (fread(file, 1, (size_t)file_len, fp) != (size_t)file_len) {
        free(file);
        fclose(fp);
        return NULL;
    }
    fclose(fp);

    music_wav_meta meta = {0};
    if (!audio_wav_parse(file, (size_t)file_len, &meta)) {
        free(file);
        return NULL;
    }

    uint64_t frames64 = meta.data_size / meta.block_align;
    if (frames64 == 0 || frames64 > (uint64_t)INT64_MAX) {
        free(file);
        return NULL;
    }
    size_t samples = (size_t)frames64 * (size_t)meta.channels;
    if (samples == 0 || samples > SIZE_MAX / sizeof(int16_t)) {
        free(file);
        return NULL;
    }

    music_audio *p = calloc(1, sizeof(*p));
    if (!p) {
        free(file);
        return NULL;
    }
    p->pcm = malloc(samples * sizeof(int16_t));
    if (!p->pcm) {
        free(file);
        free_audio_state(p);
        return NULL;
    }
    p->frames = (int64_t)frames64;
    p->sample_rate = (int)meta.sample_rate;
    p->channels = (int)meta.channels;
    p->volume = 0.8f;
    p->paused = 0;

    const uint8_t *data = file + meta.data_offset;
    if (!audio_wav_to_s16(data, p->pcm, samples, meta.format, meta.bits_per_sample)) {
        free(file);
        free_audio_state(p);
        return NULL;
    }
    free(file);

    pthread_mutex_init(&p->lock, NULL);
    pthread_cond_init(&p->wake, NULL);
    if (pthread_create(&p->thread, NULL, audio_thread, p) != 0) {
        pthread_cond_destroy(&p->wake);
        pthread_mutex_destroy(&p->lock);
        free_audio_state(p);
        return NULL;
    }
    p->thread_started = 1;
    return p;
}

/// 从 FFmpeg 字典复制一个元数据字段到定长缓冲区（空值保持空串）。
static void copy_meta(char *dst, size_t dst_len, AVDictionary *dict, const char *key) {
    AVDictionaryEntry *e = av_dict_get(dict, key, NULL, 0);
    if (e && e->value) {
        snprintf(dst, dst_len, "%s", e->value);
    }
}

/// 解码一个内嵌封面 AVPacket → 缩放到 ≤96px 的 RGBA8 缓冲，存入 p。
/// 成功返回 1，失败返回 0（p 保持不变）。
static int decode_attached_picture(music_audio *p, AVPacket *packet,
                                   AVCodecParameters *params) {
    const AVCodec *codec = avcodec_find_decoder(params->codec_id);
    AVCodecContext *ctx = codec ? avcodec_alloc_context3(codec) : NULL;
    if (!ctx || avcodec_parameters_to_context(ctx, params) < 0 ||
        avcodec_open2(ctx, codec, NULL) < 0) {
        if (ctx) avcodec_free_context(&ctx);
        return 0;
    }
    AVFrame *frame = av_frame_alloc();
    int ok = 0;
    if (frame && avcodec_send_packet(ctx, packet) >= 0 &&
        avcodec_receive_frame(ctx, frame) >= 0 &&
        frame->width > 0 && frame->height > 0 && frame->format >= 0) {
        int iw = frame->width, ih = frame->height;
        double scale = (double)MUSIC_ART_MAX_DIM / (iw > ih ? iw : ih);
        if (scale > 1.0) scale = 1.0;
        int dw = (int)(iw * scale + 0.5); if (dw < 1) dw = 1;
        int dh = (int)(ih * scale + 0.5); if (dh < 1) dh = 1;
        struct SwsContext *sws = sws_getContext(
            iw, ih, (enum AVPixelFormat)frame->format,
            dw, dh, AV_PIX_FMT_RGBA, SWS_BILINEAR, NULL, NULL, NULL);
        if (sws) {
            unsigned char *rgba = malloc((size_t)dw * dh * 4);
            if (rgba) {
                uint8_t *dst_data[1] = { rgba };
                int dst_linesize[1] = { dw * 4 };
                sws_scale(sws, (const uint8_t *const *)frame->data,
                          frame->linesize, 0, ih, dst_data, dst_linesize);
                p->art = rgba;
                p->art_w = dw;
                p->art_h = dh;
                ok = 1;
            }
            sws_freeContext(sws);
        }
    }
    av_frame_free(&frame);
    avcodec_free_context(&ctx);
    return ok;
}

/// 从已打开的 AVFormatContext 中查找内嵌封面流（ATTACHED_PIC），
/// 使用流自带 attached_pic 包解码。找到并解码成功返回 1。
static int load_attached_picture_from_format(music_audio *p, AVFormatContext *format) {
    for (unsigned i = 0; i < format->nb_streams; i++) {
        AVStream *st = format->streams[i];
        if ((st->disposition & AV_DISPOSITION_ATTACHED_PIC) &&
            st->attached_pic.data && st->attached_pic.size > 0) {
            if (decode_attached_picture(p, &st->attached_pic, st->codecpar)) {
                return 1;
            }
            break; // 封面流存在但解码失败，不再尝试
        }
    }
    return 0;
}

/// 回退路径：重新打开文件，逐个读取数据包寻找封面流（部分封装格式
/// 的封面只在读取过程中产生）。
static void load_attached_picture_fallback(music_audio *p, const char *path) {
    AVFormatContext *format = NULL;
    if (avformat_open_input(&format, path, NULL, NULL) < 0) return;
    avformat_find_stream_info(format, NULL);
    int pic_index = -1;
    for (unsigned i = 0; i < format->nb_streams; i++) {
        if (format->streams[i]->disposition & AV_DISPOSITION_ATTACHED_PIC) {
            pic_index = (int)i;
            break;
        }
    }
    if (pic_index >= 0) {
        AVStream *st = format->streams[pic_index];
        if (st->attached_pic.data && st->attached_pic.size > 0) {
            decode_attached_picture(p, &st->attached_pic, st->codecpar);
        } else {
            AVPacket *packet = av_packet_alloc();
            int found = 0;
            for (int tries = 0; tries < 128 && av_read_frame(format, packet) >= 0; tries++) {
                if (packet->stream_index == pic_index) {
                    found = decode_attached_picture(p, packet, st->codecpar);
                    break;
                }
                av_packet_unref(packet);
            }
            av_packet_free(&packet);
            (void)found;
        }
    }
    avformat_close_input(&format);
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
        // ---- 暂停分支：先淡出当前块（防硬切爆音），再进入等待 ----
        if (!p->playing && !p->stop) {
            if (p->paused) {
                pthread_cond_wait(&p->wake, &p->lock);
                if (p->stop) { pthread_mutex_unlock(&p->lock); break; }
                // pthread_cond_wait() 返回时仍持有 p->lock。回到循环顶部前
                // 必须显式解锁，否则下一轮会在同一线程上再次加锁而死锁，
                // 导致暂停后 play/seek/position 全部卡住。
                pthread_mutex_unlock(&p->lock);
                continue;
            }
            int64_t fade_left = p->frames - p->cursor;
            int fade = (fade_left > 0 && p->ramp_gain > 0.003f) ? 1 : 0;
            if (!fade) {
                pthread_cond_wait(&p->wake, &p->lock);
                if (p->stop) { pthread_mutex_unlock(&p->lock); break; }
                // 被唤醒（play/seek）：锁仍持有，继续正常流程
            } else {
                int64_t fade_count = fade_left > 512 ? 512 : fade_left;
                int64_t fade_start = p->cursor;
                uint64_t gen = p->generation;
                int16_t *buffer = malloc((size_t)fade_count * (size_t)p->channels * sizeof(int16_t));
                if (!buffer) { p->finished = 1; p->stop = 1; pthread_mutex_unlock(&p->lock); break; }
                // 汇编：把剩余音频在 512 帧内线性淡出。ramp_gain 由
                // pause/seek 在持锁时清零，这里也在持锁状态下读写，
                // 消除音频线程与控制线程之间的数据竞争。
                dsp_vol_s16(p->pcm + fade_start * p->channels, buffer,
                            (uint32_t)(fade_count * p->channels), &p->ramp_gain, 0.0f);
                pthread_mutex_unlock(&p->lock);
                snd_pcm_sframes_t written = snd_pcm_writei(pcm, buffer, (snd_pcm_uframes_t)fade_count);
                free(buffer);
                pthread_mutex_lock(&p->lock);
                if (written < 0) { snd_pcm_recover(pcm, (int)written, 1); pthread_mutex_unlock(&p->lock); continue; }
                if (gen == p->generation && fade_start == p->cursor) p->cursor += written;
                if (p->stop) { pthread_mutex_unlock(&p->lock); break; }
                pthread_mutex_unlock(&p->lock);
                continue; // 回到循环顶部：此时 ramp_gain≈0，转入等待
            }
        }
        while (!p->playing && !p->stop) pthread_cond_wait(&p->wake, &p->lock);
        if (p->stop) { pthread_mutex_unlock(&p->lock); break; }
        int64_t left = p->frames - p->cursor;
        int64_t start = p->cursor;
        float volume = p->volume;
        uint64_t generation = p->generation;
        if (generation != played_generation) {
            pthread_mutex_unlock(&p->lock);
            snd_pcm_drop(pcm);
            snd_pcm_prepare(pcm);
            played_generation = generation;
            continue;
        }
        if (left <= 0) {
            /* 快照与判定都在持锁状态下完成，seek 无法在两者之间移动
             * cursor，因此不会把新请求的播放位置误判为文件结束。 */
            if (generation == p->generation && start == p->cursor && p->playing) {
                p->finished = 1;
                p->playing = 0;
            }
            pthread_mutex_unlock(&p->lock);
            continue;
        }
        int64_t count = left > 512 ? 512 : left;
        int16_t *buffer = malloc((size_t)count * (size_t)p->channels * sizeof(int16_t));
        if (!buffer) { pthread_mutex_unlock(&p->lock); break; }
        // 汇编：音量渐变（音量突变不产生咔哒声）+ 饱和钳制。
        // ramp_gain 在持锁状态下读写，避免与 pause/seek 的清零竞争；
        // 锁内 DSP 很快，只影响控制接口的微秒级等待。
        dsp_vol_s16(p->pcm + start * p->channels, buffer,
                    (uint32_t)(count * p->channels), &p->ramp_gain, volume);
        pthread_mutex_unlock(&p->lock);
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
    if (is_wav_path(path)) {
        music_audio *wav = load_wav_direct(path);
        if (wav) {
            return wav;
        }
    }

    AVFormatContext *format = NULL;
    if (avformat_open_input(&format, path, NULL, NULL) < 0 || avformat_find_stream_info(format, NULL) < 0) {
        set_error(error, error_len, "FFmpeg 无法打开音频文件"); if (format) avformat_close_input(&format); return NULL;
    }
    // ---- 元数据（标题/艺术家/专辑/作曲家/日期/风格）----
    music_audio *p = calloc(1, sizeof(*p));
    if (!p) { avformat_close_input(&format); set_error(error, error_len, "内存不足"); return NULL; }
    copy_meta(p->meta_title, sizeof(p->meta_title), format->metadata, "title");
    copy_meta(p->meta_artist, sizeof(p->meta_artist), format->metadata, "artist");
    copy_meta(p->meta_album, sizeof(p->meta_album), format->metadata, "album");
    copy_meta(p->meta_composer, sizeof(p->meta_composer), format->metadata, "composer");
    copy_meta(p->meta_date, sizeof(p->meta_date), format->metadata, "date");
    copy_meta(p->meta_genre, sizeof(p->meta_genre), format->metadata, "genre");

    // ---- 内嵌封面（MP3 APIC / FLAC PICTURE / M4A covr）----
    int art_loaded = load_attached_picture_from_format(p, format);

    int stream = av_find_best_stream(format, AVMEDIA_TYPE_AUDIO, -1, -1, NULL, 0);
    if (stream < 0) { set_error(error, error_len, "文件中没有音频流"); avformat_close_input(&format); music_audio_close(p); return NULL; }
    AVCodecParameters *params = format->streams[stream]->codecpar;
    const AVCodec *codec = avcodec_find_decoder(params->codec_id);
    AVCodecContext *ctx = codec ? avcodec_alloc_context3(codec) : NULL;
    if (!ctx || avcodec_parameters_to_context(ctx, params) < 0 || avcodec_open2(ctx, codec, NULL) < 0) {
        set_error(error, error_len, "FFmpeg 无法初始化音频解码器"); if (ctx) avcodec_free_context(&ctx); avformat_close_input(&format); music_audio_close(p); return NULL;
    }
    int rate = ctx->sample_rate > 0 ? ctx->sample_rate : 48000;
    int channels = 2;
    AVChannelLayout output_layout = AV_CHANNEL_LAYOUT_STEREO;
    SwrContext *swr = NULL;
    if (swr_alloc_set_opts2(&swr, &output_layout, AV_SAMPLE_FMT_S16, rate,
                            &ctx->ch_layout, ctx->sample_fmt, ctx->sample_rate, 0, NULL) < 0 ||
        swr_init(swr) < 0) {
        set_error(error, error_len, "FFmpeg 音频重采样初始化失败"); if (swr) swr_free(&swr); avcodec_free_context(&ctx); avformat_close_input(&format); music_audio_close(p); return NULL;
    }
    p->sample_rate = rate; p->channels = channels; p->volume = 0.8f;
    p->paused = 0;
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
    // 封面在打开阶段未能直接取得时，回退重读（封面不随播放线程竞争，安全）
    if (!art_loaded) load_attached_picture_fallback(p, path);
    if (!p->pcm || p->frames == 0) { set_error(error, error_len, "音频文件没有可播放数据"); music_audio_close(p); return NULL; }
    if (pthread_create(&p->thread, NULL, audio_thread, p) != 0) { set_error(error, error_len, "无法创建音频播放线程"); music_audio_close(p); return NULL; }
    p->thread_started = 1;
    return p;
}

int music_audio_play(music_audio *p) { if (!p) return -1; pthread_mutex_lock(&p->lock); p->paused = 0; p->playing = 1; p->finished = 0; pthread_cond_signal(&p->wake); pthread_mutex_unlock(&p->lock); return 0; }
int music_audio_pause(music_audio *p) { if (!p) return -1; pthread_mutex_lock(&p->lock); p->paused = 1; p->playing = 0; p->ramp_gain = 0.0f; pthread_cond_signal(&p->wake); pthread_mutex_unlock(&p->lock); return 0; }
int music_audio_seek(music_audio *p, int64_t ms) {
    if (!p) return -1;
    pthread_mutex_lock(&p->lock);
    int was_finished = p->finished;
    p->cursor = (ms < 0 ? 0 : ms > p->frames * 1000 / p->sample_rate ? p->frames : ms * p->sample_rate / 1000);
    p->ramp_gain = 0.0f;
    p->generation++;
    p->finished = 0;
    /* A seek made after natural EOF is a new play request.  An explicit
     * pause remains sticky, matching MIDI/TUI behavior. */
    if (was_finished && !p->paused && !p->stop) p->playing = 1;
    pthread_cond_signal(&p->wake);
    pthread_mutex_unlock(&p->lock);
    return 0;
}
int64_t music_audio_position_ms(music_audio *p) { if (!p) return 0; pthread_mutex_lock(&p->lock); int64_t v = p->cursor * 1000 / p->sample_rate; pthread_mutex_unlock(&p->lock); return v; }
int64_t music_audio_duration_ms(music_audio *p) { return p ? p->frames * 1000 / p->sample_rate : 0; }
int music_audio_finished(music_audio *p) { if (!p) return 1; pthread_mutex_lock(&p->lock); int v = p->finished; pthread_mutex_unlock(&p->lock); return v; }
void music_audio_set_volume(music_audio *p, float v) { if (!p) return; pthread_mutex_lock(&p->lock); p->volume = v < 0.0f ? 0.0f : (v > 5.0f ? 5.0f : v); pthread_mutex_unlock(&p->lock); }
float music_audio_volume(music_audio *p) { if (!p) return 0.8f; pthread_mutex_lock(&p->lock); float v = p->volume; pthread_mutex_unlock(&p->lock); return v; }

// ---- 元数据 / 封面查询 ----
const char *music_audio_metadata(music_audio *p, const char *key) {
    if (!p || !key) return NULL;
    if (strcmp(key, "title") == 0) return p->meta_title[0] ? p->meta_title : NULL;
    if (strcmp(key, "artist") == 0) return p->meta_artist[0] ? p->meta_artist : NULL;
    if (strcmp(key, "album") == 0) return p->meta_album[0] ? p->meta_album : NULL;
    if (strcmp(key, "composer") == 0) return p->meta_composer[0] ? p->meta_composer : NULL;
    if (strcmp(key, "date") == 0) return p->meta_date[0] ? p->meta_date : NULL;
    if (strcmp(key, "genre") == 0) return p->meta_genre[0] ? p->meta_genre : NULL;
    return NULL;
}

int music_audio_art(music_audio *p, const unsigned char **data, int *width, int *height) {
    if (!p || !p->art) return 0;
    if (data) *data = p->art;
    if (width) *width = p->art_w;
    if (height) *height = p->art_h;
    return 1;
}
void music_audio_spectrum(music_audio *p, uint8_t levels[16]) {
    if (!levels) return;
    memset(levels, 0, 16);
    if (!p) return;
    pthread_mutex_lock(&p->lock);
    int64_t start = p->cursor;
    int64_t count = p->frames - start;
    if (count > 1024) count = 1024;
    if (count < 32) { pthread_mutex_unlock(&p->lock); return; }
    // 汇编：16 段对数频带 Goertzel 频谱（4 路 SSE 并行）
    dsp_spectrum_s16(p->pcm + start * p->channels, (uint32_t)count,
                     (uint32_t)p->channels, (uint32_t)p->sample_rate, levels);
    pthread_mutex_unlock(&p->lock);
}
void music_audio_close(music_audio *p) {
    if (!p) return;
    if (p->thread_started) {
        pthread_mutex_lock(&p->lock); p->stop = 1; p->playing = 1; pthread_cond_signal(&p->wake); pthread_mutex_unlock(&p->lock);
        pthread_join(p->thread, NULL);
    }
    free(p->pcm);
    free(p->art);
    pthread_cond_destroy(&p->wake);
    pthread_mutex_destroy(&p->lock);
    free(p);
}
