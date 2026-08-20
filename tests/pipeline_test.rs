use std::sync::atomic::AtomicBool;

use smol::channel;

use transform_video::transcode::event::TranscodeEvent;
use transform_video::transcode::job::{JobConfig, VariantSpec};
use transform_video::transcode::pipeline;

fn variant_480p() -> VariantSpec {
    VariantSpec {
        name: "480p",
        height: 480,
        bit_rate_kbps: 1200,
        max_rate_kbps: 1500,
        buf_size_kbps: 2500,
    }
}

/// lavfi 虚拟输入:2 秒 320x240@30;with_audio 时再挂一路 48k 正弦波。
/// 注意 1:input 是 lavfi 图字符串,含 ':' 等,不能直接当路径派生输出目录——
/// JobConfig 用 output_stem 覆盖,测试里固定 "lavfi-test"。
/// 注意 2:lavfi 输入设备要求多输出图的每个输出垫命名为 out0/out1…(ffmpeg 9 实测)。
fn config(dir: &std::path::Path, with_audio: bool) -> JobConfig {
    let src = if with_audio {
        "testsrc2=duration=2:size=320x240:rate=30[out0];sine=frequency=440:sample_rate=48000:duration=2[out1]"
    } else {
        "testsrc2=duration=2:size=320x240:rate=30"
    };
    JobConfig {
        input: src.into(),
        output_dir: dir.into(),
        variants: vec![variant_480p()],
        force_software: true,
        input_format: Some("lavfi".into()),
        output_stem: Some("lavfi-test".into()),
        ..JobConfig::default()
    }
}

fn run(
    cfg: &JobConfig,
    cancel: &AtomicBool,
) -> (
    Result<std::path::PathBuf, pipeline::Outcome>,
    channel::Receiver<TranscodeEvent>,
) {
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
    // init 文件带变体下标(init_0.mp4 等)
    assert!(v.read_dir().unwrap().any(|p| {
        let p = p.unwrap().path();
        p.extension().is_some_and(|e| e == "mp4")
            && p.file_stem()
                .is_some_and(|s| s.to_string_lossy().starts_with("init"))
    }));
    assert!(
        v.read_dir()
            .unwrap()
            .any(|p| p.unwrap().path().extension().is_some_and(|e| e == "m4s"))
    );
    assert!(!root.join("audio").exists());
    let mut final_pct: f64 = 0.0;
    while let Ok(e) = rx.try_recv() {
        if let TranscodeEvent::Progress { percent, .. } = e {
            final_pct = final_pct.max(percent);
        }
    }
    assert!(
        (final_pct - 1.0).abs() < 1e-6,
        "最终进度应为 100%,实际 {final_pct}"
    );
}

#[test]
fn transcodes_with_audio_variant() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path(), true);
    let (result, _rx) = run(&cfg, &AtomicBool::new(false));
    let root = result.expect("应成功");
    assert!(root.join("audio/index.m3u8").is_file());
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
