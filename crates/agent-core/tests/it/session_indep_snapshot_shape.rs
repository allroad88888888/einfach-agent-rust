//! 026 独立测试：快照形状——`primitives()` 只含 primitive（个数恰等于槽位数），
//! serde 往返（红线 3：primitive atom 的值必须全部可序列化，用一轮真实对话
//! 攒出来的完整快照做整份 to_string/from_str 相等断言）。

use crate::support::session::new_session;
use crate::support::{
    provider_done_end_turn, provider_done_tool_use, tool_result_event, user_input_event,
};
use agent_core::{AgentValue, AtomKey};

/// 槽位表的条数**写死在这里**，不从 `Slot::ALL.len()` 取：这个数字就是这个测试
/// 的全部价值——顺手加一个槽位而没想清楚它进不进快照时，这里会红。
///
/// 026 是 9；028 加了 `Slot::ToolsAllowed`（spawn 时快照的工具子集，同时是子 agent
/// 的活名单）→ 10；039 加了 `Slot::SkillsActive`（激活的 skill id 集）→ 11；
/// 073 加了 `Slot::HostTools`（宿主建会话时声明的工具）→ 12；
/// 064 加了 `Slot::HostSkills`（宿主建会话时声明的 skill）→ 13；
/// 076 加了 `Slot::DisabledBuiltins`（这个会话关掉了哪些内置工具）→ 14；
/// 093 追加 `Slot::ExecutionProfile`（子 agent 的 durable 执行身份）→ 15；
/// 100 追加 `Slot::SendPlan`（这一轮实际要发给 provider 的历史坐标）→ 16；
/// 103 追加 `Slot::PrevSendPlan`（上一次请求实际用的那份 `SendPlan`）→ 17；
/// 107 追加 `Slot::Summaries`（历次压缩产出的摘要正文）→ 18；
/// 134 追加 `Slot::PrefixChunks`（会话创建期定下的 system 前缀块）→ 19；
/// 144 追加 `Slot::PrefixAllowed`（spawn 时快照的「开局产物」授予名单）→ 20；
/// 154 追加 `Slot::HostPrefix`（宿主经 `capabilities.prefix` 声明的开局块，
/// 决策 31）→ 21。改这个数之前先问：新槽位是不是真的**必须**进快照。
///
/// `HostTools` 的答案是必须：不进快照 = 一次落快照之后声明就丢了，恢复出来的
/// 会话少几个工具且不报错——正是这个测试要拦的那种「顺手加一个槽位」。
/// `HostSkills` 同理，而且更硬：`SkillsActive` 本来就在快照里，声明不进快照就是
/// 一份**指向空 registry 的激活集**（状态说某个 skill 激活着、正文却取不到）。
/// `DisabledBuiltins` 也必须：它是**减法**，不进快照 = 恢复出来的会话把当初藏起来
/// 的工具又端给模型看，而那段历史里从没出现过它们。`SendPlan` 同样必须：不进快照
/// = 崩溃恢复之后压缩状态丢了，恢复出来的会话把已经清掉的工具结果又当作没清过、
/// 边界也退回 0——那正是「压缩与完整历史各自独立恢复」（095 §2）说好的对半落空。
/// `PrevSendPlan` 同理：不进快照 = 崩溃恢复后第一轮的 `PrefixIntent` 判定丢了
/// 参照物，把刚恢复出来的正常状态误判成漂移事故。`PrefixChunks`（134）同样必须：
/// 不进快照 = 恢复出来的会话要么少一整段 system 前缀、要么得重跑那些算出前缀的
/// 东西（而它们读的是外部世界，这一次的结果不保证跟当初一样）——前缀在 prompt
/// 最前面，两种结局都是缓存全断 + 上下文跟历史对不上，且都不报错。
/// `PrefixAllowed`（144）跟 `ToolsAllowed` 同一条理由：不进快照 = undo 回到
/// spawn 那一刻时，重建出来的子 agent 拿不到「当时被授予了什么」，只能落一个
/// 猜出来的默认值——而 undo 的语义正是要精确回到 spawn 那一刻，不是回到一个
/// 看起来差不多的状态。`HostPrefix`（154）跟 `HostTools` 同一条理由：不进快照
/// = 一次落快照之后声明就丢了，恢复出来的会话没有当初那份开局块且不报错。
const EXPECTED_SLOT_COUNT: usize = 24;

#[test]
fn a_fresh_session_has_exactly_the_documented_number_of_primitives() {
    let session = new_session();
    let primitives = session.primitives();
    assert_eq!(
        primitives.len(),
        EXPECTED_SLOT_COUNT,
        "槽位表恰好这么多，一个不多一个不少"
    );
    for (key, _) in &primitives {
        assert!(
            matches!(key, AtomKey::Agent(_, _)),
            "会话刚建好，不该有任何 ToolCall 键"
        );
    }
}

#[test]
fn tool_calls_do_not_add_extra_atoms_to_the_snapshot() {
    // 工具槽整体住一个槽位（Slots(Arc<Vec<ToolSlot>>)），不建 per-call atom
    // ——所以哪怕跑完一整轮带两个工具调用的对话，primitives() 的条数也不变。
    let mut session = new_session();
    let _ = session.step(user_input_event("hi"));
    let epoch = session.epoch();
    let _ = session.step(provider_done_tool_use(
        epoch,
        &[("call_1", "srv:fs/read"), ("call_2", "srv:fs/read")],
    ));
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
    let _ = session.step(provider_done_tool_use(
        epoch,
        &[("call_1", "srv:fs/read"), ("call_2", "srv:fs/read")],
    ));
    let _ = session.step(tool_result_event(epoch, "call_1", "内容一"));
    let _ = session.step(tool_result_event(
        epoch,
        "call_2",
        "内容二，带换行\n和引号\"",
    ));
    let _ = session.step(provider_done_end_turn(epoch, "读完了，两个文件都是空的"));

    let snapshot = session.primitives();
    assert_eq!(snapshot.len(), EXPECTED_SLOT_COUNT);

    let encoded =
        serde_json::to_string(&snapshot).expect("primitive atom 的值必须全部可序列化（红线 3）");
    let decoded: Vec<(AtomKey, AgentValue)> =
        serde_json::from_str(&encoded).expect("必须能原样解回来");

    assert_eq!(
        decoded, snapshot,
        "整份快照 to_string/from_str 之后逐值相等"
    );
}
