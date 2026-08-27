# audio_wav.S —— music_rust 的 WAV 快路径
# Copyright (C) 2026 FuturePioneer-3
# SPDX-License-Identifier: GPL-3.0-or-later
#
# 本文件处理“纯 WAV”这种不需要动用 FFmpeg 的情况。文件 I/O 和播放线程
# 的存活期归 C 侧（src/audio_file.c）管理，这里只负责两个与格式强相关的
# 热操作：
#
#   1. 走一遍 RIFF/WAVE 的 chunk 表，定位 "fmt " 和 "data" 两个 chunk；
#   2. 把 WAV 的样本载荷（多种编码）转换成播放引擎原生使用的 int16 PCM。
#
# 为什么单独写汇编：WAV 解析逻辑简单，但样本转换是逐样本循环，对几百兆
# 的 WAV 来说就是几亿次迭代；手写循环可以把分支减到最少，转换表完全
# 摊平在寄存器里，性能比通用解码器稳定。
#
# 注释风格：这里故意写得比较“叙事”。WAV 格式本身不复杂，难的是让读者
# 在读汇编时脑子里始终有文件布局和寄存器分工两张图，所以每个阶段都会
# 先交代“现在我在读哪块、各寄存器里是什么”。
#
# SysV AMD64 调用约定速查（本文件只用了整数部分）：
#   整数/指针参数：rdi、rsi、rdx、rcx、r8、r9，第 7 个起压栈
#   返回值：eax/rax
#   调用者保存：rax、rcx、rdx、rsi、rdi、r8-r11、xmm0-xmm15
#   被调用者保存：rbx、rbp、r12-r15（本文件两个函数都不动它们）
#   栈 16 字节对齐规则同 audio_dsp.S；本文件不调用任何外部函数。

    .text

# ---------------------------------------------------------------------------
# int audio_wav_parse(const uint8_t *file, size_t len, music_wav_meta *out)
#
# 功能：解析 RIFF/WAVE 容器，找到 "fmt " 和 "data" 两个 chunk，把关键参数
#       和样本数据的偏移/大小写进 out。成功返回 1，失败返回 0。
#       失败时调用方（C 的 load_wav_direct）会放弃快路径，回退到 FFmpeg，
#       所以这里可以放心地“识别不了就拒绝”，不必强求兼容所有怪文件。
#
# WAV 的文件布局（RIFF 容器，全部小端）：
#   0..3   "RIFF"
#   4..7   文件总长减 8（小端，实际解析时不依赖这个值，只按 chunk 走）
#   8..11  "WAVE"
#   之后是若干 chunk，每个 chunk：
#     4 字节 FourCC（如 "fmt "、"data"）
#     4 字节小端载荷长度 N
#     N 字节载荷
#     若 N 为奇数，后面补 1 个填充字节（保证下一个 chunk 头从偶数偏移开始）
#
# 支持两种编码（见 audio_wav.h / C 侧约定）：
#   format=1：PCM 整数，8/16/24/32 位
#   format=3：IEEE float，目前只支持 32 位
#
# chunk 循环内的寄存器分工：
#   rdi = 文件基址        rsi = 文件长度
#   rdx = 输出结构体指针   r8  = 当前 chunk 头指针
#   r9  = 文件末尾（one-past-end）指针
#   r10 = 当前 chunk 载荷长度（小端读入）
#   r11 = 当前 chunk 载荷起始指针
#   ecx = 已见标志位：bit0 见过 "fmt "，bit1 见过 "data"（=3 才算齐）
# ---------------------------------------------------------------------------
    .globl  audio_wav_parse
    .type   audio_wav_parse,@function
audio_wav_parse:
    # 参数空指针、文件太短都直接失败。
    # 最短合法文件：12 字节 RIFF 头 + 至少 8 字节一个 chunk 头 = 20 字节。
    testq   %rdi, %rdi
    jz      .Lparse_fail
    testq   %rdx, %rdx
    jz      .Lparse_fail
    cmpq    $20, %rsi                  # 12-byte header + at least one chunk header
    jb      .Lparse_fail

    # 校验 RIFF 魔数。cmp 按 4 字节小端读内存，和立即数比较；因此
    # "RIFF" 写成 0x46464952（内存字节序反转后的数值）。
    cmpl    $0x46464952, 0(%rdi)       # "RIFF" in little-endian memory
    jne     .Lparse_fail
    cmpl    $0x45564157, 8(%rdi)       # "WAVE"
    jne     .Lparse_fail

    # 清空输出结构体（16 字节，两段 8 字节清零），避免残留旧值。
    xorl    %eax, %eax
    movq    $0, 0(%rdx)                # clear first 8 bytes of music_wav_meta
    movq    $0, 8(%rdx)                # clear remaining fields

    # r8 指向第一个 chunk 头（文件偏移 12）；r9 是文件末尾 one-past-end，
    # 所有越界判断都用它做上界。
    leaq    12(%rdi), %r8              # first chunk header
    leaq    (%rdi,%rsi), %r9           # one-past-end pointer
    # 已见标志清零。
    xorl    %ecx, %ecx                 # seen flags

.Lchunk_loop:
    # 先确认“当前指针 + 8 字节 chunk 头”没有越过文件末尾；越过说明
    # 文件已读完或残余不足一个 chunk 头，跳到收尾检查。
    leaq    8(%r8), %rax               # need a full chunk header
    cmpq    %r9, %rax
    ja      .Lparse_done_check

    # 读载荷长度（小端，movl 直接读 4 字节即可），算出载荷结束地址
    # r11 + N；加法进位或超出文件末尾都算坏文件。
    movl    4(%r8), %r10d              # payload size (WAV uses little-endian)
    leaq    8(%r8), %r11               # payload starts right after the chunk header
    movq    %r10, %rax
    addq    %r11, %rax                 # payload end
    jc      .Lparse_fail
    cmpq    %r9, %rax
    ja      .Lparse_fail

    # 按 FourCC 分派。同样是小端比较："fmt " = 0x20746d66、"data" =
    # 0x61746164。都不认识就直接跳到下一个 chunk（本快路径不关心
    # LIST、JUNK 等额外块）。
    cmpl    $0x20746d66, 0(%r8)        # "fmt "
    je      .Lchunk_fmt
    cmpl    $0x61746164, 0(%r8)        # "data"
    je      .Lchunk_data
    jmp     .Lchunk_next

.Lchunk_fmt:
    # 标准 PCM 的 fmt chunk 是 16 字节；扩展 WAV（WAVEFORMATEXTENSIBLE）
    # 可能更长，但前 16 字节字段布局不变，快路径只需要这些。
    cmpl    $16, %r10d
    jb      .Lparse_fail
    # 把关键字段抄进输出结构体（偏移与 Rust 侧 WavMeta / C 侧
    # music_wav_meta 一一对应）：
    #   0(%r11) 音频格式（1=PCM，3=float）→ out+0
    #   2(%r11) 声道数                          → out+2
    #   4(%r11) 采样率                          → out+4
    #  12(%r11) block align（每帧字节数）       → out+8
    #  14(%r11) 位深                            → out+10
    movw    0(%r11), %ax               # audio format: 1 PCM, 3 float
    movw    %ax, 0(%rdx)
    movw    2(%r11), %ax               # channel count
    movw    %ax, 2(%rdx)
    movl    4(%r11), %eax              # sample rate
    movl    %eax, 4(%rdx)
    movw    12(%r11), %ax              # bytes per frame
    movw    %ax, 8(%rdx)
    movw    14(%r11), %ax              # bits per sample
    movw    %ax, 10(%rdx)
    # 记下“见过 fmt”，然后跳到下一个 chunk。
    orl     $1, %ecx
    jmp     .Lchunk_next

.Lchunk_data:
    # data chunk：把“载荷相对文件基址的偏移”和“载荷字节数”写进结构体。
    # C 侧拿到后会据此在已分配好的 PCM 缓冲区上定位源数据。
    movq    %r11, %rax
    subq    %rdi, %rax
    movl    %eax, 12(%rdx)
    movl    %r10d, 16(%rdx)
    # 记下“见过 data”。两个标志都齐了（ecx==3）就提前结束遍历。
    orl     $2, %ecx
    jmp     .Lchunk_next

.Lchunk_next:
    # 前进到下一个 chunk 头：8 字节头 + 载荷长度 + （奇数长度时）1 个
    # 填充字节。加 1 再对齐到偶数（andq $-2）就是“长度向偶数取整”。
    movq    %r10, %rax
    addq    $1, %rax                   # chunks with odd length have one pad byte
    andq    $-2, %rax
    leaq    8(%r8,%rax), %r8           # next chunk header
    # fmt 和 data 都见过就直接收尾，不必继续扫后面的块。
    cmpl    $3, %ecx
    jne     .Lchunk_loop

.Lparse_done_check:
    # 走到这里说明要么文件遍历完、要么提前收尾；先确认两个关键块都在。
    cmpl    $3, %ecx
    jne     .Lparse_fail

    # 基本合理性检查：声道数、采样率、block align、位深、数据长度都非 0。
    # 只要有一个为 0，后面的转换就会除零或产生空缓冲，直接拒绝。
    cmpw    $0, 2(%rdx)
    je      .Lparse_fail
    cmpl    $0, 4(%rdx)
    je      .Lparse_fail
    cmpw    $0, 8(%rdx)
    je      .Lparse_fail
    cmpw    $0, 10(%rdx)
    je      .Lparse_fail
    cmpl    $0, 16(%rdx)
    je      .Lparse_fail

    # 编码白名单：PCM 8/16/24/32 位，或 float32。其它格式（如 ADPCM、
    # 24 位 float、8 位 A-law/µ-law）一律返回失败，让 C 侧回退 FFmpeg，
    # 奇特的 WAV 仍有机会播放。
    movzwl  0(%rdx), %eax
    cmpl    $1, %eax
    je      .Lcheck_pcm_bits
    cmpl    $3, %eax
    jne     .Lparse_fail
    # float 目前只收 32 位。
    cmpw    $32, 10(%rdx)
    jne     .Lparse_fail
    movl    $1, %eax
    ret

.Lcheck_pcm_bits:
    # PCM 位深白名单：8/16/24/32。
    movzwl  10(%rdx), %eax
    cmpl    $8, %eax
    je      .Lparse_ok
    cmpl    $16, %eax
    je      .Lparse_ok
    cmpl    $24, %eax
    je      .Lparse_ok
    cmpl    $32, %eax
    je      .Lparse_ok
    jmp     .Lparse_fail

.Lparse_ok:
    movl    $1, %eax
    ret
.Lparse_fail:
    xorl    %eax, %eax
    ret
    .size   audio_wav_parse, .-audio_wav_parse

# ---------------------------------------------------------------------------
# int audio_wav_to_s16(const uint8_t *src, int16_t *dst, size_t samples,
#                      uint16_t format, uint16_t bits_per_sample)
#
# 功能：把 src 的 samples 个样本（注意是“样本数”，交错数据里等于
#       帧数 × 声道数）从 WAV 的原始编码转成 int16 写入 dst。
#       成功返回 1，格式不认返回 0。
#
# 本函数只做“格式归一化”。音量渐变、饱和钳制这些播放期处理由音频线程
# 里的 dsp_vol_s16 汇编负责，这里不掺和。
#
# 转换规则：
#   u8 PCM    静音中心是 128，先减 128 移到 0，再左移 8 位
#             （0..255 → -32768..32512，天然落在 int16 范围内，无需钳制）
#   s16 PCM   直接拷贝
#   s24 PCM   三个小端字节拼成低 24 位，第 23 位为 1 时符号扩展，
#             再右移 8 位，保留高 16 位（丢掉最低字节）
#   s32 PCM   算术右移 16 位，取高 16 位
#   f32 float 先钳到 [-1,1]，乘 32767，再转整数
#              （cvtss2si 是向零截断，和 C 里 (int) 语义一致）
#
# 寄存器分工（各循环通用）：
#   rdi = 源指针（每处理一个样本按该格式的字节数前进）
#   rsi = 目标指针（每处理一个样本前进 2 字节）
#   rdx = 剩余样本数（循环计数）
#   eax/r9d = 格式与位深的暂存（进入分派前）
# ---------------------------------------------------------------------------
    .globl  audio_wav_to_s16
    .type   audio_wav_to_s16,@function
audio_wav_to_s16:
    # 源、目标指针为 NULL 直接失败；samples == 0 是合法空转换，直接成功
    # （返回 1），避免调用方把“无数据”当成错误。
    testq   %rdi, %rdi
    jz      .Lconv_fail
    testq   %rsi, %rsi
    jz      .Lconv_fail
    testq   %rdx, %rdx
    jz      .Lconv_ok                  # empty conversion is harmless

    # 取格式（cx 的低 16 位）和位深（r8w），按格式分派。
    # 注意这里参数分别是 uint16_t，所以读寄存器低位即可。
    movzwl  %cx, %eax                  # format
    movzwl  %r8w, %r9d                 # bits per sample
    # format=3 → float32；format=1 → 按位深再分；其它 → 失败。
    cmpl    $3, %eax
    je      .Lconv_float32
    cmpl    $1, %eax
    jne     .Lconv_fail

    # PCM 位深分派：8/16/24/32 各走一个转换循环。
    cmpl    $8, %r9d
    je      .Lconv_u8
    cmpl    $16, %r9d
    je      .Lconv_s16
    cmpl    $24, %r9d
    je      .Lconv_s24
    cmpl    $32, %r9d
    je      .Lconv_s32
    jmp     .Lconv_fail

.Lconv_u8:
    # u8：样本值 0..255，静音是 128。减 128 后范围 -128..127，左移 8 位
    # 变成 -32768..32512，直接存低 16 位即可（不会溢出 int16）。
    movzbl  (%rdi), %eax
    subl    $128, %eax
    shll    $8, %eax
    movw    %ax, (%rsi)
    incq    %rdi                       # u8：源前进 1 字节
    addq    $2, %rsi                   # 目标前进 2 字节
    decq    %rdx
    jnz     .Lconv_u8
    jmp     .Lconv_ok

.Lconv_s16:
    # s16：直接拷贝。故意不调 memcpy，让这个文件保持自包含，也便于
    # 性能剖析时一眼看到快路径在做什么。
    movw    (%rdi), %ax
    movw    %ax, (%rsi)
    addq    $2, %rdi                   # 源前进 2 字节
    addq    $2, %rsi
    decq    %rdx
    jnz     .Lconv_s16
    jmp     .Lconv_ok

.Lconv_s24:
    # s24：一个样本 3 个字节，小端。先拼出低 24 位（byte0 | byte1<<8 |
    # byte2<<16），若 bit23 为 1 说明是负数，把高 8 位全填 1 做符号扩展，
    # 再算术右移 8 位，只保留高 16 位（丢掉最低有效字节）。
    movzbl  0(%rdi), %eax
    movzbl  1(%rdi), %r10d
    shll    $8, %r10d
    orl     %r10d, %eax
    movzbl  2(%rdi), %r10d
    shll    $16, %r10d
    orl     %r10d, %eax
    testl   $0x800000, %eax
    jz      .Lconv_s24_shift
    orl     $0xff000000, %eax
.Lconv_s24_shift:
    sarl    $8, %eax
    movw    %ax, (%rsi)
    addq    $3, %rdi                   # 源前进 3 字节
    addq    $2, %rsi
    decq    %rdx
    jnz     .Lconv_s24
    jmp     .Lconv_ok

.Lconv_s32:
    # s32：小端 4 字节即一个 int32，算术右移 16 位取高 16 位（保留符号）。
    movl    (%rdi), %eax
    sarl    $16, %eax
    movw    %ax, (%rsi)
    addq    $4, %rdi                   # 源前进 4 字节
    addq    $2, %rsi
    decq    %rdx
    jnz     .Lconv_s32
    jmp     .Lconv_ok

.Lconv_float32:
    # float32：位深必须正好 32，否则拒绝。
    cmpl    $32, %r9d
    jne     .Lconv_fail
    # 预载三个常量：缩放系数 32767、钳制上下界 +1.0 / -1.0。
    movss   .Lwav_f32_scale(%rip), %xmm1
    movss   .Lwav_f32_one(%rip), %xmm2
    movss   .Lwav_f32_neg_one(%rip), %xmm3
.Lconv_f32_loop:
    # 读一个 float → minss 钳上界 +1.0 → maxss 钳下界 -1.0 →
    # 乘 32767 → cvtss2si 转整数（向零截断，同 C 的 (int)）→ 存低 16 位。
    # 经过钳制后结果必然落在 int16 范围，movw 只存低 16 位是安全的。
    movss   (%rdi), %xmm0
    minss   %xmm2, %xmm0               # clamp upper bound +1.0
    maxss   %xmm3, %xmm0               # clamp lower bound -1.0
    mulss   %xmm1, %xmm0
    cvtss2sil %xmm0, %eax
    movw    %ax, (%rsi)
    addq    $4, %rdi                   # 源前进 4 字节
    addq    $2, %rsi
    decq    %rdx
    jnz     .Lconv_f32_loop

.Lconv_ok:
    # 全部样本转换完成。
    movl    $1, %eax
    ret
.Lconv_fail:
    # 格式或参数不合法。
    xorl    %eax, %eax
    ret
    .size   audio_wav_to_s16, .-audio_wav_to_s16

    .section .rodata
    .align 4
# float 转换用的三个常量：缩放系数与钳制边界。
.Lwav_f32_scale:
    .float 32767.0
.Lwav_f32_one:
    .float 1.0
.Lwav_f32_neg_one:
    .float -1.0

    .section .note.GNU-stack,"",@progbits
