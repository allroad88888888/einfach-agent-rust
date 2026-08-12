//! 073 独立测试（agent-core 层）：宿主注入的声明在**恢复路径**上原模原样回来。
//!
//! `docs/HOST-CAPABILITIES.md` §三 原文——注入的声明**是会话状态，不是部署配置**：
//! 建会话时 journaled 地写进去，恢复时跟别的 primitive 一样从日志回放自动回来，
//! 宿主不必也不该在重连时再声明一遍。这份测试只钉 agent-core 这一层的契约：
//! `Session::restore` 灌回快照/日志之后，`host_tools()` 把**四个字段一个不差**地
//! 带回来（名字、描述、schema、可逆性）。
//!
//! 三条区别于 `skill_indep_restore.rs` 的地方，正是本 issue 特有的风险：
//!
//! 1. **存的是声明本身，不是一个 id**：skill 的正文在 registry 里现取，注入的工具
//!    在 store 外**没有第二份**——落丢一个字段就永远丢了；
//! 2. **`schema` 是一段自由 JSON**（红线 3 的「值必须全部可序列化」在这里才真的
//!    被考验到），它逐字节稳定是**假设不得**的事，所以有一条专门的断言；
//! 3. **这份值会进 prompt 最前面**（红线 11），所以断言比的是**字节**，不是
//!    「集合相等」。

use std::sync::Arc;

use agent_core::{AgentId, AgentValue, AtomKey, Reversibility, Session, Slot, ToolSpec};

fn root() -> AgentId {
    AgentId::root()
}

/// 一份「像真的」的声明：一个标了 `pure`、schema 里有嵌套对象；一个保守的
/// `Irreversible`、无参 schema。**故意不按字典序给**。
fn declaration() -> Vec<(ToolSpec, Reversibility)> {
    vec![
        (
            ToolSpec {
                name: Arc::from("web:crm/lookup"),
                description: Arc::from("按客户 ID 查 CRM 档案"),
                schema: Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": { "id": { "type": "string" }, "all": { "type": "boolean" } },
                    "required": ["id"]
                })),
            },
            Reversibility::Pure,
        ),
        (
            ToolSpec {
                name: Arc::from("desk:clipboard/write"),
                description: Arc::from("写系统剪贴板"),
                schema: Arc::new(serde_json::json!({ "type": "object" })),
            },
            Reversibility::Irreversible,
        ),
    ]
}

/// 造一个「声明过 `declaration()`」的会话，`Slot::HostTools` 该有的值——现查一个
/// 真实写入过的 `Session`，不猜它落进哪个 `AgentValue` 变体。
fn host_tools_value() -> AgentValue {
    let mut session = Session::new(root());
    session.declare_host_tools(declaration());
    session
        .primitives()
        .into_iter()
        .find(|(k, _)| *k == AtomKey::Agent(root(), Slot::HostTools))
        .map(|(_, v)| v)
        .expect("Slot::HostTools 是一个 source 槽位，build_agent 建图时就该带默认值")
}

/// 直接注入快照：`Session::restore` 认得这个值形状，四个字段一个不差。
#[test]
fn a_snapshot_with_host_tools_restores_every_field_of_every_declaration() {
    let snapshot = vec![(AtomKey::Agent(root(), Slot::HostTools), host_tools_value())];
    let mut unknown = Vec::new();
    let session = Session::restore(
        root(),
        Some(snapshot),
        Vec::new(),
        0,
        0,
        100,
        agent_core::AgentLimits::default(),
        &mut |k| unknown.push(k.clone()),
    )
    .expect("合法快照该能恢复");

    assert!(
        unknown.is_empty(),
        "HostTools 是这一版认识的槽位，不该报进 on_unknown_key"
    );
    let restored = session.host_tools();
    assert_eq!(restored.len(), 2);

    // 名字：写入时按名字排过序（红线 11），读回就是有序的。
    assert_eq!(&*restored[0].0.name, "desk:clipboard/write");
    assert_eq!(&*restored[1].0.name, "web:crm/lookup");
    // 描述与 schema：它们**进 prompt**，掉一个字就是改 prompt 字节。
    assert_eq!(&*restored[1].0.description, "按客户 ID 查 CRM 档案");
    assert_eq!(*restored[1].0.schema, *declaration()[0].0.schema);
    // 可逆性：它**不进 prompt**，但 `/undo` 撞上去停不停全看它——落丢就等于把
    // 宿主声明的 `pure` 悄悄按 `Irreversible` 办（或者反过来，更糟）。
    assert_eq!(restored[0].1, Reversibility::Irreversible);
    assert_eq!(restored[1].1, Reversibility::Pure);
}

/// **红线 3 + 红线 11 的合并断言**：整份快照 serde 往返之后，`HostTools` 那一项
/// 逐字节相同——`schema` 那个自由 `serde_json::Value` 是这条的重点（对象后端是
/// `BTreeMap`，根 `Cargo.toml` 显式不开 `preserve_order`）。
///
/// **这条不能只靠「反正 serde 能跑」**：它会红的那一天，是有人给某个依赖打开了
/// `preserve_order`（特性统一会传染到整个 workspace），那时工具表的字节会随插入
/// 顺序漂，功能完全正常、只是每一轮都全价。
#[test]
fn the_declaration_survives_a_serde_roundtrip_byte_for_byte() {
    let mut session = Session::new(root());
    session.declare_host_tools(declaration());
    let primitives = session.primitives();

    let text = serde_json::to_string(&primitives).expect("红线 3：primitive 的值必须全部可序列化");
    let back: Vec<(AtomKey, AgentValue)> =
        serde_json::from_str(&text).expect("自己写出来的快照必须读得回来");
    assert_eq!(
        serde_json::to_string(&back).unwrap(),
        text,
        "快照两次序列化必须逐字节相同"
    );

    // 顺带钉住「声明真的在快照里」——上面那句对一份空快照也成立。
    let key = AtomKey::Agent(root(), Slot::HostTools);
    let value = back
        .into_iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
        .expect("快照里该有 HostTools");
    assert_eq!(
        value,
        host_tools_value(),
        "往返回来的值必须跟写进去的那份相等"
    );
    assert!(
        serde_json::to_string(&value)
            .unwrap()
            .contains("按客户 ID 查 CRM 档案"),
        "描述得真的在落盘字节里，不是靠某个运行时 registry 现取：{value:?}"
    );
}

/// 快照里根本没有 `HostTools` 这个键（073 之前建的老会话文件）——走
/// `Slot::default_value()` 兜底：**空**，不是恐慌，也不是把某一份部署期的清单
/// 当默认值塞进来。老会话就该继续没有注入的工具。
#[test]
fn an_old_session_without_the_key_falls_back_to_no_injection() {
    let session = Session::restore(
        root(),
        None,
        Vec::new(),
        0,
        0,
        100,
        agent_core::AgentLimits::default(),
        &mut |_| {},
    )
    .expect("全新会话，没有任何快照/日志，该能正常建出来");
    assert!(
        session.host_tools().is_empty(),
        "默认值必须是「没有任何注入」"
    );
}

/// 全链路往返：真实写一次声明 → 取整段日志 → 喂进一个全新 `Session::restore` →
/// `host_tools()` 跟原会话一致。手搭快照证的是「`restore` 认得这个值形状」，
/// 这条证的是「声明确实**落进了日志**」——两件不同的事，缺一条就有洞。
#[test]
fn restoring_a_real_sessions_log_reproduces_its_declaration() {
    let mut original = Session::new(root());
    original.declare_host_tools(declaration());

    let entries: Vec<_> = original.history().entries().cloned().collect();
    assert_eq!(entries.len(), 1, "声明是一条 journaled entry：{entries:?}");
    assert_eq!(entries[0].meta.label, "declare_host_tools");

    let cursor = original.cursor();
    let restored = Session::restore(
        root(),
        None,
        entries.clone(),
        cursor,
        entries.len() as u64,
        100,
        agent_core::AgentLimits::default(),
        &mut |_| {},
    )
    .expect("原会话产出的日志必须能被自己重放");

    let (before, after) = (original.host_tools(), restored.host_tools());
    assert_eq!(after.len(), before.len());
    for (a, b) in after.iter().zip(before.iter()) {
        assert_eq!(a.0, b.0, "ToolSpec 三个字段逐个相等（它们是 prompt 字节）");
        assert_eq!(a.1, b.1);
    }
}

/// 游标退到声明**之前**（undo 掉那一步）之后再恢复 → 这个会话没有注入的工具。
/// 「undo 一致性」在 agent-core 这一层的落点：它是白拿的，因为走的是同一条
/// journaled 路——但白拿也要有断言，不然「白拿」只是一句话。
#[test]
fn a_log_whose_cursor_sits_before_the_declaration_restores_without_it() {
    let mut original = Session::new(root());
    original.declare_host_tools(declaration());
    let entries: Vec<_> = original.history().entries().cloned().collect();

    let restored = Session::restore(
        root(),
        None,
        entries.clone(),
        0,
        entries.len() as u64,
        100,
        agent_core::AgentLimits::default(),
        &mut |_| {},
    )
    .expect("游标为 0 的日志是合法的（全部可 redo）");
    assert!(
        restored.host_tools().is_empty(),
        "游标在声明之前 = 那一步还没发生，恢复出来的会话不该认得那些工具"
    );

    // 正对照：同一份日志、游标在声明之后，工具就该在——只断言上面那一句的话，
    // 一个「从来不恢复任何声明」的实现同样会绿。
    let with_cursor_at_the_end = Session::restore(
        root(),
        None,
        entries.clone(),
        entries.len(),
        entries.len() as u64,
        100,
        agent_core::AgentLimits::default(),
        &mut |_| {},
    )
    .expect("游标在末尾是最普通的那种恢复");
    assert_eq!(with_cursor_at_the_end.host_tools().len(), 2);
}
