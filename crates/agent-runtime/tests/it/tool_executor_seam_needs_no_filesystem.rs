//! 112 验收：`RunnerCtx.fs` 不再 structurally 要求一个真实文件系统目录。
//!
//! `agent_tools::NullToolExecutor` 不 canonicalize 任何目录、不摸磁盘；
//! `RunnerCtx::new` 现在收 `impl ToolExecution + 'static`（`agent-tools` 的新接缝，
//! `agent-runtime/src/ctx.rs`），native 装配路径喂 `ToolExecutor` 时逐字节不变，
//! 这里喂 `NullToolExecutor` 换出一条完全不接触文件系统的路径。
//!
//! 这条测试拿它跑一整轮真实 loop：一次工具调用命中它、被拒
//! （`no_tool_executor`），loop 按 003 的哲学吞下这次失败继续到收敛（不是卡在
//! `ToolsPending`，也不是 panic），随后 `/undo` 把这一轮完整退回去——状态流转与
//! undo 在没有文件系统的宿主上跟 native 路径一样成立。
//!
//! **不建任何用完即扔的一次性目录**：这是本文件存在的全部意义。
//! `docs/ARCHITECTURE.md` 写着「mock 一个 tool executor 就能离线测 loop」，
//! 112 之前这句话是假的——唯一能构造的 executor 是 `ToolExecutor::new(root)`，
//! `root` 必须真实存在，mock 只能靠给一个真实存在的目录，那是集成测试不是单元
//! 测试。这条测试证明现在真的不需要了：本文件不引入任何一次性目录辅助类型
//! （验收口径见 112 issue 原文，此处不复述关键词，避免文件内容自我命中）。

use std::sync::Arc;

use agent_core::{AgentId, ContentBlock, Session, SessionConfig, TurnStatus, UndoReport};
use agent_providers::deepseek::DeepSeek;
use agent_providers::wire_name;
use agent_runtime::{RunnerCtx, ToolTable, run_turn};
use agent_tools::NullToolExecutor;
use agent_transport::Client;

use crate::support::{sse_text, sse_tool_call};

/// 跟 `support::build_ctx*` 系列故意不同：那一族固定接 `ToolExecutor::new(root)`,
/// 存在的理由就是覆盖 native 路径。这里手工装一份，`fs` 换成 `NullToolExecutor`,
/// 全函数没有一次 `std::fs` 调用，也不引入任何一次性目录辅助类型。
fn build_ctx_without_a_filesystem(port: u16) -> RunnerCtx {
    RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(Client::new()),
        format!("http://127.0.0.1:{port}/chat/completions"),
        "fake-key".to_string(),
        NullToolExecutor,
        ToolTable::builtin(),
        Vec::new(),
        SessionConfig {
            model: Arc::from("deepseek-v4-pro"),
            temperature: None,
            max_tokens: None,
            context_window: None,
        },
        agent_runtime::open_backend(None, |_| {}),
        Box::new(|_ev| {}),
    )
}

fn tool_results(session: &Session) -> Vec<(String, bool)> {
    session
        .messages()
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => Some((content.to_string(), *is_error)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_null_executor_runs_a_full_turn_and_undo_without_touching_disk() {
    let port = crate::support::spawn_scripted_server(vec![
        sse_tool_call(
            "call_1",
            &wire_name::to_wire("srv:fs/read"),
            r#"{\"path\": \"whatever.txt\"}"#,
        ),
        sse_text("那个工具不可用，我换个办法。"),
    ]);
    let mut ctx = build_ctx_without_a_filesystem(port);
    let mut session = Session::new(AgentId::root());

    let status = agent_runtime::block_on(run_turn(&mut session, &mut ctx, "读一个文件"));

    // 1. loop 没有因为工具被拒而中止（003 哲学：工具失败不中止 loop），
    //    跑完两跳收敛——不是卡在 `ToolsPending`。
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "NullToolExecutor 拒绝工具调用不该让 loop 卡住：{status:?}"
    );

    // 2. 工具调用真的到达了 executor 并被明确拒绝（不是被别的路由分流掉、
    //    压根没执行到）。
    let results = tool_results(&session);
    assert_eq!(results.len(), 1, "该正好有一条 tool_result: {results:#?}");
    assert!(
        results[0].1,
        "NullToolExecutor 的拒绝该落 is_error：{results:#?}"
    );
    assert!(
        results[0].0.contains("no_tool_executor"),
        "错误里该说清是「这个宿主没有本地 executor」：{results:#?}"
    );

    // 3. undo：这一轮完整退回去。`srv:fs/read` 是 Pure
    //    （`tool_table_names.rs::reversibility_of`），没有屏障——`undo_turn`
    //    （不是 `_force`）就该走完，跟 native 路径上「一次被拒绝的纯读调用」
    //    的 undo 行为一致。
    let report = session.undo_turn();
    assert!(
        matches!(report, UndoReport::Applied { .. }),
        "该整轮退掉，没有屏障拦得住一次被拒绝的纯读调用：{report:?}"
    );
    assert!(
        session.messages().is_empty(),
        "undo 之后消息历史该清空，跟这一轮开始之前一致"
    );
}
