//! 宿主建会话时声明的能力（061，接缝见 `docs/HOST-CAPABILITIES.md` §四）：
//! `POST /sessions` 请求体里 `capabilities` 字段的**协议形状**——怎么从 JSON 解
//! 出来、每个字段是什么。这个文件只有形状，三件事各在一个文件里：名字合法不合法在
//! [`validate`]（纯函数），翻成工具表要的料在 [`assemble`]（062），装进哪个会话由
//! `SessionTemplate::open_spec` → `OpenSpec` → `ToolTable::with_host_tools` 那条
//! per-session 的路决定。
//!
//! **形状层刻意零 IO、零解释**：061 只定死两件事——宿主能说什么、什么样的声明会被
//! 当场拒掉；「没说 `reversibility` 算什么」这类解释留给 [`assemble`]。
//!
//! # 076：同一个字段里多了一个**减法**
//!
//! `tools`/`skills` 是加法（宿主报进来的能力），[`Capabilities::disable_builtin`]
//! 是减法（这个会话把部署方给的哪几件内置工具藏起来不给模型看）。它的校验够不着
//! 这一层——「名字在不在这个部署装配出来的表里」要看部署配置，是
//! [`builtin_switch`] 的事；这里只有形状。
//!
//! # 三处形状选择
//!
//! - **`reversibility` 是小写字符串**（`"pure"`/`"reversible"`/`"irreversible"`，
//!   HOST-CAPABILITIES.md §四的原文形状），所以这里有一个自己的
//!   [`CapabilityReversibility`]，不直接复用 `agent_core::Reversibility`——后者的
//!   serde 形状是 PascalCase，而且已经落进了会话 jsonl 和 `ToolCallRequest` 的 TS
//!   类型（`packages/protocol/src/generated/Reversibility.ts`），改它等于改存量数据
//!   的格式。两种拼法之间只隔一个 `From`（见下），宿主面用宿主面的拼法。
//! - **缺字段一律有默认值**（`#[serde(default)]` 用足）——`{}`、`{"tools":[]}`、
//!   少写一个 `description` 都该解析成功，而不是 400。
//! - **连名字也有默认值**（空串）。名字缺了当然要拒，但那一拒该由 [`validate`] 用
//!   一条说得清「哪一项、为什么」的结构化错误来拒；让 serde 去拒只会得到
//!   [`crate::http::json::ApiJson`] 那句通用的「请求体字段形状跟期望的不符」。
//!
//! `ts` feature 打开时整组类型经 [`crate::ts_protocol`] 导出给前端（065 直接用生成
//! 的类型，不手写一份会漂移的镜像）。

mod assemble;
mod builtin_switch;
mod validate;

pub(in crate::http) use assemble::{host_skills, host_tools};
pub(in crate::http) use builtin_switch::{check_builtin_switch, disabled_builtins};
pub(in crate::http) use validate::validate;

use serde::Deserialize;

use agent_core::Reversibility;

/// 宿主这一次建会话交进来的全部能力。三个字段都可缺省：`"capabilities": {}` 合法，
/// 等价于「什么都没声明、什么都没关」。
#[derive(Debug, Default, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub(crate) struct Capabilities {
    // `ts(optional, as = ...)`：wire 上这两个数组是**可以整个不写**的
    // （`#[serde(default)]`），生成的 TS 就该是 `tools?:`——否则前端为了过类型
    // 检查得写一串 `skills: []`，而那正是「协议类型从 Rust 生成」要避免的偏差。
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<Vec<CapabilityTool>>"))]
    pub(crate) tools: Vec<CapabilityTool>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<Vec<CapabilitySkill>>"))]
    pub(crate) skills: Vec<CapabilitySkill>,
    /// **减法**（076）：这个会话**不启用**哪些内置工具。列出来的那些**连名字带
    /// 描述都不进 prompt**，模型压根不知道有它。省略/空数组 = 今天的行为，工具表
    /// 逐字节不变。
    ///
    /// **只能减不能加**：名字必须在这个部署实际装配出来的那张表里
    /// （`ToolTableSpec` 的五档），不认识的名字 → **400 且点名**（校验在
    /// `capabilities::check_builtin_switch`）。部署方装配出来的表是天花板，
    /// 会话只能在它下面挑——反过来意味着前端一句 JSON 就能给自己开
    /// `srv:shell/exec`，而这条路上的客户端是浏览器。
    ///
    /// 名字里带 `srv:` 前缀是**对的**（跟 `tools` 那个字段的规则正相反）：这里给的
    /// 不是「我有一个工具」，是「把你那个工具关掉」，指的就是服务端那批。
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<Vec<String>>"))]
    pub(crate) disable_builtin: Vec<String>,
}

/// 一个跑在宿主侧的工具。前三个字段对着 `agent_core::ToolSpec`
/// （`name`/`description`/`schema`），[`assemble`] 原样落成 `ToolSpec`；第四个落进
/// 工具表旁边那张可逆性表，**不进 `ToolSpec`**——它进 prompt，加字段要重算红线 11
/// 的账，而可逆性纯查表（HOST-CAPABILITIES.md §五）。
// 061 在这里挂过一句 `#[allow(dead_code)]`（除 `name` 外的字段那时没有读者）——
// 062 把四个字段全接进 `assemble::host_tools` 了，那句记账连同理由一起删掉。
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub(crate) struct CapabilityTool {
    /// **必须**带 `web:` / `desk:` 前缀，规则与理由见 [`validate`]。
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: String,
    /// 缺省 `{"type":"object"}`——照 `agent_runtime::skill` 装载 SKILL.md 时
    /// `tools[].schema` 的既有兜底（「一个不吃参数的工具」），不是新发明的默认值。
    #[serde(default = "empty_object_schema")]
    pub(crate) schema: serde_json::Value,
    /// 缺省 `None`。**协议层不在这里落保守值**：把「宿主没说」解释成
    /// `Irreversible` 是装配那一层的事（062，HOST-CAPABILITIES.md §五），这里只
    /// 如实记录说没说。
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub(crate) reversibility: Option<CapabilityReversibility>,
}

/// 一个宿主侧的 skill：`description` 进常驻索引（每个 skill 一行），`body` 与
/// `tools` 等模型 `srv:skill/activate` 之后才注入（skill 的既有形状，
/// HOST-CAPABILITIES.md §一）。
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub(crate) struct CapabilitySkill {
    #[serde(default)]
    pub(crate) id: String,
    // 061 在这两个字段上挂过 `#[allow(dead_code)]`（skill 的装配那时还没做）——
    // 064 把它们全接进 `assemble::host_skills` 了：`description` 进常驻索引那一行，
    // `body` 等 `srv:skill/activate` 之后进 `late_system`。那两句记账连同理由一起删掉。
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) body: String,
    /// 自带的工具——**跟顶层 `tools` 过同一条校验**（[`validate`]）：激活之后它们
    /// 进的是同一张工具表，放宽这里等于给 `srv:` 开了个后门。
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<Vec<CapabilityTool>>"))]
    pub(crate) tools: Vec<CapabilityTool>,
}

/// 宿主面的可逆性拼法：小写。跟 `agent_core::Reversibility` 一一对应，[`From`] 是
/// 两者之间唯一的桥（062 装配时用它）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub(crate) enum CapabilityReversibility {
    Pure,
    Reversible,
    Irreversible,
}

impl From<CapabilityReversibility> for Reversibility {
    fn from(value: CapabilityReversibility) -> Self {
        match value {
            CapabilityReversibility::Pure => Reversibility::Pure,
            CapabilityReversibility::Reversible => Reversibility::Reversible,
            CapabilityReversibility::Irreversible => Reversibility::Irreversible,
        }
    }
}

fn empty_object_schema() -> serde_json::Value {
    serde_json::json!({ "type": "object" })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parse(value: serde_json::Value) -> Capabilities {
        serde_json::from_value(value).expect("该解析成功")
    }

    /// 缺字段不该是解析失败：空对象、只给一半、工具只给名字，全部合法。
    #[test]
    fn every_field_has_a_default() {
        assert!(parse(json!({})).tools.is_empty());
        assert!(parse(json!({})).skills.is_empty());

        let only_tools = parse(json!({ "tools": [ { "name": "web:crm/lookup" } ] }));
        assert!(only_tools.skills.is_empty());
        assert_eq!(only_tools.tools[0].description, "");
        assert_eq!(only_tools.tools[0].schema, json!({ "type": "object" }));

        let only_skills = parse(json!({ "skills": [ { "id": "crm-flow" } ] }));
        assert!(only_skills.tools.is_empty());
        assert_eq!(only_skills.skills[0].body, "");
        assert!(only_skills.skills[0].tools.is_empty());
    }

    /// 061 验收原文：`reversibility` 缺省 → 解析成 `None`（062 才把它落成保守的
    /// `Irreversible`——协议层不替宿主做这个解释）。
    #[test]
    fn a_missing_reversibility_parses_to_none() {
        let caps = parse(json!({ "tools": [ { "name": "web:crm/lookup" } ] }));
        assert_eq!(caps.tools[0].reversibility, None);
    }

    /// 三个等级都是**小写**字符串；PascalCase（`agent_core::Reversibility` 的
    /// serde 拼法）在宿主面不认——两种拼法都认会让「协议形状」变成两份。
    #[test]
    fn reversibility_is_lowercase_on_the_wire() {
        for (text, expected) in [
            ("pure", CapabilityReversibility::Pure),
            ("reversible", CapabilityReversibility::Reversible),
            ("irreversible", CapabilityReversibility::Irreversible),
        ] {
            let caps = parse(json!({ "tools": [ { "name": "web:x/y", "reversibility": text } ] }));
            assert_eq!(caps.tools[0].reversibility, Some(expected));
        }
        let pascal: Result<Capabilities, _> =
            serde_json::from_value(json!({ "tools": [ { "name": "web:x/y", "reversibility": "Pure" } ] }));
        assert!(pascal.is_err(), "PascalCase 不该被宿主面接受");
    }

    /// 宿主面拼法 → core 拼法的桥（062 装配时唯一的转换点）。
    #[test]
    fn reversibility_maps_onto_the_core_enum() {
        assert_eq!(Reversibility::from(CapabilityReversibility::Pure), Reversibility::Pure);
        assert_eq!(Reversibility::from(CapabilityReversibility::Reversible), Reversibility::Reversible);
        assert_eq!(Reversibility::from(CapabilityReversibility::Irreversible), Reversibility::Irreversible);
    }

    /// schema 原样收下（红线 11：`serde_json::Value` 的对象后端是 `BTreeMap`，
    /// 逐字节确定，这里不做任何重排或改写）。
    #[test]
    fn a_given_schema_is_taken_as_is() {
        let schema = json!({ "type": "object", "properties": { "id": { "type": "string" } } });
        let caps = parse(json!({ "tools": [ { "name": "web:crm/lookup", "schema": schema } ] }));
        assert_eq!(caps.tools[0].schema, schema);
    }

    /// skill 自带的工具跟顶层工具是**同一个类型**——形状上就没有「宽松一档」的
    /// 余地（校验也一样，见 [`validate`] 的测试）。
    #[test]
    fn a_skill_carries_the_same_tool_shape() {
        let caps = parse(json!({
            "skills": [ {
                "id": "crm-flow",
                "description": "处理客户工单",
                "body": "第一步……",
                "tools": [ { "name": "web:crm/close", "reversibility": "irreversible" } ]
            } ]
        }));
        let skill = &caps.skills[0];
        assert_eq!(skill.description, "处理客户工单");
        assert_eq!(skill.body, "第一步……");
        assert_eq!(skill.tools[0].name, "web:crm/close");
        assert_eq!(skill.tools[0].reversibility, Some(CapabilityReversibility::Irreversible));
    }

    /// 认不得的字段忽略、不报错——宿主比 server 先升级是常态（协议加字段时旧
    /// server 不该 400）。
    #[test]
    fn unknown_fields_are_ignored() {
        let caps = parse(json!({ "tools": [ { "name": "web:x/y", "future_field": 1 } ], "future_field": true }));
        assert_eq!(caps.tools.len(), 1);
    }
}
