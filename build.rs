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

//! 构建脚本：自动链接系统 libfluidsynth
//!
//! 优先使用 pkg-config 探测（大多数 Linux 发行版提供 -dev 包）；
//! 若 pkg-config 不可用，则直接按名称链接 fluidsynth。

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_SYSROOT_DIR");

    let has_pkgconfig = Command::new("pkg-config")
        .args(["--exists", "fluidsynth"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if has_pkgconfig {
        // 链接参数
        if let Ok(out) = Command::new("pkg-config")
            .args(["--libs", "fluidsynth"])
            .output()
        {
            for flag in String::from_utf8_lossy(&out.stdout).split_whitespace() {
                if let Some(path) = flag.strip_prefix("-L") {
                    println!("cargo:rustc-link-search=native={}", path);
                } else if let Some(lib) = flag.strip_prefix("-l") {
                    println!("cargo:rustc-link-lib=dylib={}", lib);
                }
            }
        }
        // 兜底：确保按名称链接
        println!("cargo:rustc-link-lib=dylib=fluidsynth");
    } else {
        // 无 pkg-config 的发行版：直接链接
        println!("cargo:rustc-link-lib=dylib=fluidsynth");
        println!("cargo:rustc-link-lib=dylib=m");
    }
}
