//! 069 的看门狗：**进 prompt 的那张工具表里，一个名字只能出现一次。**
//!
//! `ToolTable::with_*` 一路 `push`，不检测不去重（069 §拍板 D）——今天五档 + CLI
//! 那条链**实际没有撞名**，这个文件就是把「实际没有」钉成会红的断言。069 之前
//! 全仓没有任何测试断言过重名行为，于是「哪天某一档多加一个工具刚好撞上」这件事
//! 没有任何东西拦得住：它不 panic、不告警，只是模型看到两个同名工具，调到哪一个
//! 取决于 provider 怎么处理重复声明。
//!
//! 第二条断言看住的是另一半：**没有任何内置档的工具用 `web:`/`desk:` 前缀**。
//! 这正是「061 只在一份声明内部判唯一」够用的**结构性理由**——宿主注入的工具被
//! 校验强制成 `web:`/`desk:` 开头，只要内置档一个都不用这两个前缀，注入的名字就
//! **不可能**撞上内置的名字，跨路径查重那一格就压根不存在。哪天有人给某一档加一个
//! `web:` 前缀的内置工具，这条会先红——那时要连 061 的校验一起重新算账
//! （docs/TOOLS.md §撞名）。

use std::collections::BTreeMap;

use agent_core::AgentLimits;
use agent_runtime::{SkillRegistry, ToolTable};

/// 一张表里所有重复出现的名字（按名字排序，红线 11 精神：断言输出也要确定）。
fn duplicates(table: &ToolTable) -> Vec<String> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for spec in table.specs() {
        *counts.entry(&spec.name).or_default() += 1;
    }
    counts
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(name, _)| name.to_string())
        .collect()
}

/// 五档 + CLI 那条链，逐张表点名。`with_skills`/`with_mcp`/`with_host_tools` 收的
/// 是运行时数据（磁盘 skill、第三方 `tools/list`、客户端请求体），这里喂空的——
/// 这条测试看的是**装配链本身**有没有内置的撞名，运行时那半归各自的上游裁判点。
fn all_tables() -> Vec<(&'static str, ToolTable)> {
    vec![
        ("builtin", ToolTable::builtin()),
        ("with_shell", ToolTable::with_shell()),
        ("standard_local", ToolTable::standard_local()),
        ("standard", ToolTable::standard()),
        (
            "Full（server 第五档）",
            ToolTable::with_shell()
                .with_spawn(AgentLimits::default())
                .with_status()
                .with_collect(),
        ),
        (
            "agent-cli 的链",
            ToolTable::standard_local()
                .with_spawn(AgentLimits::default())
                .with_status()
                .with_collect()
                .with_skills(SkillRegistry::empty())
                .with_mcp(Vec::new()),
        ),
    ]
}

#[test]
fn no_assembled_table_declares_the_same_name_twice() {
    for (label, table) in all_tables() {
        let dup = duplicates(&table);
        assert!(
            dup.is_empty(),
            "{label} 这张表里有重名工具 {dup:?}——两条同名 spec 都会进 prompt，\
             而 declares()/snapshot()/五条截获闸全部按名字查，第二份永远调不到。\
             见 docs/issues/069-name-collision-policy.md §拍板"
        );
    }
}

/// 内置档一个都不许用 `web:`/`desk:` 前缀——注入的工具被 061 强制用这两个前缀，
/// 这条不变量就是「注入的名字不可能撞上内置的名字」的全部依据。
///
/// `standard` 档那三个跑在前端的工具（`ask_user_question`/`browser_action`/
/// `save_file`）是**裸名**，靠 `location_of` 的硬编码白名单捞成 `Location::Web`
/// （docs/TOOLS.md §命名空间）——它们不带前缀，所以也不构成例外。
#[test]
fn no_builtin_tier_squats_on_the_host_injection_prefixes() {
    for (label, table) in all_tables() {
        for spec in table.specs() {
            assert!(
                !spec.name.starts_with("web:") && !spec.name.starts_with("desk:"),
                "{label} 里的 {} 占了宿主注入专用的前缀——061 的「一份声明内部判唯一」\
                 从这一刻起不够用了，必须补跨路径查重（docs/TOOLS.md §撞名）",
                spec.name
            );
        }
    }
}
