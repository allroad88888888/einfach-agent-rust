//! 原子图的**地址空间**：[`AtomKey`]（落盘的逻辑键，红线 4）与它的两级槽位枚举。
//!
//! 这个文件只回答一个问题：**「一个槽位怎么称呼」**。「它没有值的时候是什么」在
//! 同目录的 [`slot_default`](super::slot_default)（107 加 `Slot::Summaries` 时拆出去
//! 的——本文件原本同时答两个问题，那就是两件事），「谁来建它」在
//! [`build`](super::build)，「谁来写它」在 `command/`。
//!
//! ## 为什么键是逻辑键
//!
//! `AtomId` 是自增 `u64`，完全依赖创建顺序：快照存 `(AtomId, Value)` 的话，只要有人
//! 往构图函数中间插一行 `create_atom`，所有旧快照的值就整体错位——**而且不报错**
//! （红线 4）。`AtomKey` 是「怎么还原」（`Slot`）+「还原哪一个」（`AgentId`），
//! 与创建顺序无关，于是快照能跨进程、日志能跨版本、019 的按需重建才有依据
//! （拿不到 `Slot` 就不知道该建什么）。
//!
//! 顺带白拿 schema 演进：新增槽位在旧快照里找不到键，用 [`Slot::default_value`]；
//! 删掉的槽位在快照里是多余项，忽略即可。不需要迁移脚本。
//!
//! ## `Slot` 还是个子集，`AtomKey` 不是
//!
//! `Slot` 照 `docs/STATE-MODEL.md` 的槽位表**裁剪**到真的有写入点的那些：
//! `config` / `system_base` / `skills_active` / `tools_registry_version` 现在没有任何
//! 写入点，先不定（021 的教训：没被真实使用验证过的槽位，跟没写一样，只是它看起来
//! 像做完了）。028 只加了一个 [`Slot::ToolsAllowed`]——它有写入点
//! （`Session::spawn_child`）也有读者（029 的子 agent 工具表 + 活名单判定）。
//!
//! 每个槽位还要回答第三个问题「别的 agent 能不能读它」，那是隔壁
//! [`visibility`](super::visibility) 的事（红线 10）。
//!
//! `AtomKey` 的**两个变体一个不少**，即使 M2 只构造 `Agent` 那一支：它是落盘键的
//! 类型，改它的形状等于让所有旧日志/快照解不出来。`Slot` 可以往里加（旧快照缺键
//! 用默认值），`AtomKey` 的变体集合不能事后改——两者的稳定性要求不是一个量级。

use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, ToolCallId};

/// 一个 agent 的槽位。**只有 source（primitive）槽位**——derived 不进日志、不进
/// 快照，它们的键是 [`DerivedKey`]，两套键分开正是为了让「快照只存 primitive」
/// 成为类型上的事实而不是纪律。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum Slot {
    /// 消息历史。
    Messages,
    /// 这一轮走到哪了。
    Status,
    /// 本轮的工具槽，顺序 = 模型请求顺序。
    ToolSlots,
    /// 上一次请求的前缀镜像；第一轮之前是 [`AgentValue::Null`]。
    PrevPrefix,
    /// 下一个要铸的 `MessageId`（从 1 起严格递增）。
    NextMessageId,
    /// 本轮已经发起的 `CallProvider` 次数（新一轮和重试都算）。
    TurnsUsed,
    MaxTurns,
    /// 当前这条失败-重试链已经连续失败了几次。
    RetriesUsed,
    MaxRetries,
    /// **spawn 当时快照的工具子集**（028 唯一新增的槽位，029 消费）。
    ///
    /// 值是 [`AgentValue::Json`] 里的一个字符串数组，`Null` = 「这个 agent 不在
    /// 活名单上」。两件事共用一个槽位不是省事，是它们本来就是一件事：
    /// **「这个 agent 是被 spawn 出来的，带着这份工具子集」**——`Null` 是这个事实
    /// 的缺席，不是第二个字段。于是「从没 spawn 过」「spawn 被 undo 掉了」
    /// 「已经 despawn」三种情况在状态上完全一致，因为它们**就是**同一种状态。
    ///
    /// 为什么是 spawn 当时的快照而不是现查工具表：和 `ToolCallSlot::Request` 存
    /// 发起时 `Reversibility` 是同一个道理（issue 006 §注意）——undo 回到 spawn
    /// 那一刻，用的必须是当时的工具表，不是现在的。
    ///
    /// 排序去重后落盘（红线 11）：它会被渲染进子 agent 的 prompt，顺序一漂前缀
    /// 缓存就全价。写入点在 `Session::spawn_child`。
    ToolsAllowed,
    /// **当前激活的 skill id 列表**（039）。值是 [`AgentValue::Json`] 里一个
    /// **排序去重的字符串数组**，`Json([])` = 没有激活任何 skill（默认值）。
    ///
    /// store 里只存「哪些被激活」，skill 的正文/工具在 store 外的 registry 里
    /// （TOOLS.md §Skills；也是 `AtomKey` 没有 `Skill` 变体的原因）——于是
    /// undo 撤一次激活就退化成一次普通的值回滚（跟 `ToolsAllowed` 一视同仁），
    /// 崩溃恢复靠这个 primitive 自动回来、正文从 registry 现取。
    ///
    /// 为什么是排序去重的数组而不是 `HashSet`（红线 11）：它会被 registry 展开成
    /// 注入进 system prompt 的正文，顺序一漂前缀缓存就全价。写入点在
    /// `Session::activate_skill` / `deactivate_skill`，那两处落值前排序去重。
    SkillsActive,
    /// **宿主建会话时声明的工具**（073）。值是 [`AgentValue::Json`] 里一个
    /// **按名字排序的对象数组**（`value::host_tools` 那一处编解码），
    /// `Json([])` = 这个会话没有任何注入（默认值）。
    ///
    /// 跟 [`Slot::SkillsActive`] **同构**：声明（可序列化的静态描述）在 store，
    /// 执行（真的去跑这个工具）在宿主侧。差别只在存的是什么——skill 存 id、正文
    /// 从 registry 现取，注入的工具**连描述和 schema 一起存**：它们是宿主这一次
    /// 报进来的、store 外没有第二份，取不回来就没有别处可取。
    ///
    /// 为什么必须进 store 而不是每次建会话时由宿主重报（用户 2026-08-04 拍板）：
    /// **历史对话是在那一份工具表下产生的**，恢复时装上宿主今天的新清单，历史就
    /// 自相矛盾（模型当初说「我调了 `web:crm/lookup`」，而今天的清单里可能没有
    /// 它了）；而且工具表在 prompt 最前面，换一份 = 恢复出来的第一轮前缀全断
    /// （红线 11）。恢复是忠实重放，不是用今天的配置重建。
    HostTools,
    /// **宿主建会话时声明的 skill**（064）。值是 [`AgentValue::Json`] 里一个
    /// **按 id 排序的对象数组**（`value::host_skills` 那一处编解码），
    /// `Json([])` = 这个会话没有任何注入的 skill（默认值）。
    ///
    /// 跟 [`Slot::HostTools`] 同一条理由（声明是会话状态、索引行进 prompt 最前面、
    /// 恢复是忠实重放），另外还有两条是 skill 独有的：
    ///
    /// - [`Slot::SkillsActive`] **早就在 store 里了**。声明不落盘，恢复出来就是一份
    ///   指向空 registry 的激活集——状态说某个 skill 激活着、展开注入却什么都取不到
    ///   （查不到的 id 静默跳过），而模型的历史里明明写着它读过那段正文。
    /// - 073 之后有历史的会话**不接受再声明**（400 `session_has_history`），所以
    ///   不存下来就是永久没了，连「重连时重报一遍」这条退路都不存在。
    HostSkills,
    /// **这个会话关掉了哪些内置工具**（076）。值是 [`AgentValue::Json`] 里一个
    /// **排序去重的字符串数组**（跟 [`Slot::SkillsActive`] 共用 `value::str_set`
    /// 那一处编解码），`Json([])` = 一个都没关（默认值，也就是今天的行为）。
    ///
    /// 前三个 Host* 槽位是**加法**（宿主报进来的能力），这一个是**减法**：它列的
    /// 名字必须在部署方装配出来的那张表里，装表时整条剔掉，于是那些工具**连名字
    /// 带描述都不进 prompt**，模型压根不知道有它。
    ///
    /// 为什么它跟声明一样必须进 store（073 那三条原样成立）：历史对话是在**那一份
    /// 减过的表**下产生的；工具表在 prompt 最前面（红线 11），恢复时按今天的开关
    /// 重建 = 第一轮前缀全断；恢复是忠实重放，不是用今天的配置重建。
    ///
    /// **默认值必须是空数组**——019 的按需重建拿的就是它，若默认成别的，undo 路径上
    /// 凭空重建出来的 atom 会把一个从没关过任何东西的会话的工具表悄悄削掉几项。
    DisabledBuiltins,
    /// 子 agent 出生时固化的不透明执行 profile id（093）。
    ///
    /// `Text(id)` 只负责恢复后还能指向同一个 runtime registry 项；core 不知道该项
    /// 里是哪家 provider、哪个模型或哪份凭证。`Null` 是 root、既有默认 spawn 与
    /// 缺少本槽位的旧 snapshot 的兼容值。
    ExecutionProfile,
    /// **这一轮实际要发给 provider 的历史坐标**（099/100，M12 压缩主干）。
    ///
    /// 值是 [`AgentValue::Json`] 里 [`SendPlan`] 自身的序列化（`send_plan_codec`）
    /// ——跟这张表里最近几个槽位一样，是复用既有变体而不是新开一个：`AgentValue`
    /// 的变体集合在 026 定死之后只增 `Slot`、不增变体（`atom_value.rs` 模块注释）。
    ///
    /// 默认值是 [`SendPlan::new()`] 的编码——**恒等元**，[`send_plan::project`]
    /// 投影它等于完整历史（099 验收）。这正是「不用这个功能就逐字节不变」在这个
    /// 槽位上的落点：没写过 `SendPlan` 的 agent，`send_plan_of` 永远读到这份编码，
    /// `encode` 的入参因此跟 100 落地之前逐字节相同。
    ///
    /// [`send_plan::project`]: crate::value::send_plan::project
    SendPlan,
    /// **上一次 `CallProvider` 实际用的那份 [`SendPlan`]**（103：兜底第 1 层
    /// `PrefixIntent` 的判定材料，同 `Slot::PrevPrefix`「上一次发出去的长什么样」
    /// 的另一半，同一时刻在 `provider_done` 一起写）。跟当前 `SendPlan` 不等 ⇒
    /// 中间压缩改过计划 ⇒ 漂移是预期内的，不是事故。默认值同 `Slot::SendPlan`
    /// （pristine 编码）。
    PrevSendPlan,
    /// **这个 agent 历次压缩产出的摘要正文**（107，M12 压缩第 3 档的落点）。
    ///
    /// 值是 [`AgentValue::Json`] 里一个 `[[id, 正文], …]` 数组（`value::summaries`
    /// 那一处编解码），`Json([])` = 从没压过（默认值）。[`Slot::SendPlan`] 里只有
    /// 引用（`SummaryId`），正文住这里——**大值不进 `SendPlan`**（红线 5：
    /// `SendPlan` 每轮都要被读出来投影一次，它的序列化大小不该随摘要长度增长）。
    ///
    /// 为什么容器是 `Vec` 而不是 map（红线 11）、为什么正文是 `Arc`（红线 5）、
    /// 为什么**只增不删**（回收了 redo 就取不回正文，投影会把边界作废），三条
    /// 理由写在 [`value::summaries`](crate::value::summaries) 的模块文档里，
    /// 不在这里重复一遍。
    ///
    /// 跟 `Slot::SendPlan` 分成两个槽位而不是把正文塞进那一个：一次压缩要同时改
    /// 两个槽位，而它们由**同一条 command**（`Session::apply_summary`）在同一个
    /// batch 里写完，落成一条 `Entry`——所以「两个槽位」不会长出「边界推了但摘要
    /// 还没进库」的中间态，换来的是「推边界」那条 entry 的 `prev` 不必抄一份正文。
    Summaries,
}

/// 一次工具调用自己的槽位。
///
/// M2 只有 `Result` 一个：`Request`（发起当时的 `Location` / `Reversibility` 快照，
/// STATE-MODEL §「落盘的键必须是 AtomKey」）要等**持有工具表的宿主**来记——core
/// 没有工具表，现造一份占位快照是编造（002 合并时的裁决：假的 `Irreversible`
/// 会让 undo 白拦一次 `fs/read`，正是静默错值）。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum ToolCallSlot {
    /// 在飞时持 [`AgentValue::Pending`]，回来后持内容。
    Result,
}

/// 落盘的逻辑键。`Snapshot` 与 `Entry.changes` 用它，`AtomId` 只在进程内有效。
///
/// **只有两个变体**。没有 `Skill(SkillId)`——skill 的内容在 store 外的 registry 里，
/// store 里只有「哪些被激活」，那是某个 `Agent(_, _)` 槽位（STATE-MODEL）。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum AtomKey {
    Agent(AgentId, Slot),
    ToolCall(AgentId, ToolCallId, ToolCallSlot),
}

impl AtomKey {
    /// 这个键属于哪个 agent。`undo` 不看它（一条扁平日志按时间排序），
    /// 逐出与 UI 时间线看它。
    pub fn agent(&self) -> &AgentId {
        match self {
            AtomKey::Agent(a, _) | AtomKey::ToolCall(a, _, _) => a,
        }
    }
}

impl Slot {
    /// 一个 agent 的全部 source 槽位。`Session::new` 建图、`Session::primitives`
    /// 出快照都用它——**新增槽位只要加进这个数组，两条路径自动跟上**，
    /// 忘了改其中一条正是「快照缺一块」的来源。
    ///
    /// 新槽位**追加在末尾**：旧快照里找不到新键，按 [`Slot::default_value`] 落值
    /// （schema 演进白拿的那一条），而追加不改动既有槽位的相对次序，
    /// 快照的排序输出因此在版本之间是稳定的。
    pub const ALL: [Slot; 18] = [
        Slot::Messages,
        Slot::Status,
        Slot::ToolSlots,
        Slot::PrevPrefix,
        Slot::NextMessageId,
        Slot::TurnsUsed,
        Slot::MaxTurns,
        Slot::RetriesUsed,
        Slot::MaxRetries,
        Slot::ToolsAllowed,
        Slot::SkillsActive,
        Slot::HostTools,
        Slot::HostSkills,
        Slot::DisabledBuiltins,
        Slot::ExecutionProfile,
        Slot::SendPlan,
        Slot::PrevSendPlan,
        Slot::Summaries,
    ];
}

/// derived atom 的键。**刻意不 derive serde**：derived 不进日志也不进快照
/// （它们全部可重算，这正是「完整状态 = 所有 primitive」成立的原因），给它一个
/// `Serialize` 就是给「把算出来的值也存一份」开了口子。
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum DerivedKey {
    /// 「本 agent 的工具槽全都不是 `Pending` 了吗」。003 预言的那个 derived。
    ToolsConverged(AgentId),
}
