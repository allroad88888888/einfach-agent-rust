//! 026 独立测试：一轮完整对话的记账正确性（红线 2/4 的核心）。
//!
//! 驱动 UserInput → ProviderDone(ToolUse×2) → ToolResult×2 → ProviderDone(EndTurn)，
//! 逐条核对 `history()` 里每个 Entry 的 changes：prev/next 与转移语义吻合、
//! 每次转移恰一条 Entry（batch 语义）、label 可辨、derived（`tools_converged`）
//! 永不出现在任何 Entry 里。后一条在这里靠一个穷举 `match` 钉成编译期事实：
//! `AtomKey` 压根没有对应 derived 的变体（那是 `DerivedKey`，另一张表），新增
//! `AtomKey`/`Slot` 变体不改这个 match 就编译不过。

use std::sync::Arc;

use crate::support::session::new_session;
use crate::support::{
    provider_done_end_turn, provider_done_tool_use, tool_result_event, user_input_event,
};
use agent_core::command::meta::{AgentChange, AgentEntry};
use agent_core::{
    AtomKey, ContentBlock, Role, Slot, SlotState, ToolCallId, ToolCallSlot, TurnStatus,
};

/// 只有落在这个集合里的键才是「已知的 primitive」。
fn assert_known_primitive_key(key: &AtomKey) {
    match key {
        AtomKey::Agent(_, slot) => match slot {
            Slot::Messages
            | Slot::Status
            | Slot::ToolSlots
            | Slot::PrevPrefix
            | Slot::NextMessageId
            | Slot::TurnsUsed
            | Slot::MaxTurns
            | Slot::RetriesUsed
            | Slot::MaxRetries
            // 028 新增：spawn 时快照的工具子集，同时是活名单。
            | Slot::ToolsAllowed
            // 039 新增：激活的 skill id 集。
            | Slot::SkillsActive
            // 073 新增：宿主建会话时声明的工具。
            | Slot::HostTools
            // 064 新增：宿主建会话时声明的 skill。
            | Slot::HostSkills
            // 076 新增：这个会话关掉了哪些内置工具（唯一一个减法槽位）。
            | Slot::DisabledBuiltins
            // 093 新增：runtime 已解析并授权的不透明执行 profile id。
            | Slot::ExecutionProfile
            // 100 新增：这一轮实际要发给 provider 的历史坐标（`SendPlan`）。
            | Slot::SendPlan
            // 103 新增：上一次请求实际用的那份 `SendPlan`（`PrevSendPlan`）。
            | Slot::PrevSendPlan
            // 107 新增：历次压缩产出的摘要正文（引用在 `SendPlan` 里）。
            | Slot::Summaries
            // 134 新增：会话创建期定下的一列 system 前缀块（顺序即信息，不排序）。
            | Slot::PrefixChunks => {}
        },
        AtomKey::ToolCall(_, _, slot) => match slot {
            ToolCallSlot::Result => {}
        },
    }
}

fn find<'a>(entry: &'a AgentEntry, key: &AtomKey) -> &'a AgentChange {
    entry
        .changes
        .iter()
        .find(|c| &c.key == key)
        .unwrap_or_else(|| {
            panic!(
                "entry (label={}) 里没找到键 {key:?}，changes={:?}",
                entry.meta.label, entry.changes
            )
        })
}

#[test]
fn one_full_turn_leaves_one_entry_per_transition_with_matching_prev_next() {
    let mut session = new_session();

    let _ = session.step(user_input_event("你好"));
    let epoch = session.epoch();
    let _ = session.step(provider_done_tool_use(
        epoch,
        &[("call_1", "srv:fs/read"), ("call_2", "srv:fs/read")],
    ));
    let _ = session.step(tool_result_event(epoch, "call_1", "first content"));
    let _ = session.step(tool_result_event(epoch, "call_2", "second content"));
    let _ = session.step(provider_done_end_turn(epoch, "final answer"));

    assert_eq!(
        session.history_len(),
        5,
        "五次真实转移，每次都该恰好落一条 entry（batch 语义）"
    );
    let entries: Vec<&AgentEntry> = session.history().entries().collect();
    assert_eq!(entries.len(), 5);

    let agent = entries[0].changes[0].key.agent().clone();

    for e in &entries {
        assert!(!e.changes.is_empty(), "全是合法转移，changes 不该是空的");
        for c in &e.changes {
            assert_known_primitive_key(&c.key);
        }
        assert!(!e.meta.label.is_empty());
    }

    // label 可辨：不同种类的事件触发的转移，label 不同。
    assert_ne!(
        entries[0].meta.label, entries[1].meta.label,
        "UserInput 与 ProviderDone 的 label 应能区分"
    );
    assert_ne!(
        entries[0].meta.label, entries[2].meta.label,
        "UserInput 与 ToolResult 的 label 应能区分"
    );
    assert_ne!(
        entries[1].meta.label, entries[2].meta.label,
        "ProviderDone 与 ToolResult 的 label 应能区分"
    );

    let status_key = AtomKey::Agent(agent.clone(), Slot::Status);
    let turns_key = AtomKey::Agent(agent.clone(), Slot::TurnsUsed);
    let messages_key = AtomKey::Agent(agent.clone(), Slot::Messages);
    let next_id_key = AtomKey::Agent(agent.clone(), Slot::NextMessageId);
    let tool_slots_key = AtomKey::Agent(agent.clone(), Slot::ToolSlots);
    let prefix_key = AtomKey::Agent(agent.clone(), Slot::PrevPrefix);

    // entry 0：UserInput，Idle -> Thinking。
    let c = find(entries[0], &status_key);
    assert_eq!(c.prev.as_status(), Some(&TurnStatus::Idle));
    assert_eq!(c.next.as_status(), Some(&TurnStatus::Thinking));
    let c = find(entries[0], &turns_key);
    assert_eq!(c.prev.as_u64(), Some(0));
    assert_eq!(c.next.as_u64(), Some(1));
    let c = find(entries[0], &messages_key);
    assert!(c.prev.as_messages().unwrap().is_empty());
    let next_messages = c.next.as_messages().unwrap();
    assert_eq!(next_messages.len(), 1);
    assert_eq!(next_messages[0].role, Role::User);
    assert_eq!(
        next_messages[0].blocks,
        vec![ContentBlock::Text(Arc::from("你好"))]
    );
    let c = find(entries[0], &next_id_key);
    assert_eq!(c.prev.as_u64(), Some(1));
    assert_eq!(c.next.as_u64(), Some(2));

    // entry 1：ProviderDone(ToolUse×2)，Thinking -> ToolsPending，开两个槽。
    let c = find(entries[1], &status_key);
    assert_eq!(c.prev.as_status(), Some(&TurnStatus::Thinking));
    assert_eq!(c.next.as_status(), Some(&TurnStatus::ToolsPending));
    let c = find(entries[1], &tool_slots_key);
    assert!(c.prev.as_slots().unwrap().is_empty());
    let slots = c.next.as_slots().unwrap();
    assert_eq!(slots.len(), 2);
    assert!(slots.iter().all(|s| matches!(s.state, SlotState::Pending)));
    let c = find(entries[1], &prefix_key);
    assert!(c.prev.as_prefix().is_none(), "第一轮之前没有前缀镜像");
    assert!(c.next.as_prefix().is_some());

    // entry 2：ToolResult(call_1)，call_2 还 Pending，不收敛，只改 ToolSlots 一处。
    assert_eq!(
        entries[2].changes.len(),
        1,
        "非收敛的 ToolResult 只该动 ToolSlots 一个槽位"
    );
    let c = find(entries[2], &tool_slots_key);
    let prev = c.prev.as_slots().unwrap();
    let next = c.next.as_slots().unwrap();
    assert!(matches!(prev[0].state, SlotState::Pending));
    assert!(matches!(prev[1].state, SlotState::Pending));
    match &next[0].state {
        SlotState::Finished { content, is_error } => {
            assert_eq!(&**content, "first content");
            assert!(!is_error);
        }
        SlotState::Pending => panic!("call_1 应该已经落地"),
    }
    assert!(
        matches!(next[1].state, SlotState::Pending),
        "call_2 还没回来"
    );

    // entry 3：ToolResult(call_2)，最后一个槽落地：清槽、拼消息、状态回 Thinking、轮数+1。
    let c = find(entries[3], &status_key);
    assert_eq!(c.prev.as_status(), Some(&TurnStatus::ToolsPending));
    assert_eq!(c.next.as_status(), Some(&TurnStatus::Thinking));
    let c = find(entries[3], &turns_key);
    assert_eq!(c.prev.as_u64(), Some(1));
    assert_eq!(c.next.as_u64(), Some(2));
    let tool_slot_changes: Vec<&AgentChange> = entries[3]
        .changes
        .iter()
        .filter(|c| c.key == tool_slots_key)
        .collect();
    assert_eq!(
        tool_slot_changes.len(),
        2,
        "先落地 call_2、再清槽，是同一条 entry 里对同一个 atom 的两次写"
    );
    let cleared = tool_slot_changes.last().unwrap();
    assert!(
        cleared.next.as_slots().unwrap().is_empty(),
        "收敛之后槽位清空"
    );
    let c = find(entries[3], &messages_key);
    let msgs = c.next.as_messages().unwrap();
    assert_eq!(msgs.len(), 3);
    assert_eq!(
        msgs[2].blocks,
        vec![
            ContentBlock::ToolResult {
                id: ToolCallId::new("call_1"),
                content: Arc::from("first content"),
                is_error: false
            },
            ContentBlock::ToolResult {
                id: ToolCallId::new("call_2"),
                content: Arc::from("second content"),
                is_error: false
            },
        ]
    );

    // entry 4：ProviderDone(EndTurn)，Thinking -> Done{truncated:false}。
    let c = find(entries[4], &status_key);
    assert_eq!(c.prev.as_status(), Some(&TurnStatus::Thinking));
    assert_eq!(
        c.next.as_status(),
        Some(&TurnStatus::Done { truncated: false })
    );
    let c = find(entries[4], &messages_key);
    assert_eq!(c.next.as_messages().unwrap().len(), 4);
}
