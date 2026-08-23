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

//! 播放时使用的轻量终端界面（2.4.0 全面重绘；2.4.1 支持真实 tty 降级）。
//!
//! 不依赖第三方终端库，避免增加运行时依赖；仅在 stdout 为终端时启用。
//!
//! 布局（自上而下，小屏自动精简）：
//!   顶框 → 标题 → 状态行 → 进度条 → 时间 → 音量 → 动态 EQ（可选）
//!   → 音符详情（可选）→ 专辑封面 + 作曲家等元数据（可选）→ 按键提示 → 底框
//!
//! ## pty vs 真实 tty（2.4.1）
//! pty（终端模拟器）：真彩色 + 半块封面 + 每帧整屏重绘（与 2.4.0 完全一致）。
//! 真实 tty（Linux 控制台/串口）：
//!   - 初始化基础中文环境：`ESC % G` 切 UTF-8，尽力 setfont 加载 CJK 字体
//!   - 用 KDFONTOP/GIO_UNIMAP 探测字体字形（CJK/圆角框/方块/箭头…），
//!     按能力降级：16 色 SGR、ASCII 边框与字符、英文标签、亮度字符封面
//!   - 增量重绘（仅刷新变化的行 + 光标定位），避免整屏清屏闪烁
//!
//! 测试钩子（内部）：MUSIC_FORCE_TTY=1 强制走 tty 路径；
//! MUSIC_FORCE_TTY_CJK=1 在强制 tty 下模拟 CJK 字体齐全的控制台。

use std::io::{self, Write};
use std::time::{Duration, Instant};

use crate::console::{self, Caps, OutputKind};
use crate::input::Control;

/// GitHub 头像由构建时固定嵌入 ELF，不依赖运行时网络访问。
const GITHUB_AVATAR_RGBA: &[u8] = include_bytes!("../assets/github_avatar.rgba");
const GITHUB_AVATAR_SIZE: usize = 96;

/// 内嵌封面：RGBA8 像素（宽 × 高），由 C 侧解码并缩放到 ≤96px。
/// 定义在本模块（而非 audio_file），使 selftest 等通过 `#[path]`
/// 复用 tui.rs 的二进制无需链接 audio_file。
#[derive(Clone)]
pub struct ArtImage {
    pub data: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// 返回项目所有者的内嵌 GitHub 头像，供无专辑封面的音频显示。
pub fn github_avatar() -> ArtImage {
    ArtImage {
        data: GITHUB_AVATAR_RGBA.to_vec(),
        width: GITHUB_AVATAR_SIZE,
        height: GITHUB_AVATAR_SIZE,
    }
}

// ---------------------------------------------------------------------------
// 调色板：pty 用真彩色，真实 tty 用 16 色（内核控制台不支持 38;2）
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, PartialEq, Eq)]
enum NamedColor {
    Cyan, CyanDeep, Magenta, Green, GreenDim, Yellow, Red, Gray, GrayDim,
}

const T_CYAN: &str = "\x1b[38;2;0;215;255m";
const T_CYAN_DEEP: &str = "\x1b[38;2;0;170;230m";
const T_MAGENTA: &str = "\x1b[38;2;255;95;215m";
const T_GREEN: &str = "\x1b[38;2;110;255;170m";
const T_GREEN_DIM: &str = "\x1b[38;2;80;200;120m";
const T_YELLOW: &str = "\x1b[38;2;255;215;0m";
const T_RED: &str = "\x1b[38;2;255;95;95m";
const T_GRAY: &str = "\x1b[38;2;140;140;150m";
const T_GRAY_DIM: &str = "\x1b[38;2;90;90;100m";

const C_CYAN: &str = "\x1b[96m";
const C_CYAN_DEEP: &str = "\x1b[36m";
const C_MAGENTA: &str = "\x1b[95m";
const C_GREEN: &str = "\x1b[92m";
const C_GREEN_DIM: &str = "\x1b[32m";
const C_YELLOW: &str = "\x1b[93m";
const C_RED: &str = "\x1b[91m";
const C_GRAY: &str = "\x1b[37m";
const C_GRAY_DIM: &str = "\x1b[90m";

/// EQ 柱颜色（自下而上 level 1..7）
const T_EQ: [&str; 7] = [
    "\x1b[38;2;70;100;160m",  // 1 深蓝
    "\x1b[38;2;90;130;200m",  // 2
    "\x1b[38;2;0;170;230m",   // 3 青
    "\x1b[38;2;0;215;255m",   // 4 亮青
    "\x1b[38;2;110;255;170m", // 5 绿
    "\x1b[38;2;255;215;0m",   // 6 黄
    "\x1b[38;2;255;95;95m",   // 7 红
];
const C_EQ: [&str; 7] = [
    "\x1b[94m", "\x1b[36m", "\x1b[96m", "\x1b[96m", "\x1b[92m", "\x1b[93m", "\x1b[91m",
];

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

/// 每根 EQ 柱在标签行中的中心列（内容内偏移，不含 EQ_LABEL_PAD 缩进）：
/// 柱 i 对准标签 i 的中点，偶数宽标签取左中点，避免向右侧字符偏移。
fn freq_bar_cols() -> [usize; 16] {
    let mut cols = [0usize; 16];
    let mut c = 0usize;
    for (i, l) in FREQ_LABELS.iter().enumerate() {
        cols[i] = c + (l.len().saturating_sub(1)) / 2;
        // 标签间单空格；最后一个标签后无空格（与 FREQ_LABELS.join(" ") 一致）
        c += l.len() + usize::from(i < 15);
    }
    cols
}

/// 进度条 1/8 精度分块字符（仅 pty 或字形齐全的控制台）
const PARTIAL_BLOCKS: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

// ---------------------------------------------------------------------------
// 渲染风格：字形 + 调色板 + 语言
// ---------------------------------------------------------------------------

struct BoxChars {
    tl: &'static str,
    tr: &'static str,
    bl: &'static str,
    br: &'static str,
    h: &'static str,
    v: &'static str,
}

impl BoxChars {
    const ROUNDED: BoxChars = BoxChars { tl: "╭", tr: "╮", bl: "╰", br: "╯", h: "─", v: "│" };
    const ASCII: BoxChars = BoxChars { tl: "+", tr: "+", bl: "+", br: "+", h: "-", v: "|" };
}

struct Palette {
    sgr: [&'static str; 9],
    eq: [&'static str; 7],
}

impl Palette {
    fn full() -> Palette {
        Palette {
            sgr: [T_CYAN, T_CYAN_DEEP, T_MAGENTA, T_GREEN, T_GREEN_DIM, T_YELLOW, T_RED, T_GRAY, T_GRAY_DIM],
            eq: T_EQ,
        }
    }
    fn basic() -> Palette {
        Palette {
            sgr: [C_CYAN, C_CYAN_DEEP, C_MAGENTA, C_GREEN, C_GREEN_DIM, C_YELLOW, C_RED, C_GRAY, C_GRAY_DIM],
            eq: C_EQ,
        }
    }
    fn col(&self, c: NamedColor) -> &'static str {
        self.sgr[c as usize]
    }
}

/// 一套完整的界面风格（字形/颜色/语言/渲染方式）
struct Style {
    p: Palette,
    box_: BoxChars,
    /// 是否使用中文界面（真实 tty 字体无 CJK 字形时降级英文）
    cjk: bool,
    /// pty 真彩色半块封面 vs tty 亮度字符封面
    truecolor: bool,
    /// 是否可用方块/块元素字形（█░▀▏…）
    block: bool,
    prog_full: char,
    prog_empty: char,
    marker: char,
    dot: char,
    music: char,
    middot: char,
    ellipsis: &'static str,
    arrow_l: &'static str,
    arrow_r: &'static str,
    bar_fill: char,
    bar_empty: char,
    /// 数值区间分隔符（如 20Hz–10kHz）：pty 用 en-dash，tty 用 ASCII '-'
    range: &'static str,
}

impl Style {
    /// pty：完整特性（与 2.4.0 渲染完全一致）
    fn full() -> Style {
        Style {
            p: Palette::full(),
            box_: BoxChars::ROUNDED,
            cjk: true,
            truecolor: true,
            block: true,
            prog_full: '█',
            prog_empty: '░',
            marker: '▸',
            dot: '●',
            music: '♪',
            middot: '·',
            ellipsis: "…",
            arrow_l: "←",
            arrow_r: "→",
            bar_fill: '▓',
            bar_empty: '░',
            range: "–",
        }
    }

    /// 真实 tty：按字体能力降级
    fn console(caps: Caps) -> Style {
        let block = caps.has_block;
        Style {
            p: Palette::basic(),
            box_: if caps.has_box { BoxChars::ROUNDED } else { BoxChars::ASCII },
            cjk: caps.has_cjk,
            truecolor: false,
            block,
            prog_full: if block { '█' } else { '#' },
            prog_empty: if block { '░' } else { '.' },
            marker: '>',
            dot: if caps.has_dot { '●' } else { '*' },
            music: if caps.has_music { '♪' } else { '>' },
            middot: if caps.has_middot { '·' } else { '|' },
            ellipsis: if caps.has_ellipsis { "…" } else { "..." },
            arrow_l: if caps.has_arrow_l { "←" } else { "<-" },
            arrow_r: if caps.has_arrow_r { "→" } else { "->" },
            bar_fill: if block { '█' } else { '#' },
            bar_empty: if block { '░' } else { '.' },
            range: "-",
        }
    }

    /// 中英标签：tty 无 CJK 字形时用英文，避免乱码
    fn tr<'a>(&self, zh: &'a str, en: &'static str) -> &'a str {
        if self.cjk { zh } else { en }
    }

    /// 模式名翻译（来自 main/synth 的固定字符串）
    fn tr_mode(&self, mode: &'static str) -> &'static str {
        if self.cjk {
            mode
        } else {
            match mode {
                "音乐文件" => "Music File",
                "MIDI 音乐" => "MIDI Music",
                "简谱" => "Score",
                _ => mode,
            }
        }
    }

    /// 不可变文本净化：无 CJK 字形时把非 ASCII 字符替换为 '?'，防止乱码
    fn safe(&self, s: &str) -> String {
        if self.cjk {
            s.to_string()
        } else {
            s.chars()
                .map(|c| if c.is_ascii() { c } else { '?' })
                .collect()
        }
    }
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
    logo: ArtImage,
    art: Option<ArtImage>,
    meta: MetaInfo,
    kind: OutputKind,
    style: Style,
    /// 1-based 行号（鼠标 SGR 坐标）：点击状态行切换播放/暂停
    status_row: u16,
    /// 1-based 行号：点击进度条跳转
    bar_row: u16,
    /// 增量重绘：上一帧各行（仅 tty 使用）
    prev_lines: Vec<String>,
    /// 渲染节流（tty 100ms 一帧，慢速控制台防闪烁；pty 不限）
    throttle: Duration,
    last_emit: Instant,
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
        // 2.4.1：区分 pty 与真实 tty；tty 初始化中文环境并按字体能力降级
        let mut kind = console::output_kind();
        let mut caps = Caps::default();
        let forced = std::env::var("MUSIC_FORCE_TTY").is_ok();
        if forced {
            kind = OutputKind::Tty;
            if std::env::var("MUSIC_FORCE_TTY_CJK").is_ok() {
                caps = Caps {
                    charcount: 1024,
                    has_cjk: true,
                    has_box: true,
                    has_block: true,
                    has_upper_half: true,
                    has_dot: true,
                    has_music: true,
                    has_arrow_l: true,
                    has_arrow_r: true,
                    has_middot: true,
                    has_ellipsis: true,
                };
            }
        }
        let (style, throttle) = match kind {
            OutputKind::Pty => (Style::full(), Duration::ZERO),
            OutputKind::Tty => {
                if !forced {
                    console::init_tty_environment();
                    caps = console::probe_caps();
                }
                (Style::console(caps), Duration::from_millis(100))
            }
        };
        let mut tui = Self {
            title: title.to_string(),
            mode,
            width: terminal_width(),
            height: terminal_height(),
            active: true,
            logo: github_avatar(),
            art,
            meta,
            kind,
            style,
            status_row: 3, // 状态行 = 0-based 第 2 行
            bar_row: 4,    // 进度条 = 0-based 第 3 行
            prev_lines: Vec::new(),
            throttle,
            // 初始化为过去的时间点，保证第一帧不被节流跳过
            last_emit: Instant::now() - throttle - Duration::from_millis(1),
        };
        // 屏幕初始化：pty 用备用屏 + 鼠标；tty 不支持，仅清屏 + 隐藏光标
        match kind {
            OutputKind::Pty => {
                print!("\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1006h\x1b[2J\x1b[H");
            }
            OutputKind::Tty => {
                print!("\x1b[2J\x1b[H\x1b[?25l");
            }
        }
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
        let st = &self.style;

        // ---- 自适应布局 ----
        let need_details = details.len().min(4);
        let footer_rows = if h >= 19 { 2 } else { 1 };
        // 固定主体：顶框 + 标题 + 状态 + 进度 + 时间 + 音量 + 脚注 + 底框。
        // 顶部头像曾未计入高度预算，音频模式再加封面后会把整帧撑出屏幕并
        // 触发终端滚屏，导致屏幕上的进度条行落到逻辑“暂停行”的坐标。
        let core_rows = 7 + footer_rows;
        let logo_max_w = if inner >= 64 { 18 } else { 12 };
        let preferred_logo_h = if h >= 18 { 8 } else { 6 };
        let logo_room = h.saturating_sub(core_rows);
        let (logo_w, logo_h) = if logo_room >= 2 {
            art_size(&self.logo, logo_max_w, preferred_logo_h.min(logo_room - 1))
        } else {
            (0, 0)
        };
        let logo_rows = if logo_h > 0 { logo_h + 1 } else { 0 }; // 图像 + 标签
        let fixed = core_rows + logo_rows;
        let meta = self.meta.lines();
        let meta_w = if !meta.is_empty() { (inner / 3).clamp(14, 30) } else { 0 };

        // 封面区：宽 ≤ 46 列，高 ≤ 16 行且不超过剩余空间的 45%（小一点），
        // 小屏保底 6 行（不要太小）。元数据并排或下置。
        let mut art_disp: Option<(usize, usize)> = None; // (显示宽, 显示行)
        let mut art_side = false;
        let mut section_rows = 0usize;
        if let Some(img) = &self.art {
            let max_art = ((h.saturating_sub(fixed)) as f64 * 0.45) as usize;
            let search_max = max_art.min(16);
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
        }
        // 封面放不下时仍可显示元数据，但必须受剩余高度约束。
        if art_disp.is_none() && !meta.is_empty() {
            section_rows = meta.len().min(4).min(h.saturating_sub(fixed));
        }

        let left = h.saturating_sub(fixed + section_rows);
        let eq_rows = if left >= 8 { 8 } else { 0 };
        let det_rows = if left.saturating_sub(eq_rows) >= need_details { need_details } else { 0 };

        // ---- 渲染 ----
        let mut frame: Vec<String> = Vec::with_capacity(h);

        // 顶框
        let header = format!(
            "{} {} {} {}",
            st.music, "MUSIC RUST", st.middot, st.tr("音乐播放器", "Music Player"));
        let head_prefix = format!("{}{} {} ", st.box_.tl, st.box_.h, header);
        let fill = w.saturating_sub(disp_width(&head_prefix) + 1);
        frame.push(format!(
            "{}{}{}{}{}",
            st.p.col(NamedColor::Cyan), head_prefix, st.box_.h.repeat(fill), st.box_.tr, RESET));

        // 启动 logo：始终显示编译进 ELF 的 GitHub 头像，作为界面标识。
        // 采用近方形布局，避免横向横幅挤压主体信息。
        let logo_label = format!("{}  {}", st.tr("GitHub 头像", "GitHub Avatar"), "FuturePioneer-3");
        // 1-based 鼠标坐标：top=1，logo 行随后，logo label/title/status/bar 依次向下排布。
        self.status_row = (3 + logo_rows) as u16;
        self.bar_row = (4 + logo_rows) as u16;
        for row in 0..logo_h {
            let logo_body = if st.truecolor {
                render_art_row(&self.logo, row, logo_w, logo_h)
            } else {
                render_art_row_ascii(&self.logo, row, logo_w, logo_h)
            };
            let line_text = format!("{}{}{}", st.p.col(NamedColor::Magenta), logo_body, RESET);
            frame.push(line(inner, st, &line_text));
        }
        if logo_h > 0 {
            frame.push(line(inner, st, &format!("{}{}{} {}", st.p.col(NamedColor::Magenta), st.music, RESET, logo_label)));
        }

        // 标题行
        let title_disp = clip_w(&st.safe(&self.title), inner - 2, st.ellipsis);
        let (b_l, b_r) = if st.cjk { ("《", "》") } else { ("", "") };
        frame.push(line(inner, st, &format!(
            "{}{}{} {}{}{}{}{}",
            st.p.col(NamedColor::Magenta), st.music, RESET, BOLD, b_l, title_disp, b_r, RESET)));

        // 状态行（鼠标点击切换播放/暂停）
        let (dot_color, dot, state_color, state_text) = if paused {
            (NamedColor::Red, st.dot, NamedColor::Yellow, st.tr("已暂停", "Paused"))
        } else {
            (NamedColor::Green, st.dot, NamedColor::Green, st.tr("正在播放", "Playing"))
        };
        let loop_text = if looping { st.tr("开", "ON") } else { st.tr("关", "OFF") };
        let loop_color = if looping { NamedColor::Green } else { NamedColor::GrayDim };
        let mode_part = format!("{}{}{}", st.p.col(NamedColor::CyanDeep), st.tr_mode(self.mode), RESET);
        let dot_part = format!("{}{}{}", st.p.col(dot_color), dot, RESET);
        let state_part = format!("{}{}{}", st.p.col(state_color), state_text, RESET);
        let loop_part = format!(
            "{}{}{} {}{}{}",
            st.p.col(NamedColor::GrayDim), RESET, st.tr("循环", "Loop"),
            st.p.col(loop_color), loop_text, RESET);
        frame.push(line(inner, st, &format!("{}  {}   {}{}", mode_part, dot_part, state_part, loop_part)));

        // 进度条（鼠标点击跳转）
        let bar_w = (inner.saturating_sub(12)).clamp(8, 72);
        let filled = pct * bar_w as f64;
        let whole = filled.floor() as usize;
        let frac = filled - whole as f64;
        let partial = if frac > 0.0 && st.block {
            PARTIAL_BLOCKS[((frac * 8.0).round() as usize).min(7)].to_string()
        } else if frac > 0.0 {
            st.prog_full.to_string()
        } else {
            String::new()
        };
        let filled_part = format!("{}{}", st.prog_full.to_string().repeat(whole), partial);
        let empty_part = st.prog_empty.to_string().repeat(bar_w.saturating_sub(whole + partial.chars().count()));
        let bar = format!(
            "{}{}{}{}{}",
            st.p.col(NamedColor::Cyan), filled_part, RESET,
            st.p.col(NamedColor::GrayDim), empty_part);
        frame.push(line(inner, st, &format!(
            "{} {} {}{}{:>5.1}%{}",
            st.p.col(NamedColor::Gray), st.marker, RESET, bar, pct * 100.0, RESET)));

        // 时间行
        let remaining = total_ms.saturating_sub(elapsed_ms);
        frame.push(line(inner, st, &format!(
            "{}{}{} / {}{}{}    {}{} {}{}",
            BOLD, format_time(elapsed_ms), RESET,
            DIM, format_time(total_ms), RESET,
            DIM, st.tr("剩余", "left"), format_time(remaining), RESET)));

        // 音量行
        let vol_filled = ((volume as f64 / 500.0) * 10.0).round() as usize;
        let vol_bar = format!(
            "{}{}{}{}",
            st.p.col(NamedColor::Yellow), st.bar_fill.to_string().repeat(vol_filled),
            st.p.col(NamedColor::GrayDim), st.bar_empty.to_string().repeat(10 - vol_filled));
        frame.push(line(inner, st, &format!(
            "{}{}{} {} {}{:>3}%{}",
            st.p.col(NamedColor::Yellow), st.tr("音量", "Volume"), RESET, vol_bar, BOLD, volume, RESET)));

        // 动态 EQ
        if eq_rows > 0 {
            let bar_cols = freq_bar_cols();
            if inner >= 74 {
                let label_line = FREQ_LABELS.join(" ");
                frame.push(line(inner, st, &format!(
                    "{}{}{}  {}",
                    st.p.col(NamedColor::Cyan), st.tr("动态 EQ", "Dynamic EQ"), RESET, label_line)));
            } else {
                frame.push(line(inner, st, &format!(
                    "{}{}{}  {}  {}Hz {} {}kHz",
                    st.p.col(NamedColor::Cyan), st.tr("动态 EQ", "Dynamic EQ"), RESET,
                    st.middot, "20", st.range, "10")));
            }
            for row in (1..=7).rev() {
                let color = st.p.eq[row - 1];
                let mut s = String::with_capacity(inner);
                s.push_str(&" ".repeat(EQ_LABEL_PAD));
                if inner >= 74 {
                    // 宽屏：柱体精确置于标签起始列（非均匀间距）
                    let mut cur = 0usize;
                    for band in 0..16 {
                        s.push_str(&" ".repeat(bar_cols[band].saturating_sub(cur)));
                        cur = bar_cols[band] + 1;
                        if spectrum[band] >= row as u8 {
                            s.push_str(&format!("{}{}{}", color, st.prog_full, RESET));
                        } else {
                            s.push_str(&format!("{}{}{}", st.p.col(NamedColor::GrayDim), st.prog_empty, RESET));
                        }
                    }
                } else {
                    // 窄屏：无逐频段标签，均匀排布
                    let spacing = ((inner.saturating_sub(EQ_LABEL_PAD + 16)) / 15).clamp(0, 3);
                    for band in 0..16 {
                        if spectrum[band] >= row as u8 {
                            s.push_str(&format!("{}{}{}", color, st.prog_full, RESET));
                        } else {
                            s.push_str(&format!("{}{}{}", st.p.col(NamedColor::GrayDim), st.prog_empty, RESET));
                        }
                        if band < 15 {
                            s.push_str(&" ".repeat(spacing));
                        }
                    }
                }
                frame.push(line(inner, st, &s));
            }
        }

        // 音符详情
        for d in details.iter().take(det_rows) {
            frame.push(line(inner, st, &format!("{}{}{}", DIM, clip_w(&st.safe(d), inner - 2, st.ellipsis), RESET)));
        }

        // 封面 + 元数据区
        if let Some((aw, ah)) = art_disp {
            self.render_art_section(&mut frame, inner, aw, ah, art_side, &meta);
        } else if !meta.is_empty() {
            for (label, value) in meta.iter().take(section_rows) {
                frame.push(line(inner, st, &format!("{}", meta_text(label, value, inner - 2, st))));
            }
        }

        // 按键提示
        let hints = if st.cjk {
            format!(
                "{} {} {} {} {} {} {} {} {} {} {} {} {}",
                DIM, "空格 暂停", st.middot, "←/→ 快退/快进", st.middot,
                "↑/↓ 10s", st.middot, "R 循环", st.middot,
                "9/0 音量", st.middot, "Q 退出", RESET)
        } else {
            format!(
                "{} {} {} {}/{} {} {} {} {} {} {} {} {}",
                DIM, "Space Pause", st.middot, st.arrow_l, st.arrow_r, "seek", st.middot,
                "Up/Down 10s", st.middot, "R Loop", st.middot, "9/0 Vol", RESET)
        };
        frame.push(line(inner, st, &hints));
        if footer_rows == 2 {
            let mouse_hint = if st.cjk {
                "鼠标：点击进度条跳转 · 点击状态行播放/暂停"
            } else {
                "Mouse: click bar to seek, click status row to play/pause"
            };
            frame.push(line(inner, st, &format!("{}{}{}", DIM, mouse_hint, RESET)));
        }

        // 底框
        frame.push(format!(
            "{}{}{}{}",
            st.p.col(NamedColor::Cyan), st.box_.bl, st.box_.h.repeat(w.saturating_sub(2)), st.box_.br));

        debug_assert!(frame.len() <= h, "TUI frame exceeds terminal height");
        self.emit(&frame);
    }

    /// 输出一帧：pty 整屏重绘（与 2.4.0 一致）；tty 仅刷新变化的行，防闪烁。
    fn emit(&mut self, frame: &[String]) {
        let now = Instant::now();
        if !self.throttle.is_zero() && now.duration_since(self.last_emit) < self.throttle {
            return;
        }
        let mut out = String::with_capacity((self.width + 8) * frame.len() + 4096);
        match self.kind {
            OutputKind::Pty => {
                append_pty_frame(&mut out, frame);
            }
            OutputKind::Tty => {
                if self.prev_lines.is_empty() {
                    out.push_str("\x1b[2J\x1b[H");
                }
                for (i, l) in frame.iter().enumerate() {
                    if self.prev_lines.get(i) != Some(l) {
                        out.push_str(&format!("\x1b[{};1H{}", i + 1, l));
                        out.push_str("\x1b[K");
                    }
                }
                for i in frame.len()..self.prev_lines.len() {
                    out.push_str(&format!("\x1b[{};1H\x1b[K", i + 1));
                }
                self.prev_lines = frame.to_vec();
            }
        }
        print!("{}", out);
        let _ = io::stdout().flush();
        self.last_emit = now;
    }

    /// 封面区：封面框（左侧）+ 元数据（右侧并排 / 下方）。
    fn render_art_section(
        &self,
        frame: &mut Vec<String>,
        inner: usize,
        aw: usize,
        ah: usize,
        side: bool,
        meta: &[(&'static str, String)],
    ) {
        let st = &self.style;
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
        let title = st.tr("封面", "Cover");
        let top = if aw >= 10 {
            // 与柱体行同宽（aw+2）：tl + h + " " + 标题 + " " + h*(aw-7) + tr
            format!(
                "{}{}{}{} {} {}{}",
                indent, st.p.col(NamedColor::Gray), st.box_.tl, st.box_.h, title,
                st.box_.h.repeat(aw.saturating_sub(7)), st.box_.tr)
        } else {
            format!("{}{}{}{}{}", indent, st.p.col(NamedColor::Gray), st.box_.tl, st.box_.h.repeat(aw), st.box_.tr)
        };
        art_lines.push(format!("{}{}", top, RESET));

        // 封面本体：pty 半块真彩色；tty 亮度字符（无真彩色可用）
        for row in 0..ah {
            let body = if st.truecolor {
                render_art_row(img, row, aw, ah)
            } else {
                render_art_row_ascii(img, row, aw, ah)
            };
            art_lines.push(format!(
                "{}{}{}{}{}{}",
                indent, st.p.col(NamedColor::Gray), st.box_.v, body, st.box_.v, RESET));
        }
        art_lines.push(format!(
            "{}{}{}{}{}{}",
            indent, st.p.col(NamedColor::Gray), st.box_.bl, st.box_.h.repeat(aw), st.box_.br, RESET));

        if side {
            // 元数据垂直居中于封面框右侧
            let pad_top = (box_rows.saturating_sub(meta.len())) / 2;
            let mut meta_iter = meta.iter();
            for (i, box_line) in art_lines.iter().enumerate() {
                let mut s = box_line.clone();
                if i >= pad_top {
                    if let Some((label, value)) = meta_iter.next() {
                        let avail = meta_cols.saturating_sub(2).max(4);
                        s.push_str(&format!("  {}", meta_text(label, value, avail, st)));
                    }
                }
                frame.push(line(inner, st, &s));
            }
        } else {
            for box_line in &art_lines {
                frame.push(line(inner, st, box_line));
            }
            // 窄屏：元数据下置（最多 2 行）
            for (label, value) in meta.iter().take(2) {
                frame.push(line(inner, st, &format!("  {}", meta_text(label, value, inner - 4, st))));
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
            match self.kind {
                OutputKind::Pty => {
                    print!("\x1b[?1000l\x1b[?1006l\x1b[?25h\x1b[?1049l");
                }
                OutputKind::Tty => {
                    // 无备用屏：退出时清屏恢复
                    print!("\x1b[?25h\x1b[2J\x1b[H");
                }
            }
            let _ = io::stdout().flush();
            self.active = false;
        }
    }
}

/// 生成 PTY 全屏帧。最后一行刻意不带换行：若它恰好位于终端底部，
/// 换行会触发滚屏，使后续 SGR 鼠标坐标整体错位一行。
fn append_pty_frame(out: &mut String, frame: &[String]) {
    out.push_str("\x1b[H\x1b[2J");
    for (i, line) in frame.iter().enumerate() {
        out.push_str(line);
        if i + 1 < frame.len() {
            out.push('\n');
        }
    }
}

// ---------------------------------------------------------------------------
// 渲染辅助
// ---------------------------------------------------------------------------

/// 输出一行内容：带左右边框、按显示宽度右对齐填充到内容宽度。
/// 注意：pad 只追加空格。
fn line(inner: usize, st: &Style, content: &str) -> String {
    let pad = " ".repeat(inner.saturating_sub(disp_width(content)));
    format!(
        "{}{} {}{} {}{}",
        st.p.col(NamedColor::Gray), st.box_.v, content, pad, st.box_.v, RESET)
}

/// 元数据行：彩色标签 + 内容。
fn meta_text(label: &str, value: &str, max: usize, st: &Style) -> String {
    let color = match label {
        "作曲家" => NamedColor::Magenta,
        "艺术家" => NamedColor::Cyan,
        "专辑" => NamedColor::GreenDim,
        _ => NamedColor::Gray,
    };
    let label = st.tr(label, match label {
        "作曲家" => "Composer",
        "艺术家" => "Artist",
        "专辑" => "Album",
        _ => "Date/Genre",
    });
    let value = st.safe(value);
    format!(
        "{}{} {}{}{}",
        st.p.col(color), label, RESET,
        clip_w(&value, max.saturating_sub(disp_width(label) + 1), st.ellipsis), RESET)
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

/// 按显示宽度截断，超长时追加省略号（tty 无 … 字形时用 "..."）。
fn clip_w(s: &str, max: usize, ellipsis: &str) -> String {
    if max == 0 { return String::new(); }
    if disp_width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for c in s.chars() {
        let cw = if is_wide(c) { 2 } else { 1 };
        if w + cw > max.saturating_sub(disp_width(ellipsis)) { break; }
        out.push(c);
        w += cw;
    }
    out.push_str(ellipsis);
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

/// 渲染封面的一行（半块字符 + 真彩色前景/背景 = 上下两个像素）。仅 pty。
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

/// 亮度字符封面（真实 tty：无真彩色、可能无 ▀ 字形）。
/// 每行对应 2 像素高的条带，取平均亮度映射到 ASCII 渐变字符。
fn render_art_row_ascii(img: &ArtImage, row: usize, disp_w: usize, disp_h: usize) -> String {
    const RAMP: [char; 10] = [' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];
    let total_px_h = disp_h * 2;
    let mut s = String::with_capacity(disp_w);
    for col in 0..disp_w {
        let x = (col as f64 + 0.5) * img.width as f64 / disp_w as f64;
        let mut lum = 0.0f64;
        for sub in 0..2 {
            let y = (row as f64 * 2.0 + sub as f64 + 0.5) * img.height as f64 / total_px_h as f64;
            let y = y.clamp(0.0, img.height as f64 - 1.0);
            let xi = (x.floor() as usize).min(img.width - 1);
            let yi = (y.floor() as usize).min(img.height - 1);
            let i = (yi * img.width + xi) * 4;
            let (r, g, b, a) = (
                img.data[i] as f64 / 255.0,
                img.data[i + 1] as f64 / 255.0,
                img.data[i + 2] as f64 / 255.0,
                img.data[i + 3] as f64 / 255.0,
            );
            // 预乘 alpha 合成到黑底后取亮度
            let r = r * a;
            let g = g * a;
            let b = b * a;
            lum += 0.2126 * r + 0.7152 * g + 0.0722 * b;
        }
        lum /= 2.0;
        let idx = ((lum * (RAMP.len() - 1) as f64).round() as usize).min(RAMP.len() - 1);
        s.push(RAMP[idx]);
    }
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
    const ART_BG: (u8, u8, u8) = (14, 14, 22);
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
    fn pty_frame_does_not_scroll_after_last_row() {
        let mut out = String::new();
        append_pty_frame(&mut out, &["row1".into(), "row2".into()]);
        assert_eq!(out, "\x1b[H\x1b[2Jrow1\nrow2");
        assert!(!out.ends_with('\n'));
    }

    #[test]
    fn clip_and_pad_respect_cjk_width() {
        assert_eq!(disp_width("音乐"), 4);
        assert_eq!(disp_width("♪"), 1);
        assert_eq!(clip_w("中文标题很长的歌曲名字", 8, "…"), "中文标…"); // 7 列 + 省略号 = 8 列
        assert_eq!(clip_w("abcdefghijk", 6, "..."), "abc...");
        assert_eq!(clip_w("abcd", 8, "…"), "abcd");
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
            logo: github_avatar(),
            art: None,
            meta: MetaInfo::default(),
            kind: OutputKind::Pty,
            style: Style::full(),
            status_row: 3,
            bar_row: 4,
            prev_lines: Vec::new(),
            throttle: Duration::ZERO,
            last_emit: Instant::now(),
        }
    }

    /// 进度条点击映射必须与渲染布局严格一致（回归测试：修复 +3 列系统性偏移）。
    #[test]
    fn mouse_bar_click_maps_precisely() {
        let tui = test_tui();
        // 80 列 → inner=76 → bar_w=64 → 进度条占 1-based 第 6..69 列
        assert_eq!(BAR_X0, 6);
        let bar_w = 64.0;
        let bar_row = tui.bar_row;
        // 起点：进度条第 1 格 → 0%
        assert_eq!(tui.mouse_control(BAR_X0, bar_row, false), Control::SeekPercent(0.0));
        // 终点：进度条最后 1 格 → 100%
        let c = tui.mouse_control(BAR_X0 + (bar_w as u16) - 1, bar_row, false);
        assert!(matches!(c, Control::SeekPercent(p) if (p - 1.0).abs() < 1e-9));
        // 中点：第 32 格（列 6+31=37）→ 31/63 ≈ 49.2%
        let c = tui.mouse_control(37, bar_row, false);
        assert!(matches!(c, Control::SeekPercent(p) if (p - 31.0 / 63.0).abs() < 1e-9));
        // 点击进度条左侧（▸ 标记区域）→ 0%
        assert_eq!(tui.mouse_control(3, bar_row, false), Control::SeekPercent(0.0));
        // 超出进度条右端（百分比文字区域）→ 100%（钳制）
        let c = tui.mouse_control(76, bar_row, false);
        assert!(matches!(c, Control::SeekPercent(p) if (p - 1.0).abs() < 1e-9));
        // 其它行不响应
        assert_eq!(tui.mouse_control(37, bar_row + 1, false), Control::None);
        // 状态行切换播放/暂停
        assert_eq!(tui.mouse_control(10, tui.status_row, true), Control::Play);
        assert_eq!(tui.mouse_control(10, tui.status_row, false), Control::Pause);
    }

    /// 窄终端（bar_w=24）下偏移映射同样精确。
    #[test]
    fn mouse_bar_click_narrow_terminal() {
        let mut tui = test_tui();
        tui.width = 40; // inner=36 → bar_w=24 → 进度条占第 6..29 列
        let bar_w = 24.0;
        let bar_row = tui.bar_row;
        assert_eq!(tui.mouse_control(BAR_X0, bar_row, false), Control::SeekPercent(0.0));
        let c = tui.mouse_control(BAR_X0 + (bar_w as u16) - 1, bar_row, false);
        assert!(matches!(c, Control::SeekPercent(p) if (p - 1.0).abs() < 1e-9));
        let c = tui.mouse_control(17, bar_row, false); // 第 12 格 → 11/23 ≈ 47.8%
        assert!(matches!(c, Control::SeekPercent(p) if (p - 11.0 / 23.0).abs() < 1e-9));
    }

    /// EQ 柱必须对准每个标签的中点（回归测试：短标签不再偏向左侧字符）。
    #[test]
    fn freq_bars_align_with_labels() {
        let cols = freq_bar_cols();
        assert_eq!(cols, [0, 3, 6, 9, 13, 17, 21, 25, 29, 33, 37, 42, 47, 52, 57, 62]);
        // 与标签行（FREQ_LABELS.join(" ")）中每个标签的中点一致
        let line = FREQ_LABELS.join(" ");
        let mut c = 0usize;
        for (i, l) in FREQ_LABELS.iter().enumerate() {
            assert_eq!(line[c..].starts_with(l), true, "band {} 标签错位", i);
            assert_eq!(cols[i], c + (l.len().saturating_sub(1)) / 2, "band {} 柱与标签错位", i);
            c += l.len() + usize::from(i < 15); // 与 freq_bar_cols 同规则
        }
        // 标签行总宽（含缩进）须能放入宽屏阈值 inner=74
        assert_eq!(c, 64);
        assert!(EQ_LABEL_PAD + c <= 74);
    }

    /// tty 降级风格：无 CJK/无方块字形时用英文 + ASCII 字符，且不产生乱码字符
    #[test]
    fn console_style_degrades_gracefully() {
        let caps = Caps::default(); // 保守：256 字形、无扩展字符
        let st = Style::console(caps);
        assert_eq!(st.tr("循环", "Loop"), "Loop");
        assert_eq!(st.tr_mode("音乐文件"), "Music File");
        assert_eq!(st.tr_mode("MIDI 音乐"), "MIDI Music");
        assert_eq!(st.tr_mode("简谱"), "Score");
        assert_eq!(st.box_.tl, "+");
        assert_eq!(st.box_.v, "|");
        assert_eq!(st.prog_full, '#');
        assert_eq!(st.marker, '>');
        assert_eq!(st.safe("贝多芬"), "???");
        assert_eq!(st.safe("Beethoven"), "Beethoven");
        // 16 色：不含 38;2
        assert!(!st.p.col(NamedColor::Cyan).contains("38;2"));
        // CJK 齐全的控制台：保持中文与圆角框
        let caps2 = Caps {
            charcount: 1024, has_cjk: true, has_box: true, has_block: true,
            has_upper_half: true, has_dot: true, has_music: true,
            has_arrow_l: true, has_arrow_r: true, has_middot: true, has_ellipsis: true,
        };
        let st2 = Style::console(caps2);
        assert_eq!(st2.tr("循环", "Loop"), "循环");
        assert_eq!(st2.box_.tl, "╭");
        assert_eq!(st2.prog_full, '█');
    }

    /// 亮度字符封面：输出全部为 ASCII 安全字符
    #[test]
    fn ascii_art_uses_safe_chars() {
        let img = ArtImage {
            data: vec![255u8; 64 * 64 * 4], // 全白不透明
            width: 64,
            height: 64,
        };
        let s = render_art_row_ascii(&img, 0, 16, 8);
        assert!(!s.is_empty());
        assert!(s.chars().all(|c| c.is_ascii() && c.is_ascii_graphic() || c == ' '));
        assert!(s.chars().all(|c| c != '▀'));
    }
}
