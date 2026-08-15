// music_rust —— x86-64 汇编音频 DSP 接口
// Copyright (C) 2026 FuturePioneer-3
// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef MUSIC_AUDIO_DSP_H
#define MUSIC_AUDIO_DSP_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// 限制器状态（4 个 f32，与 Rust 侧 LimiterState 布局一致）
typedef struct dsp_limiter {
    float target;        /* 目标峰值电平（线性满刻度） */
    float current_gain;  /* 当前增益（平滑包络） */
    float attack_coef;   /* 压降系数（0..1，越大越快） */
    float release_coef;  /* 恢复系数（0..1，越大越快） */
} dsp_limiter;

/*
 * 16 位样本音量渐变 + 饱和钳制（SSE2 汇编）。
 * in/out 可为同一缓冲区；samples 为 int16 样本总数（帧数 × 声道数）。
 * gain 为渐变状态：从 *gain 线性渐变到 target，防止音量突变产生爆音。
 * 函数返回时 *gain 已被更新为本次缓冲区末尾的实际增益。
 */
void dsp_vol_s16(const int16_t *in, int16_t *out, uint32_t samples,
                 float *gain, float target);

/*
 * f32 缓冲块级峰值限制器（SSE2 汇编）：
 *   1. 求块峰值；2. 目标增益 = min(1, target/peak)，经 attack/release 平滑；
 *   3. 整块应用增益并硬钳制到 ±target。
 */
void dsp_limiter_f32(float *buf, uint32_t len, dsp_limiter *st);

/*
 * 16 段对数频带 Goertzel 频谱分析（SSE2 汇编，4 频带并行）。
 * pcm 为 int16 交错样本，samples 为帧数，channels 为声道数（取第一声道）。
 * 结果写入 levels[0..15]，每段 0..7。
 */
void dsp_spectrum_s16(const int16_t *pcm, uint32_t samples, uint32_t channels,
                      uint32_t sample_rate, uint8_t levels[16]);

#ifdef __cplusplus
}
#endif

#endif /* MUSIC_AUDIO_DSP_H */
