//! `ext:stats` 包本身的单测：开关、审计文件的落点与节奏、跨重启续号，以及
//! **不开开关时工具表逐字节零变化**（149 验收第 4 条的测试级证据）。
//!
//! 正文渲染的断言在 `ext_stats_report_tests.rs`——两份职责各测各的。

use std::path::PathBuf;

use agent_core::AgentId;
use agent_runtime::ToolTable;
use serde_json::Value;

use super::*;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// 每个用例一个独立目录，跑完自己收拾。
fn tmp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "agent-cli-ext-stats-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn lines(path: &PathBuf) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| l.to_string())
        .collect()
}

/// 表半边装完之后，模型面上多的正好是 `ext:stats/report` 这一条，**在表尾**。
/// timed 那条（`ext:stats/audit`）一个字节都不进 `specs()`。
#[test]
fn the_pack_appends_exactly_one_model_facing_spec_at_the_tail() {
    let before: Vec<String> = ToolTable::standard_local()
        .specs()
        .iter()
        .map(|s| s.name.to_string())
        .collect();

    let (table, pending) = ToolTable::standard_local().with_extension(pack(Ledger::new(None)));
    let after: Vec<String> = table.specs().iter().map(|s| s.name.to_string()).collect();

    assert_eq!(after.len(), before.len() + 1);
    assert_eq!(
        &after[..before.len()],
        &before[..],
        "前面那段一个字节都不该动"
    );
    assert_eq!(after.last().unwrap(), REPORT_TOOL);
    assert!(
        !after.iter().any(|n| n == AUDIT_TOOL),
        "timed 条目不进 specs"
    );

    // `PendingInterceptors` 必须被消费，否则 Drop 里会炸——这里没有 ctx 可装，
    // 用 forget 明确表示「这个用例只看表半边」。
    std::mem::forget(pending);
}

/// **验收 4 的测试级证据**：`install(.., on=false, ..)` 之后的表，与压根没经过
/// 这个函数的表**逐字节相同**（spec 的三个字段全序列化出来比）。
#[test]
fn the_switch_off_leaves_the_table_byte_identical() {
    let untouched = json_specs(&ToolTable::standard_local());
    let (passed_through, pending) = install(
        ToolTable::standard_local(),
        false,
        Some(std::path::Path::new("/tmp/never-written.jsonl")),
        &mut |_| {},
    );
    assert!(pending.is_none(), "没装就不该给出 ctx 半边");
    assert_eq!(json_specs(&passed_through), untouched);
}

fn json_specs(table: &ToolTable) -> String {
    serde_json::to_string(table.specs()).unwrap()
}

#[test]
fn the_flag_is_off_unless_it_is_spelled_out() {
    assert!(!enabled(&args(&["agent-cli"])));
    assert!(!enabled(&args(&["agent-cli", "--session", "/tmp/s.jsonl"])));
    assert!(enabled(&args(&["agent-cli", "--ext-stats"])));
    assert!(enabled(&args(&[
        "agent-cli",
        "--session",
        "/tmp/s.jsonl",
        "--ext-stats"
    ])));
}

/// 审计文件在会话文件**旁边**，整体追加 `.audit.log`；临时会话没有落点。
#[test]
fn the_audit_file_sits_next_to_the_session_file() {
    assert_eq!(
        audit_path(Some(std::path::Path::new("/tmp/a/s.jsonl"))),
        Some(PathBuf::from("/tmp/a/s.jsonl.audit.log"))
    );
    assert_eq!(audit_path(None), None);
}

/// 一次 hook 触发 = 恰好一行。三次触发三行，一行不多一行不少（验收 3 的
/// 「每完成轮恰一行」在这一层的断言；「取消轮不触发」由 136 的驱动保证，
/// `agent-runtime` 的 `turn_end_indep.rs` 已经钉死）。153 起 `append_turn_line`
/// 收 `&Session`——一个从没变过的空会话，三行该逐字节相同（只有 `turn=` 递增）。
#[test]
fn every_fire_appends_exactly_one_line() {
    let dir = tmp_dir("cadence");
    let path = dir.join("s.jsonl.audit.log");
    let ledger = Ledger::new(Some(path.clone()));
    let session = Session::new(AgentId::root());

    for _ in 0..3 {
        ledger.append_turn_line(&session).expect("写审计行该成功");
    }

    let written = lines(&path);
    assert_eq!(written.len(), 3);
    assert_eq!(written[0], "turn=1 entries=0/0 agents=1 tools=0");
    assert_eq!(written[2], "turn=3 entries=0/0 agents=1 tools=0");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 153（决策 30）：`audit` 不再靠 `report` 传话，每次触发都在轮末**现读**一次
/// `&Session`——两次触发之间给会话添一条真实的 command log entry，第二行的
/// `entries`（`X/Y` 的 `Y`，物理条数）必须等于**那一刻**的 `session.history_len()`，
/// 不是第一行的老数字、也不是任何缓存值。
#[test]
fn the_second_audit_line_reports_the_history_len_at_that_moment() {
    let dir = tmp_dir("live-read");
    let path = dir.join("s.jsonl.audit.log");
    let ledger = Ledger::new(Some(path.clone()));

    let mut session = Session::new(AgentId::root());
    ledger.append_turn_line(&session).unwrap(); // 第 1 轮：空会话

    session.set_max_turns(7); // 造一条真实的 command log entry
    let history_len = session.history_len();
    assert!(
        history_len > 0,
        "夹具没有制造出任何 entry，这条测试测不出东西"
    );
    ledger.append_turn_line(&session).unwrap(); // 第 2 轮：账本已经多了这条 entry

    let written = lines(&path);
    assert_eq!(written.len(), 2);
    assert_eq!(written[0], "turn=1 entries=0/0 agents=1 tools=0");
    assert_eq!(
        written[1],
        format!("turn=2 entries={history_len}/{history_len} agents=1 tools=0"),
        "第二行的 entries 必须等于调用那一刻的 history_len——现读，不是传话"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `kill -9` 之后重开一本账：轮序号从既有文件的行数续起，不从 1 重新数
/// （验收 6 那条路上的小事，但审计文件被数第二遍就没法 `wc -l` 了）。
#[test]
fn the_turn_ordinal_continues_after_a_restart() {
    let dir = tmp_dir("restart");
    let path = dir.join("s.jsonl.audit.log");
    let session = Session::new(AgentId::root());

    let first = Ledger::new(Some(path.clone()));
    first.append_turn_line(&session).unwrap();
    first.append_turn_line(&session).unwrap();
    drop(first);

    let second = Ledger::new(Some(path.clone()));
    assert_eq!(second.turns(), 2, "新账本该知道前面已经跑过两轮");
    second.append_turn_line(&session).unwrap();

    let written = lines(&path);
    assert_eq!(written.len(), 3);
    assert!(written[2].starts_with("turn=3 "));
    let _ = std::fs::remove_dir_all(&dir);
}

/// 没有会话文件（临时会话）：钩子照跑、照数轮，只是不写盘，也不报错。
#[test]
fn a_memory_session_counts_turns_without_writing_anything() {
    let ledger = Ledger::new(None);
    let session = Session::new(AgentId::root());
    assert!(ledger.append_turn_line(&session).is_ok());
    assert_eq!(ledger.turns(), 1);
}

/// 写不进去（路径是个目录）→ 回 `Err`，交给 136 的驱动记日志；**不 panic**，
/// 这一轮的结果一个字节都不受影响。
#[test]
fn a_write_failure_comes_back_as_err_not_a_panic() {
    let dir = tmp_dir("unwritable");
    let ledger = Ledger::new(Some(dir.clone()));
    let session = Session::new(AgentId::root());
    let outcome = ledger.append_turn_line(&session);
    assert!(outcome.is_err(), "往一个目录写行该失败");
    assert_eq!(ledger.turns(), 1, "失败也照样算这一轮跑过");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 两条 spec 的名字都吃 `ext:stats/` 前缀强制（装配期硬闸在 `ExtensionPack`
/// 那边，这里钉住我们写的名字本身没跑偏），描述里没有会变的字节。
#[test]
fn both_names_live_in_the_packs_namespace() {
    assert!(REPORT_TOOL.starts_with(&format!("ext:{PACK}/")));
    assert!(AUDIT_TOOL.starts_with(&format!("ext:{PACK}/")));
    let spec = report_spec();
    assert_eq!(&*spec.name, REPORT_TOOL);
    assert_eq!(
        spec.schema.as_ref(),
        &serde_json::json!({ "type": "object", "properties": {} })
    );
    assert_eq!(report_spec(), spec, "同一份声明两次调用逐字节相同");
}

/// 截获执行体真的能跑：拿一个空会话调一次，回的是报告正文。153 起 `report_run`
/// 不再收 `Ledger`——它没有任何副作用可记（见 `ext_stats.rs` 模块文档「`Pure` 的
/// 举证」），这条测试因此不再断言账本状态。
#[test]
fn the_intercept_body_renders() {
    let run = report_run();
    let mut session = Session::new(AgentId::root());
    let (body, aftermath) =
        run(&mut session, &AgentId::root(), &Value::Null).expect("纯读不该失败");
    assert!(body.starts_with("本会话至今："));
    // 201：交 `Nothing`——`report` 没碰外部世界，`/undo` 路过它不该停下来问。
    assert!(matches!(aftermath, Aftermath::Nothing));
}
