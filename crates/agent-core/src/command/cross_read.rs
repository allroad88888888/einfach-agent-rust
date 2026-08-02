//! 跨 agent 读的**两个口，没有第三个**（红线 10）。
//!
//! 整棵 agent 树在同一个 store 里，谁都**物理可达**——依赖图必须靠 API 约束保持是树。
//! 所以这个文件的全部价值不在它有什么，在它**没有**什么：没有
//! `read_sibling`，没有 `read_any(agent, slot)`，也没有一个「拿到 store 自己去 get」
//! 的逃生口（`Session` 不暴露 `store()`，那是红线 2 的结构面，红线 10 顺带白拿）。
//! 兄弟互读在 API 面上不存在，环因此在结构上不可能——不靠运行时的 `CyclicRef` 兜底。
//!
//! ## 两道校验，缺一不可
//!
//! | 校验 | 挡住什么 |
//! |---|---|
//! | **方向**：目标必须真的是祖先 / 后代（`AgentId` 的路径代数，不读 store） | 兄弟、自己、方向传反了、别的会话的 id |
//! | **可见性**：槽位的 [`Visibility`] 必须与方向一致 | 「往上读一个只该往下读的槽位」——这正是 `U ∩ D = ∅` 会被破坏的那一下 |
//!
//! 两道分别对应「图恒为树」和「两个方向的槽位集合不相交」，合起来才推出无环
//! （证明写在 `graph/visibility.rs` 的模块文档里）。少一道都不成立：只查方向的话，
//! 一个既能上读又能下读的槽位就能在两个 agent 之间连出一对反向边。
//!
//! ## 为什么读口是**非创建**的
//!
//! 命令层写槽位走 `graph::source_atom`（get-or-create），读口走
//! `Session::peek`（非创建）。差别是刻意的：写入必须保证目标存在，而读取有副作用
//! 就意味着「宿主传错一个 `AgentId`」会在 family 里静静留下十个没人写的 atom，
//! 它们还会跟着进快照。读不到就说读不到（[`ReadDenied::NoSuchAtom`]）。

use crate::graph::{AtomKey, Slot, Visibility};
use crate::ids::AgentId;
use crate::value::atom_value::AgentValue;

use super::session::Session;

/// 一次跨 agent 读被拒的理由。
///
/// 全部是**显式拒绝**，不是 `None`：读不到和不许读是两件事，糊成一个 `Option`
/// 之后，调用方只能靠猜来决定要不要重试、要不要报错。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ReadDenied {
    /// `target` 不是 `reader` 的祖先。**兄弟、自己、以及把后代传给
    /// [`Session::read_ancestor`] 都落在这里**——横读在这一条上被挡住。
    NotAnAncestor { reader: AgentId, target: AgentId },
    /// `target` 不是 `reader` 的后代（同上，方向相反）。
    NotADescendant { reader: AgentId, target: AgentId },
    /// 方向对了，但这个槽位不朝这个方向开（`graph::visibility`）。
    NotVisible { slot: Slot, visibility: Visibility },
    /// 方向和可见性都对，但这个 atom 不在图上：从没建过，或者已经被 despawn
    /// 逐出了。**不顺手建一个**（见模块文档）。
    NoSuchAtom { key: AtomKey },
}

impl Session {
    /// 往上读：`reader` 读它的祖先 `target` 的 `slot`。
    ///
    /// 这是决策 3 承诺的「子读父是一次 `get`」那一下——不需要任何消息传递机制，
    /// 走依赖图自动追踪、自动失效。放行的槽位见 [`Visibility::Upward`]。
    ///
    /// `target` 可以是任意层的祖先，不限于直接父：相隔两层去读祖父**不会**多出
    /// 一条绕过中间那层的边，方向仍然是往树根的，环的论证不受影响。
    pub fn read_ancestor(
        &self,
        reader: &AgentId,
        target: &AgentId,
        slot: Slot,
    ) -> Result<AgentValue, ReadDenied> {
        if !target.is_ancestor_of(reader) {
            return Err(ReadDenied::NotAnAncestor {
                reader: reader.clone(),
                target: target.clone(),
            });
        }
        self.read_visible(target, slot, Visibility::Upward)
    }

    /// 往下读：`reader` 读它的后代 `target` 的 `slot`。
    ///
    /// 029 的「等所有子 agent 完成」就长在这个方向上：`Pending` 沿依赖图自动汇聚，
    /// 不用写调度器（STATE-MODEL §「子 agent」第 2 条）。放行的槽位见
    /// [`Visibility::Downward`]。
    pub fn read_descendant(
        &self,
        reader: &AgentId,
        target: &AgentId,
        slot: Slot,
    ) -> Result<AgentValue, ReadDenied> {
        if !target.is_descendant_of(reader) {
            return Err(ReadDenied::NotADescendant {
                reader: reader.clone(),
                target: target.clone(),
            });
        }
        self.read_visible(target, slot, Visibility::Downward)
    }

    /// 两个口共用的后半段：可见性 + 非创建取值。
    ///
    /// 合成一处是刻意的——两份拷贝就是两处可以判错的地方，而判错的后果是
    /// 「多了一条本不该有的边」，不报错。
    fn read_visible(
        &self,
        target: &AgentId,
        slot: Slot,
        direction: Visibility,
    ) -> Result<AgentValue, ReadDenied> {
        let visibility = slot.visibility();
        if visibility != direction {
            return Err(ReadDenied::NotVisible { slot, visibility });
        }
        let key = AtomKey::Agent(target.clone(), slot);
        self.peek(&key).ok_or(ReadDenied::NoSuchAtom { key })
    }
}
