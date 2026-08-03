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

//! 极简调试日志模块

use std::sync::atomic::{AtomicBool, Ordering};

static VERBOSE: AtomicBool = AtomicBool::new(false);

pub fn init(verbose: bool) {
    VERBOSE.store(verbose, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

pub fn debug(msg: String) {
    if enabled() {
        eprintln!("[DBG] {}", msg);
    }
}

pub fn info(msg: String) {
    eprintln!("[INFO] {}", msg);
}

pub fn warn(msg: String) {
    eprintln!("[WARN] {}", msg);
}

pub fn error(msg: String) {
    eprintln!("[ERROR] {}", msg);
}
