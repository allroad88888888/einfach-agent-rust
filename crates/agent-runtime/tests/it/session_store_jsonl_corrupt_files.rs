//! 011 验收「Jsonl 文件损坏（截断最后一行）→ 明确报错指出哪里坏了，不 panic 不静默丢」
//! 加上它的另一半：中部损坏。两种坏文件手工拼出来（不依赖真的 `kill -9` 掐出半行——
//! 那样时序不可控），直接验证 `load()` 的两条分支。
//!
//! 崩溃语义（`docs/issues/011-session-store.md` + `agent_store::persist::LoadOutcome`
//! 「契约更正」——027 独测发现「中部损坏」不该跟「从没写过」共用同一个 `None`）：
//! - **尾部半行**：容忍，从该行截断，经 `on_error` 报 `TruncatedTail`，前面的内容
//!   照常加载，结果是 `Loaded`。
//! - **中部损坏**：整份拒绝，经 `on_error` 报 `CorruptLine`，`load()` 返回
//!   `LoadOutcome::Refused`（**不是** `Absent`：这个身份下明明写过东西，只是这一份
//!   数据现在读不出来）——不静默丢中段、不加载半份状态。

use std::io::Write;

use agent_store::history::{Change, Entry};
use agent_store::SessionStore;

use crate::session_store_support::{collecting_on_error, temp_path, Val};
use agent_runtime::{Jsonl, SessionStoreError};

type Backend = Jsonl<String, Val, u32>;

fn valid_entry_line(seq: u64) -> String {
    let entry = Entry {
        seq,
        meta: 1u32,
        changes: vec![Change {
            key: "a".to_string(),
            prev: Val(seq as i64),
            next: Val(seq as i64 + 1),
        }],
    };
    // 跟 `Jsonl` 内部写的格式一样：`{"kind":"entry",...}`——直接拼一行，不经过
    // `agent_runtime` 的私有 `Record` 类型（那是 crate 内部实现，测试只依赖它的
    // 外部可观察行为：这一行 `load()` 得回来）。
    format!(
        r#"{{"kind":"entry","seq":{},"meta":{},"changes":[{{"key":"{}","prev":{},"next":{}}}]}}"#,
        entry.seq,
        entry.meta,
        entry.changes[0].key,
        entry.changes[0].prev.0,
        entry.changes[0].next.0
    )
}

fn write_raw(path: &std::path::Path, content: &str) {
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(content.as_bytes()).unwrap();
}

#[test]
fn a_truncated_last_line_is_dropped_with_a_warning_and_the_rest_still_loads() {
    let path = temp_path("corrupt-tail");
    let good = valid_entry_line(0);
    // 尾部半行：一整条合法记录 + 一行写到一半就断了的 JSON（缺右括号）。
    let content =
        format!("{good}\n{{\"kind\":\"entry\",\"seq\":1,\"meta\":1,\"changes\":[{{\"key\":\"a\"");
    write_raw(&path, &content);

    let (errors, on_error) = collecting_on_error();
    let backend: Backend = Jsonl::new(&path, on_error);
    let loaded = backend
        .load()
        .loaded()
        .expect("前面那一整条合法记录应该能载入");

    assert_eq!(loaded.entries.len(), 1);
    assert_eq!(loaded.entries[0].seq, 0);
    let seen = errors.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert!(matches!(
        seen[0],
        SessionStoreError::TruncatedTail { line: 2 }
    ));
}

#[test]
fn a_corrupt_middle_line_rejects_the_whole_load_even_though_more_valid_lines_follow() {
    let path = temp_path("corrupt-middle");
    let line0 = valid_entry_line(0);
    let line2 = valid_entry_line(2);
    // 中间那行不是合法 JSON——注意后面还跟着一条完全合法的记录，验证的正是
    // 「不能因为后面还有更多合法内容就假装这一份是完整的」。
    let content = format!("{line0}\nnot json at all\n{line2}\n");
    write_raw(&path, &content);

    let (errors, on_error) = collecting_on_error();
    let backend: Backend = Jsonl::new(&path, on_error);
    let outcome = backend.load();
    let reason = match outcome {
        agent_store::persist::LoadOutcome::Refused { reason } => reason,
        other => panic!(
            "中部损坏必须整份拒绝（Refused），不能只加载前半段，也不能悄悄当成 Absent：{}",
            other.is_absent()
        ),
    };
    assert!(reason.contains('2'), "拒绝理由该带上具体行号：{reason}");

    let seen = errors.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert!(matches!(
        seen[0],
        SessionStoreError::CorruptLine { line: 2 }
    ));
}

/// 错误消息本身不能带 K/V 内容——状态里可能是用户对话。这里明确断言 `Display`
/// 输出里不含被写坏那一行本来会有的任何值片段（本测试故意把值设成一个独特、
/// 容易在错误文本里现形的数字）。
#[test]
fn the_error_never_carries_the_offending_lines_content() {
    let path = temp_path("corrupt-no-leak");
    let secret = "super-secret-user-message-content-should-never-leak";
    let content =
        format!("{{\"kind\":\"entry\",\"seq\":0,\"meta\":1,\"changes\":[{{\"key\":\"{secret}\"\n"); // 断在一半
    write_raw(&path, &content);

    let (errors, on_error) = collecting_on_error();
    let backend: Backend = Jsonl::new(&path, on_error);
    let _ = backend.load();

    let seen = errors.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    let rendered = format!("{}", seen[0]);
    assert!(
        !rendered.contains(secret),
        "错误文本泄漏了行内容：{rendered}"
    );
}
