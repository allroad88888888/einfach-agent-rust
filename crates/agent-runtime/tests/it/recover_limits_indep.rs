//! 独立测试（issue 160）：`agent_runtime::recover` 把宿主给的 `AgentLimits`
//! 真的转发到恢复出来的会话，**而不是**在路上退回默认档。
//!
//! 只依据 160 的「验收」一节 + 公开 API 写成，不看实现体。姊妹白盒在
//! `agent-core/src/command/restore_tests.rs`（那份钉 `Session::restore` 自己收不收
//! 这个参数），这份钉的是**落盘 → 新进程 → recover** 这条真链上它没被丢掉。
//!
//! 为什么值得单独走一趟真文件：`limits` 不进原子图也不进日志，它跨进程活下来的
//! 唯一凭据就是「宿主再说一遍」这条入参通道。通道断了不报错——恢复出来的会话
//! 照常能跑，只是闸悄悄退回 8，而工具描述里还写着部署方配的那个数。160 之前它
//! 就是断的（`restore` 硬写 `default()`，`recover` 连入参都没有），今天配置值恒
//! 等于默认值才没显形。
//!
//! 落盘/重启手法照抄 `jsonl_restart_after_compaction_command.rs`。

use agent_core::{AgentId, AgentLimits, ChildConfig, Session, SpawnRefused, TurnStatus};

use crate::support;
use crate::support::ScriptedResponse;

/// 宿主配的一组**非默认**上限：默认是深度 ≤3、子数 ≤8，这里两个都压到 2，
/// 于是「恢复后退回默认档」这件事一断言就看得见。
fn tight() -> AgentLimits {
    AgentLimits {
        max_depth: 2,
        max_children: 2,
        ..AgentLimits::default()
    }
}

fn plain_turn(text: &'static str) -> ScriptedResponse {
    ScriptedResponse::Sse(vec![
        Box::leak(
            format!(
                r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":"{text}"}},"finish_reason":null}}]}}"#
            )
            .into_boxed_str(),
        ),
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":5,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50}}"#,
        "data: [DONE]",
    ])
}

/// 写一个真会话到 `session.jsonl`（跑一轮 + sync），返回会话文件路径。
fn write_a_session(tag: &str) -> std::path::PathBuf {
    let dir = support::temp_dir(tag);
    let session_path = dir.join("session.jsonl");
    let port = support::spawn_scripted_server(vec![plain_turn("落盘用的一句回答")]);

    {
        let mut ctx = build_ctx_with_jsonl(port, &dir, &session_path);
        let mut session = Session::new(AgentId::root());
        let status = agent_runtime::run_turn(&mut session, &mut ctx, "第一句话")
            .expect("first turn should not be a source failure");
        assert_eq!(status, TurnStatus::Done { truncated: false });
        agent_runtime::persist::sync(&mut ctx, &mut session);
        // ctx（连同 Jsonl）在这里 drop，队列排干、真的落盘。
    }
    session_path
}

fn recover_with(session_path: &std::path::Path, limits: AgentLimits) -> Session {
    let backend = agent_runtime::open_backend(Some(session_path.to_path_buf()), |e| {
        panic!("不该有加载错误：{e}")
    });
    agent_runtime::recover(
        backend.as_ref(),
        AgentId::root(),
        agent_core::DEFAULT_HISTORY_CAP,
        limits,
        &mut |k| panic!("不该有不认识的键：{k:?}"),
    )
    .expect("recover 不该失败")
    .expect("写过一轮，该恢复出 Some")
}

/// 验收主条：落盘 → 新进程 → `recover` 传非默认上限 → 读出来就是那一组。
///
/// **160 之前这条必红**：`Session::restore` 把 `limits` 硬写成
/// `AgentLimits::default()`，宿主给什么都到不了。
#[test]
fn recover_carries_the_limits_the_host_passed_in() {
    let session_path = write_a_session("recover-limits-carries");

    let recovered = recover_with(&session_path, tight());

    assert_eq!(recovered.agent_limits(), tight());
    assert_ne!(
        recovered.agent_limits(),
        AgentLimits::default(),
        "恢复出来的必须是宿主配的那组，不是默认档——相等就说明这条通道断了"
    );
}

/// 光带回值不算数：恢复出来的会话上，真正拦人的那道闸也得按这组数拦。
///
/// 两侧数字必须是同一组（工具描述那份 + `spawn_child` 那份），这是
/// `ToolTable::with_spawn` 反复记着的耦合；恢复路径不是例外。
#[test]
fn the_gate_on_a_recovered_session_uses_those_limits() {
    let session_path = write_a_session("recover-limits-gate");

    let mut recovered = recover_with(&session_path, tight());
    let root = AgentId::root();

    recovered
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("第 1 个子该成功");
    recovered
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("第 2 个子该成功");

    // 默认档是 8。恢复路径要是把 limits 丢了，这一发会成功——本条就是那个静默
    // 失配的看门狗。
    match recovered.spawn_child(&root, ChildConfig::default(), None) {
        Err(SpawnRefused::TooManyChildren { max, .. }) => assert_eq!(
            max, 2,
            "撞的必须是宿主配的上限；撞到 8 就说明恢复时退回了默认档"
        ),
        other => panic!("第 3 个子该被 TooManyChildren 拒，实际：{other:?}"),
    }
}

/// 回归闸：传默认档时，恢复出来的会话与 160 之前逐字节同义——同一份落盘产物，
/// 消息数、状态、日志长度都不因为这个新参数而变。
#[test]
fn passing_the_default_limits_changes_nothing() {
    let session_path = write_a_session("recover-limits-default");

    let with_default = recover_with(&session_path, AgentLimits::default());
    let with_tight = recover_with(&session_path, tight());

    assert_eq!(with_default.agent_limits(), AgentLimits::default());
    assert_eq!(
        with_default.messages().len(),
        with_tight.messages().len(),
        "上限是配置不是状态：它不该改变恢复出来的消息"
    );
    assert_eq!(with_default.history_len(), with_tight.history_len());
    assert_eq!(with_default.status(), with_tight.status());
}

fn build_ctx_with_jsonl(
    port: u16,
    root: &std::path::Path,
    session_path: &std::path::Path,
) -> agent_runtime::RunnerCtx {
    use agent_core::SessionConfig;
    use agent_providers::deepseek::DeepSeek;
    use agent_runtime::{RunnerCtx, ToolTable};
    use agent_tools::ToolExecutor;
    use agent_transport::Client;
    use std::sync::Arc;

    let client = Client::with_config(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(50),
        agent_transport::Backoff {
            base: std::time::Duration::from_millis(10),
            max_attempts: 1,
        },
    );
    let fs = ToolExecutor::new(root).unwrap();
    RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(client),
        format!("http://127.0.0.1:{port}/chat/completions"),
        "fake-key".to_string(),
        fs,
        ToolTable::builtin(),
        Vec::new(),
        SessionConfig {
            model: Arc::from("deepseek-v4-pro"),
            temperature: None,
            max_tokens: None,
            context_window: None,
        },
        agent_runtime::open_backend(Some(session_path.to_path_buf()), |e| {
            panic!("不该有会话文件错误：{e}")
        }),
        Box::new(|_ev| {}),
    )
}
