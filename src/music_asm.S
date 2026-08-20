# music_asm.S —— music_rust 汇编优化例程（GNU as，AT&T 语法，非内联）
# Copyright (C) 2026 FuturePioneer-3
# SPDX-License-Identifier: GPL-3.0-or-later
#
# 三个热点函数全部用独立汇编实现（不内联），只使用 SSE2 指令集
# （x86-64 基线），保证在任意 x86-64 处理器上可用。
#
#  SysV AMD64 调用约定：
#   rdi/rsi/rdx/rcx/r8/r9 = 整数参数
#   xmm0-xmm7             = 浮点参数
#   rax/rcx/rdx/rsi/rdi/r8-r11/xmm0-xmm15 = 调用者保存
#   rbx/rbp/r12-r15       = 被调用者保存
#
# 注意：本文件内不使用任何绝对寻址（全部 RIP-relative / 寄存器寻址），
# 因此天然与 PIE 可执行文件兼容。

    .text

# ---------------------------------------------------------------------------
# void music_asm_apply_volume_s16(const int16_t *src, int16_t *dst, size_t n, float vol)
#
#   dst[i] = sat16(src[i] * vol)    （8 样本 SSE2 主循环 + 标量尾部）
#
#   每次迭代：8×s16 → 2×4×s32（符号扩展）→ cvtdq2ps → mulps 缩放 →
#   cvtps2dq（就近舍入）→ packssdw 饱和打包回 8×s16。
#   packssdw 的饱和钳制与 C 版本的 32767/-32768 硬钳制等价。
#
#   要求：src 与 dst 不重叠（原地调用会重复缩放）。
# ---------------------------------------------------------------------------
    .globl  music_asm_apply_volume_s16
    .type   music_asm_apply_volume_s16, @function
music_asm_apply_volume_s16:
    testq   %rdx, %rdx
    jz      .Lvol_done
    shufps  $0, %xmm0, %xmm0          # 广播 vol 到全部通道
    movq    %rdx, %rcx
    shrq    $3, %rcx                  # 8 样本组数
    jz      .Lvol_tail
.Lvol_loop:
    movdqu  (%rdi), %xmm1             # 8 个 s16
    movdqa  %xmm1, %xmm2
    punpcklwd %xmm1, %xmm2            # [a0,a0,a1,a1,a2,a2,a3,a3] (words)
    psrad   $16, %xmm2                # 符号扩展 → [a0,a1,a2,a3] 为 s32
    punpckhwd %xmm1, %xmm1            # [a4,a4,a5,a5,a6,a6,a7,a7] (words)
    psrad   $16, %xmm1                # 符号扩展 → [a4,a5,a6,a7] 为 s32
    cvtdq2ps %xmm2, %xmm2
    cvtdq2ps %xmm1, %xmm1
    mulps   %xmm0, %xmm2
    mulps   %xmm0, %xmm1
    cvtps2dq %xmm2, %xmm2
    cvtps2dq %xmm1, %xmm1
    packssdw %xmm1, %xmm2             # 饱和打包 → 8 个 s16
    movdqu  %xmm2, (%rsi)
    addq    $16, %rdi
    addq    $16, %rsi
    decq    %rcx
    jnz     .Lvol_loop
.Lvol_tail:
    andq    $7, %rdx
    jz      .Lvol_done
.Lvol_tail_loop:
    movswl  (%rdi), %eax              # 符号扩展 s16 → s32
    cvtsi2ssl %eax, %xmm1
    mulss   %xmm0, %xmm1
    cvtps2dq %xmm1, %xmm1             # 就近舍入到整数
    packssdw %xmm1, %xmm1             # 饱和（低 16 位即结果）
    movd    %xmm1, %eax
    movw    %ax, (%rsi)
    addq    $2, %rdi
    addq    $2, %rsi
    decq    %rdx
    jnz     .Lvol_tail_loop
.Lvol_done:
    ret
    .size   music_asm_apply_volume_s16, .-music_asm_apply_volume_s16

# ---------------------------------------------------------------------------
# void music_asm_goertzel4(const float *samples, size_t n,
#                          const float coeffs[4], float q1[4], float q2[4])
#
#   对 4 个 Goertzel 谐振器做一次完整扫描（SSE2 四通道并行）：
#     for i in 0..n:
#       q0_k = coeffs[k] * q1[k] - q2[k] + samples[i]   (k = 0..3)
#       q2[k] = q1[k]
#       q1[k] = q0_k
#
#   C 侧把 16 个频段分成 4 组各调用一次本函数。
#   相比标量实现，每样本指令数约为原来的 1/4。
# ---------------------------------------------------------------------------
    .globl  music_asm_goertzel4
    .type   music_asm_goertzel4, @function
music_asm_goertzel4:
    testq   %rsi, %rsi
    jz      .Lgz_done
    movups  (%rdx), %xmm2             # coeffs[4]
    movups  (%rcx), %xmm0             # q1[4]
    movups  (%r8),  %xmm1             # q2[4]
.Lgz_loop:
    movss   (%rdi), %xmm3             # 当前样本
    pshufd  $0, %xmm3, %xmm3          # 广播到 4 通道
    movaps  %xmm0, %xmm4
    mulps   %xmm2, %xmm4              # q0 = coeff * q1
    subps   %xmm1, %xmm4              #      - q2
    addps   %xmm3, %xmm4              #      + sample
    movaps  %xmm0, %xmm1              # q2 = q1
    movaps  %xmm4, %xmm0              # q1 = q0
    addq    $4, %rdi
    decq    %rsi
    jnz     .Lgz_loop
    movups  %xmm0, (%rcx)             # 回写 q1
    movups  %xmm1, (%r8)              # 回写 q2
.Lgz_done:
    ret
    .size   music_asm_goertzel4, .-music_asm_goertzel4

# ---------------------------------------------------------------------------
# void music_asm_limiter_process(float *buf, size_t n, float target,
#                                float attack, float release, float *gain)
#
#   音频峰值限制器（原 Rust LimiterState::process 的热循环）：
#     1) 第一遍求峰值 abs-max（SSE2 maxps，4 样本/迭代）
#     2) target_gain = min(target / peak, 1.0)   （只压缩不提升）
#     3) diff = target_gain - *gain
#        *gain += diff * (diff < 0 ? attack : release)
#     4) 第二遍：x = clamp(buf[i] * gain, ±target)（minps/maxps 向量钳制）
#
#   峰值 <= 0（静音）时直接返回，增益保持不变。
#   所有常数只读 RIP-relative，无绝对寻址。
# ---------------------------------------------------------------------------
    .globl  music_asm_limiter_process
    .type   music_asm_limiter_process, @function
music_asm_limiter_process:
    testq   %rsi, %rsi
    jz      .Llim_done
    movq    %rdi, %r8                 # 保存 buf 起始
    movq    %rsi, %r9                 # 保存 n
    movaps  %xmm0, %xmm7              # 保存 target（后面还要用）
    pxor    %xmm3, %xmm3              # 峰值累加器 = 0
    pcmpeqd %xmm5, %xmm5
    psrld   $1, %xmm5                 # xmm5 = 0x7fffffff（绝对值掩码）
    movq    %rsi, %rcx
    shrq    $2, %rcx                  # 4 样本组数
    jz      .Llim_peak_tail
.Llim_peak_loop:
    movups  (%rdi), %xmm4
    andps   %xmm5, %xmm4              # |x|
    maxps   %xmm4, %xmm3
    addq    $16, %rdi
    decq    %rcx
    jnz     .Llim_peak_loop
.Llim_peak_tail:
    andq    $3, %rsi
    jz      .Llim_peak_combine
.Llim_peak_tail_loop:
    movss   (%rdi), %xmm4
    andps   %xmm5, %xmm4
    maxss   %xmm4, %xmm3
    addq    $4, %rdi
    decq    %rsi
    jnz     .Llim_peak_tail_loop
.Llim_peak_combine:
    movaps  %xmm3, %xmm4
    shufps  $0x1b, %xmm4, %xmm4       # 反转 [x3,x2,x1,x0]
    maxps   %xmm4, %xmm3              # lane0=max(x0,x3) lane1=max(x1,x2)
    movaps  %xmm3, %xmm4
    shufps  $0x8e, %xmm4, %xmm4       # 交换相邻对
    maxps   %xmm4, %xmm3              # lane0 = 全局峰值
    xorps   %xmm6, %xmm6
    ucomiss %xmm6, %xmm3              # peak vs 0.0
    jbe     .Llim_done                # peak <= 0：静音，返回
    divss   %xmm3, %xmm7              # target_gain = target / peak
    movss   .Llim_one(%rip), %xmm4
    minss   %xmm4, %xmm7              # min(target_gain, 1.0)
    movss   (%rdx), %xmm3             # *gain
    movaps  %xmm7, %xmm4
    subss   %xmm3, %xmm4              # diff = target_gain - *gain
    xorps   %xmm5, %xmm5
    ucomiss %xmm5, %xmm4              # diff vs 0.0
    jae     .Llim_release             # diff >= 0 → release
    movaps  %xmm1, %xmm6              # coef = attack
    jmp     .Llim_gain_update
.Llim_release:
    movaps  %xmm2, %xmm6              # coef = release
.Llim_gain_update:
    mulss   %xmm4, %xmm6              # diff * coef
    addss   %xmm6, %xmm3              # *gain += ...
    movss   %xmm3, (%rdx)
    pshufd  $0, %xmm3, %xmm3          # 广播增益 g
    pshufd  $0, %xmm0, %xmm0          # 广播 target
    xorps   %xmm4, %xmm4
    subps   %xmm0, %xmm4              # -target
    movq    %r9, %rsi                 # 恢复 n
    movq    %r8, %rdi                 # 恢复 buf
    movq    %rsi, %rcx
    shrq    $2, %rcx
    jz      .Llim_apply_tail
.Llim_apply_loop:
    movups  (%rdi), %xmm5
    mulps   %xmm3, %xmm5              # x * g
    minps   %xmm0, %xmm5              # clamp 上限 +target
    maxps   %xmm4, %xmm5              # clamp 下限 -target
    movups  %xmm5, (%rdi)
    addq    $16, %rdi
    decq    %rcx
    jnz     .Llim_apply_loop
.Llim_apply_tail:
    andq    $3, %rsi
    jz      .Llim_done
.Llim_apply_tail_loop:
    movss   (%rdi), %xmm5
    mulss   %xmm3, %xmm5
    minss   %xmm0, %xmm5
    maxss   %xmm4, %xmm5
    movss   %xmm5, (%rdi)
    addq    $4, %rdi
    decq    %rsi
    jnz     .Llim_apply_tail_loop
.Llim_done:
    ret
    .size   music_asm_limiter_process, .-music_asm_limiter_process

    .section .rodata
    .align 4
.Llim_one:
    .float 1.0

    .section .note.GNU-stack,"",@progbits
