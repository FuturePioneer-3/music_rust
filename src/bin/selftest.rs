#![allow(dead_code)]
#![allow(non_camel_case_types)]
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

//! 音频自检程序
//!
//! 编译: cargo build --release --bin selftest
//! 运行: ./target/release/selftest
//!
//! 播放一段 C 大调音阶 + 和弦，用于验证：
//!   1. fluidsynth 库链接正常
//!   2. SoundFont 加载正常
//!   3. 音频驱动输出正常（应能听到钢琴声）
//!
//! 若听不到声音，尝试:
//!   MUSIC_AUDIO_DRIVER=alsa ./target/release/selftest
//!   MUSIC_AUDIO_DRIVER=pulseaudio ./target/release/selftest

#[path = "../synth.rs"]
mod synth;
#[path = "../log.rs"]
mod log;
#[path = "../progress.rs"]
mod progress;
#[path = "../input.rs"]
mod input;
#[path = "../parser.rs"]
mod parser;
#[path = "../tui.rs"]
mod tui;
#[path = "../console.rs"]
mod console;

fn main() {
    log::init(true);

    let mut player = match synth::SynthPlayer::new(None, 400, true, -1.0) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("初始化失败: {}", e);
            std::process::exit(1);
        }
    };

    println!("[selftest] 播放 C 大调音阶 + 和弦 ...");
    println!("[selftest] 如果听不到声音，请检查音频设备或用 MUSIC_AUDIO_DRIVER 切换后端");

    // C 大调音阶 (中音区, 每个音 300ms)
    let scale = [60u8, 62, 64, 65, 67, 69, 71, 72];
    for (i, key) in scale.iter().enumerate() {
        let t = (i as u32) * 300;
        player.play_note(0, *key, 100, true, t);
        player.play_note(0, *key, 100, false, t + 280);
    }

    // C 大三和弦 + F 大三和弦 + G 大三和弦
    let chords: [[u8; 3]; 3] = [
        [48, 52, 55], // C3 E3 G3
        [53, 57, 60], // F3 A3 C4
        [55, 59, 62], // G3 B3 D4
    ];
    let base_t = 2400;
    for (ci, ch) in chords.iter().enumerate() {
        for n in ch {
            player.play_note(0, *n, 90, true, base_t + ci as u32 * 700);
            player.play_note(0, *n, 90, false, base_t + ci as u32 * 700 + 650);
        }
    }

    player.wait_until(4800, false);
    player.shutdown();
    println!("[selftest] 完成。");
}
