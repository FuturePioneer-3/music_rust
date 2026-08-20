# audio_wav.S —— WAV fast path for music_rust
# Copyright (C) 2026 FuturePioneer-3
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This file handles the plain WAV case that should not need FFmpeg at all.
# C still owns file I/O and playback-thread lifetime. Assembly owns the two
# tight format-specific jobs:
#
#   1. Walk the RIFF/WAVE chunk table and find "fmt " plus "data".
#   2. Convert the WAV sample payload into the player's native int16_t PCM.
#
# The comments below are intentionally a bit narrative. WAV parsing is simple,
# but raw assembly becomes opaque quickly unless the reader can keep the file
# layout and the register roles in their head.
#
# SysV AMD64 ABI refresher:
#   integer args: rdi, rsi, rdx, rcx, r8, r9
#   return value: eax
#   caller-saved: rax, rcx, rdx, rsi, rdi, r8-r11, xmm0-xmm15
#   callee-saved: rbx, rbp, r12-r15

    .text

# ---------------------------------------------------------------------------
# int audio_wav_parse(const uint8_t *file, size_t len, music_wav_meta *out)
#
# WAV is a RIFF file:
#   0..3   "RIFF"
#   4..7   file size minus 8, little-endian
#   8..11  "WAVE"
#   then chunks: 4-byte id, 4-byte little-endian size, size bytes payload,
#                and one padding byte when size is odd.
#
# We accept two data encodings:
#   format=1: PCM integer, 8/16/24/32-bit
#   format=3: IEEE float, currently 32-bit
#
# Register map inside the chunk loop:
#   rdi = file base
#   rsi = file length
#   rdx = output struct
#   r8  = current chunk header pointer
#   r9  = file end pointer
#   r10 = chunk payload size
#   r11 = chunk payload pointer
#   ecx = flags: bit0 saw fmt, bit1 saw data
# ---------------------------------------------------------------------------
    .globl  audio_wav_parse
    .type   audio_wav_parse,@function
audio_wav_parse:
    testq   %rdi, %rdi
    jz      .Lparse_fail
    testq   %rdx, %rdx
    jz      .Lparse_fail
    cmpq    $20, %rsi                  # 12-byte header + at least one chunk header
    jb      .Lparse_fail

    cmpl    $0x46464952, 0(%rdi)       # "RIFF" in little-endian memory
    jne     .Lparse_fail
    cmpl    $0x45564157, 8(%rdi)       # "WAVE"
    jne     .Lparse_fail

    xorl    %eax, %eax
    movq    $0, 0(%rdx)                # clear first 8 bytes of music_wav_meta
    movq    $0, 8(%rdx)                # clear remaining fields

    leaq    12(%rdi), %r8              # first chunk header
    leaq    (%rdi,%rsi), %r9           # one-past-end pointer
    xorl    %ecx, %ecx                 # seen flags

.Lchunk_loop:
    leaq    8(%r8), %rax               # need a full chunk header
    cmpq    %r9, %rax
    ja      .Lparse_done_check

    movl    4(%r8), %r10d              # payload size (WAV uses little-endian)
    leaq    8(%r8), %r11               # payload starts right after the chunk header
    movq    %r10, %rax
    addq    %r11, %rax                 # payload end
    jc      .Lparse_fail
    cmpq    %r9, %rax
    ja      .Lparse_fail

    cmpl    $0x20746d66, 0(%r8)        # "fmt "
    je      .Lchunk_fmt
    cmpl    $0x61746164, 0(%r8)        # "data"
    je      .Lchunk_data
    jmp     .Lchunk_next

.Lchunk_fmt:
    # The canonical PCM fmt chunk is 16 bytes. Extended WAV can be larger,
    # but the first 16 bytes keep the same fields, which is all this fast path
    # needs before sample conversion.
    cmpl    $16, %r10d
    jb      .Lparse_fail
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
    orl     $1, %ecx
    jmp     .Lchunk_next

.Lchunk_data:
    # Store the data offset relative to file base. C will use it to point at
    # the payload after it has allocated the output PCM buffer.
    movq    %r11, %rax
    subq    %rdi, %rax
    movl    %eax, 12(%rdx)
    movl    %r10d, 16(%rdx)
    orl     $2, %ecx
    jmp     .Lchunk_next

.Lchunk_next:
    movq    %r10, %rax
    addq    $1, %rax                   # chunks with odd length have one pad byte
    andq    $-2, %rax
    leaq    8(%r8,%rax), %r8           # next chunk header
    cmpl    $3, %ecx
    jne     .Lchunk_loop

.Lparse_done_check:
    cmpl    $3, %ecx
    jne     .Lparse_fail

    # Basic sanity: nonzero channels/rate/block-align/bits/data size.
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

    # Supported encodings: PCM 8/16/24/32 and float32. Anything else falls
    # back to FFmpeg so exotic WAV files still have a chance to play.
    movzwl  0(%rdx), %eax
    cmpl    $1, %eax
    je      .Lcheck_pcm_bits
    cmpl    $3, %eax
    jne     .Lparse_fail
    cmpw    $32, 10(%rdx)
    jne     .Lparse_fail
    movl    $1, %eax
    ret

.Lcheck_pcm_bits:
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
# The playback engine wants signed 16-bit interleaved PCM. This routine does
# only format normalization. Volume ramps and clipping are still handled by the
# existing dsp_vol_s16 assembly in the audio thread.
#
# Conversion rules:
#   u8 PCM     -> center at zero, shift into s16 range
#   s16 PCM    -> direct copy
#   s24 PCM    -> sign-extend 24-bit little-endian, keep the top 16 bits
#   s32 PCM    -> keep the top 16 bits
#   f32 float  -> clamp to [-1, 1], multiply by 32767, convert to int
# ---------------------------------------------------------------------------
    .globl  audio_wav_to_s16
    .type   audio_wav_to_s16,@function
audio_wav_to_s16:
    testq   %rdi, %rdi
    jz      .Lconv_fail
    testq   %rsi, %rsi
    jz      .Lconv_fail
    testq   %rdx, %rdx
    jz      .Lconv_ok                  # empty conversion is harmless

    movzwl  %cx, %eax                  # format
    movzwl  %r8w, %r9d                 # bits per sample
    cmpl    $3, %eax
    je      .Lconv_float32
    cmpl    $1, %eax
    jne     .Lconv_fail

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
    # Unsigned 8-bit PCM stores silence as 128. Move the midpoint to zero and
    # shift left so the full byte range fills most of int16.
    movzbl  (%rdi), %eax
    subl    $128, %eax
    shll    $8, %eax
    movw    %ax, (%rsi)
    incq    %rdi
    addq    $2, %rsi
    decq    %rdx
    jnz     .Lconv_u8
    jmp     .Lconv_ok

.Lconv_s16:
    # Direct PCM copy. Keep it explicit instead of calling memcpy so this file
    # stays self-contained and the fast path remains visible in profiles.
    movw    (%rdi), %ax
    movw    %ax, (%rsi)
    addq    $2, %rdi
    addq    $2, %rsi
    decq    %rdx
    jnz     .Lconv_s16
    jmp     .Lconv_ok

.Lconv_s24:
    # A 24-bit sample is three little-endian bytes. Build the low 24 bits,
    # sign-extend if bit 23 is set, then drop the least significant byte.
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
    addq    $3, %rdi
    addq    $2, %rsi
    decq    %rdx
    jnz     .Lconv_s24
    jmp     .Lconv_ok

.Lconv_s32:
    movl    (%rdi), %eax
    sarl    $16, %eax
    movw    %ax, (%rsi)
    addq    $4, %rdi
    addq    $2, %rsi
    decq    %rdx
    jnz     .Lconv_s32
    jmp     .Lconv_ok

.Lconv_float32:
    cmpl    $32, %r9d
    jne     .Lconv_fail
    movss   .Lwav_f32_scale(%rip), %xmm1
    movss   .Lwav_f32_one(%rip), %xmm2
    movss   .Lwav_f32_neg_one(%rip), %xmm3
.Lconv_f32_loop:
    movss   (%rdi), %xmm0
    minss   %xmm2, %xmm0               # clamp upper bound +1.0
    maxss   %xmm3, %xmm0               # clamp lower bound -1.0
    mulss   %xmm1, %xmm0
    cvtss2sil %xmm0, %eax
    movw    %ax, (%rsi)
    addq    $4, %rdi
    addq    $2, %rsi
    decq    %rdx
    jnz     .Lconv_f32_loop

.Lconv_ok:
    movl    $1, %eax
    ret
.Lconv_fail:
    xorl    %eax, %eax
    ret
    .size   audio_wav_to_s16, .-audio_wav_to_s16

    .section .rodata
    .align 4
.Lwav_f32_scale:
    .float 32767.0
.Lwav_f32_one:
    .float 1.0
.Lwav_f32_neg_one:
    .float -1.0

    .section .note.GNU-stack,"",@progbits
