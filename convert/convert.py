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

将标准 MIDI 文件 (.mid/.midi) 转换为 music_rust v3 绝对事件 TXT。

用法:
    python3 convert.py 输入.mid [-o 输出.txt] [选项]

选项:
    -o, --output <文件>   输出文件 (默认: 输入同名.txt)
    -b, --bpm <n>        覆盖速度 (BPM)
    --track <编号...>     只转换指定音轨 (从0开始, 逗号分隔)
    --quantize <tick>    量化粒度 (默认 0=关闭, 建议 30)
    --max-tracks <n>     最多转换多少音轨 (默认全部非打击乐)
    --drum / --no-drum    是否包含打击乐轨 (默认 --no-drum)
    --min-vel <n>        最低力度 (默认 0)
    --velocity-scale <x> 力度倍率；交互终端未指定时会询问 (默认 1.0)
    --keep-empty         保留空白音轨
    --no-chord           不合并和弦 (每个音单独token)
    --v2                 输出旧版可读多音轨 T 格式
    --legacy             输出最旧的 v1/v2 两行一组格式

MIDI 音符编号 → 简谱:
    简谱用 1~7 表示 do re mi fa sol la si
    低音用 ',' (最多3个), 高音用 '^' (最多4个), 升音用 '#'
    休止符用 0, 延音用 '-' (增加一个四分音符), 分音用 '_' (减半)
    附点用 '.' (×1.5), 和弦用 [ ] 包裹
"""

import argparse
import math
import os
import struct
import sys
from collections import defaultdict

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
    fmt, ntrks, division = struct.unpack('>HHH', data[8:14])

    # SMF type 0: 单轨; type 1: 多轨同步; type 2: 多轨独立
    if fmt > 2:
        raise MidiError(f"不支持的 SMF 类型: {fmt}")

    if division & 0x8000:
        # SMPTE 格式 (frames/sec * ticks/frame)
        fps = -((division >> 8) & 0xff)
        ticks_per_frame = division & 0xff
        division = fps * ticks_per_frame
        # 这种情况 tick 数不可直接用毫秒，这里按保守处理
        # 实际 SMPTE 中 division 负数表示 (fps 为负值)
        division = abs(division)

    pos = 14
    tracks = []
    for _ in range(ntrks):
        if pos + 8 > len(data):
            raise MidiError("轨道头截断")
        if data[pos:pos + 4] != b'MTrk':
            raise MidiError(f"缺少 MTrk 头 (位置 {pos})")
        tlen = struct.unpack('>I', data[pos + 4:pos + 8])[0]
        pos += 8
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
                try:
                    mlen, p = _read_varlen(tdata, p)
                except Exception:
                    break
                meta_data = tdata[p:p + mlen]
                p += mlen
                if mtype == 0x03:  # Track name
                    track.name = meta_data.decode('latin-1', errors='replace')
                elif mtype == 0x51:  # Tempo (us per quarter)
                    if mlen >= 3:
                        us = (meta_data[0] << 16) | (meta_data[1] << 8) | meta_data[2]
                        track.tempo_events.append((tick, us))
                elif mtype == 0x2f:  # End of track
                    break
                # 其它 meta 忽略
                running_status = None
            elif msg_type == 0xf0 or msg_type == 0xf7:  # SysEx
                # SysEx: 读取 varlen 长度
                try:
                    slen, p = _read_varlen(tdata, p)
                except Exception:
                    slen = 0
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
    for track in tracks:
        for tick, us in track.tempo_events:
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
        # 过滤打击乐
        if not opts.drum:
            # 检查是否打击乐轨 (大部分音符在 ch9)
            if t.events and all(ev.channel == PERCUSSION_CHANNEL for ev in t.events):
                continue
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
        _convert_v3(out_tracks, division, tempo_timeline, midi_path, lines)

    # 写入
    content = '\n'.join(lines)
    with open(out_path, 'w', encoding='utf-8') as f:
        f.write(content)
    return len(out_tracks)


def _quote_track_name(name):
    """v3 track 名称按整行保存，转义反斜杠和双引号。"""
    return '"' + name.replace('\\', '\\\\').replace('"', '\\"') + '"'


def _convert_v3(out_tracks, division, tempo_timeline, midi_path, lines):
    """输出 v3：绝对 tick 音符 + 单一全局 tempo 表。

    每个 MIDI 轨道保留原始索引，音符不再通过简谱 token 的顺序和休止符重建时间，
    因而不会出现轨道错位、短音符被拉长或变速点漂移。
    """
    lines.extend([
        "#MUSIC_RUST 3",
        f"#TITLE {os.path.splitext(os.path.basename(midi_path))[0]}",
        f"#PPQ {division}",
    ])
    for tick, us in tempo_timeline:
        lines.append(f"@TEMPO {tick} {us}")
    lines.append("")
    for source_idx, name, events in out_tracks:
        channel = events[0].channel if events else 0
        lines.append(f"@TRACK {source_idx} {channel} {_quote_track_name(name)}")
        for event in sorted(events, key=lambda e: (e.start, e.note, e.end or e.start)):
            if event.end is None or event.end <= event.start:
                continue
            lines.append(f"@NOTE {source_idx} {event.start} {event.end - event.start} {event.note} {event.velocity}")
        lines.append("")


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

def main():
    parser = argparse.ArgumentParser(
        description='MIDI → 简谱TXT 转换器 (music_rust 配套)',
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument('midi', help='输入 MIDI 文件')
    parser.add_argument('-o', '--output', help='输出 TXT 文件 (默认同目录同名)')
    parser.add_argument('-b', '--bpm', type=int, help='覆盖速度 (BPM)')
    parser.add_argument('--track', help='只转换指定音轨 (从0开始, 逗号分隔)')
    parser.add_argument('--quantize', type=int, default=0, help='量化粒度 tick (默认 0=关闭)')
    parser.add_argument('--max-tracks', type=int, help='最多转换音轨数')
    parser.add_argument('--drum', action='store_true', help='包含打击乐轨')
    parser.add_argument('--min-vel', type=int, default=0, help='最低力度')
    parser.add_argument('--velocity-scale', type=float, help='MIDI 力度倍率；交互终端未指定时询问（默认 1.0）')
    parser.add_argument('--keep-empty', action='store_true', help='保留空白音轨')
    parser.add_argument('--no-chord', action='store_true', help='不合并和弦')
    parser.add_argument('--v2', action='store_true', help='输出旧版可读多音轨 T 格式（默认输出 v3）')
    parser.add_argument('--legacy', action='store_true', help='输出最旧版两行一组格式')
    args = parser.parse_args()

    if not os.path.isfile(args.midi):
        print(f"错误: 找不到文件 {args.midi}", file=sys.stderr)
        sys.exit(1)

    out = args.output
    if not out:
        base = os.path.splitext(args.midi)[0]
        out = base + '.txt'

    args.track_list = None
    if args.track:
        try:
            args.track_list = [int(x) for x in args.track.split(',')]
        except ValueError:
            print("错误: --track 参数格式应为逗号分隔的数字", file=sys.stderr)
            sys.exit(1)

    try:
        count = convert(args.midi, out, args)
    except MidiError as e:
        print(f"错误: {e}", file=sys.stderr)
        sys.exit(1)
    except ValueError as e:
        print(f"错误: {e}", file=sys.stderr)
        sys.exit(1)

    print(f"转换完成: {args.midi} → {out}")
    print(f"音轨数: {count}")
    if args.bpm:
        print(f"速度: {args.bpm} BPM")
    print(f"力度倍率: {args.velocity_scale:g}")


if __name__ == '__main__':
    main()
