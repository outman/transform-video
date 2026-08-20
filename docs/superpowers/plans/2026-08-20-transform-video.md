# Transform Video 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 构建 macOS / Windows 双平台视频转码桌面工具:gpui-component 绘制 UI,ffmpeg-next 库集成完成 HLS 多档转码(等价既有 Bash 脚本)。

**Architecture:** 转码管线运行在独立 std::thread(demux → decode → libavfilter 多路 scale → 多路 h264 编码 + aac → HLS muxer var_stream_map 输出),进度事件经 channel 回传 gpui 主线程;硬件编码器按平台探测、libx264 回退。FFmpeg 动态库随应用分发。

**Tech Stack:** Rust 2024、gpui / gpui-component(git)、ffmpeg-next(7.x)、rfd、lavfi(集成测试)。

**设计文档:** `docs/superpowers/specs/2026-08-20-transform-video-design.md`

**约定(全计划适用):**

- 命令均在本仓库根目录执行;macOS 开发机需先 `brew install ffmpeg`(版本 6/7/8 均可,ffmpeg-next 自动探测)。
- ffmpeg-next 处于维护模式,个别 API 与本计划代码可能有细微出入。每个涉及 ffmpeg-next 的任务第一步都是 `cargo check`;若签名不匹配,以 vendor 源码(`~/.cargo/registry/src/*/ffmpeg-next-*/src/`)与 docs.rs 为准修正调用点,**不得改变设计语义**。逃生舱:`ffmpeg_next::sys`(即 ffmpeg-sys-next 的 C API 重导出)。
- 集成测试(`#[cfg(test)]` + `tests/` 目录)需要 FFmpeg 库;单元测试不需要,分开放置。

---

## 文件结构总览

```
Cargo.toml                      # 加 ffmpeg-next、rfd、anyhow 等依赖
.gitignore                      # 加 vendor/、dist/
src/
  main.rs                       # gpui 入口、窗口注册
  app_state.rs                  # AppState(gpui Entity)+ 状态机
  ui/mod.rs
  ui/main_window.rs             # 主窗口布局(TitleBar + Root)
  ui/settings.rs                # 输入/输出/参数设置区
  ui/progress.rs                # 进度条、日志、控制按钮
  transcode/mod.rs              # run_job 公开接口
  transcode/job.rs              # JobConfig + 校验
  transcode/event.rs            # TranscodeEvent、channel 封装
  transcode/filters.rs          # filter graph spec 构造
  transcode/hls.rs              # var_stream_map、muxer 选项字典
  transcode/encoders.rs         # 平台编码器候选与探测
  transcode/pipeline.rs         # 转码主循环
scripts/
  vendor-macos.sh               # 从 Homebrew 提取 dylib + 修 install_name
  vendor-windows.ps1            # 下载 BtbN win64-gpl-shared
  package-macos.sh              # 组装 .app
  package-windows.ps1           # exe + dll 打 zip
tests/pipeline_test.rs          # lavfi 集成测试
.github/workflows/ci.yml        # 双平台矩阵
```

---

### Task 1: 依赖与 ffmpeg-next 编译验证(spike)

**Files:**
- Modify: `Cargo.toml`
- Modify: `.gitignore`

- [ ] **Step 1: 加依赖**

`Cargo.toml` 的 `[dependencies]` 追加:

```toml
ffmpeg-next = "7"
rfd = "0.15"
anyhow = "1"
smol = "2"
gpui-component-assets = { git = "https://github.com/longbridge/gpui-component" }
```

`[dev-dependencies]` 追加:

```toml
tempfile = "3"
```

`.gitignore` 追加两行:

```
vendor/
dist/
```

- [ ] **Step 2: 写最小编译验证代码**

`src/main.rs` 整体替换为:

```rust
fn main() {
    // spike: 验证 ffmpeg-next 链接与版本探测
    println!("ffmpeg version: {}", ffmpeg_next::format::version());
    println!("ffmpeg configuration: {}", ffmpeg_next::format::configuration());
    let _ = ffmpeg_next::codec::Id::H264;
}
```

- [ ] **Step 3: 验证编译与运行**

Run: `cargo run`
Expected: 打印 ffmpeg 版本号与编译配置。若报 pkg-config 找不到库:确认 `brew install ffmpeg` 已执行;若版本是 8.x 而 ffmpeg-next 7.x 不支持,把版本改为 `ffmpeg-next = "8"`(如已发布)或用 `brew install ffmpeg@7` 并设置 `PKG_CONFIG_PATH="$(brew --prefix ffmpeg@7)/lib/pkgconfig"`。

- [ ] **Step 4: 记录实际可用的关键 API(不写代码)**

Run: `cargo doc --open -p ffmpeg-next --no-deps`

核对以下五个签名(后续任务依赖),在 PR 描述里记录差异:
1. `ffmpeg_next::util::log::set_level` 与 `Level`
2. `format::context::Output::write_header_with(Dictionary)`
3. `Output::add_stream<E: traits::Encoder>`
4. `codec::encoder::{find, find_by_name}`(encoder 模块的自由函数;若无 `find_by_name`,后续用 `sys::avcodec_find_encoder_by_name` + `codec::Codec` 的 ptr 包装)
5. `filter::Context::{src, sink}` 与 `Source::send_frame` / `Sink::receive_frame`

> **核对结论(实际执行,ffmpeg-next 9.0.0 + Homebrew ffmpeg 9.0,提交 f420d27):**
> 1. `util::log::set_level(Level)`、`Level` —— 存在。
> 2. `Output::write_header_with(Dictionary) -> Result<Dictionary>` —— 存在;按值消耗字典,返回剩余未消费项(可用于发现拼写错误的 muxer 选项)。
> 3. `Output::add_stream<E: traits::Encoder>` —— 存在;`E` 可为 `&str`/`Id`/`Codec`/`Audio`/`Video`。
> 4. `encoder::find(id)`、`encoder::find_by_name(&str)` —— 都存在,无需 sys 兜底。
> 5. **filter 层命名与原计划不同(后续任务按此改写):**
>    - `Context::source()`(不是 `src`);`sink()` 名字一致。
>    - `Source::add(&Frame)`(不是 `send_frame`);EOF 用 `Source::flush()`。
>    - `Sink::frame(&mut Frame) -> Result<(), Error>`(不是 `receive_frame`);**取尽时返回 `Err(EAGAIN)` 而非 `Ok(false)`**,排空循环必须把 EAGAIN 当正常结束。
>    - 9.0 中 frame 趋向统一类型 `ffmpeg_next::Frame`;channel layout 为 `ChannelLayout` 类型。
> - 版本应变已触发:本机 ffmpeg 9.0,`ffmpeg-next = "9"`(7.x/8.x 编译失败);Task 10/11 的 Windows BtbN 版本需与之对齐(9.x),CI 需装 ffmpeg 9 + pkgconf。

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock .gitignore src/main.rs
git commit -m "build: add ffmpeg-next and rfd dependencies with compile spike"
```

---

### Task 2: JobConfig 与校验(TDD)

**Files:**
- Create: `src/lib.rs`
- Create: `src/transcode/mod.rs`
- Create: `src/transcode/job.rs`

- [ ] **Step 1: 写失败测试**

`src/transcode/job.rs` 底部:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_reference_script() {
        let cfg = JobConfig::default();
        let names: Vec<&str> = cfg.variants.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["1080p", "720p", "480p"]);
        assert_eq!(cfg.variants[0].height, 1080);
        assert_eq!(cfg.variants[0].bit_rate_kbps, 4000);
        assert_eq!(cfg.variants[0].max_rate_kbps, 5000);
        assert_eq!(cfg.variants[0].buf_size_kbps, 8000);
        assert_eq!(cfg.variants[1].height, 720);
        assert_eq!(cfg.variants[2].bit_rate_kbps, 1200);
        assert_eq!(cfg.fps, 30);
        assert_eq!(cfg.segment_secs, 10);
        assert_eq!(cfg.audio_bitrate_kbps, 128);
    }

    #[test]
    fn validate_rejects_empty_variants() {
        let mut cfg = JobConfig::default();
        cfg.variants.clear();
        assert_eq!(cfg.validate().unwrap_err(), "至少需要启用一档分辨率");
    }

    #[test]
    fn validate_rejects_zero_fps_and_segment() {
        let mut cfg = JobConfig::default();
        cfg.fps = 0;
        assert!(cfg.validate().is_err());
        cfg = JobConfig::default();
        cfg.segment_secs = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_missing_input() {
        let cfg = JobConfig {
            input: std::path::PathBuf::from("/nonexistent/zzz.mp4"),
            ..JobConfig::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn output_root_is_output_dir_plus_input_stem() {
        let cfg = JobConfig {
            input: std::path::PathBuf::from("/tmp/L3-考点4.mp4"),
            output_dir: std::path::PathBuf::from("/tmp/out"),
            ..JobConfig::default()
        };
        assert_eq!(cfg.output_root(), std::path::PathBuf::from("/tmp/out/L3-考点4"));
    }

    #[test]
    fn segment_prefix_derives_from_input_stem() {
        let cfg = JobConfig {
            input: std::path::PathBuf::from("/tmp/L3-考点4.mp4"),
            ..JobConfig::default()
        };
        assert_eq!(cfg.segment_prefix(), "L3-考点4");
    }

    #[test]
    fn gop_is_fps_times_segment_secs() {
        let cfg = JobConfig::default();
        assert_eq!(cfg.gop(), 300);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib transcode::job 2>&1 | head -20`
Expected: 编译错误(`JobConfig` 未定义)。

- [ ] **Step 3: 实现**

`src/lib.rs`(集成测试需要 lib target;`main.rs` 后续作为 UI 入口引用此 crate):

```rust
pub mod transcode;
```

`src/transcode/mod.rs`:

```rust
pub mod encoders;
pub mod event;
pub mod filters;
pub mod hls;
pub mod job;
pub mod pipeline;

pub use event::{run_job, TranscodeEvent, TranscodeHandle};
pub use job::{JobConfig, VariantSpec};
```

> 注:`encoders`/`event`/`filters`/`hls`/`pipeline` 模块在后续任务创建;本任务先临时注释掉尚未创建的 `pub mod` 行,只保留 `pub mod job;` 与 `pub use job::{JobConfig, VariantSpec};`,后续任务逐个恢复。

`src/transcode/job.rs`:

```rust
use std::path::{Path, PathBuf};

/// 一档视频输出(对应 var_stream_map 中的一个 v 变体)。
#[derive(Debug, Clone, PartialEq)]
pub struct VariantSpec {
    /// 变体名,同时用作输出子目录名,如 "1080p"
    pub name: &'static str,
    /// 目标高度;宽度按比例计算(-2)
    pub height: u32,
    /// 平均码率 kbps
    pub bit_rate_kbps: u32,
    /// maxrate kbps
    pub max_rate_kbps: u32,
    /// bufsize kbps
    pub buf_size_kbps: u32,
}

#[derive(Debug, Clone)]
pub struct JobConfig {
    pub input: PathBuf,
    pub output_dir: PathBuf,
    pub variants: Vec<VariantSpec>,
    pub fps: u32,
    pub segment_secs: u32,
    pub audio_bitrate_kbps: u32,
    /// true 时跳过硬件编码器探测,直接 libx264
    pub force_software: bool,
    /// 强制输入格式名(等价 ffmpeg CLI 的 -f);测试用 "lavfi",生产为 None
    pub input_format: Option<String>,
}

impl Default for JobConfig {
    fn default() -> Self {
        Self {
            input: PathBuf::new(),
            output_dir: PathBuf::new(),
            variants: vec![
                VariantSpec { name: "1080p", height: 1080, bit_rate_kbps: 4000, max_rate_kbps: 5000, buf_size_kbps: 8000 },
                VariantSpec { name: "720p", height: 720, bit_rate_kbps: 2500, max_rate_kbps: 3000, buf_size_kbps: 5000 },
                VariantSpec { name: "480p", height: 480, bit_rate_kbps: 1200, max_rate_kbps: 1500, buf_size_kbps: 2500 },
            ],
            fps: 30,
            segment_secs: 10,
            audio_bitrate_kbps: 128,
            force_software: false,
            input_format: None,
        }
    }
}

impl JobConfig {
    /// 校验配置;Err 返回面向 UI 的中文消息。
    pub fn validate(&self) -> Result<(), String> {
        if self.input.file_name().is_none() {
            return Err("未选择输入文件".into());
        }
        if !self.input.is_file() {
            return Err(format!("输入文件不存在:{}", self.input.display()));
        }
        if self.output_dir.as_os_str().is_empty() {
            return Err("未选择输出目录".into());
        }
        if self.variants.is_empty() {
            return Err("至少需要启用一档分辨率".into());
        }
        if self.fps == 0 {
            return Err("fps 必须大于 0".into());
        }
        if self.segment_secs == 0 {
            return Err("分段时长必须大于 0 秒".into());
        }
        for v in &self.variants {
            if v.bit_rate_kbps == 0 {
                return Err(format!("{} 码率必须大于 0", v.name));
            }
        }
        Ok(())
    }

    /// 输出根目录:<output_dir>/<输入文件名去扩展名>
    pub fn output_root(&self) -> PathBuf {
        let stem = self.input.file_stem().unwrap_or_default();
        self.output_dir.join(stem)
    }

    /// 分段文件名前缀(替代脚本中写死的 hedgie_english_)
    pub fn segment_prefix(&self) -> String {
        self.input.file_stem().unwrap_or_default().to_string_lossy().into_owned()
    }

    /// GOP:keyint 按一个分段时长内的帧数,保证 independent_segments 成立
    pub fn gop(&self) -> u32 {
        self.fps * self.segment_secs
    }

    /// 预创建的子目录列表(变体目录 + 纯音频目录;无音频时不建 audio)
    pub fn variant_dirs(&self, has_audio: bool) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = self.variants.iter().map(|v| self.output_root().join(v.name)).collect();
        if has_audio {
            dirs.push(self.output_root().join("audio"));
        }
        dirs
    }
}
```

- [ ] **Step 4: 恢复 main.rs 占位并跑测试**

`src/main.rs` 暂时改为:

```rust
fn main() {
    println!("ffmpeg version: {}", ffmpeg_next::format::version());
}
```

Run: `cargo test --lib transcode::job`
Expected: 全部 PASS(7 个测试)。

- [ ] **Step 5: Commit**

```bash
git add src/transcode/ src/main.rs
git commit -m "feat(transcode): JobConfig with validation and reference defaults"
```

---

### Task 3: filter graph spec 构造(TDD)

**Files:**
- Create: `src/transcode/filters.rs`
- Modify: `src/transcode/mod.rs`(取消 `pub mod filters;` 注释)

- [ ] **Step 1: 写失败测试**

`src/transcode/filters.rs` 底部:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcode::job::{JobConfig, VariantSpec};

    fn cfg(variants: Vec<VariantSpec>) -> JobConfig {
        JobConfig { variants, ..JobConfig::default() }
    }

    #[test]
    fn video_spec_matches_reference_shape() {
        let job = cfg(JobConfig::default().variants);
        let spec = video_spec(&job);
        // 与脚本对齐:split → scale(等比、偶数宽)→ fps → format
        assert!(spec.contains("[v_in]split=3[v0][v1][v2]"));
        assert!(spec.contains("[v0]scale=w=-2:h=1080:force_original_aspect_ratio=decrease:force_divisible_by=2,fps=30,format=yuv420p[s0]"));
        assert!(spec.contains("[v1]scale=w=-2:h=720:force_original_aspect_ratio=decrease:force_divisible_by=2,fps=30,format=yuv420p[s1]"));
        assert!(spec.contains("[v2]scale=w=-2:h=480:force_original_aspect_ratio=decrease:force_divisible_by=2,fps=30,format=yuv420p[s2]"));
    }

    #[test]
    fn video_spec_follows_enabled_variant_count() {
        let job = cfg(vec![VariantSpec { name: "720p", height: 720, bit_rate_kbps: 2500, max_rate_kbps: 3000, buf_size_kbps: 5000 }]);
        assert!(video_spec(&job).starts_with("[v_in]split=1[v0];"));
    }

    #[test]
    fn audio_spec_resamples_to_48k_stereo() {
        // aformat 自动插入转换,保证 sink 输出 fltp/48k/stereo 与 AAC 编码器一致
        assert_eq!(
            audio_spec(),
            "[a_in]aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo[sa]"
        );
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib transcode::filters 2>&1 | head -10`
Expected: 编译错误(函数未定义)。

- [ ] **Step 3: 实现**

`src/transcode/filters.rs`:

```rust
use crate::transcode::job::JobConfig;

/// 视频 filter_complex 中段(spec 部分)。
/// 输入标签 [v_in](=buffer 源上下文名),输出标签 [s0..sN](=各档 buffersink 上下文名)。
pub fn video_spec(job: &JobConfig) -> String {
    let n = job.variants.len();
    let mut splits: Vec<String> = Vec::with_capacity(n);
    for i in 0..n {
        splits.push(format!("[v{i}]"));
    }
    let mut chains: Vec<String> = Vec::with_capacity(n);
    for (i, v) in job.variants.iter().enumerate() {
        chains.push(format!(
            "[v{i}]scale=w=-2:h={h}:force_original_aspect_ratio=decrease:force_divisible_by=2,fps={fps},format=yuv420p[s{i}]",
            h = v.height,
            fps = job.fps,
        ));
    }
    format!("[0:v]split={n}{};{}", splits.join(""), chains.join(";"))
}

/// 音频 filter 中段:fltp/48k/立体声(aformat 自动插入所需转换,
/// 等价 CLI 的 -ar 48000 -ac 2 加编码器格式协商)。
/// 输入标签 [a_in](=abuffer 源),输出标签 [sa](=abuffersink)。
pub fn audio_spec() -> String {
    "[a_in]aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo[sa]".to_string()
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib transcode::filters`
Expected: PASS(3 个测试)。

- [ ] **Step 5: Commit**

```bash
git add src/transcode/filters.rs src/transcode/mod.rs
git commit -m "feat(transcode): filter graph spec builders"
```

---

### Task 4: HLS muxer 选项与 var_stream_map(TDD)

**Files:**
- Create: `src/transcode/hls.rs`
- Modify: `src/transcode/mod.rs`(取消 `pub mod hls;` 注释)

- [ ] **Step 1: 写失败测试**

`src/transcode/hls.rs` 底部:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcode::job::JobConfig;

    #[test]
    fn var_stream_map_matches_reference() {
        let job = JobConfig::default();
        assert_eq!(
            var_stream_map(&job, true),
            "v:0,agroup:audio,name:1080p v:1,agroup:audio,name:720p v:2,agroup:audio,name:480p a:0,agroup:audio,default:yes,name:audio"
        );
    }

    #[test]
    fn var_stream_map_single_variant() {
        let mut job = JobConfig::default();
        job.variants.truncate(1);
        assert_eq!(
            var_stream_map(&job, true),
            "v:0,agroup:audio,name:1080p a:0,agroup:audio,default:yes,name:audio"
        );
    }

    #[test]
    fn var_stream_map_without_audio() {
        let job = JobConfig::default();
        assert_eq!(
            var_stream_map(&job, false),
            "v:0,agroup:audio,name:1080p v:1,agroup:audio,name:720p v:2,agroup:audio,name:480p"
        );
    }

    #[test]
    fn muxer_options_contains_all_reference_keys() {
        let job = JobConfig {
            input: "/tmp/L3-考点4.mp4".into(),
            output_dir: "/tmp/out".into(),
            ..JobConfig::default()
        };
        let d = muxer_options(&job, true);
        assert_eq!(d.get("hls_time"), Some("10"));
        assert_eq!(d.get("hls_playlist_type"), Some("vod"));
        assert_eq!(d.get("hls_flags"), Some("independent_segments"));
        assert_eq!(d.get("hls_segment_type"), Some("fmp4"));
        assert_eq!(d.get("hls_fmp4_init_filename"), Some("init.mp4"));
        assert_eq!(d.get("master_pl_name"), Some("master.m3u8"));
        assert_eq!(
            d.get("hls_segment_filename").unwrap(),
            "/tmp/out/L3-考点4/%v/L3-考点4_%05d.m4s"
        );
        assert!(d.get("var_stream_map").unwrap().contains("name:1080p"));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib transcode::hls 2>&1 | head -10`
Expected: 编译错误。

- [ ] **Step 3: 实现**

`src/transcode/hls.rs`:

```rust
use ffmpeg_next::util::dict;
use ffmpeg_next::Dictionary;

use crate::transcode::job::JobConfig;

/// 复刻脚本 -var_stream_map:视频流共享 audio 组,纯音频变体默认启用。
/// has_audio=false 时无音频变体(对应 -map 0:a? 的可选语义)。
pub fn var_stream_map(job: &JobConfig, has_audio: bool) -> String {
    let mut parts: Vec<String> = job
        .variants
        .iter()
        .enumerate()
        .map(|(i, v)| format!("v:{i},agroup:audio,name:{}", v.name))
        .collect();
    if has_audio {
        parts.push("a:0,agroup:audio,default:yes,name:audio".to_string());
    }
    parts.join(" ")
}

/// hls muxer 私有选项字典,经 Output::write_header_with 传入。
pub fn muxer_options(job: &JobConfig, has_audio: bool) -> Dictionary {
    let root = job.output_root();
    let seg = root.join("%v").join(format!("{}_%05d.m4s", job.segment_prefix()));
    dict! {
        "hls_time" => job.segment_secs.to_string(),
        "hls_playlist_type" => "vod",
        "hls_flags" => "independent_segments",
        "hls_segment_type" => "fmp4",
        "hls_fmp4_init_filename" => "init.mp4",
        "hls_segment_filename" => seg.to_string_lossy().into_owned(),
        "master_pl_name" => "master.m3u8",
        "var_stream_map" => var_stream_map(job, has_audio),
    }
}

/// 输出 URL 模式:hls muxer 依 %v 生成各变体 playlist。
pub fn output_pattern(job: &JobConfig) -> std::path::PathBuf {
    job.output_root().join("%v").join("index.m3u8")
}
```

> 注:`dict!` 宏若不存在(检查 `ffmpeg_next::util::dict`),改为逐条 `Dictionary::new()` + `d.set(k, v)`(方法名以源码为准)。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib transcode::hls`
Expected: PASS(3 个测试)。

- [ ] **Step 5: Commit**

```bash
git add src/transcode/hls.rs src/transcode/mod.rs
git commit -m "feat(transcode): var_stream_map and HLS muxer options"
```

---

### Task 5: 编码器候选与探测(TDD)

**Files:**
- Create: `src/transcode/encoders.rs`
- Modify: `src/transcode/mod.rs`(取消 `pub mod encoders;` 注释)

- [ ] **Step 1: 写失败测试**

`src/transcode/encoders.rs` 底部:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_match_current_platform() {
        let c = candidates();
        if cfg!(target_os = "macos") {
            assert_eq!(c, vec!["h264_videotoolbox"]);
        } else if cfg!(target_os = "windows") {
            assert_eq!(c, vec!["h264_nvenc", "h264_amf", "h264_qsv", "h264_mf"]);
        } else {
            assert!(c.is_empty());
        }
    }

    #[test]
    fn software_encoder_is_libx264() {
        assert_eq!(SOFTWARE_ENCODER, "libx264");
    }

    #[test]
    fn estimate_width_even_and_proportional() {
        // 16:9 → 1080p/720p
        assert_eq!(estimate_width(1920, 1080, 1080), 1920);
        assert_eq!(estimate_width(1920, 1080, 720), 1280);
        assert_eq!(estimate_width(1920, 1080, 480), 854); // 853.33 → 854(偶数、向上取整)
        // 竖屏 9:16
        assert_eq!(estimate_width(1080, 1920, 1080), 608);
        // 结果永不为 0
        assert!(estimate_width(1, 1000, 480) >= 2);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib transcode::encoders 2>&1 | head -10`
Expected: 编译错误。

- [ ] **Step 3: 实现**

`src/transcode/encoders.rs`:

```rust
use std::ffi::CString;

use ffmpeg_next::sys;

pub const SOFTWARE_ENCODER: &str = "libx264";

/// 当前平台的硬件编码器候选,按优先级排列。
pub fn candidates() -> Vec<&'static str> {
    if cfg!(target_os = "macos") {
        vec!["h264_videotoolbox"]
    } else if cfg!(target_os = "windows") {
        vec!["h264_nvenc", "h264_amf", "h264_qsv", "h264_mf"]
    } else {
        vec![]
    }
}

/// 按输入尺寸与目标高度估算输出宽度(scale 的 force_original_aspect_ratio 行为:
/// 宽度 = in_w * h / in_h,四舍五入到偶数,最小 2)。
pub fn estimate_width(in_w: u32, in_h: u32, target_h: u32) -> u32 {
    let h = target_h.min(in_h.max(2));
    let w = (u64::from(in_w) * u64::from(h) + u64::from(in_h) - 1) / u64::from(in_h);
    (w.max(2) & !1) as u32
}

/// 用实际参数试开编码器:硬件编码器对分辨率/像素格式有限制,
/// find 成功不代表 open 成功,必须试开才算可靠探测。
pub fn probe(name: &str, width: i32, height: i32, bit_rate: i64) -> bool {
    let cname = match CString::new(name) {
        Ok(c) => c,
        Err(_) => return false,
    };
    unsafe {
        let codec = sys::avcodec_find_encoder_by_name(cname.as_ptr());
        if codec.is_null() {
            return false;
        }
        let ctx = sys::avcodec_alloc_context3(codec);
        if ctx.is_null() {
            return false;
        }
        (*ctx).codec_type = sys::AVMediaType_AVMEDIA_TYPE_VIDEO;
        (*ctx).codec_id = (*codec).id;
        (*ctx).width = width;
        (*ctx).height = height;
        (*ctx).pix_fmt = sys::AVPixelFormat_AV_PIX_FMT_YUV420P;
        (*ctx).time_base.num = 1;
        (*ctx).time_base.den = 30;
        (*ctx).bit_rate = bit_rate;
        let ok = sys::avcodec_open2(ctx, codec, std::ptr::null_mut()) == 0;
        sys::avcodec_free_context(&mut ctx);
        ok
    }
}

/// 为一档变体选择编码器:候选逐个试开,全失败回退 libx264。
/// 返回 (编码器名, 是否硬件);日志消息经 log 回调输出。
pub fn choose(
    width: u32,
    height: u32,
    bit_rate_kbps: u32,
    force_software: bool,
    mut log: impl FnMut(String),
) -> (String, bool) {
    if !force_software {
        for name in candidates() {
            if probe(name, width as i32, height as i32, i64::from(bit_rate_kbps) * 1000) {
                log(format!("已启用硬件编码:{name}"));
                return (name.to_string(), true);
            }
        }
        if !candidates().is_empty() {
            log("硬件编码不可用,回退 libx264 软件编码".to_string());
        }
    }
    (SOFTWARE_ENCODER.to_string(), false)
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib transcode::encoders`
Expected: PASS(3 个测试)。

- [ ] **Step 5: Commit**

```bash
git add src/transcode/encoders.rs src/transcode/mod.rs
git commit -m "feat(transcode): per-platform encoder candidates and open-probe"
```

---

### Task 6: 事件定义与 run_job 接口

**Files:**
- Create: `src/transcode/event.rs`
- Modify: `src/transcode/mod.rs`(取消 `pub mod event; pub mod pipeline;` 注释中 event 部分,`pipeline` 仍注释)

- [ ] **Step 1: 实现事件与句柄(纯数据,无逻辑分支可测,直接实现)**

`src/transcode/event.rs`:

```rust
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use smol::channel::Sender;

use crate::transcode::job::JobConfig;

/// 转码阶段,对应 UI 状态机。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Preparing,
    Transcoding,
    Finalizing,
}

/// 转码线程 → UI 线程的事件。
#[derive(Debug, Clone)]
pub enum TranscodeEvent {
    Phase(Phase),
    /// percent ∈ [0,1];eta_secs 为剩余时间估计
    Progress { percent: f64, elapsed_secs: f64, eta_secs: f64 },
    Log(String),
    Done { output_root: PathBuf },
    Canceled,
    Failed(String),
}

/// 取消句柄:UI 持有,转码线程逐帧检查。
#[derive(Clone)]
pub struct TranscodeHandle {
    cancel: Arc<AtomicBool>,
}

impl TranscodeHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_canceled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// 启动转码线程。立即返回句柄;所有结果经 tx 报告,
/// 线程保证恰好发送一个终止事件(Done / Canceled / Failed)。
pub fn run_job(
    config: JobConfig,
    tx: Sender<TranscodeEvent>,
) -> TranscodeHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let handle = TranscodeHandle { cancel: cancel.clone() };
    let tx = std::sync::Mutex::new(tx);

    std::thread::Builder::new()
        .name("transcode".into())
        .spawn(move || {
            let tx = tx.lock().unwrap().clone();
            let _ = tx.try_send(TranscodeEvent::Phase(Phase::Preparing));
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::transcode::pipeline::transcode(&config, &tx, &cancel)
            }));
            let _ = match result {
                Ok(Ok(root)) => tx.try_send(TranscodeEvent::Done { output_root: root }),
                Ok(Err(crate::transcode::pipeline::Outcome::Canceled)) => {
                    tx.try_send(TranscodeEvent::Canceled)
                }
                Ok(Err(crate::transcode::pipeline::Outcome::Failed(msg))) => {
                    tx.try_send(TranscodeEvent::Failed(msg))
                }
                Err(panic) => {
                    let msg = panic
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_else(|| "转码线程发生 panic".to_string());
                    tx.try_send(TranscodeEvent::Failed(msg))
                }
            };
        })
        .expect("spawn transcode thread");
    handle
}
```

- [ ] **Step 2: 补 pipeline 占位,保证编译**

`src/transcode/pipeline.rs`(占位,Task 7 完整实现):

```rust
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;

use crate::transcode::event::TranscodeEvent;

/// 终止方式:Canceled = 用户取消;Failed = 出错(消息已面向用户)。
pub enum Outcome {
    Canceled,
    Failed(String),
}

/// Ok(root) 表示成功;Err 为终止方式。
pub fn transcode(
    _config: &crate::transcode::job::JobConfig,
    _tx: &Sender<TranscodeEvent>,
    _cancel: &AtomicBool,
) -> Result<PathBuf, Outcome> {
    unimplemented!("Task 7 实现")
}
```

- [ ] **Step 3: 验证编译**

Run: `cargo check`
Expected: 无错误(允许 unused 警告)。

- [ ] **Step 4: Commit**

```bash
git add src/transcode/event.rs src/transcode/pipeline.rs src/transcode/mod.rs
git commit -m "feat(transcode): event channel and run_job thread spawn"
```

---

### Task 7: 转码管线主体(TDD,集成测试)

**Files:**
- Rewrite: `src/transcode/pipeline.rs`
- Create: `tests/pipeline_test.rs`
- Modify: `src/transcode/mod.rs`(取消 `pub mod pipeline;` 注释)

> **API 漂移提示(本任务专属):** 下面代码基于 ffmpeg-next 源码核对,但以下调用点仍可能与实际版本有出入,以 `cargo check` 报错为准微调(**语义不变**):
> `Packet::empty()`、`Input::get_packet`、`receive_frame/receive_packet 的 Ok(bool)`、`send_frame(Some/None)`、`encoder::find_by_name`、`set_frame_rate`、`Sample::F32(Type::Planar)`、`ChannelLayout::STEREO`、`add_stream(enc)`、`Encoder::time_base()`、`Dictionary::{set, iter, get}`、`filter::Context::{src, sink}`、`Pixel` 的 Display。
> 任何 setter 在安全层找不到时,用 `sys` 直接写字段(本任务已有多个先例)。

- [ ] **Step 1: 写失败的集成测试**

`tests/pipeline_test.rs`:

```rust
use std::sync::atomic::AtomicBool;

use smol::channel;

use transform_video::transcode::event::TranscodeEvent;
use transform_video::transcode::job::{JobConfig, VariantSpec};
use transform_video::transcode::pipeline;

fn variant_480p() -> VariantSpec {
    VariantSpec { name: "480p", height: 480, bit_rate_kbps: 1200, max_rate_kbps: 1500, buf_size_kbps: 2500 }
}

/// lavfi 虚拟输入:2 秒 320x240@30;with_audio 时再挂一路 48k 正弦波。
fn config(dir: &std::path::Path, with_audio: bool) -> JobConfig {
    let src = if with_audio {
        "testsrc2=duration=2:size=320x240:rate=30;sine=frequency=440:sample_rate=48000:duration=2"
    } else {
        "testsrc2=duration=2:size=320x240:rate=30"
    };
    JobConfig {
        input: src.into(),
        output_dir: dir.into(),
        variants: vec![variant_480p()],
        force_software: true,
        input_format: Some("lavfi".into()),
        ..JobConfig::default()
    }
}

fn run(cfg: &JobConfig, cancel: &AtomicBool) -> (Result<std::path::PathBuf, pipeline::Outcome>, channel::Receiver<TranscodeEvent>) {
    let (tx, rx) = channel::unbounded();
    (pipeline::transcode(cfg, &tx, cancel), rx)
}

#[test]
fn transcodes_single_variant_without_audio() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path(), false);
    let (result, rx) = run(&cfg, &AtomicBool::new(false));
    let root = result.expect("应成功");
    assert!(root.join("master.m3u8").is_file());
    let v = root.join("480p");
    assert!(v.join("index.m3u8").is_file());
    assert!(v.join("init.mp4").is_file());
    assert!(v.read_dir().unwrap().any(|p| p.unwrap().extension().is_some_and(|e| e == "m4s")));
    assert!(!root.join("audio").exists());
    // 进度事件最终达到 100%
    let mut final_pct = 0.0;
    while let Ok(e) = rx.try_recv() {
        if let TranscodeEvent::Progress { percent, .. } = e {
            final_pct = final_pct.max(percent);
        }
    }
    assert!((final_pct - 1.0).abs() < 1e-6, "最终进度应为 100%,实际 {final_pct}");
}

#[test]
fn transcodes_with_audio_variant() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path(), true);
    let (result, _rx) = run(&cfg, &AtomicBool::new(false));
    let root = result.expect("应成功");
    assert!(root.join("audio/index.m3u8").is_file());
    assert!(root.join("audio/init.mp4").is_file());
    // 用 avformat 重开 master,验证两条流(1 视频 + 1 音频)
    let master = ffmpeg_next::format::input(&root.join("master.m3u8")).unwrap();
    assert_eq!(master.streams().count(), 2);
}

#[test]
fn cancel_removes_partial_output() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path(), false);
    let (result, _rx) = run(&cfg, &AtomicBool::new(true)); // 预置取消
    assert!(matches!(result, Err(pipeline::Outcome::Canceled)));
    assert!(!cfg.output_root().exists());
}
```

`Cargo.toml` 的 `[dev-dependencies]` 确认有 `tempfile = "3"`(Task 1 已加)。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --test pipeline_test 2>&1 | head -30`
Expected: FAIL —— panic `unimplemented!("Task 7 实现")`,且 Outcome 未派生 Debug 会先编译报错(下一步实现时一并修复)。

- [ ] **Step 3: 实现 pipeline.rs**

`src/transcode/pipeline.rs` 整体替换为:

```rust
use std::ffi::CString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use smol::channel::Sender;

use anyhow::{anyhow, Context as _};

use ffmpeg_next as ffmpeg;
use ffmpeg::codec;
use ffmpeg::format;
use ffmpeg::frame;
use ffmpeg::packet::Packet;
use ffmpeg::util::rational::Rational;
use ffmpeg::Dictionary;
use ffmpeg::sys;

use crate::transcode::encoders;
use crate::transcode::event::{Phase, TranscodeEvent};
use crate::transcode::filters;
use crate::transcode::hls;
use crate::transcode::job::JobConfig;

#[derive(Debug)]
pub enum Outcome {
    Canceled,
    Failed(String),
}

enum Stop {
    Error(anyhow::Error),
    Canceled,
}

fn send(tx: &Sender<TranscodeEvent>, e: TranscodeEvent) {
    let _ = tx.try_send(e); // unbounded:不阻塞
}

fn avr(r: Rational) -> sys::AVRational {
    sys::AVRational { num: r.numerator() as i32, den: r.denominator() as i32 }
}

/// 颜色标记等价脚本的 bt709 / tv(range=MPEG),写入帧后由编码器带入码流。
fn set_color_tags(frame: &mut frame::Video) {
    unsafe {
        let p = frame.as_mut_ptr();
        (*p).color_primaries = sys::AVColorPrimaries_AVCOL_PRI_BT709;
        (*p).color_trc = sys::AVColorTransferCharacteristic_AVCOL_TRC_BT709;
        (*p).colorspace = sys::AVColorSpace_AVCOL_SPC_BT709;
        (*p).color_range = sys::AVColorRange_AVCOL_RANGE_MPEG;
    }
}

/// 探测阶段拿到的流参数(独立轻量打开一次输入,解码首个视频/音频帧)。
struct Probe {
    width: u32,
    height: u32,
    pix_fmt: format::Pixel,
    time_base: Rational,
    audio: Option<AudioProbe>,
}

struct AudioProbe {
    rate: i32,
    sample_fmt: String,
    layout: String,
}

fn open_input(config: &JobConfig) -> anyhow::Result<format::context::Input> {
    if let Some(f) = &config.input_format {
        let mut d = Dictionary::new();
        d.set("f", f);
        format::input_with_dictionary(&config.input, d).context("无法打开输入")
    } else {
        format::input(&config.input).context("无法打开输入文件")
    }
}

fn channel_layout_string(f: &frame::Audio) -> String {
    // ffmpeg ≥ 5.1 的 ch_layout;本工具支持的构建(brew / BtbN 7.x/8.x)均满足
    unsafe {
        let p = f.as_ptr();
        let mut buf = [0i8; 64];
        if sys::av_channel_layout_describe(&(*p).ch_layout, buf.as_mut_ptr(), buf.len()) >= 0 {
            let s: String = buf.iter().take_while(|&&c| c != 0).map(|&c| c as u8 as char).collect();
            if !s.is_empty() {
                return s;
            }
        }
    }
    "stereo".to_string()
}

fn probe(config: &JobConfig) -> anyhow::Result<Probe> {
    let ictx = open_input(config)?;
    let vstream = ictx
        .streams()
        .best(codec::Type::Video)
        .ok_or_else(|| anyhow!("输入没有视频流"))?;
    let v_idx = vstream.index();
    let astream = ictx.streams().best(codec::Type::Audio);
    let a_idx = astream.as_ref().map(|s| s.index());
    let mut vdec = codec::context::Context::from_parameters(vstream.parameters())?
        .decoder()
        .video()
        .context("打开视频解码器失败")?;
    let mut adec = astream
        .as_ref()
        .and_then(|s| codec::context::Context::from_parameters(s.parameters()).ok())
        .and_then(|c| c.decoder().audio().ok());

    let mut out = Probe {
        width: 0,
        height: 0,
        pix_fmt: format::Pixel::YUV420P,
        time_base: vstream.time_base(),
        audio: None,
    };
    let mut pkt = Packet::empty();
    let mut vframe = frame::Video::empty();
    let mut aframe = frame::Audio::empty();

    // 首个视频帧与音频状态(有帧 / 无流 / 400 包内无帧按无音频处理)
    for _ in 0..400 {
        if out.width > 0 && (out.audio.is_some() || a_idx.is_none()) {
            break;
        }
        if ictx.get_packet(&mut pkt).is_err() {
            break;
        }
        if pkt.stream() == v_idx && out.width == 0 {
            if vdec.send_packet(Some(&pkt)).is_ok() {
                while vdec.receive_frame(&mut vframe).unwrap_or(false) && out.width == 0 {
                    out.width = vframe.width();
                    out.height = vframe.height();
                    out.pix_fmt = vframe.format();
                    out.time_base = vstream.time_base();
                }
            }
        } else if a_idx == Some(pkt.stream()) && out.audio.is_none() {
            if let Some(a) = adec.as_mut() {
                if a.send_packet(Some(&pkt)).is_ok() {
                    while a.receive_frame(&mut aframe).unwrap_or(false) && out.audio.is_none() {
                        out.audio = Some(AudioProbe {
                            rate: aframe.rate(),
                            sample_fmt: format::Sample::name(aframe.format()).to_string(),
                            layout: channel_layout_string(&aframe),
                        });
                    }
                }
            }
        }
    }
    if out.width == 0 {
        return Err(anyhow!("无法解码任何视频帧(文件损坏或格式不支持)"));
    }
    Ok(out)
}

pub fn transcode(
    config: &JobConfig,
    tx: &Sender<TranscodeEvent>,
    cancel: &AtomicBool,
) -> Result<PathBuf, Outcome> {
    match transcode_inner(config, tx, cancel) {
        Ok(root) => Ok(root),
        Err(Stop::Canceled) => {
            let _ = std::fs::remove_dir_all(config.output_root());
            Err(Outcome::Canceled)
        }
        Err(Stop::Error(e)) => {
            let _ = std::fs::remove_dir_all(config.output_root());
            Err(Outcome::Failed(format!("{e:#}")))
        }
    }
}
```

`pipeline.rs` 续(同文件追加):

```rust
struct Pipeline<'a> {
    config: &'a JobConfig,
    tx: &'a Sender<TranscodeEvent>,
    cancel: &'a AtomicBool,
    started: Instant,
    duration_secs: f64,

    octx: format::context::Output,
    graph: ffmpeg::filter::Graph,
    n_variants: usize,
    has_audio_src: bool,   // abuffer 已建(probe.audio 存在)
    has_audio: bool,       // 首个音频 sink 帧已到,将进入输出
    audio_resolved: bool,  // has_audio 定案(首帧到,或判无)

    a_enc: Option<codec::encoder::audio::Encoder>,
    a_stream_idx: Option<usize>,
    v_encs: Vec<Option<codec::encoder::video::Encoder>>,
    v_stream_idx: Vec<Option<usize>>,

    enc_name: String,
    header_written: bool,
    pending: Vec<(Packet, sys::AVRational)>,
    last_report: Instant,
    max_pts_secs: f64,
}

impl<'a> Pipeline<'a> {
    fn check_cancel(&self) -> Result<(), Stop> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(Stop::Canceled)
        } else {
            Ok(())
        }
    }

    /// 视频帧送入滤镜;逐档 sink 取帧编码。
    fn push_video_frame(&mut self, frame: &frame::Video, out: &mut frame::Video) -> Result<(), Stop> {
        self.graph
            .get("v_in")
            .unwrap()
            .src()
            .send_frame(frame)
            .map_err(|e| Stop::Error(anyhow!("滤镜输入失败:{e}")))?;
        self.drain_video_sinks(out)?;
        self.maybe_write_header()
    }

    fn push_audio_frame(&mut self, frame: &frame::Audio, out: &mut frame::Audio) -> Result<(), Stop> {
        if !self.audio_resolved {
            self.audio_resolved = true;
            self.has_audio = true;
        }
        self.graph
            .get("a_in")
            .unwrap()
            .src()
            .send_frame(frame)
            .map_err(|e| Stop::Error(anyhow!("滤镜输入失败:{e}")))?;
        self.drain_audio_sink(out)?;
        self.maybe_write_header()
    }

    fn open_video_encoder(
        &mut self,
        i: usize,
        w: u32,
        h: u32,
        fmt: format::Pixel,
        tb: Rational,
    ) -> Result<(), Stop> {
        let v = &self.config.variants[i];
        let codec = ffmpeg::encoder::find_by_name(&self.enc_name)
            .or_else(|| ffmpeg::encoder::find(codec::Id::H264))
            .ok_or_else(|| Stop::Error(anyhow!("找不到编码器 {}", self.enc_name)))?;
        let mut b = codec::context::Context::new()
            .encoder()
            .video()
            .map_err(|e| Stop::Error(anyhow!("创建视频编码器失败:{e}")))?;
        b.set_width(w);
        b.set_height(h);
        b.set_format(fmt);
        b.set_time_base(tb);
        b.set_frame_rate(Rational::new(self.config.fps as i32, 1));
        b.set_bit_rate(i64::from(v.bit_rate_kbps) * 1000);
        b.set_gop(self.config.gop());
        b.set_flags(codec::flag::GLOBAL_HEADER);
        let mut opts = Dictionary::new();
        opts.set("maxrate", format!("{}k", v.max_rate_kbps));
        opts.set("bufsize", format!("{}k", v.buf_size_kbps));
        opts.set("profile", "high");
        opts.set("keyint_min", self.config.gop().to_string());
        let enc = b
            .open_as_with(codec, opts)
            .map_err(|e| Stop::Error(anyhow!("打开视频编码器失败({}):{e}", self.enc_name)))?;
        self.v_encs[i] = Some(enc);
        Ok(())
    }

    fn encode_video(&mut self, i: usize, frame: &frame::Video) -> Result<(), Stop> {
        let enc_tb = self.v_encs[i].as_ref().unwrap().time_base();
        let enc = self.v_encs[i].as_mut().unwrap();
        enc.send_frame(Some(frame))
            .map_err(|e| Stop::Error(anyhow!("编码视频失败:{e}")))?;
        let mut pkt = Packet::empty();
        loop {
            match enc.receive_packet(&mut pkt) {
                Ok(true) => {
                    let idx = self
                        .v_stream_idx[i]
                        .ok_or_else(|| Stop::Error(anyhow!("内部错误:视频流未创建")))?;
                    self.write_packet(&mut pkt, idx, avr(enc_tb))?;
                }
                Ok(false) => break,
                Err(e) => return Err(Stop::Error(anyhow!("编码视频输出失败:{e}"))),
            }
        }
        Ok(())
    }

    fn encode_audio(&mut self, frame: &mut frame::Audio) -> Result<(), Stop> {
        let enc_tb = Rational::new(1, 48000);
        let enc = self
            .a_enc
            .as_mut()
            .ok_or_else(|| Stop::Error(anyhow!("内部错误:音频编码器未初始化")))?;
        enc.send_frame(Some(frame))
            .map_err(|e| Stop::Error(anyhow!("编码音频失败:{e}")))?;
        let mut pkt = Packet::empty();
        loop {
            match enc.receive_packet(&mut pkt) {
                Ok(true) => {
                    let idx = self
                        .a_stream_idx
                        .ok_or_else(|| Stop::Error(anyhow!("内部错误:音频流未创建")))?;
                    self.write_packet(&mut pkt, idx, avr(enc_tb))?;
                }
                Ok(false) => break,
                Err(e) => return Err(Stop::Error(anyhow!("编码音频输出失败:{e}"))),
            }
        }
        Ok(())
    }

    /// 所有编码器就绪(视频全开 + 音频定案)后:按 v:0..N-1、a:0 顺序建流,写 header。
    fn maybe_write_header(&mut self) -> Result<(), Stop> {
        if self.header_written
            || self.v_encs.iter().any(|e| e.is_none())
            || !self.audio_resolved
        {
            return Ok(());
        }
        for i in 0..self.v_encs.len() {
            let enc = self.v_encs[i].as_ref().unwrap();
            let mut st = self
                .octx
                .add_stream(enc)
                .map_err(|e| Stop::Error(anyhow!("创建输出流失败:{e}")))?;
            st.set_time_base(enc.time_base());
            self.v_stream_idx[i] = Some(st.index());
        }
        if self.has_audio {
            let enc = self.a_enc.as_ref().unwrap();
            let mut st = self
                .octx
                .add_stream(enc)
                .map_err(|e| Stop::Error(anyhow!("创建音频输出流失败:{e}")))?;
            st.set_time_base(Rational::new(1, 48000));
            self.a_stream_idx = Some(st.index());
        }
        let leftover = self
            .octx
            .write_header_with(hls::muxer_options(self.config, self.has_audio))
            .map_err(|e| Stop::Error(anyhow!("初始化 HLS 输出失败:{e}")))?;
        for (k, v) in leftover.iter() {
            send(self.tx, TranscodeEvent::Log(format!("警告:muxer 未识别选项 {k}={v}")));
        }
        self.header_written = true;
        send(self.tx, TranscodeEvent::Phase(Phase::Transcoding));
        self.drain_pending()
    }

    fn stream_tb_avr(&self, idx: usize) -> sys::AVRational {
        self.octx
            .stream(idx)
            .map(|s| avr(s.time_base()))
            .unwrap_or(sys::AVRational { num: 1, den: 90000 })
    }

    /// header 未写时暂存(音频帧可能先于视频到达);写入按流时基重采样。
    fn write_packet(&mut self, pkt: &mut Packet, idx: usize, enc_tb: sys::AVRational) -> Result<(), Stop> {
        unsafe {
            (*pkt.as_mut_ptr()).stream_index = idx as i32;
            (*pkt.as_mut_ptr()).pos = -1;
        }
        if !self.header_written {
            let mut copy = Packet::empty();
            unsafe {
                sys::av_packet_ref(copy.as_mut_ptr(), pkt.as_ptr());
            }
            self.pending.push((copy, enc_tb));
            return Ok(());
        }
        let stb = self.stream_tb_avr(idx);
        let ret = unsafe {
            sys::av_packet_rescale_ts(pkt.as_mut_ptr(), enc_tb, stb);
            sys::av_interleaved_write_frame(self.octx.as_mut_ptr(), pkt.as_mut_ptr())
        };
        if ret < 0 {
            return Err(Stop::Error(anyhow!("写入输出失败(错误码 {ret})")));
        }
        Ok(())
    }

    fn drain_pending(&mut self) -> Result<(), Stop> {
        let pend = std::mem::take(&mut self.pending);
        for (mut pkt, etb) in pend {
            let idx = unsafe { (*pkt.as_ptr()).stream_index } as usize;
            let stb = self.stream_tb_avr(idx);
            let ret = unsafe {
                sys::av_packet_rescale_ts(pkt.as_mut_ptr(), etb, stb);
                sys::av_interleaved_write_frame(self.octx.as_mut_ptr(), pkt.as_mut_ptr())
            };
            if ret < 0 {
                return Err(Stop::Error(anyhow!("写入输出失败(错误码 {ret})")));
            }
        }
        Ok(())
    }

    fn report_progress(&mut self, pts_secs: f64) {
        if pts_secs > self.max_pts_secs {
            self.max_pts_secs = pts_secs;
        }
        if self.last_report.elapsed().as_millis() < 300 {
            return;
        }
        self.last_report = Instant::now();
        let elapsed = self.started.elapsed().as_secs_f64();
        let percent = if self.duration_secs > 0.0 {
            (self.max_pts_secs / self.duration_secs).clamp(0.0, 1.0)
        } else {
            -1.0 // 时长未知:不定进度,UI 显示动画
        };
        let eta = if self.duration_secs > 0.0 && percent > 0.02 {
            elapsed * (1.0 - percent) / percent
        } else {
            -1.0
        };
        send(
            self.tx,
            TranscodeEvent::Progress { percent, elapsed_secs: elapsed, eta_secs: eta },
        );
    }
}
```

`pipeline.rs` 续(第二个 impl 块:主循环与 flush 共用的抽帧/编码收尾方法):

```rust
impl<'a> Pipeline<'a> {
    /// 逐档视频 sink 取帧编码。首个 sink 帧到达时才知道实际宽高,编码器惰性打开。
    fn drain_video_sinks(&mut self, out: &mut frame::Video) -> Result<(), Stop> {
        for i in 0..self.n_variants {
            let name = format!("s{i}");
            loop {
                match self.graph.get(name.as_str()).unwrap().sink().receive_frame(out) {
                    Ok(true) => {
                        let (w, h, fmt) = (out.width(), out.height(), out.format());
                        let tb = out.time_base();
                        if self.v_encs[i].is_none() {
                            self.open_video_encoder(i, w, h, fmt, tb)?;
                        }
                        set_color_tags(out);
                        self.encode_video(i, out)?;
                        let pts_secs = out.pts().unwrap_or(0) as f64
                            * tb.numerator() as f64
                            / tb.denominator() as f64;
                        self.report_progress(pts_secs);
                    }
                    Ok(false) => break,
                    Err(e) => return Err(Stop::Error(anyhow!("滤镜处理失败:{e}"))),
                }
            }
        }
        Ok(())
    }

    fn drain_audio_sink(&mut self, out: &mut frame::Audio) -> Result<(), Stop> {
        loop {
            match self.graph.get("sa").unwrap().sink().receive_frame(out) {
                Ok(true) => self.encode_audio(out)?,
                Ok(false) => break,
                Err(e) => return Err(Stop::Error(anyhow!("滤镜处理失败:{e}"))),
            }
        }
        Ok(())
    }

    /// 逐编码器送 EOF 并清空剩余包(音频流若最终未创建,包直接丢弃)。
    fn flush_encoders(&mut self) -> Result<(), Stop> {
        for i in 0..self.n_variants {
            let tb = avr(self.v_encs[i].as_ref().unwrap().time_base());
            let enc = self.v_encs[i].as_mut().unwrap();
            let _ = enc.send_frame(None);
            let mut pkt = Packet::empty();
            loop {
                match enc.receive_packet(&mut pkt) {
                    Ok(true) => {
                        if let Some(idx) = self.v_stream_idx[i] {
                            self.write_packet(&mut pkt, idx, tb)?;
                        }
                    }
                    Ok(false) => break,
                    Err(_) => break,
                }
            }
        }
        if let Some(enc) = self.a_enc.as_mut() {
            let tb = avr(Rational::new(1, 48000));
            let _ = enc.send_frame(None);
            let mut pkt = Packet::empty();
            loop {
                match enc.receive_packet(&mut pkt) {
                    Ok(true) => {
                        if let Some(idx) = self.a_stream_idx {
                            self.write_packet(&mut pkt, idx, tb)?;
                        }
                    }
                    Ok(false) => break,
                    Err(_) => break,
                }
            }
        }
        Ok(())
    }
}
```

`pipeline.rs` 续(主函数 `transcode_inner`,追加到文件末尾):

```rust
fn transcode_inner(
    config: &JobConfig,
    tx: &Sender<TranscodeEvent>,
    cancel: &AtomicBool,
) -> Result<PathBuf, Stop> {
    let started = Instant::now();
    send(tx, TranscodeEvent::Phase(Phase::Preparing));

    // 0. 探测流参数(独立轻量打开,决定滤镜参数与有无音频)
    let probe = probe(config).map_err(Stop::Error)?;
    let has_audio_src = probe.audio.is_some();

    // 1. 输出目录(UI 已确认覆盖;此处防御性重建)
    let root = config.output_root();
    if root.exists() {
        std::fs::remove_dir_all(&root)
            .map_err(|e| Stop::Error(anyhow!("清理已存在的输出目录失败:{e}")))?;
    }
    for d in config.variant_dirs(has_audio_src) {
        std::fs::create_dir_all(&d)
            .map_err(|e| Stop::Error(anyhow!("创建输出目录失败:{e}")))?;
    }

    // 2. 硬件编码器选择:以最大档的估算尺寸试开
    let biggest = config.variants.iter().max_by_key(|v| v.height).unwrap();
    let est_w = encoders::estimate_width(probe.width, probe.height, biggest.height);
    let (enc_name, _hw) = encoders::choose(
        est_w,
        biggest.height,
        biggest.bit_rate_kbps,
        config.force_software,
        |m| send(tx, TranscodeEvent::Log(m)),
    );

    // 3. 输出 context:hls muxer 自管分段文件(AVFMT_NOFILE),不能 avio_open,
    //    不走 format::output_as_with,手工分配并以 %v 模式 URL 传入
    let octx = {
        let url = CString::new(hls::output_pattern(config).to_string_lossy().into_owned()).unwrap();
        let fmt = CString::new("hls").unwrap();
        unsafe {
            let mut ps = std::ptr::null_mut();
            let ret = sys::avformat_alloc_output_context2(
                &mut ps,
                std::ptr::null_mut(),
                fmt.as_ptr(),
                url.as_ptr(),
            );
            if ret != 0 || ps.is_null() {
                return Err(Stop::Error(anyhow!("创建 HLS 输出失败(错误码 {ret})")));
            }
            format::context::Output::wrap(ps)
        }
    };

    // 4. 音频编码器:固定 aac / fltp / 48k / 立体声,与输入参数无关,提前打开
    let mut a_enc = None;
    if has_audio_src {
        let aac = ffmpeg::encoder::find(codec::Id::AAC)
            .ok_or_else(|| Stop::Error(anyhow!("找不到 AAC 编码器")))?;
        let mut b = codec::context::Context::new()
            .encoder()
            .audio()
            .map_err(|e| Stop::Error(anyhow!("创建音频编码器失败:{e}")))?;
        b.set_format(format::Sample::F32(format::sample::Type::Planar)); // fltp
        b.set_rate(48000);
        b.set_channel_layout(ffmpeg::util::channel_layout::ChannelLayout::STEREO);
        b.set_bit_rate(i64::from(config.audio_bitrate_kbps) * 1000);
        b.set_flags(codec::flag::GLOBAL_HEADER);
        let opened = b
            .open_as(aac)
            .map_err(|e| Stop::Error(anyhow!("打开 AAC 编码器失败:{e}")))?;
        a_enc = Some(opened);
    }

    // 5. filter graph:buffer/abuffer 源 + 各档 sink,Parser 按 spec 标签连接
    let n = config.variants.len();
    let mut graph = ffmpeg::filter::Graph::new();
    let vargs = format!(
        "video_size={w}x{h}:pix_fmt={pix}:time_base={num}/{den}:pixel_aspect=1/1",
        w = probe.width,
        h = probe.height,
        pix = probe.pix_fmt,
        num = probe.time_base.numerator(),
        den = probe.time_base.denominator(),
    );
    graph
        .add(
            &ffmpeg::filter::find("buffer").ok_or(()).map_err(|_| Stop::Error(anyhow!("缺少 buffer 滤镜")))?,
            "v_in",
            &vargs,
        )
        .map_err(|e| Stop::Error(anyhow!("构建视频滤镜源失败:{e}")))?;
    for i in 0..n {
        graph
            .add(&ffmpeg::filter::find("buffersink").unwrap(), &format!("s{i}"), "")
            .map_err(|e| Stop::Error(anyhow!("构建视频滤镜汇失败:{e}")))?;
    }
    let mut spec = filters::video_spec(config);
    if let Some(a) = &probe.audio {
        let aargs = format!(
            "sample_rate={r}:sample_fmt={f}:channel_layout={l}:time_base=1/{r}",
            r = a.rate,
            f = a.sample_fmt,
            l = a.layout,
        );
        graph
            .add(&ffmpeg::filter::find("abuffer").unwrap(), "a_in", &aargs)
            .map_err(|e| Stop::Error(anyhow!("构建音频滤镜源失败:{e}")))?;
        graph
            .add(&ffmpeg::filter::find("abuffersink").unwrap(), "sa", "")
            .map_err(|e| Stop::Error(anyhow!("构建音频滤镜汇失败:{e}")))?;
        spec = format!("{spec};{}", filters::audio_spec());
    }
    {
        // Parser:inputs 列表 = 各 sink(段输出落点),outputs 列表 = 各源(段输入来源)
        let mut p = graph
            .input(format!("s{}", n - 1).as_str(), 0)
            .map_err(|e| Stop::Error(anyhow!("滤镜连接失败:{e}")))?;
        for i in (0..n - 1).rev() {
            p = p
                .input(format!("s{i}").as_str(), 0)
                .map_err(|e| Stop::Error(anyhow!("滤镜连接失败:{e}")))?;
        }
        if has_audio_src {
            p = p.input("sa", 0).map_err(|e| Stop::Error(anyhow!("滤镜连接失败:{e}")))?;
        }
        p = p.output("v_in", 0).map_err(|e| Stop::Error(anyhow!("滤镜连接失败:{e}")))?;
        if has_audio_src {
            p = p.output("a_in", 0).map_err(|e| Stop::Error(anyhow!("滤镜连接失败:{e}")))?;
        }
        p.parse(&spec)
            .map_err(|e| Stop::Error(anyhow!("滤镜解析失败:{e}")))?;
    }
    graph
        .validate()
        .map_err(|e| Stop::Error(anyhow!("滤镜配置失败:{e}")))?;

    // 6. 正式转码遍:重新打开输入
    let mut ictx = open_input(config).map_err(Stop::Error)?;
    let duration_secs = if ictx.duration() > 0 {
        ictx.duration() as f64 / 1_000_000.0
    } else {
        0.0
    };
    let vstream = ictx
        .streams()
        .best(codec::Type::Video)
        .ok_or_else(|| Stop::Error(anyhow!("输入没有视频流")))?;
    let v_idx = vstream.index();
    let astream = if has_audio_src { ictx.streams().best(codec::Type::Audio) } else { None };
    let a_idx = astream.as_ref().map(|s| s.index());
    let mut vdec = codec::context::Context::from_parameters(vstream.parameters())
        .map_err(Stop::Error)?
        .decoder()
        .video()
        .map_err(|e| Stop::Error(anyhow!("打开视频解码器失败:{e}")))?;
    let mut adec = astream
        .as_ref()
        .and_then(|s| codec::context::Context::from_parameters(s.parameters()).ok())
        .and_then(|c| c.decoder().audio().ok());

    let mut pipe = Pipeline {
        config,
        tx,
        cancel,
        started,
        duration_secs,
        octx,
        graph,
        n_variants: n,
        has_audio_src,
        has_audio: false,
        audio_resolved: !has_audio_src,
        a_enc,
        a_stream_idx: None,
        v_encs: (0..n).map(|_| None).collect(),
        v_stream_idx: (0..n).map(|_| None).collect(),
        enc_name,
        header_written: false,
        pending: Vec::new(),
        last_report: Instant::now(),
        max_pts_secs: 0.0,
    };

    // 7. 主循环
    let mut pkt = Packet::empty();
    let mut vframe = frame::Video::empty();
    let mut aframe = frame::Audio::empty();
    let mut out_vframe = frame::Video::empty();
    let mut out_aframe = frame::Audio::empty();
    loop {
        pipe.check_cancel()?;
        match ictx.get_packet(&mut pkt) {
            Ok(()) => {}
            Err(_e) => break, // EOF(读取错误也终止,后续校验会暴露问题)
        }
        if pkt.stream() == v_idx {
            vdec.send_packet(Some(&pkt))
                .map_err(|e| Stop::Error(anyhow!("解码视频失败:{e}")))?;
            while vdec.receive_frame(&mut vframe).unwrap_or(false) {
                pipe.check_cancel()?;
                pipe.push_video_frame(&vframe, &mut out_vframe)?;
            }
        } else if Some(pkt.stream()) == a_idx {
            if let Some(a) = adec.as_mut() {
                a.send_packet(Some(&pkt))
                    .map_err(|e| Stop::Error(anyhow!("解码音频失败:{e}")))?;
                while a.receive_frame(&mut aframe).unwrap_or(false) {
                    pipe.check_cancel()?;
                    pipe.push_audio_frame(&aframe, &mut out_aframe)?;
                }
            }
        }
    }

    // 8. flush:解码器 → 滤镜源 EOF → sink → 编码器 → trailer
    send(tx, TranscodeEvent::Phase(Phase::Finalizing));
    let _ = vdec.send_packet(None);
    while vdec.receive_frame(&mut vframe).unwrap_or(false) {
        pipe.push_video_frame(&vframe, &mut out_vframe)?;
    }
    if let Some(a) = adec.as_mut() {
        let _ = a.send_packet(None);
        while a.receive_frame(&mut aframe).unwrap_or(false) {
            pipe.push_audio_frame(&aframe, &mut out_aframe)?;
        }
    }
    let _ = pipe.graph.get("v_in").unwrap().src().send_frame(None);
    pipe.drain_video_sinks(&mut out_vframe)?;
    if has_audio_src {
        let _ = pipe.graph.get("a_in").unwrap().src().send_frame(None);
        pipe.drain_audio_sink(&mut out_aframe)?;
    }
    // 音频流始终没有产出帧:定案为无音频,补写 header 让视频输出成立
    if !pipe.audio_resolved {
        pipe.audio_resolved = true;
        send(
            tx,
            TranscodeEvent::Log("音频流未能解码,已按无音频输出".to_string()),
        );
    }
    pipe.maybe_write_header()?;
    if !pipe.header_written {
        return Err(Stop::Error(anyhow!("没有产生任何输出")));
    }
    pipe.flush_encoders()?;
    pipe.octx
        .write_trailer()
        .map_err(|e| Stop::Error(anyhow!("收尾输出失败:{e}")))?;
    send(
        tx,
        TranscodeEvent::Progress { percent: 1.0, elapsed_secs: started.elapsed().as_secs_f64(), eta_secs: 0.0 },
    );
    send(
        tx,
        TranscodeEvent::Log(format!("转码完成:{}/master.m3u8", root.display())),
    );
    Ok(root)
}
```

> **说明(设计对应):**
> - 视频编码器在**首个 sink 帧**到达时才知道实际宽高,因此惰性打开;音频参数固定,提前打开。
> - `pending` 缓存 header 之前的编码包(音频帧可能先于视频到达);流索引顺序保证 `v:0..N-1`、`a:0` 与 var_stream_map 一致。
> - 滤镜源 EOF 用 `send_frame(None)`;若安全层类型不匹配,用 `sys::av_buffersrc_add_frame(ptr, null)` 替代。

- [ ] **Step 4: 跑集成测试**

Run: `cargo test --test pipeline_test 2>&1 | tail -20`
Expected: 3 个测试全部 PASS。若 filter 连接方向报错(标签未找到),把 Parser 链中 `.input(...)`/`.output(...)` 的调用对象互换(语义对称,一次试验即定)。

- [ ] **Step 5: 真实文件冒烟测试(可选但推荐)**

准备一个真实 mp4(如 `L3-考点4.mp4`),在 `tests/` 下临时加一个 `#[ignore]` 测试或用 `cargo run` 里的小例子跑 `JobConfig::default()`,确认:
- `master.m3u8` 与各档 `index.m3u8` 可被 Safari/VLC 播放
- `ffprobe master.m3u8` 显示 4 条流(3 视频 + 1 音频,默认参数时)
- 分段为 fMP4(`init.mp4` + `.m4s`)

Run: `cargo test && ffprobe <输出>/master.m3u8`
Expected: 流数正确、播放正常。

- [ ] **Step 6: Commit**

```bash
git add src/transcode/pipeline.rs src/transcode/mod.rs tests/pipeline_test.rs
git commit -m "feat(transcode): full HLS multi-variant pipeline with progress and cancel"
```

---

### Task 8: gpui 应用骨架与 AppState 状态机

**Files:**
- Create: `src/app_state.rs`
- Create: `src/ui/mod.rs`、`src/ui/main_window.rs`(占位)
- Modify: `src/lib.rs`(加 `pub mod app_state; pub mod ui;`)
- Rewrite: `src/main.rs`

> **API 漂移提示:** `cx.spawn(async move |this, cx| ...)` 闭包签名(weak handle + AsyncApp)以当前 gpui git 版本为准;若为旧签名 `move |this, cx: &mut AsyncApp|` 同步调整。

- [ ] **Step 1: 实现 app_state.rs(状态机逻辑,单元可测部分在 Task 9 前手动验证)**

`src/app_state.rs`:

```rust
use std::path::PathBuf;

use gpui::Context;
use smol::channel;

use crate::transcode::event::{Phase, TranscodeEvent, TranscodeHandle};
use crate::transcode::job::{JobConfig, VariantSpec};

/// UI 状态机(Idle/终态由本地管理,过程态跟随 TranscodeEvent::Phase)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Preparing,
    Transcoding,
    Finalizing,
    Done,
    Canceled,
    Failed,
}

pub struct AppState {
    pub input_path: Option<PathBuf>,
    pub output_dir: Option<PathBuf>,
    /// 启用的分辨率档位(下标对应 JobConfig::default().variants)
    pub enabled_variants: [bool; 3],
    /// 各档码率 kbps(仅平均码率可改;maxrate/bufsize 用脚本默认比例)
    pub bitrates_kbps: [u32; 3],
    pub fps: u32,
    pub segment_secs: u32,
    pub audio_bitrate_kbps: u32,
    pub force_software: bool,

    pub status: Status,
    /// -1.0 = 不定(时长未知)
    pub percent: f64,
    pub elapsed_secs: f64,
    pub eta_secs: f64,
    pub logs: Vec<String>,
    pub output_root: Option<PathBuf>,
    handle: Option<TranscodeHandle>,
}

impl Default for AppState {
    fn default() -> Self {
        let d = JobConfig::default();
        Self {
            input_path: None,
            output_dir: None,
            enabled_variants: [true, true, true],
            bitrates_kbps: [
                d.variants[0].bit_rate_kbps,
                d.variants[1].bit_rate_kbps,
                d.variants[2].bit_rate_kbps,
            ],
            fps: d.fps,
            segment_secs: d.segment_secs,
            audio_bitrate_kbps: d.audio_bitrate_kbps,
            force_software: false,
            status: Status::Idle,
            percent: 0.0,
            elapsed_secs: 0.0,
            eta_secs: 0.0,
            logs: Vec::new(),
            output_root: None,
            handle: None,
        }
    }
}

impl AppState {
    pub fn build_config(&self) -> JobConfig {
        let defaults = JobConfig::default();
        let variants: Vec<VariantSpec> = defaults
            .variants
            .iter()
            .enumerate()
            .filter(|(i, _)| self.enabled_variants[*i])
            .map(|(i, v)| VariantSpec {
                name: v.name,
                height: v.height,
                bit_rate_kbps: self.bitrates_kbps[i],
                max_rate_kbps: v.max_rate_kbps,
                buf_size_kbps: v.buf_size_kbps,
            })
            .collect();
        JobConfig {
            input: self.input_path.clone().unwrap_or_default(),
            output_dir: self.output_dir.clone().unwrap_or_default(),
            variants,
            fps: self.fps,
            segment_secs: self.segment_secs,
            audio_bitrate_kbps: self.audio_bitrate_kbps,
            force_software: self.force_software,
            input_format: None,
        }
    }

    pub fn start(&mut self, cx: &mut Context<Self>) {
        let config = self.build_config();
        let (tx, rx) = channel::unbounded();
        self.handle = Some(crate::transcode::run_job(config, tx));
        self.status = Status::Preparing;
        self.percent = 0.0;
        self.logs.clear();
        self.output_root = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                if this.update(cx, |state, cx| state.on_event(event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn on_event(&mut self, event: TranscodeEvent, cx: &mut Context<Self>) {
        match event {
            TranscodeEvent::Phase(p) => {
                self.status = match p {
                    Phase::Preparing => Status::Preparing,
                    Phase::Transcoding => Status::Transcoding,
                    Phase::Finalizing => Status::Finalizing,
                };
            }
            TranscodeEvent::Progress { percent, elapsed_secs, eta_secs } => {
                self.percent = percent;
                self.elapsed_secs = elapsed_secs;
                self.eta_secs = eta_secs;
                if self.status == Status::Preparing {
                    self.status = Status::Transcoding;
                }
            }
            TranscodeEvent::Log(msg) => self.logs.push(msg),
            TranscodeEvent::Done { output_root } => {
                self.status = Status::Done;
                self.output_root = Some(output_root);
                self.percent = 1.0;
                self.handle = None;
            }
            TranscodeEvent::Canceled => {
                self.status = Status::Canceled;
                self.handle = None;
            }
            TranscodeEvent::Failed(msg) => {
                self.logs.push(format!("失败:{msg}"));
                self.status = Status::Failed;
                self.handle = None;
            }
        }
        if self.logs.len() > 2000 {
            let drop = self.logs.len() - 2000;
            self.logs.drain(0..drop);
        }
        cx.notify();
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(h) = &self.handle {
            h.cancel();
        }
        cx.notify();
    }

    pub fn busy(&self) -> bool {
        self.handle.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_config_follows_enabled_variants_and_bitrates() {
        let mut s = AppState::default();
        s.enabled_variants = [false, true, false];
        s.bitrates_kbps = [0, 2600, 0];
        s.input_path = Some("/tmp/a.mp4".into());
        s.output_dir = Some("/tmp/o".into());
        let cfg = s.build_config();
        assert_eq!(cfg.variants.len(), 1);
        assert_eq!(cfg.variants[0].name, "720p");
        assert_eq!(cfg.variants[0].bit_rate_kbps, 2600);
        assert_eq!(cfg.variants[0].max_rate_kbps, 3000);
        assert!(matches!(cfg.validate(), Ok(())));
    }

    #[test]
    fn build_config_invalid_without_variants() {
        let mut s = AppState::default();
        s.enabled_variants = [false, false, false];
        assert!(s.build_config().validate().is_err());
    }
}
```

- [ ] **Step 2: 窗口骨架**

`src/ui/mod.rs`:

```rust
pub mod main_window;
```

`src/ui/main_window.rs`(Task 9 完整化):

```rust
use gpui::prelude::*;
use gpui::*;

use gpui_component::{button::Button, ActiveTheme, Root, TitleBar};

use crate::app_state::{AppState, Status};

pub struct MainWindow {
    state: Entity<AppState>,
}

impl MainWindow {
    pub fn new(_window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|_| Self { state: cx.new(|_| AppState::default()) })
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.state.read(cx).status;
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .child(TitleBar::new().child(div().child("Transform Video")))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(format!("{status:?}")),
            )
    }
}
```

`src/lib.rs` 更新为:

```rust
pub mod app_state;
pub mod transcode;
pub mod ui;
```

`src/main.rs` 整体替换为:

```rust
use gpui::*;

use gpui_component::TitleBar;

use transform_video::ui::main_window::MainWindow;

fn main() {
    let app = gpui_platform::application().with_assets(gpui_component_assets::Assets);
    app.run(move |cx| {
        gpui_component::init(cx);
        cx.spawn(async move |cx| {
            let mut opts = TitleBar::window_options();
            opts.titlebar = Some(TitleBarOptions {
                title: Some("Transform Video".into()),
                ..Default::default()
            });
            let window = cx
                .open_window(opts, |window, cx| {
                    let view = MainWindow::new(window, cx);
                    cx.new(|cx| gpui_component::Root::new(view, window, cx))
                })
                .expect("打开窗口失败");
            window
                .update(cx, |_, window, cx| {
                    cx.activate(true);
                    window.activate_window();
                })
                .expect("激活窗口失败");
        })
        .detach();
    });
}
```

> 注:`open_window` 返回的句柄激活写法以 gpui 当前 API 为准;编译不过时用 examples 里 `gpui-component` 仓库 `examples/window_title` 的写法替换(效果:窗口获得焦点)。

- [ ] **Step 3: 验证编译与单元测试**

Run: `cargo test --lib && cargo build`
Expected: 单元测试 PASS(含 Task 8 新增 2 个);构建成功。

- [ ] **Step 4: 运行看窗口**

Run: `cargo run`
Expected: 出现带自绘标题栏 "Transform Video" 的窗口,正文显示 `Idle`。关闭正常。

- [ ] **Step 5: Commit**

```bash
git add src/ Cargo.toml Cargo.lock
git commit -m "feat(ui): gpui app skeleton with AppState state machine"
```

---

### Task 9: 完整 UI(设置 / 进度 / 文件选择)

**Files:**
- Rewrite: `src/ui/main_window.rs`
- Rewrite: `src/ui/settings.rs`
- Create: `src/ui/progress.rs`
- Modify: `src/ui/mod.rs`(加 `pub mod progress; pub mod settings;`)

> **API 说明:** rfd 的 `FileDialog::pick_file()` 与 `MessageDialog::show()` 在 macOS 上必须在主线程调用 —— gpui 的点击回调就在主线程,同步调用即可(modal 期间 gpui 循环阻塞属正常行为)。

- [ ] **Step 1: main_window.rs 完整实现**

```rust
use gpui::prelude::*;
use gpui::*;

use gpui_component::input::{InputEvent, InputState, NumberInput};
use gpui_component::select::{SelectEvent, SelectState};
use gpui_component::{button::Button, ActiveTheme, TitleBar};

use crate::app_state::AppState;
use crate::ui::{progress, settings};

const ENCODER_AUTO: &str = "自动(硬件优先)";
const ENCODER_SOFTWARE: &str = "强制软编(libx264)";

pub struct MainWindow {
    pub state: Entity<AppState>,
    pub fps_input: Entity<InputState>,
    pub seg_input: Entity<InputState>,
    pub audio_input: Entity<InputState>,
    pub encoder_select: Entity<SelectState<String>>,
}

impl MainWindow {
    pub fn new(window: &mut Window, cx: &mut App) -> Entity<Self> {
        let state = cx.new(|_| AppState::default());
        let fps_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("fps").default_value("30").min(1.).max(240.)
        });
        let seg_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("秒").default_value("10").min(1.).max(60.)
        });
        let audio_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("kbps").default_value("128").min(32.).max(512.)
        });
        let encoder_select = cx.new(|cx| {
            SelectState::new(
                vec![ENCODER_AUTO.to_string(), ENCODER_SOFTWARE.to_string()],
                Some(IndexPath::default()),
                window,
                cx,
            )
        });

        cx.new(|cx| {
            cx.subscribe_in(&fps_input, window, |v, input, e: &InputEvent, _w, cx| {
                if matches!(e, InputEvent::Change) {
                    if let Ok(n) = input.read(cx).unmask_value().parse::<u32>() {
                        v.state.update(cx, |s, _| s.fps = n.clamp(1, 240));
                    }
                }
            });
            cx.subscribe_in(&seg_input, window, |v, input, e: &InputEvent, _w, cx| {
                if matches!(e, InputEvent::Change) {
                    if let Ok(n) = input.read(cx).unmask_value().parse::<u32>() {
                        v.state.update(cx, |s, _| s.segment_secs = n.clamp(1, 60));
                    }
                }
            });
            cx.subscribe_in(&audio_input, window, |v, input, e: &InputEvent, _w, cx| {
                if matches!(e, InputEvent::Change) {
                    if let Ok(n) = input.read(cx).unmask_value().parse::<u32>() {
                        v.state.update(cx, |s, _| s.audio_bitrate_kbps = n.clamp(32, 512));
                    }
                }
            });
            cx.subscribe_in(&encoder_select, window, |v, sel, e: &SelectEvent<String>, _w, cx| {
                if let SelectEvent::Confirm(Some(val)) = e {
                    let software = val == ENCODER_SOFTWARE;
                    v.state.update(cx, |s, _| s.force_software = software);
                }
            });
            Self { state, fps_input, seg_input, audio_input, encoder_select }
        })
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .child(TitleBar::new().child(div().child("Transform Video")))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .p_4()
                    .gap_4()
                    .overflow_y_scroll()
                    .child(settings::render(self, window, cx))
                    .child(progress::render(self, cx)),
            )
    }
}
```

- [ ] **Step 2: settings.rs**

```rust
use gpui::prelude::*;
use gpui::*;

use gpui_component::button::Button;
use gpui_component::checkbox::Checkbox;
use gpui_component::input::NumberInput;
use gpui_component::select::Select;

use super::main_window::MainWindow;

fn row(label: &str, control: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(div().w(px(80.)).text_right().child(label.to_string()))
        .child(control)
}

pub fn render(v: &MainWindow, _window: &mut Window, cx: &mut Context<MainWindow>) -> impl IntoElement {
    let s = v.state.read(cx);
    let busy = s.busy();
    let input_text = s
        .input_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "未选择".into());
    let output_text = s
        .output_dir
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "未选择".into());

    let mut variants = div().flex().flex_col().gap_2();
    for (i, (name, _height)) in [("1080p", 1080usize), ("720p", 720), ("480p", 480)].iter().enumerate() {
        variants = variants.child(
            div().flex().items_center().gap_3().child(
                Checkbox::new(("variant", *name))
                    .label(*name)
                    .checked(s.enabled_variants[i])
                    .disabled(busy)
                    .on_click(cx.listener(move |v, checked: &bool, _, cx| {
                        v.state.update(cx, |s, _| s.enabled_variants[i] = *checked);
                        cx.notify();
                    })),
            ),
        );
    }

    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(row("输入文件", div().flex().gap_2().child(div().flex_1().child(input_text)).child(
            Button::new("pick-input").label("选择…").disabled(busy).on_click(cx.listener(|v, _, _, cx| {
                let picked = rfd::FileDialog::new()
                    .add_filter("视频文件", &["mp4", "mov", "mkv", "avi", "m4v", "ts", "webm", "flv"])
                    .pick_file();
                if let Some(p) = picked {
                    v.state.update(cx, |s, _| s.input_path = Some(p));
                    cx.notify();
                }
            })),
        )))
        .child(row("输出目录", div().flex().gap_2().child(div().flex_1().child(output_text)).child(
            Button::new("pick-output").label("选择…").disabled(busy).on_click(cx.listener(|v, _, _, cx| {
                let picked = rfd::FileDialog::new().pick_folder();
                if let Some(p) = picked {
                    v.state.update(cx, |s, _| s.output_dir = Some(p));
                    cx.notify();
                }
            })),
        )))
        .child(row("分辨率档", variants))
        .child(row("fps", NumberInput::new(&v.fps_input).disabled(busy).w(px(120.))))
        .child(row("分段时长(秒)", NumberInput::new(&v.seg_input).disabled(busy).w(px(120.))))
        .child(row("音频码率(kbps)", NumberInput::new(&v.audio_input).disabled(busy).w(px(120.))))
        .child(row("编码器", Select::new(&v.encoder_select).disabled(busy).w(px(200.))))
}
```

> 注:`Checkbox::new(id)` 的 id 若要求 `ElementId` 而非元组,改为 `Checkbox::new(SharedString::from(format!("variant-{name}")))`。`_height` 未用可去。

- [ ] **Step 3: progress.rs**

```rust
use std::fmt::Write as _;

use gpui::prelude::*;
use gpui::*;

use gpui_component::button::Button;
use gpui_component::progress::Progress;

use super::main_window::MainWindow;
use crate::app_state::Status;

fn mmss(secs: f64) -> String {
    let secs = secs.max(0.0) as u64;
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

pub fn render(v: &MainWindow, cx: &mut Context<MainWindow>) -> impl IntoElement {
    let s = v.state.read(cx);
    let busy = s.busy();

    let progress_bar = if s.percent < 0.0 {
        Progress::new("progress").loading(true)
    } else {
        Progress::new("progress").value((s.percent * 100.0) as f32)
    };

    let status_text = match s.status {
        Status::Idle => "就绪".to_string(),
        Status::Preparing => "准备中…".to_string(),
        Status::Transcoding => {
            if s.percent >= 0.0 {
                format!(
                    "转码中 {:.0}%  已用 {}  剩余 {}",
                    s.percent * 100.0,
                    mmss(s.elapsed_secs),
                    if s.eta_secs >= 0.0 { mmss(s.eta_secs) } else { "--:--".into() },
                )
            } else {
                "转码中…".to_string()
            }
        }
        Status::Finalizing => "收尾中…".to_string(),
        Status::Done => "已完成".to_string(),
        Status::Canceled => "已取消".to_string(),
        Status::Failed => "失败(见日志)".to_string(),
    };

    let action = if busy {
        Button::new("cancel")
            .danger()
            .label("取消")
            .on_click(cx.listener(|v, _, _, cx| v.state.update(cx, |s, cx| s.cancel(cx))))
    } else {
        Button::new("start").primary().label("开始转码").on_click(cx.listener(|v, _, _, cx| {
            let cfg = v.state.update(cx, |s, _| s.build_config());
            if let Err(msg) = cfg.validate() {
                v.state.update(cx, |s, _| s.logs.push(msg));
                cx.notify();
                return;
            }
            let target = cfg.output_root();
            if target.exists() {
                let ok = rfd::MessageDialog::new()
                    .with_title("覆盖确认")
                    .with_description(format!(
                        "输出目录 {} 已存在,将删除后重建。是否继续?",
                        target.display()
                    ))
                    .with_buttons(rfd::MessageButtons::YesNo)
                    .show();
                if !ok {
                    return;
                }
            }
            v.state.update(cx, |s, cx| s.start(cx));
        }))
    };

    let open_dir = (s.status == Status::Done && s.output_root.is_some()).then(|| {
        let path = s.output_root.clone().unwrap();
        Button::new("open-dir")
            .label("打开输出目录")
            .on_click(move |_, _, _| crate::ui::open_in_file_manager(&path))
    });

    let logs = div()
        .flex()
        .flex_col()
        .gap_1()
        .max_h(px(160.))
        .overflow_y_scroll()
        .text_size(px(12.))
        .children(s.logs.iter().map(|l| {
            let mut line = String::new();
            let _ = write!(line, "▸ {l}");
            div().child(line)
        }));

    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(progress_bar)
        .child(div().flex().items_center().gap_3().child(status_text).child(action).children(open_dir))
        .child(logs)
}
```

`src/ui/mod.rs` 更新:

```rust
pub mod main_window;
pub mod progress;
pub mod settings;

use std::path::Path;

/// 在系统文件管理器中打开目录(转码完成后用)。
pub fn open_in_file_manager(path: &Path) {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let result = std::process::Command::new("xdg-open").arg(path).spawn();
    let _ = result;
}
```

> 注:开启系统文件管理器属操作系统交互,与「不调用 ffmpeg 命令行」的约束无关。

- [ ] **Step 4: 编译并手动验证**

Run: `cargo build`
Expected: 编译通过(个别组件 builder 名以 `cargo check` 提示为准微调,布局结构不变)。

Run: `cargo run`
手动验证清单:
1. 选择文件 / 选择目录按钮弹出系统对话框
2. 勾选/取消分辨率档,修改 fps、分段时长、码率
3. 不选文件点「开始转码」→ 日志提示「未选择输入文件」
4. 选一个真实视频转码 → 进度条推进、日志显示硬件编码器选择、完成后出现「打开输出目录」
5. 转码中点「取消」→ 输出目录被清理,状态变「已取消」

- [ ] **Step 5: Commit**

```bash
git add src/ui/
git commit -m "feat(ui): settings and progress UI with file pickers and cancel"
```

---

### Task 10: FFmpeg 取库脚本

**Files:**
- Create: `scripts/vendor-macos.sh`
- Create: `scripts/vendor-windows.ps1`

- [ ] **Step 1: macOS 取库脚本**

`scripts/vendor-macos.sh`(可执行 `chmod +x`):

```bash
#!/usr/bin/env bash
# 从 Homebrew 提取 FFmpeg 动态库到 vendor/macos/,install_name 全部改 @rpath。
# 前置:brew install ffmpeg
set -euo pipefail

FF="$(brew --prefix ffmpeg)"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VENDOR="$ROOT/vendor/macos"
rm -rf "$VENDOR"
mkdir -p "$VENDOR/lib" "$VENDOR/include" "$VENDOR/lib/pkgconfig"

# 直接需要的库(av* 六件套 + libx264)
for lib in libavcodec libavformat libavfilter libavutil libswresample libswscale libx264; do
  dylib="$(ls "$FF"/lib/${lib}.*.dylib | head -1)"
  cp "$dylib" "$VENDOR/lib/"
done

# 闭包拷贝:brew 前缀下的传递依赖(GPL 构建会带 aom/dav1d/freetype 等)
brew_dep() { otool -L "$1" | tail -n +2 | awk '{print $1}' | grep -E '^(/opt/homebrew|/usr/local)/' || true; }
changed=1
while [ "$changed" -eq 1 ]; do
  changed=0
  for f in "$VENDOR/lib"/*.dylib; do
    while IFS= read -r dep; do
      base="$(basename "$dep")"
      [ -f "$VENDOR/lib/$base" ] && continue
      cp "$dep" "$VENDOR/lib/"
      changed=1
    done < <(brew_dep "$f")
  done
done

# 改 id 与引用为 @rpath,重签名
for f in "$VENDOR/lib"/*.dylib; do
  base="$(basename "$f")"
  install_name_tool -id "@rpath/$base" "$f"
  while IFS= read -r dep; do
    depbase="$(basename "$dep")"
    [ -f "$VENDOR/lib/$depbase" ] || continue
    install_name_tool -change "$dep" "@rpath/$depbase" "$f"
  done < <(brew_dep "$f")
  codesign --force --sign - "$f"
done

# 头文件与 .pc(开发期 FFMPEG_DIR 构建用)
cp -R "$FF/include/"* "$VENDOR/include/"
cp "$FF"/lib/pkgconfig/*.pc "$VENDOR/lib/pkgconfig/"

echo "完成:$VENDOR"
echo "构建:FFMPEG_DIR=$VENDOR cargo build --release"
```

- [ ] **Step 2: 验证 vendor 构建可用**

Run:
```bash
./scripts/vendor-macos.sh && FFMPEG_DIR="$PWD/vendor/macos" cargo build --release
```
Expected: 构建成功。再 `FFMPEG_DIR=... cargo test` 全绿(说明头文件/库一致)。

- [ ] **Step 3: Windows 取库脚本**

`scripts/vendor-windows.ps1`:

```powershell
# 下载 BtbN 的 win64 GPL shared 构建(含 nvenc/amf/qsv/mf 硬编与 libx264)到 vendor/windows。
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$vendor = Join-Path $root "vendor/windows"
$url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n7.1-latest-win64-gpl-shared-7.1.zip"

$zip = Join-Path $env:TEMP "ffmpeg-win64-gpl-shared.zip"
$un = Join-Path $env:TEMP "ffmpeg-win64-gpl-shared"
Invoke-WebRequest -Uri $url -OutFile $zip
Remove-Item -Recurse -Force $un -ErrorAction Ignore
Expand-Archive -Path $zip -DestinationPath $un -Force
$src = (Get-ChildItem $un -Directory | Select-Object -First 1).FullName

Remove-Item -Recurse -Force $vendor -ErrorAction Ignore
New-Item -ItemType Directory -Force -Path $vendor | Out-Null
Copy-Item -Recurse (Join-Path $src "bin") (Join-Path $vendor "bin")
Copy-Item -Recurse (Join-Path $src "lib") (Join-Path $vendor "lib")
Copy-Item -Recurse (Join-Path $src "include") (Join-Path $vendor "include")

Write-Host "完成:$vendor"
Write-Host "构建:`$env:FFMPEG_DIR = `"$vendor`"; cargo build --release"
```

> Windows 侧验证由 CI(Task 11)执行;本地若有 Windows 环境同样跑 Step 2 的等价命令。

- [ ] **Step 4: Commit**

```bash
chmod +x scripts/*.sh
git add scripts/
git commit -m "build: vendor scripts for macOS and Windows FFmpeg libs"
```

---

### Task 11: 打包脚本与双平台 CI

**Files:**
- Create: `scripts/package-macos.sh`
- Create: `scripts/package-windows.ps1`
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: macOS 打包(组装 .app)**

`scripts/package-macos.sh`(可执行):

```bash
#!/usr/bin/env bash
# 前置:FFMPEG_DIR 指向 vendor/macos 的 release 构建(见 vendor-macos.sh)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/dist/Transform Video.app"
rm -rf "$APP" "$ROOT/dist/transform-video-macos.zip"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks"

cp "$ROOT/target/release/transform-video" "$APP/Contents/MacOS/TransformVideo"
cp "$ROOT"/vendor/macos/lib/*.dylib "$APP/Contents/Frameworks/"
install_name_tool -add_rpath "@executable_path/../Frameworks" "$APP/Contents/MacOS/TransformVideo"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>TransformVideo</string>
  <key>CFBundleIdentifier</key><string>com.outman.transform-video</string>
  <key>CFBundleName</key><string>Transform Video</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$APP"
(cd "$ROOT/dist" && zip -qry "transform-video-macos.zip" "Transform Video.app")
echo "打包完成:$ROOT/dist/transform-video-macos.zip"
```

- [ ] **Step 2: Windows 打包(exe + dll → zip)**

`scripts/package-windows.ps1`:

```powershell
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$dist = Join-Path $root "dist/transform-video-win64"
Remove-Item -Recurse -Force $dist -ErrorAction Ignore
New-Item -ItemType Directory -Force -Path $dist | Out-Null
Copy-Item "$root/target/release/transform-video.exe" $dist
Copy-Item "$root/vendor/windows/bin/*.dll" $dist
if (Test-Path "$root/dist/transform-video-win64.zip") { Remove-Item "$root/dist/transform-video-win64.zip" }
Compress-Archive -Path $dist -DestinationPath "$root/dist/transform-video-win64.zip"
Write-Host "打包完成:$root/dist/transform-video-win64.zip"
```

- [ ] **Step 3: CI**

`.github/workflows/ci.yml`:

```yaml
name: CI
on:
  push:
    branches: [main]
  pull_request:

jobs:
  macos:
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: brew install ffmpeg
      - run: cargo test --all
      - run: cargo build --release
      - run: ./scripts/vendor-macos.sh
      - run: FFMPEG_DIR="$GITHUB_WORKSPACE/vendor/macos" cargo build --release
      - run: ./scripts/package-macos.sh
      - uses: actions/upload-artifact@v4
        with:
          name: transform-video-macos
          path: dist/transform-video-macos.zip

  windows:
    runs-on: windows-latest
    env:
      FFMPEG_DIR: ${{ github.workspace }}\vendor\windows
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: pwsh ./scripts/vendor-windows.ps1
      - run: cargo test --all
      - run: cargo build --release
      - run: pwsh ./scripts/package-windows.ps1
      - uses: actions/upload-artifact@v4
        with:
          name: transform-video-win64
          path: dist/transform-video-win64.zip
```

- [ ] **Step 4: 推送验证 CI**

Run: `git push` 后在 GitHub Actions 观察两个 job 均绿、产物可下载。
Expected: macos / windows job 全部通过。

- [ ] **Step 5: Commit**

```bash
chmod +x scripts/*.sh
git add scripts/ .github/
git commit -m "build: app packaging scripts and dual-platform CI"
```

---

### Task 12: README、LICENSE 与硬编验证清单

**Files:**
- Create: `README.md`
- Create: `LICENSE`

- [ ] **Step 1: LICENSE(GPL-3.0)**

Run: `curl -o LICENSE https://www.gnu.org/licenses/gpl-3.0.txt`
(FFmpeg GPL 构建 + libx264,分发即需 GPL,仓库开源发布满足要求。)

- [ ] **Step 2: README**

```markdown
# Transform Video

跨平台(macOS / Windows)视频转 HLS 多档码率的桌面工具。界面基于
[gpui-component](https://github.com/longbridge/gpui-component),转码通过
FFmpeg 库集成完成(不调用 ffmpeg 命令行)。

## 功能

- 单输入 → HLS fMP4 多档输出:1080p / 720p / 480p + 纯音频,master playlist
- 参数可调:分辨率档、各档码率、fps、分段时长、音频码率
- 硬件编码自动探测:macOS VideoToolbox;Windows nvenc / amf / qsv / mf;
  均不可用时回退 libx264(可强制软编)
- 进度 / 剩余时间、取消(清理未完成分段)、日志、完成后打开输出目录

## 开发

依赖:Rust stable;macOS 需 `brew install ffmpeg`(构建链接用)。

    cargo test
    cargo run

发布构建(macOS,链接 vendor 库):

    ./scripts/vendor-macos.sh
    FFMPEG_DIR="$PWD/vendor/macos" cargo build --release
    ./scripts/package-macos.sh

Windows(在 PowerShell):

    pwsh ./scripts/vendor-windows.ps1
    $env:FFMPEG_DIR = "$PWD\vendor\windows"; cargo build --release
    pwsh ./scripts/package-windows.ps1

## 许可

GPL-3.0(含 FFmpeg GPL 构建与 libx264)。
```

- [ ] **Step 3: 手动硬编验证清单(记录到 PR 描述)**

| 平台 | 场景 | 期望日志 |
| --- | --- | --- |
| macOS(Apple Silicon / Intel) | 默认自动 | 已启用硬件编码:h264_videotoolbox |
| macOS | 强制软编 | (无硬件日志,libx264 输出) |
| Windows + NVIDIA | 默认自动 | 已启用硬件编码:h264_nvenc |
| Windows + AMD | 默认自动 | 已启用硬件编码:h264_amf |
| Windows + Intel 核显 | 默认自动 | 已启用硬件编码:h264_qsv |
| Windows 无独显 | 默认自动 | 硬件编码不可用,回退 libx264 软件编码 |

CI 覆盖编译与功能测试;硬编矩阵按上表人工抽测。

- [ ] **Step 4: Commit**

```bash
git add README.md LICENSE
git commit -m "docs: README and GPL license"
```
