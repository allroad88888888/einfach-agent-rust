//! E0–E3：改动请求的某一处，看前缀还认不认。
//!
//! 第一轮跑出的 0% 曾被误读成「缓存被作废」。复跑全部命中说明**不是作废**：
//! 缓存按前缀键存，多个变体并存互不摧毁，0% 只表示「这个变体第一次见」。
//! 实践代价仍然硬 —— 那一轮整段对话要全价重编码一次。

use crate::client::{Ctx, verdict};
use crate::{caps, fixture};

const RECORDS: usize = 180;

pub fn run(ctx: &mut Ctx) {
    let provider = ctx.provider.clone();
    let model = ctx.model.clone();
    let msgs = fixture::messages(&ctx.nonce, RECORDS, fixture::ASK);
    let body_a = fixture::body(&provider, &model, msgs.clone(), vec![fixture::tool_a()]);

    println!("\n  E0 缓存是否开启 / 满命中基准");
    ctx.call(&body_a, "第 1 次（冷）");
    let baseline = ctx.call_after_pause(&body_a, "第 2 次（应命中）");
    let full = baseline.hit_tokens();
    if full > 0 {
        println!(
            "      → 满命中 {full}/{}，未缓存尾巴 {}",
            baseline.prompt(),
            baseline.prompt().saturating_sub(full)
        );
    }

    println!("\n  E1 顶层 tools 改动");
    let body_ab = fixture::body(
        &provider,
        &model,
        msgs.clone(),
        vec![fixture::tool_a(), fixture::tool_b()],
    );
    let o = ctx.call_after_pause(&body_ab, "加第二个工具");
    verdict(full, o.hit_tokens());

    println!("\n  E2 reasoning_effort 档位");
    match caps::effort_levels(&provider) {
        Some((lo, hi)) => {
            let mut b1 = body_a.clone();
            caps::apply_effort(&provider, &mut b1, lo);
            ctx.call_after_pause(&b1, &format!("effort={lo} 冷"));
            let base = ctx.call_after_pause(&b1, &format!("effort={lo} 复发"));

            let mut b2 = body_a.clone();
            caps::apply_effort(&provider, &mut b2, hi);
            let probe = ctx.call_after_pause(&b2, &format!("effort={hi}"));
            verdict(base.hit_tokens(), probe.hit_tokens());
        }
        None => println!("    跳过：{provider} 无 reasoning_effort 字段（不瞎猜）"),
    }

    println!("\n  E3 消息级 tools 追加到末尾");
    if caps::supports_message_level_tools(&provider) {
        let mut b = body_a.clone();
        caps::append_message_level_tools(&mut b, vec![fixture::tool_b()]);
        let o = ctx.call_after_pause(&b, "末尾追加 tools 消息");
        verdict(full, o.hit_tokens());
        println!(
            "      → prompt {} → {}，新增内容{}落在缓存之后",
            baseline.prompt(),
            o.prompt(),
            if o.hit_tokens() >= full { "" } else { "未" }
        );
    } else {
        println!("    跳过：{provider} 无消息级 tools");
    }
}
