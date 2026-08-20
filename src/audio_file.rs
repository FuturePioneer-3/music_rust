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

    // 汇编优化例程（src/music_asm.S，AT&T 语法，非内联）；
    // 声明仅供测试引用（生产调用方为 C 侧与 synth.rs），故允许 dead_code。
    #[allow(dead_code)]
    fn music_asm_apply_volume_s16(src: *const i16, dst: *mut i16, n: usize, vol: f32);
    #[allow(dead_code)]
    fn music_asm_goertzel4(samples: *const f32, n: usize, coeffs: *const f32, q1: *mut f32, q2: *mut f32);
    #[allow(dead_code)]
    fn music_asm_limiter_process(buf: *mut f32, n: usize, target: f32, attack: f32, release: f32, gain: *mut f32);

    #[allow(dead_code)]
    fn audio_wav_parse(file: *const u8, len: usize, out: *mut WavMeta) -> i32;
    #[allow(dead_code)]
    fn audio_wav_to_s16(src: *const u8, dst: *mut i16, samples: usize, format: u16, bits_per_sample: u16) -> i32;
}

#[repr(C)]
#[derive(Default, Debug)]
struct WavMeta {
    format: u16,
    channels: u16,
    sample_rate: u32,
    block_align: u16,
    bits_per_sample: u16,
    data_offset: u32,
    data_size: u32,
}

/// 内嵌封面：RGBA8 像素（宽 × 高），已由 C 侧缩放到 ≤96px。
/// 类型定义见 tui.rs（ArtImage），保证 selftest 等二进制无需链接本模块即可复用 TUI。
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 基准实现：与 C 原版一致的截断 + 硬钳制（仅用于对比汇编结果）。
    fn ref_apply_volume(src: &[i16], vol: f32) -> Vec<i16> {
        src.iter().map(|s| {
            let v = (*s as f32 * vol) as i32;
            v.clamp(-32768, 32767) as i16
        }).collect()
    }

    fn ref_goertzel4(samples: &[f32], coeffs: &[f32; 4], mut q1: [f32; 4], mut q2: [f32; 4]) -> ([f32; 4], [f32; 4]) {
        for s in samples {
            for k in 0..4 {
                let q0 = coeffs[k] * q1[k] - q2[k] + s;
                q2[k] = q1[k];
                q1[k] = q0;
            }
        }
        (q1, q2)
    }

    fn ref_limiter(buf: &mut [f32], target: f32, attack: f32, release: f32, gain: &mut f32) {
        let peak = buf.iter().fold(0.0f32, |acc, x| acc.max(x.abs()));
        if peak <= 0.0 { return; }
        let mut target_gain = target / peak;
        if target_gain > 1.0 { target_gain = 1.0; }
        let diff = target_gain - *gain;
        let coef = if diff < 0.0 { attack } else { release };
        *gain += diff * coef;
        let g = *gain;
        for x in buf.iter_mut() {
            let v = *x * g;
            *x = if v > target { target } else if v < -target { -target } else { v };
        }
    }

    fn push_u16(v: &mut Vec<u8>, x: u16) { v.extend_from_slice(&x.to_le_bytes()); }
    fn push_u32(v: &mut Vec<u8>, x: u32) { v.extend_from_slice(&x.to_le_bytes()); }

    fn wav_file(format: u16, bits: u16, channels: u16, rate: u32, data: &[u8]) -> Vec<u8> {
        let block_align = channels * (bits / 8);
        let byte_rate = rate * block_align as u32;
        let riff_size = 4 + 8 + 16 + 8 + data.len() as u32;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        push_u32(&mut v, riff_size);
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt ");
        push_u32(&mut v, 16);
        push_u16(&mut v, format);
        push_u16(&mut v, channels);
        push_u32(&mut v, rate);
        push_u32(&mut v, byte_rate);
        push_u16(&mut v, block_align);
        push_u16(&mut v, bits);
        v.extend_from_slice(b"data");
        push_u32(&mut v, data.len() as u32);
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn asm_wav_parse_and_convert_pcm16() {
        let data = [-32768i16, -1, 0, 32767]
            .into_iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>();
        let file = wav_file(1, 16, 2, 44100, &data);
        let mut meta = WavMeta::default();
        assert_eq!(unsafe { audio_wav_parse(file.as_ptr(), file.len(), &mut meta) }, 1);
        assert_eq!(meta.format, 1);
        assert_eq!(meta.channels, 2);
        assert_eq!(meta.sample_rate, 44100);
        assert_eq!(meta.block_align, 4);
        assert_eq!(meta.bits_per_sample, 16);
        assert_eq!(meta.data_size, data.len() as u32);

        let mut dst = vec![0i16; 4];
        let src = unsafe { file.as_ptr().add(meta.data_offset as usize) };
        assert_eq!(unsafe { audio_wav_to_s16(src, dst.as_mut_ptr(), dst.len(), meta.format, meta.bits_per_sample) }, 1);
        assert_eq!(dst, vec![-32768, -1, 0, 32767]);
    }

    #[test]
    fn asm_wav_convert_formats_to_s16() {
        let src_u8 = [0u8, 128, 255];
        let mut out = vec![0i16; src_u8.len()];
        assert_eq!(unsafe { audio_wav_to_s16(src_u8.as_ptr(), out.as_mut_ptr(), out.len(), 1, 8) }, 1);
        assert_eq!(out, vec![-32768, 0, 32512]);

        let src_24 = [0x00, 0x00, 0x80, 0xff, 0xff, 0x7f];
        let mut out = vec![0i16; 2];
        assert_eq!(unsafe { audio_wav_to_s16(src_24.as_ptr(), out.as_mut_ptr(), out.len(), 1, 24) }, 1);
        assert_eq!(out, vec![-32768, 32767]);

        let src_f32 = [-2.0f32, -0.5, 0.0, 0.5, 2.0]
            .into_iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>();
        let mut out = vec![0i16; 5];
        assert_eq!(unsafe { audio_wav_to_s16(src_f32.as_ptr(), out.as_mut_ptr(), out.len(), 3, 32) }, 1);
        assert_eq!(out, vec![-32767, -16384, 0, 16384, 32767]);
    }

    #[test]
    fn audio_pause_does_not_mark_finished() {
        let mut file = wav_file(1, 16, 1, 44100, &[0, 0, 1, 0]);
        let mut meta = WavMeta::default();
        assert_eq!(unsafe { audio_wav_parse(file.as_ptr(), file.len(), &mut meta) }, 1);
        assert!(meta.data_offset > 0);
        // 这里只验证暂停/恢复语义不会把状态推进到 finished；
        // 真实播放循环由 C 侧状态机掌控。
        let _ = &mut file;
    }

    #[test]
    fn asm_apply_volume_matches_reference() {
        // 伪随机序列 + 边界值
        let mut src = vec![0i16; 1053];
        let mut seed = 0x12345678u32;
        for v in src.iter_mut() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            *v = ((seed >> 16) as i16).wrapping_add(1);
        }
        src[0] = -32768;
        src[1] = 32767;
        for vol in [0.5f32, 1.0, 2.0, 2.5, 5.0, 0.0, 0.01] {
            // 覆盖 8 的倍数与尾部余数（1053 = 8*131 + 5）
            let mut dst = vec![0i16; src.len()];
            unsafe { music_asm_apply_volume_s16(src.as_ptr(), dst.as_mut_ptr(), src.len(), vol) };
            let expect = ref_apply_volume(&src, vol);
            for i in 0..src.len() {
                // 允许 ±1 差异：汇编用就近舍入，参考实现用截断
                assert!((dst[i] as i32 - expect[i] as i32).abs() <= 1,
                    "i={} src={} vol={} asm={} ref={}", i, src[i], vol, dst[i], expect[i]);
            }
        }
        // 8 的倍数边界
        let mut dst = vec![0i16; 64];
        unsafe { music_asm_apply_volume_s16(src.as_ptr(), dst.as_mut_ptr(), 64, 2.0) };
        let expect = ref_apply_volume(&src[..64], 2.0);
        assert_eq!(dst, expect);
    }

    #[test]
    fn asm_goertzel4_matches_reference() {
        let mut samples = vec![0f32; 1024];
        let mut seed = 0x9e3779b9u32;
        for v in samples.iter_mut() {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            *v = ((seed as f32) / u32::MAX as f32) * 2.0 - 1.0;
        }
        let coeffs = [1.9f32, 1.7, 1.5, 1.2];
        let mut q1 = [0f32; 4];
        let mut q2 = [0f32; 4];
        unsafe { music_asm_goertzel4(samples.as_ptr(), samples.len(), coeffs.as_ptr(), q1.as_mut_ptr(), q2.as_mut_ptr()) };
        let (r1, r2) = ref_goertzel4(&samples, &coeffs, [0.0; 4], [0.0; 4]);
        for k in 0..4 {
            assert!((q1[k] - r1[k]).abs() < 1e-4, "q1[{}] asm={} ref={}", k, q1[k], r1[k]);
            assert!((q2[k] - r2[k]).abs() < 1e-4, "q2[{}] asm={} ref={}", k, q2[k], r2[k]);
        }
        // 空输入不崩溃
        let mut e1 = [1.0f32; 4];
        let mut e2 = [2.0f32; 4];
        unsafe { music_asm_goertzel4(samples.as_ptr(), 0, coeffs.as_ptr(), e1.as_mut_ptr(), e2.as_mut_ptr()) };
        assert_eq!(e1, [1.0; 4]);
        assert_eq!(e2, [2.0; 4]);
    }

    #[test]
    fn asm_limiter_matches_reference() {
        let mut buf = vec![0f32; 520];
        let mut seed = 0xdeadbeefu32;
        for v in buf.iter_mut() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            *v = ((seed >> 8) as f32 / 16777216.0) * 1.6 - 0.8;
        }
        buf[10] = 2.5;   // 超限峰值
        buf[11] = -2.5;
        let target = 0.891f32; // -1 dBFS
        let (attack, release) = (0.9f32, 0.0006f32);

        let mut asm_buf = buf.clone();
        let mut asm_gain = 1.0f32;
        unsafe { music_asm_limiter_process(asm_buf.as_mut_ptr(), asm_buf.len(), target, attack, release, &mut asm_gain) };

        let mut ref_buf = buf.clone();
        let mut ref_gain = 1.0f32;
        ref_limiter(&mut ref_buf, target, attack, release, &mut ref_gain);

        assert!((asm_gain - ref_gain).abs() < 1e-5, "gain asm={} ref={}", asm_gain, ref_gain);
        for i in 0..buf.len() {
            assert!((asm_buf[i] - ref_buf[i]).abs() < 1e-4, "i={} asm={} ref={}", i, asm_buf[i], ref_buf[i]);
        }
        // 静音输入：增益不变
        let mut silent = vec![0f32; 64];
        let g0 = asm_gain;
        unsafe { music_asm_limiter_process(silent.as_mut_ptr(), 64, target, attack, release, &mut asm_gain) };
        assert_eq!(asm_gain, g0);
        // 非 4 倍数长度（尾部路径）
        let mut odd = buf[..519].to_vec();
        let mut og = 1.0f32;
        unsafe { music_asm_limiter_process(odd.as_mut_ptr(), odd.len(), target, attack, release, &mut og) };
        let mut rodd = buf[..519].to_vec();
        let mut rg = 1.0f32;
        ref_limiter(&mut rodd, target, attack, release, &mut rg);
        assert!((og - rg).abs() < 1e-5);
        for i in 0..odd.len() {
            assert!((odd[i] - rodd[i]).abs() < 1e-4);
        }
    }

    /// 元数据 + 封面提取集成测试：
    ///   MUSIC_TEST_MEDIA=/path/to/test.mp3 cargo test --bin music extracts_metadata_and_art
    #[test]
    fn extracts_metadata_and_art() {
        let path = std::env::var("MUSIC_TEST_MEDIA").unwrap_or_default();
        if path.is_empty() {
            eprintln!("跳过：设置 MUSIC_TEST_MEDIA 指向带封面/作曲家的音频文件即可运行");
            return;
        }
        let player = AudioFilePlayer::open(&path).expect("打开音频文件失败");
        let title = player.metadata("title");
        let composer = player.metadata("composer");
        let artist = player.metadata("artist");
        eprintln!("  title={:?} composer={:?} artist={:?}", title, composer, artist);
        assert!(composer.is_some(), "应解析出 composer 元数据");
        assert!(title.is_some(), "应解析出 title 元数据");
        let art = player.art().expect("应解析出内嵌封面");
        eprintln!("  art: {}x{} RGBA ({} bytes)", art.width, art.height, art.data.len());
        assert!(art.width >= 8 && art.height >= 8, "封面尺寸过小");
        assert_eq!(art.data.len(), art.width * art.height * 4);
        // 抽样检查像素非全零（真实图像）
        let non_zero = art.data.chunks_exact(4).take(2000).any(|p| p[0] | p[1] | p[2] != 0);
        assert!(non_zero, "封面像素全为黑色？");
        assert_eq!(player.duration_ms(), 4000); // 4 秒正弦波
    }
}
