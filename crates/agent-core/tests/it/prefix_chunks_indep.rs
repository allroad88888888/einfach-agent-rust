//! 134 独立测试（agent-core 层）：「开局前缀块」状态的四条投影——写入即时可读、
//! 快照/恢复原样往返、entry 级 undo/redo、空写不留幽灵条目。
//!
//! **独立测试规则**：这份测试只依据 `docs/issues/134-prefix-chunk-state.md` 的
//! 「验收」「注意」两节、`docs/INVARIANTS.md` 红线 2/3/4/11、以及被测接口的公开签名
//! 写成——没有读过 `command/prefix*.rs` 或 `value/prefix_chunks.rs` 的实现，跟负责
//! 实现的 agent 并行、互不通信（同一条规则见 `host_skills_indep_restore.rs` /
//! `host_tools_indep_restore.rs` 两份先例文件顶部）。
//!
//! 先例：这两份文件钉的是同一类「创建期写入、恢复原样回来」的状态，134 的
//! undo/恢复语义逐字对齐它们（issue 原文）。跟它们刻意不同的一处：前缀块**不
//! 按 id/名字排序**——顺序就是拼进 system prompt 的顺序，本身是语义的一部分
//! （料单规则：宁可分，不可合，也不可乱序），所以下面每条断言钉的是「写入序」，
//! 不是「集合相等」。
//!
//! 另一处刻意选择：恢复相关的用例一律把 `session.primitives()` 整份当快照灌回去，
//! 不去挑一个单独的 `Slot` 变体——issue 里那个变体名只是「建议」，不在被测接口的
//! 契约里，独立测试不该替实现方拍板一个没写进契约的细节。

use std::sync::Arc;

use agent_core::{AgentId, AgentValue, AtomKey, Session, SystemChunk};

fn root() -> AgentId {
    AgentId::root()
}

/// 两块前缀：label **故意不按字典序给**（`zzz_...` 写在前、`aaa_...` 写在后，
/// 字典序会把它们颠倒过来）；一块多行 ASCII，一块非 ASCII 多行（含标点与 emoji）。
fn chunks() -> Vec<SystemChunk> {
    vec![
        SystemChunk {
            label: Arc::from("zzz_skill_late"),
            text: Arc::from("Line one.\nLine two with  extra   spaces.\n"),
        },
        SystemChunk {
            label: Arc::from("aaa_base_system"),
            text: Arc::from("先做这件事。\n再做那件事，别偷懒——含标点、换行与 emoji 🎯。"),
        },
    ]
}

/// 验收 1：`set_prefix_chunks` 两块（含多行与非 ASCII）→ `prefix_chunks()` 逐字节
/// 相同，顺序 = 写入顺序，没有被排序。
#[test]
fn set_prefix_chunks_preserves_write_order_and_content_byte_for_byte() {
    let mut session = Session::new(root());
    session.set_prefix_chunks(chunks());

    let got = session.prefix_chunks();
    assert_eq!(got.len(), 2);

    // 顺序：字典序会把 aaa 排到 zzz 前面，写入序不会——这里钉的就是写入序。
    assert_eq!(
        &*got[0].label, "zzz_skill_late",
        "第一块该是写入时排第一的那块，不是字典序第一的那块"
    );
    assert_eq!(&*got[1].label, "aaa_base_system");

    // 内容逐字节相同：多行 + 非 ASCII。
    assert_eq!(&*got[0].text, "Line one.\nLine two with  extra   spaces.\n");
    assert_eq!(
        &*got[1].text,
        "先做这件事。\n再做那件事，别偷懒——含标点、换行与 emoji 🎯。"
    );

    assert_eq!(
        got,
        chunks(),
        "SystemChunk 派生了 Eq，整份也该逐项相等（label+text 一个不差）"
    );
}

/// 验收 2：快照 → 恢复 → `prefix_chunks()` 逐字节相同（含顺序）。
/// 手法照 `host_skills_indep_restore.rs`：`Session::restore` 灌回一份手上的快照，
/// 断言 `on_unknown_key` 没被叫到（这一版认得这个槽位）。
#[test]
fn a_snapshot_restores_prefix_chunks_byte_for_byte_in_write_order() {
    let mut source = Session::new(root());
    source.set_prefix_chunks(chunks());
    let snapshot = source.primitives();

    let mut unknown: Vec<AtomKey> = Vec::new();
    let restored = Session::restore(root(), Some(snapshot), Vec::new(), 0, 0, 100, &mut |k| {
        unknown.push(k.clone())
    })
    .expect("合法快照该能恢复");
    assert!(
        unknown.is_empty(),
        "前缀块是这一版认识的槽位，不该报进 on_unknown_key：{unknown:?}"
    );

    assert_eq!(
        restored.prefix_chunks(),
        chunks(),
        "快照恢复后 prefix_chunks() 必须逐字节相同、顺序不变"
    );
}

/// 验收 3（前半）：写入使 history 长度 +1，entry label 是 `"prefix_init"`。
#[test]
fn writing_appends_exactly_one_entry_labeled_prefix_init() {
    let mut session = Session::new(root());
    assert_eq!(
        session.history().entries().count(),
        0,
        "刚建出来的会话不该有任何 entry"
    );

    session.set_prefix_chunks(chunks());

    let entries: Vec<_> = session.history().entries().cloned().collect();
    assert_eq!(entries.len(), 1, "写入前缀块该正好留下一条 entry");
    assert_eq!(entries[0].meta.label, "prefix_init");
}

/// 验收 3（后半）：entry 级 undo 退掉后为空、redo 恢复。
///
/// 单一线性日志（决策 4）里，「undo 到某一条之前」就是把游标停在那条 entry
/// 之前重放；「redo」就是把游标推回它之后——这正是 `host_tools_indep_restore.rs`
/// 的 `a_log_whose_cursor_sits_before_the_declaration_restores_without_it` 钉的
/// 那件事，这里对齐同款正反两面断言，避免「从来不恢复任何东西」的假实现蒙混过关。
#[test]
fn entry_level_undo_takes_it_back_and_redo_restores_it() {
    let mut source = Session::new(root());
    source.set_prefix_chunks(chunks());
    let log: Vec<_> = source.history().entries().cloned().collect();
    assert_eq!(log.len(), 1, "前置条件：写入只留一条 entry");
    let next_seq = log.len() as u64;

    // undo：游标停在写入之前 → 前缀块该为空。
    let undone = Session::restore(root(), None, log.clone(), 0, next_seq, 100, &mut |_| {})
        .expect("游标为 0 的日志是合法的（全部可 redo）");
    assert!(
        undone.prefix_chunks().is_empty(),
        "entry 级 undo 退掉之后，前缀块不该认得任何写入"
    );

    // redo（正对照）：游标推回写入之后 → 内容跟写入时一样。
    let redone = Session::restore(root(), None, log.clone(), log.len(), next_seq, 100, &mut |_| {})
        .expect("游标在末尾是最普通的那种恢复");
    assert_eq!(
        redone.prefix_chunks(),
        chunks(),
        "redo 之后该恢复到写入时的内容，顺序也不变"
    );
}

/// 验收 4：空 `Vec` 写入不产生 entry（幽灵条目）。
#[test]
fn writing_an_empty_vec_produces_no_entry() {
    let mut session = Session::new(root());
    session.set_prefix_chunks(Vec::new());

    assert_eq!(
        session.history().entries().count(),
        0,
        "空 Vec 写入不该留下幽灵 entry"
    );
    assert!(
        session.prefix_chunks().is_empty(),
        "没写过东西，prefix_chunks() 该是空的"
    );
}

/// 验收 5：恢复出的 Session 不需要任何外部数据源，直接 `prefix_chunks()` 即得
/// （值就是状态，不是靠某个还活着的运行时对象现算）。
///
/// 做法：把快照经过一次真实的 serde 往返（字符串来回），并在恢复前 `drop` 掉
/// 原始 `Session` 与快照本身——恢复出来的值除了那份反序列化的字节，不可能再
/// 拿到任何东西；如果 `prefix_chunks()` 还能答对，就证明它是纯粹的状态读取。
/// 顺带覆盖红线 3（primitive 值必须可序列化）与红线 11（同一份内容两次序列化
/// 逐字节相同——`check-invariants` 盯的正是这条）。
#[test]
fn a_restored_session_yields_prefix_chunks_directly_with_no_external_source() {
    let mut source = Session::new(root());
    source.set_prefix_chunks(chunks());
    let primitives = source.primitives();

    let once = serde_json::to_string(&primitives).expect("红线 3：primitive 的值必须全部可序列化");
    let snapshot: Vec<(AtomKey, AgentValue)> =
        serde_json::from_str(&once).expect("自己写出来的快照必须读得回来");
    let twice = serde_json::to_string(&snapshot).expect("往返之后仍该可序列化");
    assert_eq!(once, twice, "同一份前缀块两次序列化必须逐字节相同（红线 11）");

    // 切断跟原对象的一切联系：后面只剩反序列化出来的 `snapshot`。
    drop(source);
    drop(primitives);

    let restored = Session::restore(root(), Some(snapshot), Vec::new(), 0, 0, 100, &mut |_| {})
        .expect("合法快照该能恢复");

    assert_eq!(
        restored.prefix_chunks(),
        chunks(),
        "恢复之后不接任何外部数据源，直接调 getter 就该拿到写入时的值"
    );
}
