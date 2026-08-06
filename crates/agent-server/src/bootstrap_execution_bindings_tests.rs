use std::sync::Arc;

use agent_transport::Client;

use super::*;

fn root(source: &str) -> config::RootConfig {
    toml::from_str(source).expect("测试配置必须合法")
}

#[test]
fn startup_resolves_named_execution_bindings() {
    let root = root(
        r#"
[providers.deepseek]
api_key = "test-key"
base_url = "https://api.deepseek.com"
model = "deepseek-chat"

[default]
provider = "deepseek"

[execution_profiles]
vision = "deepseek"
"#,
    );

    let bindings = resolve_execution_bindings(&root, &Arc::new(Client::new()), None)
        .expect("已配置的 profile 必须在启动期解析");

    assert!(bindings.contains_key(&ExecutionProfileId::new("vision")));
}

#[test]
fn startup_rejects_execution_profile_pointing_to_unknown_provider() {
    let root = root(
        r#"
[providers.deepseek]
api_key = "test-key"
base_url = "https://api.deepseek.com"
model = "deepseek-chat"

[default]
provider = "deepseek"

[execution_profiles]
vision = "missing"
"#,
    );

    let error = match resolve_execution_bindings(&root, &Arc::new(Client::new()), None) {
        Ok(_) => panic!("未知 profile provider 必须拒绝启动"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        BootstrapError::Config(config::ConfigError::UnknownExecutionProfileProvider(name))
            if name == "missing"
    ));
}
