//! 202 的主验收：**宿主声明的 `web:` 工具一律挡 undo，声明什么都一样**。
//!
//! 这是决策 199「现状清账」里那个真实失败场景的堵口，原文逐字如下：
//!
//! 1. 宿主声明 `{ "name": "web:crm/draft", "reversibility": "reversible" }`
//! 2. 模型调它，在 CRM 里建了一份草稿
//! 3. 用户 `/undo` → CLI 打印「回退了 3 条目」，**没有任何提示**
//! 4. **草稿还在 CRM 里**
//!
//! 199 §七 的裁决：宿主工具的执行体在浏览器/桌面进程里，还原函数交不回来，
//! 所以恒 [`Undoability::Blocked`]。于是第 3 步从「静默 `Applied`」变成
//! 「`Blocked { cause: NoHook }`，停下来问」——用户可以 `/undo!` 越过，
//! 但那是他的一次显式决定，不是我们替他做的。
//!
//! 走的是**公开接缝**：装一张 `with_host_tools` 的表、跑一轮真的 loop（假 SSE
//! server 扮 provider）、经 `resolve_remote_tool` 回传结果，再调
//! `Session::undo_turn`。不手搓 entry——手搓的话「派发那一刻标记」这条时序
//! 就没被验到，而它正是 `dispatch` 里那两处改动的全部内容。

use agent_core::{
    AgentId, BlockedCause, Session, ToolCallId, TurnStatus, Undoability, UndoReport,
};
use agent_runtime::{
    RemoteToolOutput, ToolTable, host_tools_from_declaration, resolve_remote_tool, run_turn,
};

use crate::support::{build_ctx_with, sse_text, sse_tool_call, spawn_scripted_server, temp_dir};

const DRAFT_TOOL: &str = "web:crm/draft";
const DRAFT_WIRE: &str = "web_3Acrm_2Fdraft";
const PEEK_TOOL: &str = "web:crm/peek";
const PEEK_WIRE: &str = "web_3Acrm_2Fpeek";

/// 一张只有宿主声明的表，**从真的声明 JSON 走一遍翻译**（`POST /sessions` 的
/// `capabilities.tools` 就是这个形状，`agent-server` 与 wasm 宿主都走这个函数）。
///
/// 不手搓 `(ToolSpec, Reversibility)`：199 §现状清账 那个场景的第一句话就是
/// 「宿主声明 `{ "name": "web:crm/draft", "reversibility": "reversible" }`」，
/// 从字面量声明进来才算把那句话验了——手搓等于跳过翻译那一层，而
/// `reversible` 这个词就是在那一层落成 `Reversibility::Reversible` 的。
///
/// `draft` 声明 `reversible`（199 场景里那个字），`peek` 声明 `pure`
/// （202 验收第二条那个「行为变更的钉子」）。
fn tool_table() -> ToolTable {
    let declaration = format!(
        r#"{{"tools":[
            {{"name":"{DRAFT_TOOL}","description":"在 CRM 里建一份草稿","reversibility":"reversible"}},
            {{"name":"{PEEK_TOOL}","description":"看一眼 CRM 档案","reversibility":"pure"}}
        ]}}"#
    );
    let tools = host_tools_from_declaration(&declaration).expect("这份声明是合法的");
    ToolTable::empty().with_host_tools(tools)
}

/// 跑一轮：模型调一次宿主工具 → 宿主回传结果 → hop2 收敛成 `Done`。
fn one_host_tool_turn(name: &str, wire: &str, call_id: &str) -> Session {
    let dir = temp_dir("host-tool-undo-none");
    let port = spawn_scripted_server(vec![
        sse_tool_call(call_id, wire, r#"{\"title\":\"季度复盘\"}"#),
        sse_text("草稿建好了"),
    ]);
    let (mut ctx, _events) = build_ctx_with(port, &dir, tool_table());
    let mut session = Session::new(AgentId::root());

    assert_eq!(
        run_turn(&mut session, &mut ctx, "帮我在 CRM 里建一份草稿")
            .expect("派发宿主工具不该是 source failure"),
        TurnStatus::ToolsPending,
        "前置条件：宿主工具要占住一个等待槽"
    );
    let status = resolve_remote_tool(
        &mut session,
        &mut ctx,
        AgentId::root(),
        ToolCallId::new(call_id),
        RemoteToolOutput::Success(format!("{name} 执行完毕")),
    )
    .expect("普通 web: 工具走 resolve_remote_tool 这道门");
    assert_eq!(status, TurnStatus::Done { truncated: false });
    session
}

/// **本 issue 的主验收**：声明 `reversible` 的宿主工具，`/undo` 停下来问，
/// 成因是 `NoHook`（「它没有提供还原函数」），不是静默 `Applied`。
#[test]
fn a_host_tool_declaring_reversible_still_blocks_undo_with_no_hook() {
    let mut session = one_host_tool_turn(DRAFT_TOOL, DRAFT_WIRE, "draft-1");

    let report = session.undo_turn();
    let UndoReport::Blocked {
        barrier_seq, cause, ..
    } = report
    else {
        panic!("199 的失败场景：这里静默 Applied 就等于把 CRM 里那份草稿丢了不管，拿到 {report:?}");
    };
    assert!(
        matches!(cause, BlockedCause::NoHook),
        "宿主根本没有交还原函数的通道，成因该是 NoHook 而不是钩子跑挂/钩子丢了：{cause:?}"
    );

    let entry = session
        .history()
        .entries()
        .find(|e| e.seq == barrier_seq)
        .expect("撞停的 seq 必须在日志里");
    assert_eq!(entry.meta.undoability, Undoability::Blocked);

    // 用户确认之后照样退得掉——202 收窄的是「默不作声地放过」，不是「不许退」。
    assert!(
        matches!(session.undo_turn_force(), UndoReport::Applied { .. }),
        "强制越过该成功"
    );
    assert!(session.messages().is_empty(), "越过之后这一轮该整个退掉");
}

/// 声明 `pure` 的宿主工具**不挡**——**这条是「事实/承诺」那道分界的钉子**
/// （决策 199 §七）。
///
/// 它跟上面那条 `reversible` 用的是同一条远端第五路、同一个「执行体在页面那边、
/// 还原函数交不回来」的结构性事实，**结局却相反**，分界就在声明的是什么：
///
/// - `reversible` 声明的是「有补偿动作」——一个**承诺**。承诺要兑现就得交出那个
///   函数，而它结构上交不出来 → 挡。
/// - `pure` 声明的是「没碰外部世界」——一个**事实断言**。它不需要任何函数来兑现，
///   所以「交不出函数」这个理由**对它不成立** → 不挡。
///
/// 这条初版写反过（199 §七 初稿是「一律挡」，两态时期的措辞遗留），改的代价是
/// `ask_user_question` / `browser_action` 这些 `location_of` 判 `Web` 的标准集工具
/// 会被无故挡住——它们字面上不可能有副作用，挡住保护不了任何东西。有人日后把
/// 「一律挡」改回来时，这条必红。
#[test]
fn a_host_tool_declaring_pure_does_not_block_undo() {
    let mut session = one_host_tool_turn(PEEK_TOOL, PEEK_WIRE, "peek-1");

    let report = session.undo_turn();
    assert!(
        matches!(report, UndoReport::Applied { .. }),
        "pure 是事实断言不是承诺，「交不出函数」挡不住它：{report:?}"
    );
    assert!(session.messages().is_empty(), "该整轮干净退掉");
}

/// `/undo!` **一次只放行一条**：同一轮里两个宿主工具调用，强制越过第一条之后
/// 还会在第二条上再停一次。沿用 `undo_turn_force` 的既有语义（027 原话：
/// 一次确认只放行一条，放行全部等于替用户答了几个他没被问到的问题），这里钉的是
/// 「宿主工具批量变成屏障之后，那条语义没被冲垮」。
#[test]
fn undo_force_crosses_host_tool_barriers_one_at_a_time() {
    let dir = temp_dir("host-tool-undo-none-two");
    let port = spawn_scripted_server(vec![
        sse_tool_call("draft-a", DRAFT_WIRE, r#"{\"title\":\"a\"}"#),
        sse_tool_call("draft-b", DRAFT_WIRE, r#"{\"title\":\"b\"}"#),
        sse_text("两份都建好了"),
    ]);
    let (mut ctx, _events) = build_ctx_with(port, &dir, tool_table());
    let mut session = Session::new(AgentId::root());

    assert_eq!(
        run_turn(&mut session, &mut ctx, "建两份草稿").expect("不该是 source failure"),
        TurnStatus::ToolsPending
    );
    for call_id in ["draft-a", "draft-b"] {
        let status = resolve_remote_tool(
            &mut session,
            &mut ctx,
            AgentId::root(),
            ToolCallId::new(call_id),
            RemoteToolOutput::Success("ok".to_string()),
        )
        .expect("宿主回传");
        if call_id == "draft-b" {
            assert_eq!(status, TurnStatus::Done { truncated: false });
        }
    }

    // 两条屏障在同一轮里：第一次撞停、force 越过一条之后再撞停一次。
    assert!(matches!(session.undo_turn(), UndoReport::Blocked { .. }));
    let first_force = session.undo_turn_force();
    assert!(
        matches!(first_force, UndoReport::Blocked { .. }),
        "一次确认只放行一条，第二条屏障该再停一次：{first_force:?}"
    );
    assert!(
        matches!(session.undo_turn_force(), UndoReport::Applied { .. }),
        "两条都确认过之后整轮退掉"
    );
    assert!(session.messages().is_empty());
}
