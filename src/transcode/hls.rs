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
    let seg = root
        .join("%v")
        .join(format!("{}_%05d.m4s", job.segment_prefix()))
        .to_string_lossy()
        .into_owned();
    let vsm = var_stream_map(job, has_audio);
    let mut d = Dictionary::new();
    d.set("hls_time", &job.segment_secs.to_string());
    d.set("hls_playlist_type", "vod");
    d.set("hls_flags", "independent_segments");
    d.set("hls_segment_type", "fmp4");
    d.set("hls_fmp4_init_filename", "init.mp4");
    d.set("hls_segment_filename", &seg);
    d.set("master_pl_name", "master.m3u8");
    d.set("var_stream_map", &vsm);
    d
}

/// 输出 URL 模式:hls muxer 依 %v 生成各变体 playlist。
pub fn output_pattern(job: &JobConfig) -> std::path::PathBuf {
    job.output_root().join("%v").join("index.m3u8")
}

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
