//! [`super`] 的单测：**139 本身——`with_skills` 的新装配形状**。
//!
//! 跟 `tool_table_skill_tests.rs`（跨路径撞名，069/064）是两件事：那份钉的是
//! 「表里已经有的名字怎么滤」，这份钉的是「`with_skills` 现在往表里放什么」——
//! specs 区有 read 没有 activate/deactivate、timed 区恰好一条 index、二次调用的
//! 撞名判据换成了新的两个名字、以及老会话（journal 里有 M5 期的 activate entry）
//! 恢复之后 `skill_injection` 照旧工作（141 之前的兼容态，139 §验收）。

use agent_core::{AgentId, HostSkill, Session, SkillId};
use serde_json::json;

use crate::skill::{SKILL_ACTIVATE, SKILL_DEACTIVATE, SKILL_READ};

use super::*;

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
        !table.declares(SKILL_ACTIVATE),
        "activate 不该再进 specs——139 只切装配，机制还在，但 with_skills 不再注册它"
    );
    assert!(!table.declares(SKILL_DEACTIVATE), "deactivate 同理");
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

/// 139 §验收「老会话兼容」：journal 里有一条 M5 期的 activate entry（`with_skills`
/// 改装配之前产出的那种）恢复不 panic，`skill_injection` 照旧按
/// `Slot::SkillsActive` 展开——**跟表还认不认识 `srv:skill/activate` 这个名字
/// 无关**：`skill_injection` 只读激活集 + registry，不查 `declares()`。141 删
/// 激活子系统之前，这是必须护住的兼容态。
#[test]
fn a_restored_session_with_a_journaled_activation_still_gets_its_skill_injected() {
    let root = AgentId::root();

    // 一个「老」会话：真实走 command 层激活一个 skill，产出跟 M5 期一模一样形状
    // 的 journal entry（`activate_skill` 本身没有变过，只有 `with_skills` 变了）。
    let mut original = Session::new(root.clone());
    original
        .activate_skill(&root, SkillId::new("crm-flow"))
        .expect("激活一个从没激活过的 skill 不该被拒");

    let entries: Vec<_> = original.history().entries().cloned().collect();
    let cursor = original.cursor();
    let next_seq = entries.len() as u64;

    // 恢复：新进程按今天的日志格式重放这份老历史（`Session::restore` 这条独立
    // 路径不依赖 `ToolTable` 或 registry，纯 core 状态回放）。
    let restored = Session::restore(root.clone(), None, entries, cursor, next_seq, 100, &mut |k| {
        panic!("Slot::SkillsActive 是既有槽位，不该报进 on_unknown_key：{k:?}")
    })
    .expect("含一条 activate entry 的日志必须能被今天的代码重放，不 panic、不拒绝");

    assert!(
        restored.active_skills().contains(&SkillId::new("crm-flow")),
        "恢复出来的激活集必须原样带回这个 id，实际：{:?}",
        restored.active_skills()
    );

    // 这个进程今天装的表（139 之后：只有 read + timed index，没有 activate/
    // deactivate）——用一份内容匹配的 registry 装配，模拟「新进程重新从磁盘/
    // 声明装载同一个 skill」。
    let body = "这是 crm-flow 的正文，激活后整段进 late_system。";
    let registry = SkillRegistry::from_host_skills(vec![HostSkill {
        id: SkillId::new("crm-flow"),
        description: Arc::from("处理客户工单"),
        body: Arc::from(body),
        tools: vec![ToolSpec {
            name: Arc::from("web:crm/close"),
            description: Arc::from("关闭工单"),
            schema: Arc::new(json!({ "type": "object" })),
        }],
        tool_reversibility: Default::default(),
    }]);
    let table = ToolTable::builtin().with_skills(registry);

    assert!(
        !table.declares(SKILL_ACTIVATE),
        "这张新表不认识 activate 这个名字了（139）——下面这条断言要证的正是\
         「即便如此，已经激活过的 skill 照样被注入」"
    );

    let active = restored.active_skills_of(&root);
    let (late_system, late_tools) = table.skill_injection(&active);

    assert_eq!(late_system.len(), 1, "老会话恢复出来的激活集该注入这一个 skill 的正文");
    assert_eq!(&*late_system[0].text, body);
    assert_eq!(
        late_tools.iter().map(|t| &*t.name).collect::<Vec<_>>(),
        vec!["web:crm/close"],
        "它带的工具也该照旧展开"
    );
}
