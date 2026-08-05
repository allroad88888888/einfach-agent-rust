//! 独立测试 agent（issue 027）验收点 5：坏文件。
//!
//! - 尾部截断半行（最后一行是不完整的 JSON，模拟"写到一半被杀"）→ 启动
//!   容忍（warn 语义），并且能继续对话。
//! - 中部一行整个损坏（非法 JSON，且不是最后一行）→ 启动报错且明确指出
//!   哪一行坏了，错误里不出现原始对话内容（隐私断言），**以非零退出码硬
//!   失败**，原文件一字未动。
//!
//! ## 语义修正记录（独测抓到的真 bug 2，`SessionStore::load` 三态化后改断言）
//!
//! 这个文件第二个测试原来断言的是"中部损坏 → `main.rs` 拿到 `None`、当成
//! 没有可恢复的会话、开一个全新会话、进程退出码是 0"——那不是验收原文要的
//! "启动报错"，是当时 `SessionStore::load()` 返回 `Option<LoadedSession>` 的
//! 真 bug：中部损坏和"文件不存在/从没写过"被同一个 `None` 压缩成了一件事，
//! main.rs 没法区分，只能都当成"开新会话"处理。风险不是"警告打了就没事"——
//! 一旦这个新会话后续触发一次快照（`SessionStore::snapshot` 是截断语义），
//! 用户原本还能人工修复的损坏文件就被覆盖，真的没了。
//!
//! 修法：`SessionStore::load()` 改成三态 `LoadOutcome`（`agent_store::persist::
//! LoadOutcome`：`Absent`/`Refused{reason}`/`Loaded`），`Jsonl::load()` 的
//! `CorruptLine` 分支现在翻成 `Refused`（不是 `Absent`），
//! `agent_runtime::persist::recover` 把 `Refused` 翻成 `RecoverError::Refused`，
//! `main.rs` 已有的 `Err(e) => fail(...)` 出口自动接住——不用改 main.rs 的分支
//! 逻辑，类型层面就不再允许把"拒绝加载"悄悄坍缩成"没有会话"。第二个测试因此
//! 改成断言新语义（非零退出码 + 原文件字节不变），跟第三个测试
//! （`UnknownLabel`，语法合法但标签不认识）现在是同一种硬失败形状——两条本来
//! 就该走同一个出口，"出口统一"是这次修复的一部分。

use crate::indep_support;
use crate::indep_support::{CliProcess, FakeServer, Scratch, Script, sse};
use std::path::PathBuf;
use std::time::Duration;

const T: Duration = Duration::from_secs(10);
const SECRET: &str = "SECRET_MARKER_XYZ_do_not_leak";

/// 干净地跑一轮真实对话并 `/quit`，产出一份完整、已终结（Status=Done）的
/// session 文件。用户输入里带一个独一无二的标记，方便后面断言“坏文件的
/// 错误信息里绝不出现原始对话内容”。
fn build_clean_session(scratch: &indep_support::Scratch, file_name: &str) -> PathBuf {
    let server = FakeServer::start(vec![Script::Immediate(sse::text_reply(
        "clean reply marker",
    ))]);
    let providers = scratch.write_providers_toml(&server.base_url());
    let session = scratch.path(file_name);
    let mut cli = CliProcess::spawn(&providers, Some(&session));
    assert!(
        cli.wait_for("输入一句话开始对话", T),
        "构造干净会话时启动横幅超时：{}",
        cli.combined_output()
    );
    cli.send_line(SECRET);
    assert!(
        cli.wait_for("[本轮完成]", T),
        "构造干净会话时那一轮没完成：{}",
        cli.combined_output()
    );
    cli.send_line("/quit");
    assert!(cli.wait_exit(T).is_some(), "构造干净会话时该干净退出");
    session
}

#[test]
fn truncated_tail_line_is_tolerated_and_the_session_stays_usable() {
    let scratch = Scratch::new("corrupt-tail");
    let clean = build_clean_session(&scratch, "clean.jsonl");

    // 手工模拟“进程在写最后一行的中途被杀”：在一份完整、已终结的会话文件
    // 末尾拼接一段不完整、不带换行结尾的 JSON 片段。
    let mut bytes = std::fs::read(&clean).expect("read clean session");
    bytes.extend_from_slice(br#"{"kind":"entry","seq":99,"met"#);
    let truncated = scratch.path("truncated.jsonl");
    std::fs::write(&truncated, &bytes).expect("write truncated session");

    let server = FakeServer::start(vec![Script::Immediate(sse::text_reply(
        "post truncation reply",
    ))]);
    let providers = scratch.write_providers_toml(&server.base_url());
    let mut cli = CliProcess::spawn(&providers, Some(&truncated));

    assert!(
        cli.wait_for("不完整的尾行", T),
        "该有尾部截断的容忍提示：{}",
        cli.combined_output()
    );
    assert!(
        cli.wait_for("[会话已恢复]", T),
        "截断尾行之后仍要能正常恢复：{}",
        cli.combined_output()
    );

    cli.send_line("continue after truncation");
    assert!(
        cli.wait_for("[本轮完成]", T),
        "容忍截断之后该能继续对话：{}",
        cli.combined_output()
    );

    cli.send_line("/quit");
    assert!(
        cli.wait_exit(T).is_some(),
        "容忍截断之后进程该能正常退出，不是卡死"
    );
}

#[test]
fn a_broken_middle_line_is_reported_with_its_line_number_and_never_leaks_conversation_content() {
    let scratch = Scratch::new("corrupt-middle");
    let clean = build_clean_session(&scratch, "clean.jsonl");

    // 破坏第一行的 JSON 语法（它本来就是装着上面那句带 SECRET 标记的用户
    // 输入的 entry）——011 的契约是"错误只报行号/类别，绝不转发解析到一半
    // 的内容"。
    let text = std::fs::read_to_string(&clean).expect("read clean session as text");
    let mut lines: Vec<&str> = text.lines().collect();
    let broken_first = format!("{}###BROKEN###{}", &lines[0][..15], &lines[0][15..]);
    lines[0] = &broken_first;
    let corrupted = scratch.path("corrupted.jsonl");
    std::fs::write(&corrupted, lines.join("\n")).expect("write corrupted session");
    let before = std::fs::read(&corrupted).expect("read corrupted session bytes before spawning");

    // 加载阶段就该报错拒绝，不会真的发起网络请求，随便指一个不会有人听的端口。
    let providers = scratch.write_providers_toml("http://127.0.0.1:1");
    let mut cli = CliProcess::spawn(&providers, Some(&corrupted));

    // 三态化之后（bug 2 修法）：中部损坏是 `LoadOutcome::Refused`，
    // `agent_runtime::persist::recover` 翻成 `RecoverError::Refused`，main.rs
    // 走它已有的硬失败出口——进程该在进入 REPL 之前就以非零退出码退出，不用
    // （也不能）再发 `/quit`，那时进程往往已经死了。
    let status = cli.wait_exit(T);
    let out = cli.combined_output();

    assert!(out.contains("第 1 行"), "错误该指出具体行号：{out}");
    assert!(
        out.contains("损坏") || out.contains("拒绝加载"),
        "该有清楚的损坏说明：{out}"
    );
    assert!(!out.contains(SECRET), "错误里绝不能带出原始对话内容：{out}");
    assert!(
        !out.contains("clean reply marker"),
        "错误里绝不能带出原始对话内容：{out}"
    );
    assert!(!out.contains("panicked"), "损坏文件不该让进程 panic：{out}");
    assert!(
        !out.contains("输入一句话开始对话"),
        "该在进入 REPL 之前就硬失败，不该看到启动横幅：{out}"
    );

    match status {
        Some(exit) => assert!(
            !exit.success(),
            "中部损坏该以非零退出码硬失败，实际：{exit:?}\n{out}"
        ),
        None => panic!("进程该已经因为硬失败退出，而不是卡住等待输入：{out}"),
    }

    // 硬失败的意义就在这——继续跑会在下一张快照把原文件覆盖，所以这里必须
    // 停在"连一个字节都还没动"的状态，让人有机会先备份再决定怎么办。
    let after = std::fs::read(&corrupted).expect("read corrupted session bytes after exit");
    assert_eq!(
        before, after,
        "硬失败必须原样保留损坏文件，一个字节都不能动"
    );
}

#[test]
fn a_semantically_unknown_label_is_a_hard_failure_that_still_never_leaks_content() {
    let scratch = Scratch::new("corrupt-label");
    let clean = build_clean_session(&scratch, "clean.jsonl");

    // 语法合法、但标签字符串不在编译期已知集合里——这才是真正让进程以非零
    // 退出码失败、且完全不进入 REPL 的那一类（main.rs 的 RecoverError 分支），
    // 跟上一个测试的 JSON 语法损坏是两条不同的路径。
    let text = std::fs::read_to_string(&clean).expect("read clean session as text");
    let replaced = text.replacen("\"user_input\"", "\"bogus_unknown_label_xyz\"", 1);
    assert_ne!(replaced, text, "替换该确实生效，不然这个测试没测到东西");
    let corrupted = scratch.path("unknown_label.jsonl");
    std::fs::write(&corrupted, replaced).expect("write unknown-label session");

    let providers = scratch.write_providers_toml("http://127.0.0.1:1");
    let mut cli = CliProcess::spawn(&providers, Some(&corrupted));

    let status = cli.wait_exit(T);
    let out = cli.combined_output();
    assert!(!out.contains(SECRET), "错误里绝不能带出原始对话内容：{out}");
    assert!(
        !out.contains("panicked"),
        "不认识的标签不该让进程 panic：{out}"
    );
    assert!(
        out.contains("bogus_unknown_label_xyz"),
        "该说明具体是哪个标签不认识：{out}"
    );
    match status {
        Some(exit) => assert!(
            !exit.success(),
            "语义不认识的标签该以非零退出码硬失败，实际：{exit:?}\n{out}"
        ),
        None => panic!("进程该已经因为硬失败退出，而不是卡住等待输入：{out}"),
    }
}
