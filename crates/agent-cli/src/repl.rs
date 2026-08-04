//! 会话循环：读一行 → 要么是斜杠命令（`/quit`、`/model <name>`、`/undo`、
//! `/redo`、`/undo!`、`/skills`、`/agents`、`/mcp`），要么喂给 `agent_runtime::run_turn` 跑一整轮
//! （可能含若干次 provider 调用/工具调用）→ 打摘要 → 终态就 `Session::begin_turn`
//! 开下一轮 → 再读一行。`/quit` 退出，EOF（Ctrl-D）也退出。
//!
//! **027 换接**：状态从 `TurnState`（栈上单份、不持久化）换成
//! [`agent_core::Session`]（`main.rs` 建好——可能是全新的，也可能是从
//! `SessionStore` 恢复出来的）。「取消轮丢弃」不再是这里手写的「截断消息列表」
//! （022 时代那招退役），而是 [`crate::undo::after_cancelled_turn`] 调
//! `Session::undo_turn` 的正牌答案。

use std::io::{self, BufRead, Write};

use agent_core::{Failure, Session, TurnStatus};
use agent_runtime::{RunnerCtx, run_turn};
use agent_transport::config::RootConfig;

use crate::mcp::McpStatus;
use crate::{model_switch, undo};

/// `config` 是启动时已经加载好的整份 `providers.toml`（014）——`/model <name>`
/// 切换要从里面查表，不重新读一次文件；这份引用的生命周期跟会话一样长，
/// `main.rs` 持有它，这里只借。
///
/// `mcp` 是启动时 `mcp::bootstrap` 产出的装载状态（045）——`/mcp` 命令渲染它。
/// 会话期间不变（server 在 bootstrap 就装好了），跟 `config` 一样 `main.rs` 持有、
/// 这里只借。用装载状态而非活 registry：起不来的 server 的原因只在这份状态里。
///
/// `session` 由调用方建好（`Session::new` 或者 027 的崩溃恢复），这个函数
/// 只负责驱动：**只在终态之后才 `begin_turn`**——`Session::new` 与恢复出来的
/// 会话都可能已经是 `Idle`（前者永远是，后者取决于恢复点），此时第一条
/// `UserInput` 直接喂给 `run_turn` 就够，多调一次 `begin_turn` 会把 `turn_id`
/// 平白推进一格。恢复出来卡在非终态（`ToolsPending`/`Thinking`，020/027 的
/// 「未收敛槽位不自动重发」）也不调——那种状态下第一条新输入会被转移表判成
/// 协议违规（状态原样不动），用户用 `/undo` 摆脱它，不是这里悄悄开新的一轮。
pub fn run(session: &mut Session, ctx: &mut RunnerCtx, config: &RootConfig, mcp: &McpStatus) {
    let stdin = io::stdin();
    loop {
        print!("> ");
        let _ = io::stdout().flush();

        let mut line = String::new();
        let read = stdin.lock().read_line(&mut line);
        let Ok(n) = read else {
            eprintln!("读输入失败: {}", read.unwrap_err());
            break;
        };
        if n == 0 {
            println!(); // Ctrl-D：EOF，干净地换行退出
            break;
        }
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        match input {
            "/quit" => break,
            "/undo" => {
                undo::undo(session, ctx);
                continue;
            }
            "/undo!" => {
                undo::undo_force(session, ctx);
                continue;
            }
            "/redo" => {
                undo::redo(session, ctx);
                continue;
            }
            "/skills" => {
                print_skills(session, ctx);
                continue;
            }
            "/agents" => {
                print_agent_tree(session);
                continue;
            }
            "/mcp" => {
                println!("{}", crate::print::render_mcp_status(mcp));
                continue;
            }
            _ => {}
        }
        if let Some(name) = input.strip_prefix("/model ") {
            model_switch::switch(name.trim(), ctx, session, config);
            continue;
        }

        if session.status().is_terminal() {
            session.begin_turn();
            agent_runtime::persist::sync(ctx, session);
        }

        // 取消标志的清零在 `run_turn` 内部做（跟 022 时代 `repl::run` 手动
        // `cancel.store(false, ..)` 是同一个理由：上一轮遗留的标志不该提前
        // 打断这一轮还没开始的请求），这里不用重复。
        let status = run_turn(session, ctx, input);
        crate::print::turn_outcome(&status);

        if matches!(status, TurnStatus::Failed(Failure::Cancelled)) {
            undo::after_cancelled_turn(session, ctx);
        }
        // 其余情况（正常终态 / 非终态卡住）状态原样留着：正常终态等下一轮
        // 输入时上面那个 `begin_turn` 分支处理；非终态已经打过一条协议违规
        // 通报，用户可以 /quit 重开或者 /undo。
    }
}

/// `/skills`（039 step 6）：列出宿主装载的全部 skill，标出哪些已激活。
/// 「有哪些可用」问 registry（`ctx.available_skills`），「哪些激活」问 `Session`
/// （`active_skills`）——两个来源各答各的，跟 `docs/TOOLS.md` §Skills 的分工一致。
fn print_skills(session: &Session, ctx: &RunnerCtx) {
    let available = ctx.available_skills();
    if available.is_empty() {
        println!("（没有装载任何 skill。把 <name>/SKILL.md 放进启动目录的 ./skills/ 下。）");
        return;
    }
    let active: std::collections::BTreeSet<String> =
        session.active_skills().into_iter().map(|s| s.as_str().to_string()).collect();
    println!("skills（[*] = 已激活）:");
    for (id, description) in available {
        let mark = if active.contains(&*id) { "*" } else { " " };
        println!("  [{mark}] {id}: {description}");
    }
}

/// `/agents`（047）：调 `session.agent_tree()`，渲染成缩进文本树。**只读**——
/// 不改任何状态，就是 `agent_tree()` 的一个文本渲染器（`docs/OBSERVABILITY.md`
/// §「snapshot，不是 reconstruct」：树由 core 权威算，这里只画）。渲染本身在
/// `crate::print::agent_tree`（纯格式化，独立单测），这里只负责取快照 + 打印。
fn print_agent_tree(session: &Session) {
    let tree = session.agent_tree();
    println!("{}", crate::print::render_agent_tree(&tree));
}
