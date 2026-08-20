// music_asm.h —— music_rust 汇编优化例程接口
// Copyright (C) 2026 FuturePioneer-3
// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef MUSIC_ASM_H
#define MUSIC_ASM_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// 饱和缩放：dst[i] = sat16(src[i] * vol)（8 样本 SSE2 主循环）。
// 注意：src 与 dst 不得重叠。
void music_asm_apply_volume_s16(const int16_t *src, int16_t *dst, size_t n, float vol);

// 同时更新 4 个 Goertzel 谐振器（SSE2 四通道并行）：
//   for i in 0..n:
//     q0_k = coeffs[k]*q1[k] - q2[k] + samples[i]
//     q2[k] = q1[k];  q1[k] = q0_k
// q1/q2 为输入输出参数（初值 0，扫描结束后为最终状态）。
void music_asm_goertzel4(const float *samples, size_t n,
                         const float coeffs[4], float q1[4], float q2[4]);

// 音频峰值限制器：峰值 abs-max → 平滑增益 → 逐样本应用 + 硬钳制 ±target。
// *gain 为输入输出（当前增益包络）。峰值 <= 0 时直接返回。
void music_asm_limiter_process(float *buf, size_t n, float target,
                               float attack, float release, float *gain);

#ifdef __cplusplus
}
#endif

#endif
