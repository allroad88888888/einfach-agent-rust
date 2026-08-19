//! 「收件箱 ↔ [`AgentValue::Json`] 数组」的唯一一处编解码（205，决策 35）。
//!
//! `Slot::Inbox` 装的是**别的 agent 投进来、本 agent 还没消费的消息**，形状是
//! `Vec<InboxItem>`。三条形状约束各有理由：
//!
//! - **`Vec` 不是 `HashMap`/`HashSet`**（红线 11）：这些正文会被排空进对方的
//!   `Slot::Messages`、从此每一轮都进 prompt，容器的迭代顺序一漂，前缀缓存整段
//!   作废——功能完全正常，只在账单上浮出来（DeepSeek 上 120 倍）。
//! - **正文 `Arc<str>` 不是 `String`**（红线 5）：整份值每投一条都要读出来、追加、
//!   再写回去，克隆必须是指针拷贝。
//! - **时机标记跟正文住在一起**，不拆成两个槽位：见 [`Deliver`]。
//!
//! 跟 [`super::str_set`] 不同，这里**不排序**：顺序本身是信息——它就是这些话被说出
//! 来的先后，排序会把它抹掉。红线 11 要的是「确定」不是「排序」，而确定性这里由
//! 写入点保证（`Session::deliver` 只往尾部追加，同一份日志重放两次追加顺序逐条相同）。
//! 这跟 [`super::prefix_chunks`] 是同一条判据。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ids::AgentId;
use crate::value::atom_value::AgentValue;

/// 一条投递什么时候被喂进收信人的 prompt（决策 35 §二，用户拍的两档）。
///
/// **两档共用一个槽位、靠这个标记区分，不是两个槽位。** 它们的落盘、恢复、undo、
/// `Private` 语义逐字相同，差别只有「哪个定点来收」——拆成两个槽位就要把那四样各
/// 写一遍，而它们必须永远一致。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Deliver {
    /// 加入本轮 loop：收信人**下一次组装 provider 请求之前**排空
    /// （`Session::drain_now`）。
    Now,
    /// 这一轮结束之后才送达：root **下一轮开始时**排空
    /// （`Session::drain_next_turn`）。
    ///
    /// 目标只能是 root——子 agent 不跨 turn，投给别人等于投给一个下一轮不存在的
    /// 收件箱。那道闸在 `command::inbox`，不在这里。
    NextTurn,
}

/// 收件箱里的一条。
///
/// `from` 是**路径 id**（`root/a1` 这样），逐字节确定——红线 11 在这里的落点：
/// 排空时它会被渲染进对方的 prompt，所以这里**不许**再挂时间戳、序号或随机 id。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InboxItem {
    pub from: AgentId,
    pub text: Arc<str>,
    pub when: Deliver,
}

/// 落盘形状：一条 = 一个三元数组 `[from, text, when]`。
///
/// 用元组而不是给 `InboxItem` derive `Serialize`：derive 出来的是 JSON 对象，
/// 而对象的键序取决于 `serde_json` 的 map 实现——数组保序，逐字节确定不必依赖
/// 那个实现细节（跟 [`super::summaries`] 的 `[[id, 正文], …]` 同一个形状）。
type Wire = (AgentId, Arc<str>, Deliver);

/// 收件箱 → [`AgentValue::Json`]。
///
/// `expect` 而不是静默兜底，同 [`super::summaries::to_value`] 的理由：入参只有
/// `AgentId`/`Arc<str>`/一个单元枚举，没有任何会让 `serde_json::to_value` 失败的
/// 形状（NaN 浮点、非字符串 map 键），失败只可能是 derive 坏了，那是要当场炸出来的。
pub(crate) fn to_value(items: &[InboxItem]) -> AgentValue {
    let wire: Vec<Wire> = items
        .iter()
        .map(|i| (i.from.clone(), Arc::clone(&i.text), i.when))
        .collect();
    let json = serde_json::to_value(wire).expect("收件箱可序列化（红线 3）");
    AgentValue::Json(Arc::new(json))
}

/// 从值里读回收件箱（**原样顺序**）。
///
/// 类型对不上或解析失败一律回退到**空收件箱**，同 [`super::str_set::from_value`]
/// 的「宁可空、不 panic」：这个读取点也服务恢复路径，一份形状不对的历史数据不该让
/// 整个会话起不来。
///
/// 空收件箱不是静默错值：排空时什么都不做、不落 entry，代价是那几条话没被送到，
/// 而**不是**送到了一半或送错了人。
pub(crate) fn from_value(value: &AgentValue) -> Vec<InboxItem> {
    let Some(json) = value.as_json() else {
        return Vec::new();
    };
    let wire: Vec<Wire> = serde_json::from_value(json.as_ref().clone()).unwrap_or_default();
    wire.into_iter()
        .map(|(from, text, when)| InboxItem { from, text, when })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(from: &str, text: &str, when: Deliver) -> InboxItem {
        InboxItem {
            from: AgentId::new(from),
            text: Arc::from(text),
            when,
        }
    }

    #[test]
    fn round_trips_keeping_order_and_timing() {
        let items = vec![
            item("root/a2", "先说的", Deliver::Now),
            item("root", "后说的", Deliver::NextTurn),
        ];
        assert_eq!(from_value(&to_value(&items)), items);
    }

    /// 红线 11：同一份收件箱两次序列化逐字节相同。
    #[test]
    fn the_encoding_is_byte_identical_twice() {
        let items = vec![
            item("root/a2", "一", Deliver::Now),
            item("root/a1", "二", Deliver::NextTurn),
        ];
        let (a, b) = (to_value(&items), to_value(&items));
        assert_eq!(
            a.as_json().unwrap().to_string(),
            b.as_json().unwrap().to_string()
        );
    }

    /// **不排序**：投递顺序就是话被说出来的先后，排序会把它抹掉。
    #[test]
    fn it_does_not_sort() {
        let items = vec![
            item("root/z9", "先", Deliver::Now),
            item("root/a1", "后", Deliver::Now),
        ];
        let back = from_value(&to_value(&items));
        assert_eq!(back[0].from.as_str(), "root/z9", "顺序被排掉了");
    }

    /// 形状不对 → 空收件箱，不 panic（恢复路径也走这里）。
    #[test]
    fn a_broken_shape_reads_back_as_empty_instead_of_panicking() {
        assert!(from_value(&AgentValue::Null).is_empty());
        assert!(from_value(&AgentValue::U64(7)).is_empty());
    }
}
