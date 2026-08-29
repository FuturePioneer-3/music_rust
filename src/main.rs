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

//! music_rust —— 钢琴演奏器
//!
//! 跨平台(主要 Linux) MIDI 简谱播放器。
//! 读取自定义简谱 TXT（支持多音轨），通过系统 libfluidsynth + SoundFont 演奏钢琴音色。
//!
//! 3.3：
//!   - 新增 TXT v3.2：文件可内嵌图片，支持 raw/zstd/gzip/zlib/deflate/bzip2/xz/lz4
//!     编码并保持二进制安全解析
//!   - 图片可在播放 TUI 中按音频封面相同的半区块方式显示
//!   - 播放前严格检查 TXT 要求的全部音色；任一预置缺失即报错退出
//!   - 新增无参数启动选择器，可选择乐曲、SoundFont 与 MIDI/简谱初始 GM 音色号
//!   - 无参数启动器可同时加载至多 3 个 SoundFont（任意两个合计 ≤120 MB），并为
//!     MIDI/TXT 设置至多 24 条按秒切换的 SoundFont / GM 音色规则
//!   - 修复 MIDI/TXT 音量状态与实际合成增益不一致，以及 limiter 残留包络导致的响度滞后
//!   - 修复音频文件暂停/跳转线程死锁、EOF 竞态和 TUI 进度条行号错位
//!   - TUI 全面重绘（圆角边框/真彩色/平滑进度条/渐变 EQ）
//!   - 音频文件模式解析内嵌专辑封面与作曲家等元数据，渲染在播放区下方
//!   - 音量缩放、频谱 Goertzel 谐振器、峰值限制器热循环改为独立
//!     AT&T 语法汇编（src/music_asm.S，非内联，SSE2）
//!
//! 3.3.0：
//!   - 修复 MIDI 总时长估算对非 0 通道事件的解析错位
//!   - 修复截断/损坏 MIDI 文件可能导致越界崩溃的问题
//!   - 修复音频文件播放线程与控制线程对 ramp_gain 的数据竞争
//!
//! 用法:
//!   music                       无参数时进入启动选择界面
//!   music <乐曲.txt|音频文件> [选项]
//!
//! 选项:
//!   -d, --debug          详细调试输出
//!   -s, --soundfont <p>  指定 SoundFont (.sf2/.sf3) 路径（可重复，最多 3 个）
//!   -i, --instrument <n>  MIDI/简谱初始 GM Program (0-127)
//!   -m, --midi <file>    直接播放 MIDI 文件（fluidsynth 原生多轨+变速）
//!   -t, --tempo <ms>     覆盖速度 (>0 ms/四分音符)
//!   -b, --bpm <n>        覆盖速度 (1-60000 BPM)
//!   -v, --volume <0-500> 音量（默认 80%）
//!   -h, --help           帮助
//!
//! 播放控制（类似 mpv）:
//!   ←/→ 后退/快进5秒, ↑/↓ 快进/后退10秒, PageUp/PgDn 快进/后退1分钟
//!   空格/P 暂停/继续, [ / ] 微调1秒, R 循环, 1-8 跳转10%-80%, Q 退出
//!   9/0 降低/增加音量

mod console;
mod input;
mod launcher;
mod audio_file;
mod log;
mod parser;
mod progress;
mod synth;
mod tui;

use std::process::exit;

use log::{debug, error, info};
use parser::{parse_file, print_first_events, print_score_summary};
use synth::{find_soundfont, ProgramSwitch, SynthPlayer};

/// 展示版本号（与 Cargo/Arch 包保持一致）
pub const VERSION: &str = "3.3.0";

fn print_usage() {
    println!("music_rust —— 钢琴演奏器 v{}", VERSION);
    println!();
    println!("用法:");
    println!("  music                       无参数时打开启动选择器");
    println!("  music <乐曲.txt|MIDI文件> [选项]");
    println!();
    println!("选项:");
    println!("  -d, --debug           详细调试输出");
    println!("  -s, --soundfont <p>   指定 SoundFont (.sf2/.sf3) 路径（可重复，最多 3 个）");
    println!("  -i, --instrument <n>  MIDI/简谱初始 GM Program (0-127)");
    println!("  -m, --midi <file>     直接播放 MIDI 文件（fluidsynth 原生多轨+变速）");
    println!("  -t, --tempo <ms>      覆盖速度 (>0 ms/四分音符)");
    println!("  -b, --bpm <n>         覆盖速度 (1-60000 BPM)");
    println!("  -v, --volume <0-500>  音量（默认 80%，最大 500%）");
    println!("  -l, --limit <dB>      峰值限制电平 (默认 -1.0 dBFS，防止削波)");
    println!("  -h, --help            帮助");
    println!();
    println!("说明:");
    println!("  默认模式：播放简谱 TXT（自动识别新版多轨/旧版左右手格式）");
    println!("  使用 -m 或传入 .mid 文件：直接播放 MIDI（多轨与变速完全准确）");
    println!("  传入音频文件（wav/mp3/flac/ogg/opus/aac/m4a/wma）：直接播放，");
    println!("  TUI 解析并显示内嵌专辑封面与作曲家等元数据（若有）");
    println!();
    println!("播放控制 (类似 mpv):");
    println!("  ← / →         后退/快进 5 秒");
    println!("  ↑ / ↓         快进/后退 10 秒");
    println!("  PageUp/PgDn   快进/后退 1 分钟");
    println!("  空格 / P       暂停 / 继续");
    println!("  [ / ]         后退/快进 1 秒");
    println!("  R             切换循环播放");
    println!("  1-8           跳转到 10%-80%");
    println!("  9 / 0         降低 / 增加音量");
    println!("  Q             退出");
    println!();
    println!("环境变量:");
    println!("  MUSIC_AUDIO_DRIVER   指定 fluidsynth 音频驱动 (如 alsa, pulse, pipewire)");
}

struct Args {
    file: Option<String>,
    debug: bool,
    soundfonts: Vec<String>,
    instrument: u8,
    /// `-i` 或无参数启动器明确选择了 MIDI/TXT 的初始音色。
    instrument_selected: bool,
    program_switches: Vec<ProgramSwitch>,
    tempo: Option<u32>,
    volume: u32,
    limit_db: f32,
    midi: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut out = Args {
        file: None,
        debug: false,
        soundfonts: Vec::new(),
        instrument: 0,
        instrument_selected: false,
        program_switches: Vec::new(),
        tempo: None,
        volume: 80,
        limit_db: -1.0,
        midi: false,
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "-d" | "--debug" => out.debug = true,
            "-h" | "--help" => {
                print_usage();
                exit(0);
            }
            "-s" | "--soundfont" => {
                let v = args.next().ok_or("--soundfont 需要一个参数")?;
                out.soundfonts.push(v);
                if out.soundfonts.len() > synth::MAX_SOUNDFONTS {
                    return Err(format!("最多指定 {} 个 SoundFont", synth::MAX_SOUNDFONTS));
                }
            }
            "-i" | "--instrument" => {
                let v = args.next().ok_or("--instrument 需要一个参数 (0-127)")?;
                let program: u16 = v
                    .parse()
                    .map_err(|_| "音色编号需为 0-127 的整数")?;
                if program > 127 {
                    return Err("音色编号超出范围 (0-127)".into());
                }
                out.instrument = program as u8;
                out.instrument_selected = true;
            }
            "-m" | "--midi" => {
                let v = args.next().ok_or("--midi 需要一个参数")?;
                if out.file.is_some() {
                    return Err("只能指定一个乐曲文件".into());
                }
                out.file = Some(v);
                out.midi = true;
            }
            "-t" | "--tempo" => {
                let v = args.next().ok_or("--tempo 需要一个参数")?;
                let tempo: u32 = v.parse().map_err(|_| "速度需为整数毫秒")?;
                if tempo == 0 {
                    return Err("速度必须大于 0 毫秒".into());
                }
                out.tempo = Some(tempo);
            }
            "-b" | "--bpm" => {
                let v = args.next().ok_or("--bpm 需要一个参数")?;
                let bpm: u32 = v.parse().map_err(|_| "BPM 需为整数")?;
                if !(1..=60_000).contains(&bpm) {
                    return Err("BPM 必须在 1-60000 之间".into());
                }
                out.tempo = Some(60_000 / bpm);
            }
            "-v" | "--volume" => {
                let v = args.next().ok_or("--volume 需要一个参数")?;
                let vol: u32 = v.parse().map_err(|_| "音量需为 0-500 整数")?;
                if !(0..=500).contains(&vol) {
                    return Err("音量超出范围 (0-500%)".into());
                }
                out.volume = vol;
            }
            "-l" | "--limit" => {
                let v = args.next().ok_or("--limit 需要一个参数 (dBFS, 如 -1.0)")?;
                let db: f32 = v.parse().map_err(|_| "限制电平需为数字 (dBFS)")?;
                if db > 0.0 {
                    return Err("限制电平必须 <= 0 dBFS".into());
                }
                out.limit_db = db;
            }
            _ => {
                if a.starts_with('-') {
                    return Err(format!("未知选项: {}", a));
                }
                if let Some(existing) = &out.file {
                    return Err(format!("只能指定一个乐曲文件: {} 和 {}", existing, a));
                }
                out.file = Some(a);
            }
        }
    }
    Ok(out)
}

fn main() {
    // 只有完全不带命令行参数时才进入启动选择器。这样 `music --debug`
    // 之类的“带参数但漏了文件”仍保持原有的参数错误提示，不会意外进入交互界面。
    let no_cli_args = std::env::args_os().len() == 1;
    let mut args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("参数错误: {}", e);
            print_usage();
            exit(1);
        }
    };

    log::init(args.debug);

    // 启动选择器是一次性的独立备用屏；返回时已经恢复终端，后续仍沿用
    // 原有的音频 / MIDI / 简谱分流和播放 TUI，不让选择状态混入主 TUI。
    if no_cli_args {
        match launcher::select_startup() {
            Ok(Some(selection)) => {
                args.file = Some(selection.file);
                args.soundfonts = selection.soundfonts;
                args.instrument = selection.instrument;
                args.instrument_selected = true;
                args.program_switches = selection.program_switches;
            }
            Ok(None) => return,
            Err(e) => {
                error(format!("启动选择界面失败: {}", e));
                exit(1);
            }
        }
    }

    let file = match &args.file {
        Some(f) => f.clone(),
        None => {
            eprintln!("未指定乐曲文件！");
            print_usage();
            exit(1);
        }
    };

    info(format!("music_rust —— 钢琴演奏器 v{}", VERSION));
    info(format!("乐曲文件: {}", file));

    if !args.midi && is_audio_file(&file) {
        if let Err(e) = play_audio_file(&file, args.volume, !args.debug) {
            error(format!("音频播放失败: {}", e));
            exit(1);
        }
        return;
    }

    // 判断是否为 MIDI 文件（-m 参数或 .mid/.midi 扩展名）
    let is_midi = args.midi || is_midi_file(&file);

    let soundfonts = match configured_soundfonts(&args.soundfonts) {
        Ok(paths) => paths,
        Err(e) => {
            error(format!("SoundFont 配置无效: {}", e));
            exit(1);
        }
    };

    if is_midi {
        // ---- MIDI 模式：fluidsynth 原生播放器 ----
        // 初始化合成器（tempo_ms 仅占位，MIDI 播放器自主处理速度）
        let mut player = match SynthPlayer::new_with_soundfonts(&soundfonts, 500, args.debug, args.limit_db) {
            Ok(p) => p,
            Err(e) => {
                error(format!("音频引擎初始化失败: {}", e));
                exit(1);
            }
        };
        player.set_volume_percent(args.volume);

        // 覆盖速度：-b/-t 参数 → BPM
        let bpm_override = args.tempo.map(|ms| 60_000.0 / ms as f64);

        // 估算总时长（用于进度条）
        let total_ms = midi_total_ms(&file).unwrap_or(0);
        if total_ms > 0 {
            info(format!("MIDI 总时长约 {}s", total_ms / 1000));
        }

        let start = std::time::Instant::now();
        if !args.program_switches.is_empty() {
            info(format!("MIDI 中途音色切换: {} 条", args.program_switches.len()));
        }
        // 调试模式禁用进度条；交互控制始终启用（终端可用时）
        match player.play_midi(
            &file,
            bpm_override,
            !args.debug,
            true,
            total_ms,
            args.instrument_selected.then_some((0, args.instrument)),
            &args.program_switches,
        ) {
            Ok(()) => {
                info(format!("演奏完成，用时 {:.2}s", start.elapsed().as_secs_f64()));
            }
            Err(e) => {
                error(format!("MIDI 播放失败: {}", e));
                player.shutdown();
                exit(1);
            }
        }
        player.shutdown();
        debug("播放器已退出".to_string());
        return;
    }

    // ---- 简谱 TXT 模式 ----
    // 解析
    let score = match parse_file(&file, args.tempo) {
        Ok(s) => s,
        Err(e) => {
            error(format!("解析失败: {}", e));
            exit(1);
        }
    };
    print_score_summary(&score);
    print_first_events(&score, 20);

    // TXT v3.1/v3.2 的音色计划属于乐谱内容，确保转换后的文件无需再次手工输入
    // 就能复现；v3.0 及旧格式继续沿用启动选择器/命令行配置。
    let (initial_soundfont, initial_instrument, program_switches) =
        if let Some(plan) = &score.program_plan {
            let switches = plan
                .switches
                .iter()
                .map(|switch_| ProgramSwitch {
                    at_ms: switch_.at_ms,
                    soundfont: switch_.soundfont,
                    instrument: switch_.instrument,
                })
                .collect::<Vec<_>>();
            info(format!(
                "TXT v3.x 音色计划: 初始 SoundFont #{} / GM Program {}，中途切换 {} 条",
                plan.initial_soundfont + 1,
                plan.initial_instrument,
                switches.len()
            ));
            (plan.initial_soundfont, plan.initial_instrument, switches)
        } else {
            (0, args.instrument, args.program_switches.clone())
        };

    let score_art = score.image.as_ref().and_then(|image| {
        match audio_file::decode_image(&image.data) {
            Some(art) => {
                info(format!("TXT 内嵌图片已解码: {} ({} bytes)", image.mime, image.data.len()));
                Some(art)
            }
            None => {
                log::warn(format!("TXT 内嵌图片无法解码（{}），继续播放乐谱", image.mime));
                None
            }
        }
    });

    // 初始化合成器
    let mut player = match SynthPlayer::new_with_soundfonts(&soundfonts, score.tempo_ms, args.debug, args.limit_db) {
        Ok(p) => p,
        Err(e) => {
            error(format!("音频引擎初始化失败: {}", e));
            exit(1);
        }
    };
    player.set_volume_percent(args.volume);

    // 定时 program-select 本身是异步事件，不能反馈预置缺失；在进入 TUI
    // 之前同步验证全部要求，任何缺失都立即关闭引擎并退出。
    if let Err(err) = player.validate_program_requirements(
        initial_soundfont,
        initial_instrument,
        &program_switches,
    ) {
        error(format!("TXT 音色检查失败: {}", err));
        player.shutdown();
        exit(1);
    }

    // 设置每个音轨的初始乐器。v3.1/v3.2 使用文件内要求；旧格式默认 GM 0，
    // 并可由 --instrument 或无参数启动器覆盖。
    for track in &score.tracks {
        if let Err(err) = player.set_soundfont_instrument(
            track.channel as i32,
            initial_soundfont,
            initial_instrument,
        ) {
            error(format!("设置简谱音色失败: {}", err));
            player.shutdown();
            exit(1);
        }
    }
    info(format!(
        "简谱初始音色: SoundFont #{} / GM Program {}",
        initial_soundfont + 1,
        initial_instrument
    ));

    // 收集全部事件（已按 at_ms 排序）
    let mut events: Vec<parser::ScheduledNote> = Vec::new();
    for track in &score.tracks {
        events.extend(track.events.iter().cloned());
    }
    events.sort_by(|a, b| {
        a.at_ms
            .cmp(&b.at_ms)
            .then_with(|| a.on.cmp(&b.on))
    });
    info(format!("已收集 {} 个 MIDI 事件", events.len()));
    if !program_switches.is_empty() {
        info(format!("简谱中途音色切换: {} 条", program_switches.len()));
    }

    // 开始交互式播放（调试模式禁用进度条）
    info(format!("开始演奏（总时长约 {}s）...", score.total_ms / 1000));
    let start = std::time::Instant::now();
    if let Err(err) = player.play_events_interactive(
        &events,
        score.total_ms,
        !args.debug,
        initial_soundfont,
        initial_instrument,
        &program_switches,
        score_art,
    ) {
        error(format!("简谱播放失败: {}", err));
        player.shutdown();
        exit(1);
    }
    let elapsed = start.elapsed();
    info(format!("演奏完成，用时 {:.2}s", elapsed.as_secs_f64()));

    player.shutdown();
    debug("播放器已退出".to_string());
}

/// 判断是否为 MIDI 文件（按扩展名）
fn is_midi_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".mid") || lower.ends_with(".midi")
}

fn configured_soundfonts(paths: &[String]) -> Result<Vec<String>, String> {
    let paths = if paths.is_empty() {
        vec![find_soundfont(None).ok_or_else(|| {
            "未找到任何 SoundFont (.sf2/.sf3)，请使用 --soundfont 指定路径".to_string()
        })?]
    } else {
        paths.to_vec()
    };
    synth::validate_soundfont_paths(&paths)?;
    Ok(paths)
}

fn is_audio_file(path: &str) -> bool {
    matches!(path.rsplit('.').next().map(|s| s.to_ascii_lowercase()).as_deref(),
        Some("wav" | "mp3" | "flac" | "ogg" | "opus" | "aac" | "m4a" | "wma"))
}

fn play_audio_file(path: &str, volume: u32, show_tui: bool) -> Result<(), String> {
    let mut player = audio_file::AudioFilePlayer::open(path)?;
    player.set_volume_percent(volume);
    player.play();
    let mut input = input::InputListener::start();

    // 提取元数据（标题/作曲家等）与内嵌封面（MP3 APIC / FLAC PICTURE / M4A covr）
    // MP3/FLAC/M4A 有专辑封面时优先显示专辑封面，否则显示编译进 ELF 的 GitHub 头像。
    let art = player.art().or_else(|| Some(tui::github_avatar()));
    let mut meta = tui::MetaInfo::default();
    for (key, slot) in [
        ("title", &mut meta.title),
        ("artist", &mut meta.artist),
        ("album", &mut meta.album),
        ("composer", &mut meta.composer),
        ("date", &mut meta.date),
        ("genre", &mut meta.genre),
    ] {
        *slot = player.metadata(key);
    }
    let display_title = meta.title.clone().unwrap_or_else(|| path.to_string());
    let mut tui = tui::Tui::start_full(&display_title, "音乐文件", show_tui, art, meta);
    let mut paused = false;
    let mut looping = false;
    loop {
        loop {
            match input.poll() {
                input::Control::None => break,
                input::Control::Quit => { input.stop(); return Ok(()); }
                input::Control::Pause => { paused = !paused; if paused { player.pause(); } else { player.play(); } }
                input::Control::Play => { paused = false; player.play(); }
                input::Control::VolumeDown => player.set_volume_percent(player.volume_percent().saturating_sub(10)),
                input::Control::VolumeUp => player.set_volume_percent(player.volume_percent().saturating_add(10).min(500)),
                input::Control::SeekForward(s) => player.seek(player.position_ms() as i64 + (s * 1000.0) as i64),
                input::Control::SeekBackward(s) => player.seek(player.position_ms() as i64 - (s * 1000.0) as i64),
                input::Control::SeekPercent(p) => player.seek((player.duration_ms() as f64 * p) as i64),
                input::Control::Mouse(x, y) => {
                    if let Some(ui) = &tui {
                        match ui.mouse_control(x, y, paused) {
                            input::Control::Pause => { paused = true; player.pause(); }
                            input::Control::Play => { paused = false; player.play(); }
                            input::Control::SeekPercent(p) => player.seek((player.duration_ms() as f64 * p) as i64),
                            _ => {}
                        }
                    }
                }
                input::Control::Loop => looping = !looping,
            }
        }
        let position = player.position_ms();
        let duration = player.duration_ms();
        if let Some(ui) = &mut tui {
            ui.draw(position, duration, player.volume_percent(), paused, looping, &[], &player.spectrum());
        }
        if player.finished() {
            if looping {
                player.seek(0);
                player.play();
                paused = false;
            } else {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    input.stop();
    drop(tui);
    Ok(())
}

/// 轻量解析 MIDI 文件，估算总时长（毫秒）。
/// 读取 division、tempo 事件、最长 tick，换算总时长。
/// 失败或异常时返回 None（调用方回退到仅显示经过时间）。
fn midi_total_ms(path: &str) -> Option<u32> {
    use std::io::Read;
    let mut data = Vec::new();
    std::fs::File::open(path).ok()?.read_to_end(&mut data).ok()?;
    if data.len() < 14 || &data[0..4] != b"MThd" {
        return None;
    }
    // 读取 header: MThd + len + format + ntrks + division
    let division = u16::from_be_bytes([data[12], data[13]]);
    if division == 0 || division & 0x8000 != 0 {
        return None;
    }
    // 遍历轨道，收集 tempo 事件和最大 tick
    let mut pos = 14usize;
    let mut max_tick: u64 = 0;
    let mut tempos: Vec<(u64, u32)> = Vec::new(); // (tick, us_per_quarter)

    fn read_varlen(data: &[u8], pos: &mut usize) -> Option<u64> {
        let mut v: u64 = 0;
        for _ in 0..4 {
            let b = *data.get(*pos)?;
            *pos += 1;
            v = (v << 7) | (b & 0x7f) as u64;
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }

    // 遍历所有 MTrk
    while pos + 8 <= data.len() {
        if &data[pos..pos + 4] != b"MTrk" {
            break;
        }
        let tlen = u32::from_be_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]) as usize;
        pos += 8;
        let end = pos + tlen;
        if end > data.len() {
            break;
        }
        let mut tick: u64 = 0;
        let mut running: Option<u8> = None;
        while pos < end {
            let dt = read_varlen(&data, &mut pos)?;
            tick += dt;
            let first = *data.get(pos)?;
            pos += 1;
            if first < 0x80 {
                // running status
                let s = running?;
                let msg = s & 0xf0;
                match msg {
                    // first data byte was already consumed above.
                    0x80 | 0x90 | 0xa0 | 0xb0 | 0xe0 => { pos += 1; }
                    0xc0 | 0xd0 => {}
                    _ => {}
                }
                max_tick = max_tick.max(tick);
                continue;
            }
            let status = first;
            running = Some(status);
            match status {
                0xff => {
                    // meta event
                    let mtype = *data.get(pos)?;
                    pos += 1;
                    let mlen = read_varlen(&data, &mut pos)? as usize;
                    // 元事件载荷必须完整落在当前音轨内，否则按损坏文件处理，
                    // 避免长度声明超出实际数据时越界读取（此前会 panic）。
                    let payload_end = pos.checked_add(mlen)?;
                    if payload_end > end {
                        return None;
                    }
                    if mtype == 0x51 && mlen >= 3 {
                        // tempo
                        let us = ((data[pos] as u32) << 16) | ((data[pos + 1] as u32) << 8) | data[pos + 2] as u32;
                        tempos.push((tick, us));
                    }
                    pos = payload_end;
                }
                0xf0 | 0xf7 => {
                    let slen = read_varlen(&data, &mut pos)? as usize;
                    let payload_end = pos.checked_add(slen)?;
                    if payload_end > end {
                        return None;
                    }
                    pos = payload_end;
                }
                m => {
                    // 按消息类型判断数据字节数时必须先屏蔽通道号：否则 0x91、
                    // 0x82 等非 0 通道事件会匹配不到任何分支，导致解析错位。
                    match m & 0xf0 {
                        0x80 | 0x90 | 0xa0 | 0xb0 | 0xe0 => { pos += 2; }
                        0xc0 | 0xd0 => { pos += 1; }
                        _ => {}
                    }
                    max_tick = max_tick.max(tick);
                }
            }
        }
    }

    if tempos.is_empty() {
        // 默认 120 BPM (500000 us)
        tempos.push((0, 500000));
    }
    tempos.sort_by_key(|(t, _)| *t);

    // 分段累计毫秒
    let mut ms: f64 = 0.0;
    let mut prev_tick: u64 = 0;
    let mut cur_us: u32 = 500000;
    for (t, us) in &tempos {
        let seg = (*t - prev_tick) as f64 / division as f64 * cur_us as f64 / 1000.0;
        ms += seg;
        prev_tick = *t;
        cur_us = *us;
    }
    let seg = (max_tick - prev_tick) as f64 / division as f64 * cur_us as f64 / 1000.0;
    ms += seg;

    Some((ms as u32).max(1))
}

#[cfg(test)]
mod tests {
    use super::{is_audio_file, midi_total_ms};
    use std::path::PathBuf;

    #[test]
    fn recognizes_ffmpeg_audio_extensions() {
        assert!(is_audio_file("song.MP3"));
        assert!(is_audio_file("voice.flac"));
        assert!(!is_audio_file("score.txt"));
        assert!(!is_audio_file("song.mid"));
    }

    /// 写一个单音轨 MIDI 到临时目录，返回路径。
    fn write_temp_midi(payload: &[u8]) -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "music_rust_midi_test_{}_{}.mid",
            std::process::id(),
            stamp
        ));
        let mut midi = b"MThd".to_vec();
        // format 0、1 个音轨、division 480
        midi.extend_from_slice(&[0, 0, 0, 6, 0, 0, 0, 1, 0x01, 0xE0]);
        midi.extend_from_slice(b"MTrk");
        midi.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        midi.extend_from_slice(payload);
        std::fs::write(&path, midi).unwrap();
        path
    }

    #[test]
    fn midi_total_ms_handles_channels_other_than_zero() {
        // 120 BPM 下的一个四分音符（480 tick）放在通道 1（0x91/0x81），
        // 应估算为 500ms；旧实现会因状态字节匹配不到而错位成 17 秒级。
        let payload = [
            0x00, 0x91, 0x3C, 0x64, // delta 0, note-on ch2, key 60, vel 100
            0x83, 0x60, 0x81, 0x3C, 0x00, // delta 480, note-off ch2
            0x00, 0xFF, 0x2F, 0x00, // end of track
        ];
        let path = write_temp_midi(&payload);
        let result = midi_total_ms(path.to_str().unwrap());
        let _ = std::fs::remove_file(&path);
        assert_eq!(result, Some(500));
    }

    #[test]
    fn midi_total_ms_rejects_truncated_tempo_event() {
        // tempo 元事件声明 3 字节但文件已截断：应返回 None 而不是越界 panic。
        let payload = [0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1];
        let path = write_temp_midi(&payload);
        let result = midi_total_ms(path.to_str().unwrap());
        let _ = std::fs::remove_file(&path);
        assert_eq!(result, None);
    }
}
