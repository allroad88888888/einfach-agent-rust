use super::*;

/// 编的假 key，不是真凭据——仓规矩：测试夹具永远不许放真 key。
const FAKE_HOST_KEY: &str = "sk-fake-host-injected-not-a-real-key-000111";

/// 红线之外的仓内硬规矩：`Debug` 输出绝不含 key 本身，只报长度。
/// `config_tests.rs` 已经对 toml 解析出的 `ProviderConfig` 证过这条；这里对
/// `from_host` 构造出来的实例再证一遍——新增的构造入口没有绕开手写 `Debug`
/// 的第二条路。
#[test]
fn debug_output_never_contains_the_key_for_host_constructed_config() {
    let cfg = ProviderConfig::from_host(
        "https://api.deepseek.com".to_string(),
        "deepseek-v4-pro".to_string(),
        FAKE_HOST_KEY.to_string(),
    );
    let dump = format!("{cfg:?}");
    assert!(
        !dump.contains(FAKE_HOST_KEY),
        "Debug 输出泄漏了 key: {dump}"
    );
    assert!(
        dump.contains(&FAKE_HOST_KEY.len().to_string()),
        "该报长度: {dump}"
    );

    let root = RootConfig::from_host(
        "deepseek".to_string(),
        BTreeMap::from([("deepseek".to_string(), cfg)]),
    );
    let root_dump = format!("{root:?}");
    assert!(!root_dump.contains(FAKE_HOST_KEY));
}

/// `from_host` 构造出来的实例，`resolve_key`/`endpoint`/`key_len` 跟字段直接
/// 反序列化出来的实例行为完全一致——没有为宿主注入另开一套语义。
#[test]
fn from_host_matches_field_semantics_of_a_deserialized_config() {
    let host_cfg = ProviderConfig::from_host(
        "https://api.deepseek.com/".to_string(),
        "deepseek-v4-pro".to_string(),
        FAKE_HOST_KEY.to_string(),
    );
    assert_eq!(host_cfg.resolve_key().as_deref(), Some(FAKE_HOST_KEY));
    assert_eq!(host_cfg.key_len(), FAKE_HOST_KEY.len());
    assert_eq!(
        host_cfg.endpoint(),
        "https://api.deepseek.com/chat/completions"
    );
}

/// **本 issue 的主证据**：宿主注入的配置与「解析出来的配置」产出同一个类型，
/// 走同一条装配路径。
///
/// native 侧走 `toml::from_str::<RootConfig>`（`config.rs`，那部分不在这个
/// crate 的 wasm32 目标里编译，所以这里不能直接调 `toml`）；但 wasm 宿主真正
/// 会用的形状是跨 wasm-bindgen 边界传一份 JSON，用两边都已经依赖的
/// `serde_json::from_str::<RootConfig>` 解出——同一个 `#[derive(Deserialize)]`
/// 类型，两个目标都编。这里用它模拟「解析出的配置」，跟 `ProviderConfig::
/// from_host` 手工构造的「宿主直接给结构」对比：两者必须在
/// `default_provider()` 这同一个自由函数上给出等价的 `endpoint`/`resolve_key`/
/// `model`，证明两条来源真的汇到了同一条装配路径，不是各走各的。
#[test]
fn host_injected_and_deserialized_configs_converge_on_the_same_assembly_path() {
    let parsed_json = format!(
        r#"{{
            "providers": {{
                "deepseek": {{
                    "api_key": "{FAKE_HOST_KEY}",
                    "base_url": "https://api.deepseek.com",
                    "model": "deepseek-v4-pro"
                }}
            }},
            "default": {{ "provider": "deepseek" }}
        }}"#
    );
    let parsed: RootConfig = serde_json::from_str(&parsed_json).unwrap();

    let injected = RootConfig::from_host(
        "deepseek".to_string(),
        BTreeMap::from([(
            "deepseek".to_string(),
            ProviderConfig::from_host(
                "https://api.deepseek.com".to_string(),
                "deepseek-v4-pro".to_string(),
                FAKE_HOST_KEY.to_string(),
            ),
        )]),
    );

    // 同一个自由函数 `default_provider`，两份不同来源的 `RootConfig` 都吃得下。
    let parsed_provider = default_provider(&parsed).unwrap();
    let injected_provider = default_provider(&injected).unwrap();

    assert_eq!(parsed_provider.endpoint(), injected_provider.endpoint());
    assert_eq!(parsed_provider.model, injected_provider.model);
    assert_eq!(
        parsed_provider.resolve_key(),
        injected_provider.resolve_key()
    );
}

#[test]
fn root_config_from_host_leaves_execution_profiles_empty_for_backward_compat() {
    let root = RootConfig::from_host(
        "deepseek".to_string(),
        BTreeMap::from([(
            "deepseek".to_string(),
            ProviderConfig::from_host(
                "https://api.deepseek.com".to_string(),
                "deepseek-v4-pro".to_string(),
                FAKE_HOST_KEY.to_string(),
            ),
        )]),
    );
    assert!(root.execution_profiles.is_empty());
    assert!(root.execution_profile("vision").unwrap().is_none());
}

/// 177：`adapter` 字段缺省 ⇒ 既有配置一个字不用改。
///
/// 这条钉的是**向后兼容**：三家的段里都没有 `adapter`，反序列化必须给 `None`，
/// 调用方才能回落到段名。少了 `#[serde(default)]` 这里会直接解析失败。
#[test]
fn a_config_without_an_adapter_field_still_parses_and_leaves_it_none() {
    let cfg: ProviderConfig = toml::from_str(
        r#"
            api_key = "sk-whatever"
            base_url = "https://api.deepseek.com"
            model = "deepseek-v4-pro"
        "#,
    )
    .expect("没有 adapter 字段的旧配置必须照常解析");
    assert_eq!(cfg.adapter, None);
}

/// 177：写了 `adapter` 就按它走——**段名与编解码解耦**。
///
/// 这是这个字段存在的全部理由：想同时配 Ollama 和 OpenRouter（都走通用
/// OpenAI 兼容编解码），没有这个字段就只能有一个段叫 `openai`，第二个没处放。
#[test]
fn an_explicit_adapter_decouples_the_section_name_from_the_codec() {
    let ollama: ProviderConfig = toml::from_str(
        r#"
            adapter = "openai"
            api_key = "ollama"
            base_url = "http://localhost:11434/v1"
            model = "qwen3:4b"
        "#,
    )
    .unwrap();
    assert_eq!(ollama.adapter.as_deref(), Some("openai"));

    let openrouter: ProviderConfig = toml::from_str(
        r#"
            adapter = "openai"
            api_key = "sk-or-whatever"
            base_url = "https://openrouter.ai/api/v1"
            model = "anthropic/claude-sonnet-5"
        "#,
    )
    .unwrap();
    assert_eq!(openrouter.adapter.as_deref(), Some("openai"));

    // 两个段名不同、编解码相同——这正是没有这个字段时表达不出来的东西。
    assert_ne!(ollama.base_url, openrouter.base_url);
}

/// 177：`endpoint()` **不补 `/v1`**，`base_url` 填什么就是什么。
///
/// 174 实测：`/v1` 不是通用约定——GLM 的兼容端点是
/// `/api/paas/v4/chat/completions`，硬加 `/v1` 整组 404。
#[test]
fn endpoint_never_invents_a_version_segment() {
    let glm: ProviderConfig = toml::from_str(
        r#"
            api_key = "k"
            base_url = "https://open.bigmodel.cn/api/paas/v4"
            model = "glm-5.2"
        "#,
    )
    .unwrap();
    assert_eq!(
        glm.endpoint(),
        "https://open.bigmodel.cn/api/paas/v4/chat/completions"
    );

    let ollama: ProviderConfig = toml::from_str(
        r#"
            api_key = "k"
            base_url = "http://localhost:11434/v1/"
            model = "qwen3:4b"
        "#,
    )
    .unwrap();
    // 尾斜杠不该产生双斜杠。
    assert_eq!(ollama.endpoint(), "http://localhost:11434/v1/chat/completions");
}
