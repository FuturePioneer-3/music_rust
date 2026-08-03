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

//! 动态进度条模块

use std::io::{self, Write};

/// 动态进度条
pub struct Progress {
    width: usize,
    last_printed: bool,
    enabled: bool,
}

impl Progress {
    pub fn new(enabled: bool) -> Self {
        Progress {
            width: 40,
            last_printed: false,
            enabled,
        }
    }

    /// 更新进度（elapsed_ms: 已播放毫秒，total_ms: 总毫秒）
    /// 若 total_ms <= 0 或过小，仅按百分比显示。
    pub fn update(&mut self, elapsed_ms: u64, total_ms: u64) {
        if !self.enabled {
            return;
        }
        if total_ms == 0 {
            return;
        }
        let pct = (elapsed_ms as f64 / total_ms as f64).clamp(0.0, 1.0);
        let filled = (pct * self.width as f64) as usize;
        let filled = filled.min(self.width);
        let elapsed_sec = elapsed_ms as f64 / 1000.0;
        let total_sec = total_ms as f64 / 1000.0;
        let remaining_sec = (total_sec - elapsed_sec).max(0.0);

        let bar: String = format!(
            "\r[{}>{}] {:5.1}% | {:5.1}s / {:5.1}s | 剩余 {:4.1}s",
            "=".repeat(filled),
            " ".repeat(self.width - filled),
            pct * 100.0,
            elapsed_sec,
            total_sec,
            remaining_sec,
        );
        print!("{}", bar);
        let _ = io::stdout().flush();
        self.last_printed = true;
    }

    /// 结束时清空进度行
    pub fn finish(&mut self) {
        if self.last_printed && self.enabled {
            println!();
        }
        self.last_printed = false;
    }
}

