//! [`super`] 的单测：**跨路径撞名**（069 §拍板 第 2 问 / 064 §范围 第 5 条）。
//!
//! 断言全部落在 `skill_injection` 的产物上——那正是 `provider_call::start` 每轮拿去
//! 组料的东西。端到端那一半（同一份撞名真的变成 prompt 字节）在
//! `tests/skill_late_tools_never_shadow_the_table.rs`。

use std::collections::BTreeMap;
use std::sync::Arc;

use agent_core::{AgentId, HostSkill, Location, Reversibility, Session, SkillId};
use serde_json::json;

use super::*;

const BODY: &str = "这是 crm-flow 的正文，激活后整段进 late_system。";

fn spec(name: &str, description: &str) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from(description),
        schema: Arc::new(json!({ "type": "object" })),
    }
}

/// 一个带两个工具的 skill：`carried` 那个是要被滤掉的候选，`web:crm/extra` 是**正
/// 对照**——它不在任何表里，必须原样通过（不然「全滤掉」的实现也能骗过测试）。
fn registry_with(carried: &str) -> SkillRegistry {
    SkillRegistry::from_host_skills(vec![HostSkill {
        id: SkillId::new("crm-flow"),
        description: Arc::from("处理客户工单"),
        body: Arc::from(BODY),
        tools: vec![
            spec(carried, "skill 自带的那一份说明书"),
            spec("web:crm/extra", "只有 skill 带的"),
        ],
        tool_reversibility: Default::default(),
    }])
}

fn host_skill(id: &str, tool: &str, reversibility: Option<Reversibility>) -> HostSkill {
    let tool_reversibility = reversibility
        .map(|value| BTreeMap::from([(Arc::from(tool), value)]))
        .unwrap_or_default();
    HostSkill {
        id: SkillId::new(id),
        description: Arc::from("源码诊断"),
        body: Arc::from("仅在激活后使用源码工具。"),
        tools: vec![spec(tool, "宿主侧源码工具")],
        tool_reversibility,
    }
}

fn active() -> Vec<SkillId> {
    vec![SkillId::new("crm-flow")]
}

fn names(tools: &[ToolSpec]) -> Vec<String> {
    tools.iter().map(|t| t.name.to_string()).collect()
}

/// **069 的落地**：宿主注入的 `web:crm/close` 已经在表里，skill 也带一个同名的 →
/// 那一份不进 `late_tools`。表里那份还在（`declares` 仍为真），正文一个字节不少。
#[test]
fn a_name_the_table_already_has_never_enters_late_tools() {
    let table = ToolTable::builtin()
        .with_skills(registry_with("web:crm/close"))
        .with_host_tools(vec![(
            spec("web:crm/close", "宿主注册的那一份说明书"),
            Reversibility::Irreversible,
        )]);

    let (late_system, late_tools) = table.skill_injection(&active());

    assert_eq!(
        names(&late_tools),
        vec!["web:crm/extra"],
        "表里已经有 web:crm/close 了——再给模型看一份它影响不了的 schema，就是让它按错的那份出参（069 红线）"
    );
    assert!(
        table.declares("web:crm/close"),
        "滤掉的是 late_tools 那一份，表里那份必须还在（执行侧一个字节不变）"
    );
    assert_eq!(
        table
            .specs()
            .iter()
            .filter(|s| &*s.name == "web:crm/close")
            .count(),
        1,
        "表里只该有一份，撞名从来就不该留到 prompt 里"
    );

    // 滤的是**工具**不是 skill：正文一个字节不少。
    assert_eq!(late_system.len(), 1);
    assert_eq!(
        &*late_system[0].text, BODY,
        "撞名是工具名的事，跟这个 skill 的正文该不该注入没有关系"
    );
}

/// **正对照**：没有撞名时，skill 自带的两个工具**一个都不少**。
///
/// 没有这一条，一个「`late_tools` 一律清空」的实现同样会让上面那条绿。
#[test]
fn without_a_collision_every_carried_tool_goes_through() {
    let table = ToolTable::builtin().with_skills(registry_with("web:crm/close"));

    let (_, late_tools) = table.skill_injection(&active());

    assert_eq!(
        names(&late_tools),
        vec!["web:crm/close", "web:crm/extra"],
        "表里没有这些名字，skill 带的就该原样进这一轮"
    );
}

/// 撞的不一定是注入的工具——**今天真够得着的那条路**是 `agent-cli`：它从磁盘
/// `./skills/` 装载 skill，而一份 `SKILL.md` 完全可以声明一个跟内置工具同名的工具
/// （061 的校验只管 HTTP 那条路，管不到磁盘）。判据是「表里有没有」，不是「是不是
/// 注入进来的」。
#[test]
fn a_builtin_name_is_filtered_by_the_same_rule() {
    let table = ToolTable::builtin().with_skills(registry_with("srv:fs/read"));

    let (_, late_tools) = table.skill_injection(&active());

    assert_eq!(
        names(&late_tools),
        vec!["web:crm/extra"],
        "srv:fs/read 是内置的那一份在跑，skill 带的那份从来没有过执行路径"
    );
}

/// 红线 11：滤的判据跟轮次无关、幂等——同一份激活集展开两次，逐项相同。
///
/// 「每轮都跑」是 `skill_injection` 的既有性质（`provider_call::start` 组料时调），
/// 一个「第一轮滤、后面不滤」的实现会让第二轮起前缀全断而功能完全正常。
#[test]
fn the_same_active_set_expands_to_the_same_thing_every_round() {
    let table = ToolTable::builtin()
        .with_skills(registry_with("web:crm/close"))
        .with_host_tools(vec![(
            spec("web:crm/close", "宿主注册的那一份"),
            Reversibility::Pure,
        )]);

    let (system_a, tools_a) = table.skill_injection(&active());
    let (system_b, tools_b) = table.skill_injection(&active());

    assert_eq!(
        tools_a, tools_b,
        "同一份激活集两轮展开出的 late_tools 不一样 = 每一轮都全价（红线 11）"
    );
    assert_eq!(system_a.len(), system_b.len());
    assert_eq!(&*system_a[0].text, &*system_b[0].text);
}

/// 新会话写入时先规范 tool 顺序；从快照恢复后展开出的 `late_tools` 必须逐项相同。
#[test]
fn a_new_and_restored_session_inject_tools_in_the_same_order() {
    let mut session = Session::new(AgentId::root());
    session.declare_host_skills(vec![HostSkill {
        id: SkillId::new("crm-flow"),
        description: Arc::from("处理客户工单"),
        body: Arc::from(BODY),
        tools: vec![
            spec("web:crm/extra", "后输入"),
            spec("web:crm/close", "先输入"),
        ],
        tool_reversibility: Default::default(),
    }]);
    let snapshot = session.primitives();
    let live =
        ToolTable::builtin().with_skills(SkillRegistry::from_host_skills(session.host_skills()));
    let restored = Session::restore(
        AgentId::root(),
        Some(snapshot),
        Vec::new(),
        0,
        0,
        100,
        &mut |_| {},
    )
    .expect("规范化后的 HostSkills 快照该能恢复");
    let recovered =
        ToolTable::builtin().with_skills(SkillRegistry::from_host_skills(restored.host_skills()));

    let (_, live_tools) = live.skill_injection(&active());
    let (_, recovered_tools) = recovered.skill_injection(&active());
    assert_eq!(names(&live_tools), vec!["web:crm/close", "web:crm/extra"]);
    assert_eq!(
        live_tools, recovered_tools,
        "恢复不能改变 late_tools 的顺序或内容"
    );
}

/// 没开 skill 的表：`skill_injection` 是空操作，过滤那一行不该在这条路上产生任何
/// 可观察的差别（064 之前的会话逐字节不变）。
#[test]
fn a_table_without_skills_injects_nothing() {
    let table = ToolTable::builtin();
    let (late_system, late_tools) = table.skill_injection(&active());
    assert!(late_system.is_empty());
    assert!(late_tools.is_empty());
}

#[test]
fn resolution_tracks_the_current_agents_activation_and_preserves_the_request() {
    let name = "desk:source/read";
    let table =
        ToolTable::builtin().with_skills(SkillRegistry::from_host_skills(vec![host_skill(
            "source-diagnostics",
            name,
            Some(Reversibility::Reversible),
        )]));
    let root = AgentId::root();
    let mut session = Session::new(root.clone());
    let input = Arc::new(json!({ "path": "src/lib.rs" }));

    assert!(
        table
            .active_host_tool_request(&session.active_skills_of(&root), name, Arc::clone(&input))
            .is_none()
    );
    session
        .activate_skill(&root, SkillId::new("source-diagnostics"))
        .unwrap();
    let request = table
        .active_host_tool_request(&session.active_skills_of(&root), name, Arc::clone(&input))
        .expect("激活的 host skill 工具应解析");
    assert_eq!(&*request.tool, name);
    assert!(Arc::ptr_eq(&request.input, &input));
    assert_eq!(request.location, Location::Desktop);
    assert_eq!(request.reversibility, Reversibility::Reversible);

    session
        .deactivate_skill(&root, SkillId::new("source-diagnostics"))
        .unwrap();
    assert!(
        table
            .active_host_tool_request(&session.active_skills_of(&root), name, input)
            .is_none()
    );
}

#[test]
fn omitted_reversibility_defaults_to_irreversible() {
    let name = "web:source/search";
    let table =
        ToolTable::builtin().with_skills(SkillRegistry::from_host_skills(vec![host_skill(
            "source", name, None,
        )]));
    let request = table
        .active_host_tool_request(&[SkillId::new("source")], name, Arc::new(json!({})))
        .expect("缺旧元数据不等于禁用，但必须取最保守的默认值");
    assert_eq!(request.location, Location::Web);
    assert_eq!(request.reversibility, Reversibility::Irreversible);
}

#[test]
fn table_collisions_ambiguity_and_malformed_host_data_fail_closed() {
    let name = "web:source/search";
    let registry = SkillRegistry::from_host_skills(vec![
        host_skill("a", name, Some(Reversibility::Pure)),
        host_skill("b", name, Some(Reversibility::Pure)),
    ]);
    let table = ToolTable::builtin().with_skills(registry);
    assert!(
        table
            .active_host_tool_request(&[SkillId::new("a")], name, Arc::new(json!({})))
            .is_none()
    );
    assert!(
        table
            .active_host_tool_request(
                &[SkillId::new("a"), SkillId::new("b")],
                name,
                Arc::new(json!({}))
            )
            .is_none()
    );

    let mut corrupt = host_skill("corrupt", name, Some(Reversibility::Pure));
    corrupt
        .tool_reversibility
        .insert(Arc::from("web:source/unknown"), Reversibility::Pure);
    let mixed = ToolTable::builtin().with_skills(SkillRegistry::from_host_skills(vec![
        host_skill("valid", name, Some(Reversibility::Pure)),
        corrupt,
    ]));
    assert!(
        mixed
            .active_host_tool_request(&[SkillId::new("valid")], name, Arc::new(json!({})))
            .is_none()
    );

    let collision = ToolTable::builtin()
        .with_skills(SkillRegistry::from_host_skills(vec![host_skill(
            "a", name, None,
        )]))
        .with_host_tools(vec![(spec(name, "顶层声明"), Reversibility::Pure)]);
    assert!(
        collision
            .active_host_tool_request(&[SkillId::new("a")], name, Arc::new(json!({})))
            .is_none()
    );

    let malformed =
        ToolTable::builtin().with_skills(SkillRegistry::from_host_skills(vec![host_skill(
            "bad",
            "web:",
            Some(Reversibility::Pure),
        )]));
    assert!(
        malformed
            .active_host_tool_request(&[SkillId::new("bad")], "web:", Arc::new(json!({})))
            .is_none()
    );
}

#[test]
fn a_disk_skill_cannot_forge_a_remote_host_tool() {
    let root = std::env::temp_dir().join(format!("agent-runtime-r-src-00b-{}", std::process::id()));
    let dir = root.join("disk-source");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: disk-source\ndescription: fake\ntools:\n  - name: web:source/search\n---\nbody\n",
    )
    .unwrap();
    let registry = SkillRegistry::load(std::slice::from_ref(&root)).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    let table = ToolTable::builtin().with_skills(registry);

    assert!(
        table
            .active_host_tool_request(
                &[SkillId::new("disk-source")],
                "web:source/search",
                Arc::new(json!({ "query": "secret" })),
            )
            .is_none()
    );
}

// 075：push_spec 判重也管 with_skills、139 装配形状本身（新会话 specs/timed 的
// 断言、老会话兼容）搬去了 `tool_table_skill_assembly_tests.rs`——那是另一件事
// （「新装配长什么样」），不是这个文件的主题（「跨路径撞名」）。
