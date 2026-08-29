#!/usr/bin/env python3
# -*- coding: utf-8 -*-
# music_rust —— MIDI 简谱转换器
# Copyright (C) 2026 FuturePioneer-3
# Project: https://github.com/FuturePioneer-3
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License
# along with this program.  If not, see <https://www.gnu.org/licenses/>.
# SPDX-License-Identifier: GPL-3.0-or-later
"""
MIDI → 简谱TXT 转换器
music_rust 钢琴演奏器配套工具

将标准 MIDI 文件 (.mid/.midi) 转换为 music_rust v3.2 绝对事件 TXT。

用法:
    python3 convert.py                         # 打开独立转换界面
    python3 convert.py 输入.mid [-o 输出.txt] [选项]

选项:
    -o, --output <文件>   输出文件 (默认: 输入同名.txt)
    -b, --bpm <n>        覆盖速度 (BPM，1..60000)
    --track <编号...>     只转换指定音轨 (从0开始, 逗号分隔)
    --quantize <tick>    量化粒度 (默认 0=关闭, 建议 30)
    --max-tracks <n>     最多转换多少音轨 (默认全部非打击乐)
    --drum / --no-drum    是否包含打击乐轨 (默认 --no-drum)
    --min-vel <n>        最低力度 (默认 0)
    --velocity-scale <x> 力度倍率；交互终端未指定时会询问 (默认 1.0)
    --keep-empty         保留空白音轨
    --no-chord           不合并和弦 (每个音单独token)
    --initial-soundfont <1..3>
                         初始 SoundFont 编号 (默认 1)
    --instrument <0..127>
                         初始 GM 音色号 (默认 0)
    --switch <秒:音色>  在指定时间切换音色，可重复使用
    --switch <秒:SF:音色>
                         切换 SoundFont 和音色，最多 24 条
    --embed-image <图片> 将图片二进制内嵌到 TXT v3.2
    --image-compression <raw|zstd|gzip|zlib|deflate|bzip2|xz|lz4>
                         图片二进制编码（默认 raw）
    --image-level <n>
                         压缩级别（默认 3；范围因编码而异）
    --v2                 输出旧版可读多音轨 T 格式
    --legacy             输出最旧的 v1/v2 两行一组格式

MIDI 音符编号 → 简谱:
    简谱用 1~7 表示 do re mi fa sol la si
    低音用 ',' (最多3个), 高音用 '^' (最多4个), 升音用 '#'
    休止符用 0, 延音用 '-' (增加一个四分音符), 分音用 '_' (减半)
    附点用 '.' (×1.5), 和弦用 [ ] 包裹
"""

import argparse
import bz2
import gzip
import lzma
import math
import mimetypes
import os
import shutil
import struct
import subprocess
import sys
import textwrap
import zlib
from collections import defaultdict
from dataclasses import dataclass, field
from decimal import Decimal, InvalidOperation, ROUND_HALF_UP

# ---------------------------------------------------------------------------
# 常量
# ---------------------------------------------------------------------------

# MIDI note 60 = 中央 C4 = 简谱 1 (do)
# 映射: 简谱数字(1-7) 的 MIDI 偏移 (相对 C4=60)
DEGREE_OFFSET = {0: 0, 1: 2, 2: 4, 3: 5, 4: 7, 5: 9, 6: 11}  # 0=1(do)...6=7(si)
DEGREE_NAME = {0: '1', 1: '2', 2: '3', 3: '4', 4: '5', 5: '6', 6: '7'}

MAX_LOW_OCTAVE = 3   # 最多 3 个低音逗号
MAX_HIGH_OCTAVE = 4  # 最多 4 个高音尖

# 最小/最大 MIDI 音高
MIN_MIDI = 21   # A0
MAX_MIDI = 108  # C8

# 用于检测"打击乐通道"的通道号 (GM 规定 ch9 是打击乐)
PERCUSSION_CHANNEL = 9

# TXT v3.1 音色元数据限制，与 music_rust 播放端保持一致。
MAX_SOUNDFONTS = 3
MAX_PROGRAM_SWITCHES = 24
# Rust 播放时间线的 ProgramSwitch.at_ms 为 u32。
MAX_SWITCH_MILLISECONDS = (1 << 32) - 1
MAX_EMBEDDED_IMAGE_BYTES = 64 * 1024 * 1024
IMAGE_BEGIN = b'-----BEGIN MUSIC_RUST IMAGE-----\n'
IMAGE_END = b'-----END MUSIC_RUST IMAGE-----\n'
IMAGE_ENCODINGS = ('raw', 'zstd', 'gzip', 'zlib', 'deflate', 'bzip2', 'xz', 'lz4')
IMAGE_LEVEL_RANGES = {
    'zstd': (1, 22),
    'gzip': (0, 9),
    'zlib': (0, 9),
    'deflate': (0, 9),
    'bzip2': (1, 9),
    'xz': (0, 9),
    'lz4': (1, 12),
}

# ---------------------------------------------------------------------------
# MIDI 解析器
# ---------------------------------------------------------------------------

class MidiError(Exception):
    pass


class NoteEvent:
    __slots__ = ('channel', 'note', 'velocity', 'start', 'end')

    def __init__(self, channel, note, velocity, start):
        self.channel = channel
        self.note = note
        self.velocity = velocity
        self.start = start
        self.end = None

    def __repr__(self):
        return f"<Note ch={self.channel} n={self.note} v={self.velocity} {self.start}->{self.end}>"


class MidiTrack:
    __slots__ = ('name', 'events', 'tempo_events')

    def __init__(self):
        self.name = ''
        self.events = []          # list[NoteEvent]
        self.tempo_events = []    # list[(tick, us_per_quarter)]


def _read_varlen(data, pos):
    """读取 MIDI 可变长度整数，返回 (value, new_pos)"""
    value = 0
    for i in range(4):
        if pos >= len(data):
            raise MidiError("MIDI 文件截断 (varlen)")
        b = data[pos]
        pos += 1
        value = (value << 7) | (b & 0x7f)
        if not (b & 0x80):
            return value, pos
    raise MidiError("可变长度整数过长")


def parse_midi(path):
    """解析标准 MIDI 文件。返回 (division, list[MidiTrack], meta)"""
    with open(path, 'rb') as f:
        data = f.read()

    if len(data) < 14:
        raise MidiError("文件过短，不是有效的 MIDI")

    # --- Header ---
    if data[0:4] != b'MThd':
        raise MidiError("缺少 MThd 头，不是标准 MIDI 文件")
    hlen = struct.unpack('>I', data[4:8])[0]
    if hlen < 6:
        raise MidiError("MThd 长度异常")
    header_end = 8 + hlen
    if header_end > len(data):
        raise MidiError("MThd 头截断")
    fmt, ntrks, division = struct.unpack('>HHH', data[8:14])

    # SMF type 0: 单轨; type 1: 多轨同步; type 2: 多轨独立
    if fmt > 2:
        raise MidiError(f"不支持的 SMF 类型: {fmt}")

    if division == 0:
        raise MidiError("MIDI PPQ 不能为 0")
    if division & 0x8000:
        # v3.1 当前以 PPQ + tempo map 表达时间，不能无损表示 SMPTE division。
        # 明确拒绝，避免把二补数帧率字节误当成 PPQ 后生成“看似可播、实际
        # 时间全错”的 TXT。
        raise MidiError("暂不支持 SMPTE time-division MIDI；请先转换为 PPQ MIDI")

    pos = header_end
    tracks = []
    for _ in range(ntrks):
        if pos + 8 > len(data):
            raise MidiError("轨道头截断")
        if data[pos:pos + 4] != b'MTrk':
            raise MidiError(f"缺少 MTrk 头 (位置 {pos})")
        tlen = struct.unpack('>I', data[pos + 4:pos + 8])[0]
        pos += 8
        if pos + tlen > len(data):
            raise MidiError("MIDI 轨道数据截断")
        track_data = data[pos:pos + tlen]
        pos += tlen
        tracks.append(track_data)

    # --- 解析每个轨道 ---
    result_tracks = []
    for trk_idx, tdata in enumerate(tracks):
        track = MidiTrack()
        tick = 0
        running_status = None
        p = 0
        # 预扫描轨名
        while p < len(tdata):
            dt, p = _read_varlen(tdata, p)
            if p >= len(tdata):
                break
            status = tdata[p]
            if status < 0x80:
                # 使用 running status
                if running_status is None:
                    raise MidiError(f"轨道{trk_idx} 在无 running status 时出现数据字节")
                status = running_status
                msg_type = status & 0xf0
            else:
                running_status = status
                msg_type = status & 0xf0
                p = p + 1

            tick += dt

            if msg_type == 0x80:  # Note Off
                if p + 1 >= len(tdata):
                    break
                note = tdata[p]
                vel = tdata[p + 1]
                p += 2
                chan = status & 0x0f
                _register_noteoff(track, chan, note, tick, vel)
            elif msg_type == 0x90:  # Note On
                if p + 1 >= len(tdata):
                    break
                note = tdata[p]
                vel = tdata[p + 1]
                p += 2
                chan = status & 0x0f
                if vel == 0:
                    _register_noteoff(track, chan, note, tick, 0)
                else:
                    _register_noteon(track, chan, note, vel, tick)
            elif msg_type == 0xa0:  # Poly Aftertouch
                p += 2
            elif msg_type == 0xb0:  # Control Change
                p += 2
            elif msg_type == 0xc0:  # Program Change
                p += 1
            elif msg_type == 0xd0:  # Channel Aftertouch
                p += 1
            elif msg_type == 0xe0:  # Pitch Bend
                p += 2
            elif status == 0xff:  # Meta Event
                if p >= len(tdata):
                    break
                mtype = tdata[p]
                p += 1
                mlen, p = _read_varlen(tdata, p)
                if p + mlen > len(tdata):
                    raise MidiError(f"轨道{trk_idx} 在 tick {tick} 的 meta 数据截断")
                meta_data = tdata[p:p + mlen]
                p += mlen
                if mtype == 0x03:  # Track name
                    track.name = meta_data.decode('latin-1', errors='replace')
                elif mtype == 0x51:  # Tempo (us per quarter)
                    if mlen != 3 or len(meta_data) != 3:
                        raise MidiError(
                            f"轨道{trk_idx} 在 tick {tick} 的 tempo 元事件长度必须为 3"
                        )
                    us = (meta_data[0] << 16) | (meta_data[1] << 8) | meta_data[2]
                    if us == 0:
                        raise MidiError(
                            f"轨道{trk_idx} 在 tick {tick} 的 tempo 值不能为 0"
                        )
                    track.tempo_events.append((tick, us))
                elif mtype == 0x2f:  # End of track
                    break
                # 其它 meta 忽略
                running_status = None
            elif msg_type == 0xf0 or msg_type == 0xf7:  # SysEx
                # SysEx: 读取 varlen 长度
                slen, p = _read_varlen(tdata, p)
                if p + slen > len(tdata):
                    raise MidiError(f"轨道{trk_idx} 在 tick {tick} 的 SysEx 数据截断")
                p += slen
                # running status 重置
                running_status = None
            else:
                # 未知状态：跳过（防御）
                running_status = None
                if p < len(tdata):
                    # 尝试跳过
                    p += 1
                # 保守处理：直接结束本轨道
                break

        result_tracks.append(track)

    return division, result_tracks


def _register_noteon(track, channel, note, velocity, tick):
    # 追加一个新的 note，等待匹配的 noteoff
    track.events.append(NoteEvent(channel, note, velocity, tick))


def _register_noteoff(track, channel, note, tick, _vel):
    # 从后往前找匹配的 noteon（未闭合）
    for ev in reversed(track.events):
        if ev.note == note and ev.channel == channel and ev.end is None:
            ev.end = tick
            return
    # 找不到匹配：忽略（防御）


# ---------------------------------------------------------------------------
# 音符 → 简谱
# ---------------------------------------------------------------------------

def midi_to_jianpu(note):
    """
    将 MIDI 音符编号转换为简谱字符串。
    返回 (简谱字符串, 是否成功)。
    基于 C4=60=1(do)，B3=59 是低音 7。
    """
    if not (MIN_MIDI <= note <= MAX_MIDI):
        return None, False

    # 计算相对 C4 的音级数
    degree = (note - 60) % 12  # 0=C,1=C#,2=D...
    octave_offset = (note - 60) // 12  # 0 = 中央区

    # 简谱度映射（用最接近的 do-re-mi）
    # degree: 0=C, 2=D, 4=E, 5=F, 7=G, 9=A, 11=B
    # sharp: 1=C#, 3=D#, 6=F#, 8=G#, 10=A#
    degree_map = {
        0: (0, False), 2: (1, False), 4: (2, False),
        5: (3, False), 7: (4, False), 9: (5, False), 11: (6, False),
        1: (0, True), 3: (1, True), 6: (3, True), 8: (4, True), 10: (5, True),
    }
    if degree not in degree_map:
        return None, False
    deg, is_sharp = degree_map[degree]

    # 计算八度标记
    # octave_offset=0 → 中央区 (1-7 无标记)
    # octave_offset=1 → 高音 (^)
    # octave_offset=-1 → 低音 (,)
    if octave_offset > 0:
        if octave_offset > MAX_HIGH_OCTAVE:
            octave_offset = MAX_HIGH_OCTAVE
        marker = '^' * octave_offset
    elif octave_offset < 0:
        if octave_offset < -MAX_LOW_OCTAVE:
            octave_offset = -MAX_LOW_OCTAVE
        marker = ',' * (-octave_offset)
    else:
        marker = ''

    jp = DEGREE_NAME[deg]
    if is_sharp:
        jp += '#'
    return jp + marker, True


def quantize_tick(tick, quantum):
    """将 tick 量化到最近网格"""
    if quantum <= 0:
        return tick
    return int(round(tick / quantum) * quantum)


def choose_velocity_scale(value=None):
    """选择 MIDI 力度倍率。

    交互终端默认询问；管道/重定向等非交互场景直接使用 1.0，保证转换器
    可以安全地用于脚本。倍率在真正写出前应用，并最终钳制到 MIDI 的 1-127。
    """
    if value is not None:
        if not math.isfinite(value) or value <= 0:
            raise ValueError("力度倍率必须是大于 0 的有限数字")
        return value
    if not sys.stdin.isatty():
        return 1.0
    while True:
        try:
            raw = input("MIDI 力度倍率 [1.0]（例如 0.5、1.5、2）：").strip()
        except EOFError:
            return 1.0
        if not raw:
            return 1.0
        try:
            scale = float(raw)
        except ValueError:
            print("请输入正数，例如 0.5、1 或 1.5。", file=sys.stderr)
            continue
        if math.isfinite(scale) and scale > 0:
            return scale
        print("力度倍率必须是大于 0 的有限数字。", file=sys.stderr)


def scale_velocity(velocity, multiplier):
    """按倍率调整单个 MIDI 力度，并限制到合法范围。"""
    return max(1, min(127, int(round(velocity * multiplier))))


# ---------------------------------------------------------------------------
# 时值 → 简谱后缀
# ---------------------------------------------------------------------------

def _ratio_to_suffix(ratio, quantum=0):
    """
    将"时值比"（相对初始四分音符的倍数）转换为简谱后缀。
    ratio=1 → 无后缀；ratio=0.5 → '_'；ratio=2 → '-'；ratio=1.5 → '.' 等。
    若无精确匹配，返回误差最小的近似后缀。
    """
    if quantum and quantum > 0:
        ratio = round(ratio * 64) / 64.0
    ratio = max(ratio, 0.01)

    def close(a, b, tol=0.03):
        return abs(a - b) / max(1.0, abs(b)) < tol

    # 生成所有候选后缀及对应时值
    candidates = []  # (error, suffix, value)
    for halves in range(0, 5):
        base = 1.0 / (2 ** halves)
        for dots in (0, 1):
            val = base * 1.5 if dots else base
            for n_hold in range(0, 6):
                if halves > 0 and n_hold > 0:
                    continue  # 分音+延音少用
                total = val + n_hold * 1.0
                sub = '_' * halves
                dot = '.' if dots else ''
                dash = '-' * n_hold
                suffix = sub + dot + dash
                err = abs(total - ratio)
                candidates.append((err, suffix, total))

    # 精确匹配优先
    for err, suffix, _v in candidates:
        if err / max(1.0, ratio) < 0.03:
            return suffix

    # 无精确匹配：选误差最小
    if candidates:
        best = min(candidates, key=lambda x: x[0])
        return best[1]

    # 兜底
    n = int(round(ratio))
    if n <= 0:
        n = 1
    return '-' * (n - 1)


def duration_to_suffix_simple(dur_ticks, quarter_ticks, quantum=0):
    """
    将 tick 时值转换为简谱后缀。
    规则:
      base 分音 '_'×k: 时长 = q / 2^k
      '.' 附点: ×1.5
      '-' 延音×n: +n×q
    组合: 分音 → 附点 → 延音，但简谱里常用：
      1/8  = ___?  不对，_ 表示减半一次 → 1/8 = '__'
      实际:
        1 个四分音符 = (无)
        1/2 四分音符(八分) = '_'
        1/4 四分音符(十六分) = '__'
        2 个四分音符(二分) = '-'
        4 个四分音符(全音符) = '---'
        1.5 个四分音符(附点四分) = '.'
        3 个四分音符(附点二分) = '-.'
        0.75 (附点八分) = '_.'
    """
    dur = quantize_tick(dur_ticks, quantum) if quantum else dur_ticks
    if dur <= 0:
        dur = max(1, int(quarter_ticks * 0.05))
    q = float(quarter_ticks)
    ratio = dur / q
    return _ratio_to_suffix(ratio, 0)


# ---------------------------------------------------------------------------
# 主转换逻辑
# ---------------------------------------------------------------------------

# ---------------------------------------------------------------------------
# 动态 BPM (tempo 时间线)
# ---------------------------------------------------------------------------

DEFAULT_TEMPO_US = 500000  # 120 BPM


def build_tempo_timeline(tracks):
    """
    从所有轨道收集 tempo 时间线。
    返回 (initial_us, [(tick, us_per_quarter), ...])。
    标准 MIDI 通常把 tempo 放在 conductor track，但现实文件经常不遵守这一点；
    收集全部轨道并按 tick 合并可避免丢失变速事件。
    """
    by_tick = {}
    for track_index, track in enumerate(tracks):
        for tick, us in track.tempo_events:
            if not isinstance(us, int) or us <= 0:
                raise MidiError(
                    f"音轨 {track_index} 在 tick {tick} 的 tempo 必须是大于 0 的整数"
                )
            by_tick[tick] = us
    timeline = sorted(by_tick.items())
    # 归一化：确保 tick=0 时有 tempo（无则用默认）
    initial_us = DEFAULT_TEMPO_US
    if timeline and timeline[0][0] == 0:
        initial_us = timeline[0][1]
    # 在开头补一个默认 tempo（若 tick=0 无事件）
    if not timeline or timeline[0][0] != 0:
        timeline.insert(0, (0, initial_us))
    return initial_us, timeline


def tempo_us_at(timeline, tick):
    """查询某 tick 时刻生效的 us_per_quarter。"""
    us = DEFAULT_TEMPO_US
    for t, u in timeline:
        if t <= tick:
            us = u
        else:
            break
    return us


def tempo_change_points(timeline, initial_us):
    """返回所有实际变速点 [(tick, us_per_quarter), ...]，不含初始段。"""
    points = []
    prev_us = initial_us
    for tick, us in timeline:
        if us != prev_us:
            points.append((tick, us))
            prev_us = us
    return points


def convert(midi_path, out_path, opts):
    division, tracks = parse_midi(midi_path)
    velocity_scale = choose_velocity_scale(getattr(opts, 'velocity_scale', None))
    # 记录交互选择的实际值，主程序用于回显。
    opts.velocity_scale = velocity_scale

    # 动态 BPM：构建跨所有轨道的全局 tempo 时间线
    initial_us, tempo_timeline = build_tempo_timeline(tracks)

    source_bpm = 60000000 / initial_us
    bpm = opts.bpm if opts.bpm else int(round(source_bpm))
    # -b/--bpm 是整体速度缩放，而不是只改第一拍；保留原 MIDI 的变速比例。
    if opts.bpm:
        target_initial_us = 60000000 / opts.bpm
        scale = target_initial_us / initial_us
        tempo_timeline = [(tick, max(1, int(round(us * scale)))) for tick, us in tempo_timeline]
        initial_us = tempo_timeline[0][1]
    initial_tempo_ms = 60000 / bpm  # 初始 ms/四分音符
    quarter_ticks = division  # 一个四分音符的 tick 数

    # 构建音轨
    out_tracks = []  # (source MIDI index, name, list[NoteEvent])
    for idx, t in enumerate(tracks):
        # 先按原始 MIDI 索引筛选，再做其它过滤；--track 永远引用原始轨道号。
        if opts.track_list is not None and idx not in opts.track_list:
            continue
        # 过滤打击乐。type-0 MIDI 常把多个通道塞进同一物理轨道；必须逐事件
        # 去掉 ch9，不能只在整轨“全是打击乐”时才过滤，否则鼓点会混进旋律。
        if not opts.drum:
            t.events = [ev for ev in t.events if ev.channel != PERCUSSION_CHANNEL]
        # 过滤低力度
        if opts.min_vel:
            t.events = [ev for ev in t.events if ev.velocity >= opts.min_vel]
        if velocity_scale != 1.0:
            for ev in t.events:
                ev.velocity = scale_velocity(ev.velocity, velocity_scale)
        if not opts.keep_empty and not t.events:
            continue
        name = t.name.strip() if t.name.strip() else f"Track{idx + 1}"
        out_tracks.append((idx, name, t.events))
        if opts.max_tracks is not None and len(out_tracks) >= opts.max_tracks:
            break

    # 按轨转简谱
    lines = []
    output_track_count = len(out_tracks)
    if opts.legacy:
        legacy_tracks = [(name, events) for _idx, name, events in out_tracks]
        lines.extend([f"#TITLE {os.path.splitext(os.path.basename(midi_path))[0]}", f"#BPM {bpm}", ""])
        _convert_legacy(legacy_tracks, division, opts, lines, tempo_timeline, initial_tempo_ms)
    elif opts.v2:
        v2_tracks = [(name, events) for _idx, name, events in out_tracks]
        lines.extend([f"#TITLE {os.path.splitext(os.path.basename(midi_path))[0]}", f"#BPM {bpm}", ""])
        global_start = min((e.start for _n, _name, evs in out_tracks for e in evs), default=0)
        for name, events in v2_tracks:
            _convert_track_to_lines(name, events, division, opts, lines, tempo_timeline, initial_tempo_ms, global_start)
    else:
        embedded_image = _prepare_embedded_image(opts)
        output_track_count = _convert_v3(
            out_tracks, division, tempo_timeline, midi_path, lines, opts,
            embedded_image,
        )

    # 写入
    content = '\n'.join(lines).encode('utf-8')
    if not content.endswith(b'\n'):
        content += b'\n'
    if not opts.legacy and not opts.v2 and embedded_image is not None:
        _mime, _encoding, payload, _raw_size = embedded_image
        content += IMAGE_BEGIN + payload + b'\n' + IMAGE_END
    with open(out_path, 'wb') as f:
        f.write(content)
    return output_track_count


def _quote_track_name(name):
    """v3 track 名称按整行保存，转义反斜杠和双引号。"""
    return '"' + name.replace('\\', '\\\\').replace('"', '\\"') + '"'


def _format_switch_seconds(at_ms):
    """把整数毫秒写成无浮点误差的简洁秒数。"""
    whole, fraction = divmod(at_ms, 1000)
    if not fraction:
        return str(whole)
    return f"{whole}.{fraction:03d}".rstrip('0')


def _expand_v3_tracks_by_channel(out_tracks):
    """把一个物理 MIDI 轨中的多个通道拆成独立 TXT 轨。

    TXT 的 `@TRACK` 保存唯一通道，而 `@NOTE` 本身没有 channel 字段。普通
    单通道轨继续使用原始 MIDI track id；多通道轨的第一个通道也保留该 id，
    其余通道从所有原始 id 之后分配，避免与后续物理轨冲突。
    """
    reserved_ids = {source_idx for source_idx, _name, _events in out_tracks}
    next_id = max(reserved_ids, default=-1) + 1
    expanded = []

    for source_idx, name, events in out_tracks:
        by_channel = defaultdict(list)
        for event in events:
            by_channel[event.channel].append(event)
        if not by_channel:
            by_channel[0] = []

        channels = sorted(by_channel)
        for position, channel in enumerate(channels):
            if position == 0:
                track_id = source_idx
            else:
                while next_id in reserved_ids:
                    next_id += 1
                track_id = next_id
                reserved_ids.add(track_id)
                next_id += 1
            track_name = name if len(channels) == 1 else f"{name} [ch{channel + 1}]"
            expanded.append((track_id, channel, track_name, by_channel[channel]))
    return expanded


def _image_mime(path, data):
    """用文件头优先判断图片类型，扩展名只作为兜底。"""
    signatures = (
        (b'\x89PNG\r\n\x1a\n', 'image/png'),
        (b'\xff\xd8\xff', 'image/jpeg'),
        (b'GIF87a', 'image/gif'),
        (b'GIF89a', 'image/gif'),
        (b'RIFF', 'image/webp'),
        (b'BM', 'image/bmp'),
    )
    for signature, mime in signatures:
        if data.startswith(signature):
            return mime
    return mimetypes.guess_type(path)[0] or 'application/octet-stream'


def _compress_embedded_image(raw, encoding, level):
    """按编码压缩图片，返回压缩后的二进制。"""
    if encoding == 'raw':
        return raw
    if encoding == 'gzip':
        return gzip.compress(raw, compresslevel=level, mtime=0)
    if encoding == 'zlib':
        return zlib.compress(raw, level)
    if encoding == 'deflate':
        compressor = zlib.compressobj(level, zlib.DEFLATED, -15)
        return compressor.compress(raw) + compressor.flush()
    if encoding == 'bzip2':
        return bz2.compress(raw, compresslevel=level)
    if encoding == 'xz':
        return lzma.compress(raw, preset=level, format=lzma.FORMAT_XZ)
    if encoding == 'zstd':
        zstd = shutil.which('zstd')
        if not zstd:
            raise ValueError('选择 zstd 压缩时需要安装 zstd 命令')
        result = subprocess.run(
            [zstd, f'-{level}', '--stdout', '--no-progress'],
            input=raw, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
        )
        if result.returncode != 0:
            detail = result.stderr.decode('utf-8', errors='replace').strip()
            raise ValueError(f'zstd 压缩失败: {detail or result.returncode}')
        return result.stdout
    if encoding == 'lz4':
        lz4 = shutil.which('lz4')
        if not lz4:
            raise ValueError('选择 lz4 压缩时需要安装 lz4 命令')
        result = subprocess.run(
            [lz4, '-q', f'-{level}', '-c'],
            input=raw, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
        )
        if result.returncode != 0:
            detail = result.stderr.decode('utf-8', errors='replace').strip()
            raise ValueError(f'lz4 压缩失败: {detail or result.returncode}')
        return result.stdout
    raise ValueError(f'图片编码必须是 {"、".join(IMAGE_ENCODINGS)} 之一')


def _prepare_embedded_image(opts):
    """返回 (MIME, 编码, payload, 原始大小)，未启用时返回 None。"""
    image_path = getattr(opts, 'image_path', None)
    if not image_path:
        return None
    image_path = os.path.abspath(os.path.expanduser(image_path))
    if not os.path.isfile(image_path):
        raise ValueError(f'找不到内嵌图片: {image_path}')
    with open(image_path, 'rb') as f:
        raw = f.read()
    if not raw:
        raise ValueError('内嵌图片不能为空')
    if len(raw) > MAX_EMBEDDED_IMAGE_BYTES:
        raise ValueError('内嵌图片不能超过 64 MiB')
    encoding = getattr(opts, 'image_compression', 'raw').lower()
    if encoding not in IMAGE_ENCODINGS:
        raise ValueError(f'图片编码必须是 {"、".join(IMAGE_ENCODINGS)} 之一')
    level_range = IMAGE_LEVEL_RANGES.get(encoding)
    level = int(getattr(opts, 'image_level', 3))
    if level_range is not None and not level_range[0] <= level <= level_range[1]:
        raise ValueError(f'{encoding} 压缩级别必须在 {level_range[0]}..{level_range[1]} 之间')
    payload = _compress_embedded_image(raw, encoding, level)
    return _image_mime(image_path, raw), encoding, payload, len(raw)


def _convert_v3(out_tracks, division, tempo_timeline, midi_path, lines, opts, embedded_image=None):
    """输出 v3.2：音色元数据、绝对 tick 音符、全局 tempo 表和可选图片。

    单通道 MIDI 轨保留原始索引；同一物理轨内的多个通道会拆成独立 TXT 轨，
    避免 type-0 MIDI 被折叠到首通道。音符不再通过简谱 token 的顺序和休止符
    重建时间，因而不会出现轨道错位、短音符被拉长或变速点漂移。
    """
    initial_soundfont = getattr(opts, 'initial_soundfont', 1)
    instrument = getattr(opts, 'instrument', 0)
    program_switches = getattr(opts, 'program_switches', [])
    format_version = str(getattr(opts, 'format', 'v3.2')).lstrip('vV')
    lines.extend([
        f"#MUSIC_RUST {format_version}",
        f"#TITLE {os.path.splitext(os.path.basename(midi_path))[0]}",
        f"#PPQ {division}",
        f"@INSTRUMENT {initial_soundfont} {instrument}",
    ])
    if embedded_image is not None:
        mime, encoding, payload, raw_size = embedded_image
        lines.append(f'@IMAGE {mime} {encoding} {len(payload)} {raw_size}')
    for at_ms, soundfont, switch_instrument in program_switches:
        lines.append(
            f"@SWITCH {_format_switch_seconds(at_ms)} {soundfont} {switch_instrument}"
        )
    for tick, us in tempo_timeline:
        lines.append(f"@TEMPO {tick} {us}")
    lines.append("")
    expanded_tracks = _expand_v3_tracks_by_channel(out_tracks)
    for track_id, channel, name, events in expanded_tracks:
        lines.append(f"@TRACK {track_id} {channel} {_quote_track_name(name)}")
        for event in sorted(events, key=lambda e: (e.start, e.note, e.end or e.start)):
            if event.end is None or event.end <= event.start:
                continue
            lines.append(f"@NOTE {track_id} {event.start} {event.end - event.start} {event.note} {event.velocity}")
        lines.append("")
    return len(expanded_tracks)


def _track_lead_rests(events, division, tempo_timeline, initial_tempo_ms, global_start):
    """
    计算某轨在 global_start 之前应补的休止符数量（相对初始四分音符）。
    返回 (rest_count, 需要前置的 tempo 指令列表)。
    若该轨第一个音符就在 global_start，返回空。
    """
    first_start = min((e.start for e in events), default=0)
    if first_start <= global_start:
        return 0, []
    lead_tick = first_start - global_start
    # 用 tempo 时间线把 [global_start, first_start) 换算成毫秒
    lead_ms = 0.0
    # 遍历 tempo 分段
    seg_start = global_start
    # 收集 tempo 变化点（在区间内）
    points = [(t, u) for t, u in (tempo_timeline or []) if seg_start < t < first_start]
    points.sort()
    us_now = tempo_us_at(tempo_timeline, seg_start) if tempo_timeline else 500000
    for t, u in points:
        seg_ms = (t - seg_start) / division * us_now / 1000.0
        lead_ms += seg_ms
        seg_start = t
        us_now = u
    seg_ms = (first_start - seg_start) / division * us_now / 1000.0
    lead_ms += seg_ms

    if initial_tempo_ms <= 0:
        return 0, []
    rest_count = int(round(lead_ms / initial_tempo_ms))
    return max(rest_count, 0), []


def _token_ratio(tok):
    """计算一个简谱 token 的时值（相对四分音符数），与 Rust 的 parse_token 一致。

    基础 = 1.0；'_' 减半；'*' 除3；'%' 除5；'&' 除7；'.' ×1.5；'-' +1。
    """
    ctn = 1.0
    for ch in tok:
        if ch == '_':
            ctn /= 2
        elif ch == '*':
            ctn /= 3
        elif ch == '%':
            ctn /= 5
        elif ch == '&':
            ctn /= 7
        elif ch == '.':
            ctn *= 1.5
        elif ch == '-':
            ctn += 1
    return ctn


def _convert_track_to_lines(name, events, division, opts, lines, tempo_timeline, initial_tempo_ms, global_start=0):
    """将一个音轨转换为 T 轨格式的多行简谱。

    变速指令按"绝对音乐时间"插入：模拟该轨从 0ms 起的绝对时间推进，
    当推进到全局变速点（绝对毫秒）时，插入独立 `#BPM xxx` 行，
    保证多轨的变速时刻严格对齐。
    同时为"该轨首个音符之前"补休止符，对齐全局时间线起始。
    """
    # 全局变速点（绝对毫秒）：用 tempo 时间线换算
    # timeline 每段 (tick, us)，逐段累计出每个变速点处的绝对 ms
    changes_ms = []  # [(abs_ms, bpm)]，含初始后的所有变速
    if tempo_timeline:
        base_us = tempo_timeline[0][1]
        prev_tick = tempo_timeline[0][0]
        prev_us = base_us
        acc_ms = 0.0
        for tick, us in tempo_timeline[1:]:
            seg_ms = (tick - prev_tick) / division * prev_us / 1000.0
            acc_ms += seg_ms
            if us != base_us:
                changes_ms.append((acc_ms, round(60000000 / us)))
            prev_tick, prev_us = tick, us
    changes_ms.sort()
    change_idx = 0
    initial_bpm = int(round(60000 / initial_tempo_ms)) if initial_tempo_ms else 120

    evs = sorted(events, key=lambda e: (e.start, e.note))

    # 构建 token 序列（不含变速指令）：(token_str, start_tick)
    note_tokens = []
    i = 0
    n = len(evs)
    while i < n:
        start = evs[i].start
        chord = []
        j = i
        while j < n and evs[j].start == start:
            chord.append(evs[j])
            j += 1
        tok, _consumed = _chord_to_token(chord, division, opts)
        note_tokens.append((tok, start))
        i = j

    # 轨道开头补休止符（对齐全局时间线）
    rest_count = 0
    if events:
        rest_count, _lt = _track_lead_rests(events, division, tempo_timeline, initial_tempo_ms, global_start)

    # 模拟时间推进，生成最终 token 流（含变速指令）
    final_tokens = []  # (kind, payload)
    # kind: 'note' | 'tempo' | 'rest'
    cur_tempo_ms = initial_tempo_ms if initial_tempo_ms else 500.0
    t_ms = 0.0

    # 先输出休止符
    if rest_count > 0:
        final_tokens.append(('rest', rest_count))
        t_ms += rest_count * cur_tempo_ms  # 休止符按当前 tempo 推进
        # 休止符可能跨越变速点——处理
        while change_idx < len(changes_ms) and changes_ms[change_idx][0] <= t_ms:
            bpm = changes_ms[change_idx][1]
            if bpm != initial_bpm:
                final_tokens.append(('tempo', bpm))
                cur_tempo_ms = 60000 / bpm
            change_idx += 1

    for tok, _start in note_tokens:
        ratio = _token_ratio(tok)
        t_after = t_ms + ratio * cur_tempo_ms
        # 若本 token 的时值区间 [t_ms, t_after] 跨越了变速点，则在 token 前插入变速
        # （误差 ≤ 半个 token 时值，保证变速后音符用新 tempo）
        while change_idx < len(changes_ms) and changes_ms[change_idx][0] <= t_after:
            bpm = changes_ms[change_idx][1]
            if bpm != initial_bpm:
                final_tokens.append(('tempo', bpm))
                cur_tempo_ms = 60000 / bpm
            change_idx += 1
        final_tokens.append(('note', tok))
        t_ms = t_after

    # 生成行：每行 ~40 tokens 断行。
    lines.append(f"T {name}")
    row = []
    for kind, payload in final_tokens:
        if kind == 'tempo':
            if row:
                lines.append(' '.join(row))
                row = []
            lines.append(f"#BPM {payload}")
            continue
        if kind == 'rest':
            row.extend(['0'] * payload)
            if len(row) >= 40:
                lines.append(' '.join(row))
                row = []
            continue
        row.append(payload)
        if len(row) >= 40:
            lines.append(' '.join(row))
            row = []
    if row:
        lines.append(' '.join(row))
    lines.append("")


def _convert_legacy(out_tracks, division, opts, lines, tempo_timeline=None, initial_tempo_ms=None):
    """
    旧版 v1/v2 格式：
      第一行纯数字 = 四分音符毫秒
      两行一组 = 左右手同时播放；空行打断
    简化：将前两个音轨作为左右手配对输出。
    """
    if not out_tracks:
        lines.append("500")
        lines.append("")
        return

    # 旧版无 #BPM，首行是纯数字毫秒
    # 从 #BPM 换算：tempo_ms = 60000 / bpm
    # 从 lines 里取出 #BPM
    bpm = 120
    for l in lines:
        if l.startswith('#BPM'):
            try:
                bpm = int(l.split()[1])
            except Exception:
                pass
    tempo_ms = int(60000 / bpm)
    if initial_tempo_ms is None:
        initial_tempo_ms = tempo_ms
    lines[:] = [f"{tempo_ms}", ""]

    # 将音轨两两配对（最多支持双轨）
    tracks = out_tracks[:2]
    # 每个音轨转成 token 序列
    track_tokens = []
    for name, events in tracks:
        evs = sorted(events, key=lambda e: (e.start, e.note))
        toks = []
        i = 0
        n = len(evs)
        while i < n:
            start = evs[i].start
            chord = []
            j = i
            while j < n and evs[j].start == start:
                chord.append(evs[j])
                j += 1
            tok, _consumed = _chord_to_token(chord, division, opts)
            toks.append(tok)
            i = j
        track_tokens.append(toks)

    if len(track_tokens) == 1:
        # 单轨：每行 40 tokens
        toks = track_tokens[0]
        for k in range(0, len(toks), 40):
            lines.append(' '.join(toks[k:k + 40]))
        lines.append('')
    else:
        # 双轨：两行一组
        t1, t2 = track_tokens[0], track_tokens[1]
        maxlen = max(len(t1), len(t2))
        k = 0
        while k < maxlen:
            lines.append(' '.join(t1[k:k + 40]))
            lines.append(' '.join(t2[k:k + 40]))
            lines.append('')
            k += 40


def _chord_to_token(chord_events, division, opts, tempo_timeline=None, initial_tempo_ms=None):
    """
    将同一时刻的一个或多个音符转换为简谱 token（含时值后缀）。
    返回 (token_str, 消耗的 tick)。
    简谱时值为相对四分音符数，与 BPM 无关（动态 BPM 由 #BPM 指令处理）。
    """
    del tempo_timeline, initial_tempo_ms  # 动态BPM改用 #BPM 指令，无需归一化
    chord_events = sorted(chord_events, key=lambda e: e.note)
    # 时值: 取最长
    max_dur = max((e.end - e.start for e in chord_events), default=0)
    dur_suffix = duration_to_suffix_simple(max_dur, division, opts.quantize)

    parts = []
    for e in chord_events:
        jp, ok = midi_to_jianpu(e.note)
        if ok:
            parts.append(jp)

    if opts.no_chord or len(parts) <= 1:
        tok = (parts[0] if parts else '0') + dur_suffix
    else:
        tok = '[' + ''.join(parts) + ']' + dur_suffix

    return tok, max_dur


# ---------------------------------------------------------------------------
# 主程序
# ---------------------------------------------------------------------------

def _bounded_int(name, minimum, maximum):
    """创建供 argparse 使用的整数范围校验器。"""
    def parse(value):
        try:
            number = int(value, 10)
        except ValueError as exc:
            raise argparse.ArgumentTypeError(f"{name}必须是整数") from exc
        if not minimum <= number <= maximum:
            raise argparse.ArgumentTypeError(
                f"{name}必须在 {minimum}..{maximum} 之间"
            )
        return number
    return parse


def _parse_switches(specs, initial_soundfont):
    """解析 --switch，转为毫秒并按时间稳定排序。"""
    if len(specs) > MAX_PROGRAM_SWITCHES:
        raise ValueError(f"--switch 最多可以指定 {MAX_PROGRAM_SWITCHES} 条")

    switches = []
    for spec in specs:
        parts = [part.strip() for part in spec.split(':')]
        if len(parts) == 2:
            seconds_text, instrument_text = parts
            soundfont_text = str(initial_soundfont)
        elif len(parts) == 3:
            seconds_text, soundfont_text, instrument_text = parts
        else:
            raise ValueError(
                f"--switch '{spec}' 格式错误，应为 秒:音色 或 秒:SF编号:音色"
            )

        try:
            seconds = Decimal(seconds_text)
        except InvalidOperation as exc:
            raise ValueError(f"--switch '{spec}' 的秒数无效") from exc
        if not seconds.is_finite() or seconds < 0:
            raise ValueError(f"--switch '{spec}' 的秒数必须是非负有限数")
        latest_roundable = (
            Decimal(MAX_SWITCH_MILLISECONDS) + Decimal('0.5')
        ) / Decimal(1000)
        if seconds >= latest_roundable:
            raise ValueError(f"--switch '{spec}' 的时间超出可表示范围")

        try:
            soundfont = int(soundfont_text, 10)
            instrument = int(instrument_text, 10)
        except ValueError as exc:
            raise ValueError(f"--switch '{spec}' 的 SF 编号和音色号必须是整数") from exc
        if not 1 <= soundfont <= MAX_SOUNDFONTS:
            raise ValueError(
                f"--switch '{spec}' 的 SF 编号必须在 1..{MAX_SOUNDFONTS} 之间"
            )
        if not 0 <= instrument <= 127:
            raise ValueError(f"--switch '{spec}' 的音色号必须在 0..127 之间")

        # Decimal 避免二进制浮点偏差；超过毫秒的小数按四舍五入归一。
        at_ms = int((seconds * 1000).to_integral_value(rounding=ROUND_HALF_UP))
        if at_ms > MAX_SWITCH_MILLISECONDS:
            raise ValueError(f"--switch '{spec}' 的时间超出可表示范围")
        switches.append((at_ms, soundfont, instrument))

    # Python 排序稳定，同一毫秒的记录保持命令行输入顺序。
    switches.sort(key=lambda item: item[0])
    return switches


# ---------------------------------------------------------------------------
# 无参数 curses 转换界面
# ---------------------------------------------------------------------------

TUI_FORMATS = ('v3.2', 'v3.1', 'v2', 'legacy')
TUI_FIELDS = (
    ('midi', 'MIDI 文件', 'path', '回车输入，P 浏览'),
    ('output', '输出 TXT', 'path', '留空时使用 MIDI 同名 .txt'),
    ('format', '输出格式', 'format', 'v3.2 支持内嵌图片；v3.1 仅保存音色切换'),
    ('embed_image', '内嵌图片', 'bool', 'v3.2 专用；是后填写图片文件'),
    ('image_path', '图片文件', 'path', 'v3.2 专用；PNG/JPEG/WebP 等图片'),
    ('image_compression', '图片编码', 'choice', 'raw/zstd/gzip/zlib/deflate/bzip2/xz/lz4'),
    ('image_level', '压缩级别', 'text', 'zstd 1..22；gzip/zlib/deflate/xz 0..9；bzip2 1..9；lz4 1..12'),
    ('bpm', 'BPM 覆盖', 'text', '留空时保留 MIDI 速度'),
    ('velocity_scale', '力度倍率', 'text', '大于 0，默认 1.0'),
    ('track', '音轨编号', 'text', '例如 0,2,5；留空为全部'),
    ('quantize', '量化 tick', 'text', '0 表示关闭'),
    ('max_tracks', '最多音轨', 'text', '留空为不限制'),
    ('drum', '包含打击乐', 'bool', '是/否'),
    ('min_vel', '最低力度', 'text', '0..127'),
    ('keep_empty', '保留空轨', 'bool', '是/否'),
    ('no_chord', '不合并和弦', 'bool', '是/否'),
    ('initial_soundfont', '初始 SF 编号', 'text', '1..3，v3.1 专用'),
    ('instrument', '初始 GM 音色', 'text', '0..127，v3.1 专用'),
)


@dataclass
class ConverterTuiState:
    """独立转换界面的可编辑数据，不与 Rust 主 TUI 共享状态。"""

    midi: str = ''
    output: str = ''
    format: str = 'v3.2'
    embed_image: bool = False
    image_path: str = ''
    image_compression: str = 'raw'
    image_level: str = '3'
    bpm: str = ''
    velocity_scale: str = '1.0'
    track: str = ''
    quantize: str = '0'
    max_tracks: str = ''
    drum: bool = False
    min_vel: str = '0'
    keep_empty: bool = False
    no_chord: bool = False
    initial_soundfont: str = '1'
    instrument: str = '0'
    switches: list = field(default_factory=list)


def _parse_tui_int(value, label, minimum=None, maximum=None, optional=False):
    text = value.strip()
    if optional and not text:
        return None
    try:
        number = int(text, 10)
    except ValueError as exc:
        raise ValueError(f"{label}必须是整数") from exc
    if minimum is not None and number < minimum:
        raise ValueError(f"{label}不能小于 {minimum}")
    if maximum is not None and number > maximum:
        raise ValueError(f"{label}不能大于 {maximum}")
    return number


def _build_tui_job(state):
    """校验 TUI 表单，返回 (midi_path, output_path, argparse.Namespace)。"""
    midi_path = os.path.abspath(os.path.expanduser(state.midi.strip()))
    if not state.midi.strip():
        raise ValueError('请先选择或输入 MIDI 文件')
    if not os.path.isfile(midi_path):
        raise ValueError(f"找不到 MIDI 文件: {midi_path}")
    if not midi_path.lower().endswith(('.mid', '.midi')):
        raise ValueError('输入文件必须是 .mid 或 .midi')

    output_text = state.output.strip()
    if output_text:
        output_path = os.path.abspath(os.path.expanduser(output_text))
    else:
        output_path = os.path.splitext(midi_path)[0] + '.txt'
    if os.path.normcase(output_path) == os.path.normcase(midi_path):
        raise ValueError('输出文件不能覆盖输入 MIDI')
    output_dir = os.path.dirname(output_path) or os.getcwd()
    if not os.path.isdir(output_dir):
        raise ValueError(f"输出目录不存在: {output_dir}")

    if state.format not in TUI_FORMATS:
        raise ValueError('输出格式无效')
    if state.embed_image and state.format != 'v3.2':
        raise ValueError('内嵌图片仅能用于 v3.2 输出格式')
    if state.image_compression not in IMAGE_ENCODINGS:
        raise ValueError('图片编码无效')
    image_path = state.image_path.strip() if state.embed_image else None
    if state.embed_image and not image_path:
        raise ValueError('已启用内嵌图片，请填写图片文件')
    level_range = IMAGE_LEVEL_RANGES.get(state.image_compression)
    if level_range is not None:
        image_level = _parse_tui_int(state.image_level, '压缩级别', *level_range)
    else:
        image_level = 3
    bpm = _parse_tui_int(state.bpm, 'BPM', 1, 60_000, optional=True)
    quantize = _parse_tui_int(state.quantize, '量化 tick', 0)
    max_tracks = _parse_tui_int(state.max_tracks, '最多音轨', 1, optional=True)
    min_vel = _parse_tui_int(state.min_vel, '最低力度', 0, 127)
    initial_soundfont = _parse_tui_int(state.initial_soundfont, '初始 SF 编号', 1, 3)
    instrument = _parse_tui_int(state.instrument, '初始 GM 音色', 0, 127)

    try:
        velocity_scale = float(state.velocity_scale.strip())
    except ValueError as exc:
        raise ValueError('力度倍率必须是数字') from exc
    if not math.isfinite(velocity_scale) or velocity_scale <= 0:
        raise ValueError('力度倍率必须是大于 0 的有限数字')

    track_list = None
    if state.track.strip():
        try:
            track_list = [int(item.strip(), 10) for item in state.track.split(',')]
        except ValueError as exc:
            raise ValueError('音轨编号应为逗号分隔的非负整数') from exc
        if not track_list or any(index < 0 for index in track_list):
            raise ValueError('音轨编号应为逗号分隔的非负整数')

    program_switches = list(state.switches)
    if len(program_switches) > MAX_PROGRAM_SWITCHES:
        raise ValueError(f"定时切换最多 {MAX_PROGRAM_SWITCHES} 条")
    program_switches.sort(key=lambda item: item[0])
    for at_ms, soundfont, switch_instrument in program_switches:
        if not isinstance(at_ms, int) or not 0 <= at_ms <= MAX_SWITCH_MILLISECONDS:
            raise ValueError('定时切换的时间超出可表示范围')
        if not 1 <= soundfont <= MAX_SOUNDFONTS:
            raise ValueError('定时切换的 SF 编号必须在 1..3 之间')
        if not 0 <= switch_instrument <= 127:
            raise ValueError('定时切换的 GM 音色必须在 0..127 之间')

    legacy = state.format == 'legacy'
    v2 = state.format == 'v2'
    if (v2 or legacy) and (
        initial_soundfont != 1 or instrument != 0 or program_switches
    ):
        raise ValueError('v2/legacy 无法保存音色配置；请选择 v3.1/v3.2 或清空音色切换')

    opts = argparse.Namespace(
        bpm=bpm,
        track=state.track.strip() or None,
        track_list=track_list,
        quantize=quantize,
        max_tracks=max_tracks,
        drum=state.drum,
        min_vel=min_vel,
        velocity_scale=velocity_scale,
        keep_empty=state.keep_empty,
        no_chord=state.no_chord,
        initial_soundfont=initial_soundfont,
        instrument=instrument,
        program_switches=program_switches,
        image_path=image_path,
        image_compression=state.image_compression,
        image_level=image_level,
        format=state.format,
        switch=[],
        v2=v2,
        legacy=legacy,
    )
    return midi_path, output_path, opts


def _safe_addstr(screen, y, x, value, attr=0):
    """在终端边界内写文字，窗口缩放时不抛 curses.error。"""
    try:
        height, width = screen.getmaxyx()
        if y < 0 or y >= height or x < 0 or x >= width - 1:
            return
        # curses 按显示列计算宽度，Python 切片按字符计算；多留一列
        # 并捕获边界错误，可兼容中文宽字符。
        screen.addstr(y, x, str(value)[:max(0, width - x - 1)], attr)
    except Exception:
        pass


def _set_cursor_visible(visible):
    try:
        import curses
        curses.curs_set(1 if visible else 0)
    except Exception:
        pass


def _prompt_text(screen, title, initial='', allow_empty=True):
    """单行文本输入框；回车确认，Esc 取消。"""
    import curses

    value = str(initial)
    _set_cursor_visible(True)
    try:
        while True:
            screen.erase()
            height, width = screen.getmaxyx()
            _safe_addstr(screen, 1, 2, title, curses.A_BOLD)
            _safe_addstr(screen, 3, 2, '回车确认  Esc 取消  Ctrl+U 清空', curses.A_DIM)
            available = max(1, width - 6)
            shown = value[-available:]
            _safe_addstr(screen, 5, 2, '> ' + shown, curses.A_REVERSE)
            if height > 7:
                _safe_addstr(screen, 7, 2, f'字符数: {len(value)}', curses.A_DIM)
            try:
                screen.move(min(5, height - 1), min(4 + len(shown), max(0, width - 2)))
            except Exception:
                pass
            screen.refresh()
            key = screen.get_wch()
            if key in ('\n', '\r') or key == curses.KEY_ENTER:
                if value or allow_empty:
                    return value
                continue
            if key == '\x1b' or key == 27:
                return None
            if key in ('\x08', '\x7f') or key in (curses.KEY_BACKSPACE, 127, 8):
                value = value[:-1]
            elif key == '\x15':  # Ctrl+U
                value = ''
            elif key == '\x17':  # Ctrl+W
                value = value.rstrip()
                value = value[:value.rfind(' ') + 1] if ' ' in value else ''
            elif isinstance(key, str) and key.isprintable() and len(value) < 4096:
                value += key
    finally:
        _set_cursor_visible(False)


def _message_dialog(screen, title, message, error=False):
    """显示明确的成功/错误对话框。"""
    import curses

    screen.erase()
    height, width = screen.getmaxyx()
    color = curses.color_pair(3 if error else 2) | curses.A_BOLD
    _safe_addstr(screen, 1, 2, title, color)
    line_width = max(20, width - 6)
    wrapped = []
    for paragraph in str(message).splitlines() or ['']:
        wrapped.extend(textwrap.wrap(paragraph, line_width) or [''])
    for index, line in enumerate(wrapped[:max(1, height - 6)]):
        _safe_addstr(screen, 3 + index, 2, line)
    _safe_addstr(screen, height - 2, 2, '按回车或 Esc 返回', curses.A_DIM)
    screen.refresh()
    while True:
        key = screen.get_wch()
        if key in ('\n', '\r', '\x1b') or key in (curses.KEY_ENTER, 27):
            return


def _midi_entries(directory):
    """列出目录与 MIDI 文件，供 TUI 文件选择器使用。"""
    dirs = []
    files = []
    with os.scandir(directory) as entries:
        for entry in entries:
            try:
                if entry.is_dir():
                    dirs.append((entry.name + '/', entry.path, True))
                elif entry.is_file() and entry.name.lower().endswith(('.mid', '.midi')):
                    files.append((entry.name, entry.path, False))
            except OSError:
                continue
    dirs.sort(key=lambda item: item[0].casefold())
    files.sort(key=lambda item: item[0].casefold())
    parent = os.path.dirname(directory)
    result = []
    if parent != directory:
        result.append(('../  [上级目录]', parent, True))
    return result + dirs + files


def _pick_midi_file(screen, initial=''):
    """只显示目录和 .mid/.midi 的简洁文件选择器。"""
    import curses

    expanded = os.path.abspath(os.path.expanduser(initial)) if initial else os.getcwd()
    if os.path.isdir(expanded):
        directory = expanded
    else:
        directory = os.path.dirname(expanded)
        if not os.path.isdir(directory):
            directory = os.getcwd()
    selected = 0
    scroll = 0
    notice = ''
    while True:
        try:
            entries = _midi_entries(directory)
        except OSError as exc:
            notice = f'无法读取目录: {exc}'
            parent = os.path.dirname(directory)
            if parent == directory:
                entries = []
            else:
                directory = parent
                selected = scroll = 0
                continue
        selected = max(0, min(selected, max(0, len(entries) - 1)))
        screen.erase()
        height, width = screen.getmaxyx()
        _safe_addstr(screen, 0, 2, '选择 MIDI 文件', curses.A_BOLD | curses.color_pair(1))
        _safe_addstr(screen, 1, 2, directory, curses.A_DIM)
        available = max(1, height - 6)
        if selected < scroll:
            scroll = selected
        if selected >= scroll + available:
            scroll = selected - available + 1
        visible = entries[scroll:scroll + available]
        if not visible:
            _safe_addstr(screen, 3, 4, '（本目录没有 MIDI 文件）', curses.A_DIM)
        for row, (label, _path, is_dir) in enumerate(visible, start=3):
            index = scroll + row - 3
            attr = curses.A_REVERSE if index == selected else 0
            if is_dir:
                attr |= curses.color_pair(1)
            _safe_addstr(screen, row, 3, label, attr)
        if notice:
            _safe_addstr(screen, height - 3, 2, notice, curses.color_pair(3))
        _safe_addstr(screen, height - 2, 2, '↑↓ 选择  Enter 打开/确认  M 手动输入  Backspace 上级  Esc 取消', curses.A_DIM)
        screen.refresh()
        key = screen.get_wch()
        if key in ('\x1b', 'q', 'Q') or key == 27:
            return None
        if key in (curses.KEY_UP, 'k', 'K') and entries:
            selected = (selected - 1) % len(entries)
        elif key in (curses.KEY_DOWN, 'j', 'J') and entries:
            selected = (selected + 1) % len(entries)
        elif key in ('\x08', '\x7f') or key in (curses.KEY_BACKSPACE, 127, 8):
            parent = os.path.dirname(directory)
            if parent != directory:
                directory = parent
                selected = scroll = 0
        elif key in ('m', 'M'):
            manual = _prompt_text(screen, '输入 MIDI 文件路径', initial)
            if manual is not None:
                path = os.path.abspath(os.path.expanduser(manual.strip()))
                if os.path.isfile(path):
                    return path
                notice = f'找不到文件: {path}'
        elif (key in ('\n', '\r') or key == curses.KEY_ENTER) and entries:
            _label, path, is_dir = entries[selected]
            if is_dir:
                directory = os.path.abspath(path)
                selected = scroll = 0
            else:
                return os.path.abspath(path)


def _tui_field_value(state, name):
    value = getattr(state, name)
    if isinstance(value, bool):
        return '是' if value else '否'
    if name == 'output' and not value:
        return '（自动：MIDI 同名 .txt）'
    if name in ('bpm', 'track', 'max_tracks') and not value:
        return '（自动）'
    return str(value)


def _draw_tui(screen, state, page, selected, scroll, status='', status_error=False):
    import curses

    screen.erase()
    height, width = screen.getmaxyx()
    if height < 15 or width < 55:
        _safe_addstr(screen, 1, 2, '终端窗口太小', curses.A_BOLD | curses.color_pair(3))
        _safe_addstr(screen, 3, 2, '请调整到至少 55 列 × 15 行；按 Q 退出。')
        screen.refresh()
        return scroll

    _safe_addstr(screen, 0, 2, 'music_rust  MIDI → TXT 转换器', curses.A_BOLD | curses.color_pair(1))
    settings_tab = '[ 转换设置 ]' if page == 0 else '  转换设置  '
    switches_tab = '[ 定时切换 ]' if page == 1 else '  定时切换  '
    _safe_addstr(screen, 1, 2, f'{settings_tab}    {switches_tab}', curses.A_REVERSE if page == 0 else 0)

    available = max(1, height - 7)
    if page == 0:
        selected = max(0, min(selected, len(TUI_FIELDS) - 1))
        if selected < scroll:
            scroll = selected
        if selected >= scroll + available:
            scroll = selected - available + 1
        for row, (name, label, _kind, _help) in enumerate(TUI_FIELDS[scroll:scroll + available], start=3):
            index = scroll + row - 3
            attr = curses.A_REVERSE if index == selected else 0
            if state.format not in ('v3.1', 'v3.2') and name in ('initial_soundfont', 'instrument'):
                attr |= curses.A_DIM
            if state.format != 'v3.2' and name in ('embed_image', 'image_path', 'image_compression', 'image_level'):
                attr |= curses.A_DIM
            _safe_addstr(screen, row, 2, f"{'>' if index == selected else ' '} {label:<16} {_tui_field_value(state, name)}", attr)
        _name, _label, _kind, help_text = TUI_FIELDS[selected]
        _safe_addstr(screen, height - 4, 2, help_text, curses.A_DIM)
        controls = 'Tab 切换页  ↑↓ 选择  Enter 修改  P 浏览 MIDI  F5/C 开始转换  Q 退出'
    else:
        _safe_addstr(screen, 3, 2, f'已设置 {len(state.switches)}/{MAX_PROGRAM_SWITCHES} 条（v3.1/v3.2 保存）', curses.A_BOLD)
        list_rows = max(1, available - 2)
        if state.switches:
            selected = max(0, min(selected, len(state.switches) - 1))
            if selected < scroll:
                scroll = selected
            if selected >= scroll + list_rows:
                scroll = selected - list_rows + 1
            for row, (at_ms, soundfont, instrument) in enumerate(state.switches[scroll:scroll + list_rows], start=5):
                index = scroll + row - 5
                attr = curses.A_REVERSE if index == selected else 0
                text = f"{'>' if index == selected else ' '} #{index + 1:02d}  {_format_switch_seconds(at_ms):>10} 秒   SF {soundfont}   GM {instrument}"
                _safe_addstr(screen, row, 2, text, attr)
        else:
            _safe_addstr(screen, 5, 4, '暂无定时切换；按 A 添加。', curses.A_DIM)
        _safe_addstr(screen, height - 4, 2, '输入格式：秒:音色（沿用初始 SF）或 秒:SF编号:音色', curses.A_DIM)
        controls = 'Tab 切换页  ↑↓ 选择  A 添加  Enter/E 编辑  D/Delete 删除  F5/C 转换  Q 退出'
    _safe_addstr(screen, height - 3, 2, controls, curses.A_DIM)
    if status:
        attr = curses.color_pair(3 if status_error else 2) | curses.A_BOLD
        _safe_addstr(screen, height - 2, 2, status, attr)
    else:
        _safe_addstr(screen, height - 2, 2, '无参数启动的独立界面，不会修改主 TUI 设置。', curses.A_DIM)
    screen.refresh()
    return scroll


def _edit_tui_field(screen, state, field_index):
    """编辑当前表单字段，返回状态提示。"""
    name, label, kind, _help = TUI_FIELDS[field_index]
    if kind == 'bool':
        setattr(state, name, not getattr(state, name))
        return f'{label}已设为 {_tui_field_value(state, name)}'
    if kind == 'choice':
        choices = IMAGE_ENCODINGS
        index = choices.index(getattr(state, name))
        setattr(state, name, choices[(index + 1) % len(choices)])
        return f'{label}已设为 {_tui_field_value(state, name)}'
    if kind == 'format':
        index = TUI_FORMATS.index(state.format)
        state.format = TUI_FORMATS[(index + 1) % len(TUI_FORMATS)]
        if state.format not in ('v3.1', 'v3.2') and (
            state.initial_soundfont != '1' or state.instrument != '0' or state.switches
        ):
            return f'已选 {state.format}；该格式不能保存非默认音色配置'
        return f'输出格式已设为 {state.format}'
    value = _prompt_text(screen, f'输入 {label}', getattr(state, name))
    if value is not None:
        setattr(state, name, value.strip())
        return f'{label}已更新'
    return ''


def _switch_prompt(screen, state, initial=''):
    """输入并校验一条定时切换。"""
    value = _prompt_text(
        screen,
        '定时切换：秒:音色 或 秒:SF编号:音色',
        initial,
        allow_empty=False,
    )
    if value is None:
        return None
    try:
        initial_sf = _parse_tui_int(state.initial_soundfont, '初始 SF 编号', 1, 3)
        return _parse_switches([value.strip()], initial_sf)[0]
    except ValueError as exc:
        _message_dialog(screen, '切换设置无效', str(exc), error=True)
        return None


def _convert_from_tui(screen, state):
    """执行转换，所有成功与错误都在 curses 界面内显示。"""
    import curses

    try:
        midi_path, output_path, opts = _build_tui_job(state)
        screen.erase()
        _safe_addstr(screen, 2, 2, '正在转换，请稍候…', curses.A_BOLD | curses.color_pair(1))
        _safe_addstr(screen, 4, 2, midi_path, curses.A_DIM)
        screen.refresh()
        count = convert(midi_path, output_path, opts)
        state.output = output_path
    except (MidiError, ValueError, OSError, struct.error) as exc:
        _message_dialog(screen, '转换失败', str(exc), error=True)
        return False, f'转换失败: {exc}'
    _message_dialog(
        screen,
        '转换完成',
        f'已输出: {output_path}\n音轨数: {count}\n格式: {state.format}',
        error=False,
    )
    return True, f'转换完成: {output_path}'


def _tui_main(screen):
    """curses.wrapper 内部主循环。"""
    import curses

    screen.keypad(True)
    _set_cursor_visible(False)
    try:
        curses.start_color()
        curses.use_default_colors()
        curses.init_pair(1, curses.COLOR_CYAN, -1)
        curses.init_pair(2, curses.COLOR_GREEN, -1)
        curses.init_pair(3, curses.COLOR_RED, -1)
    except curses.error:
        pass

    state = ConverterTuiState()
    page = 0
    setting_selected = 0
    switch_selected = 0
    setting_scroll = 0
    switch_scroll = 0
    status = ''
    status_error = False

    while True:
        selected = setting_selected if page == 0 else switch_selected
        scroll = setting_scroll if page == 0 else switch_scroll
        scroll = _draw_tui(screen, state, page, selected, scroll, status, status_error)
        if page == 0:
            setting_scroll = scroll
        else:
            switch_scroll = scroll
        try:
            key = screen.get_wch()
        except curses.error:
            continue

        if key in ('q', 'Q'):
            return 0
        if key == '\t':
            page = 1 - page
            status = ''
            status_error = False
            continue
        if key in (curses.KEY_F5, 'c', 'C'):
            success, status = _convert_from_tui(screen, state)
            status_error = not success
            continue

        if page == 0:
            if key in (curses.KEY_UP, 'k', 'K'):
                setting_selected = (setting_selected - 1) % len(TUI_FIELDS)
            elif key in (curses.KEY_DOWN, 'j', 'J'):
                setting_selected = (setting_selected + 1) % len(TUI_FIELDS)
            elif key in (curses.KEY_LEFT, curses.KEY_RIGHT):
                name, _label, kind, _help = TUI_FIELDS[setting_selected]
                if kind == 'bool':
                    setattr(state, name, not getattr(state, name))
                    status = f'{_label}已设为 {_tui_field_value(state, name)}'
                    status_error = False
                elif kind == 'format':
                    direction = -1 if key == curses.KEY_LEFT else 1
                    index = TUI_FORMATS.index(state.format)
                    state.format = TUI_FORMATS[(index + direction) % len(TUI_FORMATS)]
                    status = f'输出格式已设为 {state.format}'
                    status_error = False
                elif kind == 'choice':
                    index = IMAGE_ENCODINGS.index(getattr(state, name))
                    direction = -1 if key == curses.KEY_LEFT else 1
                    setattr(state, name, IMAGE_ENCODINGS[(index + direction) % len(IMAGE_ENCODINGS)])
                    status = f'{_label}已设为 {_tui_field_value(state, name)}'
                    status_error = False
            elif key in ('p', 'P'):
                chosen = _pick_midi_file(screen, state.midi)
                if chosen:
                    state.midi = chosen
                    status = f'已选择: {chosen}'
                    status_error = False
            elif key in ('\n', '\r') or key == curses.KEY_ENTER:
                status = _edit_tui_field(screen, state, setting_selected)
                status_error = bool(
                    status and state.format not in ('v3.1', 'v3.2') and '不能保存' in status
                )
        else:
            if key in (curses.KEY_UP, 'k', 'K') and state.switches:
                switch_selected = (switch_selected - 1) % len(state.switches)
            elif key in (curses.KEY_DOWN, 'j', 'J') and state.switches:
                switch_selected = (switch_selected + 1) % len(state.switches)
            elif key in ('a', 'A'):
                if len(state.switches) >= MAX_PROGRAM_SWITCHES:
                    status = f'最多只能设置 {MAX_PROGRAM_SWITCHES} 条切换'
                    status_error = True
                    continue
                rule = _switch_prompt(screen, state)
                if rule is not None:
                    state.switches.append(rule)
                    state.switches.sort(key=lambda item: item[0])
                    switch_selected = max(
                        index for index, item in enumerate(state.switches) if item == rule
                    )
                    status = '已添加定时切换'
                    status_error = False
            elif (key in ('e', 'E', '\n', '\r') or key == curses.KEY_ENTER) and state.switches:
                old_rule = state.switches[switch_selected]
                initial = ':'.join((
                    _format_switch_seconds(old_rule[0]),
                    str(old_rule[1]),
                    str(old_rule[2]),
                ))
                rule = _switch_prompt(screen, state, initial)
                if rule is not None:
                    state.switches[switch_selected] = rule
                    state.switches.sort(key=lambda item: item[0])
                    switch_selected = next(
                        index for index, item in enumerate(state.switches) if item == rule
                    )
                    status = '已更新定时切换'
                    status_error = False
            elif (key in ('d', 'D') or key in (curses.KEY_DC, 127)) and state.switches:
                del state.switches[switch_selected]
                switch_selected = min(switch_selected, max(0, len(state.switches) - 1))
                status = '已删除定时切换'
                status_error = False


def run_tui():
    """启动无参数独立 TUI；不可交互时返回友好错误。"""
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        print(
            '错误: 无参数模式需要可交互终端。'
            '请在终端中运行，或传入 MIDI 文件使用命令行模式。',
            file=sys.stderr,
        )
        return 2
    try:
        import curses
        return curses.wrapper(_tui_main)
    except KeyboardInterrupt:
        return 130
    except Exception as exc:
        # `_curses.error` 在某些 Python 版本不方便于导入前引用；
        # wrapper 中的转换错误已就地处理，到这里的通常是 curses/TERM 初始化失败。
        print(f'错误: 无法启动转换界面: {exc}', file=sys.stderr)
        return 2


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    if not argv:
        return run_tui()

    parser = argparse.ArgumentParser(
        description='MIDI → 简谱TXT 转换器 (music_rust 配套)',
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument('midi', help='输入 MIDI 文件')
    parser.add_argument('-o', '--output', help='输出 TXT 文件 (默认同目录同名)')
    parser.add_argument(
        '-b', '--bpm', type=_bounded_int('BPM', 1, 60_000),
        help='覆盖速度 (BPM，1..60000)',
    )
    parser.add_argument('--track', help='只转换指定音轨 (从0开始, 逗号分隔)')
    parser.add_argument('--quantize', type=int, default=0, help='量化粒度 tick (默认 0=关闭)')
    parser.add_argument('--max-tracks', type=int, help='最多转换音轨数')
    drum_group = parser.add_mutually_exclusive_group()
    drum_group.add_argument('--drum', dest='drum', action='store_true', help='包含打击乐轨')
    drum_group.add_argument('--no-drum', dest='drum', action='store_false', help='排除打击乐轨（默认）')
    parser.set_defaults(drum=False)
    parser.add_argument('--min-vel', type=int, default=0, help='最低力度')
    parser.add_argument('--velocity-scale', type=float, help='MIDI 力度倍率；交互终端未指定时询问（默认 1.0）')
    parser.add_argument('--keep-empty', action='store_true', help='保留空白音轨')
    parser.add_argument('--no-chord', action='store_true', help='不合并和弦')
    parser.add_argument(
        '--initial-soundfont', type=_bounded_int('SoundFont 编号', 1, MAX_SOUNDFONTS),
        default=1, metavar='1..3', help='初始 SoundFont 编号（默认 1）',
    )
    parser.add_argument(
        '--instrument', type=_bounded_int('音色号', 0, 127), default=0,
        metavar='0..127', help='初始 GM 音色号（默认 0）',
    )
    parser.add_argument(
        '--switch', action='append', default=[], metavar='秒[:SF编号]:音色',
        help='定时切换音色，格式为 秒:音色 或 秒:SF编号:音色（最多 24 条）',
    )
    parser.add_argument(
        '--embed-image', '--image', dest='image_path', metavar='图片',
        help='将图片二进制内嵌到 TXT v3.2（默认不内嵌）',
    )
    parser.add_argument(
        '--image-compression', choices=IMAGE_ENCODINGS, default='raw',
        help='图片二进制编码：raw/zstd/gzip/zlib/deflate/bzip2/xz/lz4（默认 raw）',
    )
    parser.add_argument(
        '--image-level', type=_bounded_int('压缩级别', 0, 22), default=3,
        metavar='0..22',
        help='压缩级别（默认 3；zstd 1..22，gzip/zlib/deflate/xz 0..9，bzip2 1..9，lz4 1..12）',
    )
    parser.add_argument('--v2', action='store_true', help='输出旧版可读多音轨 T 格式（默认输出 v3.2）')
    parser.add_argument('--legacy', action='store_true', help='输出最旧版两行一组格式')
    args = parser.parse_args(argv)

    try:
        args.program_switches = _parse_switches(args.switch, args.initial_soundfont)
    except ValueError as e:
        parser.error(str(e))

    has_nondefault_instrument = (
        args.initial_soundfont != 1 or args.instrument != 0 or bool(args.program_switches)
    )
    if (args.v2 or args.legacy) and has_nondefault_instrument:
        parser.error('--v2/--legacy 无法保存音色配置；请使用默认 v3.2 格式')

    if not os.path.isfile(args.midi):
        print(f"错误: 找不到文件 {args.midi}", file=sys.stderr)
        return 1

    out = args.output
    if not out:
        base = os.path.splitext(args.midi)[0]
        out = base + '.txt'

    args.track_list = None
    if args.image_path and (args.v2 or args.legacy):
        parser.error('--embed-image 只能用于默认 v3.2 格式')
    if args.track:
        try:
            args.track_list = [int(x) for x in args.track.split(',')]
        except ValueError:
            print("错误: --track 参数格式应为逗号分隔的数字", file=sys.stderr)
            return 1

    try:
        count = convert(args.midi, out, args)
    except (MidiError, ValueError, OSError, struct.error) as e:
        print(f"错误: {e}", file=sys.stderr)
        return 1

    print(f"转换完成: {args.midi} → {out}")
    print(f"音轨数: {count}")
    if args.bpm:
        print(f"速度: {args.bpm} BPM")
    print(f"力度倍率: {args.velocity_scale:g}")
    if not args.v2 and not args.legacy:
        print("TXT 格式: music_rust v3.2")
        print(
            f"初始音色: SoundFont {args.initial_soundfont}, "
            f"GM {args.instrument}"
        )
        print(f"定时切换: {len(args.program_switches)} 条")
        if args.image_path:
            print(f"内嵌图片: {args.image_path} ({args.image_compression}, 级别 {args.image_level})")
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
