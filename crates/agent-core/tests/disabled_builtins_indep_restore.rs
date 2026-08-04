//! 076 独立测试（agent-core 层）：**这个会话关掉了哪些内置工具**在恢复路径上原模
//! 原样回来。
//!
//! 形状照 073 的 `host_tools_indep_restore.rs` / 064 的 `host_skills_indep_restore.rs`
//! （同一条用户拍板：「历史对话就该跟历史一致，原模原样 100% 复刻」），但要钉的风险
//! **方向相反**——前两者丢了会「少几个工具」，这一个丢了会**多几个工具**：
//!
//! 一个关掉了 `srv:shell/exec` 的会话，整段历史里模型都没见过这个工具；开关不落盘，
//! 恢复出来它就突然多出一件从没被告知过的能力，而历史里没有任何铺垫。而且工具表在
//! prompt 最前面，多一项 = 恢复出来的第一轮前缀全断（红线 11）。
//!
//! 「多了」比「少了」更难查：少一个工具，模型会说「我没有这个工具」；多一个工具，
//! 什么症状都没有，直到它真的去调了。

use std::sync::Arc;

use agent_core::{AgentId, AgentValue, AtomKey, Session, Slot};

fn root() -> AgentId {
    AgentId::root()
}

/// 三个名字，**故意不按字典序给**，而且**故意含一个重复项**。
fn switch() -> Vec<Arc<str>> {
    vec![
        Arc::from("srv:shell/exec"),
        Arc::from("srv:agent/spawn"),
        Arc::from("srv:fs/list"),
        Arc::from("srv:shell/exec"),
    ]
}

fn sorted() -> Vec<Arc<str>> {
    vec![Arc::from("srv:agent/spawn"), Arc::from("srv:fs/list"), Arc::from("srv:shell/exec")]
}

/// 「关过 `switch()`」的会话里 `Slot::DisabledBuiltins` 该有的值——现查一个真实写入
/// 过的 `Session`，不猜它落进哪个 `AgentValue` 变体。
fn switch_value() -> AgentValue {
    value_of(switch())
}

fn value_of(names: Vec<Arc<str>>) -> AgentValue {
    let mut session = Session::new(root());
    session.disable_builtins(names);
    session
        .primitives()
        .into_iter()
        .find(|(k, _)| *k == AtomKey::Agent(root(), Slot::DisabledBuiltins))
        .map(|(_, v)| v)
        .expect("Slot::DisabledBuiltins 是一个 source 槽位，build_agent 建图时就该带默认值")
}

fn restore_with(snapshot: Vec<(AtomKey, AgentValue)>) -> Session {
    let mut unknown = Vec::new();
    let session = Session::restore(root(), Some(snapshot), Vec::new(), 0, 0, 100, &mut |k| unknown.push(k.clone()))
        .expect("合法快照该能恢复");
    assert!(unknown.is_empty(), "DisabledBuiltins 是这一版认识的槽位，不该报进 on_unknown_key：{unknown:?}");
    session
}

/// 直接注入快照：三个名字一个不差、一个不多（重复那一项已经在写入时去掉了）。
#[test]
fn a_snapshot_with_the_switch_restores_every_name() {
    let session = restore_with(vec![(AtomKey::Agent(root(), Slot::DisabledBuiltins), switch_value())]);
    assert_eq!(session.disabled_builtins(), sorted(), "写入时排序去重（红线 11），读回就是这一份");
}

/// 红线 3 + 红线 11：整份快照 serde 往返，而且**两次序列化逐字节相同**。
#[test]
fn the_switch_survives_a_serde_roundtrip_byte_for_byte() {
    let snapshot = vec![(AtomKey::Agent(root(), Slot::DisabledBuiltins), switch_value())];

    let once = serde_json::to_string(&snapshot).expect("快照该可序列化");
    let back: Vec<(AtomKey, AgentValue)> = serde_json::from_str(&once).expect("快照该可反序列化");
    let twice = serde_json::to_string(&back).expect("往返之后仍该可序列化");
    assert_eq!(once, twice, "同一份开关两次序列化必须逐字节相同（红线 11）");

    assert_eq!(restore_with(back).disabled_builtins(), sorted());
}

/// **红线 11 的落盘那一半**：同一份关闭列表**打乱顺序 / 多写一个重复项**，落进
/// 会话状态的字节完全相同。
///
/// 删掉 `agent_core::value::str_set::to_value` 里的 `sort()`/`dedup()` 这条就红。
///
/// 为什么这一条要断在**落盘字节**上而不是工具表字节上：工具表那一侧对顺序天生免疫
/// （剔除是集合运算，`ToolTable::without_builtins` 用 `retain` 保住五档原有次序，
/// 有断言钉在 `agent-runtime` 那边）。真正会跟着输入顺序漂的是**存进会话状态的那份
/// 字节**——它是恢复时回放的东西，也是「同一份配置的两个会话是不是同一份历史」的
/// 判据。顺序漏进这里，症状是两条本该一模一样的会话日志逐字节不同，而且不报错。
#[test]
fn the_stored_bytes_do_not_depend_on_input_order_or_duplicates() {
    let bytes = |v: &AgentValue| {
        let AgentValue::Json(json) = v else { panic!("落 Json") };
        serde_json::to_string(&**json).expect("值该可序列化")
    };

    let canonical = value_of(sorted());
    for (label, names) in [
        ("倒序", vec![Arc::<str>::from("srv:shell/exec"), Arc::from("srv:fs/list"), Arc::from("srv:agent/spawn")]),
        ("乱序 + 重复", switch()),
        ("再来一遍同一份", sorted()),
    ] {
        assert_eq!(
            bytes(&value_of(names)),
            bytes(&canonical),
            "{label}：关闭列表的输入顺序/重复项漏进了会话状态的落盘字节（红线 11）"
        );
    }
    assert_eq!(bytes(&canonical), r#"["srv:agent/spawn","srv:fs/list","srv:shell/exec"]"#);
}

/// 日志游标停在开关**之前** → 恢复出来就什么都没关（undo 那一条在核心层的落点）。
///
/// **带正对照**：只断言「游标在前面时没关」是自欺欺人——一个「从来就没恢复过任何
/// 开关」的实现同样会绿。
#[test]
fn a_log_whose_cursor_sits_before_the_switch_restores_without_it() {
    let mut source = Session::new(root());
    source.disable_builtins(switch());
    let log: Vec<_> = source.history().entries().cloned().collect();
    assert_eq!(log.len(), 1, "开关该正好留下一条 entry");
    assert_eq!(log[0].meta.label, "disable_builtins");
    let len = log.len() as u64;

    // 正对照：游标在开关之后 → 三个都关着。
    let after = Session::restore(root(), None, log.clone(), source.cursor(), len, 100, &mut |_| {}).expect("恢复该成功");
    assert_eq!(after.disabled_builtins(), sorted(), "游标在开关之后，三个都该还关着");

    // 游标在开关之前 → 一个都没关。
    let before = Session::restore(root(), None, log, 0, len, 100, &mut |_| {}).expect("恢复该成功");
    assert!(before.disabled_builtins().is_empty(), "游标停在开关之前，这个会话不该关着任何东西");
}

/// **开关跟声明一起回来**：一个会话可以既注入了工具、又关掉了几件内置的，两个槽位
/// 各自独立、互不覆盖。
///
/// 分开落盘、漏掉一半的形状是：恢复出来工具表比当初**多几项**（开关丢了）或者
/// **少几项**（声明丢了），两种都不报错。
#[test]
fn the_switch_and_the_declaration_come_back_together() {
    let mut source = Session::new(root());
    source.declare_host_tools(vec![(
        agent_core::ToolSpec {
            name: Arc::from("web:crm/lookup"),
            description: Arc::from("查档案"),
            schema: Arc::new(serde_json::json!({ "type": "object" })),
        },
        agent_core::Reversibility::Pure,
    )]);
    source.disable_builtins(switch());

    let session = restore_with(source.primitives());
    assert_eq!(session.disabled_builtins(), sorted(), "开关该回来");
    assert_eq!(session.host_tools().len(), 1, "声明也该回来——两个槽位互不覆盖");
    assert_eq!(&*session.host_tools()[0].0.name, "web:crm/lookup");
}
