use std::sync::{atomic::AtomicBool, Mutex, MutexGuard, OnceLock};

use smol::channel;

use transform_video::transcode::event::TranscodeEvent;
use transform_video::transcode::job::{JobConfig, VariantSpec};
use transform_video::transcode::pipeline;

/// Windows 的 HLS 路径兼容层会在转码期间临时切换进程 cwd。Rust 默认并行运行
/// 同一测试二进制中的用例，因此这组集成测试必须把“准备配置—转码—断言”整体串行化；
/// 仅依赖生产代码中 transcode 调用内部的锁，仍会让调用前后的 cwd 断言互相干扰。
fn pipeline_test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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
    let _test_lock = pipeline_test_lock();
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path(), false);
    let working_dir = std::env::current_dir().unwrap();
    let (result, rx) = run(&cfg, &AtomicBool::new(false));
    assert_eq!(std::env::current_dir().unwrap(), working_dir);
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
    assert!(v.read_dir().unwrap().any(|p| p
        .unwrap()
        .path()
        .extension()
        .is_some_and(|e| e == "m4s")));
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
    let _test_lock = pipeline_test_lock();
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
    let _test_lock = pipeline_test_lock();
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path(), false);
    let (result, _rx) = run(&cfg, &AtomicBool::new(true)); // 预置取消
    assert!(matches!(result, Err(pipeline::Outcome::Canceled)));
    assert!(!cfg.output_root().exists());
}

#[test]
fn transcodes_three_variants_with_interleaving() {
    let _test_lock = pipeline_test_lock();
    let dir = tempfile::tempdir().unwrap();
    let cfg = JobConfig {
        variants: JobConfig::default().variants,
        force_software: true,
        ..config(dir.path(), true)
    };
    let (result, _rx) = run(&cfg, &AtomicBool::new(false));
    let root = result.expect("应成功");
    for name in ["1080p", "720p", "480p", "audio"] {
        assert!(root.join(name).join("index.m3u8").is_file(), "缺 {name}");
    }
    let master = std::fs::read_to_string(root.join("master.m3u8")).unwrap();
    assert!(master.contains("1080p/") && master.contains("720p/") && master.contains("480p/"));
}
