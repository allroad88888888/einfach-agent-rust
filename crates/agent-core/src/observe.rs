//! 子 agent 可观测：[`Session::agent_tree`] 把整棵活 agent 树此刻的状态摆成一棵快照。
//! 接缝定义见 docs/OBSERVABILITY.md。
//!
//! **一次纯派生读，不是新机制。** 所有字段都是现有 primitive 的投影——没有一个新槽。
//! 于是 undo / 崩溃恢复 / 审计回放的一致性白拿（红线 1/4：不捕获 `AtomId`、不读时钟；
//! 红线 10：只往下读 `status` / `messages` / 在飞槽）。子 agent 不是黑盒，正因为它的
//! 状态一直在 store 里，这个方法只是把它读出来摆成人能看的形状。
//!
//! # 046 边界（当前只有接口，`agent_tree` 体是 `todo!`）
//!
//! 类型 + 签名由主会话钉死，实现与独立测试并行、再合并（WORKFLOW §四）。

use imbl::Vector;
use serde::{Deserialize, Serialize};

use crate::command::Session;
use crate::engine::state::{Failure, SlotState, TurnStatus};
use crate::ids::AgentId;
use crate::value::message::{ContentBlock, Message, Role};

/// 一个 agent 在树上的一格快照。全部是现有 primitive 的投影，**没有一个新槽**。
///
/// 不含 `usage`（per-agent 累计 token 不是 core 槽——见 docs/OBSERVABILITY.md
/// §「usage 不在 M7」）。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AgentNode {
    /// agent id = 它在树里的地址（路径语义，028）。
    pub id: AgentId,
    /// 父 agent 的 id；root 是 `None`（`AgentId::parent`）。
    pub parent: Option<AgentId>,
    /// 离 root 几层，root = 0（`AgentId::depth`）。
    pub depth: u32,
    /// 这个 agent 的第一条 user 消息——子 agent 是 spawn 的任务文本，root 是首轮输入。
    /// 还没有任何 user 消息就是 `None`。
    pub task: Option<String>,
    /// 此刻在干啥（`TurnStatus` 的呈现投影，不是新状态）。
    pub activity: AgentActivity,
}

/// agent 此刻在干啥。**不是新 primitive**——由 `status_of` + 在飞工具槽推出来，
/// 是 [`crate::TurnStatus`] 的呈现层投影（见 docs/OBSERVABILITY.md §「activity 不新增字段」）。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum AgentActivity {
    /// 等用户输入 / 无在飞（`TurnStatus::Idle`）。
    Idle,
    /// provider 调用在飞（`TurnStatus::Thinking`）。
    Thinking,
    /// 有工具槽还是 `Pending`（`TurnStatus::ToolsPending`）。`tools` 是在飞的工具名
    /// （含 spawn 出去的子 agent 那一路）；一时推不出具体名字时可以是空 `Vec`——
    /// 「在忙」这个事实本身来自 status，工具名只是锦上添花。
    Working { tools: Vec<String> },
    /// 这轮结束了（`TurnStatus::Done`）。`truncated` 语义见 `TurnStatus::Done`。
    Done { truncated: bool },
    /// 这轮没走完（`TurnStatus::Failed`）。`reason` 是失败的可读描述。
    Failed { reason: String },
}

/// 整棵活 agent 树此刻的快照，root 在前（`live_agents()` 的顺序：字典序，稳定——
/// 树渲染不该抖）。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AgentTree {
    pub nodes: Vec<AgentNode>,
}

impl Session {
    /// 整棵活 agent 树此刻的快照。
    ///
    /// **纯派生读**：遍历 [`Session::live_agents`]，逐个 [`Session::status_of`] /
    /// [`Session::messages_of`] 组 [`AgentNode`]。字段全是现有 primitive 的投影——
    /// 不捕获 `AtomId`（红线 4 孪生条款，按逻辑键现查）、不读时钟/随机（红线 1）、
    /// 只往下读（红线 10）。
    ///
    /// 于是 `/undo` 撤一轮 spawn 之后，被撤的子 agent **自动**不在结果里——
    /// `live_agents` 靠 `ToolsAllowed` 的活性判定，那本身就是被 undo 回滚的槽，
    /// 树跟着退，零专门代码。
    pub fn agent_tree(&self) -> AgentTree {
        let nodes = self
            .live_agents()
            .into_iter()
            .map(|id| self.agent_node(id))
            .collect();
        AgentTree { nodes }
    }

    /// 组一个 agent 的那一格快照。`id` 拥有所有权（不是引用）：`AgentNode.id`
    /// 要直接搬进结构体，`AgentId` 的克隆是指针拷贝（红线 5），没必要为了省这一次
    /// clone 折腾生命周期。
    fn agent_node(&self, id: AgentId) -> AgentNode {
        let parent = id.parent();
        let depth = id.depth() as u32;
        let task = first_user_text(&self.messages_of(&id));
        let activity = self.activity_of(&id);
        AgentNode {
            id,
            parent,
            depth,
            task,
            activity,
        }
    }

    /// [`TurnStatus`] → [`AgentActivity`] 的呈现投影（本文件顶部文档的判据）。
    /// `ToolsPending` 额外读一次这个 agent 自己的工具槽——在飞的工具名（含 spawn
    /// 出去的子 agent 那一路：spawn 本身就是一次落在父槽位上的工具调用）是
    /// `SlotState::Pending` 的那些条目的投影，不是新槽。
    fn activity_of(&self, agent: &AgentId) -> AgentActivity {
        match self.status_of(agent) {
            TurnStatus::Idle => AgentActivity::Idle,
            TurnStatus::Thinking => AgentActivity::Thinking,
            TurnStatus::ToolsPending => AgentActivity::Working {
                tools: self.pending_tool_names(agent),
            },
            TurnStatus::Done { truncated } => AgentActivity::Done { truncated },
            TurnStatus::Failed(failure) => AgentActivity::Failed {
                reason: describe_failure(&failure),
            },
        }
    }

    /// 这个 agent 自己的工具槽里还没回来的那些的工具名，**顺序即槽位顺序**
    /// （`tool_slots_of` 原样返回，红线 11 的有序性在源头已经保证）。
    fn pending_tool_names(&self, agent: &AgentId) -> Vec<String> {
        self.tool_slots_of(agent)
            .iter()
            .filter(|slot| matches!(slot.state, SlotState::Pending))
            .map(|slot| slot.tool.to_string())
            .collect()
    }
}

/// 第一条 `Role::User` 消息的可见文本——spawn 的任务文本（子 agent）或首轮输入
/// （root）。没有 user 消息就 `None`。**不用工具名/id 顶替**：那样「没写清任务」
/// 和「写了但恰好是个空字符串」会在 UI 上长得一样，前者该显式是 `None`。
fn first_user_text(messages: &Vector<Message>) -> Option<String> {
    let first_user = messages.iter().find(|m| m.role == Role::User)?;
    let text = first_user
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_ref()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(text)
}

/// [`Failure`] 的可读描述。`{class:?}` 用的是 `runtime::subtree::outcome` 同一套
/// 写法（`crates/agent-runtime/src/subtree.rs`）——`ErrorClass` 的 `Debug` 只是
/// 枚举变体名，不带厂商信息，红线 12 管的是「core 里没有 `match provider`」，
/// 不是「不能打印一个已经分类过的错误类别」。
fn describe_failure(failure: &Failure) -> String {
    match failure {
        Failure::Cancelled => "cancelled".to_string(),
        Failure::Provider(class) => format!("provider error: {class:?}"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::command::ChildConfig;
    use crate::engine::Event;
    use crate::ids::{MessageId, ToolCallId};
    use crate::seam::{ErrorClass, PrefixImage};
    use crate::value::session::{StopReason, TokenUsage};

    use super::*;

    fn user_input(session: &mut Session, agent: &AgentId, text: &str) {
        let _ = session.step(Event::UserInput {
            agent: agent.clone(),
            text: Arc::from(text),
            images: Vec::new(),
        });
    }

    /// 单 agent 会话：1 个节点，`parent = None`，`depth = 0`，`task` = 首轮输入，
    /// `activity` 跟 `Session::status()`（这里是 `Thinking`：`UserInput` 之后立刻
    /// 发起了一次 provider 调用）对得上。
    #[test]
    fn single_agent_tree_has_one_node_matching_status() {
        let mut session = Session::new(AgentId::root());
        user_input(&mut session, &AgentId::root(), "帮我读一下这个文件");

        let tree = session.agent_tree();
        assert_eq!(tree.nodes.len(), 1);
        let node = &tree.nodes[0];
        assert_eq!(node.id, AgentId::root());
        assert_eq!(node.parent, None);
        assert_eq!(node.depth, 0);
        assert_eq!(node.task.as_deref(), Some("帮我读一下这个文件"));
        assert_eq!(session.status(), TurnStatus::Thinking);
        assert_eq!(node.activity, AgentActivity::Thinking);
    }

    /// spawn 两个子 agent 后：3 个节点，root 在前、子在后，子的 `parent` = root、
    /// `depth` = 1；两次调用 `agent_tree()` 节点顺序逐个相同——树渲染不该抖。
    #[test]
    fn spawned_children_show_up_with_parent_depth_and_stable_order() {
        let mut session = Session::new(AgentId::root());
        let root = AgentId::root();
        let c1 = session.spawn_child(&root, ChildConfig::default()).unwrap();
        let c2 = session.spawn_child(&root, ChildConfig::default()).unwrap();

        let first = session.agent_tree();
        let second = session.agent_tree();
        assert_eq!(first, second, "两次调用节点顺序该逐个相同");

        assert_eq!(first.nodes.len(), 3);
        assert_eq!(first.nodes[0].id, root);
        assert_eq!(first.nodes[1].id, c1);
        assert_eq!(first.nodes[2].id, c2);
        for child in &first.nodes[1..] {
            assert_eq!(child.parent, Some(root.clone()));
            assert_eq!(child.depth, 1);
        }
    }

    /// 红线 1/4 的实检：spawn 那一轮被 `/undo` 撤了之后，子 agent 从树上消失——
    /// `agent_tree` 没有一行专门代码认识「撤销」这件事，它只是照 `live_agents`
    /// 现在的答案重新读了一遍。
    #[test]
    fn undoing_a_spawn_drops_the_child_from_the_tree() {
        let mut session = Session::new(AgentId::root());
        let root = AgentId::root();
        let child = session.spawn_child(&root, ChildConfig::default()).unwrap();
        assert_eq!(session.agent_tree().nodes.len(), 2);

        let _ = session.undo_turn();

        let tree = session.agent_tree();
        assert_eq!(tree.nodes.len(), 1);
        assert_eq!(tree.nodes[0].id, root);
        assert!(!tree.nodes.iter().any(|n| n.id == child));
    }

    /// `ToolsPending` → `Working`，带在飞的工具名。
    #[test]
    fn tools_pending_activity_carries_the_in_flight_tool_name() {
        let mut session = Session::new(AgentId::root());
        user_input(&mut session, &AgentId::root(), "跑个命令");
        let call_id = ToolCallId::new("call_1");
        let _ = session.step(Event::ProviderDone {
            agent: AgentId::root(),
            epoch: session.epoch(),
            blocks: vec![ContentBlock::ToolUse {
                id: call_id,
                name: Arc::from("srv:shell/exec"),
                input: Arc::new(serde_json::json!({"cmd": "echo hi"})),
            }],
            stop: StopReason::ToolUse,
            usage: TokenUsage {
                prompt: 10,
                completion: 5,
                cached: None,
            },
            prefix: PrefixImage {
                segments: Vec::new(),
                prompt_tokens: None,
            },
            adjustments: Vec::new(),
        });

        let tree = session.agent_tree();
        assert_eq!(tree.nodes.len(), 1);
        assert_eq!(
            tree.nodes[0].activity,
            AgentActivity::Working {
                tools: vec!["srv:shell/exec".to_string()]
            }
        );
    }

    /// `task` 取的是**第一条** user 消息，后续的 user 消息（新一轮的输入）不改写它。
    #[test]
    fn task_is_only_the_first_user_message_not_the_latest() {
        let messages = Vector::from(vec![
            Message {
                id: MessageId(1),
                role: Role::User,
                blocks: vec![ContentBlock::Text(Arc::from("第一句"))],
            },
            Message {
                id: MessageId(2),
                role: Role::Assistant,
                blocks: vec![ContentBlock::Text(Arc::from("回复"))],
            },
            Message {
                id: MessageId(3),
                role: Role::User,
                blocks: vec![ContentBlock::Text(Arc::from("第二句"))],
            },
        ]);
        assert_eq!(first_user_text(&messages).as_deref(), Some("第一句"));
    }

    /// 没有任何 user 消息就是 `None`——不用工具名 / id 顶替。
    #[test]
    fn no_user_message_means_no_task() {
        let messages = Vector::from(vec![Message {
            id: MessageId(1),
            role: Role::Assistant,
            blocks: vec![ContentBlock::Text(Arc::from("hi"))],
        }]);
        assert_eq!(first_user_text(&messages), None);
    }

    #[test]
    fn failure_reason_is_a_readable_string() {
        assert_eq!(describe_failure(&Failure::Cancelled), "cancelled");
        assert_eq!(
            describe_failure(&Failure::Provider(ErrorClass::Auth)),
            "provider error: Auth"
        );
    }
}
