//! E5：多个 agent 共享同一段 system + tools 前缀，能不能共享缓存。
//!
//! 这是整个系统里潜在最大的一笔成本优化：一个 root spawn N 个子 agent，
//! 若它们共用 system + tools，第一个建立缓存、其余全部命中。
//!
//! 但它和「给每个子 agent 精确裁剪工具子集」直接冲突 —— 裁剪不同则前缀不同，
//! 缓存分裂成 N 份。所以要先确认收益是真的，再决定值不值得牺牲裁剪精度。

use crate::client::{Ctx, verdict};
use crate::fixture;

const RECORDS: usize = 180;

pub fn run(ctx: &mut Ctx) {
    println!("\n  E5 子 agent 共享 system+tools 前缀");

    let provider = ctx.provider.clone();
    let model = ctx.model.clone();
    let nonce = format!("{}-share", ctx.nonce);
    let tools = vec![fixture::tool_a()];

    // agent #1：建立共享前缀。
    let b1 = fixture::body(
        &provider,
        &model,
        fixture::messages(&nonce, RECORDS, "任务 A：Record 0042 的 owner？只答 team-N。"),
        tools.clone(),
    );
    ctx.call_after_pause(&b1, "agent#1 冷");
    let base = ctx.call_after_pause(&b1, "agent#1 复发（基准）");

    // agent #2：同样的 system + tools，**不同的任务**。
    let b2 = fixture::body(
        &provider,
        &model,
        fixture::messages(&nonce, RECORDS, "任务 B：Record 0107 的 region？只答 RN。"),
        tools,
    );
    let probe = ctx.call_after_pause(&b2, "agent#2 首次（换任务）");

    verdict(base.hit_tokens(), probe.hit_tokens());
    if probe.hit_tokens() > 0 {
        println!(
            "      → agent#2 首次调用即命中 {}，共享前缀成立",
            probe.hit_tokens()
        );
    } else {
        println!("      → agent#2 首次未命中，共享前缀不成立（或分歧点太靠前）");
    }
}
