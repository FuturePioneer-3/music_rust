<div align="center">

# 🎹 music_rust

**终端音乐播放器：简谱 / MIDI / 音频文件 —— 跨平台（主要 Linux），Rust 主体 + C/汇编音频层**

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

music_rust 是一个**终端音乐播放器**，内置三套播放引擎：

| 模式 | 输入 | 引擎 |
| ---- | ---- | ---- |
| 🎼 **简谱模式** | 简谱 TXT（v3 / v2 / 旧版 v1） | 系统 **libfluidsynth** + SoundFont，毫秒级排程 |
| 🎵 **MIDI 模式** | `.mid` / `.midi` | fluidsynth 原生播放器（多轨同步 + tempo 变速，最准确） |
| 📁 **音乐文件模式** | WAV / MP3 / FLAC / OGG / Opus / AAC / M4A / WMA | C/FFmpeg 解码 + ALSA 输出，专辑封面与元数据渲染 |

原项目 `music_release/` 基于 Windows API (`winmm.lib`) 开发，仅支持 Windows。
本项目完全用 **Rust** 重写，通过 FFI 直连系统 `libfluidsynth` 合成器，支持绝大多数 Linux 发行版。
**不带参数**运行会打开交互式启动选择器；带文件运行则直接开始播放。

## v3.20 新特性

- 🧭 **无参数启动选择器**：浏览当前目录的可播放文件、选择 SoundFont、设置简谱播放使用的
  GM 音色号（0–127），确认后直接进入播放 TUI。
- 🎛️ **内置 `electronic_synth.sf2` 电子合成器音色库**：随仓库与 Arch 包分发
  （安装到 `/usr/share/music_rust/soundfonts/`），启动时自动发现。
- 🎚️ 新增 `-i, --instrument <0-127>`：简谱播放可任选 GM 音色
  （默认 0 = 原声大钢琴；如 81 = 锯齿波主音）。
- 🔧 修复 MIDI/TXT 音量状态与实际合成增益不一致、限制器包络残留导致的响度滞后。
- 🔧 修复音频文件暂停/跳转线程死锁、EOF 竞态与 TUI 进度条行号错位。

近期里程碑：**v3.1** 新增纯汇编 WAV 快速路径（普通 WAV 完全不经过 FFmpeg）；
**v3.0** 引入绝对 tick 的 TXT v3 格式与全局 tempo 表。

## 特性

1. 💯 **纯 Rust** 主体，无任何 Windows 依赖；音频热路径由 C 与手写 x86-64 SSE2 汇编实现。
2. 🧭 **启动选择器**（无参数运行）：文件浏览、SoundFont 选择、GM 音色号输入；`Q`/`Esc` 取消。
3. 🎹 用任意 SoundFont 音色演奏自定义简谱 TXT，GM 音色可通过 `--instrument` 选择。
4. 🎼 **直接播放 MIDI 文件**（`-m` 或 `.mid` 扩展名）：fluidsynth 原生多轨同步 + tempo 变速。
5. 📄 **TXT v3 格式**：绝对 tick 事件 + 全局 tempo 表；换行/休止/输出顺序不再影响时间，保留原始 MIDI 轨道 ID。
6. 📜 完全兼容旧格式：v2 多轨 `T` 行、原版 v1 两行一组左右手格式。
7. ⏱️ `fluid_sequencer` 毫秒级精确事件调度；快进/后退/循环/暂停精确到毫秒。
8. 📊 **动态进度条**：实时百分比、已播/总时长、剩余时间。
9. 🎮 **交互控制（类似 mpv）**：方向键快进/后退、空格暂停、R 循环、Q 退出、`9`/`0` 音量、鼠标点击进度条跳转。
10. 🖥️ **全屏 TUI**：圆角边框 + 真彩色、平滑 1/8 精度进度条、渐变动态 EQ 频谱（频段标签精确对齐）、彩色音量条与状态指示；**真实 tty 支持**（Linux 控制台/串口自动降级 16 色 SGR + ASCII 边框 + 增量重绘，无乱码无闪烁）。
11. 🖼️ **专辑封面 + 元数据**：内嵌封面（MP3 APIC / FLAC PICTURE / M4A covr）经 FFmpeg 解码缩放，以半块字符渲染，并显示作曲家/艺术家/专辑/年代风格等信息。
12. ⚡ **WAV 快速路径**（`src/audio_wav.S`）：RIFF 块遍历 + PCM 转换全部汇编实现——普通 WAV 完全不经过 FFmpeg。
13. ⚙️ **SSE2 汇编音频内核**（`src/audio_dsp.S`、`src/music_asm.S`）：逐样本音量线性渐变 + 饱和钳制（音量调节/暂停/跳转不爆音）、峰值限制、16 段 Goertzel 频谱（4 路并行）。
14. 🔇 **峰值限制器（默认 -1dBFS）**：自动防止多音符叠加削波/电流声。
15. 📜 **自研彩色分级日志**（`log.rs`）：TRACE/DEBUG/INFO/WARN/ERROR 分级着色 + 毫秒时间戳 + 300 条环形缓冲。
16. 🐍 配套纯 Python 转换器：`MIDI → 简谱 TXT`（默认输出 v3）。

## 快速开始

所有二进制均以 **release 资产**形式分发——请从
[Releases](https://github.com/FuturePioneer-3/music_rust/releases) 页面下载
（git 仓库只保留源码）。

### AppImage（推荐，开箱即用）

从最新 release 下载 `music_rust-<版本>-x86_64.AppImage`（及对应 `.sha256`）。
内置精简 GM 音源，无需任何依赖。

```bash
chmod +x music_rust-*-x86_64.AppImage
./music_rust-*-x86_64.AppImage            # 打开启动选择器
./music_rust-*-x86_64.AppImage 乐曲.txt    # 播放简谱
./music_rust-*-x86_64.AppImage 歌曲.mid    # 播放 MIDI
```

内置 SoundFont 已自动加载；如需指定其它音源：`--soundfont /path/to/xx.sf2`。

### Arch Linux（pkg.tar.zst）

从最新 release 下载 `music_rust-<版本>-1-x86_64.pkg.tar.zst`，然后：

```bash
sudo pacman -U music_rust-3.20-1-x86_64.pkg.tar.zst
# 包内已包含电子合成器 SoundFont；自动安装 fluidsynth 等运行依赖
music 乐曲.txt
music                # 或打开启动选择器
```

### 从源码构建

```bash
git clone https://github.com/FuturePioneer-3/music_rust
cd music_rust
cargo build --release
./target/release/music 乐曲.txt
```

**运行依赖**：简谱/MIDI 需要 `libfluidsynth.so` + 任意 SoundFont 文件；
音频文件需要 FFmpeg 运行库（`libavformat`/`libavcodec`/`libavutil`/`libswresample`）与 ALSA（`libasound`）。

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
# 打开启动选择器（文件、SoundFont、GM 音色号）
./target/release/music

# 播放一首简谱 TXT（自动发现 SoundFont）
./target/release/music 乐曲.txt

# 直接播放 MIDI 文件（fluidsynth 原生多轨+变速，最准确）
./target/release/music 歌曲.mid
./target/release/music -m 歌曲.mid

# 播放音频文件（TUI 显示专辑封面与元数据）
./target/release/music song.flac

# 调试模式（详细日志，不启用 TUI）
./target/release/music 乐曲.txt -d

# 指定 SoundFont
./target/release/music 乐曲.txt --soundfont /path/to/piano.sf2

# 使用内置电子合成器 SoundFont，并选择锯齿波主音（GM 81）
./target/release/music 乐曲.txt --soundfont ./electronic_synth.sf2 --instrument 81

# 安装 Arch 包后，随包音色会被自动发现
music 乐曲.txt --instrument 81

# 覆盖速度
./target/release/music 乐曲.txt --bpm 90
./target/release/music 歌曲.mid -b 90
./target/release/music 乐曲.txt --tempo 500   # 500ms/四分音符

# 音量
./target/release/music 乐曲.txt --volume 110
```

**SoundFont 查找顺序**：`--soundfont` 指定路径 → 随包音色
（`/usr/share/music_rust/soundfonts/electronic_synth.sf2`）→ `./electronic_synth.sf2` →
常见系统路径（FluidR3、GeneralUser、MS Basic 等）→ `~/.local/share/soundfonts`。
启动选择器会列出所有找到的候选音源供挑选。

**模式判定**：`.mid`/`.midi`（或使用 `-m`）→ MIDI 模式；`.wav`/`.mp3`/`.flac`/`.ogg`/`.opus`/
`.aac`/`.m4a`/`.wma` → 音乐文件模式；其余文本 → 简谱模式。TUI 标题会明确显示当前模式。

### 命令行参数

| 参数 | 说明 |
| ---- | ---- |
| `-d, --debug` | 详细调试输出（解析 + 每个 MIDI 事件） |
| `-s, --soundfont <路径>` | 指定 SoundFont (.sf2/.sf3) 文件 |
| `-i, --instrument <0-127>` | 简谱音色的 GM Program（默认 0；如 81 = 锯齿波主音） |
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

鼠标：点击进度条跳转，点击状态行暂停/继续。

> **简谱模式**基于事件动态重排，快进/后退/循环/暂停都精确到毫秒；
> **MIDI 模式**使用 fluidsynth 原生播放器（`fluid_player_seek` / `set_loop`），按键相同；
> **音乐文件模式**默认音量 80%，可调范围 0%–500%。
> 键盘控制仅在 stdin 为终端时启用（管道/重定向自动禁用，不影响自动化）。
> `-d` 调试模式保持传统日志输出，不启用 TUI。

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

限制器特性（热循环由汇编实现）：
- **只压缩不放大**：正常音量段保持原始动态，仅当峰值超过目标电平时自动压下来
- **平滑增益包络**（快 attack / 慢 release）：压降不爆音，恢复无泵浦感
- **硬钳制兜底**：任何时刻样本都不会超过目标电平

通过 `--limit` 可自定义目标电平，例如更保守的 `-6 dBFS` 或更激进的 `-0.5 dBFS`：

```bash
./target/release/music 乐曲.txt --limit -6
```

## TXT v3 格式（推荐）

v3 专门解决旧版"按行推进时间"的歧义。它使用**绝对 tick 音符事件**和一张全局 tempo 表：
换行、休止符数量、轨道输出顺序都不会改变实际时间；MIDI 的每个原始轨道 ID 也会保留。

```text
#MUSIC_RUST 3
#TITLE 示例
#PPQ 480
@TEMPO 0 500000       # tick, 每四分音符微秒数（120 BPM）
@TEMPO 1920 666667    # 从 tick 1920 起切换到约 90 BPM
@TRACK 0 0 "旋律"     # 原始 MIDI 轨道 ID, MIDI 通道, 轨名
@NOTE 0 0 480 60 96   # 轨道, 起始 tick, 时长 tick, MIDI 音高, 力度
@NOTE 0 480 480 62 96
@TRACK 3 1 "伴奏"
@NOTE 3 0 960 48 80
```

字段约定：

- `#PPQ` 是 tick 分辨率，通常等于 MIDI 文件的 division。
- `@TEMPO` 对所有轨道全局生效，单位是 microseconds per quarter，不使用四舍五入后的 BPM。
- 休止通过绝对时间间隔自然表达。
- MIDI 音高 0–127，力度 1–127。

MIDI 转换器默认生成 v3：

```bash
python3 convert/convert.py song.mid -o song.v3.txt
python3 convert/convert.py song.mid --bpm 90       # 按比例缩放整张 tempo 表
python3 convert/convert.py song.mid --track 2,5    # 按原始 MIDI 轨道编号选择
```

需要旧格式时使用 `--v2`（可读的 `T` 多轨格式）或 `--legacy`（v1 两行一组）。旧格式仍可由播放器读取。

## TXT 格式（v2 兼容）

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
python3 convert/convert.py 歌曲.mid              # 默认输出 歌曲.txt（v3 格式）
python3 convert/convert.py 歌曲.mid -o 输出.txt  # 指定输出
python3 convert/convert.py 歌曲.mid --bpm 100    # 指定速度
python3 convert/convert.py 歌曲.mid --track 0,1  # 按原始 MIDI 编号转换 0、1 轨
python3 convert/convert.py 歌曲.mid              # 交互输入力度倍率，如 0.5、1.25、2
python3 convert/convert.py 歌曲.mid --velocity-scale 1.25
python3 convert/convert.py 歌曲.mid --v2       # 输出旧版可读 T 多轨格式
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
| `--velocity-scale <x>` | MIDI 力度倍率；交互终端未指定时自由输入，默认 1.0 |
| `--keep-empty` | 保留空白音轨 |
| `--no-chord` | 不合并和弦 |
| `--v2` | 输出旧版可读多音轨 T 格式 |
| `--legacy` | 输出旧版 v1/v2 格式 |

> 转换器纯 Python 标准库实现，无第三方依赖，支持标准 MIDI (SMF type 0/1/2)。
> 在交互终端运行且未指定 `--velocity-scale` 时，转换器会询问力度倍率；可输入任意正数，输出会钳制到 MIDI 的 1–127。v3 会保存力度，v2/legacy 简谱格式没有力度字段。

### 动态 BPM（中途变速）

转换器会从所有 MIDI 轨道收集 tempo 事件，并在 v3 文件中写成全局 `@TEMPO tick us_per_quarter` 表；
播放器按绝对 tick 换算时间，变速不会在轨道之间漂移。`--bpm` 会等比例缩放整张 tempo 表。
`--v2` 输出的旧格式仍保留 `#BPM` 兼容行为：

```
#BPM 120
T Trk
1 2 3 4          # 120 BPM
#BPM 90
5 6 7 1^         # 从此处起 90 BPM
```

| 速度指令 | 说明 |
| -------- | ---- |
| `#BPM 120` | 切换到 120 BPM |
| `90`（旧版纯数字行） | 旧版格式：切换到 90ms/四分音符 |

## 项目对比

| | 原版 `music_release` | 本项目 `music_rust` |
| --- | --- | --- |
| 语言 | C++ | Rust（+ C / x86-64 汇编音频层） |
| 平台 | 仅 Windows x64 | Linux（兼容大多数发行版） |
| 音频后端 | Windows MIDI API (`winmm`) | fluidsynth + SoundFont |
| 音色 | 系统默认 MIDI 音色 | 任意 SoundFont + 可选 GM 音色号 |
| 音轨 | 最多 2 轨（左右手） | 任意多轨并行 |
| 调度精度 | 线程 + `clock()` 忙等 | fluidsynth sequencer 毫秒级 |
| 播放等待 | 忙等 | 事件驱动等待 |
| 交互控制 | ✗ | ✅（方向键/暂停/循环/退出 + 音量 + 鼠标） |
| MIDI 直接播放 | ✗ | ✅（`-m`，原生多轨+变速） |
| 音频文件播放 | ✗ | ✅（WAV/MP3/FLAC/OGG/Opus/AAC/M4A/WMA + 封面） |

## 调试

`-d` 参数会输出（调试模式下不启用 TUI，日志与进度不混排）：

- 解析速度、音轨数量、每轨音符数
- 前 20 条排程事件
- 每个 MIDI 事件的精确时间戳（channel / note-on/off / key / velocity）

```bash
./target/release/music 1.txt -d 2>&1 | head -40
```

随 `music` 一起构建的 `selftest` 会通过完整合成/限制器/ALSA 链路播放 C 大调音阶与和弦，
用来快速验证音频环境：

```bash
cargo build --release --bin selftest
./target/release/selftest
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
