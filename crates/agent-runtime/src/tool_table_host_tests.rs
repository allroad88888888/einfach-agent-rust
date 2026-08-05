//! 宿主注入（062）的单元测试（红线 9：实现文件只留实现，`#[path]` 子模块）。
//! 验收对着 issue 062 §验收：进得来、排在表尾、可逆性三级优先级、location 恒从
//! 前缀推、客户端数组顺序进不了 prompt。
//!
//! **作用域隔离**（同一个 server 上不带声明的会话看不到这些工具）在这一层是白拿的
//! ——这张表是 `ToolTable` 的一部分，`ToolTable` 每个会话一份。真正把它端到端钉死的
//! 是 `agent-server/tests/http_capabilities_scoped_to_one_session.rs`（两个 chatid
//! 跑在同一个进程上，各自的 prompt 里有什么由假上游收到的请求体作证）。

use std::sync::Arc;

use agent_core::{Location, Reversibility, ToolSpec};
use serde_json::{Value, json};

use super::ToolTable;

fn spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from(format!("{name} 干的事")),
        schema: Arc::new(json!({ "type": "object" })),
    }
}

fn host(name: &str, reversibility: Reversibility) -> (ToolSpec, Reversibility) {
    (spec(name), reversibility)
}

fn names(table: &ToolTable) -> Vec<String> {
    table.specs().iter().map(|s| s.name.to_string()).collect()
}

fn snap(table: &ToolTable, tool: &str) -> agent_core::ToolCallRequest {
    table.snapshot(tool, Arc::new(Value::Null))
}

/// 验收「注入排在表尾」：拿一张**不带注入**的表做基线，带注入那张的前 N 项与基线
/// 逐项相同——所有会话共有的那一段（连 MCP 之后）一个字节不动（红线 11）。
#[test]
fn injected_tools_are_appended_after_everything_the_sessions_share() {
    let baseline =
        ToolTable::with_shell().with_mcp(vec![(spec("mcp:everything/echo"), Reversibility::Pure)]);
    let injected = ToolTable::with_shell()
        .with_mcp(vec![(spec("mcp:everything/echo"), Reversibility::Pure)])
        .with_host_tools(vec![
            host("web:crm/lookup", Reversibility::Pure),
            host("desk:clipboard/write", Reversibility::Irreversible),
        ]);

    let (base, all) = (names(&baseline), names(&injected));
    assert_eq!(all[..base.len()], base[..], "共有的那一段必须逐项相同");
    assert_eq!(
        all[base.len()..],
        ["desk:clipboard/write", "web:crm/lookup"],
        "注入的排在表尾，且按名字排序"
    );
    assert!(injected.declares("web:crm/lookup"));
    assert!(
        !baseline.declares("web:crm/lookup"),
        "没注入的表里不该有这个名字"
    );
}

/// 红线 11 第二条（HOST-CAPABILITIES §六）：客户端给的数组顺序不可靠，**不许**变成
/// prompt 字节。打乱输入顺序，表里的顺序（连同 `ToolSpec` 的内容）必须一模一样。
/// 删掉 `with_host_tools` 里那行 `sort_by` 这条就红。
#[test]
fn the_client_array_order_never_reaches_the_table() {
    let one = ToolTable::builtin().with_host_tools(vec![
        host("web:b/second", Reversibility::Pure),
        host("desk:a/first", Reversibility::Reversible),
        host("web:a/third", Reversibility::Irreversible),
    ]);
    let other = ToolTable::builtin().with_host_tools(vec![
        host("web:a/third", Reversibility::Irreversible),
        host("web:b/second", Reversibility::Pure),
        host("desk:a/first", Reversibility::Reversible),
    ]);
    assert_eq!(names(&one), names(&other));
    assert_eq!(
        names(&one)[2..],
        ["desk:a/first", "web:a/third", "web:b/second"]
    );
    // 顺序一样还不够：同名那一项的可逆性也不能被顺序影响。
    assert_eq!(
        snap(&one, "web:a/third").reversibility,
        snap(&other, "web:a/third").reversibility
    );
}

/// 验收「声明了就用」：三个等级原样落地；`location` 恒走既有的 `location_of`
/// （`web:` → Web、`desk:` → Desktop，050 的规则一个字节没动）。
#[test]
fn a_declared_reversibility_is_taken_as_is_and_location_comes_from_the_prefix() {
    let table = ToolTable::builtin().with_host_tools(vec![
        host("web:crm/lookup", Reversibility::Pure),
        host("web:crm/draft", Reversibility::Reversible),
        host("desk:mail/send", Reversibility::Irreversible),
    ]);

    assert_eq!(
        snap(&table, "web:crm/lookup").reversibility,
        Reversibility::Pure
    );
    assert_eq!(snap(&table, "web:crm/lookup").location, Location::Web);
    assert_eq!(
        snap(&table, "web:crm/draft").reversibility,
        Reversibility::Reversible
    );
    assert_eq!(
        snap(&table, "desk:mail/send").reversibility,
        Reversibility::Irreversible
    );
    assert_eq!(snap(&table, "desk:mail/send").location, Location::Desktop);
}

/// 注入映射**先于名字规则**——这是 062 之前那个 `starts_with("mcp:")` 前缀门会咬人的
/// 地方：`web:crm/lookup` 不在 `reversibility_of` 的已知表里，落进 `_ =>
/// Irreversible`；宿主声明了 `pure` 就必须是 `Pure`。同一张表里另一个**没注入**的
/// `web:` 名字仍然走保守兜底，证明第一级不是「凡 web: 都算」。
#[test]
fn the_injection_map_wins_over_the_name_rules() {
    let table =
        ToolTable::builtin().with_host_tools(vec![host("web:crm/lookup", Reversibility::Pure)]);
    assert_eq!(
        snap(&table, "web:crm/lookup").reversibility,
        Reversibility::Pure
    );

    let not_injected = snap(&table, "web:crm/anything-else");
    assert_eq!(
        not_injected.reversibility,
        Reversibility::Irreversible,
        "没注入的名字仍旧保守兜底"
    );
    assert_eq!(not_injected.location, Location::Web);
}

/// 注入映射**先于 MCP 映射**——但这只在两张映射里的名字**不撞**的时候才有的谈。
/// 同一个名字同时进两张表在 HTTP 那一层不可能发生（`mcp:` 前缀会被 `capabilities`
/// 校验当场拒掉，061），这里直接调库 API 造出没撞名的那半状态，钉住优先级本身。
///
/// **075 之前**这里还有一段用同名撞过的构造，断言「注入映射赢」——那其实是撞名场景
/// 下的一个巧合产物：`specs` 两条都留、`host_reversibility.insert` 后来居上，
/// `snapshot` 先查 host 表所以看着像「注入赢」。075 把 `with_*` 的 `push` 收进
/// `push_spec` 之后，那个场景不再产生「两条都留、可逆性各显各的」这种状态——
/// 撞名从**跨 with_* 边界**就被 `push_spec` 拦下，见下面
/// `a_name_that_collides_across_with_mcp_and_with_host_tools_keeps_the_first_one_registered`。
#[test]
fn the_injection_map_wins_over_the_mcp_map() {
    // 没被注入盖住的那个 MCP 工具照旧查 MCP 映射（第二级还在）。
    let table = ToolTable::builtin()
        .with_mcp(vec![(spec("mcp:everything/echo"), Reversibility::Pure)])
        .with_host_tools(vec![host("web:crm/lookup", Reversibility::Irreversible)]);
    assert_eq!(
        snap(&table, "mcp:everything/echo").reversibility,
        Reversibility::Pure
    );
    assert_eq!(
        snap(&table, "web:crm/lookup").reversibility,
        Reversibility::Irreversible
    );
}

// ── 075：push_spec 判重跨 with_mcp / with_host_tools 边界依然生效 ───────────

/// `with_host_tools`：重复声明同一个工具名（两次的可逆性故意写得不一样）→
/// `specs()` 长度不变、可逆性仍是**先来的**那份。跟 `with_mcp` 那条同一个道理
/// （`tool_table_tests.rs` 的 `with_mcp_loading_the_same_name_twice_keeps_the_first_reversibility`），
/// 这里换成 `with_host_tools` 的入口——它收的是客户端请求体，069 §拍板 D 明确否决
/// 让它硬失败。
#[test]
fn with_host_tools_loading_the_same_name_twice_keeps_the_first_reversibility() {
    let build = || {
        ToolTable::builtin().with_host_tools(vec![
            host("web:dup/tool", Reversibility::Pure),
            host("web:dup/tool", Reversibility::Irreversible),
        ])
    };
    let result = std::panic::catch_unwind(build);
    if cfg!(debug_assertions) {
        assert!(
            result.is_err(),
            "debug 构建下重复声明应该在 with_host_tools 内部 debug_assert 炸掉"
        );
    } else {
        let table = result.expect("release 构建下 with_host_tools 不该 panic");
        assert_eq!(
            table
                .specs()
                .iter()
                .filter(|s| &*s.name == "web:dup/tool")
                .count(),
            1,
            "撞名的那条不该真的多进一条 spec"
        );
        assert_eq!(
            snap(&table, "web:dup/tool").reversibility,
            Reversibility::Pure,
            "可逆性该是先来的那份，不是后来的 Irreversible"
        );
    }
}

/// `debug_assert!` 点得出名字：`with_host_tools` 撞名同样在 debug 构建下 panic，
/// 消息里含撞的那个工具名。
#[test]
#[should_panic(expected = "web:dup/tool")]
fn with_host_tools_names_the_offender_in_a_debug_build() {
    let _ = ToolTable::builtin().with_host_tools(vec![
        host("web:dup/tool", Reversibility::Pure),
        host("web:dup/tool", Reversibility::Irreversible),
    ]);
}

/// 跨 `with_*` 边界的撞名（`with_mcp` 先声明、`with_host_tools` 后来撞上同一个
/// 名字）也被 `push_spec` 统一拦下——**先来的那条（`with_mcp`）整条保留**，后来的
/// `with_host_tools` 那条整条丢弃。HTTP 那一层今天已经靠 061 的 `web:`/`desk:`
/// 前缀强制让这个具体名字（`mcp:` 前缀）结构上进不了 `with_host_tools`，这里直接
/// 调库 API 造出它，钉住 `push_spec` 不只在同一个 `with_*` 内部生效。
#[test]
fn a_name_that_collides_across_with_mcp_and_with_host_tools_keeps_the_first_one_registered() {
    let build = || {
        ToolTable::builtin()
            .with_mcp(vec![(spec("mcp:everything/echo"), Reversibility::Pure)])
            .with_host_tools(vec![host(
                "mcp:everything/echo",
                Reversibility::Irreversible,
            )])
    };
    let result = std::panic::catch_unwind(build);
    if cfg!(debug_assertions) {
        assert!(
            result.is_err(),
            "debug 构建下跨 with_* 边界撞名也该 debug_assert"
        );
    } else {
        let table = result.expect("release 构建下不该 panic");
        assert_eq!(
            table
                .specs()
                .iter()
                .filter(|s| &*s.name == "mcp:everything/echo")
                .count(),
            1
        );
        assert_eq!(
            snap(&table, "mcp:everything/echo").reversibility,
            Reversibility::Pure,
            "先来的是 with_mcp 那条，可逆性该是它的 Pure，不是后来 with_host_tools 声明的 Irreversible"
        );
    }
}

/// 空注入 = 什么都没发生：表、可逆性判定跟没调过这个方法逐项相同（不带
/// `capabilities` 的老会话走的正是这条路）。
#[test]
fn injecting_nothing_changes_nothing() {
    let untouched = ToolTable::with_shell();
    let empty = ToolTable::with_shell().with_host_tools(Vec::new());
    assert_eq!(names(&empty), names(&untouched));
    assert_eq!(
        snap(&empty, "srv:fs/read").reversibility,
        snap(&untouched, "srv:fs/read").reversibility
    );
    assert_eq!(
        snap(&empty, "srv:shell/exec").reversibility,
        Reversibility::Irreversible
    );
}
