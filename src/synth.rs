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

//! fluidsynth FFI 封装 + 钢琴合成器播放器
//!
//! 直接通过 `extern "C"` 调用系统 libfluidsynth（libfluidsynth.so.x）。
//! 支持大多数 Linux 发行版：只要安装了 fluidsynth 运行时库即可。
//! SoundFont (.sf2/.sf3) 会依次尝试：用户指定路径 → 随包音源 → 常见系统路径 → 用户目录。
//!
//! 注：本模块同时被 selftest 通过 `#[path]` 复用，因此部分方法在不同 crate
//! 中可能被标记为 dead_code，这里统一允许。

#![allow(dead_code)]

use std::ffi::{c_char, c_int, c_short, c_uint, c_void, CString};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::log::{debug, info, warn};

// ---------------------------------------------------------------------------
// FFI 声明（对应 /usr/include/fluidsynth/*.h）
// ---------------------------------------------------------------------------

#[allow(non_camel_case_types)]
type fluid_settings_t = c_void;
#[allow(non_camel_case_types)]
type fluid_synth_t = c_void;
#[allow(non_camel_case_types)]
type fluid_sequencer_t = c_void;
#[allow(non_camel_case_types)]
type fluid_event_t = c_void;
#[allow(non_camel_case_types)]
type fluid_midi_event_t = c_void;
#[allow(non_camel_case_types)]
type fluid_seq_id_t = c_short;

#[allow(non_camel_case_types, dead_code)]
type fluid_player_t = c_void;

/// fluidsynth MIDI 播放器状态枚举
#[allow(dead_code)]
const FLUID_PLAYER_READY: c_int = 0;
const FLUID_PLAYER_PLAYING: c_int = 1;
#[allow(dead_code)]
const FLUID_PLAYER_STOPPING: c_int = 2;
const FLUID_PLAYER_DONE: c_int = 3;

/// fluidsynth 播放器 tempo 类型
#[allow(dead_code)]
const FLUID_PLAYER_TEMPO_INTERNAL: c_int = 0;
const FLUID_PLAYER_TEMPO_EXTERNAL_BPM: c_int = 1;
#[allow(dead_code)]
const FLUID_PLAYER_TEMPO_EXTERNAL_MIDI: c_int = 2;

/// fluidsynth 自定义音频回调：负责渲染音频到 out[] buffers
#[allow(non_camel_case_types)]
type audio_func_t = unsafe extern "C" fn(
    data: *mut c_void,
    len: c_int,
    nfx: c_int,
    fx: *mut *mut f32,
    nout: c_int,
    out: *mut *mut f32,
) -> c_int;

#[allow(non_camel_case_types)]
type midi_tick_func_t = unsafe extern "C" fn(data: *mut c_void, tick: c_int) -> c_int;
#[allow(non_camel_case_types)]
type midi_event_func_t = unsafe extern "C" fn(
    data: *mut c_void,
    event: *mut fluid_midi_event_t,
) -> c_int;

const FLUID_OK: c_int = 0;
const FLUID_FAILED: c_int = -1;

/// 用户界面音量是相对于合成器基准增益的线性倍率。
///
/// FluidSynth 的 `synth.gain` 是一个内部增益参数；这里保持项目原有的
/// 约定（100% = 1.0），不要把它和 FluidSynth 的默认配置值 0.2 混用。
/// 这样 MIDI/TXT 与音频文件模式的 0%-500% 音量语义一致。
const DEFAULT_VOLUME_SCALE: f32 = 0.8;
const MAX_VOLUME_SCALE: f32 = 5.0;
const MAX_VOLUME_PERCENT: u32 = 500;

#[inline]
fn volume_scale_from_percent(percent: u32) -> f32 {
    percent.min(MAX_VOLUME_PERCENT) as f32 / 100.0
}

#[inline]
fn volume_percent_from_scale(scale: f32) -> u32 {
    (scale.clamp(0.0, MAX_VOLUME_SCALE) * 100.0).round() as u32
}

// ---------------------------------------------------------------------------
// 音频限制器 (Limiter)
// ---------------------------------------------------------------------------
//
// 作用：将合成器输出峰值硬限制在指定电平（默认 -1dBFS），防止削波/电流声。
//      不主动提升音量（保持原动态），仅当峰值超过目标电平时才压缩。
//      超过目标电平的部分用平滑增益压下来，避免爆音。
//
// 实现：
//   1. 分析缓冲区峰值
//   2. 若峰值超过 target，计算需要的增益并快速压降（attack）
//   3. 未超限时缓慢恢复增益（release），避免泵浦感
//   4. 逐样本应用增益 + 硬钳制，保证任何时刻不超 target

/// 限制器状态（全局，由音频回调访问）
///
/// 布局与 C 侧 `dsp_limiter` 一致（4×f32），热路径由 x86-64 SSE2
/// 汇编 `dsp_limiter_f32` 处理（src/audio_dsp.S），压榨 CPU 效率。
#[repr(C)]
struct LimiterState {
    /// 目标峰值电平（线性满刻度，1.0 = 0dBFS）
    target: f32,
    /// 当前增益包络
    current_gain: f32,
    /// 增益平滑系数（越大越快）
    attack_coef: f32,
    release_coef: f32,
}

extern "C" {
    /// 汇编：f32 块级峰值限制器（src/audio_dsp.S）
    fn dsp_limiter_f32(buf: *mut f32, len: u32, st: *mut LimiterState);
}

impl LimiterState {
    fn new(target_db: f32) -> Self {
        LimiterState {
            target: db_to_lin(target_db),
            current_gain: 1.0,
            attack_coef: 0.9,
            release_coef: 0.0006,
        }
    }

    /// 处理一个缓冲区的样本（整块分析 + 平滑增益 + 硬钳制）
    ///
    /// 峰值分析、增益包络与逐样本应用全部在 SSE2 汇编中完成：
    ///   1. 分析缓冲区峰值
    ///   2. 若峰值超过 target，计算需要的增益并快速压降（attack）
    ///   3. 未超限时缓慢恢复增益（release），避免泵浦感
    ///   4. 逐样本应用增益 + 硬钳制，保证任何时刻不超 target
    #[inline]
    unsafe fn process(&mut self, buf: *mut f32, len: usize) {
        dsp_limiter_f32(buf, len as u32, self as *mut LimiterState);
    }
}

#[inline]
fn db_to_lin(db: f32) -> f32 {
    (10.0_f32).powf(db / 20.0)
}

/// 在闭包执行期间将 stderr 重定向到 /dev/null，执行完后恢复。
/// 用于静默 fluidsynth 库初始化时对 ALSA/SDL 等后端的探测噪音，
/// 这些消息由 C 库直接写 stderr，无法通过回调过滤。
fn silence_stderr<T>(f: impl FnOnce() -> T) -> T {
    let saved = unsafe { libc::dup(libc::STDERR_FILENO) };
    if saved < 0 {
        return f();
    }
    let devnull = unsafe {
        libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY)
    };
    if devnull >= 0 {
        unsafe {
            libc::dup2(devnull, libc::STDERR_FILENO);
            libc::close(devnull);
        }
    }
    let r = f();
    unsafe {
        libc::dup2(saved, libc::STDERR_FILENO);
        libc::close(saved);
    }
    r
}

/// 全局限制器实例与合成器指针（音频回调使用）
static mut LIMITER: LimiterState = LimiterState {
    target: 0.891,       // -1 dBFS
    current_gain: 1.0,
    attack_coef: 0.9,
    release_coef: 0.0006,
};

static mut LIMITER_SYNTH: *mut fluid_synth_t = std::ptr::null_mut();

/// 音量变化后请求音频线程丢弃旧的 limiter 包络。
///
/// limiter 的 release 很慢（用来避免泵浦），如果用户从大音量调小，
/// 旧包络会让后续数秒的输出继续被压低，造成 UI 音量与实际响度不符。
/// 主线程只递增这个原子计数，音频回调独占修改 LIMITER，避免跨线程
/// 直接写 `static mut` 的数据竞争。
static LIMITER_RESET_REVISION: AtomicU32 = AtomicU32::new(0);
static mut LIMITER_RESET_SEEN: u32 = 0;

#[inline]
fn request_limiter_reset() {
    LIMITER_RESET_REVISION.fetch_add(1, Ordering::Release);
}

#[inline]
fn reset_limiter_if_requested(
    revision: u32,
    seen: *mut u32,
    limiter: *mut LimiterState,
) {
    if revision != unsafe { *seen } {
        unsafe {
            (*limiter).current_gain = 1.0;
            *seen = revision;
        }
    }
}

/// 音频回调：渲染 + 限制
unsafe extern "C" fn audio_render_callback(
    _data: *mut c_void,
    len: c_int,
    nfx: c_int,
    fx: *mut *mut f32,
    nout: c_int,
    out: *mut *mut f32,
) -> c_int {
    let synth = LIMITER_SYNTH;
    if synth.is_null() {
        return FLUID_FAILED;
    }
    // 渲染音频到 out[]。普通音频驱动通常不给回调提供 fx 缓冲区
    // （nfx == 0）；FluidSynth 文档要求此时用 dry 输出作为 effect
    // 缓冲别名，否则内置混响/合唱可能被丢弃，造成 MIDI/TXT 与预期响度
    // 不一致。常见输出是立体声，4 个 effect 槽按声道循环别名即可。
    let mut fx_alias = [std::ptr::null_mut(); 4];
    let ret = if nfx == 0 && nout > 0 && !out.is_null() {
        for (i, slot) in fx_alias.iter_mut().enumerate() {
            *slot = *out.add(i % nout as usize);
        }
        fluid_synth_process(
            synth,
            len,
            fx_alias.len() as c_int,
            fx_alias.as_mut_ptr(),
            nout,
            out,
        )
    } else {
        fluid_synth_process(synth, len, nfx, fx, nout, out)
    };
    if ret != FLUID_OK {
        return ret;
    }
    // 音量改变后不要沿用上一个音量的慢释放包络，否则实际响度会滞后
    // UI 数值数秒。LIMITER 只由音频回调线程写入，主线程通过原子版本号
    // 通知，避免直接跨线程访问 static mut。
    let l = &raw mut LIMITER;
    let revision = LIMITER_RESET_REVISION.load(Ordering::Acquire);
    let seen = &raw mut LIMITER_RESET_SEEN;
    reset_limiter_if_requested(revision, seen, l);
    // 对每个输出通道应用限制器
    for ch in 0..nout {
        let buf = *out.add(ch as usize);
        if !buf.is_null() {
            (*l).process(buf, len as usize);
        }
    }
    FLUID_OK
}

extern "C" {
    #[allow(dead_code)]
    fn new_fluid_audio_driver(settings: *mut fluid_settings_t, synth: *mut fluid_synth_t) -> *mut c_void;
    fn new_fluid_audio_driver2(
        settings: *mut fluid_settings_t,
        func: audio_func_t,
        data: *mut c_void,
    ) -> *mut c_void;
    fn delete_fluid_audio_driver(driver: *mut c_void);

    /// 2.4.0：汇编峰值限制器（src/music_asm.S，AT&T 语法，非内联）
    fn music_asm_limiter_process(buf: *mut f32, n: usize, target: f32, attack: f32, release: f32, gain: *mut f32);

    fn new_fluid_settings() -> *mut fluid_settings_t;
    fn delete_fluid_settings(s: *mut fluid_settings_t);
    fn fluid_settings_setstr(s: *mut fluid_settings_t, name: *const c_char, value: *const c_char) -> c_int;
    #[allow(dead_code)]
    fn fluid_settings_setnum(s: *mut fluid_settings_t, name: *const c_char, value: f64) -> c_int;
    fn fluid_settings_setint(s: *mut fluid_settings_t, name: *const c_char, value: c_int) -> c_int;
    #[allow(dead_code)]
    fn fluid_synth_noteon(s: *mut fluid_synth_t, chan: c_int, key: c_int, vel: c_int) -> c_int;
    #[allow(dead_code)]
    fn fluid_synth_noteoff(s: *mut fluid_synth_t, chan: c_int, key: c_int) -> c_int;

    fn new_fluid_synth(settings: *mut fluid_settings_t) -> *mut fluid_synth_t;
    fn delete_fluid_synth(synth: *mut fluid_synth_t);
    fn fluid_synth_sfload(synth: *mut fluid_synth_t, filename: *const c_char, reset_presets: c_int) -> c_int;
    fn fluid_synth_program_reset(synth: *mut fluid_synth_t) -> c_int;
    fn fluid_synth_program_change(synth: *mut fluid_synth_t, chan: c_int, program: c_int) -> c_int;
    fn fluid_synth_program_select(
        synth: *mut fluid_synth_t,
        chan: c_int,
        sfont_id: c_int,
        bank_num: c_int,
        preset_num: c_int,
    ) -> c_int;
    fn fluid_synth_get_program(
        synth: *mut fluid_synth_t,
        chan: c_int,
        sfont_id: *mut c_int,
        bank_num: *mut c_int,
        preset_num: *mut c_int,
    ) -> c_int;
    fn fluid_synth_unset_program(synth: *mut fluid_synth_t, chan: c_int) -> c_int;
    fn fluid_synth_cc(synth: *mut fluid_synth_t, chan: c_int, ctrl: c_int, val: c_int) -> c_int;
    fn fluid_synth_all_notes_off(synth: *mut fluid_synth_t, chan: c_int) -> c_int;
    fn fluid_synth_all_sounds_off(synth: *mut fluid_synth_t, chan: c_int) -> c_int;
    fn fluid_synth_set_gain(synth: *mut fluid_synth_t, gain: f32);
    #[allow(dead_code)]
    fn fluid_synth_get_gain(synth: *mut fluid_synth_t) -> f32;
    fn fluid_synth_process(
        synth: *mut fluid_synth_t,
        len: c_int,
        nfx: c_int,
        fx: *mut *mut f32,
        nout: c_int,
        out: *mut *mut f32,
    ) -> c_int;

    fn new_fluid_sequencer2(use_system_timer: c_int) -> *mut fluid_sequencer_t;
    fn delete_fluid_sequencer(seq: *mut fluid_sequencer_t);
    fn fluid_sequencer_register_fluidsynth(seq: *mut fluid_sequencer_t, synth: *mut fluid_synth_t) -> fluid_seq_id_t;
    fn fluid_sequencer_set_time_scale(seq: *mut fluid_sequencer_t, scale: f64);
    fn fluid_sequencer_get_tick(seq: *mut fluid_sequencer_t) -> c_uint;
    fn fluid_sequencer_send_at(
        seq: *mut fluid_sequencer_t,
        evt: *mut fluid_event_t,
        time: c_uint,
        absolute: c_int,
    ) -> c_int;
    fn fluid_sequencer_send_now(seq: *mut fluid_sequencer_t, evt: *mut fluid_event_t);
    fn fluid_sequencer_remove_events(seq: *mut fluid_sequencer_t, source: fluid_seq_id_t, dest: fluid_seq_id_t, ty: c_int);

    fn new_fluid_event() -> *mut fluid_event_t;
    fn delete_fluid_event(evt: *mut fluid_event_t);
    fn fluid_event_set_source(evt: *mut fluid_event_t, src: fluid_seq_id_t);
    fn fluid_event_set_dest(evt: *mut fluid_event_t, dest: fluid_seq_id_t);
    fn fluid_event_noteon(evt: *mut fluid_event_t, channel: c_int, key: c_short, vel: c_short);
    fn fluid_event_noteoff(evt: *mut fluid_event_t, channel: c_int, key: c_short);
    fn fluid_event_program_change(evt: *mut fluid_event_t, channel: c_int, preset_num: c_int);
    fn fluid_event_program_select(
        evt: *mut fluid_event_t,
        channel: c_int,
        sfont_id: c_uint,
        bank_num: c_short,
        preset_num: c_short,
    );

    // ---- MIDI 文件播放器 (fluid_player) ----
    fn new_fluid_player(synth: *mut fluid_synth_t) -> *mut c_void;
    fn delete_fluid_player(player: *mut c_void);
    fn fluid_player_add(player: *mut c_void, midifile: *const c_char) -> c_int;
    fn fluid_player_play(player: *mut c_void) -> c_int;
    #[allow(dead_code)]
    fn fluid_player_stop(player: *mut c_void) -> c_int;
    #[allow(dead_code)]
    fn fluid_player_join(player: *mut c_void) -> c_int;
    fn fluid_player_get_status(player: *mut c_void) -> c_int;
    fn fluid_player_set_tempo(player: *mut c_void, tempo_type: c_int, tempo: f64) -> c_int;
    fn fluid_player_set_tick_callback(
        player: *mut c_void,
        handler: midi_tick_func_t,
        data: *mut c_void,
    ) -> c_int;
    fn fluid_player_set_playback_callback(
        player: *mut c_void,
        handler: midi_event_func_t,
        data: *mut c_void,
    ) -> c_int;
    fn fluid_player_get_bpm(player: *mut c_void) -> c_int;
    #[allow(dead_code)]
    fn fluid_player_set_loop(player: *mut c_void, loop_times: c_int) -> c_int;
    #[allow(dead_code)]
    fn fluid_player_seek(player: *mut c_void, ticks: c_int) -> c_int;
    #[allow(dead_code)]
    fn fluid_player_get_current_tick(player: *mut c_void) -> c_int;
    #[allow(dead_code)]
    fn fluid_player_get_total_ticks(player: *mut c_void) -> c_int;
    #[allow(dead_code)]
    fn fluid_player_get_division(player: *mut c_void) -> c_int;
    fn fluid_midi_event_get_type(event: *const fluid_midi_event_t) -> c_int;
    fn fluid_midi_event_get_channel(event: *const fluid_midi_event_t) -> c_int;
    fn fluid_synth_handle_midi_event(
        data: *mut c_void,
        event: *mut fluid_midi_event_t,
    ) -> c_int;
}

// ---------------------------------------------------------------------------
// SoundFont 搜索
// ---------------------------------------------------------------------------

const SF2_CANDIDATES: &[&str] = &[
    // Arch 包随程序安装的电子合成器音源
    "/usr/share/music_rust/soundfonts/electronic_synth.sf2",
    // 项目根目录中的可选电子合成器音源（从项目目录启动时自动发现）
    "electronic_synth.sf2",
    "/usr/share/soundfonts/FluidR3_GM.sf2",
    "/usr/share/soundfonts/FluidR3_GS.sf2",
    "/usr/share/soundfonts/FluidR3_GM.sf3",
    "/usr/share/soundfonts/gm.sf2",
    "/usr/share/sounds/sf2/FluidR3_GM.sf2",
    "/usr/share/sounds/sf2/fluid-soundfont-gm.sf2",
    "/usr/share/sounds/sf2/FluidR3_GM.sf3",
    "/usr/share/mscore-4.7/sound/MS Basic.sf3",
    "/usr/share/mscore/sound/MS Basic.sf3",
    "/usr/share/soundfonts/GeneralUser GS v1.471.sf2",
    "/usr/share/soundfonts/generaluser.gs.sf2",
    "/usr/share/soundfonts/FreeFont.sf2",
    "/usr/share/soundfonts/8bitsf.sf2",
    "/usr/share/soundfonts/Emu-OS.sf2",
    "/usr/share/soundfonts/SGM-V2.01-Sal-Guit-Bass-V1.5.sf2",
];

fn user_sf_dirs() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        out.push(Path::new(&home).join(".local/share/soundfonts"));
        out.push(Path::new(&home).join(".local/share/sf2"));
        out.push(Path::new(&home).join("soundfonts"));
    }
    out
}

/// 自动探测最优音频驱动。
///
/// 优先级：
///   1. PipeWire（现代 Linux 桌面，音频/视频统一框架）
///   2. PulseAudio（经典桌面，或 PipeWire 的 pulse 兼容层）
///   3. ALSA（无桌面环境的回退）
///
/// 通过检查运行时 socket 是否存在来判断服务是否可用。
/// 返回驱动的字符串名；探测不到时返回 None（让 fluidsynth 自己回退）。
fn detect_audio_driver() -> Option<String> {
    let uid = std::process::id() as u64;
    let run_dir = format!("/run/user/{}", uid);

    // 注意：用实际 XDG_RUNTIME_DIR 更可靠
    let run_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or(run_dir);

    // 1. PipeWire socket
    let pw_socket = Path::new(&run_dir).join("pipewire-0");
    if pw_socket.exists() {
        return Some("pipewire".to_string());
    }

    // 2. PulseAudio socket
    let pa_socket = Path::new(&run_dir).join("pulse/native");
    if pa_socket.exists() {
        return Some("pulseaudio".to_string());
    }

    // 3. 回退：ALSA
    Some("alsa".to_string())
}

/// 定位可用的 SoundFont 文件。依次尝试：指定路径 → 系统候选 → 用户目录。
pub fn find_soundfont(explicit: Option<&str>) -> Option<String> {
    if let Some(p) = explicit {
        let path = Path::new(p);
        if path.is_file() {
            info(format!("使用指定的 SoundFont: {}", path.display()));
            return Some(p.to_string());
        }
        warn(format!("指定的 SoundFont 不存在: {}", path.display()));
    }

    for c in SF2_CANDIDATES {
        if Path::new(c).is_file() {
            info(format!("发现系统 SoundFont: {}", c));
            return Some(c.to_string());
        }
    }

    for dir in user_sf_dirs() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                if name.ends_with(".sf2") || name.ends_with(".sf3") {
                    info(format!("发现用户 SoundFont: {}", p.display()));
                    return Some(p.to_string_lossy().into_owned());
                }
            }
        }
    }

    None
}

/// 返回当前系统中可供选择的 SoundFont 文件。
///
/// 顺序与自动加载保持一致：随包/项目自带的电子合成器音源优先，
/// 然后是系统音源和用户音源。该函数只负责枚举，不输出日志，方便
/// 启动选择界面时安全地扫描而不污染终端画面。
pub fn available_soundfonts() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    // 候选表里的 `electronic_synth.sf2` 与当前目录扫描得到的
    // `./electronic_synth.sf2` 可能指向同一个文件；按规范化路径去重，
    // 避免启动选择器显示两个相同音源。
    let same_file = |a: &str, b: &str| {
        a == b
            || match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
                (Ok(a), Ok(b)) => a == b,
                _ => false,
            }
    };
    let mut add = |path: String| {
        if !out.iter().any(|p| same_file(p, &path)) {
            out.push(path);
        }
    };

    for candidate in SF2_CANDIDATES {
        let path = Path::new(candidate);
        if path.is_file() {
            add(candidate.to_string());
        }
    }

    // 当前目录是最常见的“临时音源”放置位置；自动加载候选中只列了
    // 默认电子合成器，因此这里补充同目录下的其它 .sf2/.sf3 文件。
    if let Ok(entries) = std::fs::read_dir(".") {
        let mut paths: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| is_soundfont_path(p))
            .collect();
        paths.sort();
        for path in paths {
            add(path.to_string_lossy().into_owned());
        }
    }

    for dir in user_sf_dirs() {
        if let Ok(entries) = std::fs::read_dir(dir) {
            let mut paths: Vec<_> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| is_soundfont_path(p))
                .collect();
            paths.sort();
            for path in paths {
                add(path.to_string_lossy().into_owned());
            }
        }
    }

    out
}

/// 判断路径是否看起来像 SoundFont 文件。
fn is_soundfont_path(path: &Path) -> bool {
    path.is_file()
        && matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .as_deref(),
            Some("sf2" | "sf3")
        )
}

// ---------------------------------------------------------------------------
// SynthPlayer
// ---------------------------------------------------------------------------

/// 启动选择器最多允许同时载入的音源数量。
pub const MAX_SOUNDFONTS: usize = 3;
/// FluidSynth 的音源会把大量采样映射到内存；任意两个音源的文件大小
/// 合计不得超过此值，避免同时载入两个大型 SF2 时触发段错误。
pub const MAX_SOUNDFONT_PAIR_BYTES: u64 = 120 * 1_000_000;
/// 无参数启动器允许的音色切换事件数量（12 组，每组最多两个事件）。
pub const MAX_PROGRAM_SWITCHES: usize = 24;

/// 在 MIDI 或简谱播放时间线上切换当前通道所使用的音源与 GM 音色。
/// `at_ms` 是相对乐曲开始的位置；`soundfont` 是启动器中音源列表的
/// 0-based 下标，`instrument` 为 GM Program (0--127)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgramSwitch {
    pub at_ms: u32,
    pub soundfont: usize,
    pub instrument: u8,
}

const MIDI_PROGRAM_CHANGE: c_int = 0xc0;

/// FluidSynth player 线程中的精确 MIDI 音色切换状态。
///
/// playback callback 会在转发本 tick 的第一个 MIDI 事件前推进用户切换，
/// tick callback 则覆盖没有 MIDI 事件的 tick；两者配合避免主/UI 线程
/// 50ms 轮询造成的延迟，并在用户切换后覆盖文件自身的 program-change。
struct MidiSwitchRuntime {
    synth: *mut fluid_synth_t,
    player: *mut fluid_player_t,
    sfont_ids: Vec<c_int>,
    switches: Vec<(c_int, ProgramSwitch)>,
    next_index: usize,
    active: Option<ProgramSwitch>,
    last_tick: c_int,
    error: Option<String>,
}

impl MidiSwitchRuntime {
    fn select(&mut self, switch_: ProgramSwitch) -> c_int {
        let Some(&sfont_id) = self.sfont_ids.get(switch_.soundfont) else {
            self.error = Some(format!(
                "MIDI 切换引用了不存在的 SoundFont #{}",
                switch_.soundfont + 1
            ));
            return FLUID_FAILED;
        };
        for channel in 0..16 {
            let result = unsafe {
                fluid_synth_program_select(
                    self.synth,
                    channel,
                    sfont_id,
                    0,
                    switch_.instrument as c_int,
                )
            };
            if result != FLUID_OK {
                self.error = Some(format!(
                    "MIDI 精确切换失败：SoundFont #{} / GM Program {}",
                    switch_.soundfont + 1,
                    switch_.instrument
                ));
                return FLUID_FAILED;
            }
        }
        self.active = Some(switch_);
        FLUID_OK
    }

    fn select_channel(&mut self, channel: c_int) -> c_int {
        let Some(switch_) = self.active else {
            return FLUID_OK;
        };
        let Some(&sfont_id) = self.sfont_ids.get(switch_.soundfont) else {
            return FLUID_FAILED;
        };
        let result = unsafe {
            fluid_synth_program_select(
                self.synth,
                channel,
                sfont_id,
                0,
                switch_.instrument as c_int,
            )
        };
        if result != FLUID_OK {
            self.error = Some(format!(
                "MIDI program-change 覆盖失败：SoundFont #{} / GM Program {}",
                switch_.soundfont + 1,
                switch_.instrument
            ));
        }
        result
    }

    /// 在处理某个 MIDI tick 的任何原生事件之前，把用户切换时间线推进到
    /// 该 tick。FluidSynth 2.6 会先调用 playback callback，处理完本 tick
    /// 的事件后才调用 tick callback，因此两个回调都必须使用同一推进逻辑。
    fn advance_to_tick(&mut self, tick: c_int) -> c_int {
        // seek/loop 回退时按目标 tick 重建计划位置。active 必须先清空，
        // 这样 seek 为目标位置重放的原生 program-change 不会继续被旧的
        // 用户切换覆盖；随后若目标已越过切换点，再恢复最后一条用户规则。
        if tick < self.last_tick {
            // 必须在本 tick 的第一个 playback 事件之前清掉上一位置的尾音。
            // 由 UI 线程事后观察 tick 回退再静音会误杀目标点刚发出的首音。
            for channel in 0..16 {
                unsafe {
                    fluid_synth_all_sounds_off(self.synth, channel);
                    fluid_synth_all_notes_off(self.synth, channel);
                    fluid_synth_cc(self.synth, channel, 64, 0);
                }
            }
            self.next_index = self
                .switches
                .partition_point(|(switch_tick, _)| *switch_tick <= tick);
            self.active = None;
            if self.next_index > 0 {
                let switch_ = self.switches[self.next_index - 1].1;
                if self.select(switch_) != FLUID_OK {
                    return FLUID_FAILED;
                }
            }
        } else {
            while let Some((switch_tick, switch_)) =
                self.switches.get(self.next_index).copied()
            {
                if switch_tick > tick {
                    break;
                }
                if self.select(switch_) != FLUID_OK {
                    return FLUID_FAILED;
                }
                self.next_index += 1;
            }
        }
        self.last_tick = tick;
        FLUID_OK
    }
}

unsafe extern "C" fn midi_switch_tick_callback(data: *mut c_void, tick: c_int) -> c_int {
    if data.is_null() {
        return FLUID_FAILED;
    }
    let state = &mut *(data as *mut MidiSwitchRuntime);
    state.advance_to_tick(tick)
}

unsafe extern "C" fn midi_switch_playback_callback(
    data: *mut c_void,
    event: *mut fluid_midi_event_t,
) -> c_int {
    if data.is_null() || event.is_null() {
        return FLUID_FAILED;
    }
    let state = &mut *(data as *mut MidiSwitchRuntime);
    // FluidSynth 2.6 的 playback callback 早于同 tick 的 tick callback。
    // 先用 player 当前 tick 推进计划，保证切换点上的首个 note-on 已经使用
    // 新音色；seek 回退时也会在原生 program-change 被处理前清掉旧 active。
    let tick = fluid_player_get_current_tick(state.player).max(0);
    if state.advance_to_tick(tick) != FLUID_OK {
        return FLUID_FAILED;
    }
    let result = fluid_synth_handle_midi_event(state.synth, event);
    if result != FLUID_OK {
        return result;
    }
    if state.active.is_some() && fluid_midi_event_get_type(event) == MIDI_PROGRAM_CHANGE {
        return state.select_channel(fluid_midi_event_get_channel(event));
    }
    FLUID_OK
}

/// 验证切换列表。切换时间允许相同；同一秒的多条切换按列表顺序执行，
/// 因而最后一条成为该时间点之后的有效音色。
pub fn validate_program_switches(
    switches: &[ProgramSwitch],
    soundfont_count: usize,
) -> Result<(), String> {
    if switches.len() > MAX_PROGRAM_SWITCHES {
        return Err(format!(
            "最多设置 {} 条中途音色切换",
            MAX_PROGRAM_SWITCHES
        ));
    }
    if soundfont_count == 0 {
        return Err("至少需要一个 SoundFont".to_string());
    }
    let mut previous = 0u32;
    for (index, switch) in switches.iter().enumerate() {
        if switch.soundfont >= soundfont_count {
            return Err(format!(
                "第 {} 条切换引用了不存在的 SoundFont #{}",
                index + 1,
                switch.soundfont + 1
            ));
        }
        if index > 0 && switch.at_ms < previous {
            return Err("中途音色切换必须按时间从早到晚排列".to_string());
        }
        previous = switch.at_ms;
    }
    Ok(())
}

/// 计算某个播放位置应当生效的音色。`switches` 已由
/// `validate_program_switches` 保证按时间排列；等时的多条记录保留最后一条。
fn selection_at(
    initial_soundfont: usize,
    initial_instrument: u8,
    switches: &[ProgramSwitch],
    at_ms: u32,
) -> ProgramSwitch {
    let mut selected = ProgramSwitch {
        at_ms: 0,
        soundfont: initial_soundfont,
        instrument: initial_instrument,
    };
    for switch_ in switches {
        if switch_.at_ms > at_ms {
            break;
        }
        selected = *switch_;
    }
    selected
}

/// 校验音源列表的数量、存在性，以及任意两个音源的合计大小。该检查在
/// 启动器和实际加载层都执行，防止其它调用路径绕过 UI 后仍把危险的
/// 大型音源组合交给 C 库。
pub fn validate_soundfont_paths(paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("至少需要一个 SoundFont".to_string());
    }
    if paths.len() > MAX_SOUNDFONTS {
        return Err(format!("最多同时加载 {} 个 SoundFont", MAX_SOUNDFONTS));
    }
    let mut seen = BTreeSet::new();
    let mut sizes = Vec::with_capacity(paths.len());
    for path in paths {
        let metadata = std::fs::metadata(path)
            .map_err(|e| format!("无法读取 SoundFont {}: {}", path, e))?;
        if !metadata.is_file() {
            return Err(format!("SoundFont 不是普通文件: {}", path));
        }
        if !is_soundfont_path(Path::new(path)) {
            return Err(format!("不是支持的 SoundFont 文件 (.sf2/.sf3): {}", path));
        }
        // 同一个音源载入两次既没有实际用途，又会占用两份采样内存；以规范化
        // 路径（不可规范化时退回原始路径）去重，保证成对大小限制有意义。
        let identity = std::fs::canonicalize(path)
            .unwrap_or_else(|_| Path::new(path).to_path_buf());
        if !seen.insert(identity) {
            return Err(format!("SoundFont 重复选择: {}", path));
        }
        if metadata.len() > MAX_SOUNDFONT_PAIR_BYTES {
            return Err(format!(
                "SoundFont {} 大小 {:.1} MB，单个文件不能超过 120 MB",
                path,
                metadata.len() as f64 / 1_000_000.0
            ));
        }
        sizes.push((path, metadata.len()));
    }
    for first in 0..sizes.len() {
        for second in first + 1..sizes.len() {
            let pair_bytes = sizes[first].1.saturating_add(sizes[second].1);
            if pair_bytes > MAX_SOUNDFONT_PAIR_BYTES {
                return Err(format!(
                    "SoundFont {} 与 {} 合计 {:.1} MB，任意两个不能超过 120 MB",
                    sizes[first].0,
                    sizes[second].0,
                    pair_bytes as f64 / 1_000_000.0
                ));
            }
        }
    }
    Ok(())
}

/// 取得选择器展示用的已选音源总大小。路径尚未完整/可读时返回错误，
/// 与真正加载前的校验保持同一套错误语义。
pub fn soundfont_total_bytes(paths: &[String]) -> Result<u64, String> {
    validate_soundfont_paths(paths)?;
    paths.iter().try_fold(0u64, |total, path| {
        let bytes = std::fs::metadata(path)
            .map_err(|e| format!("无法读取 SoundFont {}: {}", path, e))?
            .len();
        Ok(total.saturating_add(bytes))
    })
}

pub struct SynthPlayer {
    settings: *mut fluid_settings_t,
    synth: *mut fluid_synth_t,
    audio_driver: *mut c_void,
    sequencer: *mut fluid_sequencer_t,
    synth_client: fluid_seq_id_t,
    #[allow(dead_code)]
    sfont_id: c_int,
    /// 按启动器顺序保存的 FluidSynth 音源 ID。
    sfont_ids: Vec<c_int>,
    /// 当前加载的所有音源路径（第一个保持与旧版 `soundfont` 一致）。
    pub soundfonts: Vec<String>,
    #[allow(dead_code)]
    pub soundfont: String,
    #[allow(dead_code)]
    tempo_ms: u32,
    /// 当前合成器增益（音量），默认 1.0
    gain: f32,
    freed: bool,
}

/// 初始化阶段（stderr 静默期内）的临时结果，完成后字段搬运到 SynthPlayer。
struct SynthPlayerInner {
    settings: *mut fluid_settings_t,
    synth: *mut fluid_synth_t,
    audio_driver: *mut c_void,
    sequencer: *mut fluid_sequencer_t,
    synth_client: fluid_seq_id_t,
    #[allow(dead_code)]
    sfont_id: c_int,
    sfont_ids: Vec<c_int>,
    soundfonts: Vec<String>,
    #[allow(dead_code)]
    soundfont: String,
}

impl SynthPlayer {
    /// 初始化单个 SoundFont（保留命令行/API 的兼容入口）。
    pub fn new(soundfont_path: Option<&str>, tempo_ms: u32, verbose: bool, limit_db: f32) -> Result<Self, String> {
        let soundfont = find_soundfont(soundfont_path)
            .ok_or_else(|| "未找到任何 SoundFont (.sf2/.sf3)，请使用 --soundfont 指定路径".to_string())?;
        Self::new_with_soundfonts(&[soundfont], tempo_ms, verbose, limit_db)
    }

    /// 初始化多个 SoundFont。路径顺序决定启动器中的音源编号（1、2、3），
    /// 并在进入 FluidSynth 前严格执行数量与任意两个合计 120 MB 的限制。
    pub fn new_with_soundfonts(
        soundfont_paths: &[String],
        tempo_ms: u32,
        verbose: bool,
        limit_db: f32,
    ) -> Result<Self, String> {
        validate_soundfont_paths(soundfont_paths)?;
        Self::new_from_paths(soundfont_paths, tempo_ms, verbose, limit_db)
    }

    fn new_from_paths(soundfont_paths: &[String], tempo_ms: u32, verbose: bool, limit_db: f32) -> Result<Self, String> {
        info("正在初始化 fluidsynth ...".to_string());

        // fluidsynth 初始化期间，C 库会对 ALSA/SDL 等音频后端做探测并直接写 stderr，
        // 产生大量与本机环境相关的噪音（Unknown PCM、unable to open slave、SDL3 未初始化等）。
        // 这些消息与播放成败无关，在初始化阶段静默它们。
        let init = silence_stderr(|| -> Result<SynthPlayerInner, String> {
            let settings = unsafe { new_fluid_settings() };
            if settings.is_null() {
                return Err("new_fluid_settings 失败".into());
            }

        // 音频驱动：优先探测 PipeWire / PulseAudio（大多数 Linux 桌面环境），
        // 其次回退 ALSA。用户可通过 MUSIC_AUDIO_DRIVER 环境变量强制指定。
        let mut driver_name: Option<String> = None;
        if let Ok(driver) = std::env::var("MUSIC_AUDIO_DRIVER") {
            if !driver.is_empty() {
                info(format!("强制音频驱动: {}", driver));
                driver_name = Some(driver);
            }
        }
        let detected = detect_audio_driver();
        if let Some(d) = &detected {
            info(format!("自动选择音频驱动: {}", d));
        }
        let chosen = driver_name.as_deref().or(detected.as_deref());
        if let Some(driver) = chosen {
            let key = CString::new("audio.driver").unwrap();
            let val = CString::new(driver).unwrap();
            unsafe { fluid_settings_setstr(settings, key.as_ptr(), val.as_ptr()) };
        }

        // 增大音频缓冲区，防止音频线程调度延迟导致的爆音（xrun）。
        // 默认 period-size=64 / periods=16 太小，调度不及时就破音。
        // 这里用较大缓冲区（512×4 ≈ 46ms @44.1kHz），大幅提高调度容忍度。
        let pk = CString::new("audio.period-size").unwrap();
        unsafe { fluid_settings_setint(settings, pk.as_ptr(), 512) };
        let ppk = CString::new("audio.periods").unwrap();
        unsafe { fluid_settings_setint(settings, ppk.as_ptr(), 4) };

        if verbose {
            // 调试模式下让 fluidsynth 把日志打到 stderr
            let key = CString::new("synth.verbose").unwrap();
            unsafe { fluid_settings_setint(settings, key.as_ptr(), 1) };
        }

        // 合成器采样率（默认 44100 已足够，无需改动）

        let synth = unsafe { new_fluid_synth(settings) };
        if synth.is_null() {
            unsafe { delete_fluid_settings(settings) };
            return Err("new_fluid_synth 失败 —— 请检查 fluidsynth 运行时库是否已安装".into());
        }

        // 加载 SoundFont。即使调用方来自命令行而非启动器，也在这里再次
        // 受 validate_soundfont_paths 保护，避免 C 库收到危险的大组合。
        let mut sfont_ids = Vec::with_capacity(soundfont_paths.len());
        for (index, sf) in soundfont_paths.iter().enumerate() {
            let sf_c = CString::new(sf.as_str())
                .map_err(|_| format!("SoundFont 路径包含非法字符: {}", sf))?;
            // 第一个音源沿用 FluidSynth 的预置重置语义；后续音源只加入
            // 音源库，不能把刚明确选中的默认预置再重置一次。
            let sfont_id = unsafe {
                fluid_synth_sfload(synth, sf_c.as_ptr(), if index == 0 { 1 } else { 0 })
            };
            if sfont_id < 0 {
                unsafe {
                    delete_fluid_synth(synth);
                    delete_fluid_settings(settings);
                }
                return Err(format!("加载 SoundFont 失败: {}", sf));
            }
            info(format!("SoundFont 加载成功 (id={}): {}", sfont_id, sf));
            sfont_ids.push(sfont_id);
        }
        let sfont_id = sfont_ids[0];
        let sf = soundfont_paths[0].clone();

        // 设置合成器增益。项目把 100% 定义为 FluidSynth gain=1.0；
        // 必须和下面的 `SynthPlayer.gain` 同时初始化，否则启动瞬间会
        // 显示 80% 但实际仍是 100%。峰值交给限制器兜底，保证不削波。
        unsafe { fluid_synth_set_gain(synth, DEFAULT_VOLUME_SCALE) };

        // 所有通道默认钢琴 (GM Program 0)
        unsafe { fluid_synth_program_reset(synth) };
        for ch in 0..16 {
            unsafe {
                // 明确选中第一个音源，避免多音源时 FluidSynth 按加载顺序
                // 自动挑选到最后一个音源的同编号预置。
                fluid_synth_program_select(synth, ch, sfont_id, 0, 0);
                fluid_synth_cc(synth, ch, 7, 120); // 主音量
                fluid_synth_cc(synth, ch, 10, 64); // 声像居中
            }
        }
        info("已将所有通道设为钢琴 (GM Program 0)".to_string());

        // 创建音频驱动（必须显式创建，否则合成器无输出）
        // 创建音频驱动：使用自定义回调驱动（带峰值限制器，防止削波）
        unsafe {
            LIMITER_SYNTH = synth;
            LIMITER = LimiterState::new(limit_db); // 峰值限制到 limit_db dBFS
        }
        request_limiter_reset();
        let audio_driver = unsafe { new_fluid_audio_driver2(settings, audio_render_callback, std::ptr::null_mut()) };
        if audio_driver.is_null() {
            unsafe {
                LIMITER_SYNTH = std::ptr::null_mut();
                delete_fluid_synth(synth);
                delete_fluid_settings(settings);
            }
            return Err(
                "创建音频驱动失败 —— 请检查音频系统(ALSA/PulseAudio/PipeWire)是否正常".into()
            );
        }
        info(format!("音频驱动创建成功（峰值限制器已启用: {}dBFS）", limit_db));

        // 创建 sequencer，time_scale = 1000 → tick 单位 = 毫秒
        let sequencer = unsafe { new_fluid_sequencer2(0) };
        if sequencer.is_null() {
            unsafe {
                delete_fluid_audio_driver(audio_driver);
                delete_fluid_synth(synth);
                delete_fluid_settings(settings);
            }
            return Err("new_fluid_sequencer2 失败".into());
        }
        unsafe { fluid_sequencer_set_time_scale(sequencer, 1000.0) };
        let synth_client = unsafe { fluid_sequencer_register_fluidsynth(sequencer, synth) };
        info(format!("sequencer 客户端注册成功 (id={})", synth_client));

        Ok(SynthPlayerInner {
            settings,
            synth,
            audio_driver,
            sequencer,
            synth_client,
            sfont_id,
            sfont_ids: sfont_ids.clone(),
            soundfonts: soundfont_paths.to_vec(),
            soundfont: sf,
        })
        });
        let inner = init?;

        Ok(SynthPlayer {
            settings: inner.settings,
            synth: inner.synth,
            audio_driver: inner.audio_driver,
            sequencer: inner.sequencer,
            synth_client: inner.synth_client,
            sfont_id: inner.sfont_id,
            sfont_ids: inner.sfont_ids,
            soundfonts: inner.soundfonts,
            soundfont: inner.soundfont,
            tempo_ms,
            gain: DEFAULT_VOLUME_SCALE,
            freed: false,
        })
    }

    /// 调整音量（合成器增益），步进 ±0.1，范围 0%-500%。
    pub fn adjust_volume(&mut self, delta: f32) -> f32 {
        let mut g = self.gain + delta;
        // 交互步进按百分比工作；先量化到 1% 再送入 FluidSynth，避免
        // 0.8 - 0.1 - 0.1 之类的二进制浮点累积让实际增益落在 0.6
        // 附近、而 UI 显示/音频模式使用的是精确的 60%。
        g = (g * 100.0).round() / 100.0;
        g = g.clamp(0.0, MAX_VOLUME_SCALE);
        self.gain = g;
        unsafe { fluid_synth_set_gain(self.synth, g) };
        request_limiter_reset();
        debug(format!("音量: {:.0}%", g * 100.0));
        g
    }

    /// 当前音量百分比
    #[allow(dead_code)]
    pub fn volume(&self) -> u32 {
        volume_percent_from_scale(self.gain)
    }

    /// 设置绝对音量百分比，范围 0%-500%。
    pub fn set_volume_percent(&mut self, percent: u32) {
        self.gain = volume_scale_from_percent(percent);
        unsafe { fluid_synth_set_gain(self.synth, self.gain) };
        request_limiter_reset();
    }

    /// 调度一个音符事件到绝对时间点（毫秒）。
    /// `on_off`: true=noteon, false=noteoff
    fn schedule_note(
        &self,
        channel: c_int,
        key: u8,
        vel: u8,
        at_ms: u32,
        on: bool,
    ) -> Result<(), String> {
        let evt = unsafe { new_fluid_event() };
        if evt.is_null() {
            return Err("无法创建 MIDI 音符事件（内存不足）".to_string());
        }
        unsafe {
            fluid_event_set_source(evt, 0);
            fluid_event_set_dest(evt, self.synth_client);
            if on {
                fluid_event_noteon(evt, channel, key as c_short, vel as c_short);
            } else {
                fluid_event_noteoff(evt, channel, key as c_short);
            }
        }
        // 注意：fluid_sequencer_send_at(seq, evt, time, absolute)
        //   time=绝对时间(ms)，absolute=1 表示绝对时间
        //   dest 已在 fluid_event_set_dest 中设置
        let result = unsafe { fluid_sequencer_send_at(self.sequencer, evt, at_ms, 1) };
        unsafe { delete_fluid_event(evt) };
        if result != FLUID_OK {
            return Err(format!(
                "排程 MIDI 音符失败：ch{} key{} @{}ms",
                channel, key, at_ms
            ));
        }
        Ok(())
    }

    /// 调度一个相对事件：`at_ms` 是相对"当前 tick"的偏移（absolute=0）。
    /// 用于交互模式下动态重排（快进/后退/循环）。
    fn schedule_note_relative(
        &self,
        channel: c_int,
        key: u8,
        vel: u8,
        at_ms: u32,
        on: bool,
    ) -> Result<(), String> {
        let evt = unsafe { new_fluid_event() };
        if evt.is_null() {
            return Err("无法创建相对 MIDI 音符事件（内存不足）".to_string());
        }
        unsafe {
            fluid_event_set_source(evt, 0);
            fluid_event_set_dest(evt, self.synth_client);
            if on {
                fluid_event_noteon(evt, channel, key as c_short, vel as c_short);
            } else {
                fluid_event_noteoff(evt, channel, key as c_short);
            }
        }
        let result = unsafe { fluid_sequencer_send_at(self.sequencer, evt, at_ms, 0) };
        unsafe { delete_fluid_event(evt) };
        if result != FLUID_OK {
            return Err("排程相对 MIDI 音符失败".to_string());
        }
        Ok(())
    }

    /// 清除 sequencer 中所有已排程事件
    pub fn clear_schedule(&self) {
        unsafe {
            fluid_sequencer_remove_events(self.sequencer, -1, -1, -1);
        }
    }

    /// 立即清除所有正在发声的合成器音符。
    ///
    /// 清理 sequencer 只会移除尚未执行的事件，已经送入 synth 的
    /// note-on（尤其是带 sustain 的音符）仍可能继续发声。因此暂停、
    /// 跳转和退出时都要同时清理事件队列与 synth 当前声音。
    pub fn silence(&self) {
        // 跳转/暂停后下一段音频不应继承旧播放头的 limiter 包络。
        request_limiter_reset();
        unsafe {
            for ch in 0..16 {
                // all_sounds_off 是硬静音，保证暂停/拖动后不会留下释放尾音；
                // all_notes_off 和 sustain 复位则清理 MIDI 通道状态。
                fluid_synth_all_sounds_off(self.synth, ch);
                fluid_synth_all_notes_off(self.synth, ch);
                fluid_synth_cc(self.synth, ch, 64, 0);  // sustain pedal off
                fluid_synth_cc(self.synth, ch, 123, 0); // all notes off (CC)
            }
        }
    }

    fn soundfont_id(&self, soundfont: usize) -> Result<c_int, String> {
        self.sfont_ids.get(soundfont).copied().ok_or_else(|| {
            format!(
                "SoundFont #{} 不存在（当前已载入 {} 个）",
                soundfont + 1,
                self.sfont_ids.len()
            )
        })
    }

    /// 在开始播放 TXT 之前同步检查它要求的全部音色。
    ///
    /// 定时切换最终由 FluidSynth sequencer 异步执行，单纯把事件排进去无法
    /// 得知某个 SF2 是否真的含有对应预置。这里先在空闲通道 15 上逐一执行
    /// program-select；任意一项失败就返回硬错误，调用方必须退出，避免播放到
    /// 一半才静默回退或无声。检查结束后恢复初始音色。
    pub fn validate_program_requirements(
        &self,
        initial_soundfont: usize,
        initial_instrument: u8,
        program_switches: &[ProgramSwitch],
    ) -> Result<(), String> {
        validate_program_switches(program_switches, self.sfont_ids.len())?;

        let mut seen = BTreeSet::new();
        let mut requirements = Vec::with_capacity(program_switches.len() + 1);
        requirements.push((
            initial_soundfont,
            initial_instrument,
            "初始音色".to_string(),
        ));
        for (index, switch_) in program_switches.iter().enumerate() {
            requirements.push((
                switch_.soundfont,
                switch_.instrument,
                format!(
                    "第 {} 条切换（{:.3} 秒）",
                    index + 1,
                    switch_.at_ms as f64 / 1000.0
                ),
            ));
        }

        for (soundfont, instrument, label) in requirements {
            if !seen.insert((soundfont, instrument)) {
                continue;
            }
            let id = self.soundfont_id(soundfont)?;
            let available = unsafe {
                fluid_synth_program_select(self.synth, 15, id, 0, instrument as c_int)
            };
            if available != FLUID_OK {
                let path = self
                    .soundfonts
                    .get(soundfont)
                    .map(String::as_str)
                    .unwrap_or("未知音源");
                return Err(format!(
                    "{}要求的音色不存在：SoundFont #{} ({}) 没有 GM bank 0 / Program {}",
                    label,
                    soundfont + 1,
                    path,
                    instrument
                ));
            }
        }

        // 通道 15 可能也是乐曲通道；结束预检时恢复初始选择，之后 main 还会
        // 对所有实际使用的通道设置同一个初始音色。
        self.select_soundfont_instrument(15, initial_soundfont, initial_instrument)?;
        Ok(())
    }

    /// 在创建 FluidSynth MIDI player 之前同步确认可选的用户初始音色和
    /// 所有定时切换预置存在，并在检查后恢复探测通道的原状态。
    fn validate_midi_switch_presets(
        &self,
        initial_selection: Option<(usize, u8)>,
        program_switches: &[ProgramSwitch],
    ) -> Result<(), String> {
        validate_program_switches(program_switches, self.sfont_ids.len())?;

        let mut saved_sfont = 0;
        let mut saved_bank = 0;
        let mut saved_program = 0;
        let saved = unsafe {
            fluid_synth_get_program(
                self.synth,
                15,
                &mut saved_sfont,
                &mut saved_bank,
                &mut saved_program,
            ) == FLUID_OK
        };

        let mut requirements = Vec::with_capacity(program_switches.len() + 1);
        if let Some((soundfont, instrument)) = initial_selection {
            requirements.push((soundfont, instrument, "MIDI 初始音色".to_string()));
        }
        for (index, switch_) in program_switches.iter().enumerate() {
            requirements.push((
                switch_.soundfont,
                switch_.instrument,
                format!("第 {} 条 MIDI 切换（{:.3} 秒）", index + 1, switch_.at_ms as f64 / 1000.0),
            ));
        }

        let mut seen = BTreeSet::new();
        for (soundfont, instrument, label) in requirements {
            if !seen.insert((soundfont, instrument)) {
                continue;
            }
            let id = self.soundfont_id(soundfont)?;
            let available = unsafe {
                fluid_synth_program_select(
                    self.synth,
                    15,
                    id,
                    0,
                    instrument as c_int,
                )
            };
            if available != FLUID_OK {
                let path = self
                    .soundfonts
                    .get(soundfont)
                    .map(String::as_str)
                    .unwrap_or("未知音源");
                return Err(format!(
                    "{}要求的音色不存在：SoundFont #{} ({}) 没有 GM bank 0 / Program {}",
                    label,
                    soundfont + 1,
                    path,
                    instrument
                ));
            }
        }

        if saved {
            let restored = unsafe {
                fluid_synth_program_select(
                    self.synth,
                    15,
                    saved_sfont,
                    saved_bank,
                    saved_program,
                )
            };
            if restored != FLUID_OK {
                return Err("MIDI 音色预检后无法恢复通道状态".to_string());
            }
        } else if unsafe { fluid_synth_unset_program(self.synth, 15) } != FLUID_OK {
            return Err("MIDI 音色预检后无法清除探测通道状态".to_string());
        }
        Ok(())
    }

    /// 立即选择指定通道的 SoundFont 与 GM Program。用于开始播放、跳转后
    /// 恢复，以及 MIDI 播放器的强制覆盖；它不会中断已经发声的音符。
    pub fn select_soundfont_instrument(
        &self,
        channel: c_int,
        soundfont: usize,
        program: u8,
    ) -> Result<(), String> {
        let id = self.soundfont_id(soundfont)?;
        let ret = unsafe { fluid_synth_program_select(self.synth, channel, id, 0, program as c_int) };
        if ret != FLUID_OK {
            return Err(format!(
                "无法将通道 {} 切换到 SoundFont #{} 的 GM#{}",
                channel,
                soundfont + 1,
                program
            ));
        }
        debug(format!(
            "通道 {} 设置 SoundFont #{} / GM#{}",
            channel,
            soundfont + 1,
            program
        ));
        Ok(())
    }

    /// 对一组实际使用的通道应用音源选择。重复通道自动合并，且始终映射
    /// 到 MIDI 的 0--15 范围，避免多轨简谱的逻辑通道编号越界。
    fn select_for_channels(
        &self,
        channels: &[u8],
        soundfont: usize,
        program: u8,
    ) -> Result<(), String> {
        let mut unique = BTreeSet::new();
        for channel in channels {
            unique.insert(*channel % 16);
        }
        for channel in unique {
            self.select_soundfont_instrument(channel as c_int, soundfont, program)?;
        }
        Ok(())
    }

    /// 对 MIDI 的所有通道应用音源选择。MIDI 文件自身可能在播放中发送
    /// program-change；`play_midi` 会定期调用此方法以让启动器配置保持有效。
    fn select_for_all_channels(&self, soundfont: usize, program: u8) -> Result<(), String> {
        for channel in 0..16 {
            self.select_soundfont_instrument(channel, soundfont, program)?;
        }
        Ok(())
    }

    /// 将一个 SoundFont / Program 切换排入简谱的 FluidSynth sequencer。
    /// 所有与该简谱相关的通道会在同一毫秒切换；后续 note-on 才使用新音色，
    /// 已在发声的音符保持原来的音源，自然收尾。
    fn schedule_soundfont_switch(
        &self,
        channels: &[u8],
        switch: ProgramSwitch,
        at_ms: u32,
    ) -> Result<(), String> {
        let id = self.soundfont_id(switch.soundfont)?;
        let mut unique = BTreeSet::new();
        for channel in channels {
            unique.insert(*channel % 16);
        }
        for channel in unique {
            let evt = unsafe { new_fluid_event() };
            if evt.is_null() {
                return Err("无法创建音色切换事件".to_string());
            }
            let result = unsafe {
                fluid_event_set_source(evt, 0);
                fluid_event_set_dest(evt, self.synth_client);
                fluid_event_program_select(evt, channel as c_int, id as c_uint, 0, switch.instrument as c_short);
                let result = fluid_sequencer_send_at(self.sequencer, evt, at_ms, 1);
                delete_fluid_event(evt);
                result
            };
            if result != FLUID_OK {
                return Err(format!(
                    "排程音色切换失败：SoundFont #{} / GM#{} @{}ms",
                    switch.soundfont + 1,
                    switch.instrument,
                    at_ms
                ));
            }
        }
        Ok(())
    }

    /// 设置指定通道的默认乐器（GM Program）。兼容旧调用方，始终使用
    /// 第一个已载入的 SoundFont。
    pub fn set_instrument(&self, channel: c_int, program: u8) {
        if let Err(error) = self.select_soundfont_instrument(channel, 0, program) {
            warn(error);
        }
    }

    /// 设置指定通道的乐器（GM Program）及其 SoundFont 编号。
    pub fn set_soundfont_instrument(&self, channel: c_int, soundfont: usize, program: u8) -> Result<(), String> {
        self.select_soundfont_instrument(channel, soundfont, program)
    }

    /// 保留给旧版直接事件调度的 Program Change 接口。
    #[allow(dead_code)]
    fn schedule_legacy_program_change(&self, channel: c_int, program: u8) {
        let evt = unsafe { new_fluid_event() };
        unsafe {
            fluid_event_set_source(evt, 0);
            fluid_event_set_dest(evt, self.synth_client);
            fluid_event_program_change(evt, channel, program as c_int);
        }
        unsafe { fluid_sequencer_send_now(self.sequencer, evt) };
        unsafe { delete_fluid_event(evt) };
    }

    /// 返回当前 sequencer 时间线毫秒
    pub fn now_ms(&self) -> u32 {
        unsafe { fluid_sequencer_get_tick(self.sequencer) }
    }

    #[allow(dead_code)]
    pub fn tempo(&self) -> u32 {
        self.tempo_ms
    }

    #[allow(dead_code)]
    pub fn set_tempo(&mut self, ms: u32) {
        self.tempo_ms = ms;
        debug(format!("速度更新为 {}ms/四分音符", ms));
    }

    /// 直接播放 MIDI 文件（使用 fluidsynth 内置 MIDI 播放器）。
    /// 自动处理多轨同步与 tempo 变化，等待播放完成。
    /// `bpm_override`: Some(bpm) 时强制覆盖速度。
    /// `show_progress`: 显示动态进度条。
    /// `interactive`: 启用键盘交互控制（快进/后退/暂停/循环/退出）。
    /// `total_ms`: 估算的总时长（毫秒），用于进度条；0 时仅显示经过时间。
    /// 参数虽然多，但都是 MIDI 播放这一件事的完整配置，保持平铺方便调用方逐项传参。
    #[allow(clippy::too_many_arguments)]
    pub fn play_midi(
        &mut self,
        midi_path: &str,
        bpm_override: Option<f64>,
        show_progress: bool,
        interactive: bool,
        total_ms: u32,
        initial_selection: Option<(usize, u8)>,
        program_switches: &[ProgramSwitch],
    ) -> Result<(), String> {
        if !program_switches.is_empty() && midi_uses_smpte_division(midi_path) {
            return Err(
                "SMPTE time-division MIDI 暂不支持按秒音色切换；请先转换为 PPQ MIDI"
                    .to_string(),
            );
        }
        // 同步预检必须发生在 player/后台线程创建前；稀疏 SF2 缺少目标
        // Program 时在这里直接退出，不让异步播放进入半初始化状态。
        self.validate_midi_switch_presets(initial_selection, program_switches)?;
        let path_c = CString::new(midi_path)
            .map_err(|_| "MIDI 路径包含非法字符".to_string())?;
        let display = midi_display_events(midi_path);

        let player = unsafe { new_fluid_player(self.synth) };
        if player.is_null() {
            return Err("创建 MIDI 播放器失败".into());
        }

        let add_ret = unsafe { fluid_player_add(player, path_c.as_ptr()) };
        if add_ret != FLUID_OK {
            unsafe { delete_fluid_player(player) };
            return Err(format!("无法加载 MIDI 文件: {}", midi_path));
        }

        if let Some(bpm) = bpm_override {
            if bpm > 0.0 {
                unsafe { fluid_player_set_tempo(player, FLUID_PLAYER_TEMPO_EXTERNAL_BPM, bpm) };
                info(format!("覆盖速度为 {} BPM", bpm));
            }
        }

        // 精确切换由 FluidSynth 的 playback + tick callback 共同驱动；UI
        // 轮询只负责显示。playback callback 先于同 tick 事件转发时推进计划，
        // tick callback 则覆盖静默 tick，避免 50ms 刷新周期造成延迟。
        let fixed_quarter_ms = bpm_override
            .filter(|bpm| *bpm > 0.0)
            .map(|bpm| 60_000.0 / bpm)
            .unwrap_or(500.0);
        let midi_time_map = if bpm_override.filter(|bpm| *bpm > 0.0).is_some() {
            None
        } else {
            match midi_time_map(midi_path) {
                Some(map) => Some(map),
                None if !program_switches.is_empty() => {
                    unsafe { delete_fluid_player(player) };
                    return Err(
                        "无法读取 MIDI tempo map，不能保证按秒音色切换准确"
                            .to_string(),
                    );
                }
                None => None,
            }
        };
        let player_division = unsafe { fluid_player_get_division(player) }.max(1) as f64;
        let switch_state = if initial_selection.is_none() && program_switches.is_empty() {
            std::ptr::null_mut()
        } else {
            let mut switches = Vec::with_capacity(
                program_switches.len() + usize::from(initial_selection.is_some()),
            );
            if let Some((soundfont, instrument)) = initial_selection {
                // 初始选择不计入用户的 24 条上限，并排在同为 0 秒的用户
                // 切换之前，保证用户显式的 0 秒规则最终生效。
                switches.push((
                    0,
                    ProgramSwitch {
                        at_ms: 0,
                        soundfont,
                        instrument,
                    },
                ));
            }
            switches.extend(program_switches
                .iter()
                .map(|switch_| {
                    let tick = midi_time_map
                        .as_ref()
                        .map(|map| map.ms_to_tick(switch_.at_ms))
                        .unwrap_or_else(|| {
                            (f64::from(switch_.at_ms) * player_division / fixed_quarter_ms)
                                .ceil()
                                .max(0.0) as u64
                        })
                        .min(c_int::MAX as u64) as c_int;
                    (tick, *switch_)
                }));
            Box::into_raw(Box::new(MidiSwitchRuntime {
                synth: self.synth,
                player,
                sfont_ids: self.sfont_ids.clone(),
                switches,
                next_index: 0,
                active: None,
                last_tick: -1,
                error: None,
            }))
        };
        if !switch_state.is_null() {
            let tick_result = unsafe {
                fluid_player_set_tick_callback(
                    player,
                    midi_switch_tick_callback,
                    switch_state.cast(),
                )
            };
            let playback_result = unsafe {
                fluid_player_set_playback_callback(
                    player,
                    midi_switch_playback_callback,
                    switch_state.cast(),
                )
            };
            if tick_result != FLUID_OK || playback_result != FLUID_OK {
                unsafe {
                    delete_fluid_player(player);
                    drop(Box::from_raw(switch_state));
                }
                return Err("无法注册 MIDI 精确音色切换回调".to_string());
            }
        }

        let play_ret = unsafe { fluid_player_play(player) };
        if play_ret != FLUID_OK {
            unsafe {
                delete_fluid_player(player);
                if !switch_state.is_null() {
                    drop(Box::from_raw(switch_state));
                }
            }
            return Err("MIDI 播放器启动失败".into());
        }
        info("开始播放 MIDI 文件 ...".to_string());
        debug(format!("估算总时长: {}ms", total_ms));

        // 交互控制
        let input = if interactive {
            Some(crate::input::InputListener::start())
        } else {
            None
        };

        // 进度条
        let mut tui = crate::tui::Tui::start(midi_path, "MIDI 音乐", show_progress);
        let mut prog = crate::progress::Progress::new(show_progress && tui.is_none());
        let mut last_bpm: i32 = 0;
        let mut paused = false;
        // FluidSynth 将 stop() 定义为暂停，但 seek() 是异步的；暂停时
        // 只在这里保存目标 tick，不调用 seek，避免拖动进度条重新启动
        // player。恢复时再一次性应用目标 tick。
        let mut paused_tick: Option<i32> = None;
        let mut looping = false;
        let mut quit = false;

        // 等待播放完成（PLAYING=1 → DONE=3）
        while !quit {
            // 处理键盘指令
            if let Some(il) = &input {
                loop {
                    let c = il.poll();
                    match c {
                        crate::input::Control::None => break,
                        crate::input::Control::Quit => {
                            quit = true;
                            break;
                        }
                        crate::input::Control::Pause => {
                            if !paused {
                                paused_tick = Some(unsafe { fluid_player_get_current_tick(player) }.max(0));
                                unsafe { fluid_player_stop(player) };
                                self.silence();
                                info("暂停".to_string());
                                prog.finish();
                                paused = true;
                            } else {
                                let target = paused_tick.take()
                                    .unwrap_or_else(|| unsafe { fluid_player_get_current_tick(player) }.max(0));
                                self.silence();
                                unsafe { fluid_player_seek(player, target) };
                                let ret = unsafe { fluid_player_play(player) };
                                if ret == FLUID_OK {
                                    info("继续".to_string());
                                    paused = false;
                                } else {
                                    paused_tick = Some(target);
                                    warn("MIDI 播放器恢复失败，仍保持暂停".to_string());
                                }
                            }
                        }
                        crate::input::Control::Play => {
                            if paused {
                                let target = paused_tick.take()
                                    .unwrap_or_else(|| unsafe { fluid_player_get_current_tick(player) }.max(0));
                                self.silence();
                                unsafe { fluid_player_seek(player, target) };
                                let ret = unsafe { fluid_player_play(player) };
                                if ret == FLUID_OK {
                                    info("播放".to_string());
                                    paused = false;
                                } else {
                                    paused_tick = Some(target);
                                    warn("MIDI 播放器恢复失败，仍保持暂停".to_string());
                                }
                            }
                        }
                        crate::input::Control::Loop => {
                            looping = !looping;
                            unsafe { fluid_player_set_loop(player, if looping { -1 } else { 0 }) };
                            info(if looping {
                                "循环播放：开".to_string()
                            } else {
                                "循环播放：关".to_string()
                            });
                        }
                        crate::input::Control::SeekForward(s)
                        | crate::input::Control::SeekBackward(s) => {
                            let sign = if matches!(c, crate::input::Control::SeekForward(_)) {
                                1.0
                            } else {
                                -1.0
                            };
                            let current = if paused {
                                paused_tick.unwrap_or_else(|| unsafe {
                                    fluid_player_get_current_tick(player)
                                })
                            } else {
                                unsafe { fluid_player_get_current_tick(player) }
                            }
                            .max(0);
                            let total = unsafe { fluid_player_get_total_ticks(player) }.max(0);
                            let division = unsafe { fluid_player_get_division(player) }.max(1);
                            let target = relative_seek_target(
                                current,
                                total,
                                division,
                                sign * s,
                                midi_time_map.as_ref(),
                                fixed_quarter_ms,
                            );
                            if paused {
                                paused_tick = Some(target);
                                // 暂停状态只更新逻辑播放头，绝不调用
                                // fluid_player_seek（该操作可能触发异步播放）。
                                self.silence();
                            } else {
                                self.silence();
                                unsafe { fluid_player_seek(player, target) };
                            }
                            info(format!("跳转 {}s", (sign * s) as i32));
                        }
                        crate::input::Control::SeekPercent(p) => {
                            if paused {
                                let total = unsafe { fluid_player_get_total_ticks(player) }.max(0);
                                let target = ((total as f64 * p.clamp(0.0, 1.0)).round() as i32)
                                    .clamp(0, total);
                                paused_tick = Some(target);
                                self.silence();
                            } else {
                                self.silence();
                                seek_percent(player, p);
                            }
                            info(format!("跳转到 {}%", (p * 100.0) as i32));
                        }
                        crate::input::Control::VolumeDown => {
                            self.adjust_volume(-0.1);
                        }
                        crate::input::Control::VolumeUp => {
                            self.adjust_volume(0.1);
                        }
                        crate::input::Control::Mouse(x, y) => {
                            if let Some(ui) = &tui {
                                let mouse = ui.mouse_control(x, y, paused);
                                match mouse {
                                    crate::input::Control::Pause if !paused => {
                                        paused_tick = Some(unsafe { fluid_player_get_current_tick(player) }.max(0));
                                        unsafe { fluid_player_stop(player) };
                                        self.silence();
                                        paused = true;
                                        info("暂停".to_string());
                                        prog.finish();
                                    }
                                    crate::input::Control::Pause => {
                                        let target = paused_tick.take()
                                            .unwrap_or_else(|| unsafe { fluid_player_get_current_tick(player) }.max(0));
                                        self.silence();
                                        unsafe { fluid_player_seek(player, target) };
                                        let ret = unsafe { fluid_player_play(player) };
                                        if ret == FLUID_OK {
                                            paused = false;
                                            info("继续".to_string());
                                        } else {
                                            paused_tick = Some(target);
                                            warn("MIDI 播放器恢复失败，仍保持暂停".to_string());
                                        }
                                    }
                                    crate::input::Control::SeekPercent(p) => {
                                        let total = unsafe { fluid_player_get_total_ticks(player) }.max(0);
                                        let target = ((total as f64 * p.clamp(0.0, 1.0)).round() as i32)
                                            .clamp(0, total);
                                        if paused {
                                            paused_tick = Some(target);
                                            self.silence();
                                        } else {
                                            self.silence();
                                            unsafe { fluid_player_seek(player, target) };
                                        }
                                        info(format!("跳转到 {}%", (p * 100.0) as i32));
                                        prog.finish();
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }

            let status = unsafe { fluid_player_get_status(player) };
            let bpm = unsafe { fluid_player_get_bpm(player) };
            if bpm != last_bpm {
                debug(format!("当前速度: {} BPM", bpm));
                last_bpm = bpm;
            }

            // 进度显示（基于真实经过时间）
            let current_tick = if paused {
                paused_tick.unwrap_or_else(|| unsafe { fluid_player_get_current_tick(player) }.max(0))
            } else {
                unsafe { fluid_player_get_current_tick(player) }.max(0)
            };
            if show_progress {
                let elapsed_ms = std::time::Instant::now();
                // 用 tick 计算更准确（暂停时 tick 不前进）
                let ct = current_tick;
                let tt = unsafe { fluid_player_get_total_ticks(player) };
                if tt > 0 {
                    let pct = (ct as f64 / tt as f64).clamp(0.0, 1.0);
                    prog.update_pct(pct, total_ms as u64);
                } else if total_ms > 0 {
                    let e = elapsed_ms.elapsed().as_millis() as u64;
                    prog.update(e, total_ms as u64);
                } else {
                    let e = elapsed_ms.elapsed().as_millis() as u64;
                    prog.update(e, e + 1);
                }
            }
            if let Some(ui) = &mut tui {
                let ct = current_tick as u64;
                let tt = unsafe { fluid_player_get_total_ticks(player) }.max(1) as u64;
                let notes = active_midi_notes(&display.events, ct);
                ui.draw(
                    total_ms as u64 * ct / tt,
                    total_ms as u64,
                    self.volume(),
                    paused,
                    looping,
                    &midi_tracks_text(&notes, &display.tracks, display.main_track),
                    &midi_spectrum(&notes),
                );
            }

            if status == FLUID_PLAYER_DONE {
                // 暂停时 stop() 也会触发 DONE，但此时 paused=true 不退出
                if !paused {
                    if looping {
                        // set_loop 模式下 DONE 表示循环已结束？这里手动继续
                        paused_tick = None;
                        unsafe { fluid_player_play(player) };
                        continue;
                    } else {
                        break;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        if let Some(mut il) = input {
            il.stop();
        }
        prog.finish();
        drop(tui);

        // stop/join 先于 delete，避免用户按 Q 时后台 MIDI 线程仍访问
        // 已释放的 player；同时清除所有可能残留的合成器声音。
        unsafe { fluid_player_stop(player); }
        unsafe { fluid_player_join(player); }
        self.silence();
        unsafe { delete_fluid_player(player) };
        info("MIDI 播放完成".to_string());
        let callback_error = if switch_state.is_null() {
            None
        } else {
            unsafe { Box::from_raw(switch_state) }.error
        };
        if let Some(error) = callback_error {
            Err(error)
        } else {
            Ok(())
        }
    }

    /// 事件总量（用于等待结束）
    /// 返回 (轨道通道数, 目标结束时间)
    /// 由于 sequencer 的 send_at 是累积的，结束时等 now_ms >= end 即可
    pub fn wait_until(&self, end_ms: u32, show_progress: bool) {
        let mut prog = crate::progress::Progress::new(show_progress);
        loop {
            let n = self.now_ms();
            prog.update(n as u64, end_ms as u64);
            if n >= end_ms {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        prog.finish();
    }

    /// 交互式播放简谱事件列表（支持快进/后退/暂停/循环/退出）。
    ///
    /// 原理：不一次性把事件排入 sequencer，而是维护一个"播放头"（playhead），
    /// 把当前 playhead 之后的事件按相对偏移动态排程。快进/后退/循环时：
    ///   1. 清除所有已排事件（fluid_sequencer_remove_events）
    ///   2. 更新 playhead
    ///   3. 从新 playhead 重新排程剩余事件（相对时间）
    ///
    /// 暂停时清除已排事件并停止排程，恢复时重新排程。
    ///
    /// `events` 必须已按 at_ms 升序排序。
    pub fn play_events_interactive(
        &mut self,
        events: &[crate::parser::ScheduledNote],
        total_ms: u32,
        show_progress: bool,
        initial_soundfont: usize,
        initial_instrument: u8,
        program_switches: &[ProgramSwitch],
        art: Option<crate::tui::ArtImage>,
        art_zoom: f32,
    ) -> Result<(), String> {
        // 所有要求必须在进入播放 TUI 前可用；不再把错误降级成警告。
        self.validate_program_requirements(
            initial_soundfont,
            initial_instrument,
            program_switches,
        )?;
        let mut input = crate::input::InputListener::start();
        let mut tui = crate::tui::Tui::start_with_art("简谱演奏", "简谱", show_progress, art)
            .map(|tui| tui.with_art_zoom(art_zoom));
        let mut prog = crate::progress::Progress::new(show_progress && tui.is_none());

        let mut playhead: i64 = 0; // 当前播放位置（毫秒）
        let mut paused = false;
        let mut looping = false;
        let mut quit = false;
        let channels: Vec<u8> = events
            .iter()
            .map(|event| event.channel)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        // 排程从 playhead 开始的所有剩余事件。
        // 注：sequencer 的 tick 单调递增且无法重置，因此这里以"当前 tick"为基准，
        //     把 (at_ms - playhead) 的相对偏移换算成绝对 tick（base + rel）。
        fn schedule_from(
            player: &SynthPlayer,
            events: &[crate::parser::ScheduledNote],
            channels: &[u8],
            initial_soundfont: usize,
            initial_instrument: u8,
            program_switches: &[ProgramSwitch],
            playhead: i64,
        ) -> Result<u32, String> {
            player.clear_schedule();
            let base = player.now_ms(); // 当前绝对 tick（毫秒）

            // 跳转/恢复后 sequencer 队列会被清空，而 Synth 本身会保留上一次
            // 的预置。先按播放头重新选中当前应生效的音源，之后再补发长音符，
            // 这样跳回切换点之前也不会错误沿用后半段的音色。
            let selected = selection_at(
                initial_soundfont,
                initial_instrument,
                program_switches,
                playhead.max(0) as u32,
            );
            player.select_for_channels(
                channels,
                selected.soundfont,
                selected.instrument,
            )?;

            // 必须先排 program-select，再排同一毫秒的 note-on，保证切换秒
            // 的第一个音符已经使用新音源。当前播放头上的切换已即时应用，
            // 因此这里只排严格位于未来的事件。
            for switch_ in program_switches {
                if switch_.at_ms as i64 > playhead {
                    let rel = (switch_.at_ms as i64 - playhead).max(0) as u32;
                    player.schedule_soundfont_switch(
                        channels,
                        *switch_,
                        base.wrapping_add(rel),
                    )?;
                }
            }

            // 暂停/跳转会主动清掉 synth 中的发声。若播放头落在一个
            // 长音符中，恢复时需要在新的基准 tick 重新补发 note-on，
            // 否则只会收到后续 note-off，听感上像音符被截断。
            let mut active: BTreeMap<(u8, u8), Vec<(u8, u32)>> = BTreeMap::new();
            for ev in events {
                if ev.at_ms as i64 >= playhead {
                    break;
                }
                let key = (ev.channel, ev.key);
                if ev.on {
                    active.entry(key).or_default().push((ev.vel, ev.at_ms));
                } else if let Some(notes) = active.get_mut(&key) {
                    notes.pop();
                    if notes.is_empty() {
                        active.remove(&key);
                    }
                }
            }
            let mut recovered_long_note = false;
            for ((channel, key), notes) in active {
                for (vel, started_at) in notes {
                    // 连续播放时，切换只影响之后新起的音符；跨越切换点的
                    // 长音仍保持起音时预置。跳转恢复必须先排回起音时音色，
                    // 再补 note-on，随后才恢复播放头当前音色。
                    let onset = selection_at(
                        initial_soundfont,
                        initial_instrument,
                        program_switches,
                        started_at,
                    );
                    player.schedule_soundfont_switch(&[channel], onset, base)?;
                    player.schedule_note(channel as i32, key, vel, base, true)?;
                    recovered_long_note = true;
                }
            }
            if recovered_long_note {
                player.schedule_soundfont_switch(channels, selected, base)?;
            }

            for ev in events {
                // 只排未来事件：at_ms >= playhead
                if ev.at_ms as i64 >= playhead {
                    let rel = (ev.at_ms as i64 - playhead).max(0) as u32;
                    let abs = base.wrapping_add(rel);
                    player.schedule_note(ev.channel as i32, ev.key, ev.vel, abs, ev.on)?;
                }
            }
            Ok(base)
        }

        // 最近一次排程时的 sequencer tick（基准），从 0 开始排程
        let mut anchor_tick: u32 = schedule_from(
            self,
            events,
            &channels,
            initial_soundfont,
            initial_instrument,
            program_switches,
            0,
        )?;
        loop {
            // 处理键盘
            loop {
                let c = input.poll();
                match c {
                    crate::input::Control::None => break,
                    crate::input::Control::Quit => {
                        quit = true;
                        break;
                    }
                    crate::input::Control::Pause => {
                        if paused {
                            // 恢复：先确保旧音符已静音，再从播放头重排。
                            self.silence();
                            anchor_tick = schedule_from(self, events, &channels, initial_soundfont, initial_instrument, program_switches, playhead)?;
                            paused = false;
                            info("继续".to_string());
                        } else {
                            // 记录暂停时的位置，移除未来事件并立即清掉
                            // 已经送进 synth 的音符（含 sustain 音符）。
                            let now = self.now_ms();
                            playhead = (playhead + now as i64 - anchor_tick as i64)
                                .clamp(0, total_ms as i64);
                            anchor_tick = now;
                            self.clear_schedule();
                            self.silence();
                            paused = true;
                            info("暂停".to_string());
                            prog.finish();
                        }
                    }
                    crate::input::Control::Play => {
                        if paused {
                            self.silence();
                            anchor_tick = schedule_from(self, events, &channels, initial_soundfont, initial_instrument, program_switches, playhead)?;
                            paused = false;
                            info("播放".to_string());
                        }
                    }
                    crate::input::Control::Loop => {
                        looping = !looping;
                        info(if looping {
                            "循环播放：开".to_string()
                        } else {
                            "循环播放：关".to_string()
                        });
                    }
                    crate::input::Control::SeekForward(s)
                    | crate::input::Control::SeekBackward(s) => {
                        let sign: i64 = if matches!(c, crate::input::Control::SeekForward(_)) {
                            1
                        } else {
                            -1
                        };
                        let delta = (s * 1000.0) as i64 * sign;
                        // 从当前播放位置计算新 playhead
                        let cur = if paused {
                            playhead
                        } else {
                            playhead + self.now_ms() as i64 - anchor_tick as i64
                        };
                        let target = (cur + delta).clamp(0, total_ms as i64);
                        playhead = target;
                        self.clear_schedule();
                        self.silence();
                        anchor_tick = if paused {
                            // 暂停时只移动播放头，绝不重新排程；否则
                            // sequencer 会在暂停状态下继续触发音符。
                            self.now_ms()
                        } else {
                            schedule_from(self, events, &channels, initial_soundfont, initial_instrument, program_switches, playhead)?
                        };
                        info(format!("跳转至 {:.1}s", playhead as f64 / 1000.0));
                        prog.finish();
                    }
                    crate::input::Control::SeekPercent(p) => {
                        let target = ((total_ms as f64 * p.clamp(0.0, 1.0)).round() as i64)
                            .clamp(0, total_ms as i64);
                        playhead = target;
                        self.clear_schedule();
                        self.silence();
                        anchor_tick = if paused {
                            self.now_ms()
                        } else {
                            schedule_from(self, events, &channels, initial_soundfont, initial_instrument, program_switches, playhead)?
                        };
                        info(format!("跳转至 {}%", (p * 100.0) as i32));
                        prog.finish();
                    }
                    crate::input::Control::VolumeDown => {
                        self.adjust_volume(-0.1);
                    }
                    crate::input::Control::VolumeUp => {
                        self.adjust_volume(0.1);
                    }
                    crate::input::Control::Mouse(x, y) => {
                        if let Some(ui) = &tui {
                            match ui.mouse_control(x, y, paused) {
                                crate::input::Control::Pause => {
                                    if paused {
                                        self.silence();
                                        anchor_tick = schedule_from(self, events, &channels, initial_soundfont, initial_instrument, program_switches, playhead)?;
                                        paused = false;
                                        info("继续".to_string());
                                    } else {
                                        let now = self.now_ms();
                                        playhead = (playhead + now as i64 - anchor_tick as i64)
                                            .clamp(0, total_ms as i64);
                                        anchor_tick = now;
                                        self.clear_schedule();
                                        self.silence();
                                        paused = true;
                                        info("暂停".to_string());
                                        prog.finish();
                                    }
                                }
                                crate::input::Control::SeekPercent(p) => {
                                    playhead = (total_ms as f64 * p.clamp(0.0, 1.0)).round()
                                        .clamp(0.0, total_ms as f64) as i64;
                                    self.clear_schedule();
                                    self.silence();
                                    anchor_tick = if paused {
                                        self.now_ms()
                                    } else {
                                        schedule_from(self, events, &channels, initial_soundfont, initial_instrument, program_switches, playhead)?
                                    };
                                    info(format!("跳转至 {}%", (p * 100.0) as i32));
                                    prog.finish();
                                }
                                crate::input::Control::Play => {
                                    self.silence();
                                    anchor_tick = schedule_from(self, events, &channels, initial_soundfont, initial_instrument, program_switches, playhead)?;
                                    paused = false;
                                    info("播放".to_string());
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }

            if quit {
                break;
            }

            // 暂停时：等待恢复/退出
            if paused {
                if let Some(ui) = &mut tui {
                    let notes = active_score_notes(events, playhead);
                    ui.draw(
                        playhead as u64,
                        total_ms as u64,
                        self.volume(),
                        true,
                        looping,
                        &score_tracks_text(&notes),
                        &midi_spectrum(&notes),
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }

            let now = self.now_ms();
            // 实际播放位置 = playhead（锚点）+ 相对流逝时间
            let cur = (playhead + now as i64 - anchor_tick as i64).min(total_ms as i64);

            // 进度显示
            if show_progress {
                prog.update(cur as u64, total_ms as u64);
            }
            if let Some(ui) = &mut tui {
                let notes = active_score_notes(events, cur);
                ui.draw(
                    cur as u64,
                    total_ms as u64,
                    self.volume(),
                    paused,
                    looping,
                    &score_tracks_text(&notes),
                    &midi_spectrum(&notes),
                );
            }

            // 播放结束判定
            if cur >= total_ms as i64 {
                if looping {
                    playhead = 0;
                    self.silence();
                    anchor_tick = schedule_from(self, events, &channels, initial_soundfont, initial_instrument, program_switches, 0)?;
                    info("循环播放：从头开始".to_string());
                    prog.finish();
                    continue;
                } else {
                    break;
                }
            }

            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        // 无论是用户退出还是自然结束，都不要把最后一个音符/踏板
        // 状态留在共享的 synth 中。
        self.clear_schedule();
        self.silence();
        input.stop();
        prog.finish();
        drop(tui);
        Ok(())
    }

    /// 发出 note 调度（对外接口）
    /// 轨道数大于 16 时，超出部分映射回 [0,16)
    pub fn play_note(&self, channel: u8, key: u8, vel: u8, on: bool, at_ms: u32) {
        let ch = (channel % 16) as c_int;
        debug(format!(
            "[{}ms] ch{} {} key={} vel={}",
            at_ms,
            ch,
            if on { "note-on " } else { "note-off" },
            key,
            vel
        ));
        if let Err(error) = self.schedule_note(ch, key, vel, at_ms, on) {
            warn(error);
        }
    }

    // ---- 仅供测试 ----
    #[allow(dead_code)]
    pub fn raw_synth(&self) -> *mut fluid_synth_t {
        self.synth
    }

    /// 播放完所有事件后释放
    pub fn shutdown(&mut self) {
        if self.freed {
            return;
        }
        info("关闭音频引擎 ...".to_string());
        self.clear_schedule();
        self.silence();
        unsafe {
            delete_fluid_sequencer(self.sequencer);
            delete_fluid_audio_driver(self.audio_driver);
            delete_fluid_synth(self.synth);
            delete_fluid_settings(self.settings);
        }
        self.freed = true;
        info("音频引擎已关闭".to_string());
    }
}

fn midi_note_name(key: u8) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES[(key % 12) as usize], key as i16 / 12 - 1)
}

fn active_score_notes(events: &[crate::parser::ScheduledNote], at_ms: i64) -> BTreeMap<usize, BTreeSet<u8>> {
    let mut active: BTreeMap<u8, BTreeSet<u8>> = BTreeMap::new();
    // 预先登记所有轨道，即使当前时刻没有按下音符也要保留显示行。
    // 否则 details.len() 会随音符开关变化，导致 TUI 布局上下跳动/闪烁。
    for event in events {
        active.entry(event.channel).or_default();
    }
    for event in events.iter().take_while(|event| event.at_ms as i64 <= at_ms) {
        let notes = active.entry(event.channel).or_default();
        if event.on {
            notes.insert(event.key);
        } else {
            notes.remove(&event.key);
        }
    }
    active.into_iter().map(|(channel, notes)| (channel as usize, notes)).collect()
}

fn score_tracks_text(active: &BTreeMap<usize, BTreeSet<u8>>) -> Vec<String> {
    active
        .iter()
        .map(|(channel, notes)| {
            let text = if notes.is_empty() {
                "x".to_string()
            } else {
                notes.iter().map(|key| midi_note_name(*key)).collect::<Vec<_>>().join(" ")
            };
            format!("轨道 {}: {}", channel + 1, text)
        })
        .collect()
}

#[derive(Clone, Copy)]
struct MidiDisplayEvent {
    tick: u64,
    track: usize,
    key: u8,
    on: bool,
}

struct MidiDisplay {
    events: Vec<MidiDisplayEvent>,
    tracks: Vec<usize>,
    main_track: Option<usize>,
}

#[derive(Clone)]
struct MidiTimeMap {
    division: u16,
    tempos: Vec<(u64, u32)>,
}

impl MidiTimeMap {
    fn tick_to_ms(&self, target: u64) -> u64 {
        if self.division == 0 || self.division & 0x8000 != 0 {
            return target.saturating_mul(500) / 480;
        }
        let mut elapsed_us = 0u128;
        let mut previous_tick = 0u64;
        let mut tempo = 500_000u32;
        for &(tick, next_tempo) in &self.tempos {
            if tick > target {
                break;
            }
            elapsed_us = elapsed_us.saturating_add(
                (tick.saturating_sub(previous_tick) as u128)
                    .saturating_mul(tempo as u128)
                    / self.division as u128,
            );
            previous_tick = tick;
            tempo = next_tempo.max(1);
        }
        elapsed_us = elapsed_us.saturating_add(
            (target.saturating_sub(previous_tick) as u128)
                .saturating_mul(tempo as u128)
                / self.division as u128,
        );
        (elapsed_us / 1000).min(u128::from(u32::MAX)) as u64
    }

    /// 返回第一个时间不早于 `target_ms` 的 MIDI tick。
    fn ms_to_tick(&self, target_ms: u32) -> u64 {
        if self.division == 0 || self.division & 0x8000 != 0 {
            return u64::from(target_ms).saturating_mul(480).div_ceil(500);
        }
        let target_us = u128::from(target_ms) * 1000;
        let mut elapsed_us = 0u128;
        let mut previous_tick = 0u64;
        let mut tempo = 500_000u32;

        for &(tick, next_tempo) in &self.tempos {
            if tick <= previous_tick {
                tempo = next_tempo.max(1);
                continue;
            }
            let segment_us = u128::from(tick - previous_tick)
                .saturating_mul(u128::from(tempo))
                / u128::from(self.division);
            if target_us <= elapsed_us.saturating_add(segment_us) {
                let remaining = target_us.saturating_sub(elapsed_us);
                let ticks = remaining
                    .saturating_mul(u128::from(self.division))
                    .div_ceil(u128::from(tempo));
                return previous_tick.saturating_add(ticks.min(u128::from(u64::MAX)) as u64);
            }
            elapsed_us = elapsed_us.saturating_add(segment_us);
            previous_tick = tick;
            tempo = next_tempo.max(1);
        }

        let remaining = target_us.saturating_sub(elapsed_us);
        let ticks = remaining
            .saturating_mul(u128::from(self.division))
            .div_ceil(u128::from(tempo));
        previous_tick.saturating_add(ticks.min(u128::from(u64::MAX)) as u64)
    }
}

fn midi_uses_smpte_division(path: &str) -> bool {
    use std::io::Read;

    let mut header = [0u8; 14];
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    if file.read_exact(&mut header).is_err() || &header[..4] != b"MThd" {
        return false;
    }
    u16::from_be_bytes([header[12], header[13]]) & 0x8000 != 0
}

/// 读取 MIDI tempo map，用于把“第几秒”配置转换成实际的 MIDI tick。
/// 解析失败时由调用方退回固定 120 BPM 的近似值，不影响普通播放。
fn midi_time_map(path: &str) -> Option<MidiTimeMap> {
    use std::io::Read;
    let mut data = Vec::new();
    std::fs::File::open(path).ok()?.read_to_end(&mut data).ok()?;
    if data.len() < 14 || &data[..4] != b"MThd" {
        return None;
    }
    let header_len = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
    if header_len < 6 || 8usize.saturating_add(header_len) > data.len() {
        return None;
    }
    let division = u16::from_be_bytes([data[12], data[13]]);
    if division == 0 || division & 0x8000 != 0 {
        return None;
    }
    let mut tempos = vec![(0u64, 500_000u32)];
    let mut pos = 8 + header_len;
    fn varlen(data: &[u8], pos: &mut usize, end: usize) -> Option<u64> {
        let mut value = 0u64;
        for _ in 0..4 {
            if *pos >= end { return None; }
            let byte = *data.get(*pos)?;
            *pos += 1;
            value = (value << 7) | u64::from(byte & 0x7f);
            if byte & 0x80 == 0 { return Some(value); }
        }
        None
    }
    while pos + 8 <= data.len() && &data[pos..pos + 4] == b"MTrk" {
        let length = u32::from_be_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]) as usize;
        pos += 8;
        let end = pos.checked_add(length)?;
        if end > data.len() { return None; }
        let mut tick = 0u64;
        let mut running = 0u8;
        while pos < end {
            tick = tick.saturating_add(varlen(&data, &mut pos, end)?);
            let first = *data.get(pos)?;
            let (status, consumed_first_data) = if first < 0x80 {
                if running == 0 { return None; }
                pos += 1;
                (running, true)
            } else {
                pos += 1;
                if first < 0xf0 { running = first; }
                (first, false)
            };
            match status {
                0xff => {
                    let kind = *data.get(pos)?;
                    pos += 1;
                    let length = varlen(&data, &mut pos, end)? as usize;
                    let payload_end = pos.checked_add(length)?;
                    if payload_end > end { return None; }
                    if kind == 0x51 && length >= 3 {
                        let value = (u32::from(data[pos]) << 16)
                            | (u32::from(data[pos + 1]) << 8)
                            | u32::from(data[pos + 2]);
                        tempos.push((tick, value.max(1)));
                    }
                    pos = payload_end;
                }
                0xf0 | 0xf7 => {
                    let length = varlen(&data, &mut pos, end)? as usize;
                    pos = pos.checked_add(length)?;
                    if pos > end { return None; }
                }
                status => {
                    let needed = match status {
                        0x80..=0x8f | 0x90..=0x9f | 0xa0..=0xaf | 0xb0..=0xbf | 0xe0..=0xef => 2,
                        0xc0..=0xcf | 0xd0..=0xdf | 0xf1 | 0xf3 => 1,
                        0xf2 => 2,
                        0xf6 | 0xf8..=0xfe => 0,
                        _ => return None,
                    };
                    let remaining = needed - usize::from(consumed_first_data);
                    pos = pos.checked_add(remaining)?;
                    if pos > end { return None; }
                }
            }
        }
        pos = end;
    }
    tempos.sort_by_key(|(tick, _)| *tick);
    Some(MidiTimeMap { division, tempos })
}

fn midi_display_events(path: &str) -> MidiDisplay {
    use std::io::Read;

    fn read_varlen(data: &[u8], pos: &mut usize, end: usize) -> Option<u64> {
        let mut value = 0;
        for _ in 0..4 {
            if *pos >= end { return None; }
            let byte = data[*pos];
            *pos += 1;
            value = (value << 7) | u64::from(byte & 0x7f);
            if byte & 0x80 == 0 { return Some(value); }
        }
        None
    }

    let mut data = Vec::new();
    if std::fs::File::open(path).and_then(|mut file| file.read_to_end(&mut data)).is_err() || data.len() < 14 || &data[..4] != b"MThd" {
        return MidiDisplay { events: Vec::new(), tracks: Vec::new(), main_track: None };
    }
    let header_len = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let mut pos = 8usize.saturating_add(header_len);
    let mut result = Vec::new();
    let mut track = 0usize;
    while pos + 8 <= data.len() && &data[pos..pos + 4] == b"MTrk" {
        let length = u32::from_be_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]) as usize;
        pos += 8;
        let end = pos.saturating_add(length).min(data.len());
        let mut tick = 0u64;
        let mut running = 0u8;
        while pos < end {
            let Some(delta) = read_varlen(&data, &mut pos, end) else { break; };
            tick += delta;
            if pos >= end { break; }
            let first = data[pos];
            let (status, data1) = if first < 0x80 {
                if running == 0 { break; }
                pos += 1;
                (running, Some(first))
            } else {
                pos += 1;
                if first < 0xf0 { running = first; }
                (first, None)
            };
            match status {
                0xff => {
                    if pos >= end { break; }
                    pos += 1;
                    let Some(length) = read_varlen(&data, &mut pos, end) else { break; };
                    pos = pos.saturating_add(length as usize).min(end);
                }
                0xf0 | 0xf7 => {
                    let Some(length) = read_varlen(&data, &mut pos, end) else { break; };
                    pos = pos.saturating_add(length as usize).min(end);
                }
                _ => {
                    let kind = status & 0xf0;
                    let needed = if kind == 0xc0 || kind == 0xd0 { 1 } else { 2 };
                    let first_data = match data1 {
                        Some(value) => value,
                        None => {
                            if pos >= end { break; }
                            let value = data[pos];
                            pos += 1;
                            value
                        }
                    };
                    let second_data = if needed == 2 {
                        if pos >= end { break; }
                        let value = data[pos];
                        pos += 1;
                        Some(value)
                    } else { None };
                    if kind == 0x80 || kind == 0x90 {
                        if let Some(velocity) = second_data {
                            result.push(MidiDisplayEvent { tick, track, key: first_data, on: kind == 0x90 && velocity != 0 });
                        }
                    }
                }
            }
        }
        pos = end;
        track += 1;
    }
    // MIDI 文件按轨道存储事件；播放状态按时间查询前必须合并为 tick 顺序。
    result.sort_by_key(|event| event.tick);

    let mut stats: BTreeMap<usize, (u32, u64, u32, BTreeSet<u8>)> = BTreeMap::new();
    for event in &result {
        let entry = stats.entry(event.track).or_default();
        if event.on {
            entry.0 += 1;
            entry.1 += u64::from(event.key);
            entry.3.insert(event.key);
            entry.2 = entry.2.max(entry.3.len() as u32);
        } else {
            entry.3.remove(&event.key);
        }
    }
    // 主旋律通常位于较高音区，且同一时间的音符更少；跳过仅元数据的轨道。
    let main_track = stats.iter().filter(|(_, stat)| stat.0 >= 4).max_by(|(_, a), (_, b)| {
        let score = |stat: &(u32, u64, u32, BTreeSet<u8>)| {
            stat.1 as f64 / stat.0 as f64 + 24.0 / stat.2.max(1) as f64 + (stat.0.min(512) as f64).sqrt()
        };
        score(a).partial_cmp(&score(b)).unwrap_or(std::cmp::Ordering::Equal)
    }).map(|(track, _)| *track);
    let tracks = stats.keys().copied().collect();
    MidiDisplay { events: result, tracks, main_track }
}

fn active_midi_notes(events: &[MidiDisplayEvent], tick: u64) -> BTreeMap<usize, BTreeSet<u8>> {
    let mut active: BTreeMap<usize, BTreeSet<u8>> = BTreeMap::new();
    for event in events.iter().take_while(|event| event.tick <= tick) {
        let notes = active.entry(event.track).or_default();
        if event.on { notes.insert(event.key); } else { notes.remove(&event.key); }
    }
    active
}

fn midi_tracks_text(active: &BTreeMap<usize, BTreeSet<u8>>, tracks: &[usize], main_track: Option<usize>) -> Vec<String> {
    tracks
        .iter()
        .enumerate()
        .map(|(display_index, track)| {
            let label = if Some(*track) == main_track { "主旋律" } else { "轨道" };
            let notes = active.get(track)
                .filter(|notes| !notes.is_empty())
                .map(|notes| notes.iter().map(|key| midi_note_name(*key)).collect::<Vec<_>>().join(" "))
                .unwrap_or_else(|| "x".to_string());
            format!("{} {}: {}", label, display_index + 1, notes)
        })
        .collect()
}

fn midi_spectrum(active: &BTreeMap<usize, BTreeSet<u8>>) -> [u8; 16] {
    let mut levels: [u8; 16] = [0; 16];
    for key in active.values().flat_map(|notes| notes.iter()) {
        let frequency = 440.0_f32 * 2.0_f32.powf((*key as f32 - 69.0) / 12.0);
        let band = ((frequency / 20.0).ln() / 500.0_f32.ln() * 15.0).round().clamp(0.0, 15.0) as usize;
        levels[band] = levels[band].saturating_add(3).min(7);
        if band > 0 { levels[band - 1] = levels[band - 1].saturating_add(1).min(7); }
        if band < 15 { levels[band + 1] = levels[band + 1].saturating_add(1).min(7); }
    }
    levels
}

impl Drop for SynthPlayer {
    fn drop(&mut self) {
        if !self.freed {
            unsafe {
                delete_fluid_sequencer(self.sequencer);
                delete_fluid_audio_driver(self.audio_driver);
                delete_fluid_synth(self.synth);
                delete_fluid_settings(self.settings);
            }
        }
    }
}

#[cfg(test)]
mod display_tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn score_empty_tracks_use_x_placeholder() {
        let mut active = BTreeMap::new();
        active.insert(0usize, BTreeSet::new());
        active.insert(1usize, [60u8].into_iter().collect());
        assert_eq!(score_tracks_text(&active), vec!["轨道 1: x", "轨道 2: C4"]);
    }

    #[test]
    fn midi_empty_tracks_use_x_placeholder() {
        let active = BTreeMap::new();
        assert_eq!(midi_tracks_text(&active, &[0, 1], None), vec!["轨道 1: x", "轨道 2: x"]);
    }

    #[test]
    fn program_switches_are_limited_ordered_and_select_last_same_time() {
        let switches = [
            ProgramSwitch { at_ms: 1_000, soundfont: 0, instrument: 12 },
            ProgramSwitch { at_ms: 1_000, soundfont: 0, instrument: 41 },
            ProgramSwitch { at_ms: 2_000, soundfont: 0, instrument: 80 },
        ];
        assert!(validate_program_switches(&switches, 1).is_ok());
        assert_eq!(selection_at(0, 0, &switches, 999).instrument, 0);
        assert_eq!(selection_at(0, 0, &switches, 1_000).instrument, 41);
        assert!(validate_program_switches(
            &[ProgramSwitch { at_ms: 2_000, soundfont: 0, instrument: 0 }, ProgramSwitch { at_ms: 1_000, soundfont: 0, instrument: 0 }],
            1,
        ).is_err());
        assert!(validate_program_switches(
            &vec![ProgramSwitch { at_ms: 0, soundfont: 0, instrument: 0 }; MAX_PROGRAM_SWITCHES + 1],
            1,
        ).is_err());
    }

    #[test]
    fn soundfont_paths_reject_duplicates_and_more_than_three() {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let base = std::env::temp_dir().join(format!("music-rust-sf2-{stamp}"));
        let paths = (0..4).map(|index| base.with_extension(format!("{index}.sf2"))).collect::<Vec<_>>();
        for path in &paths { fs::write(path, [0u8]).unwrap(); }
        let first_three = paths[..3].iter().map(|path| path.to_string_lossy().into_owned()).collect::<Vec<_>>();
        assert!(validate_soundfont_paths(&first_three).is_ok());
        assert!(validate_soundfont_paths(&[first_three[0].clone(), first_three[0].clone()]).is_err());
        let four = paths.iter().map(|path| path.to_string_lossy().into_owned()).collect::<Vec<_>>();
        assert!(validate_soundfont_paths(&four).is_err());
        for path in paths { let _ = fs::remove_file(path); }
    }

    #[test]
    fn soundfont_paths_enforce_pairwise_120_mb_limit() {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let paths = (0..3)
            .map(|index| {
                std::env::temp_dir().join(format!(
                    "music-rust-pair-limit-{stamp}-{index}.sf2"
                ))
            })
            .collect::<Vec<_>>();
        for path in &paths {
            let file = fs::File::create(path).unwrap();
            file.set_len(50 * 1_000_000).unwrap();
        }
        let names = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        // 三个音源总计 150 MB，但任意一对只有 100 MB，应当允许。
        assert!(validate_soundfont_paths(&names).is_ok());

        fs::File::options()
            .write(true)
            .open(&paths[2])
            .unwrap()
            .set_len(71 * 1_000_000)
            .unwrap();
        assert!(validate_soundfont_paths(&names).is_err());
        for path in paths {
            let _ = fs::remove_file(path);
        }

        let single = std::env::temp_dir().join(format!("music-rust-single-limit-{stamp}.sf2"));
        let file = fs::File::create(&single).unwrap();
        file.set_len(MAX_SOUNDFONT_PAIR_BYTES + 1).unwrap();
        assert!(validate_soundfont_paths(&[single.to_string_lossy().into_owned()]).is_err());
        let _ = fs::remove_file(single);
    }

    #[test]
    fn midi_time_map_honors_tempo_changes_and_running_status() {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("music-rust-tempo-{stamp}.mid"));
        // PPQ=480: at tick 480 the tempo changes from 500000 to 1000000 us/qn.
        // Running-status note events after it ensure the parser advances correctly.
        let track = [
            0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20,
            0x83, 0x60, 0xff, 0x51, 0x03, 0x0f, 0x42, 0x40,
            0x00, 0x90, 0x3c, 0x40,
            0x81, 0x70, 0x3c, 0x00,
            0x00, 0xff, 0x2f, 0x00,
        ];
        let mut midi = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk".to_vec();
        midi.extend_from_slice(&(track.len() as u32).to_be_bytes());
        midi.extend_from_slice(&track);
        fs::write(&path, midi).unwrap();
        let map = midi_time_map(path.to_str().unwrap()).unwrap();
        assert_eq!(map.tick_to_ms(480), 500);
        assert_eq!(map.tick_to_ms(720), 1_000);
        assert_eq!(map.ms_to_tick(500), 480);
        assert_eq!(map.ms_to_tick(1_000), 720);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn relative_midi_seek_uses_real_tempo_and_saturates_long_files() {
        let map = MidiTimeMap {
            division: 480,
            tempos: vec![(0, 500_000), (480, 1_000_000)],
        };
        // tick 480 = 0.5s；再前进 1s（此后为 60 BPM）应到 tick 960。
        assert_eq!(
            relative_seek_target(480, 4_800, 480, 1.0, Some(&map), 500.0),
            960
        );
        // 强制 60 BPM 时，每秒恰好 480 tick。
        assert_eq!(
            relative_seek_target(480, 4_800, 480, 5.0, None, 1_000.0),
            2_880
        );
        assert_eq!(
            relative_seek_target(i32::MAX - 10, i32::MAX, 960, 60.0, None, 500.0),
            i32::MAX
        );
    }
}

#[cfg(test)]
mod volume_tests {
    use super::*;

    #[test]
    fn percent_conversion_matches_audio_mode_range() {
        assert_eq!(volume_scale_from_percent(0), 0.0);
        assert!((volume_scale_from_percent(80) - 0.8).abs() < f32::EPSILON);
        assert!((volume_scale_from_percent(500) - 5.0).abs() < f32::EPSILON);
        // CLI/API 输入即使越界，也不能让 FluidSynth 收到超过约定范围的增益。
        assert!((volume_scale_from_percent(u32::MAX) - 5.0).abs() < f32::EPSILON);
    }

    #[test]
    fn percent_display_rounds_float_accumulation() {
        // 反复按 9/0 会产生 0.599999... 之类的 f32 值；显示必须和
        // 实际设置的 0.6 一致，而不能截断成 59%。
        let mut scale = DEFAULT_VOLUME_SCALE;
        for _ in 0..2 {
            scale = (scale - 0.1).max(0.0);
        }
        assert_eq!(volume_percent_from_scale(scale), 60);
    }

    #[test]
    fn default_state_is_same_value_sent_to_fluidsynth() {
        // new() 在创建音频驱动前会把 synth gain 设为 DEFAULT_VOLUME_SCALE；
        // 这条约束防止 UI 初始值与实际增益再次分叉。
        assert_eq!(volume_percent_from_scale(DEFAULT_VOLUME_SCALE), 80);
    }

    #[test]
    fn limiter_reset_discards_stale_release_gain() {
        let mut limiter = LimiterState::new(-1.0);
        limiter.current_gain = 0.23;
        let mut seen = 4;
        reset_limiter_if_requested(5, &mut seen, &mut limiter);
        assert_eq!(seen, 5);
        assert_eq!(limiter.current_gain, 1.0);
        // 同一版本不能重复改写音频线程自己的包络状态。
        limiter.current_gain = 0.61;
        reset_limiter_if_requested(5, &mut seen, &mut limiter);
        assert_eq!(limiter.current_gain, 0.61);
    }
}

// ---- 仅供测试使用：直接 note on/off ----
#[allow(dead_code)]
pub fn fluid_synth_noteon_direct(synth: *mut fluid_synth_t, chan: i32, key: i32, vel: i32) {
    unsafe { fluid_synth_noteon(synth, chan, key, vel) };
}

#[allow(dead_code)]
pub fn fluid_synth_noteoff_direct(synth: *mut fluid_synth_t, chan: i32, key: i32) {
    unsafe { fluid_synth_noteoff(synth, chan, key) };
}

// ---------------------------------------------------------------------------
// MIDI 播放器交互辅助函数
// ---------------------------------------------------------------------------

/// 把“前进/后退若干秒”换算为目标 tick。保留原 MIDI tempo map 时按每个
/// 变速段精确换算；强制 BPM 时使用固定四分音符时长。全部计算先提升到
/// f64/u64，再钳制回播放器的 i32 tick，避免长乐曲相加溢出。
fn relative_seek_target(
    current_tick: i32,
    total_tick: i32,
    division: i32,
    delta_seconds: f64,
    time_map: Option<&MidiTimeMap>,
    fixed_quarter_ms: f64,
) -> i32 {
    let total_tick = total_tick.max(0);
    let current_tick = current_tick.clamp(0, total_tick);
    if let Some(map) = time_map {
        let current_ms = map.tick_to_ms(current_tick as u64) as f64;
        let total_ms = map.tick_to_ms(total_tick as u64) as f64;
        let target_ms = (current_ms + delta_seconds * 1000.0).clamp(0.0, total_ms);
        return map
            .ms_to_tick(target_ms.round().clamp(0.0, u32::MAX as f64) as u32)
            .min(total_tick as u64) as i32;
    }

    let quarter_ms = if fixed_quarter_ms.is_finite() && fixed_quarter_ms > 0.0 {
        fixed_quarter_ms
    } else {
        500.0
    };
    let ticks_per_second = division.max(1) as f64 * 1000.0 / quarter_ms;
    (current_tick as f64 + delta_seconds * ticks_per_second)
        .round()
        .clamp(0.0, total_tick as f64) as i32
}

/// seek 到百分比位置
fn seek_percent(player: *mut c_void, pct: f64) {
    let total = unsafe { fluid_player_get_total_ticks(player) };
    let target = ((total as f64 * pct.clamp(0.0, 1.0)).round() as i32).clamp(0, total);
    unsafe { fluid_player_seek(player, target) };
}
