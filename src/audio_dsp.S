// music_rust —— x86-64 SSE2 汇编音频 DSP 内核
// Copyright (C) 2026 FuturePioneer-3
// SPDX-License-Identifier: GPL-3.0-or-later
//
// 本文件把音频热路径交给汇编处理，压榨 CPU 效率：
//   dsp_vol_s16       —— int16 音量渐变 + 饱和钳制（逐样本线性渐变，杜绝爆音）
//   dsp_limiter_f32   —— f32 峰值限制器（块峰值分析 + attack/release 平滑增益 + 硬钳制）
//   dsp_spectrum_s16  —— 16 段对数频带 Goertzel 频谱（4 频带 SSE 并行，比标量快 ~4 倍）
//
// 全部仅依赖 SSE2（x86-64 基线指令集），无需 AVX。
// 语法：AT&T（GNU as，由 cc crate 编译）。
//
// ABI：System V AMD64。整数参数 rdi/rsi/rdx/rcx/r8/r9，浮点参数 xmm0-7。

    .text

// ---------------------------------------------------------------------------
// void dsp_vol_s16(const int16_t *in /*rdi*/, int16_t *out /*rsi*/,
//                  uint32_t samples /*edx*/, float *gain /*rcx*/,
//                  float target /*xmm0*/)
// ---------------------------------------------------------------------------
    .globl  dsp_vol_s16
    .type   dsp_vol_s16,@function
dsp_vol_s16:
    testl   %edx, %edx
    jz      .Lvol_set_target           // samples == 0 → *gain = target
    movss   (%rcx), %xmm1              // g = *gain
    xorps   %xmm4, %xmm4
    maxss   %xmm4, %xmm1               // g = max(g, 0)
    movss   %xmm0, %xmm2
    subss   %xmm1, %xmm2               // target - g
    cvtsi2ssl %edx, %xmm3
    divss   %xmm3, %xmm2               // step = (target - g) / samples
    pshufd  $0x00, %xmm2, %xmm2        // [s,s,s,s]
    movaps  %xmm2, %xmm7
    addps   %xmm2, %xmm7               // [2s,...]
    addps   %xmm2, %xmm7               // [3s,...]
    addps   %xmm2, %xmm7               // xmm7 = [4s,4s,4s,4s]（每轮推进量）
    xorl    %eax, %eax                 // 样本下标
.Lvol_loop:
    cmpl    %edx, %eax
    jae     .Lvol_done
    leal    3(%rax), %r8d
    cmpl    %edx, %r8d
    jae     .Lvol_tail                 // 不足 4 个 → 标量尾部
    // ---- 重建本轮 4 路增益向量 vg = [g, g+s, g+2s, g+3s] ----
    pshufd  $0x00, %xmm1, %xmm0        // [g,g,g,g]
    movaps  VOL_RAMP(%rip), %xmm5      // [0,1,2,3]
    mulps   %xmm2, %xmm5               // [0,s,2s,3s]
    addps   %xmm5, %xmm0               // vg
    // ---- 4 样本 SSE 处理 ----
    movq    (%rdi,%rax,2), %xmm5       // 8 字节 = 4×int16
    punpcklwd %xmm5, %xmm5             // 复制为 4×dword
    psrad   $16, %xmm5                 // 算术右移 → 符号扩展的 int32
    cvtdq2ps %xmm5, %xmm5              // → float
    mulps   %xmm0, %xmm5               // × 渐变增益
    cvtps2dq %xmm5, %xmm5
    packssdw %xmm5, %xmm5              // 饱和 → 4×int16
    movq    %xmm5, (%rsi,%rax,2)       // 存 8 字节 = 4 个样本
    addss   %xmm7, %xmm1               // g += 4s
    addl    $4, %eax
    jmp     .Lvol_loop
.Lvol_tail:
    cmpl    %edx, %eax
    jae     .Lvol_done
    movswl  (%rdi,%rax,2), %r8d
    cvtsi2ssl %r8d, %xmm5
    mulss   %xmm1, %xmm5
    cvtss2sil %xmm5, %r8d
    cmpl    $32767, %r8d
    jle     .Lvol_t1
    movl    $32767, %r8d
    jmp     .Lvol_store
.Lvol_t1:
    cmpl    $-32768, %r8d
    jge     .Lvol_store
    movl    $-32768, %r8d
.Lvol_store:
    movw    %r8w, (%rsi,%rax,2)
    movss   %xmm1, %xmm5
    addss   %xmm2, %xmm5
    movss   %xmm5, %xmm1
    incl    %eax
    jmp     .Lvol_tail
.Lvol_done:
    movss   %xmm1, (%rcx)              // 回写当前增益（供下一块继续渐变）
    ret
.Lvol_set_target:
    movss   %xmm0, (%rcx)
    ret
    .size   dsp_vol_s16, .-dsp_vol_s16

// ---------------------------------------------------------------------------
// void dsp_limiter_f32(float *buf /*rdi*/, uint32_t len /*esi*/,
//                      dsp_limiter *st /*rdx*/)
//   结构体布局：target(0) current_gain(4) attack_coef(8) release_coef(12)
// ---------------------------------------------------------------------------
    .globl  dsp_limiter_f32
    .type   dsp_limiter_f32,@function
dsp_limiter_f32:
    testl   %esi, %esi
    jz      .Llim_ret
    movq    %rdi, %r8                  // 保存缓冲区起点（第二遍要用）
    xorps   %xmm0, %xmm0               // peak = 0
    movaps  ABS_MASK(%rip), %xmm5
    movl    %esi, %eax
    shrl    $2, %eax
.Llim_peak_loop:
    testl   %eax, %eax
    jz      .Llim_peak_tail
    movups  (%rdi), %xmm1
    andps   %xmm5, %xmm1               // |x|
    maxps   %xmm1, %xmm0
    addq    $16, %rdi
    decl    %eax
    jmp     .Llim_peak_loop
.Llim_peak_tail:
    movl    %esi, %eax
    andl    $3, %eax
.Llim_peak_tail_loop:
    testl   %eax, %eax
    jz      .Llim_peak_done
    movss   (%rdi), %xmm1
    andps   %xmm5, %xmm1
    maxss   %xmm1, %xmm0
    addq    $4, %rdi
    decl    %eax
    jmp     .Llim_peak_tail_loop
.Llim_peak_done:
    // 水平归约出标量峰值
    movaps  %xmm0, %xmm1
    shufps  $0x4e, %xmm1, %xmm1
    maxps   %xmm1, %xmm0
    movaps  %xmm0, %xmm1
    shufps  $0xb1, %xmm1, %xmm1
    maxss   %xmm1, %xmm0               // peak（lane 0）
    xorps   %xmm1, %xmm1
    ucomiss %xmm1, %xmm0
    je      .Llim_ret                  // 静音块
    // target_gain = min(1, target/peak)
    movss   0(%rdx), %xmm1
    divss   %xmm0, %xmm1
    movss   ONE(%rip), %xmm2
    minss   %xmm2, %xmm1
    movss   4(%rdx), %xmm2             // current_gain
    subss   %xmm2, %xmm1               // diff
    xorps   %xmm3, %xmm3
    ucomiss %xmm3, %xmm1               // CF = (diff < 0) ?
    jb      .Llim_attack
    movss   12(%rdx), %xmm3            // release
    jmp     .Llim_coef
.Llim_attack:
    movss   8(%rdx), %xmm3             // attack
.Llim_coef:
    mulss   %xmm3, %xmm1               // diff * coef
    addss   %xmm1, %xmm2
    movss   %xmm2, 4(%rdx)             // 回写平滑增益
    pshufd  $0x00, %xmm2, %xmm2        // [g,g,g,g]
    movss   0(%rdx), %xmm3             // target
    pshufd  $0x00, %xmm3, %xmm3        // [+lim,...]
    xorps   %xmm4, %xmm4
    subps   %xmm3, %xmm4               // 0 - lim（逐 lane 取负）
    // xmm4 = [-lim,...]
    movq    %r8, %rdi                  // 回到缓冲区起点
    movl    %esi, %eax
    shrl    $2, %eax
.Llim_apply_loop:
    testl   %eax, %eax
    jz      .Llim_apply_tail
    movups  (%rdi), %xmm0
    mulps   %xmm2, %xmm0               // × 平滑增益
    maxps   %xmm4, %xmm0
    minps   %xmm3, %xmm0               // 钳制 ±target
    movups  %xmm0, (%rdi)
    addq    $16, %rdi
    decl    %eax
    jmp     .Llim_apply_loop
.Llim_apply_tail:
    movl    %esi, %eax
    andl    $3, %eax
.Llim_apply_tail_loop:
    testl   %eax, %eax
    jz      .Llim_ret
    movss   (%rdi), %xmm0
    mulss   %xmm2, %xmm0
    maxss   %xmm4, %xmm0
    minss   %xmm3, %xmm0
    movss   %xmm0, (%rdi)
    addq    $4, %rdi
    decl    %eax
    jmp     .Llim_apply_tail_loop
.Llim_ret:
    ret
    .size   dsp_limiter_f32, .-dsp_limiter_f32

// ---------------------------------------------------------------------------
// void dsp_spectrum_s16(const int16_t *pcm /*rdi*/, uint32_t samples /*esi*/,
//                       uint32_t channels /*edx*/, uint32_t sample_rate /*ecx*/,
//                       uint8_t levels[16] /*r8*/)
//
// 16 个频带按 4 组 × 4 路 SSE 并行跑 Goertzel 递推：
//   q0 = c*q1 - q2 + s；q2=q1；q1=q0
// 幅值 = sqrt(q1² + q2² - c*q1*q2) / samples
// 电平 = log10(1 + 幅值*550) * 4，钳制 0..7
// ---------------------------------------------------------------------------
    .globl  dsp_spectrum_s16
    .type   dsp_spectrum_s16,@function
dsp_spectrum_s16:
    pushq   %rbx
    pushq   %rbp
    pushq   %r12
    pushq   %r13
    pushq   %r14
    pushq   %r15
    subq    $72, %rsp
    // 栈布局：0=coeff 16= q1 32= q2 48= inv_samples 52= channels
    movq    %rdi, %rbp                 // pcm
    movl    %esi, %r13d                // samples
    movl    %ecx, %r12d                // sample_rate
    movq    %r8, %r14                  // levels
    movl    %edx, 52(%rsp)             // channels（cosf 会践踏 edx，先入栈保存）
    // inv_samples = 1.0 / samples
    xorps   %xmm0, %xmm0
    cvtsi2ssl %r13d, %xmm0
    movss   ONE(%rip), %xmm1
    divss   %xmm0, %xmm1
    movss   %xmm1, 48(%rsp)
    xorl    %r15d, %r15d               // group = 0..3
.Lgroup_loop:
    cmpl    $4, %r15d
    jae     .Lspectrum_done
    movl    %r15d, %ebx
    shll    $2, %ebx                   // band 基准 = group*4
    // ---- 计算 4 个频带的 Goertzel 系数 c = 2*cos(2πf/sr) ----
    // lane 0
    lea     BAND_FREQ(%rip), %r11
    movss   (%r11,%rbx,4), %xmm0
    mulss   TWO_PI(%rip), %xmm0
    cvtsi2ssl %r12d, %xmm1
    divss   %xmm1, %xmm0
    call    cosf@PLT
    mulss   TWO(%rip), %xmm0
    movss   %xmm0, 0(%rsp)
    // lane 1
    leal    1(%rbx), %r10d
    lea     BAND_FREQ(%rip), %r11
    movss   (%r11,%r10,4), %xmm0
    mulss   TWO_PI(%rip), %xmm0
    cvtsi2ssl %r12d, %xmm1
    divss   %xmm1, %xmm0
    call    cosf@PLT
    mulss   TWO(%rip), %xmm0
    movss   %xmm0, 4(%rsp)
    // lane 2
    leal    2(%rbx), %r10d
    lea     BAND_FREQ(%rip), %r11
    movss   (%r11,%r10,4), %xmm0
    mulss   TWO_PI(%rip), %xmm0
    cvtsi2ssl %r12d, %xmm1
    divss   %xmm1, %xmm0
    call    cosf@PLT
    mulss   TWO(%rip), %xmm0
    movss   %xmm0, 8(%rsp)
    // lane 3
    leal    3(%rbx), %r10d
    lea     BAND_FREQ(%rip), %r11
    movss   (%r11,%r10,4), %xmm0
    mulss   TWO_PI(%rip), %xmm0
    cvtsi2ssl %r12d, %xmm1
    divss   %xmm1, %xmm0
    call    cosf@PLT
    mulss   TWO(%rip), %xmm0
    movss   %xmm0, 12(%rsp)
    movaps  0(%rsp), %xmm3             // coeff 向量
    xorps   %xmm1, %xmm1               // q1 = 0
    movaps  %xmm1, %xmm2               // q2 = 0
    movl    52(%rsp), %r9d             // channels（仅内层循环使用，无调用）
    xorl    %r10d, %r10d               // 样本下标
    xorl    %ecx, %ecx                 // 循环计数
.Linner_loop:
    cmpl    %r13d, %ecx
    jae     .Linner_done
    movswl  (%rbp,%r10,2), %eax
    cvtsi2ssl %eax, %xmm4
    pshufd  $0x00, %xmm4, %xmm4
    mulps   INV_32768(%rip), %xmm4     // s = sample / 32768
    movaps  %xmm3, %xmm5
    mulps   %xmm1, %xmm5               // c*q1
    subps   %xmm2, %xmm5               // c*q1 - q2
    addps   %xmm4, %xmm5               // + s
    movaps  %xmm1, %xmm2               // q2 = q1
    movaps  %xmm5, %xmm1               // q1 = q0
    addl    %r9d, %r10d
    incl    %ecx
    jmp     .Linner_loop
.Linner_done:
    movaps  %xmm1, 16(%rsp)            // 保存 q1
    movaps  %xmm2, 32(%rsp)            // 保存 q2
    // ---- 逐频带收尾（4 路展开）----
    movss   16(%rsp), %xmm8
    movss   32(%rsp), %xmm9
    movss   0(%rsp), %xmm10
    call    .Lspec_mag
    movb    %al, (%r14,%rbx)
    movss   20(%rsp), %xmm8
    movss   36(%rsp), %xmm9
    movss   4(%rsp), %xmm10
    call    .Lspec_mag
    movb    %al, 1(%r14,%rbx)
    movss   24(%rsp), %xmm8
    movss   40(%rsp), %xmm9
    movss   8(%rsp), %xmm10
    call    .Lspec_mag
    movb    %al, 2(%r14,%rbx)
    movss   28(%rsp), %xmm8
    movss   44(%rsp), %xmm9
    movss   12(%rsp), %xmm10
    call    .Lspec_mag
    movb    %al, 3(%r14,%rbx)
    incl    %r15d
    jmp     .Lgroup_loop
.Lspectrum_done:
    addq    $72, %rsp
    popq    %r15
    popq    %r14
    popq    %r13
    popq    %r12
    popq    %rbp
    popq    %rbx
    ret

// 子过程：输入 xmm8=q1 xmm9=q2 xmm10=c，输出 eax = 电平 0..7
// 注意：此时 rsp%16==0，本子过程入栈后 rsp%16==8，调 libm 前需再 -8。
// 调用方栈上 48(%rsp) 的 inv_samples 在本子过程内为 56(%rsp)。
.Lspec_mag:
    movaps  %xmm8, %xmm0
    mulss   %xmm8, %xmm0               // q1²
    movaps  %xmm9, %xmm1
    mulss   %xmm9, %xmm1               // q2²
    addss   %xmm1, %xmm0
    movaps  %xmm8, %xmm1
    mulss   %xmm9, %xmm1
    mulss   %xmm10, %xmm1              // c*q1*q2
    subss   %xmm1, %xmm0               // m²
    xorps   %xmm1, %xmm1
    maxss   %xmm1, %xmm0               // 防浮点误差为负
    sqrtss  %xmm0, %xmm0               // m
    mulss   56(%rsp), %xmm0            // m / samples
    mulss   FIVE_FIFTY(%rip), %xmm0
    movss   ONE(%rip), %xmm1
    addss   %xmm1, %xmm0               // 1 + m*550
    subq    $8, %rsp
    call    log10f@PLT
    addq    $8, %rsp
    mulss   FOUR(%rip), %xmm0          // ×4
    xorps   %xmm1, %xmm1
    maxss   %xmm1, %xmm0
    movss   SEVEN(%rip), %xmm1
    minss   %xmm1, %xmm0               // 钳制 0..7
    movss   HALF(%rip), %xmm1
    addss   %xmm1, %xmm0               // +0.5 四舍五入
    cvtss2sil %xmm0, %eax
    ret
    .size   dsp_spectrum_s16, .-dsp_spectrum_s16

// ---------------------------------------------------------------------------
// 只读常量
// ---------------------------------------------------------------------------
    .section .rodata
    .balign 16
ABS_MASK:
    .long   0x7fffffff, 0x7fffffff, 0x7fffffff, 0x7fffffff
INV_32768:
    .float  3.0517578125e-05, 3.0517578125e-05, 3.0517578125e-05, 3.0517578125e-05
VOL_RAMP:
    .float  0.0, 1.0, 2.0, 3.0
ONE:
    .float  1.0
TWO:
    .float  2.0
FOUR:
    .float  4.0
HALF:
    .float  0.5
SEVEN:
    .float  7.0
FIVE_FIFTY:
    .float  550.0
TWO_PI:
    .float  6.283185307179586
BAND_FREQ:
    .float  20.000000, 30.266439, 45.802867, 69.314484
    .float  104.895131, 158.740105, 240.224887, 363.537596
    .float  550.149427, 832.553207, 1259.921050, 1906.666190
    .float  2885.399812, 4366.538893, 6607.979186, 10000.000000

    .section .note.GNU-stack,"",@progbits
