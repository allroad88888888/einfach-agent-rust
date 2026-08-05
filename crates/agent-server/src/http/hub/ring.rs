//! [`RingState`]：一个 session 的有界事件环形缓冲（issue 031「帧 id 单调，默认
//! 256 帧」）——纯数据结构，不碰 `tokio`，不碰锁，方便直接单元测试（`hub/mod.rs`
//! 里唯一有锁的地方只是把它包在 `Mutex` 里）。

use std::collections::VecDeque;

use crate::event::Frame;

/// 一个广播事件（034：[`Frame`] 信封，agent + event）配上它的单调帧 id——
/// **不要跟 [`Frame`] 本身混淆**：`Frame` 是协议信封（这个 crate 对外承诺的
/// wire 形状），`BufferedFrame` 是这个环形缓冲自己的记账形状（多了一个 SSE
/// `id:` 字段），只在 `crate::http::hub` 内部活动，不出这个模块树。`id` 从 1
/// 开始（0 留给「客户端从没见过任何帧」这个隐含含义——见 [`RingState::replay`]，
/// `Last-Event-ID: 0` 天然落进「补发从头开始」而不是被误判成一个真实存在过的
/// 帧）。
#[derive(Clone, Debug, PartialEq)]
pub(in crate::http) struct BufferedFrame {
    pub(in crate::http) id: u64,
    pub(in crate::http) event: Frame,
}

pub(super) struct RingState {
    frames: VecDeque<BufferedFrame>,
    capacity: usize,
    next_id: u64,
}

/// [`RingState::replay`] 的判断结果：拿到一个可选的 `Last-Event-ID` 之后，该
/// 怎么把这条新连接接上去。
pub(super) enum Replay {
    /// 没有可补的历史——这个 hub 还没广播过任何东西（缓冲空）。直接从当前
    /// 时刻开始收直播。（031 独测分歧 1 之前：「没给 `Last-Event-ID`」也落在
    /// 这一支，导致真浏览器首连收不到任何历史——见 `RingState::replay` 文档。）
    Live,
    /// 精确补发的一批帧（可能是空的——客户端已经追上了缓冲区最新帧，效果跟
    /// `Live` 一样，不必特判）。
    Backlog(Vec<BufferedFrame>),
    /// 缺口：客户端上次看到的 id 早于缓冲区当前最旧的一帧，中间那些帧已经被
    /// 挤出去，永远补不回来了。`skipped` 是精确值，不是估计
    /// （`oldest_available_id - last_event_id - 1`）。`tail` 是缓冲区**仍然
    /// 保留**的那一段（gap 只代表被冲掉的那一段，不是放弃全部补发——031 独测
    /// 分歧 2 的裁决）：跟 `gap_frame_id` 自洽，恰好是 `replay(Some(gap_frame_id))`
    /// 会给出的那份 backlog，调用方发完 gap 帧之后原样接着发这份 `tail`，再
    /// 续上直播。
    Gap {
        skipped: u64,
        gap_frame_id: u64,
        tail: Vec<BufferedFrame>,
    },
}

impl RingState {
    pub(super) fn new(capacity: usize) -> Self {
        RingState {
            frames: VecDeque::with_capacity(capacity.min(1024)),
            capacity: capacity.max(1),
            next_id: 1,
        }
    }

    /// 追加一帧（034：`event` 是 [`Frame`] 信封，agent + event）、分配它的 id；
    /// 超出容量就挤掉最旧的一帧。返回新分配的 [`BufferedFrame`]（调用方拿它去
    /// 广播给当下正在收直播的订阅者）。
    pub(super) fn push(&mut self, event: Frame) -> BufferedFrame {
        let id = self.next_id;
        self.next_id += 1;
        let frame = BufferedFrame { id, event };
        self.frames.push_back(frame.clone());
        if self.frames.len() > self.capacity {
            self.frames.pop_front();
        }
        frame
    }

    /// 见 [`Replay`] 的三种结果。
    ///
    /// `gap_frame_id`（`Gap` 分支）刻意取「缓冲区当前最旧帧的 id 减一」——这个
    /// 选择让它自洽：客户端拿这个 gap 帧的 id 当作下一次的 `Last-Event-ID`
    /// 重连时，`last + 1 == oldest.id`，落进下面的 `_` 分支精确补发出缓冲区
    /// 剩下的全部内容，不会被误判成又一次 gap；`Gap` 分支自己的 `tail` 字段
    /// 携带的就是这同一份内容，调用方不需要真的发起第二次 `replay` 调用。
    ///
    /// # 完全没带 `Last-Event-ID`（031 独测分歧 1 的裁决）
    ///
    /// 真实浏览器第一次打开 `EventSource` 时该请求天然不带这个头（从没连过，
    /// 没有可带的历史 id）——原文验收「先 input 后首连收到全部历史」要求这条
    /// 路径也能补上缓冲里现有的内容，等价于把它当成带了缓冲区「最旧可用帧
    /// id 减一」（`oldest - 1`）来处理：必然落进下面 `_` 分支的 backlog（不会
    /// 触发 gap——没有一个真实的 `Last-Event-ID` 可以拿来跟当前最旧帧比较，
    /// 谈不上「缺口」，只是「从有的地方开始给」）。
    pub(super) fn replay(&self, last_event_id: Option<u64>) -> Replay {
        let oldest = self.frames.front();
        let effective_last = match (last_event_id, oldest) {
            (Some(last), _) => last,
            (None, Some(oldest)) => oldest.id.saturating_sub(1),
            (None, None) => return Replay::Live, // 缓冲区是空的，没什么可补的。
        };
        match oldest {
            None => Replay::Live,
            Some(oldest) if effective_last + 1 < oldest.id => Replay::Gap {
                skipped: oldest.id - effective_last - 1,
                gap_frame_id: oldest.id - 1,
                tail: self.frames.iter().cloned().collect(),
            },
            _ => Replay::Backlog(
                self.frames
                    .iter()
                    .filter(|f| f.id > effective_last)
                    .cloned()
                    .collect(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use agent_core::AgentId;

    use crate::event::SessionEvent;

    fn ev(text: &str) -> Frame {
        Frame {
            agent: AgentId::root(),
            event: SessionEvent::TextDelta(Arc::from(text)),
        }
    }

    #[test]
    fn ids_are_monotonic_starting_at_one() {
        let mut ring = RingState::new(4);
        assert_eq!(ring.push(ev("a")).id, 1);
        assert_eq!(ring.push(ev("b")).id, 2);
        assert_eq!(ring.push(ev("c")).id, 3);
    }

    #[test]
    fn eviction_keeps_only_the_newest_capacity_frames() {
        let mut ring = RingState::new(2);
        ring.push(ev("a")); // id 1，很快被挤掉
        ring.push(ev("b")); // id 2
        ring.push(ev("c")); // id 3
        // last_event_id = 1（不是 0）：`1 + 1 == oldest.id(2)`，落进 backlog 分支
        // 而不是 gap 分支——见 `last_event_id_older_than_the_buffer_reports_an_
        // exact_gap` 那条测试专门覆盖 `0` 这个确实该判 gap 的情况。
        let Replay::Backlog(frames) = ring.replay(Some(1)) else {
            panic!("该是 Backlog")
        };
        assert_eq!(frames.iter().map(|f| f.id).collect::<Vec<_>>(), vec![2, 3]);
    }

    /// 031 独测分歧 1 的裁决：完全不带 `Last-Event-ID`（真浏览器首连的样子）
    /// 且缓冲区里已经有内容 → 从缓冲最旧可用帧开始补，不是直接接直播。
    #[test]
    fn no_last_event_id_replays_from_the_oldest_buffered_frame() {
        let mut ring = RingState::new(4);
        let first = ring.push(ev("a"));
        let second = ring.push(ev("b"));
        let Replay::Backlog(frames) = ring.replay(None) else {
            panic!("该是 Backlog：等价于带了 oldest-1")
        };
        assert_eq!(frames, vec![first, second]);
    }

    #[test]
    fn no_last_event_id_with_an_empty_ring_is_live() {
        let ring = RingState::new(4);
        assert!(
            matches!(ring.replay(None), Replay::Live),
            "从没广播过任何一帧，没什么好补的"
        );
    }

    #[test]
    fn empty_ring_with_a_last_event_id_is_treated_as_live_not_a_gap() {
        let ring = RingState::new(4);
        assert!(matches!(ring.replay(Some(7)), Replay::Live));
    }

    #[test]
    fn backlog_replays_exactly_the_frames_after_last_seen() {
        let mut ring = RingState::new(8);
        ring.push(ev("a"));
        let second = ring.push(ev("b"));
        let third = ring.push(ev("c"));
        let Replay::Backlog(frames) = ring.replay(Some(1)) else {
            panic!("该是 Backlog")
        };
        assert_eq!(frames, vec![second, third]);
    }

    #[test]
    fn already_caught_up_replays_an_empty_backlog() {
        let mut ring = RingState::new(8);
        let last = ring.push(ev("a")).id;
        assert!(matches!(ring.replay(Some(last)), Replay::Backlog(f) if f.is_empty()));
    }

    #[test]
    fn last_event_id_older_than_the_buffer_reports_an_exact_gap() {
        let mut ring = RingState::new(2);
        ring.push(ev("a")); // id 1, 很快被挤掉
        ring.push(ev("b")); // id 2, 被挤掉
        ring.push(ev("c")); // id 3, 缓冲区最旧
        ring.push(ev("d")); // id 4, 缓冲区最新
        let Replay::Gap {
            skipped,
            gap_frame_id,
            tail,
        } = ring.replay(Some(1))
        else {
            panic!("该是 Gap")
        };
        assert_eq!(skipped, 1, "id 2 被挤掉了，缺口大小是 1");
        assert_eq!(gap_frame_id, 2);
        // 031 独测分歧 2 的裁决：gap 只代表被冲掉的那一段，缓冲区仍然保留的
        // 尾部（这里是 id 3、4）该原样带在 `Gap` 里，不是被一并放弃。
        assert_eq!(tail.iter().map(|f| f.id).collect::<Vec<_>>(), vec![3, 4]);
    }

    #[test]
    fn reconnecting_with_the_gap_frame_id_gets_a_clean_backlog_not_another_gap() {
        let mut ring = RingState::new(2);
        ring.push(ev("a"));
        ring.push(ev("b"));
        ring.push(ev("c"));
        ring.push(ev("d"));
        let Replay::Gap { gap_frame_id, .. } = ring.replay(Some(1)) else {
            panic!("该是 Gap")
        };
        // 用 gap 帧自己的 id 重连——见本文件 `replay` 文档「自洽」那句。
        let Replay::Backlog(frames) = ring.replay(Some(gap_frame_id)) else {
            panic!("该是 Backlog，不是又一次 Gap")
        };
        assert_eq!(frames.iter().map(|f| f.id).collect::<Vec<_>>(), vec![3, 4]);
    }
}
