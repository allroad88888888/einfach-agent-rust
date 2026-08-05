//! 「宿主注入的工具声明 ↔ [`AgentValue::Json`] 数组」的唯一一处编解码（073）。
//!
//! `Slot::HostTools` 装的就是这个形状。跟隔壁 [`str_set`](super::str_set) 是同一类
//! 东西（一份会进 prompt 的集合落成 JSON），只是每一项不是一个字符串而是一个对象：
//! 工具的三个描述字段（进 prompt）+ 可逆性（不进 prompt，纯查表）。
//!
//! ## 为什么落盘的是「声明」而不是「id」
//!
//! skill 存的是激活的 id、正文从运行时 registry 现取；注入的工具**连描述和 schema
//! 一起存**。差别的根源是「store 外面有没有第二份」：skill 的正文在 `SkillRegistry`
//! 里（本机文件装载出来的资产，两次运行之间可能改），而宿主注入的声明**只在那一次
//! HTTP 请求里存在过**——不存下来就没有第二处可取，恢复时只能向宿主重新要一遍，
//! 而那正是本 issue 要拆掉的东西（历史对话该在它当初那份工具表下原样复刻）。
//!
//! ## 红线 11：排序 + 稳定字节
//!
//! [`to_value`] **按名字排序**再落值：客户端给的数组顺序不可靠（两次连接可能不同
//! 序），而它会经工具表变成 prompt 字节。`serde_json::Value` 的对象后端是
//! `BTreeMap`（根 `Cargo.toml` 显式不开 `preserve_order`），所以同一份声明两次
//! 序列化逐字节相同——`schema` 那个自由 JSON 也一样，这条有断言钉住，不是假设。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::value::atom_value::AgentValue;
use crate::value::tool::{Reversibility, ToolSpec};

/// 一项声明的落盘形状。**不复用 `ToolSpec` + 一个旁挂的可逆性表**：落盘要的是
/// 「一项就是一个自洽的对象」，把可逆性拆到另一个数组里会让两个数组的下标对齐
/// 成为一条没人检查的纪律，历史数据一旦错位就是静默错值。
#[derive(Serialize, Deserialize)]
struct Declared {
    name: Arc<str>,
    description: Arc<str>,
    schema: Arc<serde_json::Value>,
    reversibility: Reversibility,
}

/// 按名字排序 → JSON 对象数组值（红线 11）。
pub(crate) fn to_value(mut tools: Vec<(ToolSpec, Reversibility)>) -> AgentValue {
    tools.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    let items: Vec<serde_json::Value> = tools
        .into_iter()
        .map(|(spec, reversibility)| {
            let declared = Declared {
                name: spec.name,
                description: spec.description,
                schema: spec.schema,
                reversibility,
            };
            serde_json::to_value(declared).expect("四个字段全是可序列化的普通类型")
        })
        .collect();
    AgentValue::Json(Arc::new(serde_json::Value::Array(items)))
}

/// 从值里读回声明（原样顺序，写入时已排序）。
///
/// 形状对不上的**整项跳过**，不 panic：这条路径同时是恢复路径，一份历史数据里
/// 多出一项这一版认不出的东西，不该让整个会话起不来（跟 [`str_set::from_value`]
/// 同一条取向）。跳过是**可见的**——工具表里少一个工具，模型调不到它会当场报错，
/// 不是那种要等到账单上才浮出来的静默。
pub(crate) fn from_value(value: &AgentValue) -> Vec<(ToolSpec, Reversibility)> {
    let Some(array) = value.as_json().and_then(|j| j.as_array()) else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|item| serde_json::from_value::<Declared>(item.clone()).ok())
        .map(|d| {
            let spec = ToolSpec {
                name: d.name,
                description: d.description,
                schema: d.schema,
            };
            (spec, d.reversibility)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tool(name: &str, reversibility: Reversibility) -> (ToolSpec, Reversibility) {
        let spec = ToolSpec {
            name: Arc::from(name),
            description: Arc::from("说明"),
            schema: Arc::new(
                json!({ "type": "object", "properties": { "id": { "type": "string" } } }),
            ),
        };
        (spec, reversibility)
    }

    /// 往返：落值再读回来，四个字段一个不少、一个不错。
    #[test]
    fn round_trips_through_the_value() {
        let tools = vec![
            tool("web:crm/lookup", Reversibility::Pure),
            tool("desk:clipboard/write", Reversibility::Irreversible),
        ];
        let read = from_value(&to_value(tools));

        assert_eq!(read.len(), 2);
        assert_eq!(
            &*read[0].0.name, "desk:clipboard/write",
            "写入时按名字排过序"
        );
        assert_eq!(read[0].1, Reversibility::Irreversible);
        assert_eq!(&*read[1].0.name, "web:crm/lookup");
        assert_eq!(read[1].1, Reversibility::Pure);
        assert_eq!(&*read[1].0.description, "说明");
        assert_eq!(
            read[1].0.schema["properties"]["id"]["type"],
            json!("string")
        );
    }

    /// 红线 11：客户端给的数组顺序、以及 `schema` 里键的插入顺序，都进不了落盘
    /// 字节——**两次序列化逐字节相同**。`schema` 那一半是本 issue 特有的（`str_set`
    /// 只有字符串），所以这里故意用两种相反的插入顺序造同一份 schema。
    #[test]
    fn the_bytes_do_not_depend_on_input_order_anywhere() {
        let mut forward = serde_json::Map::new();
        forward.insert("id".to_string(), json!({ "type": "string" }));
        forward.insert("all".to_string(), json!({ "type": "boolean" }));
        let mut backward = serde_json::Map::new();
        backward.insert("all".to_string(), json!({ "type": "boolean" }));
        backward.insert("id".to_string(), json!({ "type": "string" }));

        let with = |schema: &serde_json::Map<String, serde_json::Value>| {
            let spec = |name: &str| ToolSpec {
                name: Arc::from(name),
                description: Arc::from("说明"),
                schema: Arc::new(serde_json::Value::Object(schema.clone())),
            };
            vec![
                (spec("web:b/x"), Reversibility::Pure),
                (spec("desk:a/y"), Reversibility::Pure),
            ]
        };
        let bytes = |v: &AgentValue| {
            let AgentValue::Json(json) = v else {
                panic!("落 Json")
            };
            serde_json::to_string(&**json).unwrap()
        };

        // 数组顺序反过来 + schema 键的插入顺序反过来 = 同一份字节。
        let a = to_value(with(&forward));
        let mut reversed = with(&backward);
        reversed.reverse();
        assert_eq!(
            bytes(&a),
            bytes(&to_value(reversed)),
            "声明的落盘字节不许跟着输入顺序漂（红线 11）"
        );
        assert_eq!(
            bytes(&a),
            bytes(&to_value(with(&forward))),
            "同一份声明两次序列化也必须逐字节相同"
        );
    }

    /// 空声明落成空数组（默认值就是它），读回来也是空——「没声明」和「声明了零个」
    /// 在状态上就是同一件事，不该有第二种表示。
    #[test]
    fn no_declaration_is_an_empty_array_not_null() {
        let value = to_value(Vec::new());
        assert_eq!(value, crate::graph::Slot::HostTools.default_value());
        assert!(from_value(&value).is_empty());
    }

    /// 非数组值、以及数组里形状不对的项：跳过，不 panic（恢复路径上的取向）。
    #[test]
    fn a_malformed_value_reads_as_empty_instead_of_panicking() {
        assert!(from_value(&AgentValue::Null).is_empty());
        assert!(from_value(&AgentValue::U64(3)).is_empty());

        let half_bad = AgentValue::Json(Arc::new(json!([
            { "name": "web:a/b", "description": "d", "schema": {}, "reversibility": "Pure" },
            { "name": "web:c/d" },
            "整项就不是对象"
        ])));
        let read = from_value(&half_bad);
        assert_eq!(read.len(), 1, "只有形状完整的那一项活下来：{read:?}");
        assert_eq!(&*read[0].0.name, "web:a/b");
    }
}
