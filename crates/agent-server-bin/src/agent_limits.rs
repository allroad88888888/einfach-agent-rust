//! 子 agent 两道结构性硬限（决策 20）的配置边界：命令行 → 环境变量 → 默认档。
//!
//! 决策 32 把配置面定在**进程级**：`--max-agent-depth` / `--max-children`
//! （`AGENT_MAX_AGENT_DEPTH` / `AGENT_MAX_CHILDREN` 兜底，命令行优先），协议面
//! 一个字节不改。取值经 [`crate::run`] 喂进 `ToolTableSpec::Full { spawn_limits }`，
//! 之后全链自动通——`ToolTableSpec::spawn_limits()` 读口负责把「写进工具描述给
//! 模型看的那份」和「`Session::spawn_child` 真正拦人的那份」对齐（034 做的），
//! 恢复路径由 `recover` 的 `limits` 入参接上（160）。
//!
//! **取值非法就拒绝启动**，不静默退默认档——跟 `agent_server::bind` 对
//! `AGENT_BIND` 的取向同款（见 [`parse_count`] 的文档），不是 `--port` 那种
//! 静默退 `None`。严格边界的形状照 [`crate::remote_tool_timeout`]：
//! `Result<_, String>` 交给 `run::run` 用 `fail` 统一变成非零退出。
//!
//! # 为什么 [`resolve`] 不读环境变量，[`from_cli_and_environment`] 才读
//!
//! 照抄 `agent_server::bind` 的模块文档那条理由：`std::env::set_var` 在 2024
//! edition 是 `unsafe fn`，而 `cargo test` 多线程并发跑测试函数——测试里改一次
//! 进程级环境变量，同一个二进制里别的测试就可能读到被改过的值。把「给定两个
//! 可选的原始字符串，算出 `AgentLimits`」写成纯函数，坏配置的行为就能并发安全
//! 地测到；真读 `std::env::var` 的那层只有几行，两头都简单。

use agent_core::AgentLimits;

use crate::cli::Cli;

const DEPTH_FLAG: &str = "--max-agent-depth";
const DEPTH_ENV: &str = "AGENT_MAX_AGENT_DEPTH";
const CHILDREN_FLAG: &str = "--max-children";
const CHILDREN_ENV: &str = "AGENT_MAX_CHILDREN";
const AUTO_TURNS_FLAG: &str = "--max-auto-turns";
const AUTO_TURNS_ENV: &str = "AGENT_MAX_AUTO_TURNS";

/// 一个上限参数的取值：正整数，**解析不出来或小于 1 就是错误**。
///
/// # 取严，跟的是 `AGENT_BIND` 那个先例不是 `--port`
///
/// 本仓这两种取向都有先例，差别在**有没有下游替它报错**：
///
/// - `--port` 解析失败退 `None`（`cli.rs` 的 `unparseable_port_is_silently_none_not_a_panic`
///   记着这层取舍）——因为下游 `default_bind_addr` 到真要用的时候会报「配置错了」，
///   解析层不必再有一套错误文案。
/// - `AGENT_BIND` 配成非法 IP 直接硬失败（`agent_server::bind::BindConfigError`
///   的文档：「用户显式设了这个变量就是想覆盖默认值，把打错的字符串当成没设，
///   是那种配置错了却看起来在正常运行的坑」）。
///
/// **上限没有下游**：静默退回默认档之后进程照常起、会话照常跑，只是闸是 8 而不是
/// 运维配的那个数——用户以为配了其实没配。所以它归后一类，决策 32 据此取严。
///
/// # 下限为什么是 1 而不是 0
///
/// `max_children = 0` 意味着 `srv:agent/spawn` 留在工具表里、描述写着「最多 0 个」、
/// 调一次拒一次——工具在表里却结构性不可用是个坏形状。要关掉它有 076 的
/// `capabilities.disable_builtin` 这条现成的路，错误文案直接把人指过去。
/// 上限不设死限：多大算贵是部署方自己的账（决策 20 说的成本兜底）。
pub fn parse_count(flag: &str, value: Option<&str>) -> Result<usize, String> {
    let Some(value) = value else {
        return Err(format!("{flag} 需要一个 ≥ 1 的整数"));
    };
    value
        .parse::<usize>()
        .ok()
        .filter(|n| *n >= 1)
        .ok_or_else(|| {
            format!(
                "{flag} 需要 ≥ 1 的整数，收到 {value:?}；\
                 要关掉子 agent 请用 capabilities.disable_builtin: [\"srv:agent/spawn\"]"
            )
        })
}

/// 纯函数：命令行 → 环境变量原始值 → 默认档，**逐项独立**。**不读环境变量**，
/// 理由见模块文档。
///
/// 「逐项」是刻意的：只给 `--max-children` 时深度必须留在默认的 3，而不是整个
/// `AgentLimits` 二选一。两道闸各管各的，一个被配了不代表另一个也该被动。
pub fn resolve(
    cli: &Cli,
    depth_env: Option<&str>,
    children_env: Option<&str>,
    auto_turns_env: Option<&str>,
) -> Result<AgentLimits, String> {
    let fallback = AgentLimits::default();
    Ok(AgentLimits {
        max_depth: pick(
            DEPTH_FLAG,
            DEPTH_ENV,
            cli.max_agent_depth,
            depth_env,
            fallback.max_depth,
        )?,
        max_children: pick(
            CHILDREN_FLAG,
            CHILDREN_ENV,
            cli.max_children,
            children_env,
            fallback.max_children,
        )?,
        max_auto_turns: pick_auto_turns(
            AUTO_TURNS_FLAG,
            AUTO_TURNS_ENV,
            cli.max_auto_turns,
            auto_turns_env,
            fallback.max_auto_turns,
        )?,
    })
}

/// 真正读环境变量的那一层——只有这几行，`run::run` 调的是它。
pub fn from_cli_and_environment(cli: &Cli) -> Result<AgentLimits, String> {
    let depth = std::env::var(DEPTH_ENV).ok();
    let children = std::env::var(CHILDREN_ENV).ok();
    let auto_turns = std::env::var(AUTO_TURNS_ENV).ok();
    resolve(
        cli,
        depth.as_deref(),
        children.as_deref(),
        auto_turns.as_deref(),
    )
}

/// 自驱动预算的取值：**允许 0**（= 关掉自驱动），跟另外两道闸的下限 1 刻意不同。
///
/// 那两道量的是「树能长多大」，0 会造出一个连一个子 agent 都开不了的荒唐配置，
/// 所以钉 1；这一道量的是「没人看着时还能自己跑几轮」，**0 是一个完全正当的
/// 部署选择**——不想让会话在没人看着时花钱，就配 0。
pub fn parse_auto_turns(source: &str, value: Option<&str>) -> Result<u32, String> {
    let Some(value) = value else {
        return Err(format!("{source} 需要一个 ≥ 0 的整数"));
    };
    value
        .parse::<u32>()
        .map_err(|_| format!("{source} 需要 ≥ 0 的整数，收到 {value:?}"))
}

/// 同 [`pick`]，值域不同（允许 0）。
fn pick_auto_turns(
    flag: &str,
    env: &str,
    from_cli: Option<u32>,
    from_env: Option<&str>,
    fallback: u32,
) -> Result<u32, String> {
    if let Some(n) = from_cli {
        return Ok(n);
    }
    let Some(raw) = from_env else {
        return Ok(fallback);
    };
    parse_auto_turns(env, Some(raw)).map_err(|e| format!("{e}（对应命令行参数 {flag}）"))
}

/// 命令行给了就用它（`cli.rs` 解析时已经过 [`parse_count`]），否则看环境变量的
/// 原始值，都没有就是默认档。环境变量走**同一条**校验——错值从哪来都是错值。
fn pick(
    flag: &str,
    env: &str,
    from_cli: Option<usize>,
    from_env: Option<&str>,
    fallback: usize,
) -> Result<usize, String> {
    if let Some(n) = from_cli {
        return Ok(n);
    }
    let Some(raw) = from_env else {
        return Ok(fallback);
    };
    // 环境变量报错时把对应的 flag 也说出来，运维不用翻文档找对应关系。
    parse_count(env, Some(raw)).map_err(|e| format!("{e}（对应命令行参数 {flag}）"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(depth: Option<usize>, children: Option<usize>) -> Cli {
        Cli {
            config: None,
            sessions_dir: None,
            port: None,
            ready_file: None,
            private_capability_stdin: false,
            max_agent_depth: depth,
            max_children: children,
            max_auto_turns: None,
        }
    }

    #[test]
    fn positive_integers_are_accepted() {
        assert_eq!(parse_count("--max-children", Some("2")).unwrap(), 2);
        assert_eq!(parse_count("--max-children", Some("64")).unwrap(), 64);
    }

    /// 决策 32 的取严条款：非数字、0、负数、缺值全部是错误，不是「退默认档」。
    #[test]
    fn unparseable_or_zero_values_are_rejected_not_silently_defaulted() {
        assert!(parse_count("--max-children", Some("abc")).is_err());
        assert!(parse_count("--max-children", Some("0")).is_err());
        assert!(parse_count("--max-children", Some("-1")).is_err());
        assert!(parse_count("--max-children", None).is_err());
    }

    /// `0` 的错误文案得把人指向真正该走的那条路，否则运维只会把它改成 1 了事。
    #[test]
    fn rejecting_zero_points_at_disable_builtin() {
        let err = parse_count("--max-children", Some("0")).unwrap_err();
        assert!(err.contains("disable_builtin"), "{err}");
        assert!(err.contains("srv:agent/spawn"), "{err}");
    }

    #[test]
    fn no_flags_and_no_env_is_the_decision_20_default() {
        assert_eq!(
            resolve(&cli(None, None), None, None, None).unwrap(),
            AgentLimits::default()
        );
    }

    /// **部分覆盖不连坐**：只配一项，另一项必须留在默认值上。
    #[test]
    fn setting_one_limit_leaves_the_other_at_its_default() {
        let only_children = resolve(&cli(None, Some(2)), None, None, None).unwrap();
        assert_eq!(only_children.max_children, 2);
        assert_eq!(only_children.max_depth, AgentLimits::default().max_depth);

        let only_depth = resolve(&cli(Some(1), None), None, None, None).unwrap();
        assert_eq!(only_depth.max_depth, 1);
        assert_eq!(only_depth.max_children, AgentLimits::default().max_children);
    }

    #[test]
    fn both_limits_come_through() {
        let both = resolve(&cli(Some(2), Some(3)), None, None, None).unwrap();
        assert_eq!(both.max_depth, 2);
        assert_eq!(both.max_children, 3);
    }

    #[test]
    fn the_environment_fills_in_what_the_command_line_left_out() {
        let got = resolve(&cli(None, None), Some("2"), Some("5"), None).unwrap();
        assert_eq!(got.max_depth, 2);
        assert_eq!(got.max_children, 5);
    }

    /// 命令行优先于环境变量——跟 `--port`/`AGENT_SERVER_PORT` 同一个既有取舍。
    #[test]
    fn the_command_line_wins_over_the_environment() {
        let got = resolve(&cli(Some(1), Some(2)), Some("9"), Some("9"), None).unwrap();
        assert_eq!(got.max_depth, 1);
        assert_eq!(got.max_children, 2);
    }

    /// 环境变量里的错值和命令行里的错值一样是硬失败，且错误文案要同时点出
    /// 环境变量名和对应的 flag。
    #[test]
    fn a_bad_environment_value_is_rejected_and_names_both_spellings() {
        let err = resolve(&cli(None, None), None, Some("0"), None).unwrap_err();
        assert!(err.contains(CHILDREN_ENV), "{err}");
        assert!(err.contains(CHILDREN_FLAG), "{err}");

        assert!(resolve(&cli(None, None), Some("abc"), None, None).is_err());
    }
}
