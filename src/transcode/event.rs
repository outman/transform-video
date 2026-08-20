use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
    /// percent ∈ [0,1];-1 表示不定(时长未知);eta_secs < 0 表示无法估计
    Progress {
        percent: f64,
        elapsed_secs: f64,
        eta_secs: f64,
    },
    Log(String),
    Done {
        output_root: PathBuf,
    },
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
pub fn run_job(config: JobConfig, tx: Sender<TranscodeEvent>) -> TranscodeHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let handle = TranscodeHandle {
        cancel: cancel.clone(),
    };

    std::thread::Builder::new()
        .name("transcode".into())
        .spawn(move || {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_job_reports_failure_from_invalid_config() {
        // 默认配置输入为空 → pipeline 打不开输入,返回 Failed(旧行为:
        // 占位 pipeline 的 unimplemented!() panic → catch_unwind → Failed)
        let (tx, rx) = smol::channel::unbounded();
        let _h = run_job(JobConfig::default(), tx);
        let mut events = Vec::new();
        while let Ok(e) = rx.recv_blocking() {
            let terminal = matches!(
                &e,
                TranscodeEvent::Done { .. } | TranscodeEvent::Canceled | TranscodeEvent::Failed(_)
            );
            events.push(e);
            if terminal {
                break;
            }
        }
        assert!(matches!(
            events.first(),
            Some(TranscodeEvent::Phase(Phase::Preparing))
        ));
        assert!(matches!(events.last(), Some(TranscodeEvent::Failed(_))));
    }
}
