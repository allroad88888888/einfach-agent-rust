//! `SessionEvent` 每个变体各铸一个样本，包成 [`Frame`] 信封（034——真实的
//! 下行 SSE wire 形状）序列化进 `fixtures.json`——issue 032 点 4「serde↔TS 形状
//! 对齐的实检」的 Rust 半边。TS 半边是 `packages/protocol/src/fixtures.test.ts`
//! 的 `satisfies Frame[]`。
//!
//! [`super::fixtures_cast`] 拆出了「骨架 → 真正想要的样本值」那个穷举 `match`
//! （109：本文件顶着行数天花板）——两个文件各自的单句职责：这里管「怎么把
//! 样本包成 `Frame`、写成 `events.json`/`events.ts`」，那边管「每个变体的样本
//! 该长什么值，为什么挑这个值」。

use std::fs;
use std::path::Path;
use std::sync::Arc;

use agent_core::{
    AgentId, AgentTree, DriftVerdict, GuardReport, Location, Notice, ReconcileVerdict,
    Reversibility, SummaryId, TokenUsage, ToolCallId, ToolCallRequest, WindowVerdict,
};

use crate::{
    BlockedCause, Frame, OrphanFate, SessionEvent, TransientSourceFailureCause,
    TransientSourceFailureEvent, UndoOutcome,
};

use super::fixtures_cast::cast_sample;

/// `SessionEvent` 每个变体一个样本，值确定（无时钟无随机——红线 1 的精神延伸到
/// fixtures：同一次生成必须逐字节相同）。
///
/// 先给每个变体铸一个占位骨架（字段用最简值），再用 [`cast_sample`] 这个**穷举
/// `match`** 把骨架换成真正想要的样本值——新增 `SessionEvent` 变体时，那个
/// match 没有 `_` 分支，编译器直接拒绝编译，直到给出对应的样本（issue 032 原话
/// 「编译器保证全覆盖」）。少铸一个骨架只会让那个变体缺样本，不会让代码编译
/// 不过——`cast_sample` 的穷举挡的是「变体存在但没人处理」，不是「样本数组
/// 没写全」，这一条留给 review 与验收表核对（当前 15 个变体、15 个骨架）。
pub fn sample_session_events() -> Vec<SessionEvent> {
    let skeletons = [
        SessionEvent::TextDelta(Arc::from("")),
        SessionEvent::ThinkingDelta(Arc::from("")),
        SessionEvent::ToolCallStarted {
            name: Arc::from(""),
        },
        SessionEvent::PreflightDriftAlert(DriftVerdict::Clean),
        SessionEvent::TransportTrouble(Arc::from("")),
        SessionEvent::ToolExecuting {
            call_id: ToolCallId::new(""),
            request: ToolCallRequest {
                tool: Arc::from(""),
                input: Arc::new(serde_json::Value::Null),
                location: Location::Server,
                reversibility: Reversibility::Pure,
            },
        },
        SessionEvent::ToolExecuted {
            call_id: ToolCallId::new(""),
            tool: Arc::from(""),
            output_len: 0,
            is_error: false,
        },
        SessionEvent::TurnGuard {
            usage: TokenUsage {
                prompt: 0,
                completion: 0,
                cached: None,
            },
            report: GuardReport {
                drift: DriftVerdict::Clean,
                reconcile: ReconcileVerdict::NoPrediction { actual: 0 },
                window: WindowVerdict::NoData { skipped: 0 },
            },
            adjustments: Vec::new(),
        },
        SessionEvent::Notice(Notice::Retrying {
            attempt: 0,
            max_retries: 0,
        }),
        SessionEvent::Undo(UndoOutcome::Blocked {
            entries: 0,
            barrier_seq: 0,
            label: String::new(),
            tool: None,
            call_id: None,
            cause: BlockedCause::NoHook,
        }),
        SessionEvent::Redo(UndoOutcome::Nothing),
        SessionEvent::Lagged { skipped: 0 },
        SessionEvent::SessionDied {
            reason: String::new(),
        },
        SessionEvent::Gap { skipped: 0 },
        SessionEvent::AgentTree(AgentTree { nodes: Vec::new() }),
        SessionEvent::OrphanedChild {
            child: AgentId::root(),
            fate: OrphanFate::Despawned { descendants: 0 },
        },
        SessionEvent::TransientSourceFailure(TransientSourceFailureEvent {
            epoch: 0,
            cause: TransientSourceFailureCause::PromptPreparation,
        }),
        SessionEvent::CompactionApplied {
            turn_id: 0,
            upto: 0,
            summary_id: SummaryId::new(""),
        },
        SessionEvent::ToolResultsCleared {
            turn_id: 0,
            call_ids: Vec::new(),
        },
    ];

    skeletons.into_iter().map(cast_sample).collect()
}

/// [`sample_session_events`] 各配一个 agent、包成 [`Frame`]——034 起，这才是
/// SSE 帧 data 的真实 wire 形状，fixtures 该证的是这一层，不是里面那个
/// `event` 单独长什么样。全部标 root：`AgentId` 到 TS 就是一个裸字符串
/// （`type AgentId = string`），选哪个字符串值不影响这一点，非 root 路径的
/// 序列化已经在 `agent-core` 的 `AgentId::agent_id_roundtrip`、`agent-server`
/// 的 `frame::tests` 里各自钉过，fixtures 不需要为了再证一遍而引入第二个
/// agent 值。
pub fn sample_frames() -> Vec<Frame> {
    sample_session_events()
        .into_iter()
        .map(|event| Frame {
            agent: AgentId::root(),
            event,
        })
        .collect()
}

/// 每个生成文件顶部多加的一行，跟 [`super::export::export_protocol_types`] 那边
/// 同一句话——两处各写各的常量而不是共享一个，是因为 JSON 文件不能有注释
/// （下面这一份不会被用到），没有值得抽的公共逻辑。
const REGEN_COMMENT: &str = "// issue 032：本文件由 Rust 生成，勿手改。重新生成：cargo run -p agent-server --features ts --example gen_protocol_ts\n";

/// 把 [`sample_frames`] 写成两份，`path`（`events.json`）与它的 `.ts`
/// 兄弟文件（同目录、同名换扩展名），**同一份内存数据**，不会跑偏。
///
/// # 为什么是两份，不是 issue 原文写的一份
///
/// issue 032 原文只要 `events.json`，TS 侧靠 `import ... from '.../events.json'`
/// 搭配 `satisfies` 检查。这在 `tsc` 里过不去——不是本仓的疏漏，是 TypeScript 对
/// JSON 模块 import 的既有行为：字符串字面量类型一律加宽成 `string`
/// （`"gap"` 变成 `string`，笔者拿这个仓库当前的 TS 工具链实测过，加了
/// `resolveJsonModule`/`esModuleInterop` 都一样）。邻接标签的判别字段
/// （`"type"`）和几乎每个嵌套枚举（`Location`/`Reversibility`/...）都是字符串
/// 字面量联合，加宽之后 `satisfies Frame[]` 不管协议形状对不对都会红
/// ——检查失去意义。
///
/// 唯一能让 TS 保住字面量类型的写法是 `as const` 修饰一个**写在源码里的**数组
/// 字面量（`as const` 对 import 进来的、已经加宽过的值不起作用，加宽发生在
/// import 边界，`as const` 救不回来）。JSON 语法恰好是合法的 TS 表达式语法，
/// 所以这份 `.ts` 直接原样内嵌跟 `events.json` **字节相同**的内容，只是外面套一层
/// `export const events = ... as const;`——不是另一份手写数据，同一次
/// `sample_frames()` 调用产出，两份不可能吵架。
pub fn write_fixtures(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut json = serde_json::to_vec_pretty(&sample_frames())
        .expect("Frame 全部字段都是 derive(Serialize) 的确定值类型，序列化不会失败");
    json.push(b'\n');
    fs::write(path, &json)?;

    // `trim_end`：`json` 结尾有 `write_fixtures` 自己加的换行，`]\nas const;`
    // 隔着一个换行——ASI 会把 `[...]` 读成独立语句收尾，`as const;` 变成下一条
    // 语句开头一个裸标识符 `as`，`tsc` 直接语法错误（parser 层面，不是类型层面）。
    // `as const` 必须跟 `]` 同一行。
    let json_str = String::from_utf8(json).expect("serde_json 输出总是合法 UTF-8");
    let ts = format!(
        "{REGEN_COMMENT}export const events = {} as const;\n",
        json_str.trim_end()
    );
    fs::write(path.with_extension("ts"), ts)
}
