//! 「这个 agent 此刻在等谁 ↔ [`AgentValue::Json`] 数组」的唯一一处编解码
//! （212，决策 35 §一）。
//!
//! `Slot::AwaitingOn` 装的就是这个形状：一列 `(target, until)`——**等待图的一行**。
//!
//! # 它必须是 journaled 状态，不能是内存里的一张表
//!
//! 这是本文件存在的全部理由。查环要遍历这张图，而**恢复之后还得查得了环**：
//! 放内存里，一次崩溃恢复就把查环能力丢了，而丢了不报错——恢复出来的会话上，
//! 一条本该被拒的反向 `await` 会被放行，然后两个 agent 互相等到天荒地老，
//! 泵安静地返回，没有 panic、没有超时、没有告警。
//!
//! # 有序（红线 11）
//!
//! 它进 `await` 的**拒绝文本**（把环上那条链原样列出来给模型看），所以落盘与读回
//! 都必须逐字节确定。[`to_value`] 按 target 排序。
//!
//! # 形状是 `[[target, until], …]`
//!
//! 两个字段都是位置固定的字符串，键名进落盘字节纯属浪费——跟隔壁
//! [`host_prefix`](super::host_prefix) / [`inbox`](super::inbox) 同一个选择。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ids::AgentId;
use crate::value::atom_value::AgentValue;

/// 等到什么算等到。
///
/// **是枚举，不是条件表达式**（212 §注意）：一个能表达任意谓词的参数等于让模型
/// 往 derived 的 read fn 里塞代码，红线 1 当场破。三档够用了。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum AwaitUntil {
    /// 任一终态（`Done` 或 `Failed`）。**缺省档**——「它到了没有」通常就是这个意思。
    Settled,
    /// 只等成功收场。它失败了 → 等待方立刻收到一个 `is_error` 的结果，
    /// 不是继续等（等一个已经死掉的条件是永远等）。
    Done,
    /// 只等失败收场。给「我要在它出错时接手」这类编排留的，跟 `Done` 对称。
    Failed,
}

impl AwaitUntil {
    /// 落盘/线上的那个词。**不用 `Debug`**：那个字符串是 Rust 的实现细节，
    /// 改个变体名就悄悄改了落盘格式，而这个值要跨进程读回来。
    pub fn as_str(self) -> &'static str {
        match self {
            AwaitUntil::Settled => "settled",
            AwaitUntil::Done => "done",
            AwaitUntil::Failed => "failed",
        }
    }

    /// 反过来。认不出就是 `None`——调用方决定是拒绝（模型写错了）还是当缺省。
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "settled" => Some(AwaitUntil::Settled),
            "done" => Some(AwaitUntil::Done),
            "failed" => Some(AwaitUntil::Failed),
            _ => None,
        }
    }
}

/// 等待图的一行：这个 agent 在等的那些 `(目标, 条件)`。
pub type Awaiting = Vec<(AgentId, AwaitUntil)>;

/// 一行 → `[[target, until], …]`，**按 target 排序**（红线 11，见模块文档）。
pub(crate) fn to_value(mut edges: Awaiting) -> AgentValue {
    edges.sort();
    edges.dedup();
    let items: Vec<serde_json::Value> = edges
        .into_iter()
        .map(|(target, until)| {
            serde_json::Value::Array(vec![
                serde_json::Value::String(target.as_str().to_string()),
                serde_json::Value::String(until.as_str().to_string()),
            ])
        })
        .collect();
    AgentValue::Json(Arc::new(serde_json::Value::Array(items)))
}

/// 从值里读回这一行。
///
/// 形状不对——**整份读空**，不逐项跳过（照 `prefix_chunks`/`host_prefix` 的先例）。
/// 这份值是一次原子写入的整体，跳掉坏的那一项会拼出一张**少一条边的等待图**，
/// 而查环正是靠这张图——少一条边就是一个本该被拒的环被放行，不报错。
pub(crate) fn from_value(value: &AgentValue) -> Awaiting {
    let Some(array) = value.as_json().and_then(|j| j.as_array()) else {
        return Awaiting::new();
    };
    let mut edges = Awaiting::with_capacity(array.len());
    for item in array {
        let Some([target, until]) = item.as_array().map(Vec::as_slice) else {
            return Awaiting::new();
        };
        let (Some(target), Some(until)) = (target.as_str(), until.as_str()) else {
            return Awaiting::new();
        };
        let Some(until) = AwaitUntil::parse(until) else {
            return Awaiting::new();
        };
        edges.push((AgentId::new(target), until));
    }
    edges
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn edge(id: &str, until: AwaitUntil) -> (AgentId, AwaitUntil) {
        (AgentId::new(id), until)
    }

    /// 往返 + 排序（红线 11）：乱序写进去，读回来按 target 升序。
    #[test]
    fn round_trips_sorted_by_target() {
        let read = from_value(&to_value(vec![
            edge("root/b", AwaitUntil::Done),
            edge("root/a", AwaitUntil::Settled),
        ]));
        assert_eq!(
            read,
            vec![
                edge("root/a", AwaitUntil::Settled),
                edge("root/b", AwaitUntil::Done)
            ]
        );
    }

    /// 同一条边写两次是一条。等待图是一张**图**，不是流水账——重复的边会让
    /// 「等到了就清掉」清一次剩一条，那条残边会把后面所有反向 `await` 都误判成环。
    #[test]
    fn a_duplicate_edge_collapses() {
        let value = to_value(vec![
            edge("root/a", AwaitUntil::Settled),
            edge("root/a", AwaitUntil::Settled),
        ]);
        assert_eq!(from_value(&value).len(), 1);
    }

    /// 同一个目标、不同条件是**两条边**：它们等到的时刻不同。
    #[test]
    fn the_same_target_with_two_conditions_is_two_edges() {
        let value = to_value(vec![
            edge("root/a", AwaitUntil::Done),
            edge("root/a", AwaitUntil::Failed),
        ]);
        assert_eq!(from_value(&value).len(), 2);
    }

    /// 空图落成空数组（默认值就是它）。
    #[test]
    fn an_empty_graph_is_an_empty_array_not_null() {
        let value = to_value(Vec::new());
        assert_eq!(value, crate::graph::Slot::AwaitingOn.default_value());
        assert!(from_value(&value).is_empty());
    }

    /// 红线 11：两次序列化逐字节相同，跟输入顺序无关。
    #[test]
    fn serialization_is_byte_stable() {
        let bytes = |v: &AgentValue| {
            let AgentValue::Json(json) = v else {
                panic!("落 Json")
            };
            serde_json::to_string(&**json).unwrap()
        };
        let a = to_value(vec![
            edge("root/b", AwaitUntil::Done),
            edge("root/a", AwaitUntil::Settled),
        ]);
        let b = to_value(vec![
            edge("root/a", AwaitUntil::Settled),
            edge("root/b", AwaitUntil::Done),
        ]);
        assert_eq!(bytes(&a), bytes(&b));
        assert_eq!(bytes(&a), r#"[["root/a","settled"],["root/b","done"]]"#);
    }

    /// **落盘的词不是 `Debug`**：改个变体名不该改落盘格式。
    #[test]
    fn the_wire_words_are_pinned() {
        assert_eq!(AwaitUntil::Settled.as_str(), "settled");
        assert_eq!(AwaitUntil::Done.as_str(), "done");
        assert_eq!(AwaitUntil::Failed.as_str(), "failed");
        for until in [AwaitUntil::Settled, AwaitUntil::Done, AwaitUntil::Failed] {
            assert_eq!(AwaitUntil::parse(until.as_str()), Some(until));
        }
        assert_eq!(AwaitUntil::parse("whenever"), None);
    }

    /// 形状不对：整份读空，不是留下前面几条拼出一张少一条边的等待图。
    #[test]
    fn a_malformed_value_reads_as_an_empty_graph() {
        assert!(from_value(&AgentValue::Null).is_empty());
        for bad in [
            json!([["root/a", "settled"], "整项不是数组"]),
            json!([["root/a", "settled"], ["root/b"]]),
            json!([["root/a", "settled"], ["root/b", 3]]),
            json!([["root/a", "settled"], ["root/b", "whenever"]]),
        ] {
            assert!(
                from_value(&AgentValue::Json(Arc::new(bad.clone()))).is_empty(),
                "{bad} 该整份读空"
            );
        }
    }
}
