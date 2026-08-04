<div align="center">

# 🎹 music_rust

**MIDI numbered-notation (jianpu) piano player — cross-platform, Linux-first, pure Rust**

<div>

<a href="https://github.com/FuturePioneer-3/music_rust/blob/main/README.md">English</a> ｜
<a href="https://github.com/FuturePioneer-3/music_rust/blob/main/README_zh.md">简体中文</a>

</div>

<br>

<div>
<a href="https://img.shields.io/github/v/release/FuturePioneer-3/music_rust"><img src="https://img.shields.io/github/v/release/FuturePioneer-3/music_rust?color=76bad9" alt="release"></a>
<a href="https://img.shields.io/badge/rust-2021-edition-blue.svg"><img src="https://img.shields.io/badge/rust-2021-edition-blue.svg" alt="rust"></a>
<a href="LICENSE"><img src="https://img.shields.io/badge/license-GPLv3-green.svg" alt="license"></a>
<a href="https://img.shields.io/github/license/FuturePioneer-3/music_rust"><img src="https://img.shields.io/github/license/FuturePioneer-3/music_rust?color=green" alt="GPLv3"></a>
</div>

</div>

music_rust is a **piano player** that renders your custom jianpu (numbered musical notation) TXT files, and plays standard MIDI files directly, using the system **libfluidsynth** synthesizer with a piano timbre (GM Program 0).

The original `music_release/` project was built on the Windows API (`winmm.lib`) and Windows-only. This project is fully rewritten in **pure Rust** and talks directly to the system `libfluidsynth` via FFI, supporting most Linux distributions.

## Key Features

1. 💯 **Pure Rust**, no Windows dependencies.
2. 🎹 Plays custom jianpu TXT files with a piano timbre via the system SoundFont.
3. 🎼 **Direct MIDI playback** (`-m` or `.mid` extension): fluidsynth-native multitrack sync + tempo changes, most accurate.
4. 🧵 **Improved multitrack TXT format**: any number of parallel tracks + sequential sections within a track.
5. 📜 Fully compatible with the legacy v1/v2 format (left/right hand dual tracks, blank-line grouping).
6. ⏱️ `fluid_sequencer` millisecond-precise event scheduling.
7. 📊 **Dynamic progress bar**: real-time percentage, elapsed/total time, remaining time.
8. 🎮 **Interactive controls (mpv-like)**: arrow-key seek, space pause, R loop, Q quit — and **`9`/`0` volume adjustment**.
9. 🔇 **Peak limiter (default -1dBFS)**: prevents clipping/buzz from overlapping notes.
10. 🛠️ Debug mode: detailed parse log + every MIDI event.
11. 🐍 Companion Python script: `MIDI → jianpu TXT` converter.

## Quick Start

### AppImage (recommended, works out of the box)

Download `music_rust-*-x86_64.AppImage` from the [Releases](https://github.com/FuturePioneer-3/music_rust/releases) page. It bundles a compact GM SoundFont, no dependencies required.

```bash
chmod +x music_rust-x86_64.AppImage
./music_rust-x86_64.AppImage 乐曲.txt     # play a jianpu TXT
./music_rust-x86_64.AppImage 歌曲.mid     # play a MIDI file
```

The bundled SoundFont is loaded automatically. To use another font: `--soundfont /path/to/xx.sf2`.

### Arch Linux (pkg.tar.zst)

```bash
sudo pacman -U music_rust-2.0.0-1-x86_64.pkg.tar.zst
# pulls in fluidsynth + soundfont-fluid automatically
music 乐曲.txt
```

### Build from Source

```bash
git clone https://github.com/FuturePioneer-3/music_rust
cd music_rust
cargo build --release
./target/release/music 乐曲.txt
```

**Runtime dependencies**: `libfluidsynth.so` + any SoundFont file.

| Distro | Install command |
| ------ | -------- |
| Debian/Ubuntu | `sudo apt install libfluidsynth3 soundfont-fluid` |
| Arch/Manjaro | `sudo pacman -S fluidsynth soundfont-fluid` |
| Fedora | `sudo dnf install fluidsynth fluid-soundfont-gm` |
| openSUSE | `sudo zypper install fluidsynth fluidsynth-soundfont` |
| Gentoo | `sudo emerge media-sound/fluidsynth` |

> Building requires dev headers: `libfluidsynth-dev` (Debian/Ubuntu) / `fluidsynth` (Arch), etc.

## Usage

```bash
# Play a jianpu TXT (auto-discovers system SoundFont)
./target/release/music 乐曲.txt

# Direct MIDI playback (native multitrack + tempo, most accurate)
./target/release/music 歌曲.mid
./target/release/music -m 歌曲.mid

# Debug mode (detailed logs, no progress bar)
./target/release/music 乐曲.txt -d

# Specify a SoundFont
./target/release/music 乐曲.txt --soundfont /path/to/piano.sf2

# Override tempo
./target/release/music 乐曲.txt --bpm 90
./target/release/music 歌曲.mid -b 90
./target/release/music 乐曲.txt --tempo 500   # 500ms per quarter note

# Volume
./target/release/music 乐曲.txt --volume 110
```

> **Two modes**: passing a `.mid`/`.midi` file or using `-m` enters MIDI mode, using
> fluidsynth's built-in player for native multitrack sync and tempo changes.
> Everything else parses the jianpu TXT.

### Command-line Options

| Option | Description |
| ---- | ---- |
| `-d, --debug` | detailed debug output (parse + each MIDI event) |
| `-s, --soundfont <path>` | specify a SoundFont file |
| `-m, --midi <file>` | direct MIDI playback (native multitrack + tempo) |
| `-t, --tempo <ms>` | override tempo (ms per quarter note) |
| `-b, --bpm <n>` | override tempo (BPM) |
| `-v, --volume <0-127>` | volume |
| `-l, --limit <dB>` | peak limiter level (default `-1.0` dBFS, prevents clipping) |
| `-h, --help` | help |

Environment variables:

| Variable | Description |
| ---- | ---- |
| `MUSIC_AUDIO_DRIVER` | force the fluidsynth audio driver, e.g. `alsa` / `pulseaudio` / `pipewire` |

### Interactive Playback Controls

Playback is fully controllable from the keyboard (mpv-like).

| Key | Function |
| --- | --- |
| `←` / `→` | rewind / fast-forward 5s |
| `↑` / `↓` | fast-forward / rewind 10s |
| `PageUp` / `PageDown` | fast-forward / rewind 1 minute |
| `Space` / `P` | pause / resume |
| `[` / `]` | rewind / fast-forward 1s |
| `R` | toggle loop playback |
| `1` – `8` | seek to 10% – 80% |
| `9` / `0` | **decrease / increase volume** |
| `Q` | quit |

> **TXT mode** (jianpu) uses dynamic event rescheduling; seek/rewind/loop/pause are millisecond-precise.
> **MIDI mode** uses the fluidsynth native player (`fluid_player_seek` / `set_loop`) with the same keys.
> Keyboard control is only enabled when stdin is a terminal (automatically disabled for pipes/redirection).

## Automatic Audio-Driver Detection (Anti-Pop)

The program auto-selects the best audio backend, no manual setup required.

1. **PipeWire** (detected via `pipewire-0` socket) — modern Linux desktop first choice
2. **PulseAudio** (detected via `pulse/native` socket) — classic desktop or PipeWire compatibility layer
3. **ALSA** — fallback when no desktop environment

It also automatically **increases the audio buffer** (`audio.period-size=512`, `audio.periods=4`,
≈46ms @44.1kHz) and **raises the synth gain** (`synth.gain=1.0`), which together with the peak
limiter greatly reduces **pops/buzz** caused by scheduling latency.

> If some environments still misbehave, force a switch with `MUSIC_AUDIO_DRIVER=alsa music 乐曲.txt`
> or `MUSIC_AUDIO_DRIVER=pulseaudio music 乐曲.txt`.

## Peak Limiter (Anti-Clipping)

The synth output passes through a **real-time peak limiter** (default `-1dBFS`), avoiding:

- clipping from many simultaneous notes (big chords)
- distortion/buzz when volume is raised
- distortion above full-scale (0dBFS)

Limiter features:
- **compress-only, never boosts**: normal segments keep original dynamics, only peaks above the target are pulled down
- **smooth gain envelope** (fast attack / slow release): no pop on compression, no pumping on recovery
- **hard-clamp fallback**: samples never exceed the target level

Customize the target with `--limit`, e.g. more conservative `-6 dBFS` or more aggressive `-0.5 dBFS`:

```bash
./target/release/music 乐曲.txt --limit -6
```

## TXT Format (Improved Multitrack)

### Header

```
#TITLE 曲名            # optional, song title
#BPM 120              # optional, tempo (BPM); mutually exclusive with legacy plain-number `500` (ms)
```

`#BPM` may also appear **anywhere in the file** to **change tempo mid-song** (from that point onward, applies to all following notes).

### Track Definition

```
T 轨名 | 段1 | 段2 | ...
```

- Each `T`-prefixed line defines a track (**parallel** playback)
- `|`-separated **sections** after the track name play **sequentially** within that track
- `|` also keeps the legacy "barline" meaning (ignored, no rest produced)
- **multiple lines** of the same track also play sequentially (blank lines may separate visual sections)

Example (three parallel tracks: melody + accompaniment + bass):

```
#TITLE 示例曲
#BPM 120
T 旋律(高音) | 1^ 2^ 3^ 4^ | 5^ 6^ 7^ 1^^ | 2^^ 3^^ 2^^ 1^^ | 7^ 6^ 7^ 5^
T 伴奏(低音) | 1, 3, 5, 1 | 4 6 2 5 | 3 1 6 4 | 5 3 2 7,
T 贝斯       | 1,, 0 1,, 0 | 4,, 0 2,, 0 | 3,, 0 6,, 0 | 5,, 0 7,, 0
```

> More than **16 tracks** are supported (channels auto-cycle).

### Note Syntax

| Char | Description | Example |
| ---- | ---- | ---- |
| `1~7` | notes (do re mi fa sol la si) | `1 2 3 4 5` |
| `0` | rest | `0` |
| `,` | lower octave (max 3) | `1,` `2,,` |
| `^` | higher octave (max 4) | `5^` `1^^` |
| `#` | sharp | `4#` `5#^` |
| `-` | sustain (each +1 quarter note) | `5-` `1---` |
| `_` | half duration | `5_` `1__` |
| `.` | dotted (duration ×1.5) | `5.` |
| `*` | duration ÷3 | `5*` |
| `%` | duration ÷5 | `5%` |
| `&` | duration ÷7 | `5&` |
| `[]` | chord (simultaneous) | `[1,3,5,]` |
| `\|` | barline (ignored) | `1 2 \| 3 4` |

Example:

```
5^ 1^^ 2^^_ 6^- | [1^3^5^]- [4^6^1^^] | 0 0 4_,_ | 5^.--- 0
```

### Legacy v1/v2 Compatibility

The original Windows version's two-line-per-group left/right hand format is still supported:

```
500                    # first line: quarter-note milliseconds
1 2 3 4 5 5 4 3 2 1    # right hand
1^- 2^- 3^ 4^- 5^--    # left hand (simultaneous with the line above)

1 1 5 5 6 6 5          # new group
```

- each two-line group = left/right hand simultaneously
- blank lines force a break in pairing
- a plain-number line in between = tempo change

## MIDI → TXT Converter

```bash
python3 convert/convert.py 歌曲.mid              # default output 歌曲.txt
python3 convert/convert.py 歌曲.mid -o 输出.txt  # specify output
python3 convert/convert.py 歌曲.mid --bpm 100    # specify tempo
python3 convert/convert.py 歌曲.mid --track 0,1  # only convert the first two tracks
python3 convert/convert.py 歌曲.mid --legacy     # output legacy two-line format
```

### Converter Options

| Option | Description |
| ---- | ---- |
| `-o, --output` | output file (default same-name `.txt`) |
| `-b, --bpm` | override tempo |
| `--track` | only convert specific tracks (`0,1,2`) |
| `--quantize <tick>` | quantization step (recommend 30 to remove tiny timing jitter) |
| `--max-tracks <n>` | max number of tracks |
| `--drum` | include drum tracks (excluded by default) |
| `--min-vel <n>` | filter low-velocity notes |
| `--keep-empty` | keep empty tracks |
| `--no-chord` | do not merge chords |
| `--legacy` | output legacy v1/v2 format |

> The converter is pure Python standard library, no third-party deps, supports standard MIDI (SMF type 0/1/2).

### Dynamic BPM (Mid-song Tempo Change)

The converter auto-detects **tempo changes** in the MIDI:

- parses all tempo events from the first (conductor) track
- emits a `#BPM xxx` directive line before each tempo change; the Rust player **switches tempo precisely** at that point

```
#BPM 120
# tempo change @tick2400: 90 BPM (666666us/q)

T Trk
1 2 3 4          # 120 BPM
#BPM 90
5 6 7 1^         # 90 BPM
```

`#BPM` may appear **anywhere** (inside or between tracks) and switches tempo **from that point on**:

| Tempo Directive | Description |
| -------- | ---- |
| `#BPM 120` | switch to 120 BPM |
| `90` (legacy plain-number line) | legacy format: switch to 90ms per quarter note |

## Project Comparison

| | Original `music_release` | This project `music_rust` |
| --- | --- | --- |
| Language | C++ | Rust |
| Platform | Windows x64 only | Linux (most distros) |
| Audio backend | Windows MIDI API (`winmm`) | fluidsynth + SoundFont |
| Timbre | system default MIDI timbre | piano (any SoundFont) |
| Tracks | max 2 (left/right hand) | arbitrary multitrack parallel |
| Scheduling | thread + `clock()` busy-wait | fluidsynth sequencer ms-level |
| Playback wait | busy-wait | event-driven wait |
| Interactive control | ✗ | ✅ (arrow keys/pause/loop/quit + volume) |
| Direct MIDI playback | ✗ | ✅ (`-m`, native multitrack + tempo) |

## Debugging

The `-d` flag outputs (the progress bar is auto-hidden in debug mode, so logs and progress never mix):

- parse tempo, track count, notes per track
- the first 20 scheduled events
- precise timestamp of every MIDI event

```bash
./target/release/music 1.txt -d 2>&1 | head -40
```

## License

This project is released under the **GNU General Public License v3.0** (GPLv3, or, at your option, any later version).

**Copyright © 2026 FuturePioneer-3**

- Project homepage: <https://github.com/FuturePioneer-3/music_rust/>
- Full license text: [LICENSE](./LICENSE)

This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

This program is distributed in the hope that it will be useful, but **WITHOUT ANY WARRANTY**; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
