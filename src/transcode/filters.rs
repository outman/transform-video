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
    format!("[v_in]split={n}{};{}", splits.join(""), chains.join(";"))
}

/// 音频 filter 中段:fltp/48k/立体声(aformat 自动插入所需转换,
/// 等价 CLI 的 -ar 48000 -ac 2 加编码器格式协商)。
/// 输入标签 [a_in](=abuffer 源),输出标签 [sa](=abuffersink)。
pub fn audio_spec() -> String {
    "[a_in]aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo[sa]".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcode::job::{JobConfig, VariantSpec};

    fn cfg(variants: Vec<VariantSpec>) -> JobConfig {
        JobConfig {
            variants,
            ..JobConfig::default()
        }
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
        let job = cfg(vec![VariantSpec {
            name: "720p",
            height: 720,
            bit_rate_kbps: 2500,
            max_rate_kbps: 3000,
            buf_size_kbps: 5000,
        }]);
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
