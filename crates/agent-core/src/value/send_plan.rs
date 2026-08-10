//! [`SendPlan`]：「这一轮实际发给 provider 的历史」的**坐标**。
//!
//! 095 §2 的分界：完整对话记录一律入库、永不压缩，压缩只改「这一轮发什么」。
//! 于是压缩不需要动历史，`/undo` 一次原文一个字节都没变过——「压缩与 undo 的
//! 窗口对立」不是被折中掉，是不成立了。
//!
//! 这个文件只管**那个纯值**：三个字段 + 维护它们三条不变量的方法。把坐标作用到
//! 一份历史上是隔壁 [`project()`] 的事。拆开是因为这是两件事：一个是要进快照的
//! 状态（红线 3），一个是要能当 derived 用的纯函数（红线 1），各自能独立验。

use serde::{Deserialize, Serialize};

use crate::ids::{SummaryId, ToolCallId};

mod project;

pub use project::{CLEARED_TOOL_RESULT, project};

/// 这一轮实际要发给 provider 的历史长什么样。完整历史永远不变，变的只有它。
///
/// | 字段 | 装什么 | 谁改 |
/// |---|---|---|
/// | 已清列表 | 被清掉的 [`ToolCallId`] | 第 2 档（清工具返回） |
/// | 边界 | 从第几条开始发 | 第 3、4 档（摘要 / 清窗口） |
/// | 摘要引用 | 边界之前那段的摘要在哪 | 第 3 档 |
///
/// **字段全私有**（同 `persist::SessionLog` 的做法）：三条不变量——已清列表去重
/// 且保序、边界只增、摘要与边界同进同退——由方法维护，不让外部直接摆弄内部
/// 坐标系。已清列表存 [`ToolCallId`] 而不是消息下标，是为了配得成对：
/// `ToolUse` / `ToolResult` 是 `Message.blocks` 里的块，只按下标切必然切出落单的
/// 一半，有的 provider 见到落单的 `ToolUse` 直接 400。
///
/// 容器是 `Vec` 不是 `HashSet`（红线 11）：它会决定进 prompt 的字节，迭代顺序
/// 必须逐字节确定。摘要只存 id 不存正文（红线 5）：序列化大小不随摘要长度增长。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)]
pub struct SendPlan {
    cleared: Vec<ToolCallId>,
    boundary: usize,
    summary: Option<SummaryId>,
}

impl SendPlan {
    /// 恒等元：不清任何东西、边界 0、无摘要。投影它等于完整历史。
    ///
    /// 「不压缩」因此**不是一条特殊路径**，是这个代数结构里的 0——发送侧永远走
    /// 同一个 `project`，少一条永远没跑过的分支。
    pub fn new() -> Self {
        Self::default()
    }

    /// 从没压过。`encode` 用它走「逐字节不变」的快路（100 及以后）。
    pub fn is_pristine(&self) -> bool {
        self.cleared.is_empty() && self.boundary == 0 && self.summary.is_none()
    }

    /// 已被清掉工具结果的调用 id，**首次加入的顺序**。
    pub fn cleared(&self) -> &[ToolCallId] {
        &self.cleared
    }

    /// 从完整历史的第几条开始发。`0` = 从头发。
    ///
    /// 这个值**不跟历史长度校验**——`SendPlan` 不知道历史有多长，那是调用方的事。
    /// 越界的边界在投影里退化成「一条正文都不发」，不 panic。
    pub fn boundary(&self) -> usize {
        self.boundary
    }

    /// 边界之前那段历史的摘要正文在哪。正文不在这里（红线 5），见 [`SummaryId`]。
    pub fn summary(&self) -> Option<&SummaryId> {
        self.summary.as_ref()
    }

    /// 第 2 档。**幂等**：已在列表里的不重复加入；保持**首次加入的顺序**。
    ///
    /// 顺序变了序列化就变了（红线 11），所以这里是「线性查重 + 追加」而不是
    /// 排序去重：已清列表的量级是一次会话里的工具调用数，线性一遍换来的是一条
    /// 稳定的、跟调用顺序对得上的字节序。
    pub fn clear_tool_results(&mut self, ids: impl IntoIterator<Item = ToolCallId>) {
        for id in ids {
            if !self.cleared.contains(&id) {
                self.cleared.push(id);
            }
        }
    }

    /// 第 3、4 档。**边界只能前进**，且摘要与边界**同进**。
    ///
    /// `next <= self.boundary()` 返回 [`BoundaryNotAdvancing`]，不静默忽略：边界
    /// 后退等于把已经被摘要覆盖过的历史重新发一遍，那是 bug 不是操作。
    ///
    /// 摘要是**赋值不是合并**：传 `Some` 就换成新摘要，传 `None` 就清掉旧的。
    /// 第 4 档（清窗口）走的正是 `None` 这一支——窗口一清，旧摘要描述的那段已经
    /// 不是新边界之前的全部了，留着它就是一句对不上号的话。一次调用改两个字段，
    /// 不给「边界动了摘要没动」这种中间态留缝。
    pub fn advance_boundary(
        &mut self,
        next: usize,
        summary: Option<SummaryId>,
    ) -> Result<(), BoundaryNotAdvancing> {
        if next <= self.boundary {
            return Err(BoundaryNotAdvancing {
                current: self.boundary,
                requested: next,
            });
        }
        self.boundary = next;
        self.summary = summary;
        Ok(())
    }
}

/// [`SendPlan::advance_boundary`] 被拒：边界没有前进。
///
/// **可预期的拒绝，不是 bug**——调用方（触发逻辑，096）算出来的新边界可能正好
/// 等于当前边界（这一轮没有新东西可压）。同 `SpawnRefused` 的定位。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BoundaryNotAdvancing {
    pub current: usize,
    pub requested: usize,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn ids(names: &[&str]) -> Vec<ToolCallId> {
        names.iter().map(|n| ToolCallId::new(*n)).collect()
    }

    /// 恒等元：三个字段都是零值，`is_pristine` 为真。
    #[test]
    fn new_is_the_identity_element() {
        let plan = SendPlan::new();
        assert!(plan.is_pristine());
        assert!(plan.cleared().is_empty());
        assert_eq!(plan.boundary(), 0);
        assert_eq!(plan.summary(), None);
        assert_eq!(plan, SendPlan::default());
    }

    /// 幂等 + 保序：重复加入不增长，顺序是**首次**加入的顺序（红线 11）。
    #[test]
    fn clear_tool_results_is_idempotent_and_keeps_first_insertion_order() {
        let mut plan = SendPlan::new();
        plan.clear_tool_results(ids(&["c", "a"]));
        plan.clear_tool_results(ids(&["a", "b", "c"]));
        assert_eq!(plan.cleared(), ids(&["c", "a", "b"]).as_slice());

        // 再加一遍已有的：一个字节都不该变。
        let before = serde_json::to_string(&plan).unwrap();
        plan.clear_tool_results(ids(&["a", "b", "c"]));
        assert_eq!(serde_json::to_string(&plan).unwrap(), before);
        assert!(!plan.is_pristine());
    }

    /// 边界只能前进：相等和后退都拒，且**拒了不留痕**（字段一个都没动）。
    #[test]
    fn advance_boundary_refuses_non_advancing() {
        let mut plan = SendPlan::new();
        assert_eq!(
            plan.advance_boundary(0, None),
            Err(BoundaryNotAdvancing {
                current: 0,
                requested: 0
            })
        );
        assert!(plan.is_pristine());

        plan.advance_boundary(5, Some(SummaryId::new("s1")))
            .unwrap();
        let snapshot = plan.clone();
        assert_eq!(
            plan.advance_boundary(3, Some(SummaryId::new("s2"))),
            Err(BoundaryNotAdvancing {
                current: 5,
                requested: 3
            })
        );
        assert_eq!(plan, snapshot);
    }

    /// 摘要与边界同进：第 3 档换摘要，第 4 档（`None`）连旧摘要一起清掉。
    #[test]
    fn summary_moves_with_the_boundary() {
        let mut plan = SendPlan::new();
        plan.advance_boundary(4, Some(SummaryId::new("s1")))
            .unwrap();
        assert_eq!(plan.summary(), Some(&SummaryId::new("s1")));

        plan.advance_boundary(9, Some(SummaryId::new("s2")))
            .unwrap();
        assert_eq!(
            (plan.summary(), plan.boundary()),
            (Some(&SummaryId::new("s2")), 9)
        );

        plan.advance_boundary(12, None).unwrap();
        assert_eq!(plan.summary(), None);
        assert_eq!(plan.boundary(), 12);
    }

    /// 红线 3：serde 往返，三个字段都非零值。
    #[test]
    fn roundtrip() {
        let mut plan = SendPlan::new();
        plan.clear_tool_results(ids(&["call_1", "call_2"]));
        plan.advance_boundary(7, Some(SummaryId::new("sum_1")))
            .unwrap();
        let s = serde_json::to_string(&plan).unwrap();
        assert_eq!(serde_json::from_str::<SendPlan>(&s).unwrap(), plan);
        assert!(!serde_json::from_str::<SendPlan>(&s).unwrap().is_pristine());
    }

    /// 红线 5：摘要正文不在 `SendPlan` 里——序列化大小只跟 id 有关，跟正文多长
    /// 无关。正文再大也只是投影的一个入参，投影也不会回写 plan。
    #[test]
    fn serialized_size_is_independent_of_summary_text() {
        let mut plan = SendPlan::new();
        plan.advance_boundary(1, Some(SummaryId::new("sum_1")))
            .unwrap();
        let serialized = serde_json::to_string(&plan).unwrap();
        assert!(serialized.len() < 128, "{serialized}");

        let huge: Arc<str> = Arc::from("x".repeat(1 << 20));
        let history = imbl::Vector::new();
        let _ = project(&history, &plan, Some(&huge));
        assert_eq!(serde_json::to_string(&plan).unwrap(), serialized);
        assert!(!serialized.contains('x'));
    }
}
