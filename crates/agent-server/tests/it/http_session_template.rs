//! `SessionTemplate` 在开会话时分配持久化路径与注入作用域。

use std::path::PathBuf;
use std::sync::Arc;

use agent_core::{HostSkill, Reversibility, ToolSpec};
use agent_providers::deepseek::DeepSeek;
use agent_server::{SessionId, SessionTemplate, ToolTableSpec};
use agent_transport::Client;

fn minimal_template(tools_root: PathBuf, default_sessions_dir: Option<PathBuf>) -> SessionTemplate {
    SessionTemplate {
        provider: Arc::new(DeepSeek),
        endpoint: "http://127.0.0.1:1/unused".to_string(),
        api_key: "fake-key".to_string(),
        model: Arc::from("deepseek-v4-pro"),
        tools: ToolTableSpec::Builtin,
        tools_root,
        system: Vec::new(),
        client: Arc::new(Client::new()),
        history_cap: None,
        snapshot_every: None,
        provider_timeout: None,
        remote_tool_timeout: None,
        default_sessions_dir,
        upload_dir: None,
        vision: None,
    }
}

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "agent-server-open-spec-test-{name}-{}",
        std::process::id()
    ))
}

#[test]
fn no_default_dir_and_no_explicit_path_stays_memory() {
    let template = minimal_template(temp_dir("tools-a"), None);
    let spec = template
        .open_spec(
            SessionId::from("s-1"),
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
    assert_eq!(
        spec.store_path, None,
        "没有 default_sessions_dir，也没有显式 session_path，该还是 Memory"
    );
}

#[test]
fn explicit_session_path_wins_over_default_dir() {
    let default_dir = temp_dir("tools-b-default");
    let explicit = temp_dir("tools-b-explicit").join("custom.jsonl");
    let template = minimal_template(temp_dir("tools-b"), Some(default_dir));
    let spec = template
        .open_spec(
            SessionId::from("s-2"),
            Some(explicit.clone()),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
    assert_eq!(
        spec.store_path,
        Some(explicit),
        "客户端显式给的 session_path 该赢"
    );
}

#[test]
fn missing_session_path_auto_assigns_under_default_dir() {
    let dir = temp_dir("tools-c-sessions");
    let template = minimal_template(temp_dir("tools-c"), Some(dir.clone()));
    let spec = template
        .open_spec(
            SessionId::from("s-3"),
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
    assert_eq!(
        spec.store_path,
        Some(dir.join("s-3.jsonl")),
        "该自动分配 <dir>/<id>.jsonl"
    );
    assert!(
        dir.is_dir(),
        "default_sessions_dir 该被现造出来，不能指望 Jsonl 的 IO 线程默默失败"
    );
}

#[test]
fn injected_tools_ride_this_one_spec_and_never_stick_to_the_template() {
    let template = minimal_template(temp_dir("tools-d"), None);
    let injected = vec![(
        ToolSpec {
            name: Arc::from("web:crm/lookup"),
            description: Arc::from("查 CRM 档案"),
            schema: Arc::new(serde_json::json!({ "type": "object" })),
        },
        Reversibility::Pure,
    )];

    let declared = template
        .open_spec(
            SessionId::from("s-4"),
            None,
            injected,
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
    assert_eq!(declared.host_tools.len(), 1);
    assert_eq!(&*declared.host_tools[0].0.name, "web:crm/lookup");
    assert_eq!(declared.host_tools[0].1, Reversibility::Pure);

    let plain = template
        .open_spec(
            SessionId::from("s-5"),
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
    assert!(
        plain.host_tools.is_empty(),
        "同一个 template 的下一个会话不该看见上一个的声明"
    );
}

#[test]
fn injected_skills_ride_this_one_spec_and_never_stick_to_the_template() {
    let template = minimal_template(temp_dir("tools-e"), None);
    let injected = vec![HostSkill {
        id: agent_core::SkillId::new("crm-flow"),
        description: Arc::from("处理客户工单"),
        body: Arc::from("第一步……"),
        tools: Vec::new(),
        tool_reversibility: Default::default(),
    }];

    let declared = template
        .open_spec(
            SessionId::from("s-6"),
            None,
            Vec::new(),
            injected,
            Vec::new(),
        )
        .unwrap();
    assert_eq!(declared.host_skills.len(), 1);
    assert_eq!(declared.host_skills[0].id.as_str(), "crm-flow");
    assert_eq!(
        declared.system.len(),
        template.system.len(),
        "template 自己的 system 段不该被这次声明改动"
    );

    let plain = template
        .open_spec(
            SessionId::from("s-7"),
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
    assert!(
        plain.host_skills.is_empty(),
        "同一个 template 的下一个会话不该看见上一个声明的 skill"
    );
}
