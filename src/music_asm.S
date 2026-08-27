# music_asm.S —— music_rust 汇编优化例程（GNU as，AT&T 语法，非内联）
# Copyright (C) 2026 FuturePioneer-3
# SPDX-License-Identifier: GPL-3.0-or-later
#
# 三个热点函数全部用独立汇编实现（不内联），只使用 SSE2 指令集
# （x86-64 基线），保证在任意 x86-64 处理器上可用：
#
#   music_asm_apply_volume_s16  8 样本/轮的 int16 饱和缩放
#   music_asm_goertzel4         4 个 Goertzel 谐振器并行扫描（SSE 4 通道）
#   music_asm_limiter_process   f32 峰值限制器（与 audio_dsp.S 的
#                               dsp_limiter_f32 同算法，接口略不同）
#
# SysV AMD64 调用约定：
#   rdi/rsi/rdx/rcx/r8/r9 = 整数/指针参数
#   xmm0-xmm7             = 浮点参数（与整数参数按位置对应）
#   rax/rcx/rdx/rsi/rdi/r8-r11/xmm0-xmm15 = 调用者保存
#   rbx/rbp/r12-r15       = 被调用者保存
#
# 注意：本文件内不使用任何绝对寻址（全部 RIP-relative / 寄存器寻址），
# 因此天然与 PIE 可执行文件兼容。所有只读常量放 .rodata，访问统一
# 写成 .Llim_one(%rip) 这种形式。
#
# 与 audio_dsp.S 的分工：
#   audio_dsp.S 里的 dsp_limiter_f32 供音频文件播放（C 侧 ALSA 线程）
#   使用，结构体是 dsp_limiter；本文件的 music_asm_limiter_process 供
#   Rust 侧（synth.rs 的 LimiterState）调用，参数是分散的 float 而不是
#   结构体指针。两者算法一致，但接口和历史调用方不同，故保留两份。

    .text

# ---------------------------------------------------------------------------
# void music_asm_apply_volume_s16(const int16_t *src, int16_t *dst, size_t n, float vol)
#
# 功能：dst[i] = sat16(src[i] * vol)，n 个 int16 样本，vol 是浮点倍率。
#       这是测试基准/辅助路径用的“瞬时缩放”（无渐变）；带渐变防爆音的
#       版本是 audio_dsp.S 的 dsp_vol_s16。
#
# 实现：8 样本 SSE2 主循环 + 标量尾部。
#   每次迭代：
#     1) movdqu 读 16 字节（8 个 int16）；
#     2) 用“自身复制 + punpcklwd/h + psrad $16”把 8 个 int16 符号扩展成
#        两组 4×int32（SSE 没有直接的 s16→s32 转换指令，这是标准手法）；
#     3) cvtdq2ps 转 float，mulps 乘上广播后的 vol；
#     4) cvtps2dq 就近舍入回整数；
#     5) packssdw 把两组 4×int32 饱和打包回 8×int16。
#   packssdw 的饱和行为（< -32768 钳 -32768，> 32767 钳 32767）与
#   C 基准实现的硬钳制完全等价。
#
#   尾部（剩余 1..7 个）：逐个处理，乘完用 packssdw 对单元素做饱和
#   （低 16 位即结果），省掉手动比较钳制。
#
# 重要约束：src 与 dst 不得重叠。原地调用会把已经放大过的样本再乘一遍
# vol（重复缩放），结果错误。
#
# 寄存器分工：
#   rdi = src        rsi = dst
#   rdx = 剩余样本数（同时做尾部分支判断）
#   xmm0 = vol（shufps $0 广播到 4 通道）   rcx = 8 样本组数
#   xmm1/xmm2 = 两组的 8 样本数据
# ---------------------------------------------------------------------------
    .globl  music_asm_apply_volume_s16
    .type   music_asm_apply_volume_s16, @function
music_asm_apply_volume_s16:
    # n == 0：无事可做，直接返回。
    testq   %rdx, %rdx
    jz      .Lvol_done
    # shufps $0 把 xmm0 的第 0 通道复制到全部 4 个通道，即广播 vol。
    shufps  $0, %xmm0, %xmm0          # 广播 vol 到全部通道
    # 组数 = n / 8；为 0 说明不足 8 个，直接走标量尾部。
    movq    %rdx, %rcx
    shrq    $3, %rcx                  # 8 样本组数
    jz      .Lvol_tail
.Lvol_loop:
    # 读 16 字节 = 8 个 int16：[a0 a1 a2 a3 a4 a5 a6 a7]。
    movdqu  (%rdi), %xmm1             # 8 个 s16
    # 低 4 个样本：先把 xmm1 复制到 xmm2，punpcklwd 把两个操作数的低
    # 64 位交织成 [a0 a0 a1 a1 a2 a2 a3 a3]（32 位字），再算术右移 16 位
    # 得到符号扩展的 [a0 a1 a2 a3]（int32）。
    movdqa  %xmm1, %xmm2
    punpcklwd %xmm1, %xmm2            # [a0,a0,a1,a1,a2,a2,a3,a3] (words)
    psrad   $16, %xmm2                # 符号扩展 → [a0,a1,a2,a3] 为 s32
    # 高 4 个样本：punpckhwd 交织两个操作数的高 64 位，得到
    # [a4,a4,a5,a5,a6,a6,a7,a7]，同样算术右移 16 位。
    punpckhwd %xmm1, %xmm1            # [a4,a4,a5,a5,a6,a6,a7,a7] (words)
    psrad   $16, %xmm1                # 符号扩展 → [a4,a5,a6,a7] 为 s32
    # int32 → float → 乘 vol → 就近舍入回 int32。
    cvtdq2ps %xmm2, %xmm2
    cvtdq2ps %xmm1, %xmm1
    mulps   %xmm0, %xmm2
    mulps   %xmm0, %xmm1
    cvtps2dq %xmm2, %xmm2
    cvtps2dq %xmm1, %xmm1
    # packssdw 把 xmm1（高组）和 xmm2（低组）饱和打包成 8×int16，
    # 结果顺序正确：xmm2 的低 64 位 = 前 4 个样本，高 64 位 = 后 4 个。
    packssdw %xmm1, %xmm2             # 饱和打包 → 8 个 s16
    movdqu  %xmm2, (%rsi)
    addq    $16, %rdi                  # 源前进 16 字节 = 8 个样本
    addq    $16, %rsi
    decq    %rcx
    jnz     .Lvol_loop

.Lvol_tail:
    # 剩余样本数 = n % 8；为 0 说明整除，收工。
    andq    $7, %rdx
    jz      .Lvol_done
.Lvol_tail_loop:
    # 逐个处理：符号扩展成 int32 → 转 float → 乘 vol → 转回整数 →
    # packssdw 对单元素饱和（结果在低 16 位）→ 存低 16 位。
    movswl  (%rdi), %eax              # 符号扩展 s16 → s32
    cvtsi2ssl %eax, %xmm1
    mulss   %xmm0, %xmm1
    cvtps2dq %xmm1, %xmm1             # 就近舍入到整数
    packssdw %xmm1, %xmm1             # 饱和（低 16 位即结果）
    movd    %xmm1, %eax
    movw    %ax, (%rsi)
    addq    $2, %rdi                   # 源前进 2 字节
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
# 功能：对 4 个 Goertzel 谐振器做一次完整扫描（SSE2 四通道并行）：
#     for i in 0..n:
#       q0_k = coeffs[k] * q1[k] - q2[k] + samples[i]   (k = 0..3)
#       q2[k] = q1[k]
#       q1[k] = q0_k
#
#   Goertzel 是“只关心少数几个频率”时比 FFT 更省事的算法：每个频带
#   只需要一个系数 c=2*cos(2πf/sr) 和两条递推状态 q1/q2，扫完样本后
#   按固定公式（见 audio_dsp.S 的 dsp_spectrum_s16）即可算出幅值。
#
#   C 侧（synth.rs 的频谱显示路径）把 16 个频段分成 4 组，每组调用
#   一次本函数；q1/q2 作为输入输出参数，可以分段扫描、跨调用累积状态。
#
#   SSE 并行方式：4 个谐振器各占一个通道。每个样本只读一次、广播到
#   4 通道，然后同时做 4 路的乘加递推——每样本指令数约为标量实现的
#   1/4。
#
# 寄存器分工：
#   rdi = samples 指针（每处理一个样本前进 4 字节）
#   rsi = 剩余样本数（循环计数）
#   xmm2 = 4 个系数（movups 载入，允许未对齐）
#   xmm0 = q1 向量（输入输出）   xmm1 = q2 向量（输入输出）
#   xmm3 = 当前样本（广播）       xmm4 = q0 暂存
# ---------------------------------------------------------------------------
    .globl  music_asm_goertzel4
    .type   music_asm_goertzel4, @function
music_asm_goertzel4:
    # n == 0：不扫描，原样返回（q1/q2 保持调用前的状态）。
    testq   %rsi, %rsi
    jz      .Lgz_done
    # 载入 4 个系数和两组递推状态。用 movups（未对齐版本），因为调用方
    # 可能传栈上的普通数组，不保证 16 字节对齐。
    movups  (%rdx), %xmm2             # coeffs[4]
    movups  (%rcx), %xmm0             # q1[4]
    movups  (%r8),  %xmm1             # q2[4]
.Lgz_loop:
    # 读当前样本（标量 float），pshufd $0 广播到 4 个通道——4 个谐振器
    # 共用同一个输入样本。
    movss   (%rdi), %xmm3             # 当前样本
    pshufd  $0, %xmm3, %xmm3          # 广播到 4 通道
    # q0 = c*q1 - q2 + s（4 路同时算）；随后 q2 ← q1、q1 ← q0。
    movaps  %xmm0, %xmm4
    mulps   %xmm2, %xmm4              # q0 = coeff * q1
    subps   %xmm1, %xmm4              #      - q2
    addps   %xmm3, %xmm4              #      + sample
    movaps  %xmm0, %xmm1              # q2 = q1
    movaps  %xmm4, %xmm0              # q1 = q0
    addq    $4, %rdi                   # 前进一个 float
    decq    %rsi
    jnz     .Lgz_loop
    # 扫描结束，把最终状态写回调用方的数组（同样用未对齐存储）。
    movups  %xmm0, (%rcx)             # 回写 q1
    movups  %xmm1, (%r8)              # 回写 q2
.Lgz_done:
    ret
    .size   music_asm_goertzel4, .-music_asm_goertzel4

# ---------------------------------------------------------------------------
# void music_asm_limiter_process(float *buf, size_t n, float target,
#                                float attack, float release, float *gain)
#
# 功能：音频峰值限制器（Rust 侧 LimiterState::process 的热循环），防止
#       合成器输出削波。n 是 size_t（64 位），float 参数按 SysV 约定放
#       xmm0（target）、xmm1（attack）、xmm2（release），增益指针在 rdx。
#       算法与 audio_dsp.S 的 dsp_limiter_f32 完全一致，只是接口不同：
#       dsp_limiter_f32 传结构体指针，这里直接传三个 float。
#
#   步骤：
#     1) 第一遍求峰值 abs-max（SSE2 每轮 4 样本 + 标量尾部）；
#     2) target_gain = min(target / peak, 1.0)  （只压缩，不提升音量）；
#     3) diff = target_gain - *gain；
#        *gain += diff * (diff < 0 ? attack : release)
#        —— 峰值超限时用 attack 快速压降，未超限时用 release 缓慢恢复，
#        避免频繁压缩造成“泵浦感”；
#     4) 第二遍：x = clamp(buf[i] * gain, ±target)（minps/maxps 向量钳制）。
#
#   峰值 <= 0（整块静音）时直接返回、增益保持不变：否则静音期间包络被
#   错误拉起，声音再次出现时会突然变大。
#
#   所有常数只读 RIP-relative，无绝对寻址，PIE 兼容。
#
# 寄存器分工：
#   rdi = buf（第一遍后保存到 r8，第二遍恢复）   rsi = n（保存到 r9）
#   xmm0 = target（保存到 xmm7，第一遍峰值扫描会用到 xmm0 的其它用途）
#   xmm1 = attack     xmm2 = release
#   rdx = *gain 指针（输入输出）      xmm3 = 峰值累加器 / 平滑增益
#   xmm5 = 绝对值掩码（第一遍）/ 第二遍的样本向量
#   rcx = 4 样本组数（第一遍与第二遍各自重新折算）
# ---------------------------------------------------------------------------
    .globl  music_asm_limiter_process
    .type   music_asm_limiter_process, @function
music_asm_limiter_process:
    # n == 0：空块直接返回。
    testq   %rsi, %rsi
    jz      .Llim_done
    # 第一遍会把 rdi 一路加过去，先把起点和 n 存到调用者保存寄存器里，
    # 第二遍还要用（本函数不 push/pop，直接借 r8/r9，因为它们本来就是
    # 调用者保存的，随便改）。
    movq    %rdi, %r8                 # 保存 buf 起始
    movq    %rsi, %r9                 # 保存 n
    # target 在计算 target_gain 时还要用，先挪到 xmm7 避开后续的
    # 峰值扫描（xmm0 会被峰值累加占用）。
    movaps  %xmm0, %xmm7              # 保存 target（后面还要用）
    # 峰值累加器清零。xmm5 构造绝对值掩码：pcmpeqd 把 4 个通道全置 1
    # （0xffffffff），psrld $1 逻辑右移 1 位得到 0x7fffffff。
    pxor    %xmm3, %xmm3              # 峰值累加器 = 0
    pcmpeqd %xmm5, %xmm5
    psrld   $1, %xmm5                 # xmm5 = 0x7fffffff（绝对值掩码）
    # 4 样本组数 = n / 4；不足一组直接跳标量尾部。
    movq    %rsi, %rcx
    shrq    $2, %rcx                  # 4 样本组数
    jz      .Llim_peak_tail
.Llim_peak_loop:
    # 每轮读 16 字节（4 个 float），与掩码按位与去掉符号位 = 取绝对值，
    # maxps 归并峰值，指针前进 16 字节、组数减一。
    movups  (%rdi), %xmm4
    andps   %xmm5, %xmm4              # |x|
    maxps   %xmm4, %xmm3
    addq    $16, %rdi
    decq    %rcx
    jnz     .Llim_peak_loop
.Llim_peak_tail:
    # 剩余 n % 4 个样本用标量逐个归并。
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
    # 把分散在 4 个通道的峰值归并成标量（第 0 通道）：
    #   1) shufps $0x1b 反转通道序 [x3,x2,x1,x0]，
    #      maxps 后 lane0=max(x0,x3)、lane1=max(x1,x2)；
    #   2) shufps $0x8e 交换相邻通道对 [x1,x0,x3,x2]，
    #      maxps 后 lane0 就是全局峰值。
    movaps  %xmm3, %xmm4
    shufps  $0x1b, %xmm4, %xmm4       # 反转 [x3,x2,x1,x0]
    maxps   %xmm4, %xmm3              # lane0=max(x0,x3) lane1=max(x1,x2)
    movaps  %xmm3, %xmm4
    shufps  $0x8e, %xmm4, %xmm4       # 交换相邻对
    maxps   %xmm4, %xmm3              # lane0 = 全局峰值

    # 静音块（peak <= 0）直接返回，保持 *gain 不变。
    # ucomiss 比较 peak 与 0：小于或等于都走 .Llim_done。
    xorps   %xmm6, %xmm6
    ucomiss %xmm6, %xmm3              # peak vs 0.0
    jbe     .Llim_done                # peak <= 0：静音，返回

    # target_gain = min(target / peak, 1.0)。peak 非 0，除法安全；
    # minss 保证只压缩不放大。
    divss   %xmm3, %xmm7              # target_gain = target / peak
    movss   .Llim_one(%rip), %xmm4
    minss   %xmm4, %xmm7              # min(target_gain, 1.0)

    # diff = target_gain - *gain；ucomiss 与 0 比较：
    # diff >= 0（需要恢复）走 release，diff < 0（需要压降）走 attack。
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
    # 一阶平滑：*gain += diff * coef，回写。
    mulss   %xmm4, %xmm6              # diff * coef
    addss   %xmm6, %xmm3              # *gain += ...
    movss   %xmm3, (%rdx)

    # 把平滑后的增益 g 和 target 各广播到 4 个通道，并构造 -target
    # 向量，供第二遍向量钳制使用。
    pshufd  $0, %xmm3, %xmm3          # 广播增益 g
    pshufd  $0, %xmm0, %xmm0          # 广播 target
    xorps   %xmm4, %xmm4
    subps   %xmm0, %xmm4              # -target

    # 恢复 buf 起点和 n，重新折算 4 样本组数，开始第二遍应用。
    movq    %r9, %rsi                 # 恢复 n
    movq    %r8, %rdi                 # 恢复 buf
    movq    %rsi, %rcx
    shrq    $2, %rcx
    jz      .Llim_apply_tail
.Llim_apply_loop:
    # 每轮 4 个样本：乘增益 → minps 钳上界 +target → maxps 钳下界
    # -target → 写回原处。顺序必须是“先乘后钳”，保证任何时刻不超 target。
    movups  (%rdi), %xmm5
    mulps   %xmm3, %xmm5              # x * g
    minps   %xmm0, %xmm5              # clamp 上限 +target
    maxps   %xmm4, %xmm5              # clamp 下限 -target
    movups  %xmm5, (%rdi)
    addq    $16, %rdi
    decq    %rcx
    jnz     .Llim_apply_loop
.Llim_apply_tail:
    # 剩余 n % 4 个样本用标量做同样的事。
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
