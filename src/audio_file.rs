// music_rust —— 音频文件播放 Rust 控制层
// Copyright (C) 2026 FuturePioneer-3
// SPDX-License-Identifier: GPL-3.0-or-later

use std::ffi::{c_char, c_int, CStr, CString};

use crate::tui::ArtImage;

#[repr(C)]
struct RawAudio { _private: [u8; 0] }

extern "C" {
    fn music_audio_open(path: *const c_char, error: *mut c_char, error_len: i32) -> *mut RawAudio;
    fn music_audio_play(player: *mut RawAudio) -> i32;
    fn music_audio_pause(player: *mut RawAudio) -> i32;
    fn music_audio_seek(player: *mut RawAudio, position_ms: i64) -> i32;
    fn music_audio_position_ms(player: *mut RawAudio) -> i64;
    fn music_audio_duration_ms(player: *mut RawAudio) -> i64;
    fn music_audio_finished(player: *mut RawAudio) -> i32;
    fn music_audio_set_volume(player: *mut RawAudio, volume: f32);
    fn music_audio_volume(player: *mut RawAudio) -> f32;
    fn music_audio_spectrum(player: *mut RawAudio, levels: *mut u8);
    fn music_audio_metadata(player: *mut RawAudio, key: *const c_char) -> *const c_char;
    fn music_audio_art(player: *mut RawAudio, data: *mut *const u8, width: *mut c_int, height: *mut c_int) -> i32;
    fn music_audio_close(player: *mut RawAudio);
}

pub struct AudioFilePlayer { raw: *mut RawAudio }
unsafe impl Send for AudioFilePlayer {}

impl AudioFilePlayer {
    pub fn open(path: &str) -> Result<Self, String> {
        let path = CString::new(path).map_err(|_| "音频路径包含非法字符".to_string())?;
        let mut error = vec![0i8; 256];
        let raw = unsafe { music_audio_open(path.as_ptr(), error.as_mut_ptr(), error.len() as i32) };
        if raw.is_null() { return Err(String::from_utf8_lossy(&error.iter().map(|v| *v as u8).take_while(|v| *v != 0).collect::<Vec<_>>()).into_owned()); }
        Ok(Self { raw })
    }
    pub fn play(&mut self) { unsafe { music_audio_play(self.raw); } }
    pub fn pause(&mut self) { unsafe { music_audio_pause(self.raw); } }
    pub fn seek(&mut self, ms: i64) { unsafe { music_audio_seek(self.raw, ms); } }
    pub fn position_ms(&self) -> u64 { unsafe { music_audio_position_ms(self.raw).max(0) as u64 } }
    pub fn duration_ms(&self) -> u64 { unsafe { music_audio_duration_ms(self.raw).max(0) as u64 } }
    pub fn finished(&self) -> bool { unsafe { music_audio_finished(self.raw) != 0 } }
    pub fn set_volume_percent(&mut self, percent: u32) { unsafe { music_audio_set_volume(self.raw, percent.clamp(0, 500) as f32 / 100.0); } }
    pub fn volume_percent(&self) -> u32 { unsafe { (music_audio_volume(self.raw) * 100.0).round() as u32 } }
    pub fn spectrum(&self) -> [u8; 16] {
        let mut levels = [0; 16];
        unsafe { music_audio_spectrum(self.raw, levels.as_mut_ptr()); }
        levels
    }

    /// 读取元数据字段（title/artist/album/composer/date/genre），不存在返回 None。
    pub fn metadata(&self, key: &str) -> Option<String> {
        let key = CString::new(key).ok()?;
        let ptr = unsafe { music_audio_metadata(self.raw, key.as_ptr()) };
        if ptr.is_null() {
            return None;
        }
        let s = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
        if s.is_empty() { None } else { Some(s) }
    }

    /// 读取内嵌封面（若有）。
    pub fn art(&self) -> Option<ArtImage> {
        let mut data: *const u8 = std::ptr::null();
        let mut width: c_int = 0;
        let mut height: c_int = 0;
        let present = unsafe { music_audio_art(self.raw, &mut data, &mut width, &mut height) };
        if present == 0 || data.is_null() || width <= 0 || height <= 0 {
            return None;
        }
        let len = (width as usize) * (height as usize) * 4;
        let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
        Some(ArtImage { data: bytes, width: width as usize, height: height as usize })
    }
}

impl Drop for AudioFilePlayer { fn drop(&mut self) { unsafe { music_audio_close(self.raw); } } }
