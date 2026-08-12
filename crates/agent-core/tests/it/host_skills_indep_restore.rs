//! 064 独立测试（agent-core 层）：宿主注入的 **skill 声明**在恢复路径上原模原样回来。
//!
//! 形状照 073 的 `host_tools_indep_restore.rs`（同一条用户拍板：「历史对话就该跟历史
//! 一致，原模原样 100% 复刻」），但要钉的风险多一格——**skill 这一路在 store 里已经
//! 有半份状态了**：
//!
//! `Slot::SkillsActive`（039）记着「哪些 skill 被激活」，正文一直是从运行时 registry
//! 现取的。宿主注入的 skill 在 store 外**没有第二份**（它只在那一次 HTTP 请求里存在
//! 过），所以声明不落盘，恢复出来就是一份**指向空 registry 的激活集**：状态说
//! `crm-flow` 激活着、展开注入却什么都取不到（`injection` 对查不到的 id 静默跳过），
//! 而模型的历史里明明写着它读过那段正文。这份测试的最后一条就钉这个。

use std::collections::BTreeMap;
use std::sync::Arc;

use agent_core::{
    AgentId, AgentValue, AtomKey, HostSkill, Reversibility, Session, SkillId, Slot, ToolSpec,
};

fn root() -> AgentId {
    AgentId::root()
}

/// 两个 skill，**故意不按字典序给**；一个带工具、一个不带。
fn declaration() -> Vec<HostSkill> {
    vec![
        HostSkill {
            id: SkillId::new("mail-flow"),
            description: Arc::from("发信的标准流程"),
            body: Arc::from("先草拟再发送。"),
            tools: Vec::new(),
            tool_reversibility: BTreeMap::new(),
        },
        HostSkill {
            id: SkillId::new("crm-flow"),
            description: Arc::from("处理客户工单的标准流程"),
            body: Arc::from("先查档案再关单。"),
            tools: vec![ToolSpec {
                name: Arc::from("web:crm/close"),
                description: Arc::from("关掉一个工单"),
                schema: Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": { "ticket": { "type": "string" }, "force": { "type": "boolean" } },
                    "required": ["ticket"]
                })),
            }],
            tool_reversibility: BTreeMap::from([(Arc::from("web:crm/close"), Reversibility::Pure)]),
        },
    ]
}

/// 「声明过 `declaration()`」的会话里 `Slot::HostSkills` 该有的值——现查一个真实写入
/// 过的 `Session`，不猜它落进哪个 `AgentValue` 变体。
fn host_skills_value() -> AgentValue {
    let mut session = Session::new(root());
    session.declare_host_skills(declaration());
    session
        .primitives()
        .into_iter()
        .find(|(k, _)| *k == AtomKey::Agent(root(), Slot::HostSkills))
        .map(|(_, v)| v)
        .expect("Slot::HostSkills 是一个 source 槽位，build_agent 建图时就该带默认值")
}

fn restore_with(snapshot: Vec<(AtomKey, AgentValue)>) -> Session {
    let mut unknown = Vec::new();
    let session = Session::restore(root(), Some(snapshot), Vec::new(), 0, 0, 100, &mut |k| {
        unknown.push(k.clone())
    })
    .expect("合法快照该能恢复");
    assert!(
        unknown.is_empty(),
        "HostSkills 是这一版认识的槽位，不该报进 on_unknown_key：{unknown:?}"
    );
    session
}

/// 直接注入快照：五个字段一个不差（含工具 schema 与旁挂执行元数据）。
#[test]
fn a_snapshot_with_host_skills_restores_every_field_of_every_declaration() {
    let session = restore_with(vec![(
        AtomKey::Agent(root(), Slot::HostSkills),
        host_skills_value(),
    )]);
    let restored = session.host_skills();

    assert_eq!(restored.len(), 2);
    // 写入时按 id 排过序（红线 11），读回就是有序的。
    assert_eq!(restored[0].id.as_str(), "crm-flow");
    assert_eq!(restored[1].id.as_str(), "mail-flow");

    assert_eq!(
        &*restored[0].description, "处理客户工单的标准流程",
        "描述丢了 = 恢复出来的索引行变一行空的"
    );
    assert_eq!(
        &*restored[0].body, "先查档案再关单。",
        "正文丢了 = 激活之后模型什么都读不到，且不报错"
    );
    assert_eq!(restored[0].tools.len(), 1);
    assert_eq!(&*restored[0].tools[0].name, "web:crm/close");
    assert_eq!(
        restored[0].tools[0].schema["properties"]["ticket"]["type"],
        serde_json::json!("string")
    );
    assert_eq!(
        restored[0].tool_reversibility["web:crm/close"],
        Reversibility::Pure
    );
    assert!(
        restored[1].tools.is_empty(),
        "不带工具的 skill 恢复出来也不该凭空长出工具"
    );
}

/// 064 写出的旧快照没有执行元数据字段；升级后仍须读回声明，而不是整项跳过。
#[test]
fn a_legacy_snapshot_without_tool_reversibility_still_restores() {
    let legacy = AgentValue::Json(Arc::new(serde_json::json!([{
        "id": "crm-flow",
        "description": "处理客户工单",
        "body": "先查档案。",
        "tools": [{
            "name": "web:crm/lookup",
            "description": "查 CRM",
            "schema": { "type": "object" }
        }]
    }])));

    let session = restore_with(vec![(AtomKey::Agent(root(), Slot::HostSkills), legacy)]);
    let restored = session.host_skills();
    assert_eq!(restored.len(), 1, "旧形状不能因为缺新字段而被整项跳过");
    assert_eq!(&*restored[0].tools[0].name, "web:crm/lookup");
    assert!(
        restored[0].tool_reversibility.is_empty(),
        "缺字段用空映射表示；执行侧按名字缺省为 irreversible"
    );
}

/// 红线 3 + 红线 11：整份快照 serde 往返，而且**两次序列化逐字节相同**。
///
/// 它会红的那一天是有人给某个依赖打开了 `serde_json` 的 `preserve_order`
/// （`Map` 从 `BTreeMap` 换成 `IndexMap`，key 序跟着插入顺序走）——那时恢复出来的
/// 会话第一轮就 system 段前缀全断，功能一切正常，只是每一轮都全价。
#[test]
fn the_declaration_survives_a_serde_roundtrip_byte_for_byte() {
    let snapshot = vec![(
        AtomKey::Agent(root(), Slot::HostSkills),
        host_skills_value(),
    )];

    let once = serde_json::to_string(&snapshot).expect("快照该可序列化");
    let back: Vec<(AtomKey, AgentValue)> = serde_json::from_str(&once).expect("快照该可反序列化");
    let twice = serde_json::to_string(&back).expect("往返之后仍该可序列化");
    assert_eq!(once, twice, "同一份声明两次序列化必须逐字节相同（红线 11）");

    assert_eq!(
        restore_with(back).host_skills(),
        declaration_sorted(),
        "往返之后每个字段都该一模一样"
    );
}

/// 日志游标停在声明**之前** → 恢复出来就没有这些 skill（undo 那一条在核心层的落点）。
///
/// **带正对照**：只断言「游标在前面时没有」是自欺欺人——一个「从来没恢复过任何
/// skill」的实现同样会绿。
#[test]
fn a_log_whose_cursor_sits_before_the_declaration_restores_without_it() {
    let mut source = Session::new(root());
    source.declare_host_skills(declaration());
    let log: Vec<_> = source.history().entries().cloned().collect();
    assert_eq!(log.len(), 1, "声明该正好留下一条 entry");
    assert_eq!(log[0].meta.label, "declare_host_skills");
    let len = log.len() as u64;

    // 正对照：游标在声明之后 → 两个 skill 都在。
    let after = Session::restore(
        root(),
        None,
        log.clone(),
        source.cursor(),
        len,
        100,
        &mut |_| {},
    )
    .expect("恢复该成功");
    assert_eq!(
        after.host_skills().len(),
        2,
        "游标在声明之后，两个 skill 都该回来"
    );

    // 游标在声明之前 → 一个都没有。
    let before = Session::restore(root(), None, log, 0, len, 100, &mut |_| {}).expect("恢复该成功");
    assert!(
        before.host_skills().is_empty(),
        "游标停在声明之前，这个会话不该认得任何注入的 skill"
    );
}

/// **skill 这一路特有的那格**：激活集（`SkillsActive`）和声明（`HostSkills`）必须
/// 一起回来。分开落盘/漏掉一半，就是一份指向空 registry 的悬空引用。
///
/// 141 删了 `Session::activate_skill`（决策 27：没有生产代码再写这个槽位），
/// 这条测试因此**不再走那条命令**——直接在快照里手搭 `Slot::SkillsActive` 的值，
/// 模拟一个 M5 期真被激活过、现在恢复回来的老会话（跟已删的
/// `skill_indep_restore.rs` 当年的手法一致：不猜具体是哪个 `AgentValue` 变体，
/// 但今天已知它是 `value::str_set` 的编码，即排序去重的字符串数组）。
#[test]
fn the_active_set_and_the_declaration_come_back_together() {
    let mut source = Session::new(root());
    source.declare_host_skills(declaration());
    let snapshot: Vec<(AtomKey, AgentValue)> = source
        .primitives()
        .into_iter()
        .map(|(key, value)| {
            if key == AtomKey::Agent(root(), Slot::SkillsActive) {
                (
                    key,
                    AgentValue::Json(Arc::new(serde_json::json!(["crm-flow"]))),
                )
            } else {
                (key, value)
            }
        })
        .collect();

    let session = restore_with(snapshot);
    assert_eq!(
        session.active_skills(),
        vec![SkillId::new("crm-flow")],
        "激活集该原样回来（留壳的只读口）"
    );
    assert!(
        session
            .host_skills()
            .iter()
            .any(|s| s.id.as_str() == "crm-flow"),
        "激活集里那个 id 必须在声明里查得到——查不到就是悬空引用：状态说它激活着、正文却取不到，而模型的历史里写着它读过"
    );
}

fn declaration_sorted() -> Vec<HostSkill> {
    let mut sorted = declaration();
    sorted.sort_by(|a, b| a.id.0.cmp(&b.id.0));
    sorted
}
