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

/// 关窗(实体释放)即取消:包装 handle,Drop 时置 cancel 标志。
/// 对已结束的转码线程再置一次标志无害。
struct CancelOnDrop(Option<TranscodeHandle>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(h) = &self.0 {
            h.cancel();
        }
    }
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
    handle: Option<CancelOnDrop>,
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
        debug_assert_eq!(defaults.variants.len(), self.enabled_variants.len());
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
            output_stem: None,
        }
    }

    pub fn start(&mut self, cx: &mut Context<Self>) {
        if self.busy() {
            return; // 已有任务在跑:忽略重复开始,避免双事件泵
        }
        let config = self.build_config();
        let (tx, rx) = channel::unbounded();
        self.handle = Some(CancelOnDrop(Some(crate::transcode::run_job(config, tx))));
        self.status = Status::Preparing;
        self.percent = 0.0;
        self.elapsed_secs = 0.0;
        self.eta_secs = 0.0;
        self.logs.clear();
        self.output_root = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                if this
                    .update(cx, |state, cx| state.on_event(event, cx))
                    .is_err()
                {
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
            TranscodeEvent::Progress {
                percent,
                elapsed_secs,
                eta_secs,
            } => {
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
                self.logs.push(msg);
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
        if let Some(handle) = &self.handle
            && let Some(h) = &handle.0
        {
            h.cancel();
        }
        cx.notify();
    }

    pub fn busy(&self) -> bool {
        self.handle.as_ref().is_some_and(|c| c.0.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_config_follows_enabled_variants_and_bitrates() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let s = AppState {
            enabled_variants: [false, true, false],
            bitrates_kbps: [0, 2600, 0],
            input_path: Some(f.path().to_path_buf()),
            output_dir: Some(std::env::temp_dir()),
            ..AppState::default()
        };
        let cfg = s.build_config();
        assert_eq!(cfg.variants.len(), 1);
        assert_eq!(cfg.variants[0].name, "720p");
        assert_eq!(cfg.variants[0].bit_rate_kbps, 2600);
        assert_eq!(cfg.variants[0].max_rate_kbps, 3000);
        assert!(matches!(cfg.validate(), Ok(())));
    }

    #[test]
    fn build_config_invalid_without_variants() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let s = AppState {
            enabled_variants: [false, false, false],
            input_path: Some(f.path().to_path_buf()),
            output_dir: Some(std::env::temp_dir()),
            ..AppState::default()
        };
        assert_eq!(
            s.build_config().validate().unwrap_err(),
            "至少需要启用一档分辨率"
        );
    }

    #[test]
    fn dropping_cancel_on_drop_cancels_the_job() {
        // 空配置 → run_job 立即 Failed;用 clone 的句柄观察 Drop 的取消副作用
        let (tx, _rx) = smol::channel::unbounded();
        let handle = crate::transcode::run_job(JobConfig::default(), tx);
        let observer = handle.clone();
        drop(CancelOnDrop(Some(handle)));
        assert!(observer.is_canceled());
    }
}
