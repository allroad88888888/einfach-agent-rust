//! 跨 agent 读（红线 10，**决策 35 起不限方向**）。
//!
//! 整棵 agent 树在同一个 store 里，谁都**物理可达**。决策 35 之前这里只开两个方向
//! （子读父 / 父读子），论证是「`U ∩ D = ∅` + 图恒为树 ⇒ 环不可能」。
//! **那个论证的前提在这个仓里从来没成立过**——见下。
//!
//! ## 这里的读一条依赖边都不建
//!
//! 三个口最后都走 [`Session::peek`](super::session::Session) → `store.get`，
//! 那是**非追踪**读：调用它的是命令层，不是某个 derived 的 read fn，所以没有任何
//! atom 因此依赖 `target`。建边只发生在 derived 里调 `args.get`——决策 35 之前，
//! 那样的调用在生产代码里只有 `graph::build` 一处，读的还是**自己 agent** 的槽位。
//!
//! 换句话说：**方向约束防的是一类当时还不存在的边**，而且这三个口在 028～M19 期间
//! **没有任何生产调用方**（引用全在测试里）。决策 35 把它去掉，判据只剩一条：
//!
//! | 校验 | 挡住什么 |
//! |---|---|
//! | **可见性**：槽位不是 [`Visibility::Private`] | 别人的内部账本（预算 / 消息号 / 前缀镜像 / 工具槽 / 压缩账 / 收件箱） |
//!
//! 无环从此挂在别处：**跨 agent 的边只许指向 primitive**（`graph::visibility`
//! 模块文档），第一条真的跨 agent 边由 `srv:agent/await` 的 derived 建出来
//! （issue 212），断言在那里。
//!
//! ## 为什么读口是**非创建**的
//!
//! 命令层写槽位走 `graph::source_atom`（get-or-create），读口走
//! [`Session::peek`](super::session::Session)（非创建）。差别是刻意的：写入必须保证
//! 目标存在，而读取有副作用就意味着「宿主传错一个 `AgentId`」会在 family 里静静留下
//! 十个没人写的 atom，它们还会跟着进快照。读不到就说读不到（[`ReadDenied::NoSuchAtom`]）。

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
    /// `target` 不是 `reader` 的祖先。只有 [`Session::read_ancestor`] 会给出它
    /// ——那是一个**带方向断言的封装**，不是 [`Session::read_agent`] 的行为。
    NotAnAncestor { reader: AgentId, target: AgentId },
    /// `target` 不是 `reader` 的后代（同上，方向相反）。
    NotADescendant { reader: AgentId, target: AgentId },
    /// 这个槽位是 [`Visibility::Private`]——别的 agent 一律读不到
    /// （`graph::visibility`）。
    NotVisible { slot: Slot },
    /// 可见性没问题，但这个 atom 不在图上：从没建过、已经被 despawn 逐出、
    /// 或者这个 id 根本不属于本会话这棵树。**不顺手建一个**（见模块文档）。
    NoSuchAtom { key: AtomKey },
}

impl Session {
    /// 跨 agent 读：**任意方向**取一次值（决策 35）。
    ///
    /// 祖先、后代、**兄弟**都行——判据只有「这个槽位是不是别人的内部账本」。
    /// 兄弟互读就是靠这一条开出来的：`srv:agent/status`（207）看得见整棵树、
    /// `srv:agent/send`（206）发得到兄弟，都建立在它上面。
    ///
    /// **不建依赖边**（见模块文档）。所以它跟环无关，也没有必要为「订阅」再开
    /// 第二个口——`Session` 这一层没有「建边的读」这回事，两个名字会是同一个实现。
    pub fn read_agent(&self, target: &AgentId, slot: Slot) -> Result<AgentValue, ReadDenied> {
        if slot.visibility() == Visibility::Private {
            return Err(ReadDenied::NotVisible { slot });
        }
        let key = AtomKey::Agent(target.clone(), slot);
        self.peek(&key).ok_or(ReadDenied::NoSuchAtom { key })
    }

    /// 往上读：`reader` 读它的祖先 `target` 的 `slot`。
    ///
    /// **决策 35 起这是 [`Session::read_agent`] 加一道方向断言的薄封装**，
    /// 断言的是**两个 agent 的亲缘关系**，不再是槽位的方向——`Slot` 的方向分类
    /// 已经不存在了（`Visibility` 只剩 `Shared`/`Private`）。
    ///
    /// 留着不删有两个理由：现有测试一行不用改；以及调用点说出「我读的是我祖先」
    /// 这个意图是有价值的——决策 3 承诺的「子读父是一次 `get`」就长在这个方向上。
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
        self.read_agent(target, slot)
    }

    /// 往下读：`reader` 读它的后代 `target` 的 `slot`。
    ///
    /// 同 [`Session::read_ancestor`]，方向相反。029 的「等所有子 agent 完成」
    /// 长在这个方向上（`Pending` 沿依赖图自动汇聚，STATE-MODEL §「子 agent」）
    /// ——**那个汇聚 derived 至今没有被建出来**，运行时用 `Subtree::harvest`
    /// 命令式地做了同一件事，所以别把这个封装的存在当成它已经存在的证据。
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
        self.read_agent(target, slot)
    }
}
