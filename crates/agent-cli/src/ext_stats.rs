//! `ext:stats`——第一个**真扩展包**（149，教材见 docs/EXTENSIONS.md §「写你的第一个
//! 扩展包」）。这个文件管「包长什么样、怎么装、钩子往哪写」；正文渲染在
//! [`crate::ext_stats_report`]（一个文件一件事）。
//!
//! 包里两条，正好对应扩展能力面的两条路（EXTENSIONS.md §一）：
//!
//! | 条目 | 谁发起 | 干什么 |
//! |---|---|---|
//! | `ext:stats/report`（截获式，交 `Aftermath::Nothing`） | 模型自主调 | 读账本，回一段「这个会话至今干了什么」 |
//! | `ext:stats/audit`（`TurnEnd` timed） | runtime，每个完成轮 | 往 `<session>.audit.log` 追加一行 |
//!
//! # 落点为什么在 `agent-cli` 里、为什么是开关不是 feature
//!
//! 149 的原文给了「feature 门」和「`--ext-stats` 开关」两个选项，取**开关**：
//! feature 门要 `cargo build --features` 才切得动，而这条 issue 的验收第 4 条是
//! 「不装包的会话逐字节零变化」——同一个二进制、跑两次、一次带开关一次不带，是这句
//! 话最直接的证据；feature 门下这两次是两个二进制，「零变化」就得靠比对两份构建产物
//! 说话，反而更绕。不新开 crate 同样照 issue：第一个扩展包先证明接缝。
//!
//! # 「什么都没碰」的举证（201 起：举证责任在包作者，落点是交不交还原函数）
//!
//! `report` 三条都成立：**纯读**（[`crate::ext_stats_report::render`] 只收 `&Session`，
//! 一条 command 都不发）、**不落 entry**、**没有需要补偿的动作**。153 之前它还有一处
//! 副作用——把读到的数字记进 [`Ledger`] 给 `TurnEnd` 钩子传话；153 落地之后 `audit`
//! 自己能在轮末现读账本，那格传话被整个删掉，`report` 从此**连这一处副作用都没有**：
//! 重放一次算出的是同一份数字，世界一个字节都不变。
//!
//! # 153（决策 30）：`TurnEnd` 钩子现在拿只读 `&Session`，轮末现读
//!
//! 149 dogfood 时 `TimedRun` 还是 `Fn(&ToolTable, &Value) -> Result<...>`——钩子拿不到
//! `Session`，于是「每轮把 entry 数写进审计文件」只能靠 `report`（有 `&mut Session`）
//! 把数字经 [`Ledger`] 递过来，审计行还得标注这份数字是哪一轮观测的（`seen_at=`）。
//! 153 把 `&Session`（只读）加进 `TimedRun` 签名之后，那整套传话与标注都不再需要：
//! `audit` 在轮末直接拿这个参数现读一次账本（[`ext_stats_report::count`]，与 `report`
//! 共用同一份计数逻辑），审计行因此变成 `turn=N entries=X/Y agents=Z tools=W`——四个
//! 数字都是**这一轮轮末那一刻**的真实状态，不再需要区分「有没有人观测过」。
//! `Ledger` 因此收窄为只管两件事：轮序号（`kill -9` 后续号）与审计文件路径。

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use agent_core::{AgentId, Session, ToolSpec};
use agent_runtime::{
    Aftermath, CallTiming, ExtensionPack, PendingInterceptors, SessionToolFn, TimedRun, ToolTable,
};
use serde_json::{Value, json};

use crate::ext_stats_report;

/// 包名。进每条工具的全名（`ext:<pack>/<tool>`），装配期硬闸按它逐字校验。
pub const PACK: &str = "stats";
/// 截获式读工具的全名。
pub const REPORT_TOOL: &str = "ext:stats/report";
/// `TurnEnd` 钩子的全名。**不进 prompt**（timed 区对 `specs()` 不可见），但它跟
/// specs 区共用同一个名字空间，所以照样吃 `ext:` 前缀强制。
pub const AUDIT_TOOL: &str = "ext:stats/audit";

/// 命令行开关。**缺省关**——不给 `--ext-stats` 的会话一个字节都不变（验收 4）。
///
/// 收参数而不是自己读 `std::env::args()`，同 [`crate::session_path::resolve`]：
/// 测试要能喂一份夹具参数。
pub fn enabled(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--ext-stats")
}

/// 审计文件放哪：**会话文件旁**，路径整体追加 `.audit.log`
/// （`a/b/s.jsonl` → `a/b/s.jsonl.audit.log`）。
///
/// 追加而不是 `with_extension`：`with_extension("audit.log")` 会把 `s.jsonl` 换成
/// `s.audit.log`，那个名字可能撞上另一个会话文件；整体追加则永远撞不了，而且一眼看得出
/// 它属于哪份会话。临时会话（没有 `--session`）没有落点 → `None`，钩子照跑但不写盘。
pub fn audit_path(session_file: Option<&Path>) -> Option<PathBuf> {
    let path = session_file?;
    let mut name = path.as_os_str().to_os_string();
    name.push(".audit.log");
    Some(PathBuf::from(name))
}

/// 装配入口：**`on` 为假时原样把表还回去**，`with_extension` 一次都不调。
///
/// 「零变化」因此是结构性的，不是「装了但装了个空包」——空包也会往
/// `host_reversibility` 里插键、也会造一个必须 install 的 [`PendingInterceptors`]。
/// 返回的第二半是 `None`（没装）或 `Some`（装了，**调用方必须 install**，忘了会在
/// `Drop` 里 `debug_assert!` 炸，见 EXTENSIONS.md §防呆）。
pub fn install(
    table: ToolTable,
    on: bool,
    session_file: Option<&Path>,
    note: &mut dyn FnMut(&str),
) -> (ToolTable, Option<PendingInterceptors>) {
    if !on {
        note("关（默认）。工具表逐字节与不装扩展时相同。");
        return (table, None);
    }
    let audit = audit_path(session_file);
    match &audit {
        Some(p) => note(&format!("开。审计文件={}", p.display())),
        None => note("开。**没有会话文件**：审计钩子照跑但无处落盘（用 --session 指定）。"),
    }
    let ledger = Ledger::new(audit);
    let (table, pending) = table.with_extension(pack(ledger));
    (table, Some(pending))
}

/// 组包本身。条目顺序 = 这里写死的 push 顺序（红线 11，包内不排序）。
pub fn pack(ledger: Arc<Ledger>) -> ExtensionPack {
    ExtensionPack::new(PACK)
        .with_tool(report_spec(), report_run())
        .with_timed(audit_spec(), CallTiming::TurnEnd, audit_run(ledger))
}

/// 喂给模型的声明。**不带任何会变的字节**（红线 11：描述进 prompt，每一轮都在）。
pub fn report_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from(REPORT_TOOL),
        description: Arc::from(
            "看一眼这个会话到目前为止的账本汇总：开过几轮、账本上有多少条 entry、\
             你这一支有几个 agent、工具调用了多少次，外加 entry 的种类分布和你的\
             子 agent 列表。**当场返回，不阻塞、不改任何状态**。\n\
             什么时候用：用户问「这个会话到目前为止干了什么」「我们聊了多少轮」\
             「你调了几次工具」「撤销之后还剩什么」这一类关于**会话自身**的问题——\
             凭印象数是数不准的（历史会被压缩、被 undo 撤掉），这个工具读的是账本本身。\n\
             数字取的是**当前生效**的那一段：/undo 撤掉一轮之后再调，数字会跟着回落。",
        ),
        schema: Arc::new(json!({ "type": "object", "properties": {} })),
    }
}

/// timed 条目的声明。模型看不见它（timed 区不进 `specs()`），`description` 只给
/// 读代码的人和日志看。
pub fn audit_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from(AUDIT_TOOL),
        description: Arc::from("每个完成轮往 <session>.audit.log 追加一行审计记录。"),
        schema: Arc::new(json!({ "type": "object" })),
    }
}

/// 截获式执行体：纯渲染，不再有任何副作用（153 之前还要往 [`Ledger`] 里记一笔
/// 给 `audit` 传话，那格传话随 153 一起删除）。
///
/// 读写纪律（后代收窄、只走 command 面）在 [`crate::ext_stats_report`] 那边落实——
/// 这里连 `&mut Session` 都没用上，直接降成 `&Session` 交给纯函数。
///
/// **交 [`Aftermath::Nothing`]**（201）：这次调用**没碰外部世界**，状态回滚就够了。
/// 这跟 [`Aftermath::Irreversible`] 的区别是决策 199 的全部要点——后者是「碰了但撤
/// 不回」，而 `report` 连读都只读进程内的账本，`/undo` 路过它不该停下来问任何事。
fn report_run() -> SessionToolFn {
    Box::new(move |session: &mut Session, agent: &AgentId, _input: &Value| {
        let (body, _counts) = ext_stats_report::render(session, agent);
        Ok((Arc::from(body), Aftermath::Nothing))
    })
}

/// `TurnEnd` 执行体：轮序号 +1，轮末**现读**一次账本，追加一行。
///
/// 153 之前这里拿不到 `Session`，数字得靠 `report` 经 [`Ledger`] 传话；153 把
/// `&Session`（只读）加进 `TimedRun` 签名之后，这个闭包直接用它——不再需要任何
/// 跨调用的传话状态。
///
/// 失败回 `Err`——136 的驱动会记一条 `tracing::warn!` 然后接着跑下一条钩子
/// （结果丢弃、失败不影响这一轮）。不 `eprintln!`：CLI 的标准错误是给用户看对话的，
/// 一次写盘失败不该插进对话流里。
fn audit_run(ledger: Arc<Ledger>) -> TimedRun {
    Box::new(move |_table: &ToolTable, session: &Session, _input: &Value| {
        ledger.append_turn_line(session)
    })
}

/// 这个包自己的账：只剩轮序号。**住在宿主进程内存里，不进状态**——它不是 agent 的
/// 状态，undo 不该动它，恢复也不该从快照里长出来。跨重启的连续性靠一件事：轮序号
/// **从既有审计文件的行数续号**（[`Ledger::new`]），于是 `kill -9` 之后审计文件不会
/// 从 `turn=1` 重新数一遍。
///
/// 153 之前这里还有一格 `seen`（`report` 观测到的数字，给 `audit` 传话用）——`audit`
/// 现在直接现读 `&Session`，那格传话已经没有存在的理由，整个删除。
pub struct Ledger {
    turns: AtomicU64,
    audit: Option<PathBuf>,
}

impl Ledger {
    /// 开一本账。轮序号从既有审计文件的**行数**续起——一行一轮是这个文件唯一的
    /// 结构承诺（验收 3），所以行数就是上一次跑到第几轮，不需要另存一份计数。
    pub fn new(audit: Option<PathBuf>) -> Arc<Self> {
        let seeded = audit.as_deref().map(existing_lines).unwrap_or(0);
        Arc::new(Ledger {
            turns: AtomicU64::new(seeded),
            audit,
        })
    }

    /// 轮序号 +1、现读一次 `session`、追加一行。没有审计路径（临时会话）时只数轮，
    /// 不写盘。`agents` 不按调用者收窄（红线 10 管的是「喂给某个模型看到的东西」，
    /// 这条钩子没有调用者，是运行时自己在轮末跑的）——取整棵树的节点数。
    fn append_turn_line(&self, session: &Session) -> Result<Arc<str>, Arc<str>> {
        let turn = self.turns.fetch_add(1, Ordering::Relaxed) + 1;
        let agents = session.agent_tree().nodes.len();
        let counts = ext_stats_report::count(session, agents);
        let line = render_line(turn, counts);
        let Some(path) = &self.audit else {
            return Ok(Arc::from("（临时会话，无审计文件）"));
        };
        append_line(path, &line)
            .map_err(|e| Arc::from(format!("审计行写不进 {}：{}", path.display(), e).as_str()))?;
        Ok(Arc::from(line.as_str()))
    }

    /// 测试用：此刻数到第几轮。
    pub fn turns(&self) -> u64 {
        self.turns.load(Ordering::Relaxed)
    }
}

/// 一行审计长什么样。**固定字段、固定顺序、没有时钟**——这个文件是拿来 `wc -l` 和
/// `grep` 的，一行一轮。153 起四个数字都是轮末现读的真实状态，不再需要区分
/// 「有没有人观测过」，也就不再需要 `seen_at=`。
fn render_line(turn: u64, counts: ext_stats_report::Counts) -> String {
    format!(
        "turn={turn} entries={}/{} agents={} tools={}",
        counts.effective, counts.entries, counts.agents, counts.tool_calls,
    )
}

/// 追加一行（含换行）。`create(true).append(true)`：文件不在就建，多个进程先后写
/// 同一份也只是接着往后追。
fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")
}

/// 既有审计文件有几行。读不到（不存在 / 没权限）就当 0——续号是锦上添花，不能因为
/// 它让一个正常的会话起不来。
fn existing_lines(path: &Path) -> u64 {
    std::fs::read(path)
        .map(|bytes| bytes.iter().filter(|b| **b == b'\n').count() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "ext_stats_tests.rs"]
mod tests;
