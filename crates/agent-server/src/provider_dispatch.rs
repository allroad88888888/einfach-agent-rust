//! provider 名字 → 具体 adapter 的分发表，照抄 `agent_cli::provider::
//! build_provider` 的手法（issue 030 原文：「Provider/transport 的构造照
//! agent-cli 的 main 手法——配置驱动三家分发，那段 match 在宿主侧合法，红线 12
//! 只辖 core/store」）。不复用 `agent-cli` 那份代码：`agent-cli` 是二进制导向
//! 的宿主 crate，让库 crate `agent-server` 依赖它方向不对（`agent-cli` 未来
//! 应该反过来能选择内嵌 `agent-server`，见 ARCHITECTURE.md §包结构），十几行
//! 的 `match` 复制一份不构成「大杂烩」。
//!
//! [`crate::registry::SessionRegistry::open`] 不强制走这张表——`OpenSpec.
//! provider` 直接收 `Arc<dyn Provider>`，production 代码调这个函数拿到它，
//! 测试可以喂一个自造的假 `Provider`（比如 panic 测试要用到的那种）。

use std::sync::Arc;

use agent_providers::Provider;
use agent_providers::deepseek::DeepSeek;
use agent_providers::glm::Glm;
use agent_providers::kimi::Kimi;

/// `name` → 具体 adapter。未知名字报错列出可选值——跟 `agent_cli::provider::
/// build_provider` 是同一份判据，字面错误信息也保持一致，方便运维一眼认出
/// 这是同一类配置错误。
pub fn resolve_provider(name: &str) -> Result<Arc<dyn Provider>, String> {
    match name {
        "deepseek" => Ok(Arc::new(DeepSeek)),
        "kimi" => Ok(Arc::new(Kimi)),
        "glm" => Ok(Arc::new(Glm)),
        other => Err(format!(
            "\"{other}\" 没有对应的 adapter。可选：deepseek / kimi / glm（检查拼写，或 providers.toml 里 [providers.*] 的段名）"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_names_resolve() {
        assert!(resolve_provider("deepseek").is_ok());
        assert!(resolve_provider("kimi").is_ok());
        assert!(resolve_provider("glm").is_ok());
    }

    #[test]
    fn unknown_name_lists_the_choices() {
        // `.err().unwrap()`，不是 `.unwrap_err()`：`Arc<dyn Provider>` 没有
        // `Debug`（trait object 不体面地要求所有实现者都能格式化自己），
        // `unwrap_err` 的 bound 挂在 `Ok` 类型上，`.err()` 不需要。
        let err = resolve_provider("bogus").err().unwrap();
        assert!(
            err.contains("deepseek") && err.contains("kimi") && err.contains("glm"),
            "{err}"
        );
    }
}
