//! 把 [`super::ring::Replay`] 还原成可交付帧。
//!
//! SSE 和拉取式端点都是同一个 ring 的投影；这里集中处理 `Gap` 的合成，确保
//! 两条传输不会对同一段历史给出不同的帧序列。

use agent_core::AgentId;

use crate::event::{Frame, SessionEvent};

use super::BufferedFrame;
use super::ring::Replay;

/// 将 ring 的 replay 判断变为有序帧序列。
///
/// `Gap` 是传输层在重连时才会合成的事实，因此标记为 root，并紧接仍保留在
/// ring 里的尾部；这与 SSE 补发的既有语义一致。
pub(super) fn frames(replay: Replay) -> Vec<BufferedFrame> {
    match replay {
        Replay::Live => Vec::new(),
        Replay::Backlog(frames) => frames,
        Replay::Gap {
            skipped,
            gap_frame_id,
            mut tail,
        } => {
            let gap = BufferedFrame {
                id: gap_frame_id,
                event: Frame {
                    agent: AgentId::root(),
                    event: SessionEvent::Gap { skipped },
                },
            };
            tail.insert(0, gap);
            tail
        }
    }
}
