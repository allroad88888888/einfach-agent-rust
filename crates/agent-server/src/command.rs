//! [`Command`]：外界唯一能对一个 session 说的话，经 [`crate::handle::SessionHandle`]
//! 送进 actor 线程的 `mpsc` 队列（issue 030）。
//!
//! **`Cancel` 不真的排队**——[`SessionHandle::send`](crate::handle::SessionHandle::send)
//! 特判它，直接旁路写共享的取消原子标志（复用
//! [`agent_runtime::RunnerCtx::cancel_flag`]），不经过 `mpsc`：队列里的取消要等
//! 前面所有命令处理完、轮到它才生效，那时轮次早结束了，不叫「取消」。这个变体
//! 仍然留在枚举里（而不是从 `Command` 里拿掉改成一个独立方法）是防御性的第二
//! 道闸——万一将来有调用方绕过 `SessionHandle::send`、直接把 `Command::Cancel`
//! 塞进底层 `mpsc::Sender`（比如 031 的网络层反序列化出一条命令直接转发），
//! actor 循环本身也认得这个变体、也会把它当成「翻标志」处理，只是慢一拍
//! （要排在它前面的命令处理完）。两条路径殊途同归，语义不会因为走哪条线而不同。
use serde::{Deserialize, Serialize};

/// undo/redo 的粒度（决策 5 的两档）。031 把 `POST /sessions/:id/undo` 的请求体
/// 原样搬进这里——issue 原文的 wire 形状是 `{ "granularity": "turn"|"step",
/// "force": bool }`，`Granularity` 就是那个字符串字段的类型。
///
/// `agent_core::Session` 早就两档都有（`undo_turn`/`undo_turn_force` 与
/// `undo_step`/`redo_step`，见 `agent-core/src/command/undo.rs` 模块文档），030
/// 当时的 `Command::Undo { force }` 只接了 turn 档——031 把第二档接上，不是
/// 新发明一层语义。
/// 032：`ts` feature 门后面导出 TS，无字段变体落成字符串字面量联合
/// （`"turn" | "step"`，`rename_all = "snake_case"` 照旧生效）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum Granularity {
    /// UI 默认档：一次退一整个 turn。
    Turn,
    /// 开发者档：一次退一条 entry（可展开的时间线）。
    Step,
}

/// 一条对 session 的命令。**这是协议雏形**——032 从这里生成 TS 类型，字段形状
/// 就是未来 `POST /sessions/:id/*` 的请求体（ARCHITECTURE.md §传输）。
///
/// 032：没有 `#[serde(tag = ..)]`，是 serde 默认的外部标签——`Redo`/`Cancel`/
/// `Shutdown` 这类无字段变体落成裸的字符串字面量（`"Redo"`），带字段的变体落成
/// 单键对象（`{ "Input": string }`、`{ "Undo": { granularity, force } }`）。跟
/// [`SessionEvent`](crate::SessionEvent) 的邻接标签是两套不同形状，**都原样照抄
/// 现有 serde 属性**，不统一。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum Command {
    /// 一句用户输入，喂给 [`agent_runtime::run_turn`] 跑一整轮。
    Input(String),
    /// 撤一轮或一条 entry（[`Granularity`]）。`force = true` 越过第一条屏障
    /// （对应 CLI 的 `/undo!`，只对 `Granularity::Turn` 有意义——`Session` 没有
    /// `undo_step` 的 force 变体，`Granularity::Step` 时这个字段被忽略，见
    /// `crate::actor::commands::handle_undo` 模块文档），`false` 撞屏障就停
    /// （对应 `/undo`）。
    Undo { granularity: Granularity, force: bool },
    /// 反演一次 undo（turn 粒度——ARCHITECTURE.md §传输 的 `POST .../redo` 请求体
    /// 是空对象，没有 `granularity` 字段，031 原样照办）。
    Redo,
    /// 取消当前在飞的轮次——**不排队**，见本文件模块文档。
    Cancel,
    /// 优雅关闭：处理完队列里排在它前面的命令后，落最后一次持久化、退出线程。
    Shutdown,
}
