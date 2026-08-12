//! `register_builtin_intercepts` 的单元测试（红线 9：从 `builtin_intercepts.rs`
//! 挪出来，源文件只留实现）。`#[path]` 子模块，`super` 就是 `builtin_intercepts`。
//!
//! 只测「declares() ⟺ 注册」这一件事本身——四个闭包各自转发对不对，已经由
//! `spawn_tool`/`collect_tool`/`status_tool`/`skill::read` 各自的单测，以及
//! `tests/it/` 下真的跑一轮 `run_turn` 的既有集成测试覆盖，这里重复断言只会
//! 两处对不上时都要改。

use std::sync::Arc;

use agent_core::{AgentLimits, SessionConfig};
use agent_providers::deepseek::DeepSeek;
use agent_tools::ToolExecutor;
use agent_transport::Client;

use crate::collect_tool::COLLECT_TOOL;
use crate::ctx::RunnerCtx;
use crate::skill::SKILL_READ;
use crate::spawn_tool::SPAWN_TOOL;
use crate::status_tool::STATUS_TOOL;
use crate::tool_table::ToolTable;

/// 跟 `ctx_tests.rs::build` 同款最小装配，只是 `tools` 参数化——这几条测试只
/// 关心 `declares()`/`session_tool_registered()` 这两件事，不需要真的打网络。
fn build(tools: ToolTable) -> RunnerCtx {
    let fs = ToolExecutor::new(std::env::temp_dir()).unwrap();
    RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(Client::new()),
        "https://api.deepseek.com/chat/completions".to_string(),
        "deepseek-key".to_string(),
        fs,
        tools,
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

/// 四档能力都没开：`builtin()` 不 `declares()` 任何一个，注册表里也不该有
/// ——`RunnerCtx::new` 已经在构造链里调过 `register_builtin_intercepts`，这里
/// 直接读结果，不用再调第二次。
#[test]
fn nothing_declared_means_nothing_registered() {
    let ctx = build(ToolTable::builtin());
    for name in [SPAWN_TOOL, COLLECT_TOOL, STATUS_TOOL, SKILL_READ] {
        assert!(!ctx.tools().declares(name), "前提：builtin() 不该声明 {name}");
        assert!(
            !ctx.session_tool_registered(name),
            "没声明就不该注册截获——半开状态"
        );
    }
}

/// 开了 spawn/collect/status 三档、没开 skill-read：`declares()` 为真的名字，
/// 注册表里也该有；没开的那个两边都该是假——「declares ⟺ registered」不是巧合，
/// 是 [`register_builtin_intercepts`] 那条 `debug_assert_eq!` 钉住的不变量。
#[test]
fn declared_capabilities_are_registered_and_undeclared_ones_are_not() {
    let tools = ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_collect()
        .with_status();
    let ctx = build(tools);
    for name in [SPAWN_TOOL, COLLECT_TOOL, STATUS_TOOL] {
        assert!(ctx.tools().declares(name), "前提：这三档都开了");
        assert!(
            ctx.session_tool_registered(name),
            "`{name}` declares() 为真却没注册截获——147 要挡的半开状态"
        );
    }
    assert!(!ctx.tools().declares(SKILL_READ), "前提：没开 skill");
    assert!(!ctx.session_tool_registered(SKILL_READ));
}
