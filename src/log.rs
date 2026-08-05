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

/// 进度条是否处于活动状态（由进度条模块置位）。
/// 活动期间日志输出必须先结束进度条行（清行 + 换行），
/// 否则日志会接在进度条行尾，且旧进度条行会残留堆积。
static PROGRESS_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 自进度条上次渲染后是否输出过日志。
/// 进度条据此决定：在新行绘制，而非覆盖当前行。
static LOG_AFTER_PROGRESS: AtomicBool = AtomicBool::new(false);

pub fn init(verbose: bool) {
    VERBOSE.store(verbose, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

/// 设置进度条活动状态（progress.rs 在创建/结束时调用）
pub fn set_progress_active(active: bool) {
    PROGRESS_ACTIVE.store(active, Ordering::Relaxed);
}

/// 查询并清除"进度条渲染后是否输出过日志"标志
pub fn consume_log_pending() -> bool {
    LOG_AFTER_PROGRESS.swap(false, Ordering::Relaxed)
}

fn emit(prefix: &str, msg: &str) {
    if PROGRESS_ACTIVE.load(Ordering::Relaxed) {
        // 结束进度条行：清空该行并换行，日志另起一行
        eprint!("\r\x1b[2K\n");
        LOG_AFTER_PROGRESS.store(true, Ordering::Relaxed);
    }
    eprintln!("[{}] {}", prefix, msg);
}

pub fn debug(msg: String) {
    if enabled() {
        emit("DBG", &msg);
    }
}

pub fn info(msg: String) {
    emit("INFO", &msg);
}

pub fn warn(msg: String) {
    emit("WARN", &msg);
}

pub fn error(msg: String) {
    emit("ERROR", &msg);
}
