//! `ToolTable` 的单元测试（红线 9：从 `tool_table.rs` 挪出来，源文件只留实现）。
//! `#[path]` 子模块，`super` 仍是 `tool_table`，私有 `mcp_reversibility` 等照样够得着。

// 076 把名字规则挪进 `tool_table_names.rs` 之后，下面四样不再顺着 `tool_table.rs`
// 的 `use` 白拿进来——测试自己点名，比在实现文件里留几个它用不上的导入干净。
use super::*;
use crate::collect_tool::COLLECT_TOOL;
use crate::spawn_tool::SPAWN_TOOL;
use crate::status_tool::STATUS_TOOL;
use agent_core::Location;

#[test]
fn builtin_specs_are_exposed_in_order() {
    let table = ToolTable::builtin();
    let names: Vec<&str> = table.specs().iter().map(|s| &*s.name).collect();
    assert_eq!(names, vec!["srv:fs/read", "srv:fs/list"]);
}

#[test]
fn known_builtin_tools_are_pure_reads() {
    let table = ToolTable::builtin();
    let snap = table.snapshot("srv:fs/read", Arc::new(Value::Null));
    assert_eq!(snap.location, Location::Server);
    assert_eq!(snap.reversibility, Reversibility::Pure);
}

/// 拿不准的工具名：位置按前缀猜，可逆性保守落 `Irreversible`——
/// 判错成 `Pure` 的代价（重复扣款）比判错成 `Irreversible`（多问一次）大。
#[test]
fn unknown_tool_defaults_to_irreversible() {
    let table = ToolTable::builtin();
    let snap = table.snapshot("web:browser/click", Arc::new(Value::Null));
    assert_eq!(snap.location, Location::Web);
    assert_eq!(snap.reversibility, Reversibility::Irreversible);
}

/// 027 开闸：`with_shell()` 在内置只读集后面追加 `srv:shell/exec`，
/// 且它落 `Irreversible`（走的是保守默认分支，不需要额外列进
/// `reversibility_of` 的已知表——`unknown_tool_defaults_to_irreversible`
/// 已经证明这条分支的判据，这里只需确认它真的在表里）。
#[test]
fn with_shell_appends_shell_exec_after_the_read_only_builtins_and_it_is_irreversible() {
    let table = ToolTable::with_shell();
    let names: Vec<&str> = table.specs().iter().map(|s| &*s.name).collect();
    assert_eq!(names, vec!["srv:fs/read", "srv:fs/list", "srv:shell/exec"]);

    let snap = table.snapshot("srv:shell/exec", Arc::new(Value::Null));
    assert_eq!(snap.location, Location::Server);
    assert_eq!(snap.reversibility, Reversibility::Irreversible);
}

/// 029 开闸：`with_spawn` 同样只追加在末尾，且 spawn 是 `Reversible`
/// （补偿 = `despawn_child`，理由见 `reversibility_of` 的注释）——它**不是**
/// 那个保守默认分支的产物，所以这里两件事都得断言。
#[test]
fn with_spawn_appends_the_spawn_tool_and_it_is_reversible() {
    let table = ToolTable::with_shell().with_spawn(AgentLimits::default());
    let names: Vec<&str> = table.specs().iter().map(|s| &*s.name).collect();
    assert_eq!(
        names,
        vec![
            "srv:fs/read",
            "srv:fs/list",
            "srv:shell/exec",
            "srv:agent/spawn"
        ]
    );

    let snap = table.snapshot(SPAWN_TOOL, Arc::new(Value::Null));
    assert_eq!(snap.location, Location::Server);
    assert_eq!(snap.reversibility, Reversibility::Reversible);
}

/// 截获闸的输入端：宿主没开子 agent，这个名字就跟别的不存在的工具一样。
#[test]
fn a_table_without_spawn_does_not_declare_it() {
    assert!(!ToolTable::builtin().declares(SPAWN_TOOL));
    assert!(
        ToolTable::builtin()
            .with_spawn(AgentLimits::default())
            .declares(SPAWN_TOOL)
    );
}

/// 051 开闸：`with_status` 同样只追加在末尾，且 status 是 `Pure`（纯读、无副作用、
/// 无屏障）——它**不是**保守默认那条分支的产物，所以这里两件事都得断言。
#[test]
fn with_status_appends_the_status_tool_and_it_is_pure() {
    let table = ToolTable::with_shell()
        .with_spawn(AgentLimits::default())
        .with_status();
    let names: Vec<&str> = table.specs().iter().map(|s| &*s.name).collect();
    assert_eq!(
        names,
        vec![
            "srv:fs/read",
            "srv:fs/list",
            "srv:shell/exec",
            "srv:agent/spawn",
            "srv:agent/status"
        ]
    );

    let snap = table.snapshot(STATUS_TOOL, Arc::new(Value::Null));
    assert_eq!(snap.location, Location::Server);
    assert_eq!(snap.reversibility, Reversibility::Pure);
}

/// 截获闸的输入端：两个开关互不牵连——只开 spawn 的宿主里 `srv:agent/status`
/// 这个名字跟别的不存在的工具走同一条路（`dispatch` 只在 `declares` 为真时截获）。
#[test]
fn a_table_without_status_does_not_declare_it() {
    assert!(
        !ToolTable::builtin()
            .with_spawn(AgentLimits::default())
            .declares(STATUS_TOOL)
    );
    assert!(ToolTable::builtin().with_status().declares(STATUS_TOOL));
}

/// 053 开闸：`with_collect` 同样只追加在末尾（红线 11：既有前缀是契约），且
/// collect 是 `Pure`——它只读一份已经产生的结果，屏障位在子自己那条 entry 上。
#[test]
fn with_collect_appends_the_collect_tool_and_it_is_pure() {
    let table = ToolTable::with_shell()
        .with_spawn(AgentLimits::default())
        .with_status()
        .with_collect();
    let names: Vec<&str> = table.specs().iter().map(|s| &*s.name).collect();
    assert_eq!(
        names,
        vec![
            "srv:fs/read",
            "srv:fs/list",
            "srv:shell/exec",
            "srv:agent/spawn",
            "srv:agent/status",
            "srv:agent/collect"
        ]
    );

    let snap = table.snapshot(COLLECT_TOOL, Arc::new(Value::Null));
    assert_eq!(snap.location, Location::Server);
    assert_eq!(snap.reversibility, Reversibility::Pure);
}

/// 截获闸的输入端：三个开关互不牵连。
#[test]
fn a_table_without_collect_does_not_declare_it() {
    assert!(
        !ToolTable::builtin()
            .with_spawn(AgentLimits::default())
            .with_status()
            .declares(COLLECT_TOOL)
    );
    assert!(ToolTable::builtin().with_collect().declares(COLLECT_TOOL));
}

/// s5 开闸：`with_vision_inspect` 同样只追加在末尾（红线 11：既有前缀是
/// 契约），且它落 `Irreversible`（调第三方 API 计费，走保守默认分支）。
#[test]
fn with_vision_inspect_appends_the_tool_and_it_is_irreversible() {
    let table = ToolTable::builtin().with_vision_inspect();
    let names: Vec<&str> = table.specs().iter().map(|s| &*s.name).collect();
    assert_eq!(
        names,
        vec!["srv:fs/read", "srv:fs/list", "srv:vision/inspect"]
    );

    let snap = table.snapshot("srv:vision/inspect", Arc::new(Value::Null));
    assert_eq!(snap.location, Location::Server);
    assert_eq!(snap.reversibility, Reversibility::Irreversible);
}

/// 没开就不声明：`srv:vision/inspect` 跟别的不存在的工具走同一条路。
#[test]
fn a_table_without_vision_inspect_does_not_declare_it() {
    assert!(!ToolTable::builtin().declares("srv:vision/inspect"));
    assert!(ToolTable::builtin()
        .with_vision_inspect()
        .declares("srv:vision/inspect"));
}

/// 上限进描述是给模型看的（029：「描述写给模型看」），换一组数就该换一份
/// 描述——不然模型读到的上限跟真正拦它的那两道闸对不上。
#[test]
fn the_declared_limits_follow_the_limits_that_are_actually_enforced() {
    let default = ToolTable::builtin().with_spawn(AgentLimits::default());
    let tighter = ToolTable::builtin().with_spawn(AgentLimits {
        max_depth: 1,
        max_children: 2,
    });
    let text = |t: &ToolTable| t.specs().last().unwrap().description.to_string();
    assert!(text(&default).contains('8'));
    assert!(text(&tighter).contains('2') && !text(&tighter).contains('8'));
}

// ── 043：MCP 可逆性映射 ──────────────────────────────────────────────────
// 验收第一条：mapped-readOnly→Pure、mapped-非readOnly / unmapped→Irreversible，
// location 恒 Server。用 041 的 `translate` 真链路造 (spec, reversibility)，证明
// `with_mcp` 装进去的正是翻译产出的那份。

use agent_mcp::{Annotations, McpTool, translate};
use serde_json::json;

fn mcp_tool(name: &str, read_only: Option<bool>) -> McpTool {
    McpTool {
        name: name.to_string(),
        description: Some(format!("{name} desc")),
        input_schema: json!({"type": "object"}),
        annotations: read_only.map(|r| Annotations {
            read_only_hint: Some(r),
        }),
    }
}

/// `everything` server 装了 readOnly 的 `echo` 和非 readOnly 的 `sendEmail`——
/// 同前缀两个工具，可逆性不同，正是「不能从名字推」的那对反例（docs/MCP.md）。
fn table_with_echo_and_send() -> ToolTable {
    let echo = translate(&mcp_tool("echo", Some(true)), "everything");
    let send = translate(&mcp_tool("sendEmail", Some(false)), "everything");
    ToolTable::builtin().with_mcp(vec![echo, send])
}

#[test]
fn mapped_read_only_mcp_tool_is_pure_and_located_on_server() {
    let snap = table_with_echo_and_send().snapshot("mcp:everything/echo", Arc::new(Value::Null));
    assert_eq!(snap.reversibility, Reversibility::Pure);
    assert_eq!(snap.location, Location::Server);
}

#[test]
fn mapped_non_read_only_mcp_tool_is_irreversible_on_server() {
    let snap =
        table_with_echo_and_send().snapshot("mcp:everything/sendEmail", Arc::new(Value::Null));
    assert_eq!(snap.reversibility, Reversibility::Irreversible);
    assert_eq!(snap.location, Location::Server);
}

/// 前缀是 `mcp:` 但映射里没有 → 保守 `Irreversible`；location 仍恒 `Server`。
#[test]
fn unmapped_mcp_tool_falls_back_to_irreversible_but_still_server() {
    let snap = table_with_echo_and_send().snapshot("mcp:everything/x", Arc::new(Value::Null));
    assert_eq!(snap.reversibility, Reversibility::Irreversible);
    assert_eq!(snap.location, Location::Server);
}

/// `with_mcp` 只追加在末尾（红线 11：顺序是既有契约），且 `declares` 认得它们。
#[test]
fn with_mcp_appends_specs_at_end_and_declares_them() {
    let table = table_with_echo_and_send();
    let names: Vec<&str> = table.specs().iter().map(|s| &*s.name).collect();
    assert_eq!(
        names,
        vec![
            "srv:fs/read",
            "srv:fs/list",
            "mcp:everything/echo",
            "mcp:everything/sendEmail"
        ]
    );
    assert!(table.declares("mcp:everything/echo"));
    assert!(!table.declares("mcp:everything/x"));
}

// ── 075：push_spec 判重（069 §拍板 D） ──────────────────────────────────────
// `with_*` 系列 push 进 `specs` 的唯一入口收进 `push_spec`：撞名 → 整条丢弃
// （spec 不 push，配套的可逆性映射也不 insert），debug_assert 点名，release 静默丢弃。

fn raw_spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from(format!("{name} 的说明书")),
        schema: Arc::new(json!({ "type": "object" })),
    }
}

/// `push_spec` 本身：撞名不该真的 push 进 `specs`——不管 debug 还是 release。
/// `push_spec` 只借用 `&mut self`（不像 `with_*` 按值消费 `self`），所以能用
/// `catch_unwind` 把 debug 构建下的 `debug_assert` panic 吞掉，之后照样读得到
/// `table` 的状态——panic 发生在任何写入之前，捕获前后状态是同一份。
#[test]
fn push_spec_leaves_specs_untouched_when_the_name_already_exists() {
    let mut table = ToolTable::builtin(); // srv:fs/read, srv:fs/list
    let before = table.specs().len();

    let dup = raw_spec("srv:fs/read");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| table.push_spec(dup)));

    if cfg!(debug_assertions) {
        let payload = result.expect_err("debug 构建下撞名必须 debug_assert 炸出来");
        let msg = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default();
        assert!(
            msg.contains("srv:fs/read"),
            "debug_assert 的 panic 消息要点得出撞的是哪个名字，实际：{msg}"
        );
    } else {
        assert!(
            !result.unwrap(),
            "release 构建下已经在表里的名字，push_spec 该返回 false"
        );
    }
    assert_eq!(table.specs().len(), before, "撞名的那条不该真的进 specs");
}

/// `with_mcp`：重复装载同一个工具名（两次的可逆性故意写得不一样——不然「留哪条」
/// 不可判定，074 的先例）→ `specs()` 长度不变、可逆性仍是**先来的**那份。
///
/// `with_mcp` 按值消费 `self`，debug 构建下撞名会让整次调用 panic，没法在同一次
/// 调用里既拿到 panic 又拿到调用之后的状态——所以这里对整个构建结果 `catch_unwind`：
/// debug 分支钉住「确实炸了」，release 分支钉住验收要的那两条断言（specs 长度、
/// 可逆性）。
#[test]
fn with_mcp_loading_the_same_name_twice_keeps_the_first_reversibility() {
    let build = || {
        ToolTable::builtin().with_mcp(vec![
            (raw_spec("mcp:dup/tool"), Reversibility::Pure),
            (raw_spec("mcp:dup/tool"), Reversibility::Irreversible),
        ])
    };
    let result = std::panic::catch_unwind(build);
    if cfg!(debug_assertions) {
        assert!(
            result.is_err(),
            "debug 构建下重复装载应该在 with_mcp 内部 debug_assert 炸掉"
        );
    } else {
        let table = result.expect("release 构建下 with_mcp 不该 panic");
        assert_eq!(
            table
                .specs()
                .iter()
                .filter(|s| &*s.name == "mcp:dup/tool")
                .count(),
            1,
            "撞名的那条不该真的多进一条 spec"
        );
        assert_eq!(
            table
                .snapshot("mcp:dup/tool", Arc::new(Value::Null))
                .reversibility,
            Reversibility::Pure,
            "可逆性该是先来的那份（Pure），不是后来的 Irreversible"
        );
    }
}

/// `debug_assert!` 点得出名字：常规 `cargo test`（debug_assertions 打开）下，
/// 重复装载会直接 panic，panic 消息里含撞的那个工具名。
#[test]
#[should_panic(expected = "mcp:dup/tool")]
fn with_mcp_names_the_offender_in_a_debug_build() {
    let _ = ToolTable::builtin().with_mcp(vec![
        (raw_spec("mcp:dup/tool"), Reversibility::Pure),
        (raw_spec("mcp:dup/tool"), Reversibility::Irreversible),
    ]);
}
