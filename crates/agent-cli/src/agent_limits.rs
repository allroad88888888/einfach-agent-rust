//! 子 agent 两道结构性硬限（决策 20）在 CLI 侧怎么定：`--max-agent-depth` /
//! `--max-children`，环境变量 `AGENT_MAX_AGENT_DEPTH` / `AGENT_MAX_CHILDREN`
//! 兜底，命令行优先。决策 32 的 CLI 半边，跟 `agent-server-bin` 那份同款。
//!
//! **收 `&[String]` 而不是自己读 `std::env::args()`**——同
//! [`crate::session_path::resolve`]：测试要能喂一份夹具参数，不必真的用不同的
//! 进程命令行启动。环境变量那一层同理拆开（见下面 [`resolve`] 与
//! [`from_args_and_environment`] 的分工）。
//!
//! # 为什么值错了是硬失败
//!
//! 跟 `agent_server::bind` 对 `AGENT_BIND` 的取向一样，不是 `--port` 那种静默退
//! `None`：`--port` 有下游（`default_bind_addr` 会替它报「配置错了」），**上限
//! 没有**——静默退回默认档之后一切照常跑，只是闸是 8 而不是你配的那个数，
//! 用户以为配了其实没配。决策 32 据此取严；下限钉 1（要关掉子 agent 是另一件事，
//! 不是把上限设成 0）。

use std::sync::Arc;

use agent_core::AgentLimits;

const DEPTH_FLAG: &str = "--max-agent-depth";
const DEPTH_ENV: &str = "AGENT_MAX_AGENT_DEPTH";
const CHILDREN_FLAG: &str = "--max-children";
const CHILDREN_ENV: &str = "AGENT_MAX_CHILDREN";

/// 一个上限的取值：正整数，解析不出来或 `< 1` 就是错误。
fn parse_count(source: &str, value: Option<&str>) -> Result<usize, String> {
    let Some(value) = value else {
        return Err(format!("{source} 需要一个 ≥ 1 的整数"));
    };
    value
        .parse::<usize>()
        .ok()
        .filter(|n| *n >= 1)
        .ok_or_else(|| format!("{source} 需要 ≥ 1 的整数，收到 {value:?}"))
}

/// 从参数列表里找 `--flag <n>` / `--flag=<n>`，形状照
/// [`crate::session_path::resolve`]。**没找到返回 `Ok(None)`**（交给调用方退
/// 环境变量），找到了但值不合法返回 `Err`。
fn from_args(args: &[String], flag: &str) -> Result<Option<usize>, String> {
    let with_equals = format!("{flag}=");
    for (i, arg) in args.iter().enumerate() {
        if arg == flag {
            return parse_count(flag, args.get(i + 1).map(String::as_str)).map(Some);
        }
        if let Some(v) = arg.strip_prefix(&with_equals) {
            return parse_count(flag, Some(v)).map(Some);
        }
    }
    Ok(None)
}

/// 纯函数：命令行 → 环境变量原始值 → 默认档，**逐项独立**（只配一项时另一项
/// 留在决策 20 的默认值上）。**不读环境变量**，理由同
/// `agent_server::bind` 的模块文档：`std::env::set_var` 在 2024 edition 是
/// `unsafe fn`，而 `cargo test` 多线程并发跑，测试里改进程级环境变量会串味。
pub fn resolve(
    args: &[String],
    depth_env: Option<&str>,
    children_env: Option<&str>,
) -> Result<AgentLimits, String> {
    let fallback = AgentLimits::default();
    Ok(AgentLimits {
        max_depth: pick(args, DEPTH_FLAG, DEPTH_ENV, depth_env, fallback.max_depth)?,
        max_children: pick(
            args,
            CHILDREN_FLAG,
            CHILDREN_ENV,
            children_env,
            fallback.max_children,
        )?,
    })
}

/// 真正读环境变量的那一层——`main` 调的是它。
pub fn from_args_and_environment(args: &[String]) -> Result<AgentLimits, String> {
    let depth = std::env::var(DEPTH_ENV).ok();
    let children = std::env::var(CHILDREN_ENV).ok();
    resolve(args, depth.as_deref(), children.as_deref())
}

fn pick(
    args: &[String],
    flag: &str,
    env: &str,
    from_env: Option<&str>,
    fallback: usize,
) -> Result<usize, String> {
    if let Some(n) = from_args(args, flag)? {
        return Ok(n);
    }
    let Some(raw) = from_env else {
        return Ok(fallback);
    };
    parse_count(env, Some(raw)).map_err(|e| format!("{e}（对应命令行参数 {flag}）"))
}

/// 启动横幅里那一行：非默认值要显眼，默认值也说一声，省得「我配了吗」这种疑问
/// 只能靠翻代码回答。
pub fn banner(limits: AgentLimits) -> Arc<str> {
    let d = AgentLimits::default();
    if limits == d {
        Arc::from(format!(
            "深度≤{} 子数≤{}（默认）",
            limits.max_depth, limits.max_children
        ))
    } else {
        Arc::from(format!(
            "深度≤{} 子数≤{}（默认 {}/{}）",
            limits.max_depth, limits.max_children, d.max_depth, d.max_children
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn both_spellings_are_recognized() {
        let got = resolve(
            &args(&["agent-cli", "--max-agent-depth", "2", "--max-children=3"]),
            None,
            None,
        )
        .unwrap();
        assert_eq!(got.max_depth, 2);
        assert_eq!(got.max_children, 3);
    }

    #[test]
    fn no_flags_and_no_env_is_the_decision_20_default() {
        assert_eq!(
            resolve(&args(&["agent-cli"]), None, None).unwrap(),
            AgentLimits::default()
        );
    }

    /// **部分覆盖不连坐**——只配子数，深度得留在 3。
    #[test]
    fn setting_one_limit_leaves_the_other_at_its_default() {
        let got = resolve(&args(&["agent-cli", "--max-children", "2"]), None, None).unwrap();
        assert_eq!(got.max_children, 2);
        assert_eq!(got.max_depth, AgentLimits::default().max_depth);
    }

    /// 决策 32 取严：非数字 / 0 / 缺值全是硬失败，不是「退默认档」。
    #[test]
    fn bad_values_are_rejected_not_silently_defaulted() {
        assert!(resolve(&args(&["agent-cli", "--max-children", "abc"]), None, None).is_err());
        assert!(resolve(&args(&["agent-cli", "--max-children=0"]), None, None).is_err());
        assert!(resolve(&args(&["agent-cli", "--max-children"]), None, None).is_err());
    }

    #[test]
    fn the_environment_fills_in_and_the_command_line_wins() {
        let from_env = resolve(&args(&["agent-cli"]), Some("2"), Some("5")).unwrap();
        assert_eq!((from_env.max_depth, from_env.max_children), (2, 5));

        let cli_wins = resolve(&args(&["agent-cli", "--max-children=1"]), None, Some("9")).unwrap();
        assert_eq!(cli_wins.max_children, 1);
    }

    #[test]
    fn a_bad_environment_value_names_both_spellings() {
        let err = resolve(&args(&["agent-cli"]), None, Some("0")).unwrap_err();
        assert!(err.contains(CHILDREN_ENV), "{err}");
        assert!(err.contains(CHILDREN_FLAG), "{err}");
    }

    /// 横幅：默认档标「默认」，非默认档把默认值一并说出来做对照。
    #[test]
    fn the_banner_distinguishes_configured_from_default() {
        assert!(banner(AgentLimits::default()).contains("默认"));
        let tuned = banner(AgentLimits {
            max_depth: 2,
            max_children: 2,
        });
        assert!(tuned.contains("深度≤2"), "{tuned}");
        assert!(tuned.contains("默认 3/8"), "{tuned}");
    }
}
