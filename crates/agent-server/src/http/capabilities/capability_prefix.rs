//! [`CapabilityPrefix`]：一个宿主声明的开局块（M17，决策 31，156）。
//!
//! 单独成文件是因为 `mod.rs` 顶到了行数上限（红线 9）——这个类型跟
//! `CapabilityTool`/`CapabilitySkill` 是同一层的东西（`capabilities` 的一个
//! 顶层字段），只是搬了家，不是有什么特殊之处。

use serde::Deserialize;

/// 宿主建会话**之前**自己跑完逻辑、把结果文本带进来的开局块：`name` 是它落进
/// timed 区之后的规范名（校验见 `super::validate`——必须 `web:`/`desk:` 前缀，
/// 跟 `super::CapabilityTool::name` 同一条规则），`text` 是它的全部内容，原样
/// 落进 `init:<name>` 前缀块（135 的契约），一个字节不加工。两个字段都
/// `#[serde(default)]`（空串）——缺了不是解析失败，是 `validate` 用结构化错误
/// 拒（名字前缀不对 / text 是空串），跟 `CapabilityTool` 同一套「协议层不拒、
/// 校验层拒」的取舍。
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub(crate) struct CapabilityPrefix {
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) text: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::http::capabilities::Capabilities;

    fn parse(value: serde_json::Value) -> Capabilities {
        serde_json::from_value(value).expect("该解析成功")
    }

    /// 缺字段不该是解析失败——那一拒是 `validate` 的事，不是 serde 的事。
    #[test]
    fn both_fields_default_to_an_empty_string() {
        let caps = parse(json!({ "prefix": [ {} ] }));
        assert_eq!(caps.prefix[0].name, "");
        assert_eq!(caps.prefix[0].text, "");
    }

    /// 一份写全的声明——两个字段原样收下（进 prompt 的唯一字段是 `text`，这里
    /// 先只证「解析」，落进 `init:<name>` 前缀块是 `assemble`/runtime 的事）。
    #[test]
    fn a_prefix_entry_carries_its_name_and_text() {
        let caps = parse(json!({
            "prefix": [ { "name": "web:crm/briefing", "text": "今天的客户上下文：……" } ]
        }));
        assert_eq!(caps.prefix[0].name, "web:crm/briefing");
        assert_eq!(caps.prefix[0].text, "今天的客户上下文：……");
    }
}
