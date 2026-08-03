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
//!   -v, --volume <0-127> 音量
//!   -h, --help           帮助

mod log;
mod parser;
mod progress;
mod synth;

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
    println!("  -v, --volume <0-127>  音量");
    println!("  -l, --limit <dB>      峰值限制电平 (默认 -1.0 dBFS，防止削波)");
    println!("  -h, --help            帮助");
    println!();
    println!("说明:");
    println!("  默认模式：播放简谱 TXT（自动识别新版多轨/旧版左右手格式）");
    println!("  使用 -m 或传入 .mid 文件：直接播放 MIDI（多轨与变速完全准确）");
    println!();
    println!("环境变量:");
    println!("  MUSIC_AUDIO_DRIVER   指定 fluidsynth 音频驱动 (如 alsa, pulse, pipewire)");
}

struct Args {
    file: Option<String>,
    debug: bool,
    soundfont: Option<String>,
    tempo: Option<u32>,
    volume: u8,
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
        volume: 96,
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
                let vol: u32 = v.parse().map_err(|_| "音量需为 0-127 整数")?;
                if vol > 127 {
                    return Err("音量超出范围 (0-127)".into());
                }
                out.volume = vol as u8;
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

        // 覆盖速度：-b/-t 参数 → BPM
        let bpm_override = args.tempo.map(|ms| 60_000.0 / ms as f64);

        // 估算总时长（用于进度条）
        let total_ms = midi_total_ms(&file).unwrap_or(0);
        if total_ms > 0 {
            info(format!("MIDI 总时长约 {}s", total_ms / 1000));
        }

        let start = std::time::Instant::now();
        match player.play_midi(&file, bpm_override, true, total_ms) {
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

    // 排程所有事件
    info(format!("开始排程 {} 个音轨的事件 ...", score.tracks.len()));
    let mut total_events = 0usize;
    for track in &score.tracks {
        player.set_instrument(track.channel as i32, 0);
        for ev in &track.events {
            player.play_note(ev.channel, ev.key, ev.vel, ev.on, ev.at_ms);
            total_events += 1;
        }
    }
    info(format!("已排程 {} 个 MIDI 事件", total_events));

    // 等待播放结束
    info(format!("开始演奏（总时长约 {}s）...", score.total_ms / 1000));
    let start = std::time::Instant::now();
    player.wait_until(score.total_ms + 200, true);
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
