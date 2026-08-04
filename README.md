# music_rust — 钢琴演奏器 / Piano Player

使用 Rust 重写的跨平台（主要面向 Linux）MIDI 简谱播放器。
用钢琴音色（GM Program 0，通过系统 SoundFont）演奏自定义简谱 TXT 文件。

A cross-platform (Linux-first) MIDI numbered-musical-notation (jianpu) player rewritten in Rust.
It plays custom jianpu TXT files with a piano timbre (GM Program 0, via the system SoundFont).

原项目 `music_release/` 基于 Windows API (`winmm.lib`) 开发，仅支持 Windows。
本项目完全用 Rust 重写，通过 FFI 直连系统 **libfluidsynth** 合成器，支持绝大多数 Linux 发行版。

The original project `music_release/` was built on the Windows API (`winmm.lib`) and was Windows-only.
This project is fully rewritten in Rust and talks directly to the system **libfluidsynth** synthesizer
via FFI, supporting most Linux distributions.

## 特性 / Features

- ✅ 纯 Rust 实现，无任何 Windows 依赖 / Pure Rust, no Windows dependencies
- ✅ 通过 `pkg-config` 自动链接系统 libfluidsynth（无需编译期下载）/ Auto-links the system libfluidsynth via `pkg-config` (no build-time downloads)
- ✅ 自动搜索系统 SoundFont（`.sf2`/`.sf3`），也可手动指定 / Auto-discovers system SoundFonts (`.sf2`/`.sf3`), or specify manually
- ✅ **直接播放 MIDI 文件**（`-m` 或 `.mid` 扩展名）：fluidsynth 原生多轨同步 + tempo 变速，最准确 / **Direct MIDI playback** (`-m` or `.mid` extension): fluidsynth-native multitrack sync + tempo changes, most accurate
- ✅ **改进版多音轨 TXT 格式**：支持任意数量并行音轨 + 轨内顺序段落 / **Improved multitrack TXT format**: any number of parallel tracks + sequential sections within a track
- ✅ 完全兼容原版 v1/v2 格式（左右手双轨、空行分组）/ Fully compatible with original v1/v2 format (left/right hand dual tracks, blank-line grouping)
- ✅ `fluid_sequencer` 毫秒级精确事件调度 / millisecond-precise event scheduling via `fluid_sequencer`
- ✅ **动态进度条**：播放时实时显示进度百分比、已播/总时长、剩余时间 / **Dynamic progress bar**: real-time percentage, elapsed/total time, remaining time
- ✅ **交互控制（类似 mpv）**：方向键快进/后退、空格暂停、R 循环、Q 退出 / **Interactive controls (mpv-like)**: arrow-key seek, space pause, R loop, Q quit
- ✅ **峰值限制器（默认 -1dBFS）**：自动防止多音符叠加削波/电流声 / **Peak limiter (default -1dBFS)**: prevents clipping/buzz from overlapping notes
- ✅ 调试模式：详细输出解析日志与每个 MIDI 事件 / Debug mode: detailed parse log + every MIDI event
- ✅ 配套 Python 脚本：`MIDI → 简谱 TXT` 转换器 / Companion Python script: `MIDI → jianpu TXT` converter

## 目录结构 / Directory Structure

```
music_rust/
├── Cargo.toml           # Rust 项目配置 / Rust project config
├── build.rs             # 构建脚本（pkg-config 自动链接 fluidsynth）/ build script (auto-links fluidsynth via pkg-config)
├── src/
│   ├── main.rs          # CLI 入口 / CLI entry point
│   ├── parser.rs        # 简谱 TXT 解析器（多音轨）/ jianpu TXT parser (multitrack)
│   ├── synth.rs         # fluidsynth FFI 封装 + SoundFont 搜索 / fluidsynth FFI wrapper + SoundFont discovery
│   ├── progress.rs      # 动态进度条 / dynamic progress bar
│   ├── input.rs         # 非阻塞键盘输入（交互控制）/ non-blocking keyboard input (interactive controls)
│   └── log.rs           # 日志模块 / logging module
└── convert/
    └── convert.py       # MIDI → 简谱 TXT 转换器（纯 Python，无依赖）/ MIDI → jianpu TXT converter (pure Python, no deps)
```

## 编译 / Compiling

```bash
cd music_rust
cargo build --release
```

**依赖（运行时）**：`libfluidsynth.so` + 任意 SoundFont 文件。
**Dependencies (runtime)**: `libfluidsynth.so` + any SoundFont file.

| 发行版 / Distro | 安装命令 / Install command |
| ------ | -------- |
| Debian/Ubuntu | `sudo apt install libfluidsynth3 soundfont-fluid` |
| Arch/Manjaro | `sudo pacman -S fluidsynth soundfont-fluid` |
| Fedora | `sudo dnf install fluidsynth fluid-soundfont-gm` |
| openSUSE | `sudo zypper install fluidsynth fluidsynth-soundfont` |
| Gentoo | `sudo emerge media-sound/fluidsynth` |

> 编译需要开发头文件：`libfluidsynth-dev`（Debian/Ubuntu）/ `fluidsynth`（Arch）等。
> Building requires dev headers: `libfluidsynth-dev` (Debian/Ubuntu) / `fluidsynth` (Arch), etc.

SoundFont 自动搜索路径包括：`/usr/share/soundfonts/`、`/usr/share/sounds/sf2/`、
`~/.local/share/soundfonts/` 等。若未找到可用 SoundFont，用 `--soundfont` 指定。

Auto-search paths for SoundFont include: `/usr/share/soundfonts/`, `/usr/share/sounds/sf2/`,
`~/.local/share/soundfonts/`, etc. If none is found, specify one with `--soundfont`.

## 使用 / Usage

```bash
# 播放一首简谱 TXT（自动找系统 SoundFont）/ play a jianpu TXT (auto-finds system SoundFont)
./target/release/music 乐曲.txt

# 直接播放 MIDI 文件（fluidsynth 原生多轨+变速，最准确）/ direct MIDI playback (native multitrack + tempo, most accurate)
./target/release/music 歌曲.mid
./target/release/music -m 歌曲.mid          # 或显式指定 -m / or explicitly use -m

# 调试模式（详细日志，无进度条）/ debug mode (detailed logs, no progress bar)
./target/release/music 乐曲.txt -d

# 指定 SoundFont / specify SoundFont
./target/release/music 乐曲.txt --soundfont /path/to/piano.sf2

# 覆盖速度 / override tempo
./target/release/music 乐曲.txt --bpm 90
./target/release/music 乐曲.mid -b 90       # MIDI 也支持速度覆盖 / MIDI also supports tempo override
./target/release/music 乐曲.txt --tempo 500   # 500ms/四分音符 / 500ms per quarter note

# 音量 / volume
./target/release/music 乐曲.txt --volume 110
```

> **两种模式 / Two modes**：传入 `.mid`/`.midi` 文件或使用 `-m` 参数即进入 MIDI 模式，
> 用 fluidsynth 内置播放器原生处理多轨同步与 tempo 变化（最准确，推荐）。
> 其余情况解析简谱 TXT。
>
> Passing a `.mid`/`.midi` file or using `-m` enters MIDI mode, using fluidsynth's built-in
> player for native multitrack sync and tempo changes (most accurate, recommended).
> Everything else parses jianpu TXT.

### 命令行参数 / Command-line Options

| 参数 / Option | 说明 / Description |
| ---- | ---- |
| `-d, --debug` | 详细调试输出（解析 + 每个 MIDI 事件）/ detailed debug output (parse + each MIDI event) |
| `-s, --soundfont <路径>` | 指定 SoundFont 文件 / specify a SoundFont file |
| `-m, --midi <file>` | 直接播放 MIDI 文件（fluidsynth 原生多轨+变速）/ direct MIDI playback (native multitrack + tempo) |
| `-t, --tempo <ms>` | 覆盖速度（毫秒/四分音符）/ override tempo (ms per quarter note) |
| `-b, --bpm <n>` | 覆盖速度（BPM）/ override tempo (BPM) |
| `-v, --volume <0-127>` | 音量 / volume |
| `-l, --limit <dB>` | 峰值限制电平（默认 `-1.0` dBFS，防止削波）/ peak limiter level (default `-1.0` dBFS, prevents clipping) |
| `-h, --help` | 帮助 / help |

环境变量 / Environment variables:

| 变量 / Variable | 说明 / Description |
| ---- | ---- |
| `MUSIC_AUDIO_DRIVER` | 强制 fluidsynth 音频驱动，如 `alsa` / `pulseaudio` / `pipewire` / force the fluidsynth audio driver, e.g. `alsa` / `pulseaudio` / `pipewire` |

### 播放控制（交互式） / Interactive Playback Controls

播放过程中可直接用键盘控制（类似 mpv）。
During playback you can control the player directly with the keyboard (mpv-like).

| 按键 / Key | 功能 / Function |
| --- | --- |
| `←` / `→` | 后退 / 快进 5 秒 / rewind / fast-forward 5s |
| `↑` / `↓` | 快进 / 后退 10 秒 / fast-forward / rewind 10s |
| `PageUp` / `PageDown` | 快进 / 后退 1 分钟 / fast-forward / rewind 1 minute |
| `空格` / `P` | 暂停 / 继续 / pause / resume |
| `[` / `]` | 后退 / 快进 1 秒 / rewind / fast-forward 1s |
| `R` | 切换循环播放 / toggle loop playback |
| `1` – `8` | 跳转到 10% – 80% 进度 / seek to 10% – 80% |
| `9` / `0` | 降低 / 增加音量 / decrease / increase volume |
| `Q` | 退出 / quit |

> **TXT 模式**（简谱）基于事件动态重排，快进/后退/循环/暂停都精确到毫秒；
> **MIDI 模式**使用 fluidsynth 原生播放器（`fluid_player_seek` / `set_loop`），同样支持上述按键。
> 键盘控制仅在 stdin 为终端时启用（管道/重定向输入自动禁用，不影响自动化）。
>
> **TXT mode** (jianpu) uses dynamic event rescheduling; seek/rewind/loop/pause are millisecond-precise.
> **MIDI mode** uses the fluidsynth native player (`fluid_player_seek` / `set_loop`) with the same keys.
> Keyboard control is only enabled when stdin is a terminal (automatically disabled for pipes/redirection).

### 音频驱动自动探测（防爆音） / Automatic Audio-Driver Detection (Anti-Pop)

程序自动选择最优音频后端，无需手动配置。
The program auto-selects the best audio backend, no manual setup required.

1. **PipeWire**（检测到 `pipewire-0` socket）—— 现代 Linux 桌面首选 / modern Linux desktop first choice
2. **PulseAudio**（检测到 `pulse/native` socket）—— 经典桌面或 PipeWire 兼容层 / classic desktop or PipeWire compatibility layer
3. **ALSA** —— 无桌面环境的回退 / fallback when no desktop environment

同时自动**增大音频缓冲区**（`audio.period-size=512`、`audio.periods=4`，
约 46ms @44.1kHz），并**提升合成器增益**（`synth.gain=1.0`），
配合峰值限制器，显著减少调度延迟导致的**爆音/电流声**。

It also automatically **increases the audio buffer** (`audio.period-size=512`, `audio.periods=4`,
≈46ms @44.1kHz) and **raises the synth gain** (`synth.gain=1.0`), which together with the peak
limiter greatly reduces **pops/buzz** caused by scheduling latency.

> 若仍有个别环境异常，可用 `MUSIC_AUDIO_DRIVER=alsa music 乐曲.txt` 强制切换。
> If some environments still misbehave, force a switch with `MUSIC_AUDIO_DRIVER=alsa music 乐曲.txt`.

> 若播放无声，可尝试 `MUSIC_AUDIO_DRIVER=alsa music 乐曲.txt` 或
> `MUSIC_AUDIO_DRIVER=pulseaudio music 乐曲.txt` 切换音频后端。
> If there is no sound, try switching the audio backend with
> `MUSIC_AUDIO_DRIVER=alsa music 乐曲.txt` or `MUSIC_AUDIO_DRIVER=pulseaudio music 乐曲.txt`.

## 峰值限制器（防削波） / Peak Limiter (Anti-Clipping)

合成器输出经过一个**实时峰值限制器**（默认限制到 `-1dBFS`），彻底避免：
The synth output passes through a **real-time peak limiter** (default `-1dBFS`), avoiding:

- 大量音符同时发声（大和弦）导致的削波 / clipping from many simultaneous notes (big chords)
- 音量调大后的破音/电流声 / distortion/buzz when volume is raised
- 超出音频满刻度（0dBFS）的失真 / distortion above full-scale (0dBFS)

限制器特性 / Limiter features:
- **只压缩不放大**：正常音量段保持原始动态，仅当峰值超过目标电平时自动压下来 / **compress-only, never boosts**: normal segments keep original dynamics, only peaks above the target are pulled down
- **平滑增益包络**（快 attack / 慢 release）：压降不爆音，恢复无泵浦感 / **smooth gain envelope** (fast attack / slow release): no pop on compression, no pumping on recovery
- **硬钳制兜底**：任何时刻样本都不会超过目标电平 / **hard-clamp fallback**: samples never exceed the target level

通过 `--limit` 可自定义目标电平，例如更保守的 `-6 dBFS` 或更激进的 `-0.5 dBFS`：
Customize the target with `--limit`, e.g. more conservative `-6 dBFS` or more aggressive `-0.5 dBFS`:

```bash
./target/release/music 乐曲.txt --limit -6
```

## TXT 格式（改进版多音轨） / TXT Format (Improved Multitrack)

### 头部 / Header

```
#TITLE 曲名            # 可选，乐曲标题 / optional, song title
#BPM 120              # 可选，速度（BPM）；与旧版纯数字 `500`（毫秒）二选一 / optional, tempo (BPM); mutually exclusive with legacy plain-number `500` (ms)
```

`#BPM` 指令也可出现在**文件任意位置**，表示**中途变速**（从该点起切换速度，作用于之后的所有音符）。
`#BPM` may also appear **anywhere in the file** to **change tempo mid-song** (from that point onward, applies to all following notes).

### 音轨定义 / Track Definition

```
T 轨名 | 段1 | 段2 | ...
```

- 每个 `T` 开头的行定义一个音轨（**并行**播放）/ each `T`-prefixed line defines a track (**parallel**)
- 轨名后面的 `|` 分隔的**段落**在该轨内**顺序**播放 / `|`-separated **sections** after the track name play **sequentially** within that track
- 竖线 `|` 同时兼容原版"小节线"语义（被忽略，不产生休止）/ `|` also keeps the legacy "barline" meaning (ignored, no rest produced)
- 同一轨的**多行**也按顺序播放（可用空行分隔视觉段落）/ **multiple lines** of the same track also play sequentially (blank lines may separate visual sections)

示例（三轨并行：旋律 + 伴奏 + 贝斯）/ Example (three parallel tracks: melody + accompaniment + bass):

```
#TITLE 示例曲
#BPM 120
T 旋律(高音) | 1^ 2^ 3^ 4^ | 5^ 6^ 7^ 1^^ | 2^^ 3^^ 2^^ 1^^ | 7^ 6^ 7^ 5^
T 伴奏(低音) | 1, 3, 5, 1 | 4 6 2 5 | 3 1 6 4 | 5 3 2 7,
T 贝斯       | 1,, 0 1,, 0 | 4,, 0 2,, 0 | 3,, 0 6,, 0 | 5,, 0 7,, 0
```

> 支持**超过 16 条音轨**（通道自动循环映射）。
> More than **16 tracks** are supported (channels auto-cycle).

### 音符语法 / Note Syntax

| 字符 / Char | 说明 / Description | 示例 / Example |
| ---- | ---- | ---- |
| `1~7` | 音符（do re mi fa sol la si）/ notes | `1 2 3 4 5` |
| `0` | 休止符 / rest | `0` |
| `,` | 低音（最多 3 个）/ lower octave (max 3) | `1,` `2,,` |
| `^` | 高音（最多 4 个）/ higher octave (max 4) | `5^` `1^^` |
| `#` | 升半音 / sharp | `4#` `5#^` |
| `-` | 延音（每个 +1 个四分音符）/ sustain (each +1 quarter note) | `5-` `1---` |
| `_` | 分音（时值减半）/ half duration | `5_` `1__` |
| `.` | 附点（时值 ×1.5）/ dotted (duration ×1.5) | `5.` |
| `*` | 时值 ÷3 / duration ÷3 | `5*` |
| `%` | 时值 ÷5 / duration ÷5 | `5%` |
| `&` | 时值 ÷7 / duration ÷7 | `5&` |
| `[]` | 和弦（同时发声）/ chord (simultaneous) | `[1,3,5,]` |
| `\|` | 小节线（忽略）/ barline (ignored) | `1 2 \| 3 4` |

示例 / Example:

```
5^ 1^^ 2^^_ 6^- | [1^3^5^]- [4^6^1^^] | 0 0 4_,_ | 5^.--- 0
```

### 旧版 v1/v2 兼容 / Legacy v1/v2 Compatibility

原 Windows 版的两行一组左右手格式仍然支持：
The original Windows version's two-line-per-group left/right hand format is still supported:

```
500                    # 第一行：四分音符毫秒 / first line: quarter-note milliseconds
1 2 3 4 5 5 4 3 2 1    # 右手 / right hand
1^- 2^- 3^ 4^- 5^--    # 左手（与上行同时）/ left hand (simultaneous with the line above)

1 1 5 5 6 6 5          # 新一组 / new group
```

- 每两行一组 = 左右手同时播放 / each two-line group = left/right hand simultaneously
- 空行强制打断配对 / blank lines force a break in pairing
- 中间出现纯数字行 = 切换速度 / a plain-number line in between = tempo change

## MIDI → TXT 转换器 / MIDI → TXT Converter

```bash
python3 convert/convert.py 歌曲.mid              # 默认输出 歌曲.txt / default output 歌曲.txt
python3 convert/convert.py 歌曲.mid -o 输出.txt  # 指定输出 / specify output
python3 convert/convert.py 歌曲.mid --bpm 100    # 指定速度 / specify tempo
python3 convert/convert.py 歌曲.mid --track 0,1  # 只转换前两轨 / only convert the first two tracks
python3 convert/convert.py 歌曲.mid --legacy     # 输出旧版两行一组格式 / output legacy two-line format
```

转换完成后 / After converting:

```bash
./target/release/music 输出.txt -d
```

### 转换器参数 / Converter Options

| 参数 / Option | 说明 / Description |
| ---- | ---- |
| `-o, --output` | 输出文件（默认同名 `.txt`）/ output file (default same-name `.txt`) |
| `-b, --bpm` | 覆盖速度 / override tempo |
| `--track` | 只转换指定音轨（`0,1,2`）/ only convert specific tracks |
| `--quantize <tick>` | 量化粒度（建议 30，消除微小时值抖动）/ quantization step (recommend 30 to remove tiny timing jitter) |
| `--max-tracks <n>` | 最多音轨数 / max number of tracks |
| `--drum` | 包含打击乐轨（默认排除）/ include drum tracks (excluded by default) |
| `--min-vel <n>` | 过滤低力度音符 / filter low-velocity notes |
| `--keep-empty` | 保留空白音轨 / keep empty tracks |
| `--no-chord` | 不合并和弦 / do not merge chords |
| `--legacy` | 输出旧版 v1/v2 格式 / output legacy v1/v2 format |

> 转换器纯 Python 标准库实现，无第三方依赖，支持标准 MIDI (SMF type 0/1/2)。
> The converter is pure Python standard library, no third-party deps, supports standard MIDI (SMF type 0/1/2).

### 动态 BPM（中途变速） / Dynamic BPM (Mid-song Tempo Change)

转换器自动探测 MIDI 中的**速度变化**（tempo change）：
The converter auto-detects **tempo changes** in the MIDI:

- 从第一个轨道（conductor track）解析所有 tempo 事件 / parse all tempo events from the first (conductor) track
- 变速点前输出 `#BPM xxx` 指令行，Rust 播放器在对应位置**精确切换速度** / emit a `#BPM xxx` directive line before each tempo change; the Rust player **switches tempo precisely** at that point

```
#BPM 120
# tempo change @tick2400: 90 BPM (666666us/q)

T Trk
1 2 3 4          # 120 BPM
#BPM 90
5 6 7 1^         # 90 BPM
```

`#BPM` 指令可出现在**任意位置**（轨内、轨间），作用为**从该点起切换速度**：
`#BPM` may appear **anywhere** (inside or between tracks) and switches tempo **from that point on**:

```
#BPM 120
T 旋律 | 1 2 3 4
#BPM 60
T 伴奏 | 5 6 7 1
```

| 速度指令 / Tempo Directive | 说明 / Description |
| -------- | ---- |
| `#BPM 120` | 切换到 120 BPM / switch to 120 BPM |
| `90`（旧版纯数字行）/ (legacy plain-number line) | 旧版格式：切换到 90ms/四分音符 / legacy format: switch to 90ms per quarter note |

## 项目对比 / Project Comparison

| | 原版 `music_release` | 本项目 `music_rust` |
| --- | --- | --- |
| 语言 / Language | C++ | Rust |
| 平台 / Platform | 仅 Windows x64 / Windows x64 only | Linux（兼容大多数发行版）/ Linux (most distros) |
| 音频后端 / Audio backend | Windows MIDI API (`winmm`) | fluidsynth + SoundFont |
| 音色 / Timbre | 系统默认 MIDI 音色 / system default MIDI timbre | 钢琴（可换任何 SoundFont）/ piano (any SoundFont) |
| 音轨 / Tracks | 最多 2 轨（左右手）/ max 2 (left/right) | 任意多轨并行 / arbitrary multitrack parallel |
| 调度精度 / Scheduling | 线程 + `clock()` 忙等 / thread + busy-wait | fluidsynth sequencer 毫秒级 / ms-level |
| 播放等待 / Wait | 忙等 / busy-wait | 事件驱动等待 / event-driven wait |
| 交互控制 / Interactive | ✗ | ✅（方向键/暂停/循环/退出）/ arrow keys/pause/loop/quit |
| MIDI 直接播放 / Direct MIDI | ✗ | ✅（`-m`，原生多轨+变速）/ (`-m`, native multitrack + tempo) |

## 调试 / Debugging

`-d` 参数会输出（调试模式下进度条自动隐藏，日志与进度不混排）：
The `-d` flag outputs (the progress bar is auto-hidden in debug mode, so logs and progress never mix):

- 解析速度、音轨数量、每轨音符数 / parse tempo, track count, notes per track
- 前 20 条排程事件 / the first 20 scheduled events
- 每个 MIDI 事件的精确时间戳（channel / note-on/off / key / velocity）/ precise timestamp of every MIDI event

```bash
./target/release/music 1.txt -d 2>&1 | head -40
```

## 许可证 / License

本项目基于 **GNU General Public License v3.0**（GPLv3，或后续版本）发布。
This project is released under the **GNU General Public License v3.0** (GPLv3, or, at your option, any later version).

**Copyright © 2026 FuturePioneer-3**

- 项目主页 / Project homepage: <https://github.com/FuturePioneer-3/music_rust/>
- 完整许可文本见 / Full license text: [LICENSE](./LICENSE)

本程序是自由软件：您可以在自由软件基金会发布的
GNU 通用公共许可证（第 3 版，或按您的选择更高版本）条款下
重新分发和/或修改它。

This program is free software: you can redistribute it and/or modify it
under the terms of the GNU General Public License as published by the
Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

本程序按"原样"分发，希望它有用，但**不提供任何担保**；
甚至不提供适销性或特定用途适用性的暗示担保。
详情请参阅 GNU 通用公共许可证。

This program is distributed in the hope that it will be useful, but **WITHOUT ANY WARRANTY**;
without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
See the GNU General Public License for more details.
