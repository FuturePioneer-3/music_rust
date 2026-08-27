// music_rust —— x86-64 SSE2 汇编音频 DSP 内核
// Copyright (C) 2026 FuturePioneer-3
// SPDX-License-Identifier: GPL-3.0-or-later
//
// 本文件是播放器音频热路径的汇编实现，负责三个高频操作：
//   dsp_vol_s16       —— int16 音量渐变 + 饱和钳制（逐样本线性渐变，杜绝爆音）
//   dsp_limiter_f32   —— f32 峰值限制器（块峰值分析 + attack/release 平滑增益 + 硬钳制）
//   dsp_spectrum_s16  —— 16 段对数频带 Goertzel 频谱（4 频带 SSE 并行，比标量快 ~4 倍）
//
// 为什么用汇编：
//   这些函数每播放一个音频块就会被调用一次，内部是逐样本的乘加循环。
//   手写 SSE2 一次可以处理 4 个 float（或 8 个 int16），比依赖编译器自动
//   向量化更可控，也能明确绕开对齐和分支预测上的坑。
//
// 指令集与平台约束：
//   - 只用 SSE2。这是 x86-64 的基线指令集，任何 x86-64 CPU 都支持，
//     不依赖 AVX/AVX2，也不用任何非标准扩展。
//   - 全部内存访问用 RIP-relative（PC 相对）或寄存器寻址，不出现绝对
//     地址，因此编译成 PIE 可执行文件也能正常链接运行。
//   - 语法是 AT&T（GNU as），由 build.rs 里的 cc crate 编译进静态库。
//     AT&T 里方向与直觉相反：movq %src, %dst；立即数加 $；内存用
//     disp(base,index,scale) 表示。
//
// System V AMD64 调用约定（本文件依赖的规则）：
//   - 整数/指针参数依次放 rdi、rsi、rdx、rcx、r8、r9，第 7 个起压栈。
//   - 浮点参数依次放 xmm0..xmm7，与整数参数按位置一一对应。
//   - 返回整数放 eax/rax，返回浮点放 xmm0。
//   - rax、rcx、rdx、rsi、rdi、r8-r11 与全部 xmm 是调用者保存的
//     （被调函数可以随便改）；rbx、rbp、r12-r15 是被调用者保存的
//     （要用必须先 push、用后 pop 恢复）。
//   - 栈按 16 字节对齐：执行 call 前 rsp%16==0；call 压入 8 字节返回
//     地址后，被调函数入口处 rsp%16==8。要调用外部库函数（比如 cosf）
//     前必须把栈重新对齐到 rsp%16==0，否则可能崩溃。
//
// 每个函数下面都会单独说明算法、寄存器分工和边界情况，方便对照阅读。

    .text

// ---------------------------------------------------------------------------
// void dsp_vol_s16(const int16_t *in /*rdi*/, int16_t *out /*rsi*/,
//                  uint32_t samples /*edx*/, float *gain /*rcx*/,
//                  float target /*xmm0*/)
//
// 功能：把 in 的 samples 个 int16 样本乘以一个“线性渐变”的增益写入 out，
//       增益从 *gain 开始、逐样本逼近 target，结束时把终点增益写回 *gain。
//
// 为什么用渐变而不是直接乘 target：
//   音量突变（比如按一下音量键）如果直接乘上新系数，波形会瞬间跳变，
//   听感上就是“咔哒”爆音。逐样本线性渐变把这次跳变摊到整个块里，
//   音量变化就平滑了。*gain 是跨块的“记忆”：下一块从上一块结束时的
//   增益继续渐变，因此大块小块交替时不会出现折线或台阶。
//
// 参数说明：
//   in/out 可以是同一个缓冲区（原地处理），也可以是两个不同的缓冲区。
//   samples 是 int16 样本总数（帧数 × 声道数），不是字节数。
//   target 是本次渐变的目标增益；samples == 0 时不处理数据，只把 *gain
//   同步成 target（空块语义，供上层停止输出时使用）。
//
// 算法（每块）：
//   step = (target - g0) / samples         每个样本的增益增量
//   out[i] = sat16(round(in[i] * (g0 + i*step)))
//   *gain = g0 + samples * step
//   数学上终点正好等于 target；浮点除法可能有极小的舍入差，但下一块
//   会继续以这个值为起点，误差不会累积放大。
//
// 寄存器分工：
//   rdi  = in 指针        rsi = out 指针
//   edx  = 样本总数       rcx = *gain 指针（读旧值、写回新值）
//   xmm0 = target        xmm1 = 当前增益 g（每处理 4 个样本推进一次）
//   xmm2 = step（标量，也可广播成向量）   xmm5 = 数据暂存
//   eax  = 已处理的样本数，兼作 in/out 的字节偏移（样本 × 2 字节）
// ---------------------------------------------------------------------------
    .globl  dsp_vol_s16
    .type   dsp_vol_s16,@function
dsp_vol_s16:
    // 空块特例：不读写数据，只把 *gain 写成 target。
    // 这样上层“停止输出”后，下次恢复播放时增益从新目标开始，不会
    // 从旧的中间值重新渐变。
    testl   %edx, %edx
    jz      .Lvol_set_target           // samples == 0 → *gain = target

    // 准备渐变参数。g 先做一次 max(g, 0)，防止上次调用留下的负增益
    // （正常不会出现，纯防御）。
    movss   (%rcx), %xmm1              // g = *gain
    xorps   %xmm4, %xmm4
    maxss   %xmm4, %xmm1               // g = max(g, 0)

    // step = (target - g) / samples。samples 不为 0（上面已排除），
    // 所以除法安全。
    movss   %xmm0, %xmm2
    subss   %xmm1, %xmm2               // target - g
    cvtsi2ssl %edx, %xmm3
    divss   %xmm3, %xmm2               // step = (target - g) / samples

    // 把 step 广播到 4 个通道：pshufd $0 表示每个目标通道都取
    // 源操作数第 0 个通道，即 [s,s,s,s]。
    pshufd  $0x00, %xmm2, %xmm2        // [s,s,s,s]

    // xmm7 = [4s,4s,4s,4s]：主循环每轮处理 4 个样本，增益 g 也要一次
    // 前进 4 个 step，所以预先把“每轮推进量”算好，循环里只需一条 addss。
    movaps  %xmm2, %xmm7
    addps   %xmm2, %xmm7               // [2s,...]
    addps   %xmm2, %xmm7               // [3s,...]
    addps   %xmm2, %xmm7               // xmm7 = [4s,4s,4s,4s]（每轮推进量）

    xorl    %eax, %eax                 // 样本下标

.Lvol_loop:
    // 主循环条件：还有样本且至少剩 4 个。eax+3 是这轮最后一个样本的
    // 下标，若 >= 总数说明不足 4 个，交给标量尾部逐个处理。
    cmpl    %edx, %eax
    jae     .Lvol_done
    leal    3(%rax), %r8d
    cmpl    %edx, %r8d
    jae     .Lvol_tail                 // 不足 4 个 → 标量尾部

    // ---- 重建本轮 4 路增益向量 vg = [g, g+s, g+2s, g+3s] ----
    // VOL_RAMP 是只读常量 [0,1,2,3]；先广播当前增益 g，再把常量乘上
    // step 得到每个样本的相对增量，两者相加就是本轮 4 个样本各自的增益。
    // 每轮都要重建，因为 g 每轮都在推进。
    pshufd  $0x00, %xmm1, %xmm0        // [g,g,g,g]
    movaps  VOL_RAMP(%rip), %xmm5      // [0,1,2,3]
    mulps   %xmm2, %xmm5               // [0,s,2s,3s]
    addps   %xmm5, %xmm0               // vg

    // ---- 4 样本 SSE 处理 ----
    // 从 in 读取 8 字节（4 个 int16）。int16 没法直接做浮点运算，需要
    // 先符号扩展成 int32、再转 float：
    //   movq 取来 [a0 a1 a2 a3 a4 a5 a6 a7]（每个元素 16 位）
    //   punpcklwd 复制自己后把低 64 位交织成 [a0 a0 a1 a1 a2 a2 a3 a3]
    //   psrad $16 对每个 32 位字算术右移，得到符号扩展的 [a0 a1 a2 a3]
    movq    (%rdi,%rax,2), %xmm5       // 8 字节 = 4×int16
    punpcklwd %xmm5, %xmm5             // 复制为 4×dword
    psrad   $16, %xmm5                 // 算术右移 → 符号扩展的 int32
    cvtdq2ps %xmm5, %xmm5              // → float
    mulps   %xmm0, %xmm5               // × 渐变增益
    cvtps2dq %xmm5, %xmm5              // 就近舍入回整数
    packssdw %xmm5, %xmm5              // 饱和打包 → 4×int16（钳到 ±32767）
    movq    %xmm5, (%rsi,%rax,2)       // 存 8 字节 = 4 个样本
    // 本轮 4 个样本的增益各自是 g, g+s, g+2s, g+3s，下一轮起点是 g+4s，
    // 直接用预计算的 xmm7（低 32 位 = 4s）推进。
    addss   %xmm7, %xmm1               // g += 4s
    addl    $4, %eax
    jmp     .Lvol_loop

.Lvol_tail:
    // 标量尾部：逐个处理剩余 1..3 个样本。这里乘完当前 g 后手写饱和
    // 钳制（movl $32767 / movl $-32768），与主循环 packssdw 的效果一致。
    cmpl    %edx, %eax
    jae     .Lvol_done
    movswl  (%rdi,%rax,2), %r8d
    cvtsi2ssl %r8d, %xmm5
    mulss   %xmm1, %xmm5
    cvtss2sil %xmm5, %r8d              // 截断取整（与主循环就近舍入略异，
                                       // 尾部省指令，误差至多 1 LSB）
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
    // 尾部每处理 1 个样本，g 也前进一个 step（标量版）。
    movss   %xmm1, %xmm5
    addss   %xmm2, %xmm5
    movss   %xmm5, %xmm1
    incl    %eax
    jmp     .Lvol_tail

.Lvol_done:
    // 把终点增益写回 *gain。即使有 1..3 个样本走了标量尾部，循环结束时
    // g 都恰好推进到 g0 + samples*step ≈ target，下一块从这里继续。
    movss   %xmm1, (%rcx)              // 回写当前增益（供下一块继续渐变）
    ret

.Lvol_set_target:
    movss   %xmm0, (%rcx)
    ret
    .size   dsp_vol_s16, .-dsp_vol_s16

// ---------------------------------------------------------------------------
// void dsp_limiter_f32(float *buf /*rdi*/, uint32_t len /*esi*/,
//                      dsp_limiter *st /*rdx*/)
//
// 功能：对 len 个 f32 样本做块级峰值限制，防止削波：
//   1) 第一遍扫描缓冲区，求 |x| 的最大值 peak（SSE2 每轮 4 样本 + 标量尾部）；
//   2) 需要的目标增益 = min(target / peak, 1.0)（只压缩、不提升音量）；
//   3) diff = 目标增益 - current_gain，按 diff 符号选择系数做一阶平滑，
//      更新 current_gain（压降快、恢复慢，避免“泵浦感”）；
//   4) 第二遍把每个样本乘上平滑后的增益，再硬钳制到 ±target。
//
// 整块静音（peak <= 0）时直接返回，不改变 current_gain：否则静音期间
// 增益包络会被错误拉起，等声音再次出现时会突然变大。
//
// dsp_limiter 结构体布局（与 Rust 侧 LimiterState 一致，Rust 已标 repr(C)）：
//   offset  0  target         目标峰值电平（线性满刻度，1.0 = 0dBFS）
//   offset  4  current_gain   当前增益包络（输入输出，调用之间保留）
//   offset  8  attack_coef    压降系数（0..1，越大压得越快）
//   offset 12  release_coef   恢复系数（0..1，越大恢复越快）
//
// 寄存器分工：
//   rdi = buf（第一遍扫完后保存到 r8，第二遍恢复）
//   esi = len（第一遍与第二遍各自重新折算 4 样本组数）
//   rdx = st 结构体指针
//   xmm0 = 峰值累加器（第一遍）/ 第二遍的样本向量
//   xmm5 = 绝对值掩码 0x7fffffff（第一遍）
//   eax = 剩余 4 样本组数
// ---------------------------------------------------------------------------
    .globl  dsp_limiter_f32
    .type   dsp_limiter_f32,@function
dsp_limiter_f32:
    // len == 0 的空块直接返回，避免对空缓冲区求峰值。
    testl   %esi, %esi
    jz      .Llim_ret
    // 第一遍扫峰值时会把 rdi 一路加过去，先把起点存到 r8，第二遍要用。
    movq    %rdi, %r8                  // 保存缓冲区起点（第二遍要用）
    // 峰值累加器清零；xmm5 载入绝对值掩码 0x7fffffff（对 IEEE 754 float
    // 清零符号位就是取绝对值）。eax = len/4 是 4 样本组数。
    xorps   %xmm0, %xmm0               // peak = 0
    movaps  ABS_MASK(%rip), %xmm5
    movl    %esi, %eax
    shrl    $2, %eax
.Llim_peak_loop:
    // 每轮读 16 字节（4 个 float），与掩码按位与去掉符号位，maxps 归并
    // 峰值，然后指针前进 16 字节、组数减一。
    testl   %eax, %eax
    jz      .Llim_peak_tail
    movups  (%rdi), %xmm1
    andps   %xmm5, %xmm1               // |x|
    maxps   %xmm1, %xmm0
    addq    $16, %rdi
    decl    %eax
    jmp     .Llim_peak_loop

.Llim_peak_tail:
    // 剩余 0..3 个样本用标量逐个归并。eax = len % 4。
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
    // 峰值现在分散在 xmm0 的 4 个通道里，做两次 shuffle+max 归并成
    // 标量（第 0 通道）：
    //   shufps $0x4e 把通道顺序反转 [x3,x2,x1,x0]，
    //             max 后 lane0=max(x0,x3)、lane1=max(x1,x2)
    //   shufps $0xb1 交换相邻通道对 [x1,x0,x3,x2]，
    //             max 后 lane0 就是全局峰值
    movaps  %xmm0, %xmm1
    shufps  $0x4e, %xmm1, %xmm1
    maxps   %xmm1, %xmm0
    movaps  %xmm0, %xmm1
    shufps  $0xb1, %xmm1, %xmm1
    maxss   %xmm1, %xmm0               // peak（lane 0）

    // 静音块（peak == 0）直接返回：保持 current_gain 不变。
    // 用 ucomiss 比较 peak 与 0，相等则 ZF=1，je 命中。
    xorps   %xmm1, %xmm1
    ucomiss %xmm1, %xmm0
    je      .Llim_ret                  // 静音块

    // target_gain = min(target / peak, 1.0)。
    // peak 非 0（上面已排除静音），除法安全。
    movss   0(%rdx), %xmm1
    divss   %xmm0, %xmm1
    movss   ONE(%rip), %xmm2
    minss   %xmm2, %xmm1

    // diff = target_gain - current_gain；用 ucomiss 与 0 比较：
    // diff < 0 时 CF=1，跳 attack；diff >= 0 时走 release。
    // 一阶平滑：current_gain += diff * coef。
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

    // 把平滑后的增益 g 广播到 4 个通道，同时把 target 也广播，
    // 并构造 -target 向量，供第二遍向量钳制使用。
    pshufd  $0x00, %xmm2, %xmm2        // [g,g,g,g]
    movss   0(%rdx), %xmm3             // target
    pshufd  $0x00, %xmm3, %xmm3        // [+lim,...]
    xorps   %xmm4, %xmm4
    subps   %xmm3, %xmm4               // 0 - lim（逐 lane 取负）
    // xmm4 = [-lim,...]

    // 恢复缓冲区起点，重新折算 4 样本组数，开始第二遍应用。
    movq    %r8, %rdi                  // 回到缓冲区起点
    movl    %esi, %eax
    shrl    $2, %eax
.Llim_apply_loop:
    // 每轮 4 个样本：乘增益，maxps 钳下界 -target，minps 钳上界 +target，
    // 写回原处。顺序必须是“先乘后钳”，保证任何时刻不超 target。
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
    // 剩余 0..3 个样本用标量做同样的事。
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
// 功能：从交错 PCM 的第一声道取 samples 帧（每帧间隔 channels 个样本），
//       计算 16 个对数频带（20Hz..10kHz）的能量，写成 16 个 0..7 的电平。
//       结果供 TUI 的动态 EQ 柱状图使用。
//
// Goertzel 算法（单频带 DFT 的递推形式）：
//   对频率 f，系数 c = 2*cos(2πf/sr)。逐样本递推：
//     q0 = c*q1 - q2 + s     （s 是当前样本，归一化到 [-1,1]）
//     q2 = q1;  q1 = q0
//   扫描完整块后，该频率的幅值：
//     m = sqrt(q1² + q2² - c*q1*q2) / samples
//   再映射成人耳更接近的对数电平：
//     level = log10(1 + m*550) * 4，钳制到 [0,7]，加 0.5 后取整。
//   550 和 4 是经验系数：让常见响度的音频落在 0..7 的柱子范围内，
//   太弱则贴底、太强则封顶。
//
// 并行方式：
//   16 个频带分成 4 组（每组 4 个频带），一组一组处理。每组把 4 个
//   频带的系数、q1、q2 各放一个 SSE 通道，4 路同时递推——每个样本
//   只需 4 条 SSE 乘加，比标量快约 4 倍。
//
// 栈与寄存器布局（本函数全程不用外部调用者保存的寄存器，全部自管）：
//   入口处 rsp%16==8（call 压了返回地址）；6 个 push（48 字节）+ subq $72
//   后 rsp%16==0，满足后面 call cosf 的栈对齐要求。
//   栈上：0(%rsp)=当前组的 4 个系数，16(%rsp)=q1 向量，32(%rsp)=q2 向量，
//         48(%rsp)=1/samples（浮点），52(%rsp)=channels（整数，防 cosf 践踏）
//   寄存器：rbp=pcm 指针，r13d=samples，r12d=sample_rate，r14=levels，
//           r15d=组号 0..3，ebx=组内首频带号（group*4），
//           r10d=样本字节偏移（每次加 channels*2），ecx=已处理帧数
// ---------------------------------------------------------------------------
    .globl  dsp_spectrum_s16
    .type   dsp_spectrum_s16,@function
dsp_spectrum_s16:
    // 6 个 push 保存被调用者保存寄存器（rbx/rbp/r12-r15），函数末尾
    // 按相反顺序 pop 恢复，保证返回后调用方的这些寄存器原封不动。
    pushq   %rbx
    pushq   %rbp
    pushq   %r12
    pushq   %r13
    pushq   %r14
    pushq   %r15
    // 预留 72 字节栈空间：64 字节放系数/q1/q2（各 16 字节），
    // 另 8 字节放 inv_samples（4 字节）和 channels（4 字节）。
    subq    $72, %rsp
    // 栈布局：0=coeff 16= q1 32= q2 48= inv_samples 52= channels
    movq    %rdi, %rbp                 // pcm
    movl    %esi, %r13d                // samples
    movl    %ecx, %r12d                // sample_rate
    movq    %r8, %r14                  // levels
    // channels 放进栈而不是普通寄存器：稍后要 call cosf，cosf 会随意
    // 改写调用者保存的寄存器，唯独栈上这块不会被碰。
    movl    %edx, 52(%rsp)             // channels（cosf 会践踏 edx，先入栈保存）

    // inv_samples = 1.0 / samples。调用方（C 侧）保证 samples >= 32，
    // 不会除零。
    xorps   %xmm0, %xmm0
    cvtsi2ssl %r13d, %xmm0
    movss   ONE(%rip), %xmm1
    divss   %xmm0, %xmm1
    movss   %xmm1, 48(%rsp)

    // 组号从 0 开始，一组处理完再处理下一组，直到 4 组全部完成。
    xorl    %r15d, %r15d               // group = 0..3
.Lgroup_loop:
    // r15d >= 4 说明 16 个频带都算完了，收尾退出。
    cmpl    $4, %r15d
    jae     .Lspectrum_done
    // 本组第一个频带的编号 = group*4，后面 4 个频带连续编号。
    movl    %r15d, %ebx
    shll    $2, %ebx                   // band 基准 = group*4

    // ---- 计算本组 4 个频带的 Goertzel 系数 c = 2*cos(2πf/sr) ----
    // 4 个 lane 各自取 BAND_FREQ 表里的频率 → 乘 2π → 除以采样率 →
    // call cosf → 乘 2，结果依次存入栈上 0/4/8/12(%rsp)。
    // lane 0：BAND_FREQ[group*4]
    lea     BAND_FREQ(%rip), %r11
    movss   (%r11,%rbx,4), %xmm0
    mulss   TWO_PI(%rip), %xmm0
    cvtsi2ssl %r12d, %xmm1
    divss   %xmm1, %xmm0
    // 注意：此刻 rsp%16==0，call 压入返回地址后 cosf 入口 rsp%16==8，
    // 正好满足 ABI。
    call    cosf@PLT
    mulss   TWO(%rip), %xmm0
    movss   %xmm0, 0(%rsp)
    // lane 1：BAND_FREQ[group*4+1]
    leal    1(%rbx), %r10d
    lea     BAND_FREQ(%rip), %r11
    movss   (%r11,%r10,4), %xmm0
    mulss   TWO_PI(%rip), %xmm0
    cvtsi2ssl %r12d, %xmm1
    divss   %xmm1, %xmm0
    call    cosf@PLT
    mulss   TWO(%rip), %xmm0
    movss   %xmm0, 4(%rsp)
    // lane 2：BAND_FREQ[group*4+2]
    leal    2(%rbx), %r10d
    lea     BAND_FREQ(%rip), %r11
    movss   (%r11,%r10,4), %xmm0
    mulss   TWO_PI(%rip), %xmm0
    cvtsi2ssl %r12d, %xmm1
    divss   %xmm1, %xmm0
    call    cosf@PLT
    mulss   TWO(%rip), %xmm0
    movss   %xmm0, 8(%rsp)
    // lane 3：BAND_FREQ[group*4+3]
    leal    3(%rbx), %r10d
    lea     BAND_FREQ(%rip), %r11
    movss   (%r11,%r10,4), %xmm0
    mulss   TWO_PI(%rip), %xmm0
    cvtsi2ssl %r12d, %xmm1
    divss   %xmm1, %xmm0
    call    cosf@PLT
    mulss   TWO(%rip), %xmm0
    movss   %xmm0, 12(%rsp)

    // 系数向量就绪。q1、q2 从 0 开始递推。
    movaps  0(%rsp), %xmm3             // coeff 向量
    xorps   %xmm1, %xmm1               // q1 = 0
    movaps  %xmm1, %xmm2               // q2 = 0
    // channels 取回 r9d：内层循环没有函数调用，r9 不会被践踏。
    movl    52(%rsp), %r9d             // channels（仅内层循环使用，无调用）
    // r10d 是样本字节偏移（每帧取第一声道，帧距 channels 个样本），
    // ecx 是已处理帧数，与 samples（r13d）比较作循环条件。
    xorl    %r10d, %r10d               // 样本下标
    xorl    %ecx, %ecx                 // 循环计数
.Linner_loop:
    // 读完 samples 帧就结束本组递推。
    cmpl    %r13d, %ecx
    jae     .Linner_done
    // 取一个 int16 样本，符号扩展转 float，归一化到 [-1,1]
    // （乘 INV_32768 = 1/32768），广播到 4 个通道。
    movswl  (%rbp,%r10,2), %eax
    cvtsi2ssl %eax, %xmm4
    pshufd  $0x00, %xmm4, %xmm4
    mulps   INV_32768(%rip), %xmm4     // s = sample / 32768
    // 4 路同时做 Goertzel 递推：
    //   q0 = c*q1 - q2 + s；q2 = q1；q1 = q0
    // xmm1/xmm2 分别是当前 q1/q2，xmm3 是系数。
    movaps  %xmm3, %xmm5
    mulps   %xmm1, %xmm5               // c*q1
    subps   %xmm2, %xmm5               // c*q1 - q2
    addps   %xmm4, %xmm5               // + s
    movaps  %xmm1, %xmm2               // q2 = q1
    movaps  %xmm5, %xmm1               // q1 = q0
    // 只取第一声道：样本偏移每次前进 channels 个 int16（×2 字节）。
    addl    %r9d, %r10d
    incl    %ecx
    jmp     .Linner_loop

.Linner_done:
    // 递推结束，把本组 4 个频带的 q1/q2 和系数存到栈上，供后面逐个
    // 收尾（.Lspec_mag 会读 16/32(%rsp) 的 q 和 0/4/8/12(%rsp) 的系数）。
    movaps  %xmm1, 16(%rsp)            // 保存 q1
    movaps  %xmm2, 32(%rsp)            // 保存 q2
    // ---- 逐频带收尾（4 路展开）----
    // .Lspec_mag 的输入约定：xmm8=q1、xmm9=q2、xmm10=系数 c，输出 al=
    // 0..7 电平。4 个频带的输入分别从栈上取出，结果写回 levels[组内位置]。
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
    // 按 push 的相反顺序恢复寄存器，回收栈空间后返回。
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
//
// 计算流程：
//   m² = q1² + q2² - c*q1*q2
//   先 max(0) 防浮点误差把 m² 算成微小的负数
//   m = sqrt(m²) / samples（用调用方栈里的 inv_samples 一次乘掉除法）
//   level = log10(1 + m*550) * 4，钳到 [0,7]，+0.5 后 cvtss2si 取整
//
// 栈对齐细节：
//   主函数调用本子过程时 rsp%16==0，call 压入返回地址后这里 rsp%16==8；
//   调 log10f 前必须把栈补回 16 对齐，所以 subq $8 / addq $8。
//   inv_samples 在调用方栈 48(%rsp)，加上本子过程 8 字节返回地址，
//   在这里就是 56(%rsp)。
.Lspec_mag:
    // q1²
    movaps  %xmm8, %xmm0
    mulss   %xmm8, %xmm0               // q1²
    // q2²
    movaps  %xmm9, %xmm1
    mulss   %xmm9, %xmm1               // q2²
    addss   %xmm1, %xmm0
    // c*q1*q2
    movaps  %xmm8, %xmm1
    mulss   %xmm9, %xmm1
    mulss   %xmm10, %xmm1              // c*q1*q2
    subss   %xmm1, %xmm0               // m²
    // 防止浮点舍入把 m² 弄成 -1e-30 之类的负数，sqrt 前先钳到 0。
    xorps   %xmm1, %xmm1
    maxss   %xmm1, %xmm0               // 防浮点误差为负
    sqrtss  %xmm0, %xmm0               // m
    // m / samples = m * inv_samples（一次乘法替代除法）。
    mulss   56(%rsp), %xmm0            // m / samples
    // 1 + m*550
    mulss   FIVE_FIFTY(%rip), %xmm0
    movss   ONE(%rip), %xmm1
    addss   %xmm1, %xmm0               // 1 + m*550
    // 调 log10f 前先把栈从 rsp%16==8 补到 rsp%16==0。
    subq    $8, %rsp
    call    log10f@PLT
    addq    $8, %rsp
    // level = log10(...) * 4，钳到 [0,7]，+0.5 四舍五入后转整数。
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
// 这些常量全部放在 .rodata 段，程序加载后只读、可共享，不会被改写。
// 访问一律用 RIP-relative 寻址（如 ONE(%rip)），不产生绝对地址，
// 因此 PIE 下也无需重定位。
    .section .rodata
    .balign 16
// 取 float 绝对值用的掩码：IEEE 754 单精度的符号位是最高位（bit 31），
// 与 0x7fffffff 按位与后符号位被清掉，其余位不变。4 个通道各一份。
ABS_MASK:
    .long   0x7fffffff, 0x7fffffff, 0x7fffffff, 0x7fffffff
// 1/32768：把 int16 样本换算到 [-1,1] 的浮点范围（32768 而非 32767，
// 保证 0x8000 = -32768 也落在 -1.0 内）。
INV_32768:
    .float  3.0517578125e-05, 3.0517578125e-05, 3.0517578125e-05, 3.0517578125e-05
// 每轮 4 个样本的序号 [0,1,2,3]，乘上 step 后得到各样本相对当前增益的增量。
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
// 频谱电平的经验缩放：幅值乘 550 再取 log10，让常见响度落入 0..7 柱高。
FIVE_FIFTY:
    .float  550.0
// 2π：频率（Hz）→ 角频率（rad/sample 换算用）。
TWO_PI:
    .float  6.283185307179586
// 16 个对数频带的中心频率（Hz），从 20Hz 到 10kHz，相邻比值约 1.513
// （500 开 15 次方），即每个频带大约覆盖 2.17 个半音，符合人耳感知。
BAND_FREQ:
    .float  20.000000, 30.266439, 45.802867, 69.314484
    .float  104.895131, 158.740105, 240.224887, 363.537596
    .float  550.149427, 832.553207, 1259.921050, 1906.666190
    .float  2885.399812, 4366.538893, 6607.979186, 10000.000000

    .section .note.GNU-stack,"",@progbits
