//! 026 独立测试：快照形状——`primitives()` 只含 primitive（个数恰等于槽位数），
//! serde 往返（红线 3：primitive atom 的值必须全部可序列化，用一轮真实对话
//! 攒出来的完整快照做整份 to_string/from_str 相等断言）。

mod support;

use agent_core::{AgentValue, AtomKey};
use support::session::new_session;
use support::{provider_done_end_turn, provider_done_tool_use, tool_result_event, user_input_event};

/// 槽位表的条数**写死在这里**，不从 `Slot::ALL.len()` 取：这个数字就是这个测试
/// 的全部价值——顺手加一个槽位而没想清楚它进不进快照时，这里会红。
///
/// 026 是 9；028 加了 `Slot::ToolsAllowed`（spawn 时快照的工具子集，同时是子 agent
/// 的活名单）→ 10。改这个数之前先问：新槽位是不是真的**必须**进快照。
const EXPECTED_SLOT_COUNT: usize = 10;

#[test]
fn a_fresh_session_has_exactly_the_documented_number_of_primitives() {
    let session = new_session();
    let primitives = session.primitives();
    assert_eq!(primitives.len(), EXPECTED_SLOT_COUNT, "槽位表恰好这么多，一个不多一个不少");
    for (key, _) in &primitives {
        assert!(matches!(key, AtomKey::Agent(_, _)), "会话刚建好，不该有任何 ToolCall 键");
    }
}

#[test]
fn tool_calls_do_not_add_extra_atoms_to_the_snapshot() {
    // 工具槽整体住一个槽位（Slots(Arc<Vec<ToolSlot>>)），不建 per-call atom
    // ——所以哪怕跑完一整轮带两个工具调用的对话，primitives() 的条数也不变。
    let mut session = new_session();
    let _ = session.step(user_input_event("hi"));
    let epoch = session.epoch();
    let _ = session.step(provider_done_tool_use(epoch, &[("call_1", "srv:fs/read"), ("call_2", "srv:fs/read")]));
    let _ = session.step(tool_result_event(epoch, "call_1", "r1"));
    let _ = session.step(tool_result_event(epoch, "call_2", "r2"));
    let _ = session.step(provider_done_end_turn(epoch, "done"));

    let primitives = session.primitives();
    assert_eq!(primitives.len(), EXPECTED_SLOT_COUNT);
    for (key, _) in &primitives {
        assert!(matches!(key, AtomKey::Agent(_, _)));
    }
}

#[test]
fn primitives_are_sorted_by_logical_key() {
    let session = new_session();
    let primitives = session.primitives();
    let mut sorted = primitives.clone();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(primitives, sorted, "两份快照要能逐值比对，顺序必须是确定的");
}

#[test]
fn the_snapshot_of_a_real_conversation_survives_a_serde_roundtrip() {
    let mut session = new_session();
    let _ = session.step(user_input_event("你好，帮我读一下文件"));
    let epoch = session.epoch();
    let _ = session.step(provider_done_tool_use(epoch, &[("call_1", "srv:fs/read"), ("call_2", "srv:fs/read")]));
    let _ = session.step(tool_result_event(epoch, "call_1", "内容一"));
    let _ = session.step(tool_result_event(epoch, "call_2", "内容二，带换行\n和引号\""));
    let _ = session.step(provider_done_end_turn(epoch, "读完了，两个文件都是空的"));

    let snapshot = session.primitives();
    assert_eq!(snapshot.len(), EXPECTED_SLOT_COUNT);

    let encoded = serde_json::to_string(&snapshot).expect("primitive atom 的值必须全部可序列化（红线 3）");
    let decoded: Vec<(AtomKey, AgentValue)> = serde_json::from_str(&encoded).expect("必须能原样解回来");

    assert_eq!(decoded, snapshot, "整份快照 to_string/from_str 之后逐值相等");
}
