//! 148 两份装配单测共用的夹具：那个**测试用扩展包**，外加造一张最小 `RunnerCtx`。
//!
//! 拆出来是因为 `tool_table_extension_tests.rs`（装配落位）和
//! `tool_table_extension_guard_tests.rs`（各道闸的 release 语义）都要用同一批
//! 名字/哨兵串/夹具——照 `tests/it/*_support` 的既有先例，公共部分只住一处。
#![allow(dead_code, reason = "两份测试各用其中一部分")]

use std::sync::{Arc, Mutex};

use agent_core::{AgentId, Reversibility, Session, SessionConfig, ToolSpec};
use agent_providers::deepseek::DeepSeek;
use agent_tools::ToolExecutor;
use agent_transport::Client;
use serde_json::{Value, json};

use crate::SessionToolFn;
use crate::ctx::RunnerCtx;
use crate::extension_pack::ExtensionPack;
use crate::tool_table::{CallTiming, ToolTable};

pub(super) const PACK: &str = "demo";
pub(super) const READ_TOOL: &str = "ext:demo/tree_echo";
pub(super) const HOOK_TOOL: &str = "ext:demo/turn_end_ping";
pub(super) const WRITE_TOOL: &str = "ext:demo/mark_plan";
pub(super) const READ_SENTINEL: &str = "TREE-ECHO-9f2a";
pub(super) const HOOK_SENTINEL: &str = "turn-end-ping";

/// 包里那条**纯读截获工具**的函数体。写成具名函数（而不是就地闭包）是为了让它
/// 自己也能被直接调用一次——见 `the_packs_read_tool_narrows_to_the_callers_subtree`。
///
/// 它顺带演示正门的纪律（`docs/EXTENSIONS.md` §正门）：拿到的是整个
/// `&mut Session`，但只数**调用者的后代**（红线 10），不把整棵树喂给模型。
pub(super) fn tree_echo(
    session: &mut Session,
    agent: &AgentId,
    _input: &Value,
) -> Result<Arc<str>, Arc<str>> {
    let tree = session.agent_tree();
    let mine = tree
        .nodes
        .iter()
        .filter(|node| node.id.is_descendant_of(agent))
        .count();
    Ok(Arc::from(format!("{READ_SENTINEL} descendants={mine}")))
}

/// 验收要的那个测试用 pack：一条纯读截获 + 一条 TurnEnd 钩子。钩子被调到时往
/// `log` 里记一笔——`TimedRun` 是 `Fn` 不是 `FnMut`，跨调用留痕靠内部可变性
/// （同 `turn_end.rs` 单测里的既有写法）。
pub(super) fn test_pack(log: Arc<Mutex<Vec<&'static str>>>) -> ExtensionPack {
    ExtensionPack::new(PACK)
        .with_tool(
            spec(READ_TOOL, "数一数调用者有几个后代"),
            Reversibility::Pure,
            Box::new(tree_echo),
        )
        .with_timed(
            spec(HOOK_TOOL, "每轮收尾记一笔"),
            CallTiming::TurnEnd,
            recording_hook(log),
        )
}

/// 一条只记账的 `TurnEnd` 执行体。
pub(super) fn recording_hook(log: Arc<Mutex<Vec<&'static str>>>) -> crate::tool_table::TimedRun {
    Box::new(move |_table, _session: &Session, _input: &Value| {
        log.lock().unwrap().push(HOOK_SENTINEL);
        Ok(Arc::from("pinged"))
    })
}

/// 一个最小合法 `ToolSpec`：schema 是空 object，够 `declares()` 判真就行。
pub(super) fn spec(name: &str, description: &str) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from(description),
        schema: Arc::new(json!({ "type": "object" })),
    }
}

/// 一条什么都不干的截获执行体——被测的是装配，不是它做了什么。
pub(super) fn nop_tool() -> SessionToolFn {
    Box::new(|_session: &mut Session, _agent: &AgentId, _input: &Value| Ok(Arc::from("ok")))
}

pub(super) fn log() -> Arc<Mutex<Vec<&'static str>>> {
    Arc::new(Mutex::new(Vec::new()))
}

/// 一张够用的 `RunnerCtx`：这些用例从不发请求，provider/endpoint/key 只要能造出
/// 对象即可（同 `turn_end.rs` 单测里的 `build_ctx`）。
pub(super) fn build_ctx(table: ToolTable) -> RunnerCtx {
    let fs = ToolExecutor::new(std::env::temp_dir()).unwrap();
    RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(Client::new()),
        "https://api.deepseek.com/chat/completions".to_string(),
        "runtime-test-key".to_string(),
        fs,
        table,
        Vec::new(),
        SessionConfig {
            model: Arc::from("deepseek-v4-pro"),
            temperature: None,
            max_tokens: None,
            context_window: None,
        },
        crate::persist::open_backend(None, |_| {}),
        Box::new(|_| {}),
    )
}
