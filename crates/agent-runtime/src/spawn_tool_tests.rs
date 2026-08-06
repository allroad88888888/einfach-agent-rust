//! `srv:agent/spawn` 执行边界的单元面：子集校验与拒绝文案。
//!
//! 请求 schema 与 parser 的测试归 [`crate::spawn_request`]，不与执行边界共用模块。
//! 「一次真的 spawn 长出一棵子树」那一面在 e2e（`tests/spawn_indep_*`），
//! 「后台 spawn 不挡父」在 `tests/spawn_bg_*`。

use super::*;

fn names(list: &[&str]) -> Vec<Arc<str>> {
    list.iter().map(|n| Arc::from(*n)).collect()
}

/// 提权被显式拒绝，且拒绝文本里点名缺的是哪一个。
#[test]
fn a_child_cannot_be_given_a_tool_the_parent_lacks() {
    let err = check_subset(&names(&["srv:shell/exec"]), &names(&["srv:fs/read"])).unwrap_err();
    assert!(err.contains("srv:shell/exec"), "{err}");
    assert!(err.contains("srv:fs/read"), "{err}");
    let ok = check_subset(
        &names(&["srv:fs/read"]),
        &names(&["srv:fs/read", "srv:fs/list"]),
    );
    assert_eq!(ok.unwrap(), names(&["srv:fs/read"]));
}

/// 050：模型照抄它在函数列表里看到的（转义过的）名字，一样要被认下来，并且
/// **归一化成规范名**再往下走——`ChildConfig` 里存 wire 名 = 子 agent 一个工具
/// 都没有（`subagent::tools_for` 精确过滤）。
#[test]
fn a_wire_escaped_tool_name_is_accepted_and_normalized() {
    let parent = names(&["srv:fs/read", "srv:fs/list", "mcp:everything/echo"]);
    let got = check_subset(
        &names(&["srv_3Afs_2Flist", "mcp_3Aeverything_2Fecho"]),
        &parent,
    );
    assert_eq!(got.unwrap(), names(&["srv:fs/list", "mcp:everything/echo"]));
}

/// 归一化**只往已有的名字上映**：瞎编的工具名照旧被拒，转义拼法也一样，
/// 而且拒绝文本里回显的是模型自己写的那个字符串（它要认出自己写错了什么）。
#[test]
fn escaping_does_not_let_an_invented_tool_name_through() {
    let err = check_subset(&names(&["srv_3Ashell_2Fexec"]), &names(&["srv:fs/read"])).unwrap_err();
    assert!(err.contains("srv_3Ashell_2Fexec"), "{err}");
}

/// 两条闸的文案都得带上当时的数字——只说「超限了」模型不知道该收敛到几。
#[test]
fn refusal_text_carries_the_numbers() {
    let text = refusal_text(&SpawnRefused::TooManyChildren { live: 8, max: 8 });
    assert!(text.contains('8'), "{text}");
    let text = refusal_text(&SpawnRefused::DepthExceeded { depth: 4, max: 3 });
    assert!(text.contains('4') && text.contains('3'), "{text}");
}
