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
pub fn transcode(
    _config: &crate::transcode::job::JobConfig,
    _tx: &Sender<TranscodeEvent>,
    _cancel: &AtomicBool,
) -> Result<PathBuf, Outcome> {
    unimplemented!("Task 7 实现")
}
