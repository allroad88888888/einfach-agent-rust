//! E4：缓存收益随上下文长度怎么变。
//!
//! 上一轮观测到未缓存的尾巴：DeepSeek 4、GLM 21、**Kimi 156**。尾巴是块粒度的
//! 副产物 —— 不足一整块的部分进不了缓存。真正要回答的设计问题不是「块多大」，
//! 而是**小上下文还值不值得为缓存做优化**：子 agent 往往只有几百 token。
//!
//! 方法：多个上下文规模各自建缓存再复发，看 cached/prompt 的比值曲线。

use crate::client::Ctx;
use crate::fixture;

/// 记录条数 → 大致 token 量。最小的两档专门压在 Kimi 公布的 256 门槛两侧。
const SIZES: &[usize] = &[3, 8, 22, 50, 100, 205];

pub fn run(ctx: &mut Ctx) {
    println!("\n  E4 缓存收益 vs 上下文规模");
    println!("    {:<12} {:<9} {:<9} {:<8} 覆盖率", "记录数", "prompt", "cached", "尾巴");

    let provider = ctx.provider.clone();
    let model = ctx.model.clone();

    for &records in SIZES {
        // 每档用独立 nonce，互不共享前缀，各自从冷开始。
        let nonce = format!("{}-g{}", ctx.nonce, records);
        let msgs = fixture::messages(&nonce, records, fixture::ASK);
        let body = fixture::body(&provider, &model, msgs, vec![fixture::tool_a()]);

        ctx.call_after_pause(&body, &format!("records={records} 冷"));
        let m = ctx.call_after_pause(&body, &format!("records={records} 复发"));

        if m.error.is_some() {
            continue;
        }
        let (p, c) = (m.prompt(), m.hit_tokens());
        let pct = if p > 0 { c as f64 / p as f64 * 100.0 } else { 0.0 };
        println!(
            "      {records:<12} {p:<9} {c:<9} {:<8} {pct:.1}%",
            p.saturating_sub(c)
        );
    }
    println!("      → 覆盖率显著低于 100% 的那几档，缓存优化基本无意义");
}
