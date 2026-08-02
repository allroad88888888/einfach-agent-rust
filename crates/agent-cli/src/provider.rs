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

/// `[default] provider` 或 `/model <name>` 里的名字 → 具体 adapter。未知名字
/// 报错列出可选值，不是运行到一半才发现 endpoint 和编码对不上。
pub fn build_provider(name: &str) -> Result<Box<dyn Provider>, String> {
    match name {
        "deepseek" => Ok(Box::new(DeepSeek)),
        "kimi" => Ok(Box::new(Kimi)),
        "glm" => Ok(Box::new(Glm)),
        other => Err(format!(
            "\"{other}\" 没有对应的 adapter。可选：deepseek / kimi / glm（检查拼写，或 providers.toml 里 [providers.*] 的段名）"
        )),
    }
}
