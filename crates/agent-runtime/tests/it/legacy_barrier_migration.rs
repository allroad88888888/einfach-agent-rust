//! **199 §九 的迁移端到端**：一份 199 **之前**格式的真实会话文件（`EntryMeta` 那时
//! 还是 `barrier: bool`）恢复之后，`barrier: true` 的那一条仍然挡住 undo，
//! `barrier: false` 的那些仍然不挡。
//!
//! 为什么走真字节而不是手搓 `PersistedMeta`：手搓只证明 `From<RawMeta>` 写对了，
//! 证明不了「老文件里那些字节还读得进来」。下面这份 journal 是用今天的代码跑一次
//! 「user_input → provider_done(tool_use) → mark_no_undo → tool_result」落出来的
//! 真产物，**只把 meta 那一段换回老形状**（`"undoability":"blocked"` → `"barrier":true`，
//! `"state_only"` → `"barrier":false`）——除此之外一个字节没动。
//!
//! 这一条红了的症状是最贵的那种：老会话里一次真实的不可逆操作从此不再挡 undo，
//! 功能全部正常，只有某一天用户撤销之后副作用悄悄留在了外面。

use agent_core::{AgentId, BlockedCause, UndoReport, Undoability};

use crate::support;

/// 199 之前的三行 journal + 一行 cursor。第三条（`tool_result`）是那时的 `barrier: true`。
const LEGACY_LINES: &[&str] = &[
    r#"{"kind":"entry","seq":0,"meta":{"turn_id":1,"epoch":0,"label":"user_input","barrier":false},"changes":[{"key":{"Agent":["root","NextMessageId"]},"prev":{"U64":1},"next":{"U64":2}},{"key":{"Agent":["root","Messages"]},"prev":{"Messages":[]},"next":{"Messages":[{"id":1,"role":"User","blocks":[{"Text":"hi"}]}]}},{"key":{"Agent":["root","TurnsUsed"]},"prev":{"U64":0},"next":{"U64":1}},{"key":{"Agent":["root","Status"]},"prev":{"Status":"Idle"},"next":{"Status":"Thinking"}}]}"#,
    r#"{"kind":"entry","seq":1,"meta":{"turn_id":1,"epoch":0,"label":"provider_done","barrier":false},"changes":[{"key":{"Agent":["root","NextMessageId"]},"prev":{"U64":2},"next":{"U64":3}},{"key":{"Agent":["root","Messages"]},"prev":{"Messages":[{"id":1,"role":"User","blocks":[{"Text":"hi"}]}]},"next":{"Messages":[{"id":1,"role":"User","blocks":[{"Text":"hi"}]},{"id":2,"role":"Assistant","blocks":[{"ToolUse":{"id":"call_1","name":"srv:shell/exec","input":{"cmd":"echo"}}}]}]}},{"key":{"Agent":["root","PrevPrefix"]},"prev":"Null","next":{"Prefix":{"segments":[],"prompt_tokens":1}}},{"key":{"Agent":["root","ToolSlots"]},"prev":{"Slots":[]},"next":{"Slots":[{"call_id":"call_1","tool":"srv:shell/exec","input":{"cmd":"echo"},"state":"Pending"}]}},{"key":{"Agent":["root","Status"]},"prev":{"Status":"Thinking"},"next":{"Status":"ToolsPending"}}]}"#,
    r#"{"kind":"entry","seq":2,"meta":{"turn_id":1,"epoch":0,"label":"tool_result","barrier":true},"changes":[{"key":{"Agent":["root","ToolSlots"]},"prev":{"Slots":[{"call_id":"call_1","tool":"srv:shell/exec","input":{"cmd":"echo"},"state":"Pending"}]},"next":{"Slots":[{"call_id":"call_1","tool":"srv:shell/exec","input":{"cmd":"echo"},"state":{"Finished":{"content":"ok","is_error":false}}}]}},{"key":{"Agent":["root","ToolSlots"]},"prev":{"Slots":[{"call_id":"call_1","tool":"srv:shell/exec","input":{"cmd":"echo"},"state":{"Finished":{"content":"ok","is_error":false}}}]},"next":{"Slots":[]}},{"key":{"Agent":["root","NextMessageId"]},"prev":{"U64":3},"next":{"U64":4}},{"key":{"Agent":["root","Messages"]},"prev":{"Messages":[{"id":1,"role":"User","blocks":[{"Text":"hi"}]},{"id":2,"role":"Assistant","blocks":[{"ToolUse":{"id":"call_1","name":"srv:shell/exec","input":{"cmd":"echo"}}}]}]},"next":{"Messages":[{"id":1,"role":"User","blocks":[{"Text":"hi"}]},{"id":2,"role":"Assistant","blocks":[{"ToolUse":{"id":"call_1","name":"srv:shell/exec","input":{"cmd":"echo"}}}]},{"id":3,"role":"Assistant","blocks":[{"ToolResult":{"id":"call_1","content":"ok","is_error":false}}]}]}},{"key":{"Agent":["root","TurnsUsed"]},"prev":{"U64":1},"next":{"U64":2}},{"key":{"Agent":["root","Status"]},"prev":{"Status":"ToolsPending"},"next":{"Status":"Thinking"}}]}"#,
    r#"{"kind":"cursor","cursor":3}"#,
];

#[test]
fn a_pre_199_session_file_still_blocks_undo_at_its_barrier() {
    let dir = support::temp_dir("legacy-barrier-migration");
    let path = dir.join("session.jsonl");
    std::fs::write(&path, format!("{}\n", LEGACY_LINES.join("\n"))).unwrap();

    let backend = agent_runtime::open_backend(Some(path), |e| panic!("不该有加载错误：{e}"));
    let mut session = agent_runtime::recover(
        backend.as_ref(),
        AgentId::root(),
        agent_core::DEFAULT_HISTORY_CAP,
        agent_core::AgentLimits::default(),
        &mut |k| panic!("不该有不认识的键：{k:?}"),
    )
    .unwrap()
    .expect("这份文件不是空的");

    // 逐字确定的映射：只有工具结果那一条是屏障，另外两条是 StateOnly——
    // 不是 `Hooked`（老会话本来就没有还原函数，读成 Hooked 会让整条老会话
    // 在 undo 时都去问一个不存在的钩子表，于是全部变成不可撤销）。
    let tiers: Vec<Undoability> = session
        .history()
        .entries()
        .map(|e| e.meta.undoability)
        .collect();
    assert_eq!(
        tiers,
        vec![
            Undoability::StateOnly,
            Undoability::StateOnly,
            Undoability::Blocked
        ]
    );

    // `barrier: true` 那条**仍然挡**：游标正下方就是它，一条都退不动。
    assert_eq!(
        session.undo_turn(),
        UndoReport::Blocked {
            entries: 0,
            barrier_seq: 2,
            // 老会话本来就没有还原钩子，成因只可能是「没交还原函数」。
            cause: BlockedCause::NoHook,
        }
    );
    assert_eq!(session.cursor(), 3, "被挡住时游标一个字节都不动");

    // `barrier: false` 那些**仍然不挡**：一次确认越过屏障之后一路退到底。
    assert_eq!(
        session.undo_turn_force(),
        UndoReport::Applied {
            entries: 3,
            turn_id: 1
        }
    );
    assert_eq!(session.cursor(), 0);
}
