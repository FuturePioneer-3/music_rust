// music_rust —— 钢琴演奏器
// Copyright (C) 2026 FuturePioneer-3
// SPDX-License-Identifier: GPL-3.0-or-later

//! 无命令行参数时使用的启动选择界面。
//!
//! 这个模块只负责收集播放配置。它使用备用屏幕和独立的 raw mode，
//! 在返回前会恢复终端；因此不会改变或复用播放中的 TUI / 输入监听器。

use std::cmp;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// 启动选择界面收集到的配置。
///
/// `soundfont == None` 代表保留原有的自动探测行为。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchConfig {
    pub file: String,
    pub soundfont: Option<String>,
    pub instrument: u8,
}

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[96m";
const MAGENTA: &str = "\x1b[95m";
const GREEN: &str = "\x1b[92m";
const YELLOW: &str = "\x1b[93m";
const GRAY: &str = "\x1b[90m";

/// 将终端绘制错误统一转换为启动器对外使用的中文错误信息。
macro_rules! try_draw {
    ($result:expr) => {
        $result.map_err(|error| format!("绘制启动界面失败: {error}"))?
    };
}

/// 在交互终端中运行启动选择器。
///
/// `Ok(None)` 表示用户主动取消；非交互 stdin/stdout 返回清晰错误，避免
/// 在管道或 CI 中等待键盘输入。
pub fn select_startup() -> Result<Option<LaunchConfig>, String> {
    let _terminal = TerminalSession::start()?;
    let fonts = crate::synth::available_soundfonts();
    let mut state = StartupState {
        file: None,
        soundfont: preferred_soundfont(&fonts),
        instrument: 0,
        program_input: None,
        focus: StartupField::File,
        notice: None,
    };

    loop {
        render_startup(&state)?;
        let key = read_key()?;
        if let Some(input) = state.program_input.as_mut() {
            match key {
                Key::Quit => return Ok(None),
                Key::Escape => {
                    state.program_input = None;
                    state.notice = None;
                }
                Key::Digit(digit) => {
                    // 最多三位；超过三位时把新数字视作下一次输入的开头。
                    // 128 等三位数保留到确认时再提示超出 GM 范围，避免
                    // 用户看不到自己刚输入的内容。
                    let candidate = format!("{}{}", input, digit);
                    if candidate.len() <= 3 && candidate.parse::<u16>().is_ok() {
                        input.push(char::from(b'0' + digit));
                        state.notice = None;
                    } else {
                        input.clear();
                        input.push(char::from(b'0' + digit));
                        state.notice = None;
                    }
                }
                Key::Backspace => {
                    input.pop();
                    state.notice = None;
                }
                Key::Enter => {
                    commit_program_input(&mut state);
                }
                Key::Tab => {
                    commit_program_input(&mut state);
                    state.focus = state.focus.next();
                }
                Key::Up => {
                    commit_program_input(&mut state);
                    state.focus = state.focus.previous();
                }
                Key::Down => {
                    commit_program_input(&mut state);
                    state.focus = state.focus.next();
                }
                _ => {}
            }
            continue;
        }

        match key {
            Key::Quit | Key::Escape => return Ok(None),
            Key::Up => state.focus = state.focus.previous(),
            Key::Down | Key::Tab => state.focus = state.focus.next(),
            Key::Left if state.focus == StartupField::Program => {
                state.instrument = state.instrument.saturating_sub(1);
            }
            Key::Right if state.focus == StartupField::Program => {
                state.instrument = state.instrument.saturating_add(1).min(127);
            }
            Key::Increment if state.focus == StartupField::Program => {
                state.instrument = state.instrument.saturating_add(1).min(127);
            }
            Key::Decrement if state.focus == StartupField::Program => {
                state.instrument = state.instrument.saturating_sub(1);
            }
            Key::Digit(digit) if state.focus == StartupField::Program => {
                state.program_input = Some(digit.to_string());
                state.notice = None;
            }
            Key::Enter => match state.focus {
                StartupField::File => match browse_file(BrowseKind::Playable)? {
                    Some(path) => {
                        state.file = Some(path);
                        state.notice = None;
                    }
                    None => state.notice = Some("未更改乐曲文件。".to_string()),
                },
                StartupField::Soundfont => {
                    if let Some(font) = choose_soundfont(state.soundfont.as_deref())? {
                        state.soundfont = Some(font);
                        state.notice = None;
                    } else {
                        state.notice = Some("未更改 SoundFont。".to_string());
                    }
                }
                StartupField::Program => {
                    state.program_input = Some(String::new());
                    state.notice = None;
                }
                StartupField::Start => {
                    if let Some(file) = &state.file {
                        return Ok(Some(LaunchConfig {
                            file: file.to_string_lossy().into_owned(),
                            soundfont: state.soundfont.clone(),
                            instrument: state.instrument,
                        }));
                    }
                    state.notice = Some("请先选择一个可播放的乐曲文件。".to_string());
                    state.focus = StartupField::File;
                }
            },
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartupField {
    File,
    Soundfont,
    Program,
    Start,
}

impl StartupField {
    fn next(self) -> Self {
        match self {
            Self::File => Self::Soundfont,
            Self::Soundfont => Self::Program,
            Self::Program => Self::Start,
            Self::Start => Self::File,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::File => Self::Start,
            Self::Soundfont => Self::File,
            Self::Program => Self::Soundfont,
            Self::Start => Self::Program,
        }
    }
}

struct StartupState {
    file: Option<PathBuf>,
    soundfont: Option<String>,
    instrument: u8,
    program_input: Option<String>,
    focus: StartupField,
    notice: Option<String>,
}

fn commit_program_input(state: &mut StartupState) {
    let Some(raw) = state.program_input.take() else {
        return;
    };
    match parse_program_number(&raw) {
        Ok(value) => {
            state.instrument = value;
            state.notice = None;
        }
        Err(message) => state.notice = Some(message.to_string()),
    }
}

fn parse_program_number(raw: &str) -> Result<u8, &'static str> {
    if raw.is_empty() {
        return Err("请输入 0–127 的 GM 音色号。");
    }
    match raw.parse::<u16>() {
        Ok(value) if value <= 127 => Ok(value as u8),
        _ => Err("GM 音色号必须是 0–127。"),
    }
}

/// 常用 GM Program 名称。编号与 FluidSynth 的 0--127 Program 完全一致。
const GM_PROGRAM_NAMES: [&str; 128] = [
    "Acoustic Grand Piano", "Bright Acoustic Piano", "Electric Grand Piano", "Honky-tonk Piano",
    "Electric Piano 1", "Electric Piano 2", "Harpsichord", "Clavinet",
    "Celesta", "Glockenspiel", "Music Box", "Vibraphone", "Marimba", "Xylophone", "Tubular Bells", "Dulcimer",
    "Drawbar Organ", "Percussive Organ", "Rock Organ", "Church Organ", "Reed Organ", "Accordion", "Harmonica", "Tango Accordion",
    "Acoustic Guitar (nylon)", "Acoustic Guitar (steel)", "Electric Guitar (jazz)", "Electric Guitar (clean)",
    "Electric Guitar (muted)", "Overdriven Guitar", "Distortion Guitar", "Guitar Harmonics",
    "Acoustic Bass", "Electric Bass (finger)", "Electric Bass (pick)", "Fretless Bass", "Slap Bass 1", "Slap Bass 2", "Synth Bass 1", "Synth Bass 2",
    "Violin", "Viola", "Cello", "Contrabass", "Tremolo Strings", "Pizzicato Strings", "Orchestral Harp", "Timpani",
    "String Ensemble 1", "String Ensemble 2", "Synth Strings 1", "Synth Strings 2", "Choir Aahs", "Voice Oohs", "Synth Voice", "Orchestra Hit",
    "Trumpet", "Trombone", "Tuba", "Muted Trumpet", "French Horn", "Brass Section", "Synth Brass 1", "Synth Brass 2",
    "Soprano Sax", "Alto Sax", "Tenor Sax", "Baritone Sax", "Oboe", "English Horn", "Bassoon", "Clarinet",
    "Piccolo", "Flute", "Recorder", "Pan Flute", "Blown Bottle", "Shakuhachi", "Whistle", "Ocarina",
    "Lead 1 (square)", "Lead 2 (sawtooth)", "Lead 3 (calliope)", "Lead 4 (chiff)",
    "Lead 5 (charang)", "Lead 6 (voice)", "Lead 7 (fifths)", "Lead 8 (bass + lead)",
    "Pad 1 (new age)", "Pad 2 (warm)", "Pad 3 (polysynth)", "Pad 4 (choir)",
    "Pad 5 (bowed)", "Pad 6 (metallic)", "Pad 7 (halo)", "Pad 8 (sweep)",
    "FX 1 (rain)", "FX 2 (soundtrack)", "FX 3 (crystal)", "FX 4 (atmosphere)",
    "FX 5 (brightness)", "FX 6 (goblins)", "FX 7 (echoes)", "FX 8 (sci-fi)",
    "Sitar", "Banjo", "Shamisen", "Koto", "Kalimba", "Bag pipe", "Fiddle", "Shanai",
    "Tinkle Bell", "Agogo", "Steel Drums", "Woodblock", "Taiko Drum", "Melodic Tom", "Synth Drum", "Reverse Cymbal",
    "Guitar Fret Noise", "Breath Noise", "Seashore", "Bird Tweet", "Telephone Ring", "Helicopter", "Applause", "Gunshot",
];

fn gm_program_name(program: u8) -> &'static str {
    GM_PROGRAM_NAMES[program as usize]
}

fn render_startup(state: &StartupState) -> Result<(), String> {
    let (width, _) = terminal_size();
    let mut out = io::stdout();
    clear_screen(&mut out)?;

    write_box_top(&mut out, width)?;
    try_draw!(writeln!(out, "{CYAN}{BOLD}  music_rust 启动选择器{RESET}"));
    try_draw!(writeln!(out, "{GRAY}  SoundFont 用于 MIDI/简谱；GM 音色仅用于简谱 TXT。{RESET}"));
    write_box_rule(&mut out, width)?;

    let file = state
        .file
        .as_ref()
        .map(|p| display_path(p, width.saturating_sub(23)))
        .unwrap_or_else(|| "<尚未选择>".to_string());
    let font = state
        .soundfont
        .as_deref()
        .map(|p| display_text(p, width.saturating_sub(23)))
        .unwrap_or_else(|| "<自动探测>".to_string());

    render_field(
        &mut out,
        width,
        state.focus == StartupField::File,
        "乐曲文件",
        &file,
    )?;
    render_field(
        &mut out,
        width,
        state.focus == StartupField::Soundfont,
        "SoundFont",
        &font,
    )?;
    let program = match &state.program_input {
        Some(input) if input.is_empty() => "<输入 0–127>".to_string(),
        Some(input) => format!("输入中: {}", input),
        None => format!("{:>3}  {}", state.instrument, gm_program_name(state.instrument)),
    };
    render_field(
        &mut out,
        width,
        state.focus == StartupField::Program,
        "GM 音色",
        &program,
    )?;

    let start_value = if state.file.is_some() {
        "Enter 开始播放"
    } else {
        "请先选择乐曲文件"
    };
    render_field(
        &mut out,
        width,
        state.focus == StartupField::Start,
        "开始播放",
        start_value,
    )?;
    write_box_rule(&mut out, width)?;
    if let Some(notice) = &state.notice {
        try_draw!(writeln!(out, "{YELLOW}  {}{RESET}", display_text(notice, width.saturating_sub(4))));
    } else if state.program_input.is_some() {
        try_draw!(writeln!(out, "{GRAY}  输入 0–127 · Enter 确认 · Backspace 删除 · Esc 取消{RESET}"));
    } else {
        try_draw!(writeln!(out, "{GRAY}  ↑↓/Tab 切换 · Enter 选择 · ←→/+− 调音色 · 数字直接输入 · Q/Esc 取消{RESET}"));
    }
    write_box_bottom(&mut out, width)?;
    out.flush().map_err(|e| format!("刷新启动界面失败: {e}"))
}

fn render_field(
    out: &mut impl Write,
    width: usize,
    selected: bool,
    label: &str,
    value: &str,
) -> Result<(), String> {
    let marker = if selected { ">" } else { " " };
    let color = if selected { CYAN } else { GRAY };
    let emphasis = if selected { BOLD } else { "" };
    let prefix = format!(" {marker} {} ", pad_display(label, 12));
    let value = display_text(value, width.saturating_sub(display_width(&prefix)));
    try_draw!(writeln!(out, "{color}{emphasis}{prefix}{value}{RESET}"));
    Ok(())
}

/// 选择一个系统发现的 SoundFont，或从目录浏览器挑选其它文件。
fn choose_soundfont(current: Option<&str>) -> Result<Option<String>, String> {
    let mut fonts = crate::synth::available_soundfonts();
    // 用户从“浏览其它目录”选过的自定义音源不一定属于自动扫描目录；
    // 重新打开列表时保留它，避免界面突然跳回默认项。
    if let Some(path) = current {
        if Path::new(path).is_file() && !fonts.iter().any(|font| font == path) {
            fonts.push(path.to_string());
        }
    }
    let preferred = preferred_soundfont(&fonts);
    let mut selected = current
        .and_then(|value| fonts.iter().position(|font| font == value))
        .or_else(|| preferred.as_ref().and_then(|value| fonts.iter().position(|font| font == value)))
        .unwrap_or(0);

    loop {
        let (width, height) = terminal_size();
        let mut out = io::stdout();
        clear_screen(&mut out)?;
        write_box_top(&mut out, width)?;
        try_draw!(writeln!(out, "{MAGENTA}{BOLD}  选择 SoundFont{RESET}"));
        try_draw!(writeln!(out, "{GRAY}  默认优先 electronic_synth.sf2；也可以浏览其它目录。{RESET}"));
        write_box_rule(&mut out, width)?;

        let item_count = fonts.len() + 1; // 最后一项始终是文件浏览器
        selected = selected.min(item_count.saturating_sub(1));
        let rows = height.saturating_sub(8).max(4);
        let (start, end) = page_bounds(selected, item_count, rows);
        for i in start..end {
            let is_selected = i == selected;
            let marker = if is_selected { ">" } else { " " };
            if i < fonts.len() {
                let default_mark = if preferred.as_deref() == Some(fonts[i].as_str()) {
                    " [默认]"
                } else {
                    ""
                };
                let color = if is_selected { CYAN } else { "" };
                let font = display_text(&fonts[i], width.saturating_sub(18));
                try_draw!(writeln!(out, "{color} {marker} {font}{default_mark}{RESET}"));
            } else {
                let color = if is_selected { CYAN } else { "" };
                try_draw!(writeln!(out, "{color} {marker} 浏览其它目录…{RESET}"));
            }
        }
        if end < item_count {
            try_draw!(writeln!(out, "{DIM}  ↓ 还有更多项{RESET}"));
        }
        write_box_rule(&mut out, width)?;
        try_draw!(writeln!(out, "{GRAY}  ↑/↓ 选择  ·  Enter 确认  ·  Esc 返回{RESET}"));
        write_box_bottom(&mut out, width)?;
        out.flush().map_err(|e| format!("刷新 SoundFont 列表失败: {e}"))?;

        match read_key()? {
            Key::Quit | Key::Escape | Key::Left => return Ok(None),
            Key::Up => selected = selected.saturating_sub(1),
            Key::Down | Key::Tab => selected = (selected + 1).min(item_count.saturating_sub(1)),
            Key::Enter => {
                if selected < fonts.len() {
                    return Ok(Some(fonts[selected].clone()));
                }
                if let Some(path) = browse_file(BrowseKind::Soundfont)? {
                    return Ok(Some(path.to_string_lossy().into_owned()));
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy)]
enum BrowseKind {
    Playable,
    Soundfont,
}

/// 从当前目录开始浏览文件。只显示可播放文件或 SoundFont 文件，目录始终可进入。
fn browse_file(kind: BrowseKind) -> Result<Option<PathBuf>, String> {
    let mut directory = std::env::current_dir().map_err(|e| format!("无法取得当前目录: {e}"))?;
    let mut selected = 0usize;
    let mut notice: Option<String> = None;

    loop {
        let entries = match browser_entries(&directory, kind) {
            Ok(entries) => entries,
            Err(error) => {
                notice = Some(error);
                let mut fallback = Vec::new();
                if let Some(parent) = directory.parent() {
                    fallback.push(BrowserEntry::parent(parent.to_path_buf()));
                }
                fallback
            }
        };
        selected = selected.min(entries.len().saturating_sub(1));
        render_browser(&directory, &entries, selected, kind, notice.as_deref())?;

        match read_key()? {
            Key::Quit | Key::Escape => return Ok(None),
            Key::Left => {
                if let Some(parent) = directory.parent() {
                    directory = parent.to_path_buf();
                    selected = 0;
                    notice = None;
                }
            }
            Key::Up => selected = selected.saturating_sub(1),
            Key::Down | Key::Tab => {
                if !entries.is_empty() {
                    selected = (selected + 1).min(entries.len() - 1);
                }
            }
            Key::Enter => {
                let Some(entry) = entries.get(selected) else {
                    notice = Some("此目录没有可选择的文件。".to_string());
                    continue;
                };
                match entry.kind {
                    BrowserEntryKind::Parent | BrowserEntryKind::Directory => {
                        directory = entry.path.clone();
                        selected = 0;
                        notice = None;
                    }
                    BrowserEntryKind::File => return Ok(Some(entry.path.clone())),
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BrowserEntryKind {
    Parent,
    Directory,
    File,
}

struct BrowserEntry {
    kind: BrowserEntryKind,
    path: PathBuf,
    name: String,
}

impl BrowserEntry {
    fn parent(path: PathBuf) -> Self {
        Self {
            kind: BrowserEntryKind::Parent,
            path,
            name: "..  上级目录".to_string(),
        }
    }
}

fn browser_entries(directory: &Path, kind: BrowseKind) -> Result<Vec<BrowserEntry>, String> {
    let mut directories = Vec::new();
    let mut files = Vec::new();
    let entries = std::fs::read_dir(directory)
        .map_err(|e| format!("无法读取 {}: {e}", directory.display()))?;

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        // 隐藏文件/目录通常不是用户想播放的内容，也避免把 .git 展开成巨大列表。
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            directories.push(BrowserEntry {
                kind: BrowserEntryKind::Directory,
                path,
                name: format!("{name}/"),
            });
        } else if path.is_file() && matches_browser_kind(&path, kind) {
            files.push(BrowserEntry {
                kind: BrowserEntryKind::File,
                path,
                name,
            });
        }
    }
    directories.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    let mut result = Vec::new();
    if let Some(parent) = directory.parent() {
        result.push(BrowserEntry::parent(parent.to_path_buf()));
    }
    result.extend(directories);
    result.extend(files);
    Ok(result)
}

fn matches_browser_kind(path: &Path, kind: BrowseKind) -> bool {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase());
    match kind {
        BrowseKind::Playable => matches!(
            extension.as_deref(),
            Some("txt" | "mid" | "midi" | "wav" | "mp3" | "flac" | "ogg" | "opus" | "aac" | "m4a" | "wma")
        ),
        BrowseKind::Soundfont => matches!(extension.as_deref(), Some("sf2" | "sf3")),
    }
}

fn render_browser(
    directory: &Path,
    entries: &[BrowserEntry],
    selected: usize,
    kind: BrowseKind,
    notice: Option<&str>,
) -> Result<(), String> {
    let (width, height) = terminal_size();
    let mut out = io::stdout();
    clear_screen(&mut out)?;
    let title = match kind {
        BrowseKind::Playable => "选择乐曲文件",
        BrowseKind::Soundfont => "浏览 SoundFont 文件",
    };
    let extensions = match kind {
        BrowseKind::Playable => "TXT / MIDI / WAV / MP3 / FLAC / OGG / AAC / M4A / WMA",
        BrowseKind::Soundfont => "SF2 / SF3",
    };
    write_box_top(&mut out, width)?;
    try_draw!(writeln!(out, "{GREEN}{BOLD}  {title}{RESET}"));
    try_draw!(writeln!(out, "{GRAY}  {}{RESET}", display_path(directory, width.saturating_sub(4))));
    try_draw!(writeln!(out, "{DIM}  仅显示 {extensions}；目录可进入。{RESET}"));
    write_box_rule(&mut out, width)?;

    let rows = height.saturating_sub(10).max(3);
    let (start, end) = page_bounds(selected, entries.len(), rows);
    for (index, entry) in entries.iter().enumerate().take(end).skip(start) {
        let marker = if index == selected { ">" } else { " " };
        let color = if index == selected { CYAN } else { "" };
        let prefix = match entry.kind {
            BrowserEntryKind::Parent => "↩ ",
            BrowserEntryKind::Directory => "▸ ",
            BrowserEntryKind::File => "  ",
        };
        try_draw!(writeln!(out, "{color} {marker} {prefix}{}{RESET}", display_text(&entry.name, width.saturating_sub(8))));
    }
    if entries.is_empty() {
        try_draw!(writeln!(out, "{YELLOW}  此目录没有可选择的文件。{RESET}"));
    } else if start > 0 || end < entries.len() {
        try_draw!(writeln!(out, "{DIM}  显示 {}–{} / {} 项{RESET}", start + 1, end, entries.len()));
    }
    write_box_rule(&mut out, width)?;
    if let Some(notice) = notice {
        try_draw!(writeln!(out, "{YELLOW}  {}{RESET}", display_text(notice, width.saturating_sub(4))));
    } else {
        try_draw!(writeln!(out, "{GRAY}  ↑/↓ 选择  ·  Enter 打开/确认  ·  ← 返回上级  ·  Esc 取消{RESET}"));
    }
    write_box_bottom(&mut out, width)?;
    out.flush().map_err(|e| format!("刷新文件列表失败: {e}"))
}

/// SoundFont 默认优先选择随程序提供的电子合成器音源。
fn preferred_soundfont(fonts: &[String]) -> Option<String> {
    fonts
        .iter()
        .find(|font| {
            Path::new(font)
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("electronic_synth.sf2"))
        })
        .cloned()
        .or_else(|| fonts.first().cloned())
}

/// 返回含 `selected` 的可见页范围。列表为空时返回 `(0, 0)`。
fn page_bounds(selected: usize, count: usize, rows: usize) -> (usize, usize) {
    if count == 0 {
        return (0, 0);
    }
    let rows = rows.max(1);
    let selected = selected.min(count - 1);
    let start = selected.saturating_sub(rows.saturating_sub(1));
    (start, cmp::min(start + rows, count))
}

fn clear_screen(out: &mut impl Write) -> Result<(), String> {
    write!(out, "\x1b[H\x1b[2J").map_err(|e| format!("绘制启动界面失败: {e}"))
}

fn write_box_top(out: &mut impl Write, width: usize) -> Result<(), String> {
    writeln!(out, "{CYAN}+{}+{RESET}", "-".repeat(width.saturating_sub(2).max(1)))
        .map_err(|e| format!("绘制启动界面失败: {e}"))
}

fn write_box_rule(out: &mut impl Write, width: usize) -> Result<(), String> {
    writeln!(out, "{GRAY}+{}+{RESET}", "-".repeat(width.saturating_sub(2).max(1)))
        .map_err(|e| format!("绘制启动界面失败: {e}"))
}

fn write_box_bottom(out: &mut impl Write, width: usize) -> Result<(), String> {
    writeln!(out, "{CYAN}+{}+{RESET}", "-".repeat(width.saturating_sub(2).max(1)))
        .map_err(|e| format!("绘制启动界面失败: {e}"))
}

fn display_path(path: &Path, limit: usize) -> String {
    display_text(&path.to_string_lossy(), limit)
}

/// 去掉控制字符，避免文件名把 ANSI 控制序列写回终端；再按终端显示宽度截断。
///
/// 中文、日文、韩文及全角字符占两列。启动器常用于选择中文文件名，因此不能
/// 只按 Rust 的字符数截断，否则窄终端上会意外换行、破坏备用屏布局。
fn display_text(text: &str, limit: usize) -> String {
    if limit == 0 {
        return String::new();
    }
    let safe: String = text
        .chars()
        .map(|character| if character.is_control() { '�' } else { character })
        .collect();
    if display_width(&safe) <= limit {
        return safe;
    }
    let ellipsis = "…";
    if limit <= display_width(ellipsis) {
        return "…".to_string();
    }
    let mut prefix = String::new();
    let mut used = 0usize;
    let available = limit.saturating_sub(display_width(ellipsis));
    for character in safe.chars() {
        let width = char_display_width(character);
        if used + width > available {
            break;
        }
        prefix.push(character);
        used += width;
    }
    format!("{prefix}{ellipsis}")
}

/// 对齐标签到固定的终端显示宽度，而不是 Rust 字符数。
fn pad_display(text: &str, width: usize) -> String {
    let mut out = text.to_string();
    out.push_str(&" ".repeat(width.saturating_sub(display_width(text))));
    out
}

fn display_width(text: &str) -> usize {
    text.chars().map(char_display_width).sum()
}

fn char_display_width(character: char) -> usize {
    if is_wide(character) { 2 } else { 1 }
}

fn is_wide(character: char) -> bool {
    let codepoint = character as u32;
    (0x1100..=0x115f).contains(&codepoint)
        || (0x2e80..=0x303e).contains(&codepoint)
        || (0x3041..=0x33ff).contains(&codepoint)
        || (0x3400..=0x4dbf).contains(&codepoint)
        || (0x4e00..=0x9fff).contains(&codepoint)
        || (0xa000..=0xa4cf).contains(&codepoint)
        || (0xac00..=0xd7a3).contains(&codepoint)
        || (0xf900..=0xfaff).contains(&codepoint)
        || (0xfe30..=0xfe4f).contains(&codepoint)
        || (0xff00..=0xff60).contains(&codepoint)
        || (0xffe0..=0xffe6).contains(&codepoint)
}

fn terminal_size() -> (usize, usize) {
    #[cfg(unix)]
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 {
            let width = usize::from(ws.ws_col);
            let height = usize::from(ws.ws_row);
            if width > 0 && height > 0 {
                return (width, height);
            }
        }
    }
    (80, 24)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Tab,
    Increment,
    Decrement,
    Digit(u8),
    Backspace,
    Quit,
    Escape,
    Unknown,
}

#[cfg(unix)]
fn read_byte() -> Result<Option<u8>, String> {
    let mut byte = 0u8;
    let result = unsafe { libc::read(libc::STDIN_FILENO, &mut byte as *mut u8 as *mut libc::c_void, 1) };
    if result < 0 {
        return Err(format!("读取键盘输入失败: {}", io::Error::last_os_error()));
    }
    Ok((result == 1).then_some(byte))
}

#[cfg(not(unix))]
fn read_byte() -> Result<Option<u8>, String> {
    Err("启动选择界面暂仅支持 Unix 终端。请通过命令行指定乐曲文件。".to_string())
}

fn read_key() -> Result<Key, String> {
    let first = loop {
        if let Some(byte) = read_byte()? {
            break byte;
        }
    };
    let key = match first {
        b'\r' | b'\n' => Key::Enter,
        b'\t' => Key::Tab,
        b'+' | b'=' => Key::Increment,
        b'-' => Key::Decrement,
        b'0'..=b'9' => Key::Digit(first - b'0'),
        0x08 | 0x7f => Key::Backspace,
        b'k' | b'K' => Key::Up,
        b'j' | b'J' => Key::Down,
        b'h' | b'H' => Key::Left,
        b'l' | b'L' => Key::Right,
        b'q' | b'Q' | 3 => Key::Quit,
        0x1b => read_escape_key()?,
        _ => Key::Unknown,
    };
    Ok(key)
}

fn read_escape_key() -> Result<Key, String> {
    let Some(second) = read_byte()? else {
        return Ok(Key::Escape); // 单独 Esc：取消
    };
    if second != b'[' && second != b'O' {
        return Ok(Key::Escape);
    }
    let mut sequence = Vec::new();
    for _ in 0..8 {
        let Some(byte) = read_byte()? else {
            break;
        };
        sequence.push(byte);
        if byte.is_ascii_alphabetic() || byte == b'~' {
            break;
        }
    }
    let last = sequence.last().copied();
    Ok(match last {
        Some(b'A') => Key::Up,
        Some(b'B') => Key::Down,
        Some(b'C') => Key::Right,
        Some(b'D') => Key::Left,
        // PageUp/PageDown 在文件列表中也很自然地向上/下移动。
        Some(b'~') if sequence.first() == Some(&b'5') => Key::Up,
        Some(b'~') if sequence.first() == Some(&b'6') => Key::Down,
        _ => Key::Unknown,
    })
}

/// 独立 raw mode，绝不借用播放期 `input::InputListener` 的状态。
#[cfg(unix)]
struct RawMode {
    original: libc::termios,
}

#[cfg(unix)]
impl RawMode {
    fn enable() -> Result<Self, String> {
        unsafe {
            let mut original: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(libc::STDIN_FILENO, &mut original) != 0 {
                return Err(format!("无法读取终端设置: {}", io::Error::last_os_error()));
            }
            let mut raw = original;
            raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::IEXTEN | libc::ISIG);
            raw.c_iflag &= !(libc::BRKINT | libc::ICRNL | libc::INPCK | libc::ISTRIP | libc::IXON);
            // 保留输出后处理（尤其 ONLCR），让 writeln! 的 `\n` 在真实
            // 终端上仍回到行首；选择器只需要 raw 输入，不需要 raw 输出。
            // 每 100ms 允许 read 返回，以便 ESC 可被识别为单独按键。
            raw.c_cc[libc::VMIN] = 0;
            raw.c_cc[libc::VTIME] = 1;
            if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) != 0 {
                return Err(format!("无法启用终端原始模式: {}", io::Error::last_os_error()));
            }
            Ok(Self { original })
        }
    }
}

#[cfg(unix)]
impl Drop for RawMode {
    fn drop(&mut self) {
        unsafe {
            let _ = libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.original);
        }
    }
}

struct TerminalSession {
    /// 终端模拟器支持备用屏；真实 Linux tty 与播放 TUI 一样只清屏，
    /// 退出时不能发送 `?1049l` 以免切换/覆盖用户的控制台缓冲区。
    alternate_screen: bool,
    #[cfg(unix)]
    _raw: RawMode,
}

impl TerminalSession {
    fn start() -> Result<Self, String> {
        #[cfg(unix)]
        {
            if unsafe { libc::isatty(libc::STDIN_FILENO) } != 1
                || unsafe { libc::isatty(libc::STDOUT_FILENO) } != 1
            {
                return Err(
                    "未检测到交互式终端；请传入乐曲文件，或在终端中不带参数运行。".to_string(),
                );
            }
            let raw = RawMode::enable()?;
            let alternate_screen = matches!(crate::console::output_kind(), crate::console::OutputKind::Pty)
                && std::env::var_os("MUSIC_FORCE_TTY").is_none();
            let mut out = io::stdout();
            let init = if alternate_screen {
                b"\x1b[?1049h\x1b[?25l\x1b[2J\x1b[H".as_slice()
            } else {
                b"\x1b[?25l\x1b[2J\x1b[H".as_slice()
            };
            if let Err(error) = out.write_all(init).and_then(|_| out.flush()) {
                // raw 会在返回时恢复；尽力退出备用屏，避免留下空白画面。
                let _ = if alternate_screen {
                    out.write_all(b"\x1b[?25h\x1b[?1049l")
                } else {
                    out.write_all(b"\x1b[?25h\x1b[2J\x1b[H")
                };
                return Err(format!("无法初始化启动界面: {error}"));
            }
            return Ok(Self { alternate_screen, _raw: raw });
        }
        #[cfg(not(unix))]
        {
            Err("启动选择界面暂仅支持 Unix 终端。请通过命令行指定乐曲文件。".to_string())
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let mut out = io::stdout();
        let _ = if self.alternate_screen {
            out.write_all(b"\x1b[0m\x1b[?25h\x1b[?1049l")
        } else {
            out.write_all(b"\x1b[0m\x1b[?25h\x1b[2J\x1b[H")
        };
        let _ = out.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        display_text, display_width, matches_browser_kind, page_bounds, parse_program_number,
        preferred_soundfont,
        BrowseKind,
    };
    use std::path::Path;

    #[test]
    fn bundled_electronic_soundfont_is_preferred() {
        let fonts = vec![
            "/tmp/FluidR3_GM.sf2".to_string(),
            "electronic_synth.sf2".to_string(),
            "/tmp/other.sf3".to_string(),
        ];
        assert_eq!(preferred_soundfont(&fonts).as_deref(), Some("electronic_synth.sf2"));
    }

    #[test]
    fn file_browser_recognizes_supported_extensions_case_insensitively() {
        assert!(matches_browser_kind(Path::new("song.MID"), BrowseKind::Playable));
        assert!(matches_browser_kind(Path::new("mix.FLAC"), BrowseKind::Playable));
        assert!(matches_browser_kind(Path::new("synth.SF3"), BrowseKind::Soundfont));
        assert!(!matches_browser_kind(Path::new("notes.pdf"), BrowseKind::Playable));
    }

    #[test]
    fn page_bounds_keeps_current_item_visible() {
        assert_eq!(page_bounds(0, 10, 4), (0, 4));
        assert_eq!(page_bounds(4, 10, 4), (1, 5));
        assert_eq!(page_bounds(99, 10, 4), (6, 10));
        assert_eq!(page_bounds(0, 0, 4), (0, 0));
    }

    #[test]
    fn display_text_clips_by_terminal_width_for_cjk_paths() {
        assert_eq!(display_width("音乐"), 4);
        assert_eq!(display_text("音乐文件.txt", 5), "音乐…");
        assert_eq!(display_text("music", 0), "");
    }

    #[test]
    fn program_number_is_limited_to_the_gm_range() {
        assert_eq!(parse_program_number("0"), Ok(0));
        assert_eq!(parse_program_number("127"), Ok(127));
        assert!(parse_program_number("128").is_err());
        assert!(parse_program_number("").is_err());
    }
}
