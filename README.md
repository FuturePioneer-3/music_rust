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
| 🎼 **Score mode** | jianpu TXT (v3.2 / v3.1 / v3.0 / v2 / legacy v1) | system **libfluidsynth** + SoundFont, millisecond-precise sequencer |
| 🎵 **MIDI mode** | `.mid` / `.midi` | fluidsynth native player (multitrack sync + tempo map, most accurate) |
| 📁 **Audio file mode** | WAV / MP3 / FLAC / OGG / Opus / AAC / M4A / WMA | C/FFmpeg decoder + ALSA output, album art & metadata rendering |

The original `music_release/` project was built on the Windows API (`winmm.lib`) and Windows-only.
This project is fully rewritten in **Rust** and talks directly to the system `libfluidsynth` via FFI,
supporting most Linux distributions. Run it with **no arguments** to open an interactive startup
selector; run it with a file to start playing immediately.

## What's New in v3.3.1

- 🖼️ **TXT v3.2 embedded images**: raw, zstd, gzip, zlib, deflate, bzip2, xz, or lz4-compressed image bytes can follow delimiter lines; declared lengths make arbitrary binary data, including newlines, safe.
- 🎛️ The Python converter and curses TUI can choose whether to embed an image, any of the eight encodings, and a per-algorithm compression level.
- 🔍 The launcher lets you pick a 1×–2× display zoom (0.25 steps) for TXT embedded images and audio covers.
- 🖥️ The Rust playback TUI reuses the FFmpeg decoder and half-block renderer used for audio covers, with a real-tty fallback.

## What's New in v3.22.1 (historical)

- 🐛 Fixed the MIDI duration estimator desyncing on channels 1–15, which made the progress
  bar and the estimated total time wrong in MIDI mode.
- 🛡️ Truncated or corrupt MIDI files no longer crash with an out-of-bounds panic; the
  estimator simply reports that the duration is unknown.
- 🔧 Eliminated a data race on the audio fade gain between the playback thread and the
  pause/seek controls.

## What's New in v3.22 (historical)

- 📄 **TXT v3.1** embeds its initial SoundFont/GM program and up to 24 timed timbre switches in
  the score, so a converted file carries its complete playback plan.
- 🛡️ Before playback, Rust verifies every SoundFont number and requested preset. A missing
  SoundFont or GM preset is reported immediately and playback exits instead of silently substituting it.
- 🐍 The MIDI converter now emits TXT v3.1 by default and opens an independent curses TUI
  when run without arguments; its existing CLI remains compatible and adds `--initial-soundfont`,
  `--instrument`, and repeatable `--switch` options.
- 🎚️ TXT-embedded switches are global across all TXT channels, remain correct after seeking or
  looping, and use stable file order when several switches occur at the same millisecond.

Recent milestones: **v3.21** added multi-SoundFont loading and timed MIDI/TXT switching;
**v3.0** introduced the absolute-tick TXT v3.0 format with a global tempo map.

## Key Features

1. 💯 **Pure Rust** core, no Windows dependencies; audio hot paths in C and hand-written x86-64 SSE2 assembly.
2. 🧭 **Startup selector** (no arguments): file browser, multi-SoundFont picker, GM program input and timed switch editor; `Q`/`Esc` cancels.
3. 🎹 Plays custom jianpu TXT and MIDI with any selected SoundFont timbre; timed switches are supported in both modes.
4. 🎼 **Direct MIDI playback** (`-m` or `.mid` extension): fluidsynth-native multitrack sync + tempo changes.
5. 📄 **TXT v3.2 format**: v3.0 absolute tick events and global tempo map, plus a timbre plan and optional binary image.
6. 📜 Fully compatible with TXT v3.0, v2 multi-track `T` lines, and the original v1 two-line left/right-hand format.
7. ⏱️ `fluid_sequencer` millisecond-precise event scheduling; seek/loop/pause exact to the millisecond.
8. 📊 **Dynamic progress bar**: percentage, elapsed/total time, remaining time.
9. 🎮 **Interactive controls (mpv-like)**: arrow-key seek, space pause, R loop, Q quit, `9`/`0` volume, mouse click on the progress bar.
10. 🖥️ **Fullscreen TUI**: rounded borders + truecolor palette, smooth 1/8-step progress bar, gradient dynamic-EQ spectrum with aligned band labels, colored volume bar and status indicators; **real-tty support** (Linux console/serial auto-degrades to 16-color SGR + ASCII borders + incremental redraws, no flicker).
11. 🖼️ **Album art + metadata**: embedded covers (ID3 APIC / FLAC PICTURE / M4A covr) decoded and scaled via FFmpeg, rendered with half-block characters alongside composer/artist/album/date-genre info.
12. ⚡ **WAV fast path** (`src/audio_wav.S`): RIFF chunk walk + PCM conversion in assembly — plain WAV files bypass FFmpeg entirely.
13. ⚙️ **SSE2 assembly audio core** (`src/audio_dsp.S`, `src/music_asm.S`): per-sample volume ramping + saturated clamping (no pops on volume/pause/seek), peak limiting and the 16-band Goertzel spectrum (4 lanes in parallel).
14. 🔇 **Peak limiter (default -1dBFS)**: prevents clipping/buzz from overlapping notes.
15. 📜 **Custom colored leveled logger** (`log.rs`): TRACE/DEBUG/INFO/WARN/ERROR with colors, millisecond timestamps and a 300-entry ring buffer.
16. 🐍 Companion pure-Python converter: `MIDI → jianpu TXT` (TXT v3.2 by default), with an independent no-argument curses TUI and a compatible CLI.

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
sudo pacman -U music_rust-3.3.1-1-x86_64.pkg.tar.zst
# bundles the electronic-synth SoundFont and pulls in FluidSynth automatically
music 乐曲.txt
music                # or open the startup selector
music-convert        # open the independent MIDI → TXT converter TUI
```

The package installs the converter as `/usr/bin/music-convert`. MIDI parsing uses only the Python
standard library (including curses); the optional zstd image encoder uses the packaged `zstd` command.

### Build from Source

```bash
git clone https://github.com/FuturePioneer-3/music_rust
cd music_rust
cargo build --release
./target/release/music 乐曲.txt
```

**Runtime dependencies**: `libfluidsynth.so` + any SoundFont file for scores/MIDI;
FFmpeg libraries (`libavformat`/`libavcodec`/`libavutil`/`libswresample`) + ALSA (`libasound`)
for audio files. The optional converter needs Python 3 but no packages beyond its standard library.

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

In the no-argument selector, open **Timed switches** with `Enter`: press `A` to add a rule, enter
**seconds → SoundFont number (starting at 1) → GM program**, use `Tab`/`Enter` to move between fields,
then press Right Arrow to save. Up to 24 rules apply to both MIDI and jianpu TXT. In the SoundFont list,
use `Enter`/Space to check fonts and Right Arrow to confirm; it accepts up to 3 fonts, with each font and every pair totaling no more than 120 MB.
For a TXT v3.1 score that embeds a timbre plan, the file's plan replaces these launcher rules.

**SoundFont discovery order**: `--soundfont` path → packaged font
(`/usr/share/music_rust/soundfonts/electronic_synth.sf2`) → `./electronic_synth.sf2` →
common system paths (FluidR3, GeneralUser, MS Basic, …) → `~/.local/share/soundfonts`.
Direct MIDI playback keeps the file's per-channel programs unless `--instrument` is supplied.
Choosing a GM program in the no-argument selector, or explicitly passing `--instrument`, overrides
all MIDI channels from time zero.
The startup selector lists every candidate it finds and lets you pick one.

**Mode selection**: `.mid`/`.midi` (or `-m`) → MIDI mode; `.wav`/`.mp3`/`.flac`/`.ogg`/`.opus`/
`.aac`/`.m4a`/`.wma` → audio file mode; anything else → jianpu score mode. The TUI title shows
the active mode.

### Command-line Options

| Option | Description |
| ---- | ---- |
| `-d, --debug` | detailed debug output (parse + each MIDI event) |
| `-s, --soundfont <path>` | specify a SoundFont (.sf2/.sf3) file; repeat up to 3 times (each font and every pair must total ≤120 MB) |
| `-i, --instrument <0-127>` | override the initial MIDI/score GM program; an embedded TXT v3.1 plan takes precedence (e.g. 81 = Saw Lead) |
| `-m, --midi <file>` | direct MIDI playback (native multitrack + tempo) |
| `-t, --tempo <ms>` | override tempo (>0 ms per quarter note) |
| `-b, --bpm <n>` | override tempo (1–60000 BPM) |
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

## TXT v3.2 Format (Recommended)

TXT v3.2 extends v3.0's **absolute tick events** and global tempo map with v3.1's playback-ready timbre
plan plus an optional binary image block. The image descriptor and delimiters are:

```text
#MUSIC_RUST 3.2
@INSTRUMENT <1-based SoundFont 1..3> <GM 0..127>
@SWITCH <nonnegative seconds> <SF number> <GM>
@IMAGE <MIME> <raw|zstd|gzip|zlib|deflate|bzip2|xz|lz4> <encoded-bytes> <original-bytes>
-----BEGIN MUSIC_RUST IMAGE-----
<encoded image bytes, written directly>
-----END MUSIC_RUST IMAGE-----
```

The parser reads exactly `encoded-bytes` after the begin delimiter and then requires the end delimiter,
so image data may contain newlines, zero bytes, or delimiter-like bytes. `raw` stores the original image;
`zstd` (levels 1–22), `gzip`/`zlib`/`deflate`/`xz` (levels 0–9), `bzip2` (levels 1–9), and `lz4`
(levels 1–12) store compressed bytes using the converter-selected level. Rust decompresses it and
passes it through FFmpeg, rendering it in the score TUI like an MP3 cover. A decode failure does not stop playback.

`@INSTRUMENT` selects the initial SoundFont and GM program. `@SWITCH` changes both at the stated
nonnegative time; decimal seconds are accepted and normalized to milliseconds. A file may contain
up to 24 `@SWITCH` records. Each switch is global to every TXT channel, and seeking or looping
reconstructs the selection that should be active at the destination. If several records normalize
to the same millisecond, their file order is preserved and the last one takes effect.

Complete example:

```text
#MUSIC_RUST 3.2
#TITLE Example
#PPQ 480
@INSTRUMENT 1 80
@SWITCH 2.5 1 81
@SWITCH 5 2 40
@TEMPO 0 500000       # tick, microseconds per quarter note (120 BPM)
@TEMPO 1920 666667    # switch to ~90 BPM from tick 1920
@TRACK 0 0 "Melody"   # original MIDI track ID, MIDI channel, name
@NOTE 0 0 480 60 96   # track, start tick, duration ticks, MIDI key, velocity
@NOTE 0 480 480 62 96
@TRACK 3 1 "Bass"
@NOTE 3 0 960 48 80
```

Field and playback conventions:

- SoundFont numbers are 1-based indices into the 1–3 loaded SoundFonts; GM programs are 0–127.
- If omitted in a v3.2/v3.1 file, the initial selection is SoundFont 1 / GM 0. Automatic discovery
  prioritizes the bundled `electronic_synth.sf2` as SoundFont 1, so the default remains the electronic font.
- A v3.2/v3.1 file's embedded initial selection and switch plan take precedence over the no-argument
  launcher's timbre settings and `--instrument`. SoundFont paths are still supplied by discovery,
  the launcher, or repeated `--soundfont` arguments.
- Before scheduling any note, Rust checks that every referenced SoundFont is loaded and that every
  requested GM preset exists in that font. If either is missing, it reports the requirement and exits.
- `#PPQ` is the tick resolution, usually equal to the MIDI file's division. `@TEMPO` applies globally
  to all tracks and stores exact microseconds per quarter note; silence is the gap between absolute events.
- MIDI keys are 0–127 and velocities are 1–127.

The converter emits TXT v3.2 by default:

```bash
python3 convert/convert.py song.mid -o song.v31.txt
python3 convert/convert.py song.mid --initial-soundfont 2 --instrument 40
python3 convert/convert.py song.mid --switch 5:81 --switch 12.5:2:40
python3 convert/convert.py song.mid --bpm 90       # scale the complete tempo map
python3 convert/convert.py song.mid --track 2,5    # select original MIDI track IDs
python3 convert/convert.py song.mid --embed-image cover.jpg --image-compression zstd --image-level 12
```

In `seconds:program`, the switch uses `--initial-soundfont`; use `seconds:SF:program` to choose a
different SoundFont. Use `--v2` for the readable `T` format or `--legacy` for the original two-line
format. Those formats cannot store non-default timbre settings, so the converter rejects their use
with a non-default `--initial-soundfont`, `--instrument`, or any `--switch`.

TXT v3.0 (`#MUSIC_RUST 3`), v2, and legacy v1 files remain fully supported. v3.0 retains the same
absolute timing and tempo-map behavior but obtains its timbre plan from the launcher/command line.

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

Run the converter **without arguments** to open its own curses setup screen. This interface is
separate from the player's startup selector and playback TUI, so it does not alter the main TUI:

```bash
python3 convert/convert.py   # source tree
music-convert               # installed Arch package
```

The converter TUI lets you choose the input MIDI file and output TXT path, edit the basic conversion
options, set the initial SoundFont number and GM program, add/edit up to 24 timed SoundFont/program
switches, choose whether to embed an image, choose raw/zstd encoding and its compression level, and
follow conversion status and the final success/error result in the same screen.

Supplying a MIDI path continues to use the existing non-interactive command line; all previous CLI
options and scripts remain compatible. The packaged `music-convert` command accepts the same options
as `python3 convert/convert.py`:

```bash
music-convert 歌曲.mid              # default output 歌曲.txt (TXT v3.2)
music-convert 歌曲.mid -o 输出.txt  # specify output
music-convert 歌曲.mid --bpm 100    # specify tempo
music-convert 歌曲.mid --track 0,1  # select original MIDI tracks 0 and 1
music-convert 歌曲.mid --initial-soundfont 2 --instrument 40
music-convert 歌曲.mid --switch 5:81 --switch 12.5:2:40
music-convert 歌曲.mid              # interactively enter a velocity multiplier
music-convert 歌曲.mid --velocity-scale 1.25
music-convert 歌曲.mid --v2         # output readable legacy T format
music-convert 歌曲.mid --legacy     # output legacy two-line format
music-convert 歌曲.mid --embed-image cover.jpg --image-compression zstd --image-level 12
```

### Converter Options

| Option | Description |
| ---- | ---- |
| `-o, --output` | output file (default same-name `.txt`) |
| `-b, --bpm` | override tempo (1–60000 BPM) |
| `--track` | only convert specific tracks (`0,1,2`) |
| `--quantize <tick>` | quantization step (recommend 30 to remove tiny timing jitter) |
| `--max-tracks <n>` | max number of tracks |
| `--drum` | include drum tracks (excluded by default) |
| `--min-vel <n>` | filter low-velocity notes |
| `--velocity-scale <x>` | multiply MIDI velocity; prompts interactively when omitted |
| `--keep-empty` | keep empty tracks |
| `--no-chord` | do not merge chords |
| `--initial-soundfont <1..3>` | initial 1-based SoundFont number (default 1) |
| `--instrument <0..127>` | initial GM program (default 0) |
| `--switch <seconds:program>` | timed program switch on the initial SoundFont; repeat up to 24 times |
| `--switch <seconds:SF:program>` | timed SoundFont/program switch; may be mixed and repeated up to 24 times total |
| `--embed-image <image>` | embed image bytes in TXT v3.2; omit to disable |
| `--image-compression <encoding>` | store raw, zstd, gzip, zlib, deflate, bzip2, xz, or lz4 image bytes (default raw) |
| `--image-level <n>` | compression level (default 3; ranges differ per algorithm) |
| `--v2` | output the older readable multitrack T format; accepts only default timbre configuration |
| `--legacy` | output legacy v1/v2 format; accepts only default timbre configuration |

> The MIDI parser uses only the Python standard library; gzip/zlib/deflate/bzip2/xz compression uses Python's standard library, while zstd and lz4 additionally require the system `zstd` and `lz4` commands. It supports standard MIDI (SMF type 0/1/2).
> In an interactive terminal, omitting `--velocity-scale` prompts for any positive multiplier (for example `0.5`, `1.25`, or `2`). Values are clamped to MIDI 1–127. TXT v3.2 preserves velocity; v2/legacy notation has no velocity field. `--v2` and `--legacy` reject any non-default timbre configuration because those formats cannot represent it.
> For type-0 and other single-physical-track/multi-channel files, v3.2 splits channels into independent `@TRACK` records instead of folding every note onto the first channel. Without `--drum`, percussion channel 10 (internal ch9) is filtered per event.
> The converter explicitly rejects SMPTE time-division MIDI; convert it to PPQ first so absolute-tick timing cannot be misinterpreted.

### Dynamic BPM (Mid-song Tempo Change)

The converter collects tempo events from all MIDI tracks and writes a global `@TEMPO tick us_per_quarter`
map in TXT v3.2. The player converts absolute ticks using that map, so tempo changes cannot drift between tracks.
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
