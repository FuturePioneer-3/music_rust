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

//! 非阻塞键盘输入模块。
//!
//! 在播放过程中监听用户按键，实现快进/后退/暂停/循环等控制（类似 mpv）。
//! 仅在 stdin 为终端时启用；管道/重定向输入时不干扰。
//!
//! 按键映射（参考 mpv）：
//!   ← →             后退/快进 5 秒
//!   ↑ ↓             快进/后退 10 秒
//!   PageUp/PageDown 快进/后退 1 分钟
//!   空格 / P        暂停 / 继续
//!   [ ]             后退/快进 1 秒（微调）
//!   R              切换循环播放
//!   Q              退出
//!   数字 1-9        快进到 10%..90% 进度
//!   9               降低音量
//!   0               增加音量

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 从键盘读取到的控制指令
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Control {
    /// 暂停 / 继续
    Pause,
    /// 开始 / 继续播放
    Play,
    /// 快进 n 秒（n > 0）
    SeekForward(f64),
    /// 后退 n 秒（n > 0）
    SeekBackward(f64),
    /// 跳转到百分比（0.0 - 1.0）
    SeekPercent(f64),
    /// 切换循环播放
    Loop,
    /// 降低音量
    VolumeDown,
    /// 增加音量
    VolumeUp,
    /// 终端鼠标左键点击位置（从 1 开始的列、行）
    Mouse(u16, u16),
    /// 退出
    Quit,
    /// 无输入
    None,
}

/// 终端 raw mode 封装：开启后 stdin 按键即时可用（无需回车）。
#[cfg(unix)]
pub struct TermMode {
    enabled: bool,
    orig: libc::termios,
}

#[cfg(unix)]
impl TermMode {
    pub fn enable() -> TermMode {
        unsafe {
            let mut orig: libc::termios = std::mem::zeroed();
            // 非终端（stdin 被重定向）时不启用
            if libc::isatty(libc::STDIN_FILENO) != 1 {
                return TermMode { enabled: false, orig };
            }
            if libc::tcgetattr(libc::STDIN_FILENO, &mut orig) != 0 {
                return TermMode { enabled: false, orig };
            }
            let mut raw = orig;
            raw.c_lflag &= !(libc::ICANON | libc::ECHO);
            raw.c_cc[libc::VMIN] = 0;  // 非阻塞：无输入立即返回 0
            raw.c_cc[libc::VTIME] = 0; // 不等待
            if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) != 0 {
                return TermMode { enabled: false, orig };
            }
            TermMode { enabled: true, orig }
        }
    }

    pub fn disable(&self) {
        if self.enabled {
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.orig);
            }
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

impl Drop for TermMode {
    fn drop(&mut self) {
        self.disable();
    }
}

#[cfg(not(unix))]
pub struct TermMode;

#[cfg(not(unix))]
impl TermMode {
    pub fn enable() -> TermMode {
        TermMode
    }
    pub fn disable(&self) {}
    pub fn enabled(&self) -> bool {
        false
    }
}

/// 按键监听器：独立线程读取 stdin，解析按键并存入指令队列。
pub struct InputListener {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    queue: Arc<Mutex<VecDeque<Control>>>,
    term: TermMode,
}

/// 从 fd 读取最多 n 字节（非阻塞）。返回实际读取字节数（0 = 暂无输入）。
#[cfg(unix)]
fn raw_read(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    let r = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr() as *mut libc::c_void, n) };
    if r <= 0 {
        return Vec::new();
    }
    buf.truncate(r as usize);
    buf
}

impl InputListener {
    /// 启动按键监听。非终端环境自动禁用。
    pub fn start() -> InputListener {
        let term = TermMode::enable();
        let enabled = term.enabled();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let queue: Arc<Mutex<VecDeque<Control>>> = Arc::new(Mutex::new(VecDeque::new()));
        let queue2 = queue.clone();

        let handle = std::thread::spawn(move || {
            if !enabled {
                return;
            }
            let mut pending: Vec<u8> = Vec::new();
            loop {
                if stop2.load(Ordering::Relaxed) {
                    break;
                }
                #[cfg(unix)]
                let bytes = raw_read(32);
                #[cfg(not(unix))]
                let bytes: Vec<u8> = Vec::new();

                if bytes.is_empty() {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    continue;
                }
                pending.extend(bytes);
                // 从 pending 中逐个解析按键（可能包含转义序列）：
                // 每次解析出一个完整按键就入队，剩下的字节留到下一轮；
                // 解析不了说明序列还不完整（比如 ESC 后面还在等方向键码）。
                while let (Some(ctrl), rest) = parse_key(&pending) {
                    queue2.lock().unwrap().push_back(ctrl);
                    pending = rest;
                }
                // pending 里残留的不完整转义序列（如 ESC 之后还在等待后续），
                // 若太长则丢弃，避免无限累积
                if pending.len() > 8 {
                    pending.clear();
                }
            }
        });

        InputListener { stop, handle: Some(handle), queue, term }
    }

    /// 取出一条指令（无则返回 Control::None）
    pub fn poll(&self) -> Control {
        self.queue.lock().unwrap().pop_front().unwrap_or(Control::None)
    }

    /// 停止监听线程并恢复终端
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        self.term.disable();
    }
}

impl Drop for InputListener {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 从 pending 字节流中解析一个按键。
/// 返回 (指令, 剩余字节)。若输入不完整返回 (None, pending)。
fn parse_key(pending: &[u8]) -> (Option<Control>, Vec<u8>) {
    if pending.is_empty() {
        return (None, pending.to_vec());
    }
    // 普通按键（非转义）
    if pending[0] != 0x1b {
        let c = pending[0];
        let ctrl = match c {
            b' ' | b'p' => Control::Pause,
            b'P' | b'\r' | b'\n' => Control::Play,
            b'r' | b'R' => Control::Loop,
            b'q' | b'Q' => Control::Quit,
            b'[' => Control::SeekBackward(1.0),
            b']' => Control::SeekForward(1.0),
            b'1'..=b'8' => Control::SeekPercent((c - b'0') as f64 / 10.0),
            b'9' => Control::VolumeDown,
            b'0' => Control::VolumeUp,
            _ => Control::None,
        };
        return (Some(ctrl), pending[1..].to_vec());
    }

    // ESC 开头：可能是转义序列（方向键等），也可能是单独的 ESC
    if pending.len() < 2 {
        return (None, pending.to_vec()); // 等待更多字节
    }
    // ESC [ ... 控制序列
    if pending[1] == b'[' {
        if pending.len() < 3 {
            return (None, pending.to_vec());
        }
        if pending[2] == b'<' {
            // SGR 鼠标格式：ESC [ < button;x;y M（按下）或 m（释放）。
            let end = match pending.iter().position(|b| *b == b'M' || *b == b'm') {
                Some(v) => v,
                None => return (None, pending.to_vec()),
            };
            let fields = std::str::from_utf8(&pending[3..end]).ok()
                .and_then(|v| {
                    let mut values = v.split(';').filter_map(|part| part.parse::<u16>().ok());
                    Some((values.next()?, values.next()?, values.next()?))
                });
            let ctrl = match (fields, pending[end]) {
                (Some((0, x, y)), b'M') => Control::Mouse(x, y),
                _ => Control::None,
            };
            return (Some(ctrl), pending[end + 1..].to_vec());
        }
        let cmd = pending[2];
        let ctrl = match cmd {
            b'A' => Control::SeekForward(10.0),
            b'B' => Control::SeekBackward(10.0),
            b'C' => Control::SeekForward(5.0),
            b'D' => Control::SeekBackward(5.0),
            b'5' | b'3' => {
                // PageUp: ESC [ 5 ~ 或 ESC [ 3 ~
                if pending.len() >= 4 && pending[3] == b'~' {
                    Control::SeekForward(60.0)
                } else {
                    return (None, pending.to_vec());
                }
            }
            b'6' => {
                // PageDown: ESC [ 6 ~
                if pending.len() >= 4 && pending[3] == b'~' {
                    Control::SeekBackward(60.0)
                } else {
                    return (None, pending.to_vec());
                }
            }
            _ => Control::None,
        };
        // 已消费 ESC [ cmd（PageUp/Down 多消费一个 ~）
        let consumed = match cmd {
            b'5' | b'6' | b'3' => 4,
            _ => 3,
        };
        let rest = if consumed <= pending.len() {
            pending[consumed..].to_vec()
        } else {
            Vec::new()
        };
        return (Some(ctrl), rest);
    }

    // 其它 ESC 序列：暂不识别，跳过
    (Some(Control::None), pending[1..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::{parse_key, Control};

    #[test]
    fn parses_sgr_left_mouse_press() {
        let (control, rest) = parse_key(b"\x1b[<0;17;6Mnext");
        assert_eq!(control, Some(Control::Mouse(17, 6)));
        assert_eq!(rest, b"next");
    }
}
