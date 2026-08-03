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
//! SoundFont (.sf2/.sf3) 会依次尝试：用户指定路径 → 常见系统路径 → 用户目录。
//!
//! 注：本模块同时被 selftest 通过 `#[path]` 复用，因此部分方法在不同 crate
//! 中可能被标记为 dead_code，这里统一允许。

#![allow(dead_code)]

use std::ffi::{c_char, c_int, c_short, c_uint, c_void, CString};
use std::path::Path;

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

const FLUID_OK: c_int = 0;
const FLUID_FAILED: c_int = -1;

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
struct LimiterState {
    /// 目标峰值电平（线性满刻度，1.0 = 0dBFS）
    target: f32,
    /// 当前增益包络
    current_gain: f32,
    /// 增益平滑系数（越大越快）
    attack_coef: f32,
    release_coef: f32,
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
    #[inline]
    unsafe fn process(&mut self, buf: *mut f32, len: usize) {
        // 第一遍：求峰值
        let mut peak: f32 = 0.0;
        for i in 0..len {
            let a = (*buf.add(i)).abs();
            if a > peak {
                peak = a;
            }
        }
        if peak <= 0.0 {
            return; // 静音
        }

        // 目标增益：只压缩不提升（增益 <= 1.0）
        let mut target_gain = self.target / peak;
        if target_gain > 1.0 {
            target_gain = 1.0;
        }

        // 平滑过渡：压降快（attack），恢复慢（release）
        let diff = target_gain - self.current_gain;
        let coef = if diff < 0.0 {
            self.attack_coef
        } else {
            self.release_coef
        };
        self.current_gain += diff * coef;

        // 第二遍：应用增益 + 硬钳制
        let g = self.current_gain;
        let lim = self.target;
        for i in 0..len {
            let x = *buf.add(i) * g;
            if x > lim {
                *buf.add(i) = lim;
            } else if x < -lim {
                *buf.add(i) = -lim;
            } else {
                *buf.add(i) = x;
            }
        }
    }
}

#[inline]
fn db_to_lin(db: f32) -> f32 {
    (10.0_f32).powf(db / 20.0)
}

/// 全局限制器实例与合成器指针（音频回调使用）
static mut LIMITER: LimiterState = LimiterState {
    target: 0.891,       // -1 dBFS
    current_gain: 1.0,
    attack_coef: 0.9,
    release_coef: 0.0006,
};

static mut LIMITER_SYNTH: *mut fluid_synth_t = std::ptr::null_mut();

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
    // 渲染音频到 out[]
    let ret = fluid_synth_process(synth, len, nfx, fx, nout, out);
    if ret != FLUID_OK {
        return ret;
    }
    // 对每个输出通道应用限制器
    let l = &mut *(&raw mut LIMITER);
    for ch in 0..nout {
        let buf = *out.add(ch as usize);
        if !buf.is_null() {
            l.process(buf, len as usize);
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
    fn fluid_synth_cc(synth: *mut fluid_synth_t, chan: c_int, ctrl: c_int, val: c_int) -> c_int;
    fn fluid_synth_all_notes_off(synth: *mut fluid_synth_t, chan: c_int) -> c_int;
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
}

// ---------------------------------------------------------------------------
// SoundFont 搜索
// ---------------------------------------------------------------------------

const SF2_CANDIDATES: &[&str] = &[
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

// ---------------------------------------------------------------------------
// SynthPlayer
// ---------------------------------------------------------------------------

pub struct SynthPlayer {
    settings: *mut fluid_settings_t,
    synth: *mut fluid_synth_t,
    audio_driver: *mut c_void,
    sequencer: *mut fluid_sequencer_t,
    synth_client: fluid_seq_id_t,
    #[allow(dead_code)]
    sfont_id: c_int,
    #[allow(dead_code)]
    pub soundfont: String,
    #[allow(dead_code)]
    tempo_ms: u32,
    freed: bool,
}

impl SynthPlayer {
    pub fn new(soundfont_path: Option<&str>, tempo_ms: u32, verbose: bool, limit_db: f32) -> Result<Self, String> {
        info("正在初始化 fluidsynth ...".to_string());

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

        // 加载 SoundFont
        let sf = find_soundfont(soundfont_path)
            .ok_or_else(|| "未找到任何 SoundFont (.sf2/.sf3)，请使用 --soundfont 指定路径".to_string())?;
        let sf_c = CString::new(sf.as_str()).unwrap();
        let sfont_id = unsafe { fluid_synth_sfload(synth, sf_c.as_ptr(), 1) };
        if sfont_id < 0 {
            unsafe { delete_fluid_synth(synth); delete_fluid_settings(settings); }
            return Err(format!("加载 SoundFont 失败: {}", sf));
        }
        info(format!("SoundFont 加载成功 (id={}): {}", sfont_id, sf));

        // 设置合成器增益（默认 0.2 太弱，提升到 1.0 让输出达到正常响度，
        // 峰值交给限制器兜底，保证不削波）
        unsafe { fluid_synth_set_gain(synth, 1.0) };

        // 所有通道默认钢琴 (GM Program 0)
        unsafe { fluid_synth_program_reset(synth) };
        for ch in 0..16 {
            unsafe {
                fluid_synth_program_change(synth, ch, 0);
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

        Ok(SynthPlayer {
            settings,
            synth,
            audio_driver,
            sequencer,
            synth_client,
            sfont_id,
            soundfont: sf,
            tempo_ms,
            freed: false,
        })
    }

    /// 调度一个音符事件到绝对时间点（毫秒）。
    /// `on_off`: true=noteon, false=noteoff
    fn schedule_note(&self, channel: c_int, key: u8, vel: u8, at_ms: u32, on: bool) {
        let evt = unsafe { new_fluid_event() };
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
        unsafe { fluid_sequencer_send_at(self.sequencer, evt, at_ms, 1) };
        unsafe { delete_fluid_event(evt) };
    }

    /// 调度一个相对事件：`at_ms` 是相对"当前 tick"的偏移（absolute=0）。
    /// 用于交互模式下动态重排（快进/后退/循环）。
    fn schedule_note_relative(&self, channel: c_int, key: u8, vel: u8, at_ms: u32, on: bool) {
        let evt = unsafe { new_fluid_event() };
        unsafe {
            fluid_event_set_source(evt, 0);
            fluid_event_set_dest(evt, self.synth_client);
            if on {
                fluid_event_noteon(evt, channel, key as c_short, vel as c_short);
            } else {
                fluid_event_noteoff(evt, channel, key as c_short);
            }
        }
        unsafe { fluid_sequencer_send_at(self.sequencer, evt, at_ms, 0) };
        unsafe { delete_fluid_event(evt) };
    }

    /// 清除 sequencer 中所有已排程事件
    pub fn clear_schedule(&self) {
        unsafe {
            fluid_sequencer_remove_events(self.sequencer, -1, -1, -1);
        }
    }

    /// 设置指定通道的乐器（GM Program）
    pub fn set_instrument(&self, channel: c_int, program: u8) {
        let evt = unsafe { new_fluid_event() };
        unsafe {
            fluid_event_set_source(evt, 0);
            fluid_event_set_dest(evt, self.synth_client);
            fluid_event_program_change(evt, channel, program as c_int);
        }
        unsafe { fluid_sequencer_send_now(self.sequencer, evt) };
        unsafe { delete_fluid_event(evt) };
        debug(format!("通道 {} 设置乐器 GM#{}", channel, program));
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
    pub fn play_midi(
        &mut self,
        midi_path: &str,
        bpm_override: Option<f64>,
        show_progress: bool,
        interactive: bool,
        total_ms: u32,
    ) -> Result<(), String> {
        let path_c = CString::new(midi_path)
            .map_err(|_| "MIDI 路径包含非法字符".to_string())?;

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

        let play_ret = unsafe { fluid_player_play(player) };
        if play_ret != FLUID_OK {
            unsafe { delete_fluid_player(player) };
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
        let mut prog = crate::progress::Progress::new(show_progress);
        let mut last_bpm: i32 = 0;
        let mut paused = false;
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
                                unsafe { fluid_player_stop(player) };
                                info("暂停".to_string());
                                prog.finish();
                                paused = true;
                            } else {
                                unsafe { fluid_player_play(player) };
                                info("继续".to_string());
                                paused = false;
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
                            let sign: i32 = if matches!(c, crate::input::Control::SeekForward(_)) {
                                1
                            } else {
                                -1
                            };
                            let ticks = ticks_per_second(player) * s as i32 * sign;
                            seek_relative(player, ticks);
                            info(format!("跳转 {}s", sign as i32 * s as i32));
                        }
                        crate::input::Control::SeekPercent(p) => {
                            seek_percent(player, p);
                            info(format!("跳转到 {}%", (p * 100.0) as i32));
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
            if show_progress {
                let elapsed_ms = std::time::Instant::now();
                // 用 tick 计算更准确（暂停时 tick 不前进）
                let ct = unsafe { fluid_player_get_current_tick(player) };
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

            if status == FLUID_PLAYER_DONE {
                // 暂停时 stop() 也会触发 DONE，但此时 paused=true 不退出
                if !paused {
                    if looping {
                        // set_loop 模式下 DONE 表示循环已结束？这里手动继续
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

        unsafe { delete_fluid_player(player) };
        info("MIDI 播放完成".to_string());
        Ok(())
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
    /// 暂停时清除已排事件并停止排程，恢复时重新排程。
    ///
    /// `events` 必须已按 at_ms 升序排序。
    pub fn play_events_interactive(
        &mut self,
        events: &[crate::parser::ScheduledNote],
        total_ms: u32,
        show_progress: bool,
    ) {
        let mut input = crate::input::InputListener::start();
        let mut prog = crate::progress::Progress::new(show_progress);

        let mut playhead: i64 = 0; // 当前播放位置（毫秒）
        let mut paused = false;
        let mut looping = false;
        let mut quit = false;

        // 排程从 playhead 开始的所有剩余事件。
        // 注：sequencer 的 tick 单调递增且无法重置，因此这里以"当前 tick"为基准，
        //     把 (at_ms - playhead) 的相对偏移换算成绝对 tick（base + rel）。
        fn schedule_from(
            player: &SynthPlayer,
            events: &[crate::parser::ScheduledNote],
            playhead: i64,
        ) -> u32 {
            player.clear_schedule();
            let base = player.now_ms(); // 当前绝对 tick（毫秒）
            for ev in events {
                // 只排未来事件：at_ms >= playhead
                if ev.at_ms as i64 >= playhead {
                    let rel = (ev.at_ms as i64 - playhead).max(0) as u32;
                    let abs = base.wrapping_add(rel);
                    player.schedule_note(ev.channel as i32, ev.key, ev.vel, abs, ev.on);
                }
            }
            base
        }

        // 最近一次排程时的 sequencer tick（基准），从 0 开始排程
        let mut anchor_tick: u32 = schedule_from(self, events, 0);
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
                        paused = !paused;
                        if paused {
                            // 记录暂停时的位置
                            let now = self.now_ms();
                            playhead = playhead + now as i64 - anchor_tick as i64;
                            anchor_tick = now;
                            self.clear_schedule();
                            info("暂停".to_string());
                            prog.finish();
                        } else {
                            // 恢复：从记录的位置重排
                            anchor_tick = schedule_from(self, events, playhead);
                            info("继续".to_string());
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
                        anchor_tick = schedule_from(self, events, playhead);
                        info(format!("跳转至 {:.1}s", playhead as f64 / 1000.0));
                        prog.finish();
                    }
                    crate::input::Control::SeekPercent(p) => {
                        let target = ((total_ms as f64 * p.clamp(0.0, 1.0)).round() as i64)
                            .clamp(0, total_ms as i64);
                        playhead = target;
                        anchor_tick = schedule_from(self, events, playhead);
                        info(format!("跳转至 {}%", (p * 100.0) as i32));
                        prog.finish();
                    }
                }
            }

            if quit {
                break;
            }

            // 暂停时：等待恢复/退出
            if paused {
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

            // 播放结束判定
            if cur >= total_ms as i64 {
                if looping {
                    playhead = 0;
                    anchor_tick = schedule_from(self, events, 0);
                    info("循环播放：从头开始".to_string());
                    prog.finish();
                    continue;
                } else {
                    break;
                }
            }

            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        input.stop();
        prog.finish();
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
        self.schedule_note(ch, key, vel, at_ms, on);
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
        for ch in 0..16 {
            unsafe { fluid_synth_all_notes_off(self.synth, ch) };
        }
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

/// 每秒对应的 tick 数（默认按 500ms/四分音符、PPQ=480 估算）。
/// 用于把秒换算成 tick 进行 seek。实际 tempo 变化时仅近似，够用于快进/后退。
fn ticks_per_second(player: *mut c_void) -> i32 {
    let div = unsafe { fluid_player_get_division(player) };
    if div > 0 {
        // 默认 120 BPM（500ms/四分音符）→ 每秒 2 个四分音符
        div * 2
    } else {
        960
    }
}

/// 相对当前位置 seek（单位 tick），自动钳制在 [0, total]
fn seek_relative(player: *mut c_void, delta_ticks: i32) {
    let cur = unsafe { fluid_player_get_current_tick(player) };
    let total = unsafe { fluid_player_get_total_ticks(player) };
    let target = (cur + delta_ticks).clamp(0, total);
    unsafe { fluid_player_seek(player, target) };
}

/// seek 到百分比位置
fn seek_percent(player: *mut c_void, pct: f64) {
    let total = unsafe { fluid_player_get_total_ticks(player) };
    let target = ((total as f64 * pct.clamp(0.0, 1.0)).round() as i32).clamp(0, total);
    unsafe { fluid_player_seek(player, target) };
}
