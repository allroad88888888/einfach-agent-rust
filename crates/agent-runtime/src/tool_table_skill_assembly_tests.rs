//! [`super`] 的单测：**139 本身——`with_skills` 的新装配形状**。
//!
//! specs 区有 read 没有 activate/deactivate、timed 区恰好一条 index、二次调用的
//! 撞名判据是这两个固定名字，以及**141 之后的老会话兼容**：journal 里有一条
//! M5 期的 activate entry，恢复不 panic、`active_skills()` 原样读回，但**没有
//! 任何生产代码再拿它去注入下一轮请求体**——那条曾经把激活集展开成注入料的
//! 方法已经随 141 删掉。

use std::sync::Arc;

use agent_core::{AgentId, AgentValue, AtomKey, HostSkill, Session, SkillId, Slot};

use crate::skill::SKILL_READ;

use super::*;

/// activate 工具的全名——141 删了它对应的常量本身（连同声明/截获一起没了），
/// 这里只是把它当一个**普通字符串**用来断言「表不认识这个名字」，不依赖已删
/// 的常量。
const ACTIVATE_TOOL_NAME: &str = "srv:skill/activate";
const DEACTIVATE_TOOL_NAME: &str = "srv:skill/deactivate";

/// index 的全名。**不从 `crate::skill` 拿一个专门的常量**：`SKILL_INDEX` 只在
/// `skill/index.rs` 内部被 `index_spec()` 用到，生产代码从不需要单独引用这个
/// 名字（`with_skills` 直接调 `index_spec()`，不比较名字）；专门为测试再开一条
/// `pub use` 会在非 test 构建下触发它自己的 unused-import 警告。`index_spec()`
/// 已经因为 `with_skills` 用它而是「活」的，从这里取名字不多背一条警告。
fn skill_index_name() -> Arc<str> {
    index_spec().name
}

/// 一份非空、内容无所谓的 registry——凡是要看「registry 非空时 with_skills 往
/// 表里放什么」的测试，都从这个开始（跟空 registry 的判据是两件事，139 起
/// `with_skills` 对空 registry 是彻底的无操作，见 [`an_empty_registry_leaves_specs_untouched`]）。
fn a_registry() -> SkillRegistry {
    SkillRegistry::from_host_skills(vec![HostSkill {
        id: SkillId::new("crm-flow"),
        description: Arc::from("处理客户工单"),
        body: Arc::from("body"),
        tools: Vec::new(),
        tool_reversibility: Default::default(),
    }])
}

/// 064 判据由 `with_skills` 自己守住（139，改之前只是模块文档里的一句劝告，
/// `agent-cli` 的装配链其实一直无条件调它）：空 registry 之后 `specs()` 必须跟
/// 压根没调过 `with_skills` 时逐字节相同，`timed` 区也不该多一条恒回空文本的
/// index——不能因为宿主没查 `is_empty()` 就调用，就给一个没开任何 skill 的会话
/// 平白加东西。
#[test]
fn an_empty_registry_leaves_specs_untouched() {
    let bare = ToolTable::builtin();
    let with_empty_skills = ToolTable::builtin().with_skills(SkillRegistry::empty());

    assert_eq!(
        serde_json::to_string(bare.specs()).unwrap(),
        serde_json::to_string(with_empty_skills.specs()).unwrap(),
        "空 registry 的 with_skills 之后，specs() 的字节必须与不接 with_skills 时逐字节相同"
    );
    assert_eq!(
        with_empty_skills.timed(CallTiming::SessionStart).count(),
        0,
        "空 registry 也不该往 timed 区塞一条恒回空文本的 index"
    );
}

/// 139 §验收第一条：registry 非空时，新会话 specs 有 read、没有
/// activate/deactivate/index（index 在 timed 区，`declares()`/`specs()` 一个
/// 字节看不见它——跟 133 的「timed 工具独立区」判据同一句话）。
#[test]
fn a_new_sessions_specs_have_read_but_not_activate_deactivate_or_index() {
    let table = ToolTable::builtin().with_skills(a_registry());

    assert!(table.declares(SKILL_READ), "read 该进 specs（139）");
    assert!(
        !table.declares(ACTIVATE_TOOL_NAME),
        "activate 不该再进 specs——139 切装配、141 删了机制本身，with_skills 从来\
         没有、也不会再注册这个名字"
    );
    assert!(!table.declares(DEACTIVATE_TOOL_NAME), "deactivate 同理");
    assert!(
        !table.declares(&skill_index_name()),
        "index 不进 specs——它在 timed 区，declares() 只认 specs 那张表"
    );

    let timed: Vec<_> = table.timed(CallTiming::SessionStart).collect();
    assert_eq!(
        timed.len(),
        1,
        "SessionStart 时机区该恰好一条：index，实际 {}",
        timed.len()
    );
    assert_eq!(timed[0].spec().name, skill_index_name());
}

/// index 的执行体读的是**这张表自己**的 registry（`with_timed` 的执行体只拿
/// `&ToolTable`，不捕获闭包外的 `registry` 副本，见 `tool_table_skill.rs` 里
/// `with_skills` 的文档）——不是空文本，也不是某个全局默认值。
#[test]
fn the_index_timed_tool_reads_this_tables_own_registry() {
    let table = ToolTable::builtin().with_skills(a_registry());

    let index_tool = table
        .timed(CallTiming::SessionStart)
        .next()
        .expect("index 该在 timed 区");
    let text = index_tool
        .run(&table, &serde_json::Value::Null)
        .expect("index 不该失败");

    assert!(
        text.contains("crm-flow"),
        "index 该读到这张表自己的 registry，不是一份空副本或别的表：{text}"
    );
}

/// 075 的判据换到新名字：`with_skills` 固定追加 `srv:skill/read`（specs 区）+
/// `srv:skill/index`（timed 区），两次调用（比如宿主装配代码手滑调重了）会撞在
/// 这两个固定名字上——跟 `with_mcp`/`with_host_tools` 走的是同一个
/// `push_spec`/`with_timed`，同一套「整条丢弃 + debug_assert」。**两次都得传非空
/// registry**：空 registry 是彻底的无操作（上面那条测试），不会撞出任何名字。
#[test]
fn with_skills_called_twice_does_not_duplicate_read_or_index() {
    let build = || ToolTable::builtin().with_skills(a_registry()).with_skills(a_registry());
    let result = std::panic::catch_unwind(build);
    if cfg!(debug_assertions) {
        assert!(
            result.is_err(),
            "debug 构建下二次调用 with_skills 应该 debug_assert 炸掉"
        );
    } else {
        let table = result.expect("release 构建下不该 panic");
        assert_eq!(
            table.specs().iter().filter(|s| &*s.name == SKILL_READ).count(),
            1,
            "read 不该被重复调用多留一条"
        );
        assert_eq!(
            table
                .timed(CallTiming::SessionStart)
                .filter(|t| t.spec().name == skill_index_name())
                .count(),
            1,
            "index 同理（timed 区）"
        );
    }
}

/// `debug_assert!` 点得出名字：`with_skills` 内部先 push read 再注册 index，
/// 二次调用因此先撞在 `srv:skill/read` 上，debug 构建下的 panic 消息里含它。
#[test]
#[should_panic(expected = "srv:skill/read")]
fn with_skills_names_the_offender_in_a_debug_build() {
    let _ = ToolTable::builtin()
        .with_skills(a_registry())
        .with_skills(a_registry());
}

/// **141 §验收「老会话兼容」**：一份带 `Slot::SkillsActive` 数据的老快照（模拟
/// M5 期真被激活过、journal 里本来会有一条 `activate_skill` entry 的会话——写入
/// 命令本身已经删了，所以这里跟 `host_skills_indep_restore.rs` 同一手法，直接在
/// 快照里手搭这个槽位的值）恢复不 panic、`active_skills()` 原样读回；但**这张表
/// 已经没有任何方法能把它变回注入料**——那个曾经把激活集展开成注入料的方法随
/// 141 一起删了，装出来的表就算装了一个同 id 的 registry，也没有别的口子能让它的正文
/// 进 `Ingredients`。这是「状态在、没人读」的类型层证据：不是运行时断言"body
/// 里没有正文"（那条更完整的端到端证据在 `tests/it/` 的老数据兼容测试里，走
/// `provider_call::start` 的真实请求体），是「根本没有能读它的方法可调」。
#[test]
fn a_restored_session_with_a_journaled_activation_no_longer_has_any_injection_path() {
    let root = AgentId::root();

    // 手搭一份「M5 期已激活 crm-flow」的快照——不经 `activate_skill`（已删），
    // 直接构造 `Slot::SkillsActive` 该有的编码（`value::str_set`：排序去重的
    // 字符串数组）。
    let snapshot = vec![(
        AtomKey::Agent(root.clone(), Slot::SkillsActive),
        AgentValue::Json(Arc::new(serde_json::json!(["crm-flow"]))),
    )];
    let mut unknown = Vec::new();
    let restored = Session::restore(root.clone(), Some(snapshot), Vec::new(), 0, 0, 100, &mut |k| {
        unknown.push(k.clone())
    })
    .expect("含 SkillsActive 数据的快照必须能被今天的代码重放，不 panic、不拒绝");
    assert!(
        unknown.is_empty(),
        "SkillsActive 是留壳的既有槽位，不该报进 on_unknown_key：{unknown:?}"
    );

    assert!(
        restored.active_skills().contains(&SkillId::new("crm-flow")),
        "恢复出来的激活集必须原样带回这个 id（状态还在）：{:?}",
        restored.active_skills()
    );

    // 这个进程今天装的表（139 之后：只有 read + timed index）——用一份内容匹配
    // 的 registry 装配，模拟「新进程重新从磁盘/声明装载同一个 skill」。
    let registry = SkillRegistry::from_host_skills(vec![HostSkill {
        id: SkillId::new("crm-flow"),
        description: Arc::from("处理客户工单"),
        body: Arc::from("这是 crm-flow 的正文，141 之后只能靠 srv:skill/read 取到。"),
        tools: Vec::new(),
        tool_reversibility: Default::default(),
    }]);
    let table = ToolTable::builtin().with_skills(registry);

    assert!(
        !table.declares(ACTIVATE_TOOL_NAME),
        "这张新表不认识 activate 这个名字（139/141）"
    );
    // `ToolTable` 今天只剩 `skill_registry()`（read/index 用它查正文/索引文本）——
    // 没有第二个方法能把 `restored.active_skills()` 变成一份 system/tools 注入。
    // 这不是运行时断言，是类型层的事实：把这个「已经激活」的集合变成注入料的
    // 那条方法根本不在 `ToolTable` 的公开/内部 API 里了。
    let _ = table.skill_registry();
}
