<div align="center">

# 🎹 music_rust

**MIDI 简谱钢琴演奏器 —— 跨平台（主要 Linux），Rust 主体 + C 音频层**

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

music_rust 是一个**音乐播放器**：简谱 TXT 和 MIDI 使用系统 **libfluidsynth** 钢琴合成器；WAV、MP3、FLAC、OGG、Opus、AAC、M4A 等音频文件使用 C/FFmpeg 解码并通过 ALSA 输出。

原项目 `music_release/` 基于 Windows API (`winmm.lib`) 开发，仅支持 Windows。本项目完全用 **Rust** 重写，通过 FFI 直连系统 `libfluidsynth` 合成器，支持绝大多数 Linux 发行版。

## 特性

1. 💯 **纯 Rust** 实现，无任何 Windows 依赖。
2. 🎹 通过系统 SoundFont 用钢琴音色演奏自定义简谱 TXT。
3. 🎼 **直接播放 MIDI 文件**（`-m` 或 `.mid` 扩展名）：fluidsynth 原生多轨同步 + tempo 变速，最准确。
4. 🧵 **改进版多音轨 TXT 格式**：支持任意数量并行音轨 + 轨内顺序段落。
5. 📜 完全兼容原版 v1/v2 格式（左右手双轨、空行分组）。
6. ⏱️ `fluid_sequencer` 毫秒级精确事件调度。
7. 📊 **动态进度条**：实时显示进度百分比、已播/总时长、剩余时间。
8. 🎮 **交互控制（类似 mpv）**：方向键快进/后退、空格暂停、Enter/P 播放、R 循环、Q 退出，以及 **`9`/`0` 音量调节**。
9. 🖥️ **终端播放界面（2.4.0 全面重绘）**：圆角边框 + 真彩色配色，平滑进度条（1/8 精度分块）、渐变动态 EQ 频率图、彩色音量条与状态指示；可点击进度条跳转、点击状态行暂停/继续。
10. 🖼️ **专辑封面 + 作曲家解析（2.4.0）**：MP3/FLAC/M4A 等文件内嵌封面（ID3 APIC / FLAC PICTURE / M4A covr）经 FFmpeg 解码后，以半块字符（▀）真彩色渲染在 TUI 下方，并并排显示**作曲家**、艺术家、专辑、年代/风格等元数据；封面尺寸随终端自适应（大屏 ≤ 46 列 / 45% 高度，小屏保底 6 行且自动隐藏 EQ 等次要区域，绝不与上方内容重叠）。
11. ⚡ **AT&T 语法汇编加速（2.4.0，非内联）**：音频线程的音量饱和缩放（SSE2 8 样本/迭代 + packssdw 饱和）、16 频段 Goertzel 频谱谐振器（4 通道 SSE2 并行）与峰值限制器热循环（向量峰值扫描 + minps/maxps 硬钳制）全部改为独立 `music_asm.S` 汇编例程，仅使用 x86-64 基线 SSE2，任何处理器可运行。
12. 🔇 **峰值限制器（默认 -1dBFS）**：自动防止多音符叠加削波/电流声。
13. 🛠️ 调试模式：详细输出解析日志与每个 MIDI 事件。
14. 🐍 配套 Python 脚本：`MIDI → 简谱 TXT` 转换器。

## 快速开始

### AppImage（推荐，开箱即用）

从 [Releases](https://github.com/FuturePioneer-3/music_rust/releases) 页面下载 `music_rust-*-x86_64.AppImage`，内置精简 GM 音源，无需任何依赖。

```bash
chmod +x music_rust-x86_64.AppImage
./music_rust-x86_64.AppImage 乐曲.txt     # 播放简谱
./music_rust-x86_64.AppImage 歌曲.mid     # 播放 MIDI
```

内置 SoundFont 已自动加载；如需指定其它音源：`--soundfont /path/to/xx.sf2`。

### Arch Linux（pkg.tar.zst）

```bash
sudo pacman -U music_rust-2.3.0-1-x86_64.pkg.tar.zst
# 自动安装依赖 fluidsynth + soundfont-fluid
music 乐曲.txt
```

### 从源码构建

```bash
git clone https://github.com/FuturePioneer-3/music_rust
cd music_rust
cargo build --release
./target/release/music 乐曲.txt
```

**依赖（运行时）**：MIDI/TXT 需要 `libfluidsynth.so`；WAV/MP3 等音频文件需要 FFmpeg 运行库与 ALSA（`libavformat`、`libavcodec`、`libavutil`、`libswresample`、`libasound`）。AppImage 内置 SoundFont，但音频解码库仍使用系统库。

| 发行版 | 安装命令 |
| ------ | -------- |
| Debian/Ubuntu | `sudo apt install libfluidsynth3 soundfont-fluid` |
| Arch/Manjaro | `sudo pacman -S fluidsynth soundfont-fluid` |
| Fedora | `sudo dnf install fluidsynth fluid-soundfont-gm` |
| openSUSE | `sudo zypper install fluidsynth fluidsynth-soundfont` |
| Gentoo | `sudo emerge media-sound/fluidsynth` |

> 编译需要开发头文件：`libfluidsynth-dev`（Debian/Ubuntu）/ `fluidsynth`（Arch）等。

## 使用

```bash
# 播放一首简谱 TXT（自动找系统 SoundFont）
./target/release/music 乐曲.txt

# 直接播放 MIDI 文件（fluidsynth 原生多轨+变速，最准确）
./target/release/music 歌曲.mid
./target/release/music -m 歌曲.mid

# 调试模式（详细日志，无进度条）
./target/release/music 乐曲.txt -d

# 指定 SoundFont
./target/release/music 乐曲.txt --soundfont /path/to/piano.sf2

# 覆盖速度
./target/release/music 乐曲.txt --bpm 90
./target/release/music 歌曲.mid -b 90
./target/release/music 乐曲.txt --tempo 500   # 500ms/四分音符

# 音量
./target/release/music 乐曲.txt --volume 110
```

> **两种模式**：传入 `.mid`/`.midi` 文件或使用 `-m` 参数即进入 MIDI 模式，
> 用 fluidsynth 内置播放器原生处理多轨同步与 tempo 变化（最准确，推荐）。
> 其余情况解析简谱 TXT。

### 命令行参数

| 参数 | 说明 |
| ---- | ---- |
| `-d, --debug` | 详细调试输出（解析 + 每个 MIDI 事件） |
| `-s, --soundfont <路径>` | 指定 SoundFont 文件 |
| `-m, --midi <file>` | 直接播放 MIDI 文件（fluidsynth 原生多轨+变速） |
| `-t, --tempo <ms>` | 覆盖速度（毫秒/四分音符） |
| `-b, --bpm <n>` | 覆盖速度（BPM） |
| `-v, --volume <0-500>` | 音量（默认 80%，0% 静音） |
| `-l, --limit <dB>` | 峰值限制电平（默认 `-1.0` dBFS，防止削波） |
| `-h, --help` | 帮助 |

环境变量：

| 变量 | 说明 |
| ---- | ---- |
| `MUSIC_AUDIO_DRIVER` | 强制 fluidsynth 音频驱动，如 `alsa` / `pulseaudio` / `pipewire` |

### 播放控制（交互式）

播放过程中可直接用键盘控制（类似 mpv）。

| 按键 | 功能 |
| --- | --- |
| `←` / `→` | 后退 / 快进 5 秒 |
| `↑` / `↓` | 快进 / 后退 10 秒 |
| `PageUp` / `PageDown` | 快进 / 后退 1 分钟 |
| `空格` / `P` | 暂停 / 继续 |
| `[` / `]` | 后退 / 快进 1 秒 |
| `R` | 切换循环播放 |
| `1` – `8` | 跳转到 10% – 80% |
| `9` / `0` | **降低 / 增加音量** |
| `Q` | 退出 |

普通交互终端会自动启用全屏播放界面，并支持鼠标：点击进度条可跳转，点击状态行可暂停或继续。`-d` 调试模式保持原有详细日志输出，不启用播放界面。

### 音频文件播放

传入 `.wav`、`.mp3`、`.flac`、`.ogg`、`.opus`、`.aac`、`.m4a` 或 `.wma` 文件时，程序自动进入**音乐文件模式**；`.mid`/`.midi` 仍进入 **MIDI 模式**，其它文本文件进入**简谱模式**。TUI 标题会明确显示当前模式。

音频文件模式默认音量为 **80%**，可调范围为 **0%-500%**（0% 静音）。`9` 降低 10%，`0` 增加 10%；`Space` 暂停/继续，`Enter` 或 `P` 播放，方向键和鼠标进度条跳转。音频模式在 TUI 中显示实时动态频率估算值，并在下方渲染内嵌**专辑封面**与**作曲家**等元数据（若有）；MIDI 与简谱模式显示各音轨当前按下的音名，例如 `C4`、`C#4`、`D4`。

> **TXT 模式**（简谱）基于事件动态重排，快进/后退/循环/暂停都精确到毫秒；
> **MIDI 模式**使用 fluidsynth 原生播放器（`fluid_player_seek` / `set_loop`），同样支持上述按键。
> 键盘控制仅在 stdin 为终端时启用（管道/重定向输入自动禁用，不影响自动化）。

## 音频驱动自动探测（防爆音）

程序自动选择最优音频后端，无需手动配置。

1. **PipeWire**（检测到 `pipewire-0` socket）—— 现代 Linux 桌面首选
2. **PulseAudio**（检测到 `pulse/native` socket）—— 经典桌面或 PipeWire 兼容层
3. **ALSA** —— 无桌面环境的回退

同时自动**增大音频缓冲区**（`audio.period-size=512`、`audio.periods=4`，
约 46ms @44.1kHz），并**提升合成器增益**（`synth.gain=1.0`），
配合峰值限制器，显著减少调度延迟导致的**爆音/电流声**。

> 若仍有个别环境异常，可用 `MUSIC_AUDIO_DRIVER=alsa music 乐曲.txt`
> 或 `MUSIC_AUDIO_DRIVER=pulseaudio music 乐曲.txt` 强制切换。

## 峰值限制器（防削波）

合成器输出经过一个**实时峰值限制器**（默认限制到 `-1dBFS`），彻底避免：

- 大量音符同时发声（大和弦）导致的削波
- 音量调大后的破音/电流声
- 超出音频满刻度（0dBFS）的失真

限制器特性（2.4.0 起热循环由汇编实现）：
- **只压缩不放大**：正常音量段保持原始动态，仅当峰值超过目标电平时自动压下来
- **平滑增益包络**（快 attack / 慢 release）：压降不爆音，恢复无泵浦感
- **硬钳制兜底**：任何时刻样本都不会超过目标电平

通过 `--limit` 可自定义目标电平，例如更保守的 `-6 dBFS` 或更激进的 `-0.5 dBFS`：

```bash
./target/release/music 乐曲.txt --limit -6
```

## TXT 格式（改进版多音轨）

### 头部

```
#TITLE 曲名            # 可选，乐曲标题
#BPM 120              # 可选，速度（BPM）；与旧版纯数字 `500`（毫秒）二选一
```

`#BPM` 指令也可出现在**文件任意位置**，表示**中途变速**（从该点起切换速度，作用于之后的所有音符）。

### 音轨定义

```
T 轨名 | 段1 | 段2 | ...
```

- 每个 `T` 开头的行定义一个音轨（**并行**播放）
- 轨名后面的 `|` 分隔的**段落**在该轨内**顺序**播放
- 竖线 `|` 同时兼容原版"小节线"语义（被忽略，不产生休止）
- 同一轨的**多行**也按顺序播放（可用空行分隔视觉段落）

示例（三轨并行：旋律 + 伴奏 + 贝斯）：

```
#TITLE 示例曲
#BPM 120
T 旋律(高音) | 1^ 2^ 3^ 4^ | 5^ 6^ 7^ 1^^ | 2^^ 3^^ 2^^ 1^^ | 7^ 6^ 7^ 5^
T 伴奏(低音) | 1, 3, 5, 1 | 4 6 2 5 | 3 1 6 4 | 5 3 2 7,
T 贝斯       | 1,, 0 1,, 0 | 4,, 0 2,, 0 | 3,, 0 6,, 0 | 5,, 0 7,, 0
```

> 支持**超过 16 条音轨**（通道自动循环映射）。

### 音符语法

| 字符 | 说明 | 示例 |
| ---- | ---- | ---- |
| `1~7` | 音符（do re mi fa sol la si） | `1 2 3 4 5` |
| `0` | 休止符 | `0` |
| `,` | 低音（最多 3 个） | `1,` `2,,` |
| `^` | 高音（最多 4 个） | `5^` `1^^` |
| `#` | 升半音 | `4#` `5#^` |
| `-` | 延音（每个 +1 个四分音符） | `5-` `1---` |
| `_` | 分音（时值减半） | `5_` `1__` |
| `.` | 附点（时值 ×1.5） | `5.` |
| `*` | 时值 ÷3 | `5*` |
| `%` | 时值 ÷5 | `5%` |
| `&` | 时值 ÷7 | `5&` |
| `[]` | 和弦（同时发声） | `[1,3,5,]` |
| `\|` | 小节线（忽略） | `1 2 \| 3 4` |

示例：

```
5^ 1^^ 2^^_ 6^- | [1^3^5^]- [4^6^1^^] | 0 0 4_,_ | 5^.--- 0
```

### 旧版 v1/v2 兼容

原 Windows 版的两行一组左右手格式仍然支持：

```
500                    # 第一行：四分音符毫秒
1 2 3 4 5 5 4 3 2 1    # 右手
1^- 2^- 3^ 4^- 5^--    # 左手（与上行同时）

1 1 5 5 6 6 5          # 新一组
```

- 每两行一组 = 左右手同时播放
- 空行强制打断配对
- 中间出现纯数字行 = 切换速度

## MIDI → TXT 转换器

```bash
python3 convert/convert.py 歌曲.mid              # 默认输出 歌曲.txt
python3 convert/convert.py 歌曲.mid -o 输出.txt  # 指定输出
python3 convert/convert.py 歌曲.mid --bpm 100    # 指定速度
python3 convert/convert.py 歌曲.mid --track 0,1  # 只转换前两轨
python3 convert/convert.py 歌曲.mid --legacy     # 输出旧版两行一组格式
```

### 转换器参数

| 参数 | 说明 |
| ---- | ---- |
| `-o, --output` | 输出文件（默认同名 `.txt`） |
| `-b, --bpm` | 覆盖速度 |
| `--track` | 只转换指定音轨（`0,1,2`） |
| `--quantize <tick>` | 量化粒度（建议 30，消除微小时值抖动） |
| `--max-tracks <n>` | 最多音轨数 |
| `--drum` | 包含打击乐轨（默认排除） |
| `--min-vel <n>` | 过滤低力度音符 |
| `--keep-empty` | 保留空白音轨 |
| `--no-chord` | 不合并和弦 |
| `--legacy` | 输出旧版 v1/v2 格式 |

> 转换器纯 Python 标准库实现，无第三方依赖，支持标准 MIDI (SMF type 0/1/2)。

### 动态 BPM（中途变速）

转换器自动探测 MIDI 中的**速度变化**（tempo change）：

- 从第一个轨道（conductor track）解析所有 tempo 事件
- 变速点前输出 `#BPM xxx` 指令行，Rust 播放器在对应位置**精确切换速度**

```
#BPM 120
# tempo change @tick2400: 90 BPM (666666us/q)

T Trk
1 2 3 4          # 120 BPM
#BPM 90
5 6 7 1^         # 90 BPM
```

`#BPM` 指令可出现在**任意位置**（轨内、轨间），作用为**从该点起切换速度**：

| 速度指令 | 说明 |
| -------- | ---- |
| `#BPM 120` | 切换到 120 BPM |
| `90`（旧版纯数字行） | 旧版格式：切换到 90ms/四分音符 |

## 项目对比

| | 原版 `music_release` | 本项目 `music_rust` |
| --- | --- | --- |
| 语言 | C++ | Rust |
| 平台 | 仅 Windows x64 | Linux（兼容大多数发行版） |
| 音频后端 | Windows MIDI API (`winmm`) | fluidsynth + SoundFont |
| 音色 | 系统默认 MIDI 音色 | 钢琴（可换任何 SoundFont） |
| 音轨 | 最多 2 轨（左右手） | 任意多轨并行 |
| 调度精度 | 线程 + `clock()` 忙等 | fluidsynth sequencer 毫秒级 |
| 播放等待 | 忙等 | 事件驱动等待 |
| 交互控制 | ✗ | ✅（方向键/暂停/循环/退出 + 音量） |
| MIDI 直接播放 | ✗ | ✅（`-m`，原生多轨+变速） |

## 调试

`-d` 参数会输出（调试模式下进度条自动隐藏，日志与进度不混排）：

- 解析速度、音轨数量、每轨音符数
- 前 20 条排程事件
- 每个 MIDI 事件的精确时间戳（channel / note-on/off / key / velocity）

```bash
./target/release/music 1.txt -d 2>&1 | head -40
```

## 许可证

本项目基于 **GNU General Public License v3.0**（GPLv3，或后续版本）发布。

**Copyright © 2026 FuturePioneer-3**

- 项目主页：<https://github.com/FuturePioneer-3/music_rust/>
- 完整许可文本见：[LICENSE](./LICENSE)

本程序是自由软件：您可以在自由软件基金会发布的
GNU 通用公共许可证（第 3 版，或按您的选择更高版本）条款下
重新分发和/或修改它。

本程序按"原样"分发，希望它有用，但**不提供任何担保**；
甚至不提供适销性或特定用途适用性的暗示担保。
详情请参阅 GNU 通用公共许可证。
