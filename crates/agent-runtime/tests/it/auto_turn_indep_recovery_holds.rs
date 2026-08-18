//! 211 独立验收 · 第 5 条：**恢复不自开**。
//!
//! 留言在收件箱里时把会话真的落盘（`Jsonl`）→ drop 掉进程 1 的 `ctx` → 新进程
//! `agent_runtime::recover` 载回来 → 断言**没有任何 provider 调用发生**、留言
//! 仍在、并且 `report_recovered_mail` 报了一条 `AutoTurnHeld{Recovered}`。
//!
//! 落盘/重启手法照抄 `jsonl_restart_continues.rs`：真 `Jsonl`，不是内存后端。
//! 恢复之后**只调 `report_recovered_mail`，不调 `run_auto_turns`**——这正是
//! 211 §3「恢复不自动开轮」在宿主这一层该做的事：不是靠预算数字拦，是靠调用点
//! 压根不调驱动器。
//!
//! 黑盒来源与「实现体没读」的声明见 `auto_turn_indep_support/mod.rs` 顶部。

use std::sync::Arc;

use agent_core::{AgentId, AgentLimits, Deliver, Session, TurnStatus};
use agent_runtime::{AutoTurnHold, RunnerCtx, ToolTable, report_recovered_mail};

use crate::auto_turn_indep_support::{Leg, RoutedServer, auto_turn_held, chain_routes, temp_dir};

const KICKOFF: &str = "KICKOFF-recover 留条笔记就落盘";

fn build_ctx_with_jsonl(
    port: u16,
    root: &std::path::Path,
    session_path: &std::path::Path,
) -> RunnerCtx {
    use agent_core::SessionConfig;
    use agent_providers::deepseek::DeepSeek;
    use agent_tools::ToolExecutor;
    use agent_transport::Client;

    let client = Client::with_config(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(50),
        agent_transport::Backoff {
            base: std::time::Duration::from_millis(10),
            max_attempts: 1,
        },
    );
    let fs = ToolExecutor::new(root).unwrap();
    RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(client),
        format!("http://127.0.0.1:{port}/chat/completions"),
        "fake-key".to_string(),
        fs,
        ToolTable::builtin().with_spawn(AgentLimits::default()).with_send(),
        Vec::new(),
        SessionConfig {
            model: Arc::from("deepseek-v4-pro"),
            temperature: None,
            max_tokens: None,
            context_window: None,
        },
        agent_runtime::open_backend(Some(session_path.to_path_buf()), |e| {
            panic!("不该有会话文件错误：{e}")
        }),
        Box::new(|_ev| {}),
    )
}

#[test]
fn recovering_a_session_with_pending_mail_never_calls_the_provider() {
    let dir = temp_dir("auto-turn-recover");
    let session_path = dir.join("session.jsonl");

    let leg = Leg {
        trigger_needle: KICKOFF,
        spawn_call_id: "call_spawn_0rec",
        task_needle: "TASK-A1rec",
        send_call_id: "call_send_1rec",
        note_text: "RECOVERNOTE-1",
        child_final_text: "A1-DONE-rec",
        root_final_text: "ROOT-T0-DONE-rec",
    };

    // ---- 「进程 1」：跑出一条待读笔记，落盘，drop 掉 ctx。----
    {
        let server = RoutedServer::start(chain_routes(std::slice::from_ref(&leg)));
        let mut ctx = build_ctx_with_jsonl(server.port, &dir, &session_path);
        let mut session = Session::new(AgentId::root());
        session.set_agent_limits(AgentLimits {
            max_auto_turns: 3,
            ..AgentLimits::default()
        });

        let status = agent_runtime::run_turn(&mut session, &mut ctx, KICKOFF)
            .expect("kickoff 不是 source failure");
        assert_eq!(status, TurnStatus::Done { truncated: false });
        assert_eq!(session.inbox_of(&AgentId::root()).len(), 1);

        agent_runtime::persist::sync(&mut ctx, &mut session);
        // `ctx`（连同它的 `Jsonl`）在这里 drop——真的落盘。**没有调
        // `run_auto_turns`**：这个会话「崩溃」在留言还没被自动读到的那一刻。
    }

    // ---- 「进程 2」：全新 backend 指向同一路径，recover 载回来。----
    let backend = agent_runtime::open_backend(Some(session_path.clone()), |e| {
        panic!("不该有加载错误：{e}")
    });
    let recovered = match agent_runtime::recover(
        backend.as_ref(),
        AgentId::root(),
        agent_core::DEFAULT_HISTORY_CAP,
        AgentLimits {
            max_auto_turns: 3,
            ..AgentLimits::default()
        },
        &mut |k| panic!("不该有不认识的键：{k:?}"),
    ) {
        Ok(Some(session)) => session,
        Ok(None) => panic!("写过一轮，该恢复出 Some"),
        // **这条目前会红，而且红得不是这份测试的锅**：`crates/agent-core/src/
        // command/inbox.rs`（非禁读文件）第 135 行，`deliver_to`/`deliver`
        // 那条转移用 `self.commit_as(from, "deliver", |txn| ..)` 落 entry；
        // 而 `crates/agent-core/src/command/meta.rs` 的 `KNOWN_LABELS`
        // 常量——`persist::recover` 把落盘的 label 字符串译回 `EntryMeta`
        // 就靠这张表——没有 `"deliver"` 这一项。结果是：**任何一个用过
        // `srv:agent/send` 投递过消息的会话，只要落盘再恢复，`recover` 就会
        // 硬失败**（`RecoverError::InvalidHistory(UnknownLabel("deliver"))`），
        // 不分是不是 211 的自驱动场景——206 的留言机制和 027 的持久化机制拼在
        // 一起就会炸，这条独立测试踩中的正是 211 §验收第 5 条要求的那个组合
        // （留言 + 落盘 + 恢复）。见本文件顶部黑盒来源声明之外的报告。
        Err(e) => panic!(
            "recover 失败：{e}——真实原因是 KNOWN_LABELS 里没有 \"deliver\"，\
             凡是投递过消息的会话都无法恢复，见本文件这条分支上方的注释"
        ),
    };

    // ① 留言仍在：恢复出来的会话收件箱里还是那一条。
    let root = AgentId::root();
    let inbox = recovered.inbox_of(&root);
    assert_eq!(inbox.len(), 1, "笔记该原样恢复出来：{inbox:?}");
    assert_eq!(inbox[0].when, Deliver::NextTurn);
    assert_eq!(&*inbox[0].text, "RECOVERNOTE-1");

    // ② 一个**空路由表**的服务器：只要有任何 provider 调用发生，连接会被直接
    // 断开（`support/routed.rs`：「没有路由认领：直接断开」），不会挂起，
    // `report_recovered_mail` 如果真的调了驱动器，这里会看见非零的调用数。
    let empty_server = RoutedServer::start(Vec::new());
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = std::rc::Rc::clone(&events);
    let mut ctx2 = build_ctx_with_jsonl(empty_server.port, &dir, &session_path)
        .with_agent_events(Box::new(move |ev| sink.borrow_mut().push(ev)));

    report_recovered_mail(&recovered, &mut ctx2);

    assert!(
        empty_server.calls().is_empty(),
        "恢复路径不该触发任何 provider 调用：{:?}",
        empty_server.calls().iter().map(|c| c.needle).collect::<Vec<_>>()
    );

    // ③ `report_recovered_mail` 报了一条 `AutoTurnHeld{Recovered}`。
    assert_eq!(
        auto_turn_held(&events.borrow()),
        vec![(1, AutoTurnHold::Recovered)],
        "该恰好报一条：还有 1 条留言，理由是刚恢复"
    );

    // ④ 规格里写的实现事实（211 差异说明 §3）：恢复出来的预算是 0，不是配置的
    // 上限。这里显式钉一下——如果这条红了，说明「恢复不自开」真正靠的是预算
    // 数字本身，而不是「宿主不调驱动器」，两种机制二选一，规格该说清是哪一种。
    assert_eq!(
        recovered.auto_turn_budget(),
        0,
        "规格说的是：恢复出来这一格是 0，不是配置的上限"
    );
}
