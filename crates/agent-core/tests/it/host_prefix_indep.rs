//! 154 独立测试（agent-core 层）：`Slot::HostPrefix`——宿主经 `capabilities.prefix`
//! 声明的开局块 `(name, text)` 对，跟 073 的 `Slot::HostTools` 同构：建会话时
//! journaled 地写进 store，恢复时从日志回放自动回来。
//!
//! **独立测试规则**：本文件只依据 `docs/issues/154-host-prefix-slot.md` 的「验收」
//! 「目标」两节、`docs/INVARIANTS.md` 红线 3/4/11、以及被测接口的公开签名写成——
//! 没有读过 `value/host_prefix.rs`、`command/host_prefix.rs`、`graph/atom_key.rs`
//! 的实现，跟负责实现的 agent 并行、互不通信（同一条规则见
//! `host_tools_indep_restore.rs` / `prefix_chunks_indep.rs` 两份先例文件顶部）。
//!
//! 跟 `prefix_chunks_indep.rs`（134）刻意不同的一处：`HostPrefix` **按 name 排序**
//! 落店（issue 原文「红线 11：客户端数组顺序不可信」），跟 `host_tools` 同款——
//! 所以下面的断言钉的是「排序后的序」，不是「写入序」。

use std::sync::Arc;

use agent_core::{AgentId, AgentValue, AtomKey, Session, Slot};

fn root() -> AgentId {
    AgentId::root()
}

/// 三块开局前缀，**故意不按 name 的字典序给**（`zzz_` 写在最前，`aaa_` 写在最后），
/// 内容含多行与非 ASCII，好让「排序」与「字节保真」两件事都被考验到。
fn pairs() -> Vec<(Arc<str>, Arc<str>)> {
    vec![
        (
            Arc::from("zzz_footer"),
            Arc::from("页脚：本次回答由系统生成。\n"),
        ),
        (
            Arc::from("mid_disclaimer"),
            Arc::from("以下内容仅供参考，不构成任何建议。"),
        ),
        (
            Arc::from("aaa_header"),
            Arc::from("系统身份声明\n第二行：始终保持专业与克制 🎯。"),
        ),
    ]
}

/// 跟 `pairs()` **内容完全相同**，写入顺序不同（真实打乱，不是简单反转）——用来钉
/// 「落盘字节只看内容，不看客户端给的顺序」。
fn pairs_shuffled_same_content() -> Vec<(Arc<str>, Arc<str>)> {
    vec![
        (
            Arc::from("mid_disclaimer"),
            Arc::from("以下内容仅供参考，不构成任何建议。"),
        ),
        (
            Arc::from("aaa_header"),
            Arc::from("系统身份声明\n第二行：始终保持专业与克制 🎯。"),
        ),
        (
            Arc::from("zzz_footer"),
            Arc::from("页脚：本次回答由系统生成。\n"),
        ),
    ]
}

/// 验收 1（前半）：写 → 读 roundtrip，读回按 name 排序，内容逐字节不变。
#[test]
fn write_then_read_roundtrip_sorts_by_name() {
    let mut session = Session::new(root());
    session.declare_host_prefix(pairs());

    let got = session.host_prefix();
    assert_eq!(got.len(), 3);
    assert_eq!(
        &*got[0].0, "aaa_header",
        "读回该按 name 排序，不是写入序（写入序里 zzz_footer 在最前）"
    );
    assert_eq!(&*got[1].0, "mid_disclaimer");
    assert_eq!(&*got[2].0, "zzz_footer");

    assert_eq!(&*got[0].1, "系统身份声明\n第二行：始终保持专业与克制 🎯。");
    assert_eq!(&*got[1].1, "以下内容仅供参考，不构成任何建议。");
    assert_eq!(&*got[2].1, "页脚：本次回答由系统生成。\n");
}

/// 验收 1（后半）：两次声明**内容相同、输入序不同** → 落盘序列化逐字节相同
/// （红线 11：会进 prompt 的东西不能让客户端数组顺序漏进字节）。
#[test]
fn declarations_with_different_input_order_serialize_byte_identical() {
    let mut a = Session::new(root());
    a.declare_host_prefix(pairs());

    let mut b = Session::new(root());
    b.declare_host_prefix(pairs_shuffled_same_content());

    let key = AtomKey::Agent(root(), Slot::HostPrefix);
    let value_a = a
        .primitives()
        .into_iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
        .expect("HostPrefix 是 source 槽位，建图时就该有值");
    let value_b = b
        .primitives()
        .into_iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
        .expect("同上");

    assert_eq!(
        serde_json::to_string(&value_a).unwrap(),
        serde_json::to_string(&value_b).unwrap(),
        "同一份内容、不同输入序，落盘字节必须逐字节相同"
    );
    // 顺带钉住：这不是两边都退化成空值才凑巧相等。
    assert_eq!(a.host_prefix().len(), 3);
    assert_eq!(a.host_prefix(), b.host_prefix());
}

/// 验收 2：空声明写入 = 无痕——history 长度不变，没有幽灵 entry，读口仍是空。
#[test]
fn an_empty_declaration_leaves_no_trace() {
    let mut session = Session::new(root());
    assert_eq!(session.history().entries().count(), 0);

    session.declare_host_prefix(Vec::new());

    assert_eq!(
        session.history().entries().count(),
        0,
        "空声明不该留下幽灵 entry（134 的既有语义：值等于默认值时 record_set 不产生 Change）"
    );
    assert!(
        session.host_prefix().is_empty(),
        "没声明过东西，host_prefix() 该是空的"
    );
}

/// 验收：非空声明恰好留下一条 journaled entry，label 在 `KNOWN_LABELS` 里。
#[test]
fn a_non_empty_declaration_appends_exactly_one_entry() {
    let mut session = Session::new(root());
    session.declare_host_prefix(pairs());

    let entries: Vec<_> = session.history().entries().cloned().collect();
    assert_eq!(entries.len(), 1, "声明该正好留下一条 entry：{entries:?}");
    assert_eq!(entries[0].meta.label, "declare_host_prefix");
}

/// 验收 4：undo 一条 `declare_host_prefix` entry → 读口回到空；redo 回来（073 同款
/// 正反两面断言，避免「从来不恢复任何声明」的假实现蒙混过关）。
#[test]
fn undo_clears_it_and_redo_restores_it() {
    let mut original = Session::new(root());
    original.declare_host_prefix(pairs());
    let log: Vec<_> = original.history().entries().cloned().collect();
    assert_eq!(log.len(), 1);
    let next_seq = log.len() as u64;

    let undone = Session::restore(root(), None, log.clone(), 0, next_seq, 100, &mut |_| {})
        .expect("游标为 0 的日志是合法的（全部可 redo）");
    assert!(
        undone.host_prefix().is_empty(),
        "undo 之后（游标在声明之前），host_prefix() 该回到空"
    );

    let redone = Session::restore(root(), None, log.clone(), log.len(), next_seq, 100, &mut |_| {})
        .expect("游标在末尾是最普通的那种恢复");
    assert_eq!(
        redone.host_prefix(),
        original.host_prefix(),
        "redo 之后该恢复到声明时的内容"
    );
}

/// 崩溃恢复（快照路径）：`session.primitives()` 走一次真实 serde 往返（模拟落盘/
/// 重启），`drop` 掉原始 `Session` 与快照本身，再 `Session::restore`——`host_prefix()`
/// 除了那份反序列化字节，不可能再拿到任何东西。顺带钉住同一份内容两次序列化逐
/// 字节相同（红线 11）。手法照 `prefix_chunks_indep.rs` 的同名先例。
#[test]
fn a_restored_session_yields_host_prefix_from_persisted_bytes_alone() {
    let mut source = Session::new(root());
    source.declare_host_prefix(pairs());
    let before = source.host_prefix();
    let primitives = source.primitives();

    let once = serde_json::to_string(&primitives).expect("红线 3：primitive 的值必须全部可序列化");
    let snapshot: Vec<(AtomKey, AgentValue)> =
        serde_json::from_str(&once).expect("自己写出来的快照必须读得回来");
    let twice = serde_json::to_string(&snapshot).expect("往返之后仍该可序列化");
    assert_eq!(once, twice, "同一份声明两次序列化必须逐字节相同（红线 11）");

    drop(source);
    drop(primitives);

    let restored = Session::restore(root(), Some(snapshot), Vec::new(), 0, 0, 100, &mut |_| {})
        .expect("合法快照该能恢复");

    assert_eq!(
        restored.host_prefix(),
        before,
        "崩溃恢复后，host_prefix() 必须跟恢复前一致"
    );
}

/// 崩溃恢复（日志路径）：真实写一次声明 → 取整段日志 → 喂进一个全新
/// `Session::restore` → `host_prefix()` 跟原会话一致。上一条证的是「`restore` 认得
/// 这个值形状」，这条证的是「声明确实落进了日志、日志能被自己重放」——两件不同
/// 的事（手法照 `host_tools_indep_restore.rs` 的同名先例）。
#[test]
fn restoring_a_real_sessions_log_reproduces_its_declaration() {
    let mut original = Session::new(root());
    original.declare_host_prefix(pairs());

    let entries: Vec<_> = original.history().entries().cloned().collect();
    let cursor = original.cursor();
    let restored = Session::restore(
        root(),
        None,
        entries.clone(),
        cursor,
        entries.len() as u64,
        100,
        &mut |_| {},
    )
    .expect("原会话产出的日志必须能被自己重放");

    assert_eq!(restored.host_prefix(), original.host_prefix());
}

/// 老快照（这一版加 `Slot::HostPrefix` 之前建的会话文件，没有这个键）反序列化不
/// 炸，读出默认空——不该报进 `on_unknown_key`（那是「这一版不认识的键」，反过来
/// 才对：`HostPrefix` 是这一版认识、老快照没写的键，走的是 `default_value()` 兜底，
/// 不是恐慌，也不是把某份部署期清单当默认值塞进来）。
///
/// 快照里手工放**除 `HostPrefix` 外的全部** `Slot::ALL` 默认值，逼真模拟「老快照
/// 一个字段不多不少，就是缺这一个键」，而不是笼统传 `None` 让全部槽位都缺。
#[test]
fn an_old_snapshot_missing_the_key_falls_back_to_empty_default() {
    let agent = root();
    let snapshot: Vec<(AtomKey, AgentValue)> = Slot::ALL
        .iter()
        .filter(|slot| **slot != Slot::HostPrefix)
        .map(|slot| (AtomKey::Agent(agent.clone(), *slot), slot.default_value()))
        .collect();
    assert_eq!(snapshot.len(), Slot::ALL.len() - 1, "前置条件：确实少了一个键");

    let mut unknown: Vec<AtomKey> = Vec::new();
    let session = Session::restore(agent, Some(snapshot), Vec::new(), 0, 0, 100, &mut |k| {
        unknown.push(k.clone())
    })
    .expect("缺一个这一版认识的键，不该导致恢复失败");

    assert!(
        unknown.is_empty(),
        "HostPrefix 是这一版认识的槽位，缺失不该报进 on_unknown_key：{unknown:?}"
    );
    assert!(
        session.host_prefix().is_empty(),
        "老快照没有这个键，读口该是默认空，不是恐慌"
    );
}

/// 红线钉子：`Slot::ALL` 恰好 21 个（154 从 20 加到 21），且 `HostPrefix` 在其中，
/// `default_value()` 是空 `Vec`——新建会话不用声明什么都能正常跑。
#[test]
fn slot_all_has_twenty_one_entries_including_host_prefix_with_an_empty_default() {
    assert_eq!(Slot::ALL.len(), 21, "154 把 Slot::ALL 从 20 个加到 21 个");
    assert!(Slot::ALL.contains(&Slot::HostPrefix));

    let fresh = Session::new(root());
    assert!(
        fresh.host_prefix().is_empty(),
        "新会话没声明过任何前缀块，default 该是空"
    );
}
