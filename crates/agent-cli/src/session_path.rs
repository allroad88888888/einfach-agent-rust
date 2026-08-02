//! 会话文件路径怎么定（027）：`--session <path>` 命令行参数，或者
//! `AGENT_SESSION_PATH` 环境变量，两者都没有就是 `None`——`main.rs` 据此选
//! `Jsonl`（有路径）还是 `Memory`（临时会话，进程退出即丢）。
//!
//! 命令行优先于环境变量：跟 `agent-transport::config` 的
//! `AGENT_PROVIDERS_CONFIG` 是同一类「有环境变量兜底，但显式参数说了算」的
//! 取舍，只是那边没有对应的命令行参数可比。

use std::path::PathBuf;

/// 从传入的参数列表里解析 `--session <path>` / `--session=<path>`，解析不到
/// 就退回 `AGENT_SESSION_PATH` 环境变量。**收参数而不是自己读
/// `std::env::args()`**：测试要能喂一份夹具参数，不必真的用不同的进程命令行
/// 启动。
pub fn resolve(args: &[String]) -> Option<PathBuf> {
    for (i, arg) in args.iter().enumerate() {
        if arg == "--session" {
            return args.get(i + 1).map(PathBuf::from);
        }
        if let Some(v) = arg.strip_prefix("--session=") {
            return Some(PathBuf::from(v));
        }
    }
    std::env::var("AGENT_SESSION_PATH").ok().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn two_token_form_is_recognized() {
        assert_eq!(resolve(&args(&["agent-cli", "--session", "/tmp/x.jsonl"])), Some(PathBuf::from("/tmp/x.jsonl")));
    }

    #[test]
    fn equals_form_is_recognized() {
        assert_eq!(resolve(&args(&["agent-cli", "--session=/tmp/y.jsonl"])), Some(PathBuf::from("/tmp/y.jsonl")));
    }

    #[test]
    fn neither_flag_nor_env_var_is_none() {
        // 依赖测试环境没有设 AGENT_SESSION_PATH——CI/本地跑这条测试的账户
        // 都不该设这个变量，跟 `agent-transport::config` 测试同一个假设。
        assert_eq!(std::env::var("AGENT_SESSION_PATH").ok(), None, "测试环境不该预设这个变量");
        assert_eq!(resolve(&args(&["agent-cli"])), None);
    }
}
