//! 139 独立测试（装配形状半）：只依据 `docs/issues/139-skill-assembly-switch.md`
//! 「验收」「注意」两节 + `docs/INVARIANTS.md` 红线 11 + 公开 API
//! `agent_runtime::{ToolTable, CallTiming, SkillRegistry, run_session_start}`
//! 写成，**不看** `crates/agent-runtime/src/tool_table_skill.rs` / `skill/` 目录
//! 里的实现体。实现由另一个 agent 并行写，本文件与它互不通信；切换尚未落地时
//! 编译/断言红是预期结果。
//!
//! 本文件管**静态装配形状**——`with_skills` 之后 `declares`/`specs`/`timed` 长
//! 什么样，以及 `run_session_start` 产出的前缀块内容。线级（真经过假 provider
//! 一跳）的行为在 `skill_switch_wire_indep.rs`（同一职责边界：那边管字节怎么上
//! 线，这边管表本身的形状）。
//!
//! # 一处已知偏离：不测「hidden skill 不进索引」
//!
//! 139 自己的「验收」节没有提 hidden；`docs/issues/138-skill-index-tool.md` 明文
//! 说「树形（142）会在这上面加 hidden 过滤——本条先列全量，别预留半实现的参数」；
//! `agent_core::HostSkill`（`crates/agent-core/src/value/host_skills.rs`）也确实
//! 没有 `hidden` 字段，142 的磁盘 frontmatter 支持同样是待做（142 依赖 138，不在
//! 139 的依赖链 133+135+137+138 里）。所以这里没有能构造出「隐藏 skill」的公开
//! 入口——写一个断言它被滤掉的用例，测的会是一个今天不可能满足的契约，不是本条
//! 切换的范围。`skill_index_indep.rs`（138 的独立测试）同样明确排除了这个用例，
//! 理由一致。

use std::sync::Arc;

use agent_core::{AgentId, HostSkill, Session, SkillId, SystemChunk, ToolSpec};
use agent_runtime::{run_session_start, CallTiming, SkillRegistry, ToolTable};

fn skill(id: &str, description: &str, body: &str) -> HostSkill {
    HostSkill {
        id: SkillId::new(id),
        description: Arc::from(description),
        body: Arc::from(body),
        tools: Vec::new(),
        tool_reversibility: Default::default(),
    }
}

fn non_empty_registry() -> SkillRegistry {
    SkillRegistry::from_host_skills(vec![skill(
        "switch-flow",
        "装配切换测试用的流程",
        "这是 switch-flow 的正文，索引里不该出现它的任何字节。BODY_SWITCH_ZX90",
    )])
}

/// 验收「新会话 specs：无 activate/deactivate、有 read」的前半：`with_skills`
/// 之后 `declares("srv:skill/read")` 为真，且它的 spec 真的在 `specs()` 里
/// （不是只有 `declares` 返回真，`specs()` 里却找不到对应条目）。
#[test]
fn with_skills_declares_read_and_its_spec_is_in_specs() {
    let table = ToolTable::builtin().with_skills(non_empty_registry());

    assert!(
        table.declares("srv:skill/read"),
        "非空 registry 接上之后，declares(\"srv:skill/read\") 必须为真"
    );
    assert!(
        table.specs().iter().any(|s| &*s.name == "srv:skill/read"),
        "read 的 spec 必须真的在 specs() 里，不能只是 declares() 单方面为真: {:?}",
        table.specs()
    );
}

/// 验收「新会话 specs：无 activate/deactivate」的后半：`declares` 为假，且
/// `specs()` 里连名字都找不到——不是「declares 假但 spec 还残留在表里」。
#[test]
fn with_skills_no_longer_declares_activate_or_deactivate() {
    let table = ToolTable::builtin().with_skills(non_empty_registry());

    for name in ["srv:skill/activate", "srv:skill/deactivate"] {
        assert!(
            !table.declares(name),
            "切换之后 {name} 不该再被 declares()"
        );
        assert!(
            !table.specs().iter().any(|s| &*s.name == name),
            "切换之后 {name} 不该出现在 specs() 里: {:?}",
            table.specs()
        );
    }
}

/// 验收「无 srv:skill/index（在 timed 区）」：`specs()` 里没有它，`timed(SessionStart)`
/// 里恰有一条、spec 名就是 `srv:skill/index`——它是开局驱动跑的时机工具，不是模型
/// 自主调的普通工具。
#[test]
fn srv_skill_index_is_timed_only_never_in_specs() {
    let table = ToolTable::builtin().with_skills(non_empty_registry());

    assert!(
        !table.declares("srv:skill/index"),
        "srv:skill/index 不该是一个模型可自主调的普通工具"
    );
    assert!(
        !table.specs().iter().any(|s| &*s.name == "srv:skill/index"),
        "srv:skill/index 不该出现在喂模型的 specs 里: {:?}",
        table.specs()
    );

    let timed: Vec<&ToolSpec> = table
        .timed(CallTiming::SessionStart)
        .map(|t| t.spec())
        .collect();
    let index_entries: Vec<&&ToolSpec> = timed.iter().filter(|s| &*s.name == "srv:skill/index").collect();
    assert_eq!(
        index_entries.len(),
        1,
        "timed(SessionStart) 里该恰有一条 srv:skill/index，实际: {timed:?}"
    );
}

/// 验收「空 registry 的 with_skills：specs 与 timed 区都与不接 with_skills 时逐
/// 字节相同」——空 registry 必须是一次彻底的 no-op 装配，不是「declares 为假但
/// 悄悄留了点别的东西」。
#[test]
fn with_skills_of_an_empty_registry_is_a_byte_identical_no_op() {
    let base = ToolTable::builtin();
    let with_empty = ToolTable::builtin().with_skills(SkillRegistry::empty());

    let base_specs_bytes = serde_json::to_vec(base.specs()).expect("specs() 必须能序列化");
    let with_empty_specs_bytes =
        serde_json::to_vec(with_empty.specs()).expect("specs() 必须能序列化");
    assert_eq!(
        base_specs_bytes, with_empty_specs_bytes,
        "空 registry 的 with_skills 之后，specs() 的字节必须与不接 with_skills 时逐字节相同"
    );

    let base_timed: Vec<ToolSpec> = base
        .timed(CallTiming::SessionStart)
        .map(|t| t.spec().clone())
        .collect();
    let with_empty_timed: Vec<ToolSpec> = with_empty
        .timed(CallTiming::SessionStart)
        .map(|t| t.spec().clone())
        .collect();
    assert_eq!(
        serde_json::to_vec(&base_timed).unwrap(),
        serde_json::to_vec(&with_empty_timed).unwrap(),
        "空 registry 的 with_skills 之后，timed(SessionStart) 区必须与不接 with_skills 时逐字节相同"
    );
    assert!(
        !with_empty.declares("srv:skill/read"),
        "空 registry 不该产出 read 工具"
    );
}

/// 验收「首轮 encode body：system 含索引块（label init:srv:skill/index）」在
/// `run_session_start` 这一层的落点：`session.prefix_chunks()` 里恰有一块，label
/// 是 `init:srv:skill/index`，正文含每个已装载 skill 的 id 与 description、
/// 按 id 字典序排列、不含任何 skill 的正文字节。
///
/// 三个 skill 故意按非字典序注册（zulu/alpha/mango），用来把「顺序」这条断言
/// 立成会红的——一个只按注册序拼接、不按 id 排序的实现会让这条断言失败。
#[test]
fn run_session_start_produces_one_ordered_index_chunk_with_no_body_bytes() {
    const BODY_ONE: &str = "BODY_SENTINEL_ZULU_ZX90";
    const BODY_TWO: &str = "BODY_SENTINEL_ALPHA_ZX90";
    const BODY_THREE: &str = "BODY_SENTINEL_MANGO_ZX90";

    let registry = SkillRegistry::from_host_skills(vec![
        skill("zulu-index", "最后一个索引流程", BODY_ONE),
        skill("alpha-index", "第一个索引流程", BODY_TWO),
        skill("mango-index", "第二个索引流程", BODY_THREE),
    ]);
    let table = ToolTable::builtin().with_skills(registry);

    let mut session = Session::new(AgentId::root());
    run_session_start(&mut session, &table).expect("非空 registry 的索引工具不该失败");

    let chunks: Vec<SystemChunk> = session.prefix_chunks();
    assert_eq!(
        chunks.len(),
        1,
        "builtin() 没有别的 SessionStart 工具，该恰好一块: {chunks:?}"
    );
    assert_eq!(
        &*chunks[0].label, "init:srv:skill/index",
        "前缀块的 label 必须是 init:srv:skill/index"
    );

    let text = &*chunks[0].text;
    for needle in [
        "alpha-index",
        "第一个索引流程",
        "mango-index",
        "第二个索引流程",
        "zulu-index",
        "最后一个索引流程",
    ] {
        assert!(text.contains(needle), "索引块该含 {needle}: {text}");
    }
    for body in [BODY_ONE, BODY_TWO, BODY_THREE] {
        assert!(!text.contains(body), "索引块不该含任何正文字节 {body}: {text}");
    }

    let pos_alpha = text.find("alpha-index").unwrap();
    let pos_mango = text.find("mango-index").unwrap();
    let pos_zulu = text.find("zulu-index").unwrap();
    assert!(
        pos_alpha < pos_mango && pos_mango < pos_zulu,
        "索引块正文该按 id 字典序排列（alpha < mango < zulu），实际顺序错位: {text}"
    );
}
