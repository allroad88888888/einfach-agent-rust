//! 108 验收（从 106 移交）：父的 `encode` 不含被摘要原文；父的 `encode` 不含
//! 子 agent 的摘要过程。
//!
//! 106 落地时 `Effect::Compact` 对外还不可达，这两条只能在 108 接上阶梯之后
//! 第一次真的可测（106 的「行为验收移交」一节）。用打标记的办法（照
//! [`crate::harvest_omits_child_turns_support`] / 097 的思路）：被摘要的每一轮
//! 都打一个独有标记，压缩子自己的指令文案与它写出来的摘要正文也各给一个独有
//! 标记，压缩之后父的下一轮请求体逐一断言这些标记在不在。
//!
//! 压缩子结构上零工具、单轮（106 `compact_spawn.rs` 模块文档：`ChildConfig`
//! 默认 `tools_allowed` 为空），所以这里不追求 097 那种「多轮标记」——它对
//! 摘要子没有意义（它没有第二轮可打标记）。断言的着力点因此是「原文标记」与
//! 「压缩子的指令文案」两类内容，各自都不该泄漏进父的下一跳请求体。

use agent_core::{AgentId, Session, TurnStatus};
use agent_runtime::{ToolTable, run_turn};

use crate::ladder_support::{SUMMARY_PROMPT_NEEDLE, build_ctx, text_response};
use crate::support;
use crate::support::routed::{Route, RoutedServer};

const WINDOW: u32 = 1000;
const LOW: u32 = 400; // 40%，远低于 85%——round1/2 没有工具调用，一旦过线就会
// 立刻直通第 3 档（没有第 2 档可清），所以垫轮次必须压着不冲线，直到第 5 轮
// 才让 usage 冲过去，确保两条原文标记都已经滑出保护区（最近 3 轮）。
const HIGH: u32 = 900; // 90%，冲过 85%
const ORIGINAL_MARK_1: &str = "ORIGINALTEXT_ALPHA_9f3c";
const ORIGINAL_MARK_2: &str = "ORIGINALTEXT_BETA_7e21";
const SUMMARY_MARK: &str = "SUMMARYTEXT_GAMMA_4b08";

fn routes() -> Vec<Route> {
    vec![
        Route::sse(SUMMARY_PROMPT_NEEDLE, text_response(SUMMARY_MARK, 5)),
        Route::sse("PROBE_ROUND", text_response("已经压缩过了，继续", HIGH)),
        Route::sse("ROUND5MARK", text_response("继续", HIGH)),
        Route::sse("ROUND4MARK", text_response("继续", LOW)),
        Route::sse("ROUND3MARK", text_response("继续", LOW)),
        Route::sse(ORIGINAL_MARK_2, text_response("继续", LOW)),
        Route::sse(ORIGINAL_MARK_1, text_response("继续", LOW)),
    ]
}

#[test]
fn parent_prompt_after_compaction_has_neither_the_raw_originals_nor_the_childs_own_prompt() {
    let dir = support::temp_dir("ladder-excludes-material");
    let server = RoutedServer::start(routes());
    let (mut ctx, _events) = build_ctx(server.port, &dir, ToolTable::builtin(), Some(WINDOW));
    let mut session = Session::new(AgentId::root());

    let run = |session: &mut Session, ctx: &mut agent_runtime::RunnerCtx, text: &str| {
        run_turn(session, ctx, text)
            .unwrap_or_else(|e| panic!("{text} 不该是 source failure：{e:?}"))
    };

    // 第 1、2 轮：各打一个独有的「原文标记」，这两轮之后会被摘要盖住。
    assert_eq!(
        run(
            &mut session,
            &mut ctx,
            &format!("{ORIGINAL_MARK_1} 这是会被摘要盖住的第一轮")
        ),
        TurnStatus::Done { truncated: false }
    );
    // 第 2 轮起，每一轮开跑之前都要显式 `begin_turn`（026 判断 13）——漏了不
    // 报错，新的 `UserInput` 撞上上一轮的 `Done` 会被判成
    // `Notice::ProtocolViolation`，这一轮根本没发生过任何请求。
    session.begin_turn();
    assert_eq!(
        run(
            &mut session,
            &mut ctx,
            &format!("{ORIGINAL_MARK_2} 这是会被摘要盖住的第二轮")
        ),
        TurnStatus::Done { truncated: false }
    );
    // 第 3、4、5 轮：纯文本，把前两轮一起挤出保护区（保护区=最近 3 轮）；
    // 第 5 轮末 usage 冲线——没有工具结果可清，直通第 3 档。
    session.begin_turn();
    assert_eq!(
        run(&mut session, &mut ctx, "ROUND3MARK 继续聊"),
        TurnStatus::Done { truncated: false }
    );
    session.begin_turn();
    assert_eq!(
        run(&mut session, &mut ctx, "ROUND4MARK 继续聊"),
        TurnStatus::Done { truncated: false }
    );
    session.begin_turn();
    assert_eq!(
        run(&mut session, &mut ctx, "ROUND5MARK 继续聊"),
        TurnStatus::Done { truncated: false }
    );

    let root = AgentId::root();
    let plan = session.send_plan_of(&root);
    assert!(plan.boundary() > 0, "前提：第 5 轮末该已经触发压缩");

    // 探针轮：这一跳的请求体就是「压缩之后父的下一次 encode」。
    session.begin_turn();
    assert_eq!(
        run(&mut session, &mut ctx, "PROBE_ROUND 压缩之后的探针轮"),
        TurnStatus::Done { truncated: false }
    );

    let probe_body = server
        .call("PROBE_ROUND")
        .expect("探针轮该真的发生过一次请求")
        .body;

    assert!(
        !probe_body.contains(ORIGINAL_MARK_1),
        "父的下一跳请求体不该含被摘要的第一轮原文：{probe_body}"
    );
    assert!(
        !probe_body.contains(ORIGINAL_MARK_2),
        "父的下一跳请求体不该含被摘要的第二轮原文：{probe_body}"
    );
    assert!(
        !probe_body.contains(SUMMARY_PROMPT_NEEDLE),
        "父的下一跳请求体不该含压缩子自己的指令文案（子的过程不该泄漏）：{probe_body}"
    );
    assert!(
        probe_body.contains(SUMMARY_MARK),
        "反向锁：摘要正文该确实替换进了父的请求体，不是整段消失：{probe_body}"
    );
}
