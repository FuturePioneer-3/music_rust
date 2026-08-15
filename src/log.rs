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

//! 自研彩色分级日志系统。
//!
//! 设计要点：
//!   - 分级着色：TRACE / DEBUG / INFO / WARN / ERROR，终端友好
//!   - 时间戳：HH:MM:SS.mmm（自进程启动计时）
//!   - 环形缓冲：最多保留 [`RING_CAP`] 条日志，TUI 的日志面板实时取用，
//!     播放过程中即使不输出到终端也不会丢日志
//!   - 双路输出：TUI 激活时只进环形缓冲（避免污染备用屏画面）；
//!     未激活时输出到 stderr（终端自动着色，管道无色）
//!   - 与进度条的行同步保持原样：日志输出前先清掉进度条行

use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// 日志级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[allow(dead_code)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    /// 短标签（定宽 5）
    pub fn tag(self) -> &'static str {
        match self {
            Level::Trace => "TRACE",
            Level::Debug => "DBG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }

    /// stderr 着色（ANSI 16 色，兼容性最好）
    pub fn ansi(self) -> &'static str {
        match self {
            Level::Trace => "\x1b[2m",
            Level::Debug => "\x1b[36m",
            Level::Info => "\x1b[1;36m",
            Level::Warn => "\x1b[1;33m",
            Level::Error => "\x1b[1;31m",
        }
    }

    /// TUI 256 色面板着色
    #[allow(dead_code)]
    pub fn tui_color(self) -> u8 {
        match self {
            Level::Trace => 245,
            Level::Debug => 117,
            Level::Info => 45,
            Level::Warn => 220,
            Level::Error => 196,
        }
    }

    fn verbose_only(self) -> bool {
        matches!(self, Level::Trace | Level::Debug)
    }
}

/// 一条日志（环形缓冲项）
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct LogLine {
    /// 距进程启动的毫秒数
    pub time_ms: u64,
    pub level: Level,
    pub msg: String,
}

struct LogState {
    verbose: bool,
    ring: VecDeque<LogLine>,
}

/// 环形缓冲容量：TUI 日志面板最多展示这些条
const RING_CAP: usize = 300;

static STATE: Mutex<LogState> = Mutex::new(LogState {
    verbose: false,
    ring: VecDeque::new(),
});

static START: OnceLock<Instant> = OnceLock::new();

/// stderr 是否为终端（决定是否着色）
static STDERR_TTY: AtomicBool = AtomicBool::new(false);

/// 进度条是否处于活动状态（由进度条模块置位）。
/// 活动期间日志输出必须先结束进度条行（清行 + 换行），
/// 否则日志会接在进度条行尾，且旧进度条行会残留堆积。
static PROGRESS_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 自进度条上次渲染后是否输出过日志。
/// 进度条据此决定：在新行绘制，而非覆盖当前行。
static LOG_AFTER_PROGRESS: AtomicBool = AtomicBool::new(false);

/// TUI 是否处于活动状态：激活时日志只入环形缓冲，不写 stderr
static TUI_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 初始化日志系统
pub fn init(verbose: bool) {
    START.get_or_init(Instant::now);
    #[cfg(unix)]
    STDERR_TTY.store(
        unsafe { libc::isatty(libc::STDERR_FILENO) == 1 },
        Ordering::Relaxed,
    );
    STATE.lock().unwrap().verbose = verbose;
}

#[allow(dead_code)]
pub fn enabled() -> bool {
    STATE.lock().unwrap().verbose
}

/// 设置进度条活动状态（progress.rs 在创建/结束时调用）
pub fn set_progress_active(active: bool) {
    PROGRESS_ACTIVE.store(active, Ordering::Relaxed);
}

/// 查询并清除"进度条渲染后是否输出过日志"标志
pub fn consume_log_pending() -> bool {
    LOG_AFTER_PROGRESS.swap(false, Ordering::Relaxed)
}

/// TUI 激活状态（tui.rs 在创建/销毁时调用）
#[allow(dead_code)]
pub fn set_tui_active(active: bool) {
    TUI_ACTIVE.store(active, Ordering::Relaxed);
}

#[allow(dead_code)]
pub fn tui_active() -> bool {
    TUI_ACTIVE.load(Ordering::Relaxed)
}

/// 取环形缓冲快照（TUI 日志面板使用；最新一条在末尾）
#[allow(dead_code)]
pub fn snapshot() -> Vec<LogLine> {
    STATE.lock().unwrap().ring.iter().cloned().collect()
}

fn now_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

fn emit(level: Level, msg: &str) {
    let mut st = STATE.lock().unwrap();
    if level.verbose_only() && !st.verbose {
        return;
    }
    let line = LogLine {
        time_ms: now_ms(),
        level,
        msg: msg.to_string(),
    };
    // 1) 永远入环形缓冲（TUI 日志面板 + 退出后复盘）
    st.ring.push_back(line.clone());
    if st.ring.len() > RING_CAP {
        st.ring.pop_front();
    }
    // 2) TUI 激活时不写 stderr（避免破坏备用屏）
    if TUI_ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    if PROGRESS_ACTIVE.load(Ordering::Relaxed) {
        // 结束进度条行：清空该行并换行，日志另起一行
        eprint!("\r\x1b[2K\n");
        LOG_AFTER_PROGRESS.store(true, Ordering::Relaxed);
    }
    drop(st); // 锁内不写 IO
    if STDERR_TTY.load(Ordering::Relaxed) {
        eprintln!(
            "{}[{}] [{}] {}\x1b[0m",
            level.ansi(),
            format_time(now_ms()),
            level.tag(),
            msg
        );
    } else {
        eprintln!("[{}] [{}] {}", format_time(now_ms()), level.tag(), msg);
    }
}

/// 毫秒 → HH:MM:SS.mmm
fn format_time(ms: u64) -> String {
    let h = ms / 3_600_000;
    let m = (ms % 3_600_000) / 60_000;
    let s = (ms % 60_000) / 1000;
    let mm = ms % 1000;
    format!("{:02}:{:02}:{:02}.{:03}", h, m, s, mm)
}

#[allow(dead_code)]
pub fn trace(msg: String) {
    emit(Level::Trace, &msg);
}

pub fn debug(msg: String) {
    emit(Level::Debug, &msg);
}

pub fn info(msg: String) {
    emit(Level::Info, &msg);
}

pub fn warn(msg: String) {
    emit(Level::Warn, &msg);
}

pub fn error(msg: String) {
    emit(Level::Error, &msg);
}

/// 供外部（如进度条模块）同步刷新 stderr
#[allow(dead_code)]
pub fn flush_stderr() {
    let _ = io::stderr().flush();
}
