use super::*;
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};

/// 临时文件，`Drop` 时自己删——不引入 `tempfile` 依赖，标准库够用。
struct TempFile(PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn write_temp(contents: &str) -> (TempFile, PathBuf) {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "agent-transport-test-{}-{n}.toml",
        std::process::id()
    ));
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(contents.as_bytes()).unwrap();
    (TempFile(path.clone()), path)
}

const FAKE_KEY: &str = "sk-super-secret-do-not-print-12345";

fn sample_toml() -> String {
    format!(
        r#"
[providers.deepseek]
api_key = "{FAKE_KEY}"
base_url = "https://api.deepseek.com"
beta_base_url = "https://api.deepseek.com/beta"
model = "deepseek-v4-pro"

[providers.kimi]
api_key_env = "MOONSHOT_API_KEY"
base_url = "https://api.moonshot.cn/v1"
model = "kimi-k3"

[default]
provider = "deepseek"
"#
    )
}

fn profile_toml(provider: &str) -> String {
    format!(
        "{}\n[execution_profiles]\nvision = \"{provider}\"\n",
        sample_toml()
    )
}

/// 红线之外的仓内硬规矩：`Debug` 输出绝不含 key 本身，只报长度。
#[test]
fn debug_output_never_contains_the_key() {
    let root: RootConfig = toml::from_str(&profile_toml("kimi")).unwrap();
    let ds = &root.providers["deepseek"];
    let dump = format!("{ds:?}");
    assert!(!dump.contains(FAKE_KEY), "Debug 输出泄漏了 key: {dump}");
    assert!(
        dump.contains(&FAKE_KEY.len().to_string()),
        "该报长度: {dump}"
    );
    let root_dump = format!("{root:?}");
    assert!(!root_dump.contains(FAKE_KEY));
}

#[test]
fn old_config_without_execution_profiles_stays_compatible() {
    let root: RootConfig = toml::from_str(&sample_toml()).unwrap();
    assert!(root.execution_profiles.is_empty());
    assert!(root.execution_profile("vision").unwrap().is_none());
}

#[test]
fn execution_profile_resolves_provider_and_config() {
    let root: RootConfig = toml::from_str(&profile_toml("kimi")).unwrap();
    let (provider_name, provider) = root.execution_profile("vision").unwrap().unwrap();
    assert_eq!(provider_name, "kimi");
    assert_eq!(provider.model, "kimi-k3");
    assert!(root.execution_profile("other").unwrap().is_none());
}

#[test]
fn execution_profile_errors_when_provider_is_missing() {
    let root: RootConfig = toml::from_str(&profile_toml("missing-provider")).unwrap();
    let error = root.execution_profile("vision").unwrap_err();
    assert!(
        matches!(error, ConfigError::UnknownExecutionProfileProvider(name) if name == "missing-provider")
    );
}

#[test]
fn resolve_key_prefers_env_over_inline() {
    let root: RootConfig = toml::from_str(&sample_toml()).unwrap();
    assert_eq!(
        root.providers["deepseek"].resolve_key().as_deref(),
        Some(FAKE_KEY)
    );
    let kimi = &root.providers["kimi"];
    unsafe {
        std::env::remove_var("MOONSHOT_API_KEY");
    }
    assert_eq!(kimi.resolve_key(), None);
    assert_eq!(kimi.key_len(), 0);
    unsafe {
        std::env::set_var("MOONSHOT_API_KEY", "env-value-xyz");
    }
    assert_eq!(kimi.resolve_key().as_deref(), Some("env-value-xyz"));
    unsafe {
        std::env::remove_var("MOONSHOT_API_KEY");
    }
}

#[test]
fn default_provider_resolves_the_configured_name() {
    let root: RootConfig = toml::from_str(&sample_toml()).unwrap();
    assert_eq!(default_provider(&root).unwrap().model, "deepseek-v4-pro");
}

#[test]
fn default_provider_errors_on_unknown_name() {
    let mut root: RootConfig = toml::from_str(&sample_toml()).unwrap();
    root.default.provider = "not-there".to_string();
    assert!(
        matches!(default_provider(&root), Err(ConfigError::UnknownDefault(name)) if name == "not-there")
    );
}

#[test]
fn endpoint_appends_chat_completions_and_trims_trailing_slash() {
    let root: RootConfig = toml::from_str(&sample_toml()).unwrap();
    assert_eq!(
        root.providers["deepseek"].endpoint(),
        "https://api.deepseek.com/chat/completions"
    );
}

#[test]
fn load_from_picks_first_existing_candidate() {
    let (_guard, path) = write_temp(&sample_toml());
    let missing = PathBuf::from("/definitely/does/not/exist/providers.toml");
    assert_eq!(
        load_from(&[missing, path]).unwrap().default.provider,
        "deepseek"
    );
}

#[test]
fn load_from_reports_all_tried_paths_when_none_exist() {
    let missing = PathBuf::from("/definitely/does/not/exist/providers.toml");
    match load_from(std::slice::from_ref(&missing)).unwrap_err() {
        ConfigError::NotFound { tried } => assert_eq!(tried, vec![missing]),
        other => panic!("期望 NotFound，拿到 {other:?}"),
    }
}

#[test]
fn parse_error_does_not_echo_inline_api_key_or_source_line() {
    let broken = format!(
        "[providers.deepseek]\napi_key = \"{FAKE_KEY}\"\nbase_url = 42\nmodel = \"deepseek-v4-pro\"\n[default]\nprovider = \"deepseek\""
    );
    let (_guard, path) = write_temp(&broken);
    let error = load_from(&[path]).unwrap_err();
    let display = error.to_string();
    let debug = format!("{error:?}");

    assert!(!display.contains(FAKE_KEY), "Display 泄漏了 key: {display}");
    assert!(!debug.contains(FAKE_KEY), "Debug 泄漏了 key: {debug}");
    assert!(!display.contains("base_url = 42"));
}
