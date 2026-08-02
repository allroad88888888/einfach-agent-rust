//! [`AgentId`]：一个 agent 实例的标识，**同时是它在树里的地址**。
//!
//! 这个文件只负责一件事：**路径代数**——怎么拼一个子 agent 的 id、怎么从 id 上读出
//! 父子祖孙关系。整棵 agent 树共用一个 store（决策 3），family 的键就是它。
//!
//! ## 为什么关系编码在 id 里，而不是存在某个 atom 里
//!
//! `docs/STATE-MODEL.md` §「AgentId 用路径编码」写死了这条：
//!
//! > 不要用「parent 指针存在 atom 里」——那样读取边界的判定就依赖了 store 状态，
//! > 而 undo 正在回滚 store 状态，会绕成死结。
//!
//! 具体的死结长这样：`read_ancestor` 要先判「你是不是我的祖先」，判定读了一个 atom；
//! 那个 atom 正在被 undo 回滚（比如 spawn 那一步被撤了），于是「谁是谁的祖先」在
//! 回滚进行到一半时是不确定的——而 undo 恰恰要靠这个关系决定回滚哪些槽位。
//! 路径编码把关系变成 id 上的**纯字符串运算**：不读 store，也就没有半个世界的问题。
//!
//! ## 分隔符边界是硬要求，不是防御性编程
//!
//! `root/a1` **不是** `root/a10` 的祖先。纯 `starts_with` 判成祖先是 M1 之前的旧实现
//! 真踩过的坑（issue 028 点名记录在案）：一旦判错，`read_ancestor` 会放行一次本该拒绝
//! 的跨 agent 读，红线 10「依赖图恒为树」当场破掉——而且不报错，只是多了一条边，
//! 直到某天成环被 `CyclicRef` 兜底才浮出来。
//!
//! 所以 [`AgentId::is_ancestor_of`] 判的是「前缀 **且** 下一个字节是分隔符」，
//! 单元测试里 `root/a1` vs `root/a10` 是点名要有的用例。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// 路径分隔符。**只有这一处定义**——`child` 拼它、`parent` 找它、`is_ancestor_of`
/// 拿它判边界，三处用同一个常量，改一次全跟上。
pub const AGENT_PATH_SEP: char = '/';

/// 子 agent 段的前缀（`root/a1` 里的 `a`）。让 id 一眼看得出哪一段是 agent 序号，
/// 也避免纯数字段在日志里被误读成别的东西。
const CHILD_SEGMENT_PREFIX: &str = "a";

/// 一个 agent 实例的标识。整棵 agent 树共用一个 store，family 的 key 就是它
/// （决策 3，`docs/STATE-MODEL.md`）。loop 的每个 event / effect 都带着它，
/// 宿主靠它知道「这个动作是替谁做的」，`Session::step` 靠它路由。
///
/// # 形状
///
/// 路径编码，`/` 分段：`root` / `root/a1` / `root/a1/a2`。
/// [`AgentId::root`] 是根，[`AgentId::child`] 往下长一段，
/// [`AgentId::parent`] / [`AgentId::depth`] / [`AgentId::is_ancestor_of`] 全是
/// 字符串运算，**一个 `store.get` 都不做**（见模块文档）。
///
/// # 为什么是 `Arc<str>` 而不是自增整数
///
/// 换类型的代价会摊到所有携带它的 event / effect / 快照 / 落盘键上，而 `Arc<str>`
/// 的克隆是指针拷贝（红线 5），路径语义又要求它能表达任意深度——整数做不到，
/// 除非再配一张「整数 → 路径」的表，那张表就成了「关系存在 store 外」的第二真值源。
///
/// `Hash` / `Ord`：它是 `AtomKey` 的一部分，`AtomFamily<K>` 要求 `K: Eq + Hash`，
/// 快照又要按键排序输出（顺序不定的快照没法逐值比对）。
///
/// 034：`agent-server` 的 `Frame { agent: AgentId, event: SessionEvent }` 信封
/// 让它第一次进了协议面（`ids.rs`/`engine::notice` 两处旧注释记的「AgentId 没有
/// TS 映射」到此不再成立）——`ts` feature 门后面导出 TS，单字段元组结构体，
/// 落成裸的 `type AgentId = string`（跟 `ToolCallId` 同一个映射，`Arc<str>` 对
/// ts-rs 透明）。`Notice` 本身仍然不带 agent 字段（那条判断不变，见
/// `engine::notice` 模块文档）——归属由 `Frame` 在外层携带，不是塞进 `Notice`。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AgentId(pub Arc<str>);

impl AgentId {
    /// root agent。**root 不走特殊路径**，它只是 id 为 `root` 的那一个
    /// （STATE-MODEL §「原子图」：没有单例 atom）——`Session::new` 给它建图用的是
    /// 和 `spawn_child` 给子 agent 建图**同一个** `build_agent`。
    pub fn root() -> Self {
        Self(Arc::from("root"))
    }

    /// 从一个已有的路径字符串造 id。**落盘/传输的反序列化入口**——事件、effect、
    /// 快照键里的 id 都是这么回来的。
    ///
    /// 不校验形状：校验点在用得着它的地方（`Session` 的每个跨 agent 口都先问
    /// 「这个 id 在不在我这棵树上」），在这里拒绝只会让「日志里有一个这一版不认识的
    /// id」变成一次 panic 而不是一条可诊断的拒绝。
    pub fn new(id: impl Into<Arc<str>>) -> Self {
        Self(id.into())
    }

    /// 路径本体。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 第 `seq` 个子 agent 的 id：`root` + `child(1)` = `root/a1`。
    ///
    /// **`seq` 的铸造不在这里**（这只是拼字符串）。谁给的号、为什么不复用，见
    /// `Session::spawn_child`：号从「这个父 agent 的 family 键空间里出现过的最大号」
    /// 往上取，于是同一个会话里**一个 id 只属于一个 agent 的一生**——复用了的话，
    /// 审计时间线上会出现两个同名 agent，日志读者分不出哪段是谁的。
    pub fn child(&self, seq: u32) -> AgentId {
        AgentId(Arc::from(format!(
            "{}{AGENT_PATH_SEP}{CHILD_SEGMENT_PREFIX}{seq}",
            self.0
        )))
    }

    /// 父 agent 的 id；root（没有分隔符）返回 `None`。
    ///
    /// 用 `rfind` 而不是 `split` 收集：只要最后一段之前的那一截，不需要把整条路径
    /// 拆成 `Vec`——这个函数在 spawn / despawn / 跨 agent 读的每次校验里都会被调到。
    pub fn parent(&self) -> Option<AgentId> {
        let cut = self.0.rfind(AGENT_PATH_SEP)?;
        Some(AgentId(Arc::from(&self.0[..cut])))
    }

    /// 离 root 有几层。root 是 0，`root/a1` 是 1，`root/a1/a2` 是 2。
    ///
    /// 就是分隔符的个数——`spawn_child` 的深度闸直接比这个数（决策 20 的「深度 ≤3」）。
    pub fn depth(&self) -> usize {
        self.0.matches(AGENT_PATH_SEP).count()
    }

    /// 我是不是 `other` 的**严格**祖先。
    ///
    /// **严格**：`a.is_ancestor_of(&a)` 是 `false`。自己不是自己的祖先，否则
    /// `read_ancestor(x, x, slot)` 会变成一个绕过 visibility 的自读后门。
    ///
    /// **按分隔符边界判**（模块文档里的那个坑）：
    ///
    /// ```
    /// use agent_core::AgentId;
    /// let a1 = AgentId::root().child(1);    // root/a1
    /// let a10 = AgentId::root().child(10);  // root/a10
    /// assert!(!a1.is_ancestor_of(&a10));    // 纯前缀匹配会在这里判错
    /// assert!(AgentId::root().is_ancestor_of(&a10));
    /// ```
    pub fn is_ancestor_of(&self, other: &AgentId) -> bool {
        let (me, them) = (self.as_str(), other.as_str());
        // `them` 必须比 `me` 长出至少「分隔符 + 一个字符」，且那个分隔符正好落在
        // `me` 的末尾之后——这两条合起来同时排掉「自己」和「root/a1 vs root/a10」。
        them.len() > me.len()
            && them.starts_with(me)
            && them[me.len()..].starts_with(AGENT_PATH_SEP)
    }

    /// [`is_ancestor_of`](AgentId::is_ancestor_of) 的反向。同样是严格的。
    ///
    /// 两个方向都给，是因为调用点各自读起来才顺：`read_ancestor` 问的是
    /// 「target 是不是 reader 的祖先」，`read_descendant` 问的是「target 是不是
    /// reader 的后代」。让其中一个调用点把参数反着写，是 `root/a1` vs `root/a10`
    /// 那类错误的另一种长法。
    pub fn is_descendant_of(&self, other: &AgentId) -> bool {
        other.is_ancestor_of(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_depth_zero_and_has_no_parent() {
        let root = AgentId::root();
        assert_eq!(root.as_str(), "root");
        assert_eq!(root.depth(), 0);
        assert_eq!(root.parent(), None);
    }

    #[test]
    fn child_appends_one_segment_and_parent_takes_it_back() {
        let root = AgentId::root();
        let a1 = root.child(1);
        let a1_a2 = a1.child(2);

        assert_eq!(a1.as_str(), "root/a1");
        assert_eq!(a1_a2.as_str(), "root/a1/a2");
        assert_eq!(a1.depth(), 1);
        assert_eq!(a1_a2.depth(), 2);
        assert_eq!(a1_a2.parent(), Some(a1.clone()));
        assert_eq!(a1.parent(), Some(root));
    }

    /// **点名用例**（issue 028 验收）：`root/a1` 不是 `root/a10` 的祖先。
    /// 纯 `starts_with` 会判成真——那是 M1 之前的旧实现踩过的坑。
    #[test]
    fn a_prefix_that_is_not_a_path_boundary_is_not_an_ancestor() {
        let a1 = AgentId::root().child(1);
        let a10 = AgentId::root().child(10);

        assert_eq!(a1.as_str(), "root/a1");
        assert_eq!(a10.as_str(), "root/a10");
        // 纯前缀匹配在这里是真——所以它不能是判据。
        assert!(a10.as_str().starts_with(a1.as_str()));

        assert!(!a1.is_ancestor_of(&a10));
        assert!(!a10.is_descendant_of(&a1));
    }

    /// 同一个坑的更深一层：`root/a1` 也不是 `root/a10/a1` 的祖先。
    #[test]
    fn the_boundary_rule_holds_deeper_down_too() {
        let a1 = AgentId::root().child(1);
        let a10_a1 = AgentId::root().child(10).child(1);

        assert!(a10_a1.as_str().starts_with(a1.as_str()));
        assert!(!a1.is_ancestor_of(&a10_a1));
        assert!(AgentId::root().is_ancestor_of(&a10_a1));
    }

    #[test]
    fn nobody_is_their_own_ancestor() {
        for id in [AgentId::root(), AgentId::root().child(1), AgentId::root().child(1).child(2)] {
            assert!(!id.is_ancestor_of(&id), "{id:?}");
            assert!(!id.is_descendant_of(&id), "{id:?}");
        }
    }

    #[test]
    fn ancestry_is_transitive_and_siblings_are_unrelated() {
        let root = AgentId::root();
        let a1 = root.child(1);
        let a2 = root.child(2);
        let a1_a1 = a1.child(1);

        assert!(root.is_ancestor_of(&a1_a1));
        assert!(a1.is_ancestor_of(&a1_a1));
        assert!(a1_a1.is_descendant_of(&root));

        // 兄弟之间两个方向都不成立——横读在 id 这一层就已经无从谈起。
        assert!(!a1.is_ancestor_of(&a2));
        assert!(!a2.is_ancestor_of(&a1));
        assert!(!a1_a1.is_ancestor_of(&a2));
        assert!(!a2.is_ancestor_of(&a1_a1));
    }

    /// 另一棵树的 id（别的 session）跟本树没有任何关系——`Session` 的每个跨 agent
    /// 口都靠这条把「不是我这棵树上的 agent」挡在外面。
    #[test]
    fn an_id_from_another_tree_is_neither_ancestor_nor_descendant() {
        let root = AgentId::root();
        let alien = AgentId::new("other/a1");
        assert!(!root.is_ancestor_of(&alien));
        assert!(!alien.is_ancestor_of(&root));
    }

    #[test]
    fn agent_id_roundtrip() {
        let id = AgentId::root().child(1).child(2);
        let s = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<AgentId>(&s).unwrap(), id);
    }
}
