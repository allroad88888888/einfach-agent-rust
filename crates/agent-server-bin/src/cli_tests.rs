//! [`super::cli`] 的解析单测：四个老 flag 的两种写法、`--help` 短路、未知参数
//! 报错，以及 161 加的两个上限 flag（含它们与 `--port` **取向不同**那条对照）。
//! 拆自 `cli.rs`（161 加完顶破 300 行，红线 9），跟 `agent-core` 的
//! `spawn.rs`/`spawn_tests.rs` 同一个拆分手法。

use super::*;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn run_or_panic(parsed: ParsedArgs) -> Cli {
    match parsed {
        ParsedArgs::Run(cli) => cli,
        ParsedArgs::Help => panic!("期望 Run，拿到 Help"),
        ParsedArgs::Invalid(message) => panic!("期望 Run，参数错误：{message}"),
    }
}

#[test]
fn no_flags_is_all_none() {
    let cli = run_or_panic(parse(&args(&["agent-server"])));
    assert!(cli.config.is_none());
    assert!(cli.sessions_dir.is_none());
    assert!(cli.port.is_none());
    assert!(cli.ready_file.is_none());
    assert!(!cli.private_capability_stdin);
}

#[test]
fn two_token_form_is_recognized_for_all_flags() {
    let cli = run_or_panic(parse(&args(&[
        "agent-server",
        "--config",
        "/tmp/providers.toml",
        "--sessions-dir",
        "/tmp/sessions",
        "--port",
        "8080",
        "--ready-file",
        "/tmp/agent-server.ready",
        "--private-capability-stdin",
    ])));
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/providers.toml")));
    assert_eq!(cli.sessions_dir, Some(PathBuf::from("/tmp/sessions")));
    assert_eq!(cli.port, Some(8080));
    assert_eq!(
        cli.ready_file,
        Some(PathBuf::from("/tmp/agent-server.ready"))
    );
    assert!(cli.private_capability_stdin);
}

#[test]
fn equals_form_is_recognized() {
    let cli = run_or_panic(parse(&args(&[
        "agent-server",
        "--config=/x.toml",
        "--sessions-dir=/y",
        "--port=9",
        "--ready-file=/z/agent.ready",
    ])));
    assert_eq!(cli.config, Some(PathBuf::from("/x.toml")));
    assert_eq!(cli.sessions_dir, Some(PathBuf::from("/y")));
    assert_eq!(cli.port, Some(9));
    assert_eq!(cli.ready_file, Some(PathBuf::from("/z/agent.ready")));
}

#[test]
fn help_flag_short_circuits_before_anything_else() {
    assert!(matches!(
        parse(&args(&["agent-server", "--help"])),
        ParsedArgs::Help
    ));
    assert!(matches!(
        parse(&args(&["agent-server", "-h"])),
        ParsedArgs::Help
    ));
    // --help 出现在别的 flag 之后也一样识别——不要求它必须在最前面。
    assert!(matches!(
        parse(&args(&["agent-server", "--port", "1", "--help"])),
        ParsedArgs::Help
    ));
}

#[test]
fn unknown_options_fail_instead_of_being_ignored() {
    assert!(matches!(
        parse(&args(&["agent-server", "--misspelled-option"])),
        ParsedArgs::Invalid(message) if message.contains("--misspelled-option")
    ));
}

#[test]
fn unparseable_port_is_silently_none_not_a_panic() {
    // 跟 `agent_cli::session_path` 同一个取向：命令行解析层不做验证性
    // 报错，交给下游（这里是 `default_bind_addr`）在真正需要这个值时
    // 报出「配置错了」——避免解析器和使用者各有一套错误文案。
    let cli = run_or_panic(parse(&args(&["agent-server", "--port", "not-a-number"])));
    assert_eq!(cli.port, None);
}

// ---- 161：两个上限 flag ----

#[test]
fn both_spellings_of_the_limit_flags_are_recognized() {
    let cli = run_or_panic(parse(&args(&[
        "agent-server",
        "--max-agent-depth",
        "2",
        "--max-children=3",
    ])));
    assert_eq!(cli.max_agent_depth, Some(2));
    assert_eq!(cli.max_children, Some(3));
}

/// 部分覆盖不连坐：只给一个，另一个留 `None`（真正落默认档在
/// `agent_limits::resolve`，这一层只管「命令行上给没给」）。
#[test]
fn giving_one_limit_leaves_the_other_unset() {
    let cli = run_or_panic(parse(&args(&["agent-server", "--max-children", "2"])));
    assert_eq!(cli.max_children, Some(2));
    assert_eq!(cli.max_agent_depth, None);
}

/// 决策 32 的取严条款，也是这两个 flag 与 `--port` **行为不同**的地方：
/// 上限解析不出来是 `Invalid`（拒绝启动），不是静默 `None`。
/// 理由（有没有下游替它报错）在 `agent_limits::parse_count` 的文档。
#[test]
fn an_unparseable_limit_is_rejected_unlike_port() {
    assert!(matches!(
        parse(&args(&["agent-server", "--max-children", "abc"])),
        ParsedArgs::Invalid(m) if m.contains("--max-children")
    ));
    // 对照：同一份参数里 `--port` 仍然是静默 None，两条取向并存且各有理由。
    let cli = run_or_panic(parse(&args(&["agent-server", "--port", "not-a-number"])));
    assert_eq!(cli.port, None);
}

/// `0` 被拒，且文案把人指向真正该走的那条路（`disable_builtin`）。
#[test]
fn zero_is_rejected_and_points_at_disable_builtin() {
    assert!(matches!(
        parse(&args(&["agent-server", "--max-agent-depth=0"])),
        ParsedArgs::Invalid(m) if m.contains("disable_builtin")
    ));
}

/// flag 在末尾、值缺失 → 报错，不是悄悄当成没给。
#[test]
fn a_limit_flag_without_a_value_is_rejected() {
    assert!(matches!(
        parse(&args(&["agent-server", "--max-children"])),
        ParsedArgs::Invalid(_)
    ));
}

/// `--help` 里必须提到这两个 flag 和它们的环境变量——035 把「`--help` 可读」
/// 列进了验收，新参数不写进去等于没有。
#[test]
fn help_documents_the_limit_flags_and_their_env_vars() {
    for needle in [
        "--max-agent-depth",
        "--max-children",
        "AGENT_MAX_AGENT_DEPTH",
        "AGENT_MAX_CHILDREN",
    ] {
        assert!(HELP.contains(needle), "--help 该提到 {needle}");
    }
}
