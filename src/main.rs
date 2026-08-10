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
//! 用法:
//!   music <乐曲.txt> [选项]
//!
//! 选项:
//!   -d, --debug          详细调试输出
//!   -s, --soundfont <p>  指定 SoundFont (.sf2/.sf3) 路径
//!   -m, --midi <file>    直接播放 MIDI 文件（fluidsynth 原生多轨+变速）
//!   -t, --tempo <ms>     覆盖速度 (ms/四分音符)
//!   -b, --bpm <n>        覆盖速度 (BPM)
//!   -v, --volume <0-500> 音量（默认 80%）
//!   -h, --help           帮助
//!
//! 播放控制（类似 mpv）:
//!   ←/→ 后退/快进5秒, ↑/↓ 快进/后退10秒, PageUp/PgDn 快进/后退1分钟
//!   空格/P 暂停/继续, [ / ] 微调1秒, R 循环, 1-8 跳转10%-80%, Q 退出
//!   9/0 降低/增加音量

mod input;
mod audio_file;
mod log;
mod parser;
mod progress;
mod synth;
mod tui;

use std::process::exit;

use log::{debug, error, info};
use parser::{parse_file, print_first_events, print_score_summary};
use synth::SynthPlayer;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_usage() {
    println!("music_rust —— 钢琴演奏器 v{}", VERSION);
    println!();
    println!("用法: music <乐曲.txt|MIDI文件> [选项]");
    println!();
    println!("选项:");
    println!("  -d, --debug           详细调试输出");
    println!("  -s, --soundfont <p>   指定 SoundFont (.sf2/.sf3) 路径");
    println!("  -m, --midi <file>     直接播放 MIDI 文件（fluidsynth 原生多轨+变速）");
    println!("  -t, --tempo <ms>      覆盖速度 (ms/四分音符)");
    println!("  -b, --bpm <n>         覆盖速度 (BPM)");
    println!("  -v, --volume <0-500>  音量（默认 80%，最大 500%）");
    println!("  -l, --limit <dB>      峰值限制电平 (默认 -1.0 dBFS，防止削波)");
    println!("  -h, --help            帮助");
    println!();
    println!("说明:");
    println!("  默认模式：播放简谱 TXT（自动识别新版多轨/旧版左右手格式）");
    println!("  使用 -m 或传入 .mid 文件：直接播放 MIDI（多轨与变速完全准确）");
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
    soundfont: Option<String>,
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
        soundfont: None,
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
                out.soundfont = Some(v);
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
                out.tempo = Some(v.parse().map_err(|_| "速度需为整数毫秒")?);
            }
            "-b" | "--bpm" => {
                let v = args.next().ok_or("--bpm 需要一个参数")?;
                let bpm: u32 = v.parse().map_err(|_| "BPM 需为整数")?;
                if bpm == 0 {
                    return Err("BPM 不能为 0".into());
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
                if out.file.is_some() {
                    return Err(format!("只能指定一个乐曲文件: {} 和 {}", out.file.as_ref().unwrap(), a));
                }
                out.file = Some(a);
            }
        }
    }
    Ok(out)
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("参数错误: {}", e);
            print_usage();
            exit(1);
        }
    };

    log::init(args.debug);

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

    if is_midi {
        // ---- MIDI 模式：fluidsynth 原生播放器 ----
        // 初始化合成器（tempo_ms 仅占位，MIDI 播放器自主处理速度）
        let mut player = match SynthPlayer::new(args.soundfont.as_deref(), 500, args.debug, args.limit_db) {
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
        // 调试模式禁用进度条；交互控制始终启用（终端可用时）
        match player.play_midi(&file, bpm_override, !args.debug, true, total_ms) {
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

    // 初始化合成器
    let mut player = match SynthPlayer::new(args.soundfont.as_deref(), score.tempo_ms, args.debug, args.limit_db) {
        Ok(p) => p,
        Err(e) => {
            error(format!("音频引擎初始化失败: {}", e));
            exit(1);
        }
    };
    player.set_volume_percent(args.volume);

    // 设置每个音轨的乐器（钢琴 GM Program 0）
    for track in &score.tracks {
        player.set_instrument(track.channel as i32, 0);
    }

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

    // 开始交互式播放（调试模式禁用进度条）
    info(format!("开始演奏（总时长约 {}s）...", score.total_ms / 1000));
    let start = std::time::Instant::now();
    player.play_events_interactive(&events, score.total_ms, !args.debug);
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

fn is_audio_file(path: &str) -> bool {
    matches!(path.rsplit('.').next().map(|s| s.to_ascii_lowercase()).as_deref(),
        Some("wav" | "mp3" | "flac" | "ogg" | "opus" | "aac" | "m4a" | "wma"))
}

fn play_audio_file(path: &str, volume: u32, show_tui: bool) -> Result<(), String> {
    let mut player = audio_file::AudioFilePlayer::open(path)?;
    player.set_volume_percent(volume);
    player.play();
    let mut input = input::InputListener::start();
    let mut tui = tui::Tui::start(path, "音乐文件", show_tui);
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
            ui.draw(position, duration, player.volume_percent(), paused, looping, &["动态频率图（20 Hz - 10 kHz）".to_string()], &player.spectrum());
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

#[cfg(test)]
mod tests {
    use super::is_audio_file;

    #[test]
    fn recognizes_ffmpeg_audio_extensions() {
        assert!(is_audio_file("song.MP3"));
        assert!(is_audio_file("voice.flac"));
        assert!(!is_audio_file("score.txt"));
        assert!(!is_audio_file("song.mid"));
    }
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
    if division == 0 {
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
            let status = *data.get(pos)?;
            pos += 1;
            if status < 0x80 {
                // running status
                let s = running?;
                let msg = s & 0xf0;
                match msg {
                    0x80 | 0x90 | 0xa0 | 0xb0 | 0xe0 => { pos += 2; }
                    0xc0 | 0xd0 => { pos += 1; }
                    _ => {}
                }
                max_tick = max_tick.max(tick);
                continue;
            }
            running = Some(status);
            match status {
                0xff => {
                    // meta event
                    let mtype = *data.get(pos)?;
                    pos += 1;
                    let mlen = read_varlen(&data, &mut pos)? as usize;
                    if mtype == 0x51 && mlen >= 3 {
                        // tempo
                        let us = ((data[pos] as u32) << 16) | ((data[pos + 1] as u32) << 8) | data[pos + 2] as u32;
                        tempos.push((tick, us));
                    }
                    pos += mlen;
                }
                0xf0 | 0xf7 => {
                    let slen = read_varlen(&data, &mut pos)? as usize;
                    pos += slen;
                }
                m => {
                    match m {
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
