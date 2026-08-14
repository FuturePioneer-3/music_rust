// music_rust —— 终端类型与能力检测
// Copyright (C) 2026 FuturePioneer-3
// SPDX-License-Identifier: GPL-3.0-or-later

//! 区分 pty（终端模拟器）与真实 tty（Linux 控制台/串口），并探测控制台能力。
//!
//! 真实控制台（/dev/ttyN、/dev/console、串口）与 pty 差异巨大：
//!   - 不支持真彩色（38;2），多数只支持 16 色 SGR
//!   - 字体通常只有 256/512 字形：无 CJK 汉字、无圆角框字符（╭─╮│）、
//!     无半块/方块字符（▀█░）
//!   - 可能未处于 UTF-8 模式（需 `ESC % G` 切换）
//!   - 无备用屏幕（1049）、无鼠标（1000/1006）
//!
//! 因此播放器在真实 tty 上应：
//!   1. 初始化基础中文环境：写 `ESC % G` 切 UTF-8，尽力用 setfont 加载
//!      CJK 字体（Unifont 等，失败静默）
//!   2. 用 KDFONTOP / GIO_UNIMAP 探测字体字形覆盖，决定渲染降级策略
//!      （中文 UI + 圆角框 vs 英文/ASCII UI + 16 色）
//!   3. 增量重绘（不整屏 2J 清屏），避免慢速控制台闪烁

use std::io::Write as _;

// ---- Linux 控制台 ioctl（见 <linux/kd.h>，新内核为裸常量）----
const KDFONTOP: libc::c_ulong = 0x4B72;
const GIO_UNIMAP: libc::c_ulong = 0x4B66;
const KD_FONT_OP_GET: libc::c_uint = 1;

#[repr(C)]
struct ConsoleFontOp {
    op: libc::c_uint,
    flags: libc::c_uint,
    width: libc::c_uint,
    height: libc::c_uint,
    charcount: libc::c_uint,
    data: *mut u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Unipair {
    unicode: libc::c_ushort,
    fontpos: libc::c_ushort,
}

#[repr(C)]
struct UnimapDesc {
    entry_ct: libc::c_ushort,
    entries: *mut Unipair,
}

/// 输出目标类型
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OutputKind {
    /// 终端模拟器（xterm/kitty/gnome-terminal...）：全部特性可用
    Pty,
    /// 真实 Linux 控制台 / 串口：需要降级
    Tty,
}

/// 控制台字体能力探测结果（仅 Tty 有意义）
#[derive(Clone, Copy, Debug)]
pub struct Caps {
    /// 字体字形数（256/512/1024...）；0 = 未知
    pub charcount: usize,
    pub has_cjk: bool,
    pub has_box: bool,
    pub has_block: bool,
    pub has_upper_half: bool,
    pub has_dot: bool,
    pub has_music: bool,
    pub has_arrow_l: bool,
    pub has_arrow_r: bool,
    pub has_middot: bool,
    pub has_ellipsis: bool,
}

impl Default for Caps {
    fn default() -> Self {
        // 探测失败时的保守值：按最朴素字体（256 字形、无扩展字符）处理
        Caps {
            charcount: 256,
            has_cjk: false,
            has_box: false,
            has_block: false,
            has_upper_half: false,
            has_dot: false,
            has_music: false,
            has_arrow_l: false,
            has_arrow_r: false,
            has_middot: false,
            has_ellipsis: false,
        }
    }
}

/// 判断 stdout 是否为真实 tty（Linux 控制台/串口）而非 pty。
pub fn output_kind() -> OutputKind {
    #[cfg(unix)]
    {
        unsafe {
            let name = libc::ttyname(libc::STDOUT_FILENO);
            if name.is_null() {
                return OutputKind::Pty; // 非终端；调用方已用 isatty 把关
            }
            let name = std::ffi::CStr::from_ptr(name).to_string_lossy();
            if name.starts_with("/dev/pts/") {
                OutputKind::Pty
            } else {
                OutputKind::Tty // /dev/ttyN、/dev/console、/dev/ttyS* 等
            }
        }
    }
    #[cfg(not(unix))]
    {
        OutputKind::Pty
    }
}

/// 初始化基础中文环境（仅真实控制台调用，全部尽力而为，失败静默）：
///   1. `ESC % G`：把控制台输出切到 UTF-8 模式（console_codes(4)）
///   2. 尽力用 setfont 加载 CJK 字体（Unifont 等），失败忽略
pub fn init_tty_environment() {
    // 1) UTF-8 输出模式
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(b"\x1b%G");
    let _ = stdout.flush();

    // 2) 尽力加载 CJK 控制台字体（需要 root/权限；找不到或失败都静默）
    #[cfg(unix)]
    {
        let dirs = [
            "/usr/share/kbd/consolefonts",
            "/usr/share/consolefonts",
            "/usr/share/kbd/consolefonts/Unifont",
        ];
        for dir in dirs {
            if let Ok(entries) = std::fs::read_dir(dir) {
                let mut fonts: Vec<String> = entries
                    .flatten()
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_lowercase();
                        // 只挑名称含 uni 的字体（Unifont 系是常见 CJK 控制台字体）
                        if name.contains("uni") {
                            Some(e.path().to_string_lossy().into_owned())
                        } else {
                            None
                        }
                    })
                    .collect();
                if fonts.is_empty() {
                    continue;
                }
                fonts.sort();
                let font = &fonts[0];
                let _ = std::process::Command::new("setfont")
                    .arg(font)
                    .status();
                return;
            }
        }
    }
}

/// 探测控制台字体能力（仅真实 tty；ioctl 失败返回保守默认）。
pub fn probe_caps() -> Caps {
    let mut caps = Caps::default();
    #[cfg(unix)]
    unsafe {
        let fd = libc::STDOUT_FILENO;

        // 1) 字体信息：KDFONTOP GET（charcount/尺寸）
        let mut cfo = ConsoleFontOp {
            op: KD_FONT_OP_GET,
            flags: 0,
            width: 0,
            height: 0,
            charcount: 0,
            data: std::ptr::null_mut(),
        };
        if libc::ioctl(fd, KDFONTOP, &mut cfo) == 0 && cfo.charcount > 0 {
            caps.charcount = cfo.charcount as usize;
        } else {
            return caps; // 连字体都读不到：保持保守
        }

        // 2) Unicode → 字形映射表：GIO_UNIMAP（两次调用：先取数量，再取数据）
        let mut desc = UnimapDesc {
            entry_ct: 0,
            entries: std::ptr::null_mut(),
        };
        if libc::ioctl(fd, GIO_UNIMAP, &mut desc) != 0 {
            // 读不到映射表：按 charcount 粗略推断
            caps.has_box = caps.charcount >= 512;
            caps.has_block = caps.charcount >= 512;
            caps.has_upper_half = caps.charcount >= 512;
            return caps;
        }
        let count = desc.entry_ct as usize;
        if count == 0 || count > 65535 {
            return caps;
        }
        let mut entries: Vec<Unipair> = vec![Unipair { unicode: 0, fontpos: 0 }; count];
        desc.entries = entries.as_mut_ptr();
        if libc::ioctl(fd, GIO_UNIMAP, &mut desc) != 0 {
            return caps;
        }
        let mapped = |cp: u32| -> bool {
            entries
                .iter()
                .any(|u| u32::from(u.unicode) == cp && u.fontpos != 0)
        };
        caps.has_cjk = mapped(0x4E2D); // 中
        caps.has_box = mapped(0x256D) || mapped(0x2502); // ╭ 或 │
        caps.has_block = mapped(0x2588) && mapped(0x2591); // █ 与 ░
        caps.has_upper_half = mapped(0x2580); // ▀
        caps.has_dot = mapped(0x25CF); // ●
        caps.has_music = mapped(0x266A); // ♪
        caps.has_arrow_l = mapped(0x2190); // ←
        caps.has_arrow_r = mapped(0x2192); // →
        caps.has_middot = mapped(0x00B7); // ·
        caps.has_ellipsis = mapped(0x2026); // …
    }
    caps
}
