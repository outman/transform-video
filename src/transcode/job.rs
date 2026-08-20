use std::path::PathBuf;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_reference_script() {
        let cfg = JobConfig::default();
        let names: Vec<&str> = cfg.variants.iter().map(|v| v.name).collect();
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

    /// 构造一个通过 input/output_dir 前置校验的配置,便于测试后续分支。
    fn cfg_with_input(path: &std::path::Path) -> JobConfig {
        JobConfig {
            input: path.to_path_buf(),
            output_dir: std::path::PathBuf::from("/tmp"),
            ..JobConfig::default()
        }
    }

    #[test]
    fn validate_rejects_empty_variants() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut cfg = cfg_with_input(file.path());
        cfg.variants.clear();
        assert_eq!(cfg.validate().unwrap_err(), "至少需要启用一档分辨率");
    }

    #[test]
    fn validate_rejects_zero_fps_and_segment() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut cfg = cfg_with_input(file.path());
        cfg.fps = 0;
        assert!(cfg.validate().is_err());
        let mut cfg = cfg_with_input(file.path());
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
