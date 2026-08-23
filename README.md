<div align="center">

# 🎹 music_rust

**Terminal music player: jianpu (numbered-notation) scores, MIDI and audio files — Linux-first, Rust core + C/assembly audio layer**

<div>

<a href="https://github.com/FuturePioneer-3/music_rust/blob/main/README.md">English</a> ｜
<a href="https://github.com/FuturePioneer-3/music_rust/blob/main/README_zh.md">简体中文</a>

</div>

<br>

<div>
<a href="https://img.shields.io/github/v/release/FuturePioneer-3/music_rust"><img src="https://img.shields.io/github/v/release/FuturePioneer-3/music_rust?color=76bad9" alt="release"></a>
<a href="https://img.shields.io/badge/rust-2021-edition-blue.svg"><img src="https://img.shields.io/badge/rust-2021-edition-blue.svg" alt="rust"></a>
<a href="https://img.shields.io/github/license/FuturePioneer-3/music_rust"><img src="https://img.shields.io/github/license/FuturePioneer-3/music_rust?color=green" alt="GPLv3"></a>
</div>

</div>

music_rust is a **terminal music player** with three playback engines:

| Mode | Input | Engine |
| ---- | ----- | ------ |
| 🎼 **Score mode** | jianpu TXT (v3 / v2 / legacy v1) | system **libfluidsynth** + SoundFont, millisecond-precise sequencer |
| 🎵 **MIDI mode** | `.mid` / `.midi` | fluidsynth native player (multitrack sync + tempo map, most accurate) |
| 📁 **Audio file mode** | WAV / MP3 / FLAC / OGG / Opus / AAC / M4A / WMA | C/FFmpeg decoder + ALSA output, album art & metadata rendering |

The original `music_release/` project was built on the Windows API (`winmm.lib`) and Windows-only.
This project is fully rewritten in **Rust** and talks directly to the system `libfluidsynth` via FFI,
supporting most Linux distributions. Run it with **no arguments** to open an interactive startup
selector; run it with a file to start playing immediately.

## What's New in v3.20

- 🧭 **No-argument startup selector**: browse playable files from the current directory, pick a
  SoundFont, and choose the GM program (0–127) used for jianpu playback — then hand off to the TUI.
- 🎛️ **Bundled `electronic_synth.sf2` synthesizer SoundFont**: shipped in the project root and in
  the Arch package (`/usr/share/music_rust/soundfonts/`), auto-discovered at startup.
- 🎚️ New `-i, --instrument <0-127>` option selects any GM program for score playback
  (default 0 = Acoustic Grand Piano; e.g. 81 = Saw Lead).
- 🔧 Fixed volume-state vs. synth-gain mismatch in MIDI/TXT modes and limiter-envelope loudness lag.
- 🔧 Fixed audio-file pause/seek thread deadlock, EOF race and TUI progress-bar row misalignment.

Recent milestones: **v3.1** added an all-assembly WAV fast path (plain WAV files never touch
FFmpeg); **v3.0** introduced the absolute-tick TXT v3 format with a global tempo map.

## Key Features

1. 💯 **Pure Rust** core, no Windows dependencies; audio hot paths in C and hand-written x86-64 SSE2 assembly.
2. 🧭 **Startup selector** (no arguments): file browser, SoundFont picker, GM program input; `Q`/`Esc` cancels.
3. 🎹 Plays custom jianpu TXT with any SoundFont timbre; GM instrument selectable via `--instrument`.
4. 🎼 **Direct MIDI playback** (`-m` or `.mid` extension): fluidsynth-native multitrack sync + tempo changes.
5. 📄 **TXT v3 format**: absolute tick events + one global tempo map; line breaks/rests/order never shift timing, original MIDI track IDs preserved.
6. 📜 Fully compatible with legacy formats: v2 multi-track `T` lines and the original v1 two-line left/right-hand groups.
7. ⏱️ `fluid_sequencer` millisecond-precise event scheduling; seek/loop/pause exact to the millisecond.
8. 📊 **Dynamic progress bar**: percentage, elapsed/total time, remaining time.
9. 🎮 **Interactive controls (mpv-like)**: arrow-key seek, space pause, R loop, Q quit, `9`/`0` volume, mouse click on the progress bar.
10. 🖥️ **Fullscreen TUI**: rounded borders + truecolor palette, smooth 1/8-step progress bar, gradient dynamic-EQ spectrum with aligned band labels, colored volume bar and status indicators; **real-tty support** (Linux console/serial auto-degrades to 16-color SGR + ASCII borders + incremental redraws, no flicker).
11. 🖼️ **Album art + metadata**: embedded covers (ID3 APIC / FLAC PICTURE / M4A covr) decoded and scaled via FFmpeg, rendered with half-block characters alongside composer/artist/album/date-genre info.
12. ⚡ **WAV fast path** (`src/audio_wav.S`): RIFF chunk walk + PCM conversion in assembly — plain WAV files bypass FFmpeg entirely.
13. ⚙️ **SSE2 assembly audio core** (`src/audio_dsp.S`, `src/music_asm.S`): per-sample volume ramping + saturated clamping (no pops on volume/pause/seek), peak limiting and the 16-band Goertzel spectrum (4 lanes in parallel).
14. 🔇 **Peak limiter (default -1dBFS)**: prevents clipping/buzz from overlapping notes.
15. 📜 **Custom colored leveled logger** (`log.rs`): TRACE/DEBUG/INFO/WARN/ERROR with colors, millisecond timestamps and a 300-entry ring buffer.
16. 🐍 Companion pure-Python converter: `MIDI → jianpu TXT` (v3 by default).

## Quick Start

All binaries are distributed as **release assets** — download them from the
[Releases](https://github.com/FuturePioneer-3/music_rust/releases) page
(the git repository contains source only).

### AppImage (recommended, works out of the box)

Download `music_rust-<version>-x86_64.AppImage` (and its `.sha256`) from the latest release.
It bundles a compact GM SoundFont, no dependencies required.

```bash
chmod +x music_rust-*-x86_64.AppImage
./music_rust-*-x86_64.AppImage            # open the startup selector
./music_rust-*-x86_64.AppImage 乐曲.txt    # play a jianpu TXT
./music_rust-*-x86_64.AppImage 歌曲.mid    # play a MIDI file
```

The bundled SoundFont is loaded automatically. To use another font: `--soundfont /path/to/xx.sf2`.

### Arch Linux (pkg.tar.zst)

Download `music_rust-<version>-1-x86_64.pkg.tar.zst` from the latest release, then:

```bash
sudo pacman -U music_rust-3.20-1-x86_64.pkg.tar.zst
# bundles the electronic-synth SoundFont and pulls in FluidSynth automatically
music 乐曲.txt
music                # or open the startup selector
```

### Build from Source

```bash
git clone https://github.com/FuturePioneer-3/music_rust
cd music_rust
cargo build --release
./target/release/music 乐曲.txt
```

**Runtime dependencies**: `libfluidsynth.so` + any SoundFont file for scores/MIDI;
FFmpeg libraries (`libavformat`/`libavcodec`/`libavutil`/`libswresample`) + ALSA (`libasound`)
for audio files.

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
# Open the startup selector (file, SoundFont, GM program)
./target/release/music

# Play a jianpu TXT (auto-discovers a SoundFont)
./target/release/music 乐曲.txt

# Direct MIDI playback (native multitrack + tempo, most accurate)
./target/release/music 歌曲.mid
./target/release/music -m 歌曲.mid

# Play an audio file (album art + metadata in the TUI)
./target/release/music song.flac

# Debug mode (detailed logs, no TUI)
./target/release/music 乐曲.txt -d

# Specify a SoundFont
./target/release/music 乐曲.txt --soundfont /path/to/piano.sf2

# Use the bundled electronic-synth SoundFont with Saw Lead (GM 81)
./target/release/music 乐曲.txt --soundfont ./electronic_synth.sf2 --instrument 81

# After installing the Arch package, the bundled SoundFont is found automatically
music 乐曲.txt --instrument 81

# Override tempo
./target/release/music 乐曲.txt --bpm 90
./target/release/music 歌曲.mid -b 90
./target/release/music 乐曲.txt --tempo 500   # 500ms per quarter note

# Volume
./target/release/music 乐曲.txt --volume 110
```

**SoundFont discovery order**: `--soundfont` path → packaged font
(`/usr/share/music_rust/soundfonts/electronic_synth.sf2`) → `./electronic_synth.sf2` →
common system paths (FluidR3, GeneralUser, MS Basic, …) → `~/.local/share/soundfonts`.
The startup selector lists every candidate it finds and lets you pick one.

**Mode selection**: `.mid`/`.midi` (or `-m`) → MIDI mode; `.wav`/`.mp3`/`.flac`/`.ogg`/`.opus`/
`.aac`/`.m4a`/`.wma` → audio file mode; anything else → jianpu score mode. The TUI title shows
the active mode.

### Command-line Options

| Option | Description |
| ---- | ---- |
| `-d, --debug` | detailed debug output (parse + each MIDI event) |
| `-s, --soundfont <path>` | specify a SoundFont (.sf2/.sf3) file |
| `-i, --instrument <0-127>` | GM program for jianpu playback (default 0; e.g. 81 = Saw Lead) |
| `-m, --midi <file>` | direct MIDI playback (native multitrack + tempo) |
| `-t, --tempo <ms>` | override tempo (ms per quarter note) |
| `-b, --bpm <n>` | override tempo (BPM) |
| `-v, --volume <0-500>` | volume (default 80%; 0% is mute) |
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

Mouse: click the progress bar to seek, click the status row to pause/resume.

> **Score mode** (jianpu) uses dynamic event rescheduling; seek/rewind/loop/pause are millisecond-precise.
> **MIDI mode** uses the fluidsynth native player (`fluid_player_seek` / `set_loop`) with the same keys.
> **Audio file mode** defaults to 80% volume, adjustable 0%–500%.
> Keyboard control is only enabled when stdin is a terminal (automatically disabled for pipes/redirection).
> `-d` debug mode keeps the classic log output and does not open the TUI.

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

Limiter features (hot loops in assembly):
- **compress-only, never boosts**: normal segments keep original dynamics, only peaks above the target are pulled down
- **smooth gain envelope** (fast attack / slow release): no pop on compression, no pumping on recovery
- **hard-clamp fallback**: samples never exceed the target level

Customize the target with `--limit`, e.g. more conservative `-6 dBFS` or more aggressive `-0.5 dBFS`:

```bash
./target/release/music 乐曲.txt --limit -6
```

## TXT v3 Format (Recommended)

v3 replaces the old line-oriented timing model with **absolute tick events** and one global tempo map.
Line breaks, rests, and track output order no longer affect timing, and original MIDI track IDs are preserved.

```text
#MUSIC_RUST 3
#TITLE Example
#PPQ 480
@TEMPO 0 500000       # tick, microseconds per quarter note (120 BPM)
@TEMPO 1920 666667    # switch to ~90 BPM from tick 1920
@TRACK 0 0 "Melody"   # original MIDI track ID, MIDI channel, name
@NOTE 0 0 480 60 96   # track, start tick, duration ticks, MIDI key, velocity
@NOTE 0 480 480 62 96
@TRACK 3 1 "Bass"
@NOTE 3 0 960 48 80
```

Field conventions:

- `#PPQ` is the tick resolution, usually equal to the MIDI file's division.
- `@TEMPO` applies globally to all tracks; exact microseconds per quarter, no rounded BPM.
- Silence is simply the gap between absolute events.
- MIDI keys 0–127, velocities 1–127.

The converter emits v3 by default:

```bash
python3 convert/convert.py song.mid -o song.v3.txt
python3 convert/convert.py song.mid --bpm 90       # scale the complete tempo map
python3 convert/convert.py song.mid --track 2,5    # select original MIDI track IDs
```

Use `--v2` for the older readable `T` format or `--legacy` for the original two-line format.
The player continues to read both older formats.

## TXT Format (v2 Compatibility)

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
python3 convert/convert.py 歌曲.mid              # default output 歌曲.txt (v3 format)
python3 convert/convert.py 歌曲.mid -o 输出.txt  # specify output
python3 convert/convert.py 歌曲.mid --bpm 100    # specify tempo
python3 convert/convert.py 歌曲.mid --track 0,1  # select original MIDI tracks 0 and 1
python3 convert/convert.py 歌曲.mid              # interactively enter a velocity multiplier
python3 convert/convert.py 歌曲.mid --velocity-scale 1.25
python3 convert/convert.py 歌曲.mid --v2       # output readable legacy T format
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
| `--velocity-scale <x>` | multiply MIDI velocity; prompts interactively when omitted |
| `--keep-empty` | keep empty tracks |
| `--no-chord` | do not merge chords |
| `--v2` | output the older readable multitrack T format |
| `--legacy` | output legacy v1/v2 format |

> The converter is pure Python standard library, no third-party deps, supports standard MIDI (SMF type 0/1/2).
> In an interactive terminal, omitting `--velocity-scale` prompts for any positive multiplier (for example `0.5`, `1.25`, or `2`). Values are clamped to MIDI 1–127. v3 preserves velocity; v2/legacy notation has no velocity field.

### Dynamic BPM (Mid-song Tempo Change)

The converter collects tempo events from all MIDI tracks and writes a global `@TEMPO tick us_per_quarter`
map in v3. The player converts absolute ticks using that map, so tempo changes cannot drift between tracks.
`--bpm` scales the complete tempo map. The older `--v2` output retains `#BPM` compatibility behavior:

```
#BPM 120
T Trk
1 2 3 4          # 120 BPM
#BPM 90
5 6 7 1^         # 90 BPM from here on
```

| Tempo Directive | Description |
| -------- | ---- |
| `#BPM 120` | switch to 120 BPM |
| `90` (legacy plain-number line) | legacy format: switch to 90ms per quarter note |

## Project Comparison

| | Original `music_release` | This project `music_rust` |
| --- | --- | --- |
| Language | C++ | Rust (+ C / x86-64 assembly audio layer) |
| Platform | Windows x64 only | Linux (most distros) |
| Audio backend | Windows MIDI API (`winmm`) | fluidsynth + SoundFont |
| Timbre | system default MIDI timbre | any SoundFont + selectable GM program |
| Tracks | max 2 (left/right hand) | arbitrary multitrack parallel |
| Scheduling | thread + `clock()` busy-wait | fluidsynth sequencer ms-level |
| Playback wait | busy-wait | event-driven wait |
| Interactive control | ✗ | ✅ (arrow keys/pause/loop/quit + volume + mouse) |
| Direct MIDI playback | ✗ | ✅ (`-m`, native multitrack + tempo) |
| Audio file playback | ✗ | ✅ (WAV/MP3/FLAC/OGG/Opus/AAC/M4A/WMA + covers) |

## Debugging

The `-d` flag outputs (the TUI is replaced by plain logs in debug mode, so logs and progress never mix):

- parse tempo, track count, notes per track
- the first 20 scheduled events
- precise timestamp of every MIDI event

```bash
./target/release/music 1.txt -d 2>&1 | head -40
```

A `selftest` binary is built alongside `music`; it plays a C-major scale plus a chord through
the full synth/limiter/ALSA chain — useful for verifying audio setup:

```bash
cargo build --release --bin selftest
./target/release/selftest
```

## License

This project is released under the **GNU General Public License v3.0** (GPLv3, or, at your option, any later version).

**Copyright © 2026 FuturePioneer-3**

- Project homepage: <https://github.com/FuturePioneer-3/music_rust/>
- Full license text: [LICENSE](./LICENSE)

This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

This program is distributed in the hope that it will be useful, but **WITHOUT ANY WARRANTY**; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
