//! command 层：**红线 2 的白名单目录**——primitive 写入的唯一合法住处。
//!
//! 业务代码禁止直接调 `store.set()`：绕过去的那次写入不进 undo log，undo 越过它时
//! 这个 atom 停在新值上、其余全部回滚，状态自相矛盾——而且是「测试全过、线上偶发」
//! 的那种矛盾。这里把「写」收成一条路：
//!
//! ```text
//! Session::step / begin_turn / set_max_* ─┐
//!                                          ├─→ Session::commit ─→ Txn::set ─→ record_set ─→ Change
//! Session::undo_turn / redo_turn ──────────┘        (一个 store.batch)              ↓
//!                                                                            History::append
//! ```
//!
//! | 文件 | 职责 |
//! |------|------|
//! | [`session`] | `Session` 结构本身与会话级命令（`new` / `begin_turn` / `set_max_*` / `mark_irreversible`） |
//! | [`read`] | 公开读口：宿主取料的地方，形状对齐 M1 的 `TurnState` 字段 |
//! | [`barrier`] | 034：`barrier_info`——描述一条屏障 entry「越过它意味着什么」，CLI 与 server 共用 |
//! | [`child_config`] | 子 agent 出生时固化的 durable 配置；live provider binding 不进 core |
//! | [`commit`] | 一次转移 → 一个 batch → 一条 `Entry` |
//! | [`txn`] | 一次转移的写入事务：类型化读写 + `record_set` 收口 |
//! | [`step`] | `Session::step`：epoch 闸 + agent 闸 + 分发 |
//! | `transitions` | 转移表本体（002/016/003 的语义，逐格搬进原子图） |
//! | [`undo`] | `undo_turn` / `undo_turn_force` / `redo_turn`，红线 6 在这里结账 |
//! | [`meta`] | `EntryMeta` 与 agent 侧三个日志类型别名 |
//! | [`restore`] | 027：崩溃恢复——从 `SessionStore::load()` 的产物重建 `Session`（恢复就是 redo） |
//! | [`tree`] | 028：这棵树上现在有哪些 agent、谁是谁的孩子、谁还活着 |
//! | [`spawn`] | 028：`spawn_child` + 结构性硬限（决策 20） |
//! | [`despawn`] | 028：`despawn_child`——019 三条硬约束的第一次真实执行 |
//! | [`skill`] | 039：`activate_skill` / `deactivate_skill` / `active_skills`——`SkillsActive` 槽位的 journaled 读写（决策 21） |
//! | [`host_tools`] | 073：`declare_host_tools` / `host_tools`——`HostTools` 槽位的 journaled 读写（宿主注入的声明是会话状态，恢复时原样复刻） |
//! | [`host_skills`] | 064：`declare_host_skills` / `host_skills`——`HostSkills` 槽位的同款读写（skill 的索引行也进 prompt，同一条理由） |
//! | [`disabled_builtins`] | 076：`disable_builtins` / `disabled_builtins`——`DisabledBuiltins` 槽位的同款读写，方向相反（**减法**：这个会话把部署方给的哪几件藏起来不给模型看） |
//! | [`cross_read`] | 028：跨 agent 读的两个口，没有第三个（红线 10） |
//!
//! ## 一个 `Session` = 整棵树
//!
//! 026 时 `Session` 只建 root 一个 agent 的图，形状已经就位（family 的键带
//! `AgentId`、构图函数按 agent 建、日志键是逻辑键）；028 加的确实只是**调用**：
//! `spawn_child` 调的是同一个 `build_agent`，子 agent 的写入走同一个 `commit_as`，
//! 落进同一条日志、继承同一个 `turn_id`。「跨 agent 的 undo 天生一致」因此不是
//! 一段代码，是没有代码。

pub mod barrier;
pub mod child_config;
pub mod commit;
pub mod cross_read;
pub mod despawn;
pub mod disabled_builtins;
pub mod host_skills;
pub mod host_tools;
pub mod meta;
pub mod read;
mod restore;
pub mod session;
pub mod skill;
pub mod spawn;
pub mod step;
mod transitions;
pub mod tree;
pub mod txn;
pub mod undo;

pub use barrier::BarrierInfo;
pub use child_config::ChildConfig;
pub use cross_read::ReadDenied;
pub use despawn::{DespawnRefused, DespawnReport};
pub use meta::{AgentChange, AgentEntry, AgentHistory, EntryMeta, known_label};
pub use session::{DEFAULT_HISTORY_CAP, Session};
pub use skill::SkillError;
pub use spawn::{AgentLimits, DEFAULT_MAX_AGENT_DEPTH, DEFAULT_MAX_CHILDREN, SpawnRefused};
pub use undo::UndoReport;
