// music_rust —— 钢琴演奏器
// Copyright (C) 2026 FuturePioneer-3
// Project: https://github.com/FuturePioneer-3
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.
// SPDX-License-Identifier: GPL-3.0-or-later

//! 简谱(TXT)解析器 —— 改进版：支持多音轨
//!
//! ## 格式说明
//!
//! 第一行：速度。两种形式：
//!   - 纯数字 `500`：一个四分音符的毫秒数（向后兼容 v1/v2）
//!   - `BPM 120`：直接用 BPM（每分钟拍数）
//!
//! 之后：
//!   - 以 `#` 开头的行是元指令（速度切换、注释）
//!   - 以 `T` 开头的行开启一个音轨，格式：
//!       `T 轨名 | 音符集1 | 音符集2 | ...`  —— 竖线 `|` 是该轨内的“段落”分隔，
//!       竖线同时会被解析器忽略（兼容原格式的小节线含义）。
//!   - 其它行视为一个“时刻组”：按空格/制表符切分后，每个空白分隔的 Token
//!     属于一个音轨。若同一行有多个 Token，则它们同时播放（多音轨同时发声）。
//!   - 以空行分隔的连续行组 = 按顺序播放（纵向多轨并行，横向上一个组播完再播下一组）。
//!
//! 完全兼容原 v1/v2 格式：两行一组 = 左右手两轨同时播放。
//!
//! ## 音符集语法（单个 Token 内部）
//!
//! - `1~7` 数字音符；`0` 休止符
//! - `,` 低音（最多 3 个）；`^` 高音（最多 4 个）
//! - `#` 升半音
//! - `-` 延音（每多一个多 1/4 音符时长）；`_` 分音（时长减半）
//! - `*` 三分音符；`%` 五分之一；`&` 七分之一（v1.2 扩展）
//! - `.` 附点（时长 ×1.5）
//! - `[]`/`{}` 和弦；`|` 小节线（忽略）
//!
//! 一个“事件” = 一个音符或和弦，从空格处切分。每个事件按顺序推进时间线。

use crate::log::{debug, info, warn};

/// 一条排程事件
#[derive(Debug, Clone)]
pub struct ScheduledNote {
    /// 绝对时间（毫秒）
    pub at_ms: u32,
    /// MIDI 音高 (0-127)
    pub key: u8,
    /// 力度 (1-127)
    pub vel: u8,
    /// 是否 note-on
    pub on: bool,
    /// 通道 (0-15)
    pub channel: u8,
}

/// 一个音轨的所有事件（已排序，note-on 先于同时间 note-off）
#[derive(Debug, Clone, Default)]
pub struct Track {
    pub name: String,
    pub channel: u8,
    pub events: Vec<ScheduledNote>,
}

/// 解析结果
#[derive(Debug, Default)]
pub struct Score {
    pub tempo_ms: u32,
    pub tracks: Vec<Track>,
    pub title: String,
    pub total_ms: u32,
}

// ---------------------------------------------------------------------------
// 简谱词法/语义解析
// ---------------------------------------------------------------------------

/// 简谱数字(1-7) → 相对 C 的 MIDI 偏移
/// 以中央 C4=60 为基准，数字 1 对应 C。
const DEGREE_OFFSET: [i8; 7] = [0, 2, 4, 5, 7, 9, 11];

const MAX_LOW: i8 = 3;
const MAX_HIGH: i8 = 4;

/// 解析一个“音符 token”字符串（可能含延/分/附点，也可能是一个和弦）。
/// 返回该事件包含的音符 MIDI 键（不含休止）与时长（毫秒）。
fn parse_token(token: &str, tempo_ms: u32) -> (Vec<u8>, u32) {
    // 基础时值：1/4 音符 = tempo_ms（以四分音符为基本单位）
    // 内部使用“时间基数” ctn：初始为 1 个四分音符（=tempo_ms）。
    // 处理顺序按字符流从左到右：延/分音先累计，最后在空格处结算。
    let base = tempo_ms as i64;
    let mut ctn: i64 = base;
    let mut notes: Vec<u8> = Vec::new();
    let mut is_chord = false;
    let mut chord_notes: Vec<u8> = Vec::new();

    let bytes: Vec<char> = token.chars().collect();
    let n = bytes.len();
    let mut i = 0;

    while i < n {
        let c = bytes[i];
        match c {
            '[' | '{' => {
                is_chord = true;
                chord_notes.clear();
                i += 1;
            }
            ']' | '}' => {
                is_chord = false;
                if !chord_notes.is_empty() {
                    notes.extend(&chord_notes);
                    chord_notes.clear();
                }
                i += 1;
            }
            '|' => {
                i += 1; // 小节线忽略
            }
            ' ' | '\t' => {
                i += 1; // 空白（不应出现，防御）
            }
            // 时值修饰（作用于整个事件）
            '_' => {
                ctn /= 2;
                i += 1;
            }
            '*' => {
                ctn = ctn * 1 / 3;
                i += 1;
            }
            '%' => {
                ctn = ctn * 1 / 5;
                i += 1;
            }
            '&' => {
                ctn = ctn * 1 / 7;
                i += 1;
            }
            '.' => {
                // 附点：每次把当前累计的 ctn 追加一半。
                // 与原实现一致：'.' 处理在 '-' 之后等，从右往左累计会影响。
                // 原实现：'.' ctn*=1.5；为保持与原版一致采用乘法。
                ctn = ctn * 3 / 2;
                i += 1;
            }
            '-' => {
                ctn += base;
                i += 1;
            }
            '0' => {
                // 休止符：占一个事件的时长，但不发音
                // 处理完休止符后继续读时值修饰
                i += 1;
                // 休止符本身占用一个 ctn 的时长；后续修饰词继续累计
                // 由于和弦模式内不会出现 0，直接作为单音处理
                // 注意：这里 0 已经隐含“一个基本时值”，与其它数字一致
                let _ = notes; // 不添加
                // 但 0 要占时长：ctn 已在初始时视为基本时值
            }
            c if c.is_ascii_digit() && (c as u8) >= b'1' && (c as u8) <= b'7' => {
                let degree = (c as u8 - b'1') as usize;
                let mut lvl: i8 = 0; // 相对基准八度，0=中央C4
                let mut sharp = false;
                i += 1;
                // 读取紧随其后的 高音^/低音,/升音#
                while i < n {
                    match bytes[i] {
                        '^' => {
                            lvl += 1;
                            i += 1;
                        }
                        ',' => {
                            lvl -= 1;
                            i += 1;
                        }
                        '#' => {
                            sharp = true;
                            i += 1;
                        }
                        _ => break,
                    }
                }
                // 校验八度范围
                if lvl > MAX_HIGH {
                    warn(format!(
                        "音符 {:?} 八度过高 (lvl={})，已截断",
                        token, lvl
                    ));
                    lvl = MAX_HIGH;
                }
                if lvl < -MAX_LOW {
                    warn(format!(
                        "音符 {:?} 八度过低 (lvl={})，已截断",
                        token, lvl
                    ));
                    lvl = -MAX_LOW;
                }
                // MIDI = 60 (C4) + 12*lvl + 音级偏移 (+1 升音)
                let mut midi = 60 + 12 * (lvl as i32) + DEGREE_OFFSET[degree] as i32;
                if sharp {
                    midi += 1;
                }
                if midi < 0 {
                    midi = 0;
                }
                if midi > 127 {
                    midi = 127;
                }
                let note = midi as u8;
                if is_chord {
                    chord_notes.push(note);
                } else {
                    notes.push(note);
                }
            }
            _ => {
                // 未知字符：忽略（防御）
                warn(format!("忽略未知字符 {:?}", c));
                i += 1;
            }
        }
    }

    if !chord_notes.is_empty() {
        notes.extend(&chord_notes);
    }

    // 时值至少 1ms
    let dur = ctn.max(1) as u32;
    (notes, dur)
}

/// 解析一行（可含空格分隔的多个 token）。返回多个 (notes, dur) 事件序列。
/// 每个 token 之间，时值相互独立累加（类似原版：空格处执行一次播放并推进）。
/// 返回每个事件的 (音符列表, 起始推进时长, 实际时长)。
fn parse_line(line: &str, tempo_ms: u32) -> Vec<(Vec<u8>, u32)> {
    // 先按空格/制表符切成 token；但和弦里的空格不该切。和弦内用 [ ] 包裹一般不含空格。
    // 简单起见按空白切分，然后丢弃空 token。
    let tokens: Vec<&str> = line
        .split_whitespace()
        .filter(|t| t.chars().any(|c| c != '|')) // 过滤纯小节线 token
        .collect();
    let mut out = Vec::new();
    for t in &tokens {
        let (notes, dur) = parse_token(t, tempo_ms);
        if notes.is_empty() {
            // 纯休止/空 token：仍要占时长（休止符），但 notes 为空则无法发声，
            // 保留 dur 以便推进时间线
            out.push((vec![], dur));
        } else {
            out.push((notes, dur));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 多音轨文本结构解析
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum LineKind {
    Tempo,
    Comment,
    TrackHeader,
    NoteLine,
    Blank,
}

fn classify_line(line: &str) -> LineKind {
    let t = line.trim();
    if t.is_empty() {
        return LineKind::Blank;
    }
    if let Some(rest) = t.strip_prefix('#') {
        let r = rest.trim();
        if r.starts_with("BPM") || r.starts_with("bpm") {
            LineKind::Tempo
        } else {
            LineKind::Comment
        }
    } else if t.starts_with('T') {
        // 可能的音轨头：T 轨名 或 T | 音符 | ...
        LineKind::TrackHeader
    } else if t.chars().all(|c| c.is_ascii_digit()) {
        // 纯数字行：向后兼容速度行（仅当出现在文件头部；中间出现当作注释速度切换）
        LineKind::Tempo
    } else {
        LineKind::NoteLine
    }
}

/// 从文件中读取并解析。`force_tempo` 可覆盖第一行速度。
pub fn parse_file(path: &str, force_tempo: Option<u32>) -> Result<Score, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("无法读取 {}: {}", path, e))?;
    parse_str(&content, force_tempo)
}

/// 解析字符串内容。
pub fn parse_str(content: &str, force_tempo: Option<u32>) -> Result<Score, String> {
    let mut tempo_ms = force_tempo.unwrap_or(500);
    let mut title = String::new();

    // 第一遍：扫描行分类，得到结构
    let lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();

    // 预处理：判断是否新格式（含 T 轨头），决定解析模式
    let has_track_header = lines
        .iter()
        .any(|l| classify_line(l) == LineKind::TrackHeader);

    // --- 解析第一行速度（若存在，且未强制覆盖） ---
    let mut consumed = 0;
    for l in &lines {
        match classify_line(l) {
            LineKind::Tempo => {
                if force_tempo.is_none() {
                    let t = l.trim();
                    if let Some(rest) = t.strip_prefix('#') {
                        // #BPM 120
                        let num: String = rest
                            .chars()
                            .filter(|c| c.is_ascii_digit())
                            .collect();
                        if let Ok(bpm) = num.parse::<u32>() {
                            if bpm > 0 {
                                tempo_ms = (60_000 / bpm).max(1);
                            }
                        }
                    } else {
                        // 纯数字
                        if let Ok(ms) = t.parse::<u32>() {
                            if ms > 0 {
                                tempo_ms = ms;
                            }
                        }
                    }
                } else {
                    debug(format!(
                        "命令行强制速度生效: {}ms/四分音符",
                        tempo_ms
                    ));
                }
                consumed += 1;
                break; // 只消费第一条
            }
            LineKind::Blank | LineKind::Comment => {
                // 跳过文件开头的空行/注释
                if l.trim().starts_with('#') {
                    if let Some(rest) = l.trim().strip_prefix("#") {
                        if let Some(n) = rest.trim().strip_prefix("TITLE ") {
                            title = n.trim().to_string();
                        } else if let Some(n) = rest.trim().strip_prefix("TITLE:") {
                            title = n.trim().to_string();
                        } else if let Some(n) = rest.trim().strip_prefix("Name ") {
                            title = n.trim().to_string();
                        }
                    }
                }
                consumed += 1;
            }
            _ => break,
        }
    }
    let body_start = consumed;

    debug(format!("速度: {}ms/四分音符 ({:.1} BPM)", tempo_ms, 60_000.0 / tempo_ms as f64));

    let mut score = Score {
        tempo_ms,
        title,
        tracks: Vec::new(),
        total_ms: 0,
    };

    if has_track_header {
        parse_new_format(&lines[body_start..], tempo_ms, &mut score)?;
    } else {
        parse_legacy_format(&lines[body_start..], tempo_ms, &mut score)?;
    }

    // 计算总时长（取所有 note-off 时间的最大值）
    let mut max_end = 0u32;
    for t in &score.tracks {
        for e in &t.events {
            if !e.on {
                max_end = max_end.max(e.at_ms);
            }
        }
    }
    score.total_ms = max_end;

    // 排序事件：先 note-off 后 note-on？不，同时间的 note-on 先发，note-off 后发。
    for t in &mut score.tracks {
        t.events.sort_by(|a, b| {
            a.at_ms
                .cmp(&b.at_ms)
                .then_with(|| a.on.cmp(&b.on)) // false(noteoff) 排后
        });
    }

    Ok(score)
}

/// 旧版格式：行与行按时间顺序推进；两行一组=左右手同时。
fn parse_legacy_format(
    lines: &[String],
    tempo_ms: u32,
    score: &mut Score,
) -> Result<(), String> {
    debug("旧版 v1/v2 格式：按两行一组（左右手）解析".to_string());

    let mut line_idx = 0usize;
    let mut t = 0i64; // 当前时间（毫秒）
    let mut cur_tempo = tempo_ms; // 当前速度（可变，支持中途变速）
    let mut track1 = Track::default();
    track1.name = "右手(R)".to_string();
    track1.channel = 0;
    let mut track2 = Track::default();
    track2.name = "左手(L)".to_string();
    track2.channel = 1;
    let mut groups_played = 0;

    while line_idx < lines.len() {
        let l = &lines[line_idx];
        let k = classify_line(l);
        match k {
            LineKind::Blank => {
                // 空行：强制打断多重旋律判定，但时间不推进
                line_idx += 1;
            }
            LineKind::Comment => {
                line_idx += 1;
            }
            LineKind::Tempo => {
                // 中途速度切换（v1/v2 兼容：纯数字 = 毫秒/四分音符；#BPM = BPM）
                let t = l.trim();
                if let Some(rest) = t.strip_prefix('#') {
                    let num: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
                    if let Ok(bpm) = num.parse::<u32>() {
                        if bpm > 0 {
                            let new_ms = (60_000 / bpm).max(1);
                            if new_ms != cur_tempo {
                                info(format!("旧版中途切换速度: {}ms/四分音符 ({} BPM)", new_ms, bpm));
                            }
                            cur_tempo = new_ms;
                        }
                    }
                } else if let Ok(ms) = t.parse::<u32>() {
                    if ms > 0 {
                        if ms != cur_tempo {
                            info(format!("旧版中途切换速度: {}ms/四分音符", ms));
                        }
                        cur_tempo = ms;
                    }
                }
                line_idx += 1;
            }
            LineKind::NoteLine => {
                // 主旋律行
                let line = l.trim();
                let events1 = parse_line(line, cur_tempo);
                // 尝试读第二行作为左手
                let mut events2: Vec<(Vec<u8>, u32)> = Vec::new();
                let mut consumed_second = false;
                if line_idx + 1 < lines.len() {
                    let next = &lines[line_idx + 1];
                    let nk = classify_line(next);
                    if nk == LineKind::NoteLine {
                        events2 = parse_line(next.trim(), cur_tempo);
                        consumed_second = true;
                    } else if nk == LineKind::Blank {
                        // 空行强制打断：不配对
                    }
                }

                // 处理配对
                let t_this = t as u32;
                push_events(&mut track1, &events1, t_this);
                push_events(&mut track2, &events2, t_this);

                // 推进时间：取两轨事件时值中的最大值？原版各自独立推进，两轨同时结束。
                // 这里两轨等长推进（各自内部累加），但若长度不同，短轨会提前结束。
                // 为保持同步，使用最长轨的推进长度，不足的补休止。
                let dur1: i64 = events1.iter().map(|(_, d)| *d as i64).sum();
                let dur2: i64 = events2.iter().map(|(_, d)| *d as i64).sum();
                t += dur1.max(dur2);
                groups_played += 1;

                if consumed_second {
                    line_idx += 2;
                } else {
                    line_idx += 1;
                }
            }
            LineKind::TrackHeader => {
                // 旧版模式中出现 T 头：忽略（异常）
                warn("旧版模式中出现 T 轨头，忽略".to_string());
                line_idx += 1;
            }
        }
    }

    if !track1.events.is_empty() {
        score.tracks.push(track1);
    }
    if !track2.events.is_empty() {
        score.tracks.push(track2);
    }

    debug(format!("旧版解析完成，共 {} 组 ({} 轨)", groups_played, score.tracks.len()));
    Ok(())
}

/// 新版格式：T 轨头定义多音轨；竖线分隔段落。
fn parse_new_format(
    lines: &[String],
    tempo_ms: u32,
    score: &mut Score,
) -> Result<(), String> {
    debug("新版多音轨格式：T 轨头模式".to_string());

    // 收集所有轨道定义
    struct TrackDef {
        name: String,
        channel: u8,
        raw: Vec<String>, // 该轨所有原始行（未含 T 头）
    }

    let mut defs: Vec<TrackDef> = Vec::new();
    let mut cur: Option<usize> = None;
    let mut idx = 0usize;

    while idx < lines.len() {
        let l = &lines[idx];
        let k = classify_line(l);
        match k {
            LineKind::TrackHeader => {
                // 格式: T 轨名 | 段1 | 段2 ...
                let rest = l.trim().trim_start_matches('T').trim();
                let parts: Vec<&str> = rest.splitn(2, '|').collect();
                let name = parts[0].trim().to_string();
                let remaining = if parts.len() > 1 {
                    parts[1].trim().to_string()
                } else {
                    String::new()
                };
                let channel = (defs.len() % 16) as u8;
                defs.push(TrackDef {
                    name,
                    channel,
                    raw: Vec::new(),
                });
                cur = Some(defs.len() - 1);
                if !remaining.is_empty() {
                    if let Some(c) = cur {
                        defs[c].raw.push(remaining);
                    }
                }
            }
            LineKind::NoteLine => {
                if let Some(c) = cur {
                    defs[c].raw.push(l.trim().to_string());
                } else {
                    warn("无音轨头就出现音符行，忽略".to_string());
                }
            }
            LineKind::Blank => {
                // 空行：不影响，跳过（竖线已分段）
                idx += 1;
                continue;
            }
            LineKind::Comment => {
                idx += 1;
                continue;
            }
            LineKind::Tempo => {
                // 全局中途速度切换：记录为 tempo 指令（作用于后续所有轨）
                // 用一个特殊标记行保存，推进时间线时识别
                if let Some(c) = cur {
                    defs[c].raw.push(l.trim().to_string());
                }
            }
        }
        idx += 1;
    }

    // 解析每个轨：raw 行之间按顺序推进（每行 = 一个段落；行内空白分隔 = 同时）
    // 但要注意：在 T 轨头模式里，同一行的多个 token 属于一个轨（各自累加），
    // 而不同 T 轨的行是并行的。这里约定：每个 T 轨内，行与行顺序播放。
    // 这样 T 轨可以表达“先后”，而跨轨同时。
    // 但更自然的用法：一个 T 轨 = 一条旋律线，多行 = 顺序。
    // 为了让“同时”也可表达，行内多个 token 本来就是同时的（原版语义：空格分隔即同时？不，
    // 原版语义：空格分隔是顺序！同一行内 token 顺序播放）
    //
    // 仔细想：原版一个“音符集”字符串如 "1 2 3 4 5 5 4 3 2 1"，空格分隔是顺序播放。
    // 而“两行一组”才是同时。
    // 在新版多音轨中，我们要表达“并行多轨”，因此把“T 轨”视为一个独立的时间线。
    // 每个 T 轨内部，token 按空格顺序播放（一条旋律线）。
    // 于是：不同的 T 轨 = 并行时间线，T 轨内 = 顺序。
    // 这就与 MIDI 转换器对齐：MIDI 每个 track 变成一条 T 轨，事件按 tick 排序，
    // 连续同时发声的音符写成和弦，或允许在同一条轨上并列。
    //
    // 构建 score.tracks
    for def in &defs {
        let mut track = Track {
            name: def.name.clone(),
            channel: def.channel,
            events: Vec::new(),
        };
        let mut t: i64 = 0;
        let mut cur_tempo = tempo_ms; // 该轨当前速度（支持 #BPM 中途变速）
        for raw in &def.raw {
            // 识别 tempo 指令行（#BPM xxx）
            if let Some(rest) = raw.trim().strip_prefix('#') {
                let r = rest.trim();
                if r.starts_with("BPM") || r.starts_with("bpm") {
                    let num: String = r.chars().filter(|c| c.is_ascii_digit()).collect();
                    if let Ok(bpm) = num.parse::<u32>() {
                        if bpm > 0 {
                            let new_ms = (60_000 / bpm).max(1);
                            if new_ms != cur_tempo {
                                info(format!("多轨中途变速: {}ms/四分音符 ({} BPM)", new_ms, bpm));
                            }
                            cur_tempo = new_ms;
                            continue;
                        }
                    }
                }
            }
            // 纯数字行 = 中途速度切换（毫秒/四分音符）
            if raw.trim().chars().all(|c| c.is_ascii_digit()) && !raw.trim().is_empty() {
                if let Ok(ms) = raw.trim().parse::<u32>() {
                    if ms > 0 {
                        if ms != cur_tempo {
                            info(format!("多轨中途变速: {}ms/四分音符", ms));
                        }
                        cur_tempo = ms;
                        continue;
                    }
                }
            }
            // 普通音符行
            let events = parse_line(raw, cur_tempo);
            push_events(&mut track, &events, t as u32);
            let dur: i64 = events.iter().map(|(_, d)| *d as i64).sum();
            t += dur;
        }
        if !track.events.is_empty() {
            score.tracks.push(track);
        }
    }

    info(format!(
        "新版解析完成：{} 个音轨",
        score.tracks.len()
    ));
    Ok(())
}

/// 将事件推入轨道（时间偏移 t 起，按序累加推进）
fn push_events(track: &mut Track, events: &[(Vec<u8>, u32)], start_t: u32) {
    let mut t = start_t as i64;
    for (notes, dur) in events {
        if notes.is_empty() {
            // 休止：只推进
            t += *dur as i64;
            continue;
        }
        for &note in notes {
            track.events.push(ScheduledNote {
                at_ms: t as u32,
                key: note,
                vel: 96,
                on: true,
                channel: track.channel,
            });
            track.events.push(ScheduledNote {
                at_ms: t as u32 + *dur,
                key: note,
                vel: 96,
                on: false,
                channel: track.channel,
            });
        }
        t += *dur as i64;
    }
}

/// 验证与报告
pub fn print_score_summary(score: &Score) {
    info(format!("乐曲标题: {}", score.title));
    info(format!(
        "速度: {}ms/四分音符 ({:.1} BPM)",
        score.tempo_ms,
        60_000.0 / score.tempo_ms as f64
    ));
    info(format!("音轨数: {}", score.tracks.len()));
    for t in &score.tracks {
        let ons = t.events.iter().filter(|e| e.on).count();
        info(format!(
            "  轨[{}] ch{} 音符数={} 事件数={}",
            t.name,
            t.channel,
            ons,
            t.events.len()
        ));
    }
    info(format!("总时长: {}ms ({:.1}s)", score.total_ms, score.total_ms as f64 / 1000.0));
}

/// 调试：打印前 n 条事件
pub fn print_first_events(score: &Score, n: usize) {
    debug(format!("前 {} 条排程事件:", n));
    let mut count = 0;
    'outer: for t in &score.tracks {
        for e in &t.events {
            if count >= n {
                break 'outer;
            }
            debug(format!(
                "  [ch{}] {:>8}ms {} key={:3} vel={}",
                e.channel,
                e.at_ms,
                if e.on { "note-on " } else { "note-off" },
                e.key,
                e.vel
            ));
            count += 1;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_chord_parse() {
        let (notes, dur) = parse_token("[1,3,5,]-", 500);
        println!("[1,3,5,]- notes={:?} dur={}", notes, dur);
        let (notes2, dur2) = parse_token("[4,6,1]-", 500);
        println!("[4,6,1]- notes={:?} dur={}", notes2, dur2);
        let events = parse_line("[1,3,5,]- [4,6,1]- [2,4#,6,]- [1,3,5,]-", 500);
        println!("parse_line events: {:?}", events);
    }
}
