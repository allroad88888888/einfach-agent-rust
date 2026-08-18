//! 一个 agent 的**槽位**：[`Slot`]——「一个槽位怎么称呼」，这个文件只回答这一个
//! 问题。
//!
//! 「它没有值的时候是什么」在同目录的 [`slot_default`](super::slot_default)
//! （107 加 `Slot::Summaries` 时拆出去的），「落盘的键长什么样」在
//! [`atom_key`](super::atom_key)（154 加 `Slot::HostPrefix` 时从本文件拆出去
//! 的——`AtomKey` 是「哪个 agent / 哪次工具调用」+ `Slot`，`Slot` 只是它的一部分，
//! 两者天然是两件事），「谁来建它」在 [`build`](super::build)，「谁来写它」在
//! `command/`。
//!
//! ## 为什么键是逻辑键
//!
//! `AtomId` 是自增 `u64`，完全依赖创建顺序：快照存 `(AtomId, Value)` 的话，只要有人
//! 往构图函数中间插一行 `create_atom`，所有旧快照的值就整体错位——**而且不报错**
//! （红线 4）。[`AtomKey`](super::AtomKey) 是「怎么还原」（`Slot`）+「还原哪一个」
//! （`AgentId`），与创建顺序无关，于是快照能跨进程、日志能跨版本、019 的按需重建
//! 才有依据（拿不到 `Slot` 就不知道该建什么）。
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
//! [`visibility`](super::visibility) 的事（红线 10）。「`AtomKey` 的变体集合为什么
//! 不能像 `Slot` 一样随便加」写在 [`atom_key`](super::atom_key) 的模块文档里。

use serde::{Deserialize, Serialize};

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
    /// **`#[deprecated]` 精神，槽位留壳（141，决策 27）**：曾经的「当前激活的
    /// skill id 列表」（039）。值仍是 [`AgentValue::Json`] 里一个**排序去重的
    /// 字符串数组**，`Json([])` = 没有激活任何 skill（默认值）——**141 起没有任何
    /// 写入点**：`Session::activate_skill` / `deactivate_skill` 已删，模型再也
    /// 没有办法往这个槽位写东西。
    ///
    /// 变体本身**不能删**：红线 4（落盘用 `AtomKey`），老会话的 journal 里真有
    /// `activate_skill` entry 写过这个槽位，删变体等于让那些快照反序列化直接断。
    /// 读口（[`Session::active_skills`](crate::command::Session::active_skills) /
    /// `active_skills_of`）也还在——老会话恢复回来，这个槽位里的值原样读得出来，
    /// 但**没有任何生产代码再拿它去组下一轮的请求体**（`agent-runtime` 那个曾经
    /// 把激活集展开成注入料的方法已随 141 删掉）。这是一处如实的行为变化，不是
    /// 「兼容留一半」——恢复老会话之后继续对话，模型不会再看到那个 skill 的正文。
    ///
    /// skill 的正文/工具从 039 起就不在这里（store 外的 registry，TOOLS.md
    /// §Skills；也是 `AtomKey` 没有 `Skill` 变体的原因）——这条没变。
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
    /// **会话创建期定下的一列 system 前缀块**（134）。值是 [`AgentValue::Json`] 里
    /// 一个 `[[label, text], …]` 数组（`value::prefix_chunks` 那一处编解码），
    /// `Json([])` = 这个会话没有前缀块（默认值，也就是 134 之前的行为）。
    ///
    /// core 只知道「system 段前面挂着这么一列带 label 的文本，创建期定下、之后
    /// 不变」，**不知道它是谁算出来的**（红线 12 的精神：core 里没有「时机」
    /// 「skill」这些词）。写入点只有一个：`Session::set_prefix_chunks`，宿主建
    /// 新会话时调一次。
    ///
    /// 为什么必须进 store 而不是每次恢复时重算：重算依赖外部世界，这一次给出的
    /// 结果不保证跟当初一样——历史里的对话是在 A 前缀下产生的、恢复出来的会话
    /// 挂着 B 前缀，而前缀在 prompt 最前面（红线 11），缓存当场全断、上下文跟
    /// 历史对不上，两样都不报错。**恢复是忠实重放，不是拿今天的世界重建**
    /// （跟 [`Slot::HostTools`] 同一条）。
    ///
    /// 为什么容器是 `Vec` 而且**不排序**（跟 [`Slot::HostSkills`] 刻意不同）：
    /// 顺序本身是信息——它就是这些块在 system 段里该出现的先后。红线 11 要的是
    /// 「确定」不是「排序」，而确定性这里由「一次写定、之后不改」的写入点保证。
    /// 完整论证在 [`value::prefix_chunks`](crate::value::prefix_chunks) 的模块文档。
    PrefixChunks,
    /// **spawn 当时快照的「开局产物」授予名单**（144，决策 28 的 core 半边）。
    ///
    /// 值同 [`Slot::ToolsAllowed`]——[`AgentValue::Json`] 里一个字符串数组，
    /// `Null` = **不设限 = 全带**。跟 `ToolsAllowed` 的 `Null` 语义刻意不同：
    /// 这里没有「活着/死了」的一面，`Null` 单纯是「这个子 agent 没被限定子集，
    /// 拿到父 agent 能给的全部」——判活名单继续、只能继续由 `ToolsAllowed` 一个
    /// 槽位负责（见 [`visibility`](super::visibility) 对它俩方向相同但理由不同
    /// 的说明）。core 不知道「开局产物」具体是什么（红线 12 的精神：它只是一份
    /// spawn 时刻定死的名单，装的是工具结果、skill 正文还是别的，是 145 模型面
    /// 才回答的问题）。
    ///
    /// 为什么是 spawn 当时的快照而不是现查（跟 `ToolsAllowed` 同一条理由，
    /// issue 006 §注意）：undo 回到 spawn 那一刻，用的必须是当时能给的名单，
    /// 不是现在的——「从没 spawn 过」「spawn 被 undo 掉了」「已经 despawn」
    /// 三种情况下这个槽位该看起来完全一致，那正是「随 `Session::spawn_child`
    /// 那一条命令一起落盘」换来的，不是靠约定各处保持同步。
    ///
    /// 排序去重后落盘（红线 11），编解码复用 `value::str_set`——跟
    /// `ToolsAllowed`/`SkillsActive`/`DisabledBuiltins` 是同一个「有序字符串集
    /// 当值」的形状，只有一处编解码。写入点只有一个：`Session::spawn_child`。
    PrefixAllowed,
    /// **宿主经 `capabilities.prefix` 声明的开局块**（154，决策 31 的状态位）。
    /// 值是 [`AgentValue::Json`] 里一个 **按 name 排序的 `[[name, text], …]` 数组**
    /// （`value::host_prefix` 那一处编解码），`Json([])` = 这个会话没有声明任何
    /// 开局块（默认值）。
    ///
    /// **这不是拼进 system 段的那份文本**——那是 [`Slot::PrefixChunks`]（134），
    /// 由 155 把这份声明合成的常量文本 timed 工具、经 `run_session_start` 实际
    /// 落块。这个槽位存的是**原始声明本身**：跟 [`Slot::HostTools`] 同构，
    /// 声明是**会话状态**，建会话时 journaled 写进 store，恢复时从日志回放自动
    /// 回来，宿主重连时不必也不该再声明一遍——155/156 的装配/HTTP 层拿它做
    /// 恢复期判定（这个会话是不是已经声明过、声明的是不是这一份）。
    ///
    /// 跟 [`Slot::PrefixChunks`] 的差别不只是「这份是声明、那份是落块」，也在
    /// 「顺序是否可信」：`PrefixChunks` 的输入是 core 内部一次写定的顺序，顺序
    /// 本身是信息，不排序；这里的输入是宿主一次 HTTP 请求里的
    /// `capabilities.prefix` 数组，跟 073 的工具声明同一个不可靠来源，所以照
    /// `HostTools` 的先例**按 name 排序**再落盘。core 不知道这些文本是谁算出来
    /// 的、也不知道它们会被怎么用（红线 12 的精神）——那是 155/156 装配期的事，
    /// 这里只是一处「声明」的状态位。
    HostPrefix,
    /// **别的 agent 投进来、本 agent 还没消费的消息**（205，决策 35）。值是
    /// [`AgentValue::Json`] 里一个 `[[from, text, when], …]` 数组
    /// （`value::inbox` 那一处编解码），`Json([])` = 收件箱是空的（默认值）。
    ///
    /// **两档送达时机共用这一个槽位**，靠每条自带的 `when` 区分（`Deliver::Now`
    /// 加入本轮 loop / `Deliver::NextTurn` 这一轮结束之后才送达）。不拆成两个槽位
    /// 是因为它们的落盘、恢复、undo、可见性**逐字相同**，差别只有「哪个定点来收」
    /// ——拆开就要把那四样各写一遍，而它们必须永远一致。
    ///
    /// **站 `Private`**（见 [`visibility`](super::visibility)）：**发得进去 ≠ 读得
    /// 出来**。A 能往 B 的收件箱投递（那是一条命令，不是读），但 A 读不到 B 的
    /// 收件箱——包括自己投的那条被没被消费。要确认就等对方回一条，跟人一样。
    /// 一旦开成 `Shared`，「谁给谁发过什么」就成了所有人都订阅得到的东西。
    ///
    /// **默认值必须是空数组**——019 的按需重建拿的就是它，若默认成别的，undo 路径上
    /// 凭空重建出来的 atom 会给一个从没收到过消息的 agent 平添几句话，而那几句话
    /// 会被排空进它的 `Messages`、从此每轮都进 prompt。链通、值错、不报错。
    Inbox,
    /// **这个 agent 自己记的东西**（209，决策 35 §三）。值是
    /// [`AgentValue::Json`] 里一个按 key 升序的 `[[key, value], …]` 数组
    /// （`value::notes` 那一处编解码），`Json([])` = 草稿纸是空的（默认值）。
    ///
    /// 这是**整张表里唯一属于模型自己的一格**。其余每一格都是别人的账——
    /// `MaxTurns` 是部署方的、`ToolsAllowed` 是父给的、`SendPlan`/`Summaries`
    /// 是 adapter 的、`Status` 是父要读的。用户要「改本 agent 状态」，正确的
    /// 形状是给它一个自己的槽位，而不是给现有槽位开写口：**那等于让被约束者
    /// 改自己的约束**。
    ///
    /// 新槽位不碰任何现有不变量，而且白拿全套机制：`/undo` 连带撤销、崩溃恢复
    /// 自动带回、审计看得到每一次改。这是本仓架构直接掉出来的，不是新造的机制。
    ///
    /// **站 `Private`**（见 [`visibility`](super::visibility)）：只有它自己读得到、
    /// 写得到。开成 `Shared` 听起来方便，但横读全开之后那就是**所有人都读得到**，
    /// 而且「一个 agent 改一个 key」会变成影响别人下一轮 prompt 的事，模型完全
    /// 看不到这条因果。要共享上下文有 `Messages`，要传话有 `Slot::Inbox`。
    ///
    /// **容器必须有序**（红线 11）：它以 tool_result 的形式进 prompt。`HashMap`
    /// 写起来一样、功能完全正常，只是每一轮全价且不报错——所以内存里那一份是
    /// [`Notes`](crate::value::notes::Notes)（`BTreeMap` 的别名），落盘那一份是
    /// 按 key 升序的数组。
    ///
    /// **默认值必须是空数组**——019 的按需重建拿的就是它（同 `Inbox` 那条理由）。
    Notes,
    /// **这个 agent 此刻在等谁**（212 追加）：等待图的一行。值是
    /// [`AgentValue::Json`] 里一个 `[[target, until], …]` 数组（`value::awaiting`
    /// 那一处编解码），`Json([])` = 谁也没在等（默认值）。
    ///
    /// **站 `Private`**（见 [`visibility`](super::visibility)）：它是内部账本，
    /// 开成 `Shared` = 所有人都订阅得到「谁在等谁」——那不是任何一个已知需求要的。
    ///
    /// **为什么必须是状态，不是内存里的一张表**：查环要遍历这张图，而恢复之后
    /// 还得查得了环。放内存里，一次崩溃恢复就把查环能力丢了，而丢了不报错——
    /// 恢复出来的会话上，一条本该被拒的反向 `await` 会被放行，然后两个 agent
    /// 互相等到天荒地老，泵安静地返回，没有 panic、没有超时、没有告警。
    ///
    /// **有序**（红线 11）：它进 `await` 的**拒绝文本**（把环上那条链原样列出来
    /// 给模型看），所以落盘与读回都必须逐字节确定，`value::awaiting::to_value`
    /// 按 target 排序。
    ///
    /// 写入点在 `command/await.rs`（`await_on` / `clear_await`）。
    AwaitingOn,
}
