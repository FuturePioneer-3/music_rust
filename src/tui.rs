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

//! 播放时使用的全屏终端界面。
//!
//! 自研轻量 TUI（不依赖任何终端库）：
//!   - 行级 diff 重绘：只更新变化的行，无闪烁、低开销
//!   - 渐变标题栏 + 圆角边框布局
//!   - 实时钢琴键盘（按下的键高亮发光）
//!   - 动画频谱 EQ（平滑 + 峰值保持，绿→黄→橙→红渐变）
//!   - 内嵌日志面板（读取自研 log 系统的环形缓冲）
//!   - 鼠标：点进度条跳转 / 点状态栏播放暂停

use std::io::{self, Write};

use crate::input::Control;

/// 白色琴键（未按下）：浅灰底
const WHITE_KEY: &str = "\x1b[48;5;252m  \x1b[0m";
/// 白色琴键（按下）：黄底黑字，发光效果
const PRESSED_WHITE: &str = "\x1b[48;5;226m\x1b[38;5;16m▓▓\x1b[0m";
/// 黑色琴键（未按下）：深灰底
const BLACK_KEY: &str = "\x1b[48;5;240m  \x1b[0m";
/// 黑色琴键（按下）：青底黑字
const PRESSED_BLACK: &str = "\x1b[48;5;51m\x1b[38;5;16m▓▓\x1b[0m";
/// 16 段频谱的频点标签（每格 3 字符）
const EQ_LABELS: [&str; 16] = [
    "20", "30", "46", "70", "105", "160", "240", "360", "550", "830", "1.2", "1.9", "2.9", "4.4",
    "6.6", "10k",
];

pub struct Tui {
    title: String,
    mode: &'static str,
    width: usize,
    height: usize,
    active: bool,
    color256: bool,
    /// 上一帧（行级 diff 用）
    prev: Vec<String>,
    /// 频谱平滑动画状态
    eq_anim: [f32; 16],
    /// 频谱峰值保持状态
    eq_peak: [f32; 16],
    /// 进度条所在行（1 起，鼠标跳转用）
    bar_row: usize,
    /// 进度条绘制宽度
    bar_w: usize,
    /// 状态栏所在行（1 起，鼠标播放/暂停用）
    status_row: usize,
    /// 标题行（点击切换播放/暂停）
    title_row: usize,
}

impl Tui {
    pub fn start(title: &str, mode: &'static str, enabled: bool) -> Option<Self> {
        if !enabled || !stdout_is_terminal() {
            return None;
        }
        let mut tui = Self {
            title: title.to_string(),
            mode,
            width: terminal_width(),
            height: terminal_height(),
            active: true,
            color256: terminal_256color(),
            prev: Vec::new(),
            eq_anim: [0.0; 16],
            eq_peak: [0.0; 16],
            bar_row: 2,
            bar_w: 20,
            status_row: 0,
            title_row: 1,
        };
        print!(
            "\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1006h\x1b[2J\x1b[H"
        );
        let _ = io::stdout().flush();
        crate::log::set_tui_active(true);
        tui.draw(0, 0, 80, false, false, &[], &[], &[0; 16]);
        Some(tui)
    }

    /// 绘制一帧。`keys` 为当前按住的 MIDI 键（0-127），用于钢琴键盘显示。
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        elapsed_ms: u64,
        total_ms: u64,
        volume: u32,
        paused: bool,
        looping: bool,
        details: &[String],
        keys: &[u8],
        spectrum: &[u8; 16],
    ) {
        if !self.active {
            return;
        }
        self.width = terminal_width();
        self.height = terminal_height();
        let inner = self.width.saturating_sub(2);

        // ---- 自适应布局：高度不足时依次收缩日志/详情/频谱/钢琴 ----
        let mut log_rows = 4usize;
        let mut eq_on = inner >= 56;
        let mut piano_on = inner >= 30;
        let mut details_on = true;
        loop {
            let mut rows = 5; // 顶栏 + 进度 + 时间 + 状态 + 底栏
            rows += if piano_on { 2 } else { 0 };
            rows += if eq_on { 8 } else { 0 }; // 标签行 + 7 行柱
            rows += if details_on { details.len() } else { 0 };
            rows += 1 + log_rows; // 日志标题 + 日志行
            if rows <= self.height || self.height == 0 {
                break;
            }
            if log_rows > 2 {
                log_rows -= 1;
            } else if details_on && !details.is_empty() {
                details_on = false;
            } else if eq_on {
                eq_on = false;
            } else if piano_on {
                piano_on = false;
            } else {
                break;
            }
        }
        let _ = inner;

        let pct = if total_ms == 0 {
            0.0
        } else {
            (elapsed_ms as f64 / total_ms as f64).clamp(0.0, 1.0)
        };

        // ---- 频谱平滑动画 ----
        for i in 0..16 {
            let t = spectrum[i] as f32;
            self.eq_anim[i] += (t - self.eq_anim[i]) * 0.45;
            let p = self.eq_peak[i] - 0.10;
            self.eq_peak[i] = if t > p { t } else { p.max(0.0) };
        }

        let mut frame: Vec<String> = Vec::new();
        self.title_row = 1;
        frame.push(self.line_top(paused, looping));
        frame.push(self.line_progress(pct));
        self.bar_row = frame.len(); // 1-based 行号
        frame.push(self.line_time(elapsed_ms, total_ms, volume, looping));
        if piano_on {
            let (black, white) = self.piano_rows(keys);
            frame.push(format!("│  {}  │", black));
            frame.push(format!("│  {}  │", white));
        }
        if eq_on {
            frame.push(self.line_eq_label());
            for row in (1..=7).rev() {
                frame.push(self.line_eq_row(row));
            }
        }
        if details_on {
            for detail in details {
                frame.push(format!(
                    "│ \x1b[2m♪\x1b[0m {}  │",
                    clip(detail, self.width.saturating_sub(6))
                ));
            }
        }
        // ---- 日志面板 ----
        frame.push(format!(
            "│  {}  │",
            dim(format!(
                "── {} ──{}",
                "日志",
                "─".repeat(self.width.saturating_sub(14))
            ))
        ));
        let logs = crate::log::snapshot();
        let start_idx = logs.len().saturating_sub(log_rows);
        for line in logs.iter().skip(start_idx).take(log_rows) {
            frame.push(self.line_log(line));
        }
        // ---- 状态栏（鼠标点击切换播放/暂停）----
        frame.push(self.line_status(paused));
        self.status_row = frame.len();
        frame.push(self.line_bottom());

        self.paint(frame);
    }

    /// 顶栏：渐变标题 + 播放状态徽标
    fn line_top(&self, paused: bool, looping: bool) -> String {
        let title = if self.color256 {
            // 蓝→青→蓝 渐变
            let grad = [33u8, 39, 45, 51, 45, 39, 33];
            let mut s = String::from("◆ ");
            for (i, ch) in "MUSIC RUST".chars().enumerate() {
                s.push_str(&format!(
                    "\x1b[1;38;5;{}m{}\x1b[0m",
                    grad[i % grad.len()],
                    ch
                ));
            }
            s
        } else {
            "\x1b[1;36m◆ MUSIC RUST\x1b[0m".to_string()
        };
        let subtitle = format!(" · {} · {}", self.mode, clip(&self.title, 26));
        let badge = if paused {
            if self.color256 {
                "\x1b[1;38;5;220m⏸ 已暂停\x1b[0m"
            } else {
                "\x1b[1;33m⏸ 已暂停\x1b[0m"
            }
        } else if self.color256 {
            "\x1b[1;38;5;82m▶ 播放中\x1b[0m"
        } else {
            "\x1b[1;32m▶ 播放中\x1b[0m"
        };
        let loop_badge = if looping {
            if self.color256 {
                " \x1b[1;38;5;51m🔁\x1b[0m"
            } else {
                " \x1b[1;36m🔁\x1b[0m"
            }
        } else {
            ""
        };
        let head = format!("╭─ {}{} ─ {}", title, dim(subtitle), badge);
        let tail = format!("{}╮", loop_badge);
        let fill = self
            .width
            .saturating_sub(display_width(&head) + display_width(&tail))
            .saturating_sub(2);
        format!("{}{}{}", head, "─".repeat(fill), tail)
    }

    /// 进度条行
    fn line_progress(&mut self, pct: f64) -> String {
        let pct_text = format!("{:>6.1}%", pct * 100.0);
        // "│  [" + bar + "] " + pct + "  │" → bar_w = width - 16
        let bar_w = self.width.saturating_sub(16).clamp(10, 80);
        self.bar_w = bar_w;
        let filled = (pct * bar_w as f64).round() as usize;
        let bar = format!(
            "\x1b[1;38;5;45m{}\x1b[0m{}",
            "━".repeat(filled),
            dim("░".repeat(bar_w - filled))
        );
        format!("│  [{}] {}  │", bar, pct_text)
    }

    /// 时间 / 音量 / 循环行
    fn line_time(&self, elapsed_ms: u64, total_ms: u64, volume: u32, looping: bool) -> String {
        let elapsed = format_time(elapsed_ms);
        let total = format_time(total_ms);
        let remaining = format_time(total_ms.saturating_sub(elapsed_ms));
        // 音量条：0-500% → 8 格
        let gauge_w = 8usize;
        let filled = ((volume.min(500) as f64 / 500.0) * gauge_w as f64).round() as usize;
        let gauge = format!(
            "\x1b[38;5;220m{}\x1b[0m{}",
            "█".repeat(filled),
            dim("░".repeat(gauge_w - filled))
        );
        let loop_txt = if looping {
            "\x1b[1;38;5;51m开\x1b[0m".to_string()
        } else {
            "\x1b[2m关\x1b[0m".to_string()
        };
        let line = format!(
            "│  {} / {}  ·  剩余 {}  ·  音量 {} {:>3}%  ·  循环 {}  │",
            elapsed, total, remaining, gauge, volume, loop_txt
        );
        if display_width(&line) <= self.width {
            line
        } else {
            format!(
                "│  {} / {}  ·  音量 {:>3}%  ·  循环 {}  │",
                elapsed, total, volume, loop_txt
            )
        }
    }

    /// 频谱标签行
    fn line_eq_label(&self) -> String {
        let labels: String = EQ_LABELS
            .iter()
            .map(|l| format!("{:>3}", l))
            .collect();
        format!("│  {}  │", dim(labels))
    }

    /// 频谱柱行（row: 1=最底 … 7=最顶）
    fn line_eq_row(&self, row: usize) -> String {
        let mut s = String::new();
        for i in 0..16 {
            let h = self.eq_anim[i].round() as usize;
            let peak = self.eq_peak[i];
            if h >= row {
                let c = if h >= 6 {
                    196
                } else if h >= 4 {
                    208
                } else if h >= 2 {
                    220
                } else {
                    82
                };
                s.push_str(&format!("\x1b[38;5;{}m███\x1b[0m", c));
            } else if peak >= row as f32 {
                s.push_str(&dim("· ·".to_string()));
            } else {
                s.push_str("   ");
            }
        }
        format!("│  {}  │", s)
    }

    /// 实时钢琴键盘：围绕当前音符开 2 个八度窗口
    fn piano_rows(&self, keys: &[u8]) -> (String, String) {
        let center = if keys.is_empty() {
            60.0
        } else {
            keys.iter().map(|k| *k as f32).sum::<f32>() / keys.len() as f32
        };
        let mut start = (center as i32) - 12;
        start -= start.rem_euclid(12);
        start = start.clamp(0, 84);
        let white_offsets = [0i32, 2, 4, 5, 7, 9, 11];
        let mut black = String::new();
        let mut white = String::new();
        for oct in 0..2 {
            for (wi, off) in white_offsets.iter().enumerate() {
                let wk = start + oct * 12 + off;
                let black_after = white_offsets
                    .get(wi + 1)
                    .map(|n| *n - off == 2)
                    .unwrap_or(false);
                if black_after {
                    let bk = wk + 1;
                    let pressed = keys.contains(&(bk as u8));
                    black.push_str(if pressed { PRESSED_BLACK } else { BLACK_KEY });
                } else {
                    black.push_str("  ");
                }
                let pressed = keys.contains(&(wk as u8));
                white.push_str(if pressed { PRESSED_WHITE } else { WHITE_KEY });
            }
        }
        (black, white)
    }

    /// 日志面板行
    fn line_log(&self, line: &crate::log::LogLine) -> String {
        let time = format!(
            "{:02}:{:02}.{:03}",
            (line.time_ms / 60_000) % 100,
            (line.time_ms / 1000) % 60,
            line.time_ms % 1000
        );
        let tag = format!("\x1b[1;38;5;{}m[{}]\x1b[0m", line.level.tui_color(), line.level.tag());
        let body = clip(&line.msg, self.width.saturating_sub(24));
        format!("│ {} {} {}  │", dim(time), tag, body)
    }

    /// 状态栏（帮助键位 + 鼠标提示）
    fn line_status(&self, paused: bool) -> String {
        let k = |s: &str| format!("\x1b[7m{}\x1b[0m", s);
        let mut s = format!(
            "│  {} {}   {} {}   {} {}   {} {}   {} {}   {} {}   {} {}   {} {}   {} {}  │",
            k("Space"),
            if paused { "播放" } else { "暂停" },
            k("R"),
            "循环",
            k("←→"),
            "5s",
            k("↑↓"),
            "10s",
            k("PgUp/Dn"),
            "1m",
            k("[ ]"),
            "1s",
            k("9/0"),
            "音量",
            k("Q"),
            "退出",
            k("🖱"),
            "进度条=跳转/状态栏=播放",
        );
        if display_width(&s) > self.width {
            s = format!(
                "│  {} {}   {} {}   {} {}   {} {}   {} {}   {} {}   {} {}  │",
                k("Space"), if paused { "播放" } else { "暂停" }, k("R"), "循环", k("←→"), "5s",
                k("↑↓"), "10s", k("PgUp/Dn"), "1m", k("9/0"), "音量", k("Q"), "退出"
            );
        }
        s
    }

    fn line_bottom(&self) -> String {
        format!("╰{}╯", "─".repeat(self.width.saturating_sub(2)))
    }

    /// 行级 diff 重绘：只输出变化的行
    fn paint(&mut self, frame: Vec<String>) {
        if frame.is_empty() {
            return;
        }
        let mut out = String::new();
        for (i, line) in frame.iter().enumerate() {
            let changed = self.prev.get(i).map(|p| p != line).unwrap_or(true);
            if changed {
                out.push_str(&format!("\x1b[{};1H\x1b[2K{}", i + 1, line));
            }
        }
        // 清掉上一帧多出的行
        for i in frame.len()..self.prev.len() {
            out.push_str(&format!("\x1b[{};1H\x1b[2K", i + 1));
        }
        self.prev = frame;
        print!("{}", out);
        let _ = io::stdout().flush();
    }

    /// 鼠标点击映射
    pub fn mouse_control(&self, x: u16, y: u16, paused: bool) -> Control {
        let y = usize::from(y);
        let x = usize::from(x);
        if y == self.bar_row {
            let offset = x.saturating_sub(3).min(self.bar_w);
            return Control::SeekPercent(offset as f64 / self.bar_w as f64);
        }
        if y == self.status_row || y == self.title_row {
            return if paused { Control::Play } else { Control::Pause };
        }
        Control::None
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        if self.active {
            crate::log::set_tui_active(false);
            print!("\x1b[?1000l\x1b[?1006l\x1b[?25h\x1b[?1049l");
            let _ = io::stdout().flush();
            self.active = false;
        }
    }
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

fn dim(s: String) -> String {
    format!("\x1b[2m{}\x1b[0m", s)
}

fn format_time(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// 估算字符串的显示宽度（忽略 ANSI 转义序列）
fn display_width(s: &str) -> usize {
    let mut w = 0usize;
    let mut in_esc = false;
    for ch in s.chars() {
        if in_esc {
            if ch == 'm' {
                in_esc = false;
            }
            continue;
        }
        if ch == '\x1b' {
            in_esc = true;
            continue;
        }
        w += 1;
    }
    w
}

#[cfg(unix)]
fn stdout_is_terminal() -> bool {
    unsafe { libc::isatty(libc::STDOUT_FILENO) == 1 }
}

#[cfg(not(unix))]
fn stdout_is_terminal() -> bool {
    false
}

/// 终端是否支持 256 色
fn terminal_256color() -> bool {
    std::env::var("TERM")
        .map(|t| {
            t.contains("256color")
                || t.contains("truecolor")
                || t.contains("xterm")
                || t.contains("screen")
                || t.contains("tmux")
                || t.contains("kitty")
                || t.contains("alacritty")
                || t.contains("foot")
                || t.contains("wezterm")
        })
        .unwrap_or(false)
}

#[cfg(unix)]
fn terminal_width() -> usize {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) } == 0
        && size.ws_col > 0
    {
        return usize::from(size.ws_col).max(40);
    }
    80
}

#[cfg(not(unix))]
fn terminal_width() -> usize {
    80
}

#[cfg(unix)]
fn terminal_height() -> usize {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) } == 0
        && size.ws_row > 0
    {
        return usize::from(size.ws_row);
    }
    24
}

#[cfg(not(unix))]
fn terminal_height() -> usize {
    24
}
