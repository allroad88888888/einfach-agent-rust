//! provider 名字 → 具体 adapter 的分发表。**唯一的出口**——`main.rs` 启动时
//! 选初始 provider、`model_switch.rs` 处理 `/model <name>` 时都调这一份
//! [`build_provider`]，不各自维护一张容易分叉的名字表。
//!
//! 这条 `match` 写在 `agent-cli` 是合法的：红线 12 只约束 `agent-core`/
//! `agent-store`，`scripts/check-invariants.sh` 的 `check_no_model_branch`
//! 也只查那两个 crate（见该函数开头的 `case` 分支）。

use agent_providers::Provider;
use agent_providers::deepseek::DeepSeek;
use agent_providers::glm::Glm;
use agent_providers::kimi::Kimi;
use agent_providers::openai::OpenAiCompat;

/// 一段 `[providers.<name>]` 该用哪套编解码：**段内 `adapter` 优先，缺省回落段名**。
///
/// 177：段名是「这个端点叫什么」，`adapter` 是「用哪套编解码」。既有配置一个字
/// 不用改（没写 `adapter` 就还是按段名分发）；想接一个通用 OpenAI 兼容端点就写
/// `adapter = "openai"`，段名随便叫——**否则想同时配 Ollama 和 OpenRouter
/// 就只能有一个段叫 `openai`，第二个没处放**。
pub fn adapter_name<'a>(section: &'a str, cfg: &'a agent_transport::ProviderConfig) -> &'a str {
    cfg.adapter.as_deref().unwrap_or(section)
}

/// `[default] provider` 或 `/model <name>` 里的名字 → 具体 adapter。未知名字
/// 报错列出可选值，不是运行到一半才发现 endpoint 和编码对不上。
pub fn build_provider(name: &str) -> Result<Box<dyn Provider>, String> {
    match name {
        "deepseek" => Ok(Box::new(DeepSeek)),
        "kimi" => Ok(Box::new(Kimi)),
        "glm" => Ok(Box::new(Glm)),
        // 175：通用 OpenAI 兼容。**它不是第四家 provider，是「所有还没被单独
        // 适配的家」的兜底**——所以名字不该出现在 providers.toml 的段名里，
        // 而是由段里的 `adapter = "openai"` 指过来（段名随便叫 ollama/openrouter/…）。
        "openai" => Ok(Box::new(OpenAiCompat)),
        other => Err(format!(
            "\"{other}\" 没有对应的 adapter。可选：deepseek / kimi / glm / openai（通用 OpenAI 兼容）。检查拼写，或 providers.toml 里 [providers.*] 的段名 / 段内的 adapter 字段"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_transport::ProviderConfig;

    fn cfg(toml_body: &str) -> ProviderConfig {
        toml::from_str(toml_body).unwrap()
    }

    /// 三家：段里没有 `adapter`，回落段名——既有配置一个字不用改。
    #[test]
    fn without_an_adapter_field_the_section_name_decides() {
        let c = cfg(r#"api_key="k"
base_url="https://api.deepseek.com"
model="deepseek-v4-pro""#);
        assert_eq!(adapter_name("deepseek", &c), "deepseek");
        assert!(build_provider(adapter_name("deepseek", &c)).is_ok());
    }

    /// 177 的全部意义：段名随便叫，编解码由 `adapter` 指定。
    #[test]
    fn an_explicit_adapter_wins_over_the_section_name() {
        let c = cfg(r#"adapter="openai"
api_key="ollama"
base_url="http://localhost:11434/v1"
model="qwen3:4b""#);
        // 段名叫 ollama —— 分发表里根本没有这个名字，靠 adapter 指过去。
        assert_eq!(adapter_name("ollama", &c), "openai");
        assert!(build_provider(adapter_name("ollama", &c)).is_ok());
        assert!(
            build_provider("ollama").is_err(),
            "段名本身不该是一个合法 adapter 名——否则这个字段就白加了"
        );
    }

    /// 四个合法 adapter 名，一个都不能少。
    #[test]
    fn all_known_adapters_resolve() {
        for name in ["deepseek", "kimi", "glm", "openai"] {
            assert!(build_provider(name).is_ok(), "{name} 该能解析");
        }
    }

    /// 未知名字报错，且错误文案要提到 `adapter` 字段——配错的人第一反应是
    /// 去看段名，得告诉他还有第二个地方可能写错。
    #[test]
    fn an_unknown_name_errors_and_mentions_both_places() {
        // `Box<dyn Provider>` 没有 Debug，不能 unwrap_err()——手工解出错误分支。
        let Err(e) = build_provider("gpt4") else {
            panic!("未知名字必须报错");
        };
        assert!(e.contains("openai"), "该列出可选值：{e}");
        assert!(e.contains("adapter"), "该提示 adapter 字段：{e}");
    }
}
