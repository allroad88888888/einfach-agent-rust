//! 108 验收核心断言：造一个「清工具返回够用」的场景 → 第 3 档一次都没触发，
//! **零模型调用**（第 3 档需要 spawn 一个真的摘要子 agent，一次都没发生）。
//!
//! 顺序反了或者两档并行开火，这条当场红——这是整条阶梯最有分量的一条。
//!
//! 场景：一轮工具调用建立可清历史，usage 一直很低；第 5 轮突然冲过 85%，
//! 第 2 档开火清掉第 1 轮的工具结果；清完之后 usage 立刻被压回低位（测试直接
//! 钦定这个数字，096 决策记录：触发是纯算术，不量真实 token），后面几轮 usage
//! 继续走低——第 3 档因此永远没有机会开火。
//!
//! 用严格顺序的 [`support::spawn_recording_server`]（不是内容路由）：**这条测试
//! 全程只有 10 个真实 hop（1 轮工具调用两跳 + 8 轮纯文本），压缩子的 hop 一次
//! 都不该发生**——脚本按这个精确顺序摆好，压缩子一旦真的被 spawn，多出来的那
//! 条连接会因为脚本耗尽而连不上，测试会直接可见地挂/失败，而不是悄悄放过。

use agent_core::{AgentId, Session, TurnStatus};
use agent_runtime::{RunnerEvent, ToolTable, run_turn};

use crate::ladder_support::{build_ctx, text_response, tool_call_response};
use crate::support;
use crate::support::ScriptedResponse;

const WINDOW: u32 = 1000;
const LOW: u32 = 400; // 40%，远低于 85%
const HIGH: u32 = 900; // 90%，冲过 85%

fn leak(lines: Vec<String>) -> Vec<&'static str> {
    lines
        .into_iter()
        .map(|l| -> &'static str { Box::leak(l.into_boxed_str()) })
        .collect()
}

#[test]
fn tier2_alone_keeps_tier3_from_ever_firing() {
    let dir = support::temp_dir("ladder-tier2-alone");
    std::fs::write(dir.join("seed.txt"), b"SEED-CONTENT").unwrap();

    let script = vec![
        // 第 1 轮，hop1：模型要求读一个文件。
        ScriptedResponse::Sse(leak(tool_call_response(
            "call_r1",
            "srv_3Afs_2Fread",
            r#"{"path": "seed.txt"}"#,
            LOW,
        ))),
        // 第 1 轮，hop2：工具结果回来，模型收敛。
        ScriptedResponse::Sse(leak(text_response("读完了", LOW))),
        // 第 2、3、4 轮：纯文本，把第 1 轮的工具结果挤出保护区。
        ScriptedResponse::Sse(leak(text_response("继续", LOW))),
        ScriptedResponse::Sse(leak(text_response("继续", LOW))),
        ScriptedResponse::Sse(leak(text_response("继续", LOW))),
        // 第 5 轮：usage 冲过阈值——第 2 档该在这一轮末开火。
        ScriptedResponse::Sse(leak(text_response("继续", HIGH))),
        // 第 6～9 轮：usage 已经被清出来的空间压回低位，第 3 档不该有机会开火。
        ScriptedResponse::Sse(leak(text_response("继续", LOW))),
        ScriptedResponse::Sse(leak(text_response("继续", LOW))),
        ScriptedResponse::Sse(leak(text_response("继续", LOW))),
        ScriptedResponse::Sse(leak(text_response("继续", LOW))),
    ];
    let (port, bodies) = support::spawn_recording_server(script);
    let (mut ctx, events) = build_ctx(port, &dir, ToolTable::builtin(), Some(WINDOW));
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();

    for (i, text) in [
        "读一下 seed.txt",
        "继续聊",
        "继续聊",
        "继续聊",
        "继续聊", // 这一轮末 usage 冲线，第 2 档该开火
        "继续聊",
        "继续聊",
        "继续聊",
        "继续聊",
    ]
    .into_iter()
    .enumerate()
    {
        // 第 2 轮起，每一轮开跑之前都要显式 `begin_turn`（026 判断 13）——漏了
        // 不报错：会话停在上一轮的 `Done`，新的 `UserInput` 会被判成
        // `Notice::ProtocolViolation`，状态原样停在 `Done`，这一轮**根本没
        // 发请求**。`assert_eq!(status, Done)` 那种断言在「真的跑完」和
        // 「压根没发生」两种情况下都成立，本身测不出这个坑，全靠下面的
        // `bodies.len()`（真实连接数）这条硬指标兜底。
        if i > 0 {
            session.begin_turn();
        }
        let status = run_turn(&mut session, &mut ctx, text)
            .unwrap_or_else(|e| panic!("{text} 不该是 source failure：{e:?}"));
        assert_eq!(status, TurnStatus::Done { truncated: false }, "{text}");
    }

    // 核心断言：第 2 档确实清过东西（证明这条测试真的驱动到了阶梯，不是空跑）。
    let plan = session.send_plan_of(&root);
    assert!(
        !plan.cleared().is_empty(),
        "第 2 档该至少清过一批工具结果，否则这条测试没有意义"
    );

    // 核心断言：第 3 档一次都没触发——边界纹丝不动。
    assert_eq!(plan.boundary(), 0, "第 3 档一次都不该触发，边界不该被推动");
    assert_eq!(plan.summary(), None);

    // 核心断言：零模型调用——一共只发生过脚本里那 10 个 hop，没有多出来的连接
    // （压缩子若被 spawn，会是第 11 个 hop）。
    assert_eq!(
        bodies.lock().unwrap().len(),
        10,
        "全程该恰好 10 次 provider 调用（1 轮工具调用两跳 + 8 轮纯文本），\
         多了就说明第 3 档偷偷开火过"
    );
    assert!(
        session.children_of(&root).is_empty(),
        "没有任何压缩子该被 spawn 过"
    );

    // 核心断言：没有任何压缩相关的 Notice。
    let events = events.borrow();
    let notices: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.event {
            RunnerEvent::Notice(n) => Some(n.clone()),
            _ => None,
        })
        .collect();
    assert!(
        !notices.iter().any(|n| matches!(
            n,
            agent_core::Notice::CompactionSummaryReceived | agent_core::Notice::CompactionFailed
        )),
        "不该有任何压缩相关的 Notice：{notices:?}"
    );
}
