//! 轮末清算：root 已经答完，而后台子还没人领（052）。
//!
//! # 这里**不是**「修静止条件」
//!
//! 043 之后泵的静止条件是 `calls.is_empty() && mcp_calls.is_empty()`，而后台子
//! 自己的 provider 调用就住在同一张 `calls` 表里——所以 root 落终态、后台子还在
//! 飞时，泵**自然继续**驱动它，直到全静止才返回。语义天然成立：「一轮结束 =
//! root 终态 **且** 后台子静止」，不会卡死。`runner.rs` 模块文档当年那句「拒绝
//! root 终态 + 子树还跑的世界」是过虑（ORCHESTRATION §二的修正）。
//!
//! 真问题是**浪费**：root 已经答完了，还把没人要的子跑到底（烧 token）。
//!
//! # 定点 `despawn_child`，**不碰会话级取消**
//!
//! 既有的取消是**会话级**的（`Effect::CancelInFlight` 没有 agent 字段），而且它
//! 会把这一轮判成 `Failed(Cancelled)`——root 明明答成功了，把轮次标成取消是
//! **错的状态**。这里用的是 spawn 自己的补偿命令 [`Session::despawn_child`]
//! （自叶向根逐出、整棵子树一次 `store.batch` = 一个 undo 步、活值记进 `prev`
//! 所以 undo 拿得回来）：
//!
//! - 被拆的子 `is_live` 变 false → 它**在飞**的那条 provider 回执回来时撞
//!   `Session::step` 的活性闸（`step.rs`，跟红线 6 的 epoch 闸并列的第二道），
//!   被静默丢弃 —— 不写进一个已经收尾的世界。**复用既有闸，不新造。**
//! - 那些在飞凭据照常经泵的 D 落地、从 `calls` 里移除 → 表排空 → 下一次 B 正常
//!   返回 **root 的终态**。有界，不空转。
//! - 效果是**砍尾**：子当前这一轮 provider 调用还是会回来（已经在飞，砍不掉），
//!   但它的**下一轮不会再起**——死 agent 的事件被闸丢掉，不产生新的 effect。
//!   十轮的子被砍在第一轮末，不是跑满十轮。
//!
//! # 两类都要告警，别静默
//!
//! 「spawn 了后台子却没 collect 就收尾」是模型的编排失误，两种形态：还在跑的
//! （拆掉）和已经跑完躺在 stash 里没人领的（丢掉）。两种都经 `ctx.emit` 报一句
//! 出去。
//!
//! **054：告警有了专属变体 [`RunnerEvent::OrphanedChild`]。** 052 落地时借的是
//! `RunnerEvent::TransportTrouble`（既有变体里唯一「一句话文本、只进日志/面板、
//! 不参与任何判断」的口子），当时就诚实标注了那个名字对不上语义——这不是传输
//! 故障。专属变体要连锁改 `SessionEvent`（跨 SSE 的协议枚举）→ 生成的 TS →
//! fixtures，054 一次做完，接住的地方也齐了。
//!
//! 附带的结果是**这个文件不再拼句子**：它报的是三个事实（谁、怎么收场的、丢了
//! 多少），措辞归呈现层（`agent-cli::print::events` 一份、`packages/web/src/
//! render/notice.ts` 一份）。归属统一挂在**父**身上——没领是父的编排失误。

use agent_core::{DespawnRefused, Session};

use crate::ctx::RunnerCtx;
use crate::event::{OrphanFate, RunnerEvent};
use crate::persist;
use crate::subtree::Subtree;

/// root 落终态时清算后台子：活孤儿拆掉，stash 里没人领的丢掉，两种都告警。
///
/// 返回**这次真的动过东西没有**——调用方（泵的 B 点）据此决定要不要重发一次树
/// 快照（拆掉一棵子树会改变 `agent_tree()`，而树快照的变化检测住在 A 那一段，
/// 这条路不经过 `session.step`）。
///
/// root 还没终态就直接返回 `false`：后台子这时候还有机会被 collect（053），
/// 它们不是孤儿，是正在被等的活。
///
/// **053 之后「孤儿」这个词才真的有对立面。** 判据的三条
/// （`Subtree::take_orphans`：detached 且 `is_live` 且没有 collect 绑定）在 052
/// 落地时第三条恒真——那时候没有任何东西会绑。现在挡人的是两道：
///
/// - **正常路径**：父那个 collect 槽 `Pending` → root 进不了终态 → 这个函数一开头
///   就返回。子被领走时 `harvest_slots` 把它从 detached 名单里划掉，两张表干净，
///   下一次 `reap` 无事可做（`tests/collect_three_out_of_order.rs` 断言一句告警
///   都没有）。
/// - **取消路径**：Ctrl-C 把 root 推成 `Failed(Cancelled)`（终态）时 collect 还绑
///   着。第三条判据把这个子挡在 `despawn_child` 之外，于是它以活着的状态跨过这一
///   轮。**这是刻意跟前台 spawn 保持一致**：前台 spawn 的子从 029 起就只住在
///   `slots` 里、从来不在 detached 名单上，`reap` 一直看不见它们。绑了 collect 的
///   子就是一个前台子，只是记账时刻晚一点，没有理由单独给它一套收尾。
///   它也烧不了 token——取消 bump 过世代，它接下来的每一条回执都撞 epoch 闸。
pub(crate) fn reap(session: &mut Session, ctx: &mut RunnerCtx, subtree: &mut Subtree) -> bool {
    if !session.status().is_terminal() {
        return false;
    }
    let orphans = subtree.take_orphans(session);
    let unclaimed = subtree.take_stash();
    if orphans.is_empty() && unclaimed.is_empty() {
        return false;
    }

    for orphan in orphans {
        let fate = match session.despawn_child(&orphan.child) {
            // `agents` 一定含它自己（`live_subtree_leaf_first` 的形状），减掉它
            // 就是后代数。`saturating_sub` 是防御性的：一条告警不值得为一个将来
            // 可能变的实现细节留一条 panic 路径。
            Ok(report) => OrphanFate::Despawned { descendants: report.agents.len().saturating_sub(1) },
            // 拆不掉（子树之外还有读者，`DespawnRefused::StillRead`）：状态一个
            // 字节没改，这个子会以活着的状态跨过这一轮。照样喊一声——静默的话
            // 下一轮会冒出一个来历不明的活 agent。
            Err(refused) => OrphanFate::Kept { reason: refusal_reason(&refused) },
        };
        ctx.emit(&orphan.parent, RunnerEvent::OrphanedChild { child: orphan.child, fate });
    }

    for stashed in unclaimed {
        ctx.emit(
            &stashed.parent,
            RunnerEvent::OrphanedChild {
                child: stashed.child,
                fate: OrphanFate::Discarded {
                    bytes: stashed.content.len(),
                    is_error: stashed.is_error,
                },
            },
        );
    }

    // `despawn_child` 是一条命令，落了一条 teardown `Entry`——跟别的命令一样立刻
    // 转发进持久化后端，否则恢复出来的会话里会有一个「已经被拆掉、日志里却还活
    // 着」的子 agent。
    persist::sync(ctx, session);
    true
}

/// `DespawnRefused` 的可读描述。`{:?}` 就够——这个枚举的 `Debug` 只有变体名加
/// 一个 `AgentId`/`AtomKey`，没有任何要翻译的东西，而这条文本只进面板/日志、
/// 不参与任何判断。
fn refusal_reason(refused: &DespawnRefused) -> String {
    format!("{refused:?}")
}
