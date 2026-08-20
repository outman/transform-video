use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use smol::channel::Sender;

use crate::transcode::event::TranscodeEvent;

/// 终止方式:Canceled = 用户取消;Failed = 出错(消息已面向用户)。
#[derive(Debug)]
pub enum Outcome {
    Canceled,
    Failed(String),
}

/// Ok(root) 表示成功;Err 为终止方式。
///
/// 契约:终止事件(Done/Canceled/Failed)由 run_job 统一发送;
/// 本函数只经 tx 发送 Phase/Progress/Log,终止方式只通过返回值上报。
pub fn transcode(
    _config: &crate::transcode::job::JobConfig,
    _tx: &Sender<TranscodeEvent>,
    _cancel: &AtomicBool,
) -> Result<PathBuf, Outcome> {
    unimplemented!("Task 7 实现")
}
