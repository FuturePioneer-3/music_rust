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

//! 播放时使用的轻量终端界面。
//!
//! 不依赖第三方终端库，避免增加运行时依赖；仅在 stdout 为终端时启用。

use std::io::{self, Write};

use crate::input::Control;

pub struct Tui {
    title: String,
    mode: &'static str,
    width: usize,
    active: bool,
}

impl Tui {
    pub fn start(title: &str, mode: &'static str, enabled: bool) -> Option<Self> {
        if !enabled || !stdout_is_terminal() {
            return None;
        }

        let tui = Self {
            title: title.to_string(),
            mode,
            width: terminal_width(),
            active: true,
        };
        print!("\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1006h\x1b[2J\x1b[H");
        let _ = io::stdout().flush();
        let mut tui = tui;
        tui.draw(0, 0, 80, false, false, "");
        Some(tui)
    }

    pub fn draw(&mut self, elapsed_ms: u64, total_ms: u64, volume: u32, paused: bool, looping: bool, detail: &str) {
        if !self.active {
            return;
        }
        self.width = terminal_width();
        let pct = if total_ms == 0 { 0.0 } else { (elapsed_ms as f64 / total_ms as f64).clamp(0.0, 1.0) };
        let bar_width = self.width.saturating_sub(8).clamp(20, 72);
        let filled = (pct * bar_width as f64).round() as usize;
        let state = if paused { "已暂停" } else { "正在播放" };
        let loop_state = if looping { "开启" } else { "关闭" };
        let title = clip(&self.title, self.width.saturating_sub(4));

        print!("\x1b[H\x1b[2J");
        println!("\x1b[1;36m  MUSIC RUST\x1b[0m  \x1b[2m音乐播放器\x1b[0m");
        println!("\x1b[2m  {}\x1b[0m", "─".repeat(self.width.saturating_sub(4)));
        println!("  \x1b[1m{}\x1b[0m", title);
        println!("  \x1b[2m{}  |  {}\x1b[0m", self.mode, state);
        println!();
        println!("  \x1b[36m[{}{}]\x1b[0m  \x1b[1m{:>5.1}%\x1b[0m", "█".repeat(filled), "░".repeat(bar_width - filled), pct * 100.0);
        println!("  \x1b[2m{} / {}\x1b[0m", format_time(elapsed_ms), format_time(total_ms));
        println!();
        println!("  \x1b[33m音量\x1b[0m {:>3}% / 500%    \x1b[35m循环\x1b[0m {}", volume, loop_state);
        if !detail.is_empty() {
            println!("  \x1b[2m{}\x1b[0m", clip(detail, self.width.saturating_sub(4)));
        }
        println!();
        let action = if paused { "播放" } else { "暂停" };
        println!("  \x1b[7m 播放 \x1b[0m  \x1b[7m 暂停 \x1b[0m    Enter/P 播放    Space {}    ← / → 快退/快进    9 / 0 音量    Q 退出", action);
        println!("  \x1b[2m鼠标：点击进度条跳转，点击状态栏切换播放\x1b[0m");
        let _ = io::stdout().flush();
    }

    pub fn mouse_control(&self, x: u16, y: u16, paused: bool) -> Control {
        // The bar is rendered on terminal row 6, with its first cell at column 3.
        if y == 6 {
            let bar_width = self.width.saturating_sub(8).clamp(20, 72);
            let offset = usize::from(x).saturating_sub(3).min(bar_width);
            return Control::SeekPercent(offset as f64 / bar_width as f64);
        }
        if y == 4 {
            return if paused { Control::Play } else { Control::Pause };
        }
        if y == 12 {
            return if x < 14 { Control::Play } else { Control::Pause };
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

fn format_time(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
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
        return usize::from(size.ws_col).max(40);
    }
    80
}

#[cfg(not(unix))]
fn terminal_width() -> usize { 80 }
