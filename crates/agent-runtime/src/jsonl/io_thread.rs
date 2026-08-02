//! 真正碰文件的地方：专门的 IO 线程 + `mpsc`（issue 011 硬约束——写扔给这个线程，
//! actor 不阻塞；`agent-runtime` 里 `io_thread.rs`（provider 那个）已经是这个手法的
//! 先例，这里同名不是巧合）。
//!
//! ## 打开失败：报一次，之后静默吞
//!
//! 构造只读目录/坏路径时，`OpenOptions::open` 在这个线程启动时就会失败——报一次
//! `on_error`，`file` 记成 `None`，之后每条消息仍然照常更新内存里的 [`SessionLog`]
//! 镜像（保持行为一致，虽然没人会去读它），但**不再重复报错**：根因只有一个（这个
//! 路径写不了），报一百次和报一次传达的信息量相同，只会淹没真正有用的信号。写入本身
//! 从不 panic——`append` 等方法只是把消息塞进 channel，channel 发送失败（IO 线程已经
//! 退出）也只是静默丢弃，调用方感知不到。
//!
//! ## 快照落盘 = 压实：整份重写
//!
//! `Msg::Snapshot` 到达时，[`SessionLog::record_snapshot`] 已经把内存镜像的 `held`
//! 清空——这一刻文件里「快照之前的旧 entries」全部过时了。**用 `set_len(0)` 截断
//! 再写这一行**：`File` 是 append 模式打开的，`set_len` 缩短之后下一次 `write_all`
//! 仍然从（新的）文件尾写起，不需要额外 `seek`。压实之后文件只剩这一行，后续的
//! `Entry`/`Cursor`/`DropOldest`/`DropAfter` 正常追加在它后面——`load` 重放时见到
//! 一个 `Snapshot` 记录就重置累积器，天然只保留「最近一张快照 + 之后的日志」。
//!
//! ## 压实之后为什么不能落「原始值」
//!
//! `load()` 重放（`load.rs`）用的是一份**全新** `SessionLog`——`boundary` 从 0 起步。
//! 但这个线程里养的 `mirror` 是**连续存活**的：`boundary` 从进程启动一路累积到现在，
//! 每次快照都在往上加，从来不会回到 0（`SessionLog::record_snapshot` 的语义）。
//! `SetCursor` 收到的 `cursor` 和 `DropOldest` 收到的 `count` 都是相对 `mirror` 那个
//! **真实、累积的** `boundary` 定义的；压实截断文件之后，重放端只看得到「最近一次
//! 压实之后」这一段，它的 `boundary` 起点是 0，跟 `mirror` 的真实 `boundary`差着
//! 「更早被压实掉多少条」这一截。如果把 `cursor`/`count` 原样写进文件，重放端会用
//! 错误的起点重新做一遍换算，结果和 `mirror` 实际发生的对不上（issue 011 测试
//! `session_store_backend_choice.rs` 曾经的失败就是这个）。
//!
//! 修法：**落盘前用 `mirror` 自己已经算好的「净效果」**——`SetCursor` 落
//! [`SessionLog::relative_cursor`]（不是原始 `cursor`），`DropOldest` 落
//! [`SessionLog::record_drop_oldest`] 的返回值（这一次真正从 `held` 前端切掉了多少条，
//! 不是调用方给的原始 `count`）。两者都已经是「相对 `mirror` 当前 `held` 的净效果」，
//! 重放端从 `boundary = 0` 起步直接消费就是对的——不需要（也没有能力）还原压实之前
//! 的真实 `boundary`。`DropAfter` 不用这样处理：它是按 `seq` 谓词过滤尾部，`seq` 在
//! `mirror` 和重放端指的是同一批物理条目，没有需要换算的坐标。
//!
//! ## `mirror` 起手式必须追平已有文件（独测抓到的真 bug 1 的另一半）
//!
//! 上面这一整套「`mirror` 连续存活、`boundary`/`held` 只增不减」的推导有一个隐藏
//! 前提：`mirror` 从进程一开始就完整参与了这份文件的全部历史。「重启」恰恰打破
//! 这个前提——新进程起一条全新 IO 线程，`mirror` 如果仍然从 `SessionLog::new()`
//! 起步，它对文件里已经有的内容一无所知：`held` 是空的，但物理文件里可能已经有
//! N 条未压实的 entries。下一次 `SetCursor` 落盘时，`relative_cursor()` 算的是
//! 「这个进程自己见过多少条」而不是「文件里实际有多少条」，系统性地把游标写小；
//! 再下一次重启，`recover()` 读到一个 `cursor < entries.len()` 的会话（明明什么
//! 都没 undo 过），它自己的下一次写入被 `History` 当成「覆盖 redo 尾」处理，上一个
//! 进程真实写过的整轮对话被一条 `drop_after` 悄悄冲掉——不 panic、不报错，红线
//! 1-6 点名的那类最贵的静默错值 bug。
//!
//! 修法：`run` 起步时调 [`load::seed_from_disk`] 把 `mirror` 追平到文件已有内容
//! （跟 `load()` 走同一条重放逻辑，只是静默——见该函数文档）。追平之后
//! `boundary`/`held`/`max_seq`/`last_cursor` 就跟真实文件状态一致，这份「连续
//! 存活」的假设对「重启后新起的这条 IO 线程」才真正成立，不只是对「同一条 IO
//! 线程活到现在」成立。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

use serde::Serialize;
use serde::de::DeserializeOwned;

use agent_store::history::{Entry, Snapshot};
use agent_store::persist::SessionLog;

use super::error::SessionStoreError;
use super::load;
use super::record::Record;

pub(super) enum Msg<K, V, M> {
    Append(Entry<K, V, M>),
    DropOldest(usize),
    DropAfter { first_seq: u64, count: usize },
    SetCursor(usize),
    Snapshot(Snapshot<K, V>),
    /// 排干信号：处理到这条消息时，前面的写入必然都已经落盘（`mpsc` 是 FIFO）——
    /// `flush()`/`load()` 靠这个手法确认「不只是入队，是真的写完了」。
    Flush(Sender<()>),
}

type OnError = Arc<dyn Fn(SessionStoreError) + Send + Sync>;

pub(super) fn run<K, V, M>(path: PathBuf, rx: Receiver<Msg<K, V, M>>, on_error: OnError)
where
    K: Clone + Serialize + DeserializeOwned,
    V: Clone + Serialize + DeserializeOwned,
    M: Clone + Serialize + DeserializeOwned,
{
    let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => Some(f),
        Err(e) => {
            on_error(SessionStoreError::Io { detail: e.kind().to_string() });
            None
        }
    };
    // 追平已有文件——见模块文档「`mirror` 起手式必须追平已有文件」。这一步只在
    // 「文件已存在且能正常重放」时有实质效果；全新会话/读不出来/中部损坏都退化成
    // 一份空日志，跟以前 `SessionLog::new()` 的起手式完全一样，不改变那些场景的
    // 行为。
    let mut mirror: SessionLog<K, V, M> = load::seed_from_disk(&path);

    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Append(entry) => {
                mirror.record_append(&entry);
                write_line(&mut file, &Record::Entry(entry), &on_error);
            }
            Msg::DropOldest(count) => {
                // 落盘的是「这一次真的从 held 前端切掉了多少条」，不是调用方给的
                // 原始 count——见模块文档「压实之后为什么不能落原始值」。
                let removed = mirror.record_drop_oldest(count);
                write_line::<K, V, M>(&mut file, &Record::DropOldest { count: removed }, &on_error);
            }
            Msg::DropAfter { first_seq, count } => {
                mirror.record_drop_after(first_seq, count);
                write_line::<K, V, M>(&mut file, &Record::DropAfter { first_seq, count }, &on_error);
            }
            Msg::SetCursor(cursor) => {
                mirror.record_cursor(cursor);
                // 落盘的是换算之后的相对游标，不是调用方给的原始 cursor——同上。
                let cursor = mirror.relative_cursor();
                write_line::<K, V, M>(&mut file, &Record::Cursor { cursor }, &on_error);
            }
            Msg::Snapshot(snap) => {
                mirror.record_snapshot(&snap);
                compact::<K, V, M>(&mut file, snap, &on_error);
            }
            Msg::Flush(ack) => {
                let _ = ack.send(());
            }
        }
    }
}

/// 序列化一行、写入、失败就报一次错并把 `file` 记成 `None`（见模块文档的「报一次」
/// 策略）。`file` 已经是 `None` 时直接跳过，不重复报告。
fn write_line<K: Serialize, V: Serialize, M: Serialize>(
    file: &mut Option<File>,
    record: &Record<K, V, M>,
    on_error: &OnError,
) {
    let Some(f) = file.as_mut() else { return };
    // 序列化失败不该发生（红线 3：primitive 必须全部可序列化），这里只做防御，
    // 不当成「文件坏了」处理——不吞掉这条消息对应的记账（`mirror` 已经更新过了），
    // 只是这一行没能落盘，等下一条消息再写的时候文件就会跟内存对不上；这是
    // fire-and-forget 明确接受的风险，不在这里升级成更重的处理。
    let Ok(mut bytes) = serde_json::to_vec(record) else { return };
    bytes.push(b'\n');
    if let Err(e) = f.write_all(&bytes) {
        on_error(SessionStoreError::Io { detail: e.kind().to_string() });
        *file = None;
    }
}

/// 快照落盘：截断 + 只写这一行。`M` 单独作为泛型参数传入——`Snapshot<K, V>` 本身
/// 不带 `M`，但落盘的行类型是 `Record<K, V, M>`，构造 `Record::Snapshot` 时仍然要
/// 让编译器知道完整的三个类型参数。
fn compact<K: Serialize, V: Serialize, M: Serialize>(
    file: &mut Option<File>,
    snap: Snapshot<K, V>,
    on_error: &OnError,
) {
    let Some(f) = file.as_mut() else { return };
    if let Err(e) = f.set_len(0) {
        on_error(SessionStoreError::Io { detail: e.kind().to_string() });
        *file = None;
        return;
    }
    write_line::<K, V, M>(file, &Record::Snapshot(snap), on_error);
}
