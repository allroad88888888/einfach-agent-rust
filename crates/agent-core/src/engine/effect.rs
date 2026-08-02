//! core 产出的 effect：**描述，不是执行**。
//!
//! core 不调 HTTP、不跑工具、不读时钟，只说「请去调这个 provider」「请去执行这个
//! 工具」。红线 7 能成立不是靠自觉，是这个形状的自然结果——整个 loop 因此可零 IO
//! 单元测试（005）、超时可模拟、状态机可穷举。
//!
//! **两条别越的线**：
//!
//! 1. effect 里不许出现不可序列化的活对象（红线 3 的精神）。[`Effect::ExecuteTool`]
//!    带的是 `ToolCallRequest` **快照**，不是执行句柄、不是 `JoinHandle`、不是
//!    `oneshot::Sender`。活对象放宿主的 runtime registry。
//! 2. 每个在飞的 effect 都带 [`Epoch`]（红线 6），宿主原样带回结果事件里。
//!
//! ## M1 只定四个变体
//!
//! issue 001 列了七个，这里只有四个——`SpawnChild` / `Compact` / `Persist` **不定**，
//! 连空壳变体都不留。021 的教训：上一版把类型一次定全，结果一半在 M1 根本用不上，
//! 而用得上的那半有几个形状是错的。空壳变体比不定更糟，它看起来像做完了。
//!
//! | 推迟的 | 等谁 | 为什么现在定不了 |
//! |---|---|---|
//! | `SpawnChild` | issue 006（M3） | 子 agent 由模型主动 spawn 还是编排层按计划 spawn 都没拍板，两者是完全不同的产品形态，字段跟着不同 |
//! | `Compact` | 决策 18（M2/M3） | 触发在 core、实现在 core、摆盘在 adapter，但**压缩是状态变更要走 command 层进 undo log**，而 M1 没有 store。阈值取多少也还是 ROADMAP §四的未决问题 |
//! | `Persist` | issue 011（M2） | 落盘的单位是 `Entry`（009 定），M1 连 `Entry` 都还不存在。012 已经写明 M1 阶段丢弃 |

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, ToolCallId};

use super::epoch::Epoch;
use super::notice::Notice;

/// 宿主该去做的一件事。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum Effect {
    /// 该调 provider 了。
    ///
    /// **没有 payload，这是决策 15 的直接后果**：请求由 adapter 组装，core 说的是
    /// 「该调了」不是「照这个调」。宿主收到它，在 actor 线程上从状态取料
    /// （`Ingredients`）、让 adapter 按自己的能力组装（`encode`），产出能跨线程带走的
    /// `Encoded` 再发出去（ADAPTER.md §时序）。
    ///
    /// 上一版在 core 里写过一个 `build_request()` 想把 payload 塞进来，做出来的是个
    /// 不做任何模型相关判断的搬运函数。**effect 变胖是接缝错位的第一个症状。**
    ///
    /// **消费者**：012 的 runner；005 的 `MockProvider` 拦下它直接回 `ProviderDone`。
    CallProvider { agent: AgentId, epoch: Epoch },

    /// 该执行这个工具了。**只带名字和输入**——core 手里就这两样（模型的
    /// `ToolUse` 块里只有它们），工具表在宿主侧。
    ///
    /// 002 合并时定的形状（改自 001 的 `request: ToolCallRequest`）：core 没有
    /// `Location`/`Reversibility` 数据，现造占位快照是**编造**——M1 碰巧无害，
    /// M2 的 undo 会因假的 `Irreversible` 白拦一次 `fs/read`，正是静默错值。
    /// 「发起当时的快照」原则不变，但记录点在**宿主/command 层**（它持有注册表，
    /// M2 的 009 `Entry` 就在那记）；core 不装有自己没有的数据。
    ///
    /// `call_id` 是模型给的配对凭证（`ToolUse` ↔ `ToolResult`）。
    ///
    /// **消费者**：012 的 runner（按 `tool` 查表补全快照后路由）；005 的 `MockExecutor`。
    ExecuteTool {
        agent: AgentId,
        call_id: ToolCallId,
        tool: Arc<str>,
        input: Arc<serde_json::Value>,
        epoch: Epoch,
    },

    /// 取消这个世代的所有在飞请求。
    ///
    /// 没有 `agent` 字段：epoch 是**会话级**的（STATE-MODEL：一个 root agent + 它的
    /// 整棵子树 = 一个 session），取消一个世代就是取消这个世代里所有 agent 发出去的
    /// 东西。按 agent 取消是另一回事，M1 没有这个需求。
    ///
    /// **消费者**：016（取消在任意状态下都生效，且一定发出这个）；012 的 runner
    /// 把它翻译成「置取消标志 / 断流」，`agent-transport` 的 `StreamOutcome::Cancelled`
    /// 已经在等这一下。
    CancelInFlight { epoch: Epoch },

    /// 说一句给人听的话。载荷见 [`Notice`]。
    ///
    /// 变体名沿用 issue 里的 `Emit`，但字段不叫 `event`——那个词在这个模块里已经是
    /// [`super::event::Event`]（进来的事件），同名两义读代码时必然踩。
    ///
    /// **消费者**：012（M1 打到 stdout，M3 换成推 SSE）、014（打印）。
    Emit(Notice),
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::engine::state::TurnStatus;

    /// 四个变体全部 serde 往返。effect 要能跨线程、进日志、进快照——不可序列化的
    /// 东西溜进来（红线 3），第一次真的从崩溃恢复时才会发现。
    #[test]
    fn roundtrip_all_variants() {
        let effects = vec![
            Effect::CallProvider {
                agent: AgentId::root(),
                epoch: Epoch(1),
            },
            Effect::ExecuteTool {
                agent: AgentId::root(),
                call_id: ToolCallId::new("call_1"),
                tool: Arc::from("srv:fs/read"),
                input: Arc::new(json!({"path": "/tmp/a"})),
                epoch: Epoch(1),
            },
            Effect::CancelInFlight { epoch: Epoch(2) },
            Effect::Emit(Notice::TurnStatusChanged {
                status: TurnStatus::Done { truncated: false },
            }),
        ];

        let s = serde_json::to_string(&effects).unwrap();
        assert_eq!(serde_json::from_str::<Vec<Effect>>(&s).unwrap(), effects);
    }

    /// `CallProvider` 里除了路由用的 `agent` 和校验用的 `epoch` **什么都没有**——
    /// 决策 15 的最小实检：序列化出来的 key 只有这两个，多一个就是 payload 爬回来了。
    #[test]
    fn call_provider_carries_no_payload() {
        let json = serde_json::to_value(Effect::CallProvider {
            agent: AgentId::root(),
            epoch: Epoch(0),
        })
        .unwrap();
        let fields = json["CallProvider"].as_object().unwrap();
        let mut keys: Vec<&str> = fields.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["agent", "epoch"]);
    }
}
