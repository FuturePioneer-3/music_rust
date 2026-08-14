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

//! 播放时使用的轻量终端界面（2.4.0 全面重绘）。
//!
//! 不依赖第三方终端库，避免增加运行时依赖；仅在 stdout 为终端时启用。
//!
//! 布局（自上而下，小屏自动精简）：
//!   顶框 → 标题 → 状态行 → 进度条 → 时间 → 音量 → 动态 EQ（可选）
//!   → 音符详情（可选）→ 专辑封面 + 作曲家等元数据（可选）→ 按键提示 → 底框
//!
//! 封面图使用半块字符（▀）＋真彩色渲染，宽高按终端尺寸自适应：
//! 大屏不超过 45% 高度 / 46 列，小屏最小保底 6 行，绝不与上方内容重叠。

use std::io::{self, Write};

use crate::input::Control;

/// 内嵌封面：RGBA8 像素（宽 × 高），由 C 侧解码并缩放到 ≤96px。
/// 定义在本模块（而非 audio_file），使 selftest 等通过 `#[path]`
/// 复用 tui.rs 的二进制无需链接 audio_file。
#[derive(Clone)]
pub struct ArtImage {
    pub data: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

// ---------------------------------------------------------------------------
// 调色板（真彩色；支持鼠标的现代终端均支持）
// ---------------------------------------------------------------------------
const CYAN: (u8, u8, u8) = (0, 215, 255);
const CYAN_DEEP: (u8, u8, u8) = (0, 170, 230);
const MAGENTA: (u8, u8, u8) = (255, 95, 215);
const GREEN: (u8, u8, u8) = (110, 255, 170);
const GREEN_DIM: (u8, u8, u8) = (80, 200, 120);
const YELLOW: (u8, u8, u8) = (255, 215, 0);
const RED: (u8, u8, u8) = (255, 95, 95);
const GRAY: (u8, u8, u8) = (140, 140, 150);
const GRAY_DIM: (u8, u8, u8) = (90, 90, 100);
const ART_BG: (u8, u8, u8) = (14, 14, 22); // 封面透明像素的衬底

/// EQ 柱状图颜色（自下而上 level 1..7）
const EQ_COLORS: [(u8, u8, u8); 7] = [
    (70, 100, 160),   // 1  深蓝
    (90, 130, 200),   // 2
    (0, 170, 230),    // 3  青
    (0, 215, 255),    // 4  亮青
    (110, 255, 170),  // 5  绿
    (255, 215, 0),    // 6  黄
    (255, 95, 95),    // 7  红
];

/// 进度条 1/8 精度分块字符
const PARTIAL_BLOCKS: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";

/// 进度条行的内容布局（见 draw()）："│ " + " ▸ " + 进度条 + " 45.2%" + " │"。
/// "│ " 前缀 2 列 + " ▸ " 3 列 → 进度条首个字符位于 1-based 第 6 列。
/// SGR 鼠标坐标（ESC[<0;x;yM）的 x 同样为 1-based，点击映射必须以该列为基准，
/// 否则会产生数列的系统性偏移（旧版从第 3 列起算，误差约 5~12%）。
const BAR_X0: u16 = 6;

/// EQ 柱状行相对内容区左缘的缩进：disp_width("动态 EQ") = 7 + 2 空格 = 9，
/// 使第一根柱（20Hz）与标签行的 "20" 对齐。
const EQ_LABEL_PAD: usize = 9;

/// EQ 频段标签（纯 ASCII，自然单空格分隔）。注意 "1.2k" 等为 4 字符，
/// 占用 5 列（4 字符 + 1 空格），与 2/3 字符标签不同宽。
const FREQ_LABELS: [&str; 16] = [
    "20", "30", "46", "70", "105", "160", "240", "360",
    "550", "830", "1.2k", "1.9k", "2.9k", "4.4k", "6.6k", "10k",
];

/// 每根 EQ 柱在标签行中的起始列（内容内偏移，不含 EQ_LABEL_PAD 缩进）：
/// 柱 i 精确对准标签 i 的首字符。按各标签实际宽度（+1 空格）逐项累计，
/// 因此 4 字符标签之后列距变为 5，杜绝右端累计漂移。
fn freq_bar_cols() -> [usize; 16] {
    let mut cols = [0usize; 16];
    let mut c = 0usize;
    for (i, l) in FREQ_LABELS.iter().enumerate() {
        cols[i] = c;
        // 标签间单空格；最后一个标签后无空格（与 FREQ_LABELS.join(" ") 一致）
        c += l.len() + usize::from(i < 15);
    }
    cols
}

// ---------------------------------------------------------------------------
// 元数据
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
pub struct MetaInfo {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub composer: Option<String>,
    pub date: Option<String>,
    pub genre: Option<String>,
}

impl MetaInfo {
    /// (标签, 内容) 列表，用于封面右侧/下方展示。
    fn lines(&self) -> Vec<(&'static str, String)> {
        let mut v: Vec<(&'static str, String)> = Vec::new();
        if let Some(s) = &self.composer { v.push(("作曲家", s.clone())); }
        if let Some(s) = &self.artist { v.push(("艺术家", s.clone())); }
        if let Some(s) = &self.album { v.push(("专辑", s.clone())); }
        let extra = match (&self.date, &self.genre) {
            (Some(d), Some(g)) => Some(format!("{} · {}", d, g)),
            (Some(d), None) => Some(d.clone()),
            (None, Some(g)) => Some(g.clone()),
            (None, None) => None,
        };
        if let Some(s) = extra { v.push(("年代/风格", s)); }
        v
    }
}

// ---------------------------------------------------------------------------
// TUI
// ---------------------------------------------------------------------------

pub struct Tui {
    title: String,
    mode: &'static str,
    width: usize,
    height: usize,
    active: bool,
    art: Option<ArtImage>,
    meta: MetaInfo,
    /// 1-based 行号（鼠标 SGR 坐标）：点击状态行切换播放/暂停
    status_row: u16,
    /// 1-based 行号：点击进度条跳转
    bar_row: u16,
}

impl Tui {
    pub fn start(title: &str, mode: &'static str, enabled: bool) -> Option<Self> {
        Self::start_full(title, mode, enabled, None, MetaInfo::default())
    }

    /// 带封面与元数据的完整启动（音频文件模式）。
    pub fn start_full(
        title: &str,
        mode: &'static str,
        enabled: bool,
        art: Option<ArtImage>,
        meta: MetaInfo,
    ) -> Option<Self> {
        if !enabled || !stdout_is_terminal() {
            return None;
        }
        let mut tui = Self {
            title: title.to_string(),
            mode,
            width: terminal_width(),
            height: terminal_height(),
            active: true,
            art,
            meta,
            status_row: 3, // 状态行 = 0-based 第 2 行
            bar_row: 4,    // 进度条 = 0-based 第 3 行
        };
        print!("\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1006h\x1b[2J\x1b[H");
        let _ = io::stdout().flush();
        tui.draw(0, 0, 80, false, false, &[], &[0; 16]);
        Some(tui)
    }

    pub fn draw(&mut self, elapsed_ms: u64, total_ms: u64, volume: u32, paused: bool, looping: bool, details: &[String], spectrum: &[u8; 16]) {
        if !self.active {
            return;
        }
        self.width = terminal_width();
        self.height = terminal_height();
        let w = self.width;
        let h = self.height;
        let inner = w.saturating_sub(4); // "│ " 与 " │" 之间的内容宽度
        let pct = if total_ms == 0 { 0.0 } else { (elapsed_ms as f64 / total_ms as f64).clamp(0.0, 1.0) };

        // ---- 自适应布局 ----
        let need_details = details.len().min(4);
        let footer_rows = if h >= 19 { 2 } else { 1 };
        let fixed = 7 + footer_rows; // 顶框+标题+状态+进度+时间+音量+脚注+底框
        let meta = self.meta.lines();
        let meta_w = if !meta.is_empty() { (inner / 3).clamp(14, 30) } else { 0 };

        // 封面区：宽 ≤ 46 列，高 ≤ 16 行且不超过剩余空间的 45%（小一点），
        // 小屏保底 6 行（不要太小）。元数据并排或下置。
        let mut art_disp: Option<(usize, usize)> = None; // (显示宽, 显示行)
        let mut art_side = false;
        let mut section_rows = 0usize;
        if let Some(img) = &self.art {
            let max_art = ((h.saturating_sub(fixed)) as f64 * 0.45) as usize;
            let search_max = max_art.clamp(1, 16);
            for mh in (1..=search_max).rev() {
                // 并排元数据时：可用宽度 = 内宽 − 元数据列 − 间距，再取 46 列上限；
                // 屏幕足够宽时封面可到 46 列，而不是被元数据挤到十几列。
                let aw_max = if inner >= 56 && meta_w >= 14 {
                    inner.saturating_sub(4 + meta_w).min(46).max(10)
                } else {
                    (inner - 2).min(46).max(10)
                };
                let (aw, ah) = art_size(img, aw_max, mh);
                let side = aw + meta_w + 4 <= inner && meta_w >= 14;
                let extra = if side { 0 } else { meta.len().min(2) };
                let section = ah + 2 + extra;
                if fixed + section <= h {
                    art_disp = Some((aw, ah));
                    art_side = side;
                    section_rows = section;
                    break;
                }
            }
        } else if !meta.is_empty() {
            section_rows = meta.len().min(4);
        }

        let left = h.saturating_sub(fixed + section_rows);
        let eq_rows = if left >= 8 { 8 } else { 0 };
        let det_rows = if left.saturating_sub(eq_rows) >= need_details { need_details } else { 0 };

        // ---- 渲染 ----
        let mut out = String::with_capacity((w + 8) * h * 2 + 8192);

        // 顶框
        let head = "╭─ ♪ MUSIC RUST · 音乐播放器 ";
        let fill = w.saturating_sub(disp_width(head) + 1);
        out.push_str(&format!("{}╭─ ♪ MUSIC RUST · 音乐播放器 {}{}╮\n",
            fg(CYAN), "─".repeat(fill), RESET));

        // 标题行
        let title_disp = clip_w(&self.title, inner - 2);
        line(&mut out, inner, &format!(
            "{}♪{} {}《{}》{}", fg(MAGENTA), RESET, BOLD, title_disp, RESET));

        // 状态行（鼠标点击切换播放/暂停）
        let (dot_color, dot, state_color, state_text) = if paused {
            (RED, "●", YELLOW, "已暂停")
        } else {
            (GREEN, "●", GREEN, "正在播放")
        };
        let loop_text = if looping { "开" } else { "关" };
        let loop_color = if looping { GREEN } else { GRAY_DIM };
        let mode_part = format!("{}{}{}", fg(CYAN_DEEP), self.mode, RESET);
        let dot_part = format!("{}{}{}", fg(dot_color), dot, RESET);
        let state_part = format!("{}{}{}", fg(state_color), state_text, RESET);
        let loop_part = format!("{}{}循环 {}{}{}", fg(GRAY_DIM), RESET, fg(loop_color), loop_text, RESET);
        line(&mut out, inner, &format!("{}  {}   {}{}", mode_part, dot_part, state_part, loop_part));

        // 进度条（鼠标点击跳转）
        let bar_w = (inner.saturating_sub(12)).clamp(8, 72);
        let filled = pct * bar_w as f64;
        let whole = filled.floor() as usize;
        let frac = filled - whole as f64;
        let partial = if frac > 0.0 {
            PARTIAL_BLOCKS[((frac * 8.0).round() as usize).min(7)].to_string()
        } else { String::new() };
        let filled_part = format!("{}{}", "█".repeat(whole), partial);
        let empty_part = "░".repeat(bar_w.saturating_sub(whole + partial.chars().count()));
        let bar = format!("{}{}{}{}{}", fg(CYAN), filled_part, RESET, fg(GRAY_DIM), empty_part);
        line(&mut out, inner, &format!(
            "{} ▸ {} {}{:>5.1}%{}", fg(GRAY), RESET, bar, pct * 100.0, RESET));

        // 时间行
        let remaining = total_ms.saturating_sub(elapsed_ms);
        line(&mut out, inner, &format!(
            "{}{}{} / {}{}{}    {}剩余 {}{}",
            BOLD, format_time(elapsed_ms), RESET,
            DIM, format_time(total_ms), RESET,
            DIM, format_time(remaining), RESET));

        // 音量行
        let vol_filled = ((volume as f64 / 500.0) * 10.0).round() as usize;
        let vol_bar = format!(
            "{}{}{}{}",
            fg(YELLOW), "▓".repeat(vol_filled),
            fg(GRAY_DIM), "░".repeat(10 - vol_filled));
        line(&mut out, inner, &format!(
            "{}音量{} {} {}{:>3}%{}",
            fg(YELLOW), RESET, vol_bar, BOLD, volume, RESET));

        // 动态 EQ
        if eq_rows > 0 {
            // 标签行与柱状行严格对齐：柱状行从内容第 EQ_LABEL_PAD 列起，
            // 柱状行与标签行精确对齐：每根柱直接放在对应标签的起始列。
            // 标签为 ASCII 且自然单空格分隔，按实际宽度逐项累计（"1.2k" 等
            // 4 字符标签占 5 列），不能假设均匀槽位——否则越靠右偏差越大
            // （"1.2k" 之后每根偏 1 列，"10k" 累计偏 5 列）。
            let bar_cols = freq_bar_cols();
            if inner >= 74 {
                let label_line = FREQ_LABELS.join(" ");
                line(&mut out, inner, &format!(
                    "{}动态 EQ{}  {}", fg(CYAN), RESET, label_line));
            } else {
                line(&mut out, inner, &format!(
                    "{}动态 EQ{}  ·  20Hz – 10kHz", fg(CYAN), RESET));
            }
            for row in (1..=7).rev() {
                let color = EQ_COLORS[row - 1];
                let mut s = String::with_capacity(inner);
                s.push_str(&" ".repeat(EQ_LABEL_PAD));
                if inner >= 74 {
                    // 宽屏：柱体精确置于标签起始列（非均匀间距）
                    let mut cur = 0usize;
                    for band in 0..16 {
                        s.push_str(&" ".repeat(bar_cols[band].saturating_sub(cur)));
                        cur = bar_cols[band] + 1;
                        if spectrum[band] >= row as u8 {
                            s.push_str(&format!("{}█{}", fg(color), RESET));
                        } else {
                            s.push_str(&format!("{}░{}", fg(GRAY_DIM), RESET));
                        }
                    }
                } else {
                    // 窄屏：无逐频段标签，均匀排布
                    let spacing = ((inner.saturating_sub(EQ_LABEL_PAD + 16)) / 15).clamp(0, 3);
                    for band in 0..16 {
                        if spectrum[band] >= row as u8 {
                            s.push_str(&format!("{}█{}", fg(color), RESET));
                        } else {
                            s.push_str(&format!("{}░{}", fg(GRAY_DIM), RESET));
                        }
                        if band < 15 {
                            s.push_str(&" ".repeat(spacing));
                        }
                    }
                }
                line(&mut out, inner, &s);
            }
        }

        // 音符详情
        for d in details.iter().take(det_rows) {
            line(&mut out, inner, &format!("{}{}{}", DIM, clip_w(d, inner - 2), RESET));
        }

        // 封面 + 元数据区
        if let Some((aw, ah)) = art_disp {
            self.render_art_section(&mut out, inner, aw, ah, art_side, &meta);
        } else if !meta.is_empty() {
            for (label, value) in meta.iter().take(4) {
                line(&mut out, inner, &format!("{}", meta_text(label, value, inner - 2)));
            }
        }
        // 按键提示
        line(&mut out, inner, &format!(
            "{}空格 暂停 · ←/→ 快退/快进 · ↑/↓ 10s · R 循环 · 9/0 音量 · Q 退出{}",
            DIM, RESET));
        if footer_rows == 2 {
            line(&mut out, inner, &format!(
                "{}鼠标：点击进度条跳转 · 点击状态行播放/暂停{}", DIM, RESET));
        }

        // 底框
        out.push_str(&format!("{}{}{}{}\n", fg(CYAN), "╰", "─".repeat(w.saturating_sub(2)), "╯"));
        if w > 0 {
            out.push_str(RESET);
        }

        print!("\x1b[H\x1b[2J{}", out);
        let _ = io::stdout().flush();
    }

    /// 封面区：封面框（左侧）+ 元数据（右侧并排 / 下方）。
    fn render_art_section(
        &self,
        out: &mut String,
        inner: usize,
        aw: usize,
        ah: usize,
        side: bool,
        meta: &[(&'static str, String)],
    ) {
        let img = match &self.art { Some(i) => i, None => return };
        let box_rows = ah + 2;
        let meta_cols = inner.saturating_sub(aw + 4);
        // 无元数据时封面框水平居中
        let indent = if meta.is_empty() {
            " ".repeat(inner.saturating_sub(aw + 2) / 2)
        } else {
            String::new()
        };
        let mut art_lines: Vec<String> = Vec::with_capacity(box_rows);

        // 封面框顶边（宽度足够时内嵌标题）
        let top = if aw >= 10 {
            format!("{}{}┌─ 封面 {}{}┐", indent, fg(GRAY), RESET, "─".repeat(aw - 7))
        } else {
            format!("{}{}┌{}┐{}", indent, fg(GRAY), "─".repeat(aw), RESET)
        };
        art_lines.push(top);

        // 半块字符渲染
        for row in 0..ah {
            art_lines.push(format!("{}{}│{}│{}", indent, fg(GRAY), render_art_row(img, row, aw, ah), RESET));
        }
        art_lines.push(format!("{}{}└{}┘{}", indent, fg(GRAY), "─".repeat(aw), RESET));

        if side {
            // 元数据垂直居中于封面框右侧
            let pad_top = (box_rows.saturating_sub(meta.len())) / 2;
            let mut meta_iter = meta.iter();
            for (i, box_line) in art_lines.iter().enumerate() {
                let mut s = box_line.clone();
                if i >= pad_top {
                    if let Some((label, value)) = meta_iter.next() {
                        let avail = meta_cols.saturating_sub(2).max(4);
                        s.push_str(&format!("  {}", meta_text(label, value, avail)));
                    }
                }
                line(out, inner, &s);
            }
        } else {
            for box_line in &art_lines {
                line(out, inner, box_line);
            }
            // 窄屏：元数据下置（最多 2 行）
            for (label, value) in meta.iter().take(2) {
                line(out, inner, &format!("  {}", meta_text(label, value, inner - 4)));
            }
        }
    }

    pub fn mouse_control(&self, x: u16, y: u16, paused: bool) -> Control {
        if y == self.bar_row {
            let bar_w = (self.width.saturating_sub(4 + 12)).clamp(8, 72) as f64;
            if bar_w <= 1.0 {
                return Control::None;
            }
            // SGR 鼠标 x 为 1-based；进度条自第 BAR_X0 列起，占 bar_w 格。
            // offset ∈ [0, bar_w-1] → 百分比 = offset / (bar_w-1)，首/末格精确对应 0%/100%。
            let offset = (f64::from(x) - f64::from(BAR_X0))
                .max(0.0)
                .min(bar_w - 1.0);
            return Control::SeekPercent(offset / (bar_w - 1.0));
        }
        if y == self.status_row {
            return if paused { Control::Play } else { Control::Pause };
        }
        Control::None
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        if self.active {
            print!("\x1b[?1000l\x1b[?1006l\x1b[?25h\x1b[?1049l");
            let _ = io::stdout().flush();
            self.active = false;
        }
    }
}

// ---------------------------------------------------------------------------
// 渲染辅助
// ---------------------------------------------------------------------------

fn fg(c: (u8, u8, u8)) -> String {
    format!("\x1b[38;2;{};{};{}m", c.0, c.1, c.2)
}

/// 输出一行内容：带左右边框、按显示宽度右对齐填充到内容宽度。
/// 注意：pad 只追加空格（pad_to 会返回内容本身 + 空格，不能直接拼接）。
fn line(out: &mut String, inner: usize, content: &str) {
    let pad = " ".repeat(inner.saturating_sub(disp_width(content)));
    out.push_str(&format!("{}│ {}{} │{}\n", fg(GRAY), content, pad, RESET));
}

/// 元数据行：彩色标签 + 内容。
fn meta_text(label: &str, value: &str, max: usize) -> String {
    let color = match label {
        "作曲家" => MAGENTA,
        "艺术家" => CYAN,
        "专辑" => GREEN_DIM,
        _ => GRAY,
    };
    format!(
        "{}{} {}{}{}",
        fg(color), label, RESET, clip_w(value, max.saturating_sub(disp_width(label) + 1)), RESET
    )
}

/// 计算显示宽度（CJK 全角按 2 列，忽略 ANSI 转义序列）。
fn disp_width(s: &str) -> usize {
    let mut w = 0usize;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // 跳过 ESC 序列（CSI：直到 final byte；其余 2 字符）
            match chars.next() {
                Some('[') => {
                    for c2 in chars.by_ref() {
                        if ('@'..='~').contains(&c2) {
                            break;
                        }
                    }
                }
                Some(_) => {}
                None => break,
            }
        } else {
            w += if is_wide(c) { 2 } else { 1 };
        }
    }
    w
}

fn is_wide(c: char) -> bool {
    let cp = c as u32;
    (0x1100..=0x115f).contains(&cp) || (0x2e80..=0x303e).contains(&cp)
        || (0x3041..=0x33ff).contains(&cp) || (0x3400..=0x4dbf).contains(&cp)
        || (0x4e00..=0x9fff).contains(&cp) || (0xa000..=0xa4cf).contains(&cp)
        || (0xac00..=0xd7a3).contains(&cp) || (0xf900..=0xfaff).contains(&cp)
        || (0xfe30..=0xfe4f).contains(&cp) || (0xff00..=0xff60).contains(&cp)
        || (0xffe0..=0xffe6).contains(&cp)
}

/// 按显示宽度截断，超长时追加省略号。
fn clip_w(s: &str, max: usize) -> String {
    if max == 0 { return String::new(); }
    if disp_width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for c in s.chars() {
        let cw = if is_wide(c) { 2 } else { 1 };
        if w + cw > max.saturating_sub(1) { break; }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// 按宽高约束计算封面显示尺寸（行数 = 像素高 / 2，因半块字符一行为两像素）。
fn art_size(img: &ArtImage, max_w: usize, max_h_rows: usize) -> (usize, usize) {
    if img.width == 0 || img.height == 0 {
        return (0, 0);
    }
    let scale = (max_w as f64 / img.width as f64)
        .min(max_h_rows as f64 * 2.0 / img.height as f64);
    let w = ((img.width as f64 * scale).round() as usize).clamp(1, max_w);
    let h = (((img.height as f64 * scale).round() as usize) / 2).clamp(1, max_h_rows);
    (w, h)
}

/// 渲染封面的一行（半块字符 + 真彩色前景/背景 = 上下两个像素）。
fn render_art_row(img: &ArtImage, row: usize, disp_w: usize, disp_h: usize) -> String {
    let mut s = String::with_capacity(disp_w * 24);
    let total_px_h = disp_h * 2;
    for col in 0..disp_w {
        let x = (col as f64 + 0.5) * img.width as f64 / disp_w as f64;
        let y_top = (row as f64 * 2.0 + 0.5) * img.height as f64 / total_px_h as f64;
        let top = sample_px(img, x, y_top);
        let bot = if row * 2 + 1 < total_px_h {
            let y_bot = (row as f64 * 2.0 + 1.5) * img.height as f64 / total_px_h as f64;
            sample_px(img, x, y_bot)
        } else {
            // 最后一行无下半像素：下半压暗收边
            ((top.0 as u32 * 3 / 4) as u8, (top.1 as u32 * 3 / 4) as u8, (top.2 as u32 * 3 / 4) as u8)
        };
        s.push_str(&format!(
            "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m▀",
            top.0, top.1, top.2, bot.0, bot.1, bot.2
        ));
    }
    s.push_str(RESET);
    s
}

/// 双线性采样 RGBA 像素并合成到深色衬底上。
fn sample_px(img: &ArtImage, x: f64, y: f64) -> (u8, u8, u8) {
    let iw = img.width as f64;
    let ih = img.height as f64;
    let x = x.clamp(0.0, iw - 1.0);
    let y = y.clamp(0.0, ih - 1.0);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(img.width - 1);
    let y1 = (y0 + 1).min(img.height - 1);
    let fx = (x - x0 as f64) as f32;
    let fy = (y - y0 as f64) as f32;
    let at = |xi: usize, yi: usize| -> (f32, f32, f32, f32) {
        let i = (yi * img.width + xi) * 4;
        (
            img.data[i] as f32 / 255.0,
            img.data[i + 1] as f32 / 255.0,
            img.data[i + 2] as f32 / 255.0,
            img.data[i + 3] as f32 / 255.0,
        )
    };
    // 预乘 alpha 的双线性插值（颜色正确且无黑边）
    let (r00, g00, b00, a00) = at(x0, y0);
    let (r10, g10, b10, a10) = at(x1, y0);
    let (r01, g01, b01, a01) = at(x0, y1);
    let (r11, g11, b11, a11) = at(x1, y1);
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let blend = |a: f32, b: f32, c: f32, d: f32| {
        let top = lerp(a, b, fx);
        let bot = lerp(c, d, fx);
        lerp(top, bot, fy)
    };
    let pr = blend(r00 * a00, r10 * a10, r01 * a01, r11 * a11);
    let pg = blend(g00 * a00, g10 * a10, g01 * a01, g11 * a11);
    let pb = blend(b00 * a00, b10 * a10, b01 * a01, b11 * a11);
    let pa = blend(a00, a10, a01, a11);
    // 合成到衬底
    let (br, bg_, bb) = ART_BG;
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    (
        to8(pr + br as f32 / 255.0 * (1.0 - pa)),
        to8(pg + bg_ as f32 / 255.0 * (1.0 - pa)),
        to8(pb + bb as f32 / 255.0 * (1.0 - pa)),
    )
}

fn format_time(ms: u64) -> String {
    let total = ms / 1000;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}

#[cfg(unix)]
fn stdout_is_terminal() -> bool {
    unsafe { libc::isatty(libc::STDOUT_FILENO) == 1 }
}

#[cfg(not(unix))]
fn stdout_is_terminal() -> bool { false }

#[cfg(unix)]
fn terminal_width() -> usize {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) } == 0 && size.ws_col > 0 {
        return usize::from(size.ws_col).max(30);
    }
    80
}

#[cfg(not(unix))]
fn terminal_width() -> usize { 80 }

#[cfg(unix)]
fn terminal_height() -> usize {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) } == 0 && size.ws_row > 0 {
        return usize::from(size.ws_row).max(10);
    }
    24
}

#[cfg(not(unix))]
fn terminal_height() -> usize { 24 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_and_pad_respect_cjk_width() {
        assert_eq!(disp_width("音乐"), 4);
        assert_eq!(disp_width("♪"), 1);
        assert_eq!(clip_w("中文标题很长的歌曲名字", 8), "中文标…"); // 7 列 + 省略号 = 8 列
        assert_eq!(clip_w("abcd", 8), "abcd");
        // 显示宽度计算忽略 ANSI 转义
        assert_eq!(disp_width("\x1b[38;2;1;2;3m音\x1b[0m"), 2);
        let t = format_time(3723_000);
        assert_eq!(t, "1:02:03");
        assert_eq!(format_time(83_000), "01:23");
    }

    #[test]
    fn art_size_respects_constraints() {
        let img = ArtImage { data: vec![0u8; 64 * 64 * 4], width: 64, height: 64 };
        let (w, h) = art_size(&img, 46, 16);
        assert!(w <= 46 && h <= 16 && h >= 1);
        let (w2, h2) = art_size(&img, 20, 10);
        assert!(w2 <= 20 && h2 <= 10);
        // 宽图
        let wide = ArtImage { data: vec![0u8; 96 * 32 * 4], width: 96, height: 32 };
        let (w3, h3) = art_size(&wide, 46, 10);
        assert!(w3 <= 46 && h3 <= 10);
        assert!(w3 > h3, "宽图应保持横向比例");
    }

    fn test_tui() -> Tui {
        Tui {
            title: "t".into(),
            mode: "m",
            width: 80,
            height: 24,
            active: false,
            art: None,
            meta: MetaInfo::default(),
            status_row: 3,
            bar_row: 4,
        }
    }

    /// 进度条点击映射必须与渲染布局严格一致（回归测试：修复 +3 列系统性偏移）。
    #[test]
    fn mouse_bar_click_maps_precisely() {
        let tui = test_tui();
        // 80 列 → inner=76 → bar_w=64 → 进度条占 1-based 第 6..69 列
        assert_eq!(BAR_X0, 6);
        let bar_w = 64.0;
        // 起点：进度条第 1 格 → 0%
        assert_eq!(tui.mouse_control(BAR_X0, 4, false), Control::SeekPercent(0.0));
        // 终点：进度条最后 1 格 → 100%
        let c = tui.mouse_control(BAR_X0 + (bar_w as u16) - 1, 4, false);
        assert!(matches!(c, Control::SeekPercent(p) if (p - 1.0).abs() < 1e-9));
        // 中点：第 32 格（列 6+31=37）→ 31/63 ≈ 49.2%
        let c = tui.mouse_control(37, 4, false);
        assert!(matches!(c, Control::SeekPercent(p) if (p - 31.0 / 63.0).abs() < 1e-9));
        // 点击进度条左侧（▸ 标记区域）→ 0%
        assert_eq!(tui.mouse_control(3, 4, false), Control::SeekPercent(0.0));
        // 超出进度条右端（百分比文字区域）→ 100%（钳制）
        let c = tui.mouse_control(76, 4, false);
        assert!(matches!(c, Control::SeekPercent(p) if (p - 1.0).abs() < 1e-9));
        // 其它行不响应
        assert_eq!(tui.mouse_control(37, 5, false), Control::None);
        // 状态行切换播放/暂停
        assert_eq!(tui.mouse_control(10, 3, true), Control::Play);
        assert_eq!(tui.mouse_control(10, 3, false), Control::Pause);
    }

    /// 窄终端（bar_w=24）下偏移映射同样精确。
    #[test]
    fn mouse_bar_click_narrow_terminal() {
        let mut tui = test_tui();
        tui.width = 40; // inner=36 → bar_w=24 → 进度条占第 6..29 列
        let bar_w = 24.0;
        assert_eq!(tui.mouse_control(BAR_X0, 4, false), Control::SeekPercent(0.0));
        let c = tui.mouse_control(BAR_X0 + (bar_w as u16) - 1, 4, false);
        assert!(matches!(c, Control::SeekPercent(p) if (p - 1.0).abs() < 1e-9));
        let c = tui.mouse_control(17, 4, false); // 第 12 格 → 11/23 ≈ 47.8%
        assert!(matches!(c, Control::SeekPercent(p) if (p - 11.0 / 23.0).abs() < 1e-9));
    }

    /// EQ 柱起始列必须与标签起始列逐一相等（回归测试：4 字符标签 "1.2k"
    /// 占 5 列导致右端累计漂移，10kHz 柱曾偏 5 列）。
    #[test]
    fn freq_bars_align_with_labels() {
        let cols = freq_bar_cols();
        assert_eq!(cols, [0, 3, 6, 9, 12, 16, 20, 24, 28, 32, 36, 41, 46, 51, 56, 61]);
        // 与标签行（FREQ_LABELS.join(" ")）中每个标签的起始字符位置一致
        let line = FREQ_LABELS.join(" ");
        let mut c = 0usize;
        for (i, l) in FREQ_LABELS.iter().enumerate() {
            assert_eq!(line[c..].starts_with(l), true, "band {} 标签错位", i);
            assert_eq!(cols[i], c, "band {} 柱与标签错位", i);
            c += l.len() + usize::from(i < 15); // 与 freq_bar_cols 同规则
        }
        // 标签行总宽（含缩进）须能放入宽屏阈值 inner=74
        assert_eq!(c, 64);
        assert!(EQ_LABEL_PAD + c <= 74);
    }
}
