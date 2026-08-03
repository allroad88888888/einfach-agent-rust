//! 标准命令发现工具的模型输入声明。
//!
//! 发现范围固定为 executor root，输入故意为空，避免模型把路径或任意配置文件扩成读取面。

use agent_core::ToolSpec;
use serde_json::json;
use std::sync::Arc;

pub(crate) fn find_test_lint_commands_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from("find_test_lint_commands"),
        description: Arc::from(
            "只读地检查 root 内受限数量的 Cargo.toml、package.json、pyproject.toml 和\
             go.mod，返回 test/lint 命令候选 argv。不会执行任何命令，也不会把项目\
             script 当作 shell 字符串返回；需要执行时，把某条 argv 交给独立 shell\
             工具并先审阅。origin=declared 表示 manifest 声明了任务名；inferred 表示\
             由已识别的生态配置保守推断。truncated=true 或 warnings 非空时，结果\
             可能不完整，不能据此断言没有验证命令。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })),
    }
}
