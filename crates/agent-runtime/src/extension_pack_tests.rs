//! 148 的**名字规则**单测（`#[path]` 子模块，`super` 是 `extension_pack` 本身——
//! `ExtensionPack`/`Arc`/`ToolSpec` 都经它的 `use` 语句透传进来，模式同
//! `tool_table_timed_tests.rs`）。
//!
//! 「包进了表之后会怎样」不在这里：那是装配的事，全部在
//! `tool_table_extension_tests.rs`（含每条被拒条目在 release 下的真实下场）。

use agent_core::{AgentId, Session};
use serde_json::{Value, json};

use super::*;

const PACK: &str = "demo";

fn spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from("测试用扩展工具"),
        schema: Arc::new(json!({ "type": "object" })),
    }
}

fn nop_tool() -> SessionToolFn {
    Box::new(|_session: &mut Session, _agent: &AgentId, _input: &Value| {
        Ok((Arc::from("ok"), crate::Aftermath::Nothing))
    })
}

fn nop_timed() -> TimedRun {
    Box::new(|_table, _session: &Session, _input: &Value| Ok(Arc::from("ok")))
}

/// 判据表：一个名字属不属于这个包。四种被拒的形状各一行——裸名、冒用别人的位置
/// 前缀（`srv:`/`web:`/`desk:`/`mcp:`）、别的包的命名空间、以及尾巴为空。
///
/// `ext:demo2/x` 那一行是这张表存在的理由：包名比较必须**带上那个 `/`**，
/// 「`ext:` + 包名开头」这个天真判据会把 `demo2` 的工具认成 `demo` 的。
#[test]
fn a_name_belongs_to_a_pack_only_when_it_is_exactly_ext_pack_slash_something() {
    assert!(belongs_to(PACK, "ext:demo/tree_echo"));
    assert!(belongs_to(PACK, "ext:demo/a/b"), "尾巴内部长什么样是包作者的事");

    assert!(!belongs_to(PACK, "tree_echo"), "裸名");
    assert!(!belongs_to(PACK, "srv:demo/tree_echo"), "冒用 srv:");
    assert!(!belongs_to(PACK, "web:demo/tree_echo"), "冒用 web:");
    assert!(!belongs_to(PACK, "desk:demo/tree_echo"), "冒用 desk:");
    assert!(!belongs_to(PACK, "mcp:demo/tree_echo"), "冒用 mcp:");
    assert!(!belongs_to(PACK, "ext:other/tree_echo"), "别的包的命名空间");
    assert!(!belongs_to(PACK, "ext:demo2/tree_echo"), "包名要整段匹配到 `/`");
    assert!(!belongs_to(PACK, "ext:demo/"), "尾巴不能为空");
    assert!(!belongs_to(PACK, "ext:demo"), "少了 `/`");
    assert!(!belongs_to(PACK, "EXT:demo/tree_echo"), "前缀区分大小写");
}

/// 裸名截获工具：debug 构建当场炸，文案点名是哪个包的哪个名字。
#[test]
#[should_panic(expected = "`tree_echo` 不叫 `ext:demo/<tool>`")]
fn with_tool_rejects_a_bare_name() {
    let _ = ExtensionPack::new(PACK).with_tool(spec("tree_echo"), nop_tool());
}

/// 冒用 `srv:` 比裸名更该拦：它会让 `location_of` 把一个扩展工具判成内置服务端
/// 工具，dispatch 于是拿它去查一个根本没有实现的名字。
#[test]
#[should_panic(expected = "`srv:demo/shell` 不叫 `ext:demo/<tool>`")]
fn with_tool_rejects_a_borrowed_srv_prefix() {
    let _ = ExtensionPack::new(PACK).with_tool(spec("srv:demo/shell"), nop_tool());
}

/// 冒用 `web:`：`location_of` 判成远端 → dispatch 去等一个永远不会来的宿主回传。
#[test]
#[should_panic(expected = "`web:demo/pick` 不叫 `ext:demo/<tool>`")]
fn with_tool_rejects_a_borrowed_web_prefix() {
    let _ = ExtensionPack::new(PACK).with_tool(spec("web:demo/pick"), nop_tool());
}

/// 别的包的命名空间：包名是这道闸的主语，不是摆设。
#[test]
#[should_panic(expected = "`ext:other/tree_echo` 不叫 `ext:demo/<tool>`")]
fn with_tool_rejects_another_packs_namespace() {
    let _ = ExtensionPack::new(PACK).with_tool(spec("ext:other/tree_echo"), nop_tool());
}

/// timed 条目吃同一条前缀强制：它虽然不进 prompt，却跟 specs 区共用同一个名字
/// 空间（`with_timed` 的撞名双向查），一条裸名钩子照样能占掉一个内置工具名。
#[test]
#[should_panic(expected = "timed 工具 `turn_end_ping` 不叫 `ext:demo/<tool>`")]
fn with_timed_rejects_a_bare_name_too() {
    let _ = ExtensionPack::new(PACK).with_timed(
        spec("turn_end_ping"),
        CallTiming::TurnEnd,
        nop_timed(),
    );
}

/// 包名自己也有形状：含 `/` 会让 `ext:a/b` 同时能被包 `a/b` 和包 `a` 声称，
/// 命名空间那份红利当场作废。
#[test]
#[should_panic(expected = "扩展包名 `a/b` 不合法")]
fn new_rejects_a_pack_name_that_would_blur_the_namespace() {
    let _ = ExtensionPack::new("a/b");
}

/// 包名进宿主的授权面和日志，所以问得到。
#[test]
fn a_pack_knows_its_own_name() {
    assert_eq!(ExtensionPack::new(PACK).name(), PACK);
}
