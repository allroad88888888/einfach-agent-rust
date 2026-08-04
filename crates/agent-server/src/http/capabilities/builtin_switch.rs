//! `capabilities.disable_builtin`：这个会话**不启用**哪些内置工具（076）。
//!
//! 两件事，都只在建会话那一次发生：
//!
//! 1. [`check_builtin_switch`]——名字必须在**这个部署实际装配出来的那张表**里，
//!    不认识的一律 **400 且点名**；
//! 2. [`disabled_builtins`]——翻成纯 `agent_core` 数据（`Vec<Arc<str>>`），跟
//!    [`host_tools`](super::assemble::host_tools) 一样只翻译、不排序（排序去重是
//!    `agent_core::value::str_set` 的事，红线 11 该在最靠近落盘/prompt 的那一层结账）。
//!
//! # 为什么这条校验必须在这一跳报（069 §拍板的判据）
//!
//! 拼错一个名字（`srv:agent/spawnn`）如果被**静默忽略**，客户端会以为关掉了、其实
//! 没关——模型照样调得到 `srv:shell/exec`，**没有任何报错**。症状离现场十万八千里，
//! 正是本仓最贵的那一类。而这一刻客户端还在线、能改，所以该在这里失败。
//!
//! 对比 064 的 `skill_injection` 过滤：那一段**每轮都跑**，作者早就不在场了，所以
//! 那里绝不能报错。同一条判据，两个相反的结论——差别只在「报给谁、他还在不在」。
//!
//! # 天花板 = `ToolTableSpec` 的五档，**不含**注入进来的东西
//!
//! 判据是 `spec.build()` 出来的那张表（部署方在 `SessionTemplate` 里定的档），不是
//! 装配完的最终表：
//!
//! - **宿主注入的 `web:`/`desk:` 工具不在天花板里**——那是宿主自己这一次报进来的，
//!   不想给就别报，不需要第二个开关；
//! - **`srv:skill/activate` / `deactivate` 也不在**——它们只在宿主声明了 skill 时才
//!   出现（`registry` 非空才接 `.with_skills(..)`），同样已经完全由宿主自己决定。
//!   给同一件事配两个开关，只会造出「同一个名字在两次请求里一次合法一次 400」这种
//!   说不清的面。
//!
//! 也就是说：**这个开关只减部署方给的那批**——那正是宿主今天唯一控制不了的那批
//! （`docs/HOST-CAPABILITIES.md` §三之二）。

use std::fmt;
use std::sync::Arc;

use crate::registry::ToolTableSpec;

use super::Capabilities;
use super::validate::elide;

/// 关闭列表里出现了这个部署根本没装配的名字。
///
/// **点名**是这条错误的全部价值：只说「有个名字不认识」等于让调用方自己去猜是哪一个，
/// 而它一次可以关五个。附上这个部署实际有的那张名单（服务端自己的数据，不是把请求体
/// 弹回去），调用方一眼看得出是拼错了还是这一档本来就没有。
#[derive(Clone, PartialEq, Eq, Debug)]
pub(in crate::http) struct UnknownBuiltin {
    name: String,
    available: Vec<String>,
}

impl fmt::Display for UnknownBuiltin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "capabilities.disable_builtin 里的 \"{}\" 不是这个部署装配出来的内置工具——只能关掉已经有的，不能凭空开启。\
             这个部署实际有的是：{}",
            self.name,
            self.available.join("、")
        )
    }
}

/// 校验关闭列表：每个名字都必须在 `deployed` 这一档装配出来的表里。
///
/// 第一个不认识的名字就返回——错误是给人看的，一次说清一项即可（同 061 的
/// [`validate`](super::validate::validate)）。
pub(in crate::http) fn check_builtin_switch(capabilities: &Capabilities, deployed: ToolTableSpec) -> Result<(), UnknownBuiltin> {
    if capabilities.disable_builtin.is_empty() {
        return Ok(());
    }
    let table = deployed.build();
    for name in &capabilities.disable_builtin {
        if !table.declares(name) {
            return Err(UnknownBuiltin {
                name: elide(name),
                available: table.specs().iter().map(|spec| spec.name.to_string()).collect(),
            });
        }
    }
    Ok(())
}

/// 把 `capabilities.disable_builtin` 翻成装配那一侧要的 `Vec<Arc<str>>`。
///
/// 没带 `capabilities`（老调用方）或者列表为空 → 空 `Vec`，下游一路空操作，工具表
/// 与 076 之前**逐字节相同**。
pub(in crate::http) fn disabled_builtins(capabilities: Option<&Capabilities>) -> Vec<Arc<str>> {
    let Some(capabilities) = capabilities else { return Vec::new() };
    capabilities.disable_builtin.iter().map(|name| Arc::from(name.as_str())).collect()
}

#[cfg(test)]
mod tests {
    use agent_core::AgentLimits;
    use serde_json::json;

    use super::*;

    fn caps(value: serde_json::Value) -> Capabilities {
        serde_json::from_value(value).expect("该解析成功")
    }

    fn full() -> ToolTableSpec {
        ToolTableSpec::Full { spawn_limits: AgentLimits::default() }
    }

    /// 装配出来的名字全过——这一档真有的那几件都关得掉。
    #[test]
    fn a_name_this_deployment_really_has_is_accepted() {
        let switch = caps(json!({ "disable_builtin": ["srv:agent/spawn", "srv:shell/exec"] }));
        assert_eq!(check_builtin_switch(&switch, full()), Ok(()));
    }

    /// **本 issue 最贵的那条**：拼错一个名字 → 400 且点名。
    ///
    /// 静默忽略的话，客户端以为关掉了、其实没关，模型照样调得到 `srv:shell/exec`，
    /// 而且没有任何报错。
    #[test]
    fn a_typo_is_named_not_silently_ignored() {
        let switch = caps(json!({ "disable_builtin": ["srv:agent/spawnn"] }));
        let rejection = check_builtin_switch(&switch, full()).expect_err("拼错的名字该被拒");
        let message = rejection.to_string();
        assert!(message.contains("srv:agent/spawnn"), "报文必须点名是哪一个：{message}");
        assert!(message.contains("srv:agent/spawn"), "报文该附上这个部署实际有的那张名单：{message}");
    }

    /// **天花板是「这个部署」的表，不是「所有部署的并集」**：`srv:agent/spawn` 是
    /// 一个完全合法的内置工具名，但 `Builtin` 这一档没装配它 → 照样 400。
    ///
    /// 这条是上一条的正对照：只测拼错的名字的话，一个「凡是没见过的字符串就拒」的
    /// 实现同样会绿；这里给的名字在别的档下完全合法，判据必须是**这一份**表。
    #[test]
    fn a_tool_another_tier_has_is_still_rejected_here() {
        let switch = caps(json!({ "disable_builtin": ["srv:agent/spawn"] }));
        assert_eq!(check_builtin_switch(&switch, full()), Ok(()), "夹具前提：Full 这一档真有 spawn");
        assert!(
            check_builtin_switch(&switch, ToolTableSpec::Builtin).is_err(),
            "Builtin 这一档没装配 spawn，关它就是关一个不存在的东西——静默放过等于让客户端以为关掉了"
        );
    }

    /// 宿主自己注入的工具**不在天花板里**：那是它自己这一次报进来的，不想给就别报。
    #[test]
    fn an_injected_name_is_not_part_of_the_ceiling() {
        let switch = caps(json!({
            "tools": [ { "name": "web:crm/lookup" } ],
            "disable_builtin": ["web:crm/lookup"]
        }));
        assert!(check_builtin_switch(&switch, full()).is_err(), "注入的工具不该能被这个开关关掉——它本来就该由宿主自己决定报不报");
    }

    /// 不带这个字段、以及空数组：什么都不做（老调用方一个字都不用改）。
    #[test]
    fn no_switch_means_nothing_to_check_and_nothing_to_disable() {
        assert_eq!(check_builtin_switch(&caps(json!({})), full()), Ok(()));
        assert_eq!(check_builtin_switch(&caps(json!({ "disable_builtin": [] })), full()), Ok(()));
        assert!(disabled_builtins(None).is_empty());
        assert!(disabled_builtins(Some(&caps(json!({})))).is_empty());
    }

    /// 翻译原样搬（顺序不动——排序去重在 `agent_core::value::str_set`，红线 11 该在
    /// 最靠近落盘的那一层结账，不在这里各写一遍）。
    #[test]
    fn the_switch_is_carried_over_as_is() {
        let switch = caps(json!({ "disable_builtin": ["srv:shell/exec", "srv:agent/spawn"] }));
        let names: Vec<String> = disabled_builtins(Some(&switch)).iter().map(|n| n.to_string()).collect();
        assert_eq!(names, vec!["srv:shell/exec", "srv:agent/spawn"]);
    }
}
