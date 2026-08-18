//! 148 的**各道闸**单测：撞上前缀强制、只装一半、包内重名时各自会怎样。
//!
//! 装配正常落位那一面在姊妹文件 `tool_table_extension_tests.rs`；共用夹具在
//! `tool_table_extension_fixtures.rs`。
//!
//! 每道闸都是「debug 炸 + release 丢弃」两个分支，所以这里每条 release 语义的用例
//! 都用 `catch_unwind` 兼顾两种构建——同 `tool_table_timed_tests.rs` 的既有写法。
//! debug 那一半的文案则由 `extension_pack_tests.rs` 逐条钉住。

use std::sync::Arc;

use agent_core::{AgentId, Session};

use super::fixtures::{
    PACK, READ_TOOL, build_ctx, log, nop_tool, recording_hook, spec, test_pack,
};
use super::*;
use crate::tool_table::CallTiming;
use crate::turn_end;

/// 裸名 / 冒用 `srv:` 的条目在 debug 下当场炸（文案见 `extension_pack_tests.rs`），
/// 这里钉 release 那一半：**被丢的条目既不进 prompt、也不进任何执行路径**，同一包
/// 里合法的那条照常装——丢的粒度是「这一条」，不是「整包」。
#[test]
fn a_misnamed_entry_reaches_neither_the_prompt_nor_any_execution_path() {
    let log = log();
    let hook_log = Arc::clone(&log);
    let build = move || {
        ExtensionPack::new(PACK)
            .with_tool(spec("tree_echo", "裸名"), nop_tool())
            .with_tool(spec("srv:demo/shell", "冒用 srv:"), nop_tool())
            .with_timed(
                spec("turn_end_ping", "裸名钩子"),
                CallTiming::TurnEnd,
                recording_hook(hook_log),
            )
            .with_tool(spec(READ_TOOL, "同一包里合法的那条"), nop_tool())
    };

    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(build));
    if cfg!(debug_assertions) {
        assert!(built.is_err(), "debug 构建下冒名条目必须炸");
        return;
    }

    let (tools, pending) = ToolTable::builtin().with_extension(built.ok().unwrap());
    assert!(!tools.declares("tree_echo"), "裸名不该进 prompt");
    assert!(!tools.declares("srv:demo/shell"), "冒用的前缀不该进 prompt");
    assert_eq!(tools.timed(CallTiming::TurnEnd).count(), 0);
    assert!(tools.declares(READ_TOOL), "合法的那条照常装（丢一条不丢整包）");

    let mut ctx = build_ctx(tools);
    pending.install(&mut ctx);
    assert!(!ctx.session_tool_registered("tree_echo"));
    assert!(ctx.session_tool_registered(READ_TOOL));
    turn_end::fire(&ctx, &Session::new(AgentId::root()));
    assert!(log.lock().unwrap().is_empty());
}

/// 丢掉 ctx 半边 = debug 构建当场炸，文案点名是哪个包。
#[test]
#[should_panic(expected = "扩展包 `demo` 的 PendingInterceptors 没被 install")]
fn dropping_the_ctx_half_is_loud_in_debug() {
    let (_tools, _pending) = ToolTable::builtin().with_extension(test_pack(log()));
}

/// 上一条的 release 分支：不 panic，但**半开是真的半开**——specs 已经进了 prompt，
/// 截获却一条没注册。这条用例把炸弹那句话钉成事实：它警告的不是一个假想的危险，
/// 就是下面两行断言描述的状态（模型看得见这个工具，调了只会拿到 `unknown_tool`）。
#[test]
fn the_table_half_survives_a_dropped_ctx_half_and_that_is_exactly_the_hazard() {
    let build = || {
        let (tools, _pending) = ToolTable::builtin().with_extension(test_pack(log()));
        tools
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(build));
    if cfg!(debug_assertions) {
        assert!(result.is_err(), "debug 构建下必须炸");
        return;
    }

    let tools = result.ok().unwrap();
    assert!(tools.declares(READ_TOOL), "specs 已经进了 prompt……");
    let ctx = build_ctx(tools);
    assert!(
        !ctx.session_tool_registered(READ_TOOL),
        "……而截获一条都没注册：这就是炸弹要吼的那件事"
    );
}

/// 包里同一个名字写了两遍：`push_spec` 丢后来的那条（075），**它的执行体也不会
/// 跟着进 ctx 半边**——否则 `install` 会拿一个只有一份声明的名字去撞 146 的第三道
/// 闸，一个问题炸两次；release 下更糟：第二条执行体会悄悄顶掉第一条的执行路径，
/// 而模型看到的仍是第一条的说明书。
#[test]
fn a_spec_the_table_refused_never_leaves_its_interceptor_behind() {
    let assemble = || {
        let pack = ExtensionPack::new(PACK)
            .with_tool(spec(READ_TOOL, "第一条"), nop_tool())
            .with_tool(spec(READ_TOOL, "重名的第二条"), nop_tool());
        let (tools, pending) = ToolTable::builtin().with_extension(pack);
        let mut ctx = build_ctx(tools);
        pending.install(&mut ctx);
        ctx
    };
    let assembled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(assemble));

    if cfg!(debug_assertions) {
        assert!(
            assembled.is_err(),
            "debug 构建下包内重名必须炸（push_spec 那道闸）"
        );
        return;
    }

    let ctx = assembled.ok().unwrap();
    let table = ctx.tools();
    assert_eq!(
        table
            .specs()
            .iter()
            .filter(|s| &*s.name == READ_TOOL)
            .count(),
        1,
        "模型面只能有一份说明书"
    );
    assert_eq!(
        &*table
            .specs()
            .iter()
            .find(|s| &*s.name == READ_TOOL)
            .unwrap()
            .description,
        "第一条",
        "留下的是先到的那条"
    );
    assert!(ctx.session_tool_registered(READ_TOOL));
}
