//! 宿主注入的 skill 声明：**形状**（[`HostSkill`]）与它 ↔ [`AgentValue::Json`] 的
//! 唯一一处编解码（064）。
//!
//! `Slot::HostSkills` 装的就是这个形状。跟隔壁 [`host_tools`](super::host_tools) 是
//! 同一类东西（一份会进 prompt 的声明落成 JSON），只是每一项多了「正文」和「自带
//! 的工具」两样。
//!
//! ## 为什么 skill 声明也要进 store（跟 073 同一条理由，而且更硬）
//!
//! 073 已经为注入的**工具**回答过一遍：历史对话是在那一份工具表下产生的、工具表在
//! prompt 最前面（红线 11）、恢复是忠实重放而不是用今天的配置重建。skill 声明是
//! 同一类东西——**它的索引行进 system 段**（`SkillRegistry::skill_index_chunk`），
//! 同样是稳定前缀的一部分。
//!
//! 还有两条是 skill 特有的，比工具那边更硬：
//!
//! 1. **[`Slot::SkillsActive`](crate::graph::Slot::SkillsActive) 已经在 store 里了。**
//!    激活集恢复回来、而 registry 空着，就是一个**悬空引用**：会话状态说
//!    `crm-flow` 是激活的，展开注入时却什么都取不到（registry 查不到的 id 静默跳过），
//!    模型的历史里明明写着「我激活了它、读过它的正文」。那正是本仓最怕的静默错值。
//! 2. **宿主没有第二次机会报。** 073 落地之后，有历史的会话再带 `capabilities` 一律
//!    400 `session_has_history`——不存下来就**永久没了**，连「重连时重报一遍」这条
//!    (被否决的) 退路都不存在。
//!
//! ## 红线 11：排序 + 稳定字节
//!
//! [`to_value`] **按 skill id、每个 skill 的 tool name 排序**再落值：客户端给的数组
//! 顺序不可靠（两次连接可能不同序），而它会经 registry 变成 prompt 字节。
//! `serde_json::Value` 的对象后端是 `BTreeMap`（根 `Cargo.toml` 显式不开
//! `preserve_order`），所以同一份声明两次序列化逐字节相同——`schema` 那个自由 JSON
//! 也一样，这条有断言钉住，不是假设。
//!
//! ## 自带工具的 `reversibility` 随声明落盘
//!
//! `ToolSpec` 只装三个进 prompt 的字段，不能为了执行元数据改它的稳定字节；可逆性
//! 因此按工具名旁挂在 [`HostSkill::tool_reversibility`]。映射用 `BTreeMap`，客户端的
//! 工具输入顺序不会让这段持久化字节漂移。旧 journal 没有这个字段时按空映射恢复；
//! 执行侧查不到某个名字时必须保守按 `Irreversible`，不能把历史缺省猜成安全。

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ids::SkillId;
use crate::value::atom_value::AgentValue;
use crate::value::tool::{Reversibility, ToolSpec};

/// 宿主建会话时声明的一个 skill：`description` 进常驻索引（每个 skill 一行），
/// `body` 与 `tools` 等模型 `srv:skill/activate` 之后才注入这一轮
/// （skill 的既有形状，`docs/HOST-CAPABILITIES.md` §一）。
///
/// 形状对着 `agent_runtime::skill::Skill`（registry 里那个私有类型）——本类型是
/// 「宿主报进来、落进 store、再交给 registry」这条路上的**唯一**一份数据，
/// registry 从它现造自己的内部表示，不反过来。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct HostSkill {
    pub id: SkillId,
    /// 进常驻索引的那一行描述（`<id>: <描述>`）。
    pub description: Arc<str>,
    /// 激活后整段进 `late_system` 的正文。
    ///
    /// **今天没有长度上限**（HOST-CAPABILITIES §九「这一节还没定的」最后一条：
    /// 本机目录装载的 skill 没上限是因为那是本机文件，网络注入应该有——那条属于
    /// 安全那一节，064 明确不做）。也就是说一份很长的 `body` 会让**激活之后的
    /// 每一轮**都变贵，这是确定的成本、不是不确定的风险。
    pub body: Arc<str>,
    /// 激活时进 `late_tools` 的工具。三个字段就是进 prompt 的那三个。
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    /// 按工具名保存的执行可逆性；不进 prompt。缺字段兼容旧 journal。
    #[serde(default)]
    pub tool_reversibility: BTreeMap<Arc<str>, Reversibility>,
}

/// 按 skill id、每个 skill 的 tool name 排序 → JSON 对象数组值（红线 11）。
pub(crate) fn to_value(mut skills: Vec<HostSkill>) -> AgentValue {
    for skill in &mut skills {
        skill.tools.sort_by(|a, b| a.name.cmp(&b.name));
    }
    skills.sort_by(|a, b| a.id.0.cmp(&b.id.0));
    let items: Vec<serde_json::Value> = skills
        .into_iter()
        .map(|skill| serde_json::to_value(skill).expect("五个字段全是可序列化的普通类型"))
        .collect();
    AgentValue::Json(Arc::new(serde_json::Value::Array(items)))
}

/// 从值里读回声明（原样顺序；skill 与它的 tools 写入时都已排序）。
///
/// 形状对不上的**整项跳过**，不 panic：这条路径同时是恢复路径，一份历史数据里多出
/// 一项这一版认不出的东西，不该让整个会话起不来（跟 `host_tools::from_value`
/// 同一条取向）。跳过是**可见的**——索引里少一行，
/// 模型激活它会当场收到 `is_error`，不是那种要等到账单上才浮出来的静默。
pub(crate) fn from_value(value: &AgentValue) -> Vec<HostSkill> {
    let Some(array) = value.as_json().and_then(|j| j.as_array()) else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|item| serde_json::from_value::<HostSkill>(item.clone()).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn skill(id: &str, tool: &str) -> HostSkill {
        HostSkill {
            id: SkillId::new(id),
            description: Arc::from("一行描述"),
            body: Arc::from("正文若干"),
            tools: vec![ToolSpec {
                name: Arc::from(tool),
                description: Arc::from("说明"),
                schema: Arc::new(
                    json!({ "type": "object", "properties": { "id": { "type": "string" } } }),
                ),
            }],
            tool_reversibility: BTreeMap::from([(Arc::from(tool), Reversibility::Pure)]),
        }
    }

    /// 往返：落值再读回来，五个字段一个不少、一个不错。
    #[test]
    fn round_trips_through_the_value() {
        let read = from_value(&to_value(vec![
            skill("zeta-flow", "web:z/one"),
            skill("alpha-flow", "web:a/one"),
        ]));

        assert_eq!(read.len(), 2);
        assert_eq!(read[0].id.as_str(), "alpha-flow", "写入时按 id 排过序");
        assert_eq!(read[1].id.as_str(), "zeta-flow");
        assert_eq!(&*read[0].body, "正文若干");
        assert_eq!(&*read[0].tools[0].name, "web:a/one");
        assert_eq!(
            read[0].tools[0].schema["properties"]["id"]["type"],
            json!("string")
        );
        assert_eq!(read[0].tool_reversibility["web:a/one"], Reversibility::Pure);
    }

    /// 红线 11：skill 数组、同一 skill 的 tool 数组、`schema` 键序与元数据键序
    /// 都进不了落盘字节。
    #[test]
    fn the_bytes_do_not_depend_on_input_order_anywhere() {
        let mut forward = serde_json::Map::new();
        forward.insert("id".to_string(), json!({ "type": "string" }));
        forward.insert("all".to_string(), json!({ "type": "boolean" }));
        let mut backward = serde_json::Map::new();
        backward.insert("all".to_string(), json!({ "type": "boolean" }));
        backward.insert("id".to_string(), json!({ "type": "string" }));

        let with = |schema: &serde_json::Map<String, serde_json::Value>, reverse_nested: bool| {
            let make = |id: &str, tool: &str| {
                let spec = |name: &str| ToolSpec {
                    name: Arc::from(name),
                    description: Arc::from("说明"),
                    schema: Arc::new(serde_json::Value::Object(schema.clone())),
                };
                let mut tools = vec![spec(tool), spec("web:common/audit")];
                if reverse_nested {
                    tools.reverse();
                }
                let entries = [
                    (Arc::from("web:common/audit"), Reversibility::Irreversible),
                    (Arc::from(tool), Reversibility::Pure),
                ];
                let tool_reversibility = if reverse_nested {
                    entries.into_iter().rev().collect()
                } else {
                    entries.into_iter().collect()
                };
                HostSkill {
                    id: SkillId::new(id),
                    description: Arc::from("一行描述"),
                    body: Arc::from("正文若干"),
                    tools,
                    tool_reversibility,
                }
            };
            vec![make("b-flow", "web:b/x"), make("a-flow", "web:a/y")]
        };
        let bytes = |v: &AgentValue| {
            let AgentValue::Json(json) = v else {
                panic!("落 Json")
            };
            serde_json::to_string(&**json).unwrap()
        };

        let a = to_value(with(&forward, false));
        let mut reversed = with(&backward, true);
        reversed.reverse();
        assert_eq!(
            bytes(&a),
            bytes(&to_value(reversed)),
            "声明的落盘字节不许跟着输入顺序漂（红线 11）"
        );
        assert_eq!(
            bytes(&a),
            bytes(&to_value(with(&forward, false))),
            "同一份声明两次序列化也必须逐字节相同"
        );
    }

    /// 空声明落成空数组（默认值就是它）——「没声明」和「声明了零个」在状态上就是
    /// 同一件事，不该有第二种表示。
    #[test]
    fn no_declaration_is_an_empty_array_not_null() {
        let value = to_value(Vec::new());
        assert_eq!(value, crate::graph::Slot::HostSkills.default_value());
        assert!(from_value(&value).is_empty());
    }

    /// 非数组值、以及数组里形状不对的项：跳过，不 panic（恢复路径上的取向）。
    /// 缺 `tools` 的项**不算坏**（`serde(default)`）——一个不带工具的 skill 合法。
    #[test]
    fn a_malformed_value_reads_as_empty_instead_of_panicking() {
        assert!(from_value(&AgentValue::Null).is_empty());
        assert!(from_value(&AgentValue::U64(3)).is_empty());

        let half_bad = AgentValue::Json(Arc::new(json!([
            { "id": "ok-flow", "description": "d", "body": "b" },
            { "id": "no-body" },
            "整项就不是对象"
        ])));
        let read = from_value(&half_bad);
        assert_eq!(read.len(), 1, "只有形状完整的那一项活下来：{read:?}");
        assert_eq!(read[0].id.as_str(), "ok-flow");
        assert!(
            read[0].tools.is_empty(),
            "缺 tools 是合法的（不带工具的 skill）"
        );
        assert!(
            read[0].tool_reversibility.is_empty(),
            "旧 journal 缺执行元数据时按空映射恢复"
        );
    }
}
