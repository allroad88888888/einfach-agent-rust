//! `srv:shell/exec` 的静态声明（issue 020 钉死规格）：`shell_spec()` 不进
//! `builtin_specs()`——这是「默认关着」的证据，模型看不到这个工具，除非宿主显式
//! 把它加进某张工具表。同时钉死名字/位置/可逆性三件事，以及红线 11 的序列化
//! 稳定性。
//!
//! **`agent_core::ToolSpec` 本身只有 `name`/`description`/`schema` 三个字段**
//! （喂给模型的那三样）——`Location`/`Reversibility` 是 router/undo 用的正交
//! 维度，`agent_tools::shell_spec()` 的文档原话是「这个等级不是 `ToolSpec` 的
//! 字段，是调用方工具表要标注的元数据」（见 `lib.rs`）。所以本文件的
//! 「location/reversibility 断言」只能落在**名字**上：`srv:` 前缀是 location
//! 的唯一编码（`docs/TOOLS.md` §「命名空间」），未被特例列为 `Pure` 的名字保守
//! 落 `Irreversible`（`docs/TOOLS.md` §「reversibility 等级怎么定」：拿不准就
//! `Irreversible`）——这正是 shell「拿得准」的地方：它落在既有的保守默认值上，
//! 不需要额外声明。下面两个分类函数是那条既有命名约定的最小复刻，不是新发明
//! 的规则，也不依赖 `agent-runtime`（本 crate 的下游，不能反向依赖）。

use agent_core::{Location, Reversibility, ToolSpec};
use agent_tools::{builtin_specs, shell_spec};

/// `docs/TOOLS.md`「命名空间」：`<location-prefix>:<namespace>/<tool>`。
fn location_of_name(name: &str) -> Location {
    match name.split_once(':').map(|(prefix, _)| prefix) {
        Some("web") => Location::Web,
        Some("desk") => Location::Desktop,
        _ => Location::Server,
    }
}

/// 拿不准就 `Irreversible`；已知的纯读白名单之外一律保守默认。
fn reversibility_of_name(name: &str) -> Reversibility {
    match name {
        "srv:fs/read" | "srv:fs/list" => Reversibility::Pure,
        _ => Reversibility::Irreversible,
    }
}

fn spec() -> ToolSpec {
    shell_spec()
}

#[test]
fn shell_spec_name_is_pinned() {
    assert_eq!(&*spec().name, "srv:shell/exec");
}

#[test]
fn shell_spec_name_encodes_server_location_via_srv_prefix() {
    assert_eq!(location_of_name(&spec().name), Location::Server);
}

#[test]
fn shell_spec_name_is_not_on_the_pure_allowlist_so_it_is_irreversible() {
    assert_eq!(
        reversibility_of_name(&spec().name),
        Reversibility::Irreversible
    );
}

#[test]
fn builtin_specs_does_not_include_shell_by_default() {
    let specs = builtin_specs();
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_ref()).collect();
    assert!(
        !names.contains(&"srv:shell/exec"),
        "shell/exec 必须默认关着，不能出现在喂给模型的内置工具表里，实际：{names:?}"
    );
}

#[test]
fn shell_spec_serializes_byte_identical_across_calls() {
    // 红线 11 最小实检，套用 `srv:fs/read`/`srv:fs/list` 已有的手法
    // （`tool_table_stability.rs`），换成单独暴露的 `shell_spec()`。
    let bytes_a = serde_json::to_vec(&spec()).unwrap();
    let bytes_b = serde_json::to_vec(&spec()).unwrap();
    assert_eq!(bytes_a, bytes_b);
}

#[test]
fn shell_spec_wrapped_in_vec_serializes_byte_identical_across_calls() {
    // 真正进 prompt 的形态是 `Vec<ToolSpec>`（红线 11 的措辞就是这样写的），
    // 不只测单个 ToolSpec。
    let bytes_a = serde_json::to_vec(&vec![spec()]).unwrap();
    let bytes_b = serde_json::to_vec(&vec![spec()]).unwrap();
    assert_eq!(bytes_a, bytes_b);
}
