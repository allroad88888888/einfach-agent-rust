//! E7：`thinking.type` 开关本身是否进前缀。
//!
//! 上一轮 GLM 首次开 thinking 时命中归零，但那一组前面跑过 E1，干扰排除不掉。
//! 这里用独立 nonce 从冷开始，只切 `thinking.type`，其余逐字节不变。
//!
//! 结论直接决定：`thinking` 能不能中途开关，还是必须会话级锁定。

use crate::client::{Ctx, verdict};
use crate::{caps, fixture};

const RECORDS: usize = 180;

pub fn run(ctx: &mut Ctx) {
    println!("\n  E7 thinking.type 开关是否进前缀（隔离）");

    let provider = ctx.provider.clone();
    let model = ctx.model.clone();
    let nonce = format!("{}-think", ctx.nonce);
    let msgs = fixture::messages(&nonce, RECORDS, fixture::ASK);
    let plain = fixture::body(&provider, &model, msgs, vec![fixture::tool_a()]);

    let mut off = plain.clone();
    if !caps::apply_thinking(&provider, &mut off, false) {
        println!("    跳过：{provider} 无 thinking 字段");
        return;
    }

    ctx.call_after_pause(&off, "thinking=disabled 冷");
    let base = ctx.call_after_pause(&off, "thinking=disabled 复发");

    let mut on = plain.clone();
    caps::apply_thinking(&provider, &mut on, true);
    let switched = ctx.call_after_pause(&on, "切到 thinking=enabled");
    verdict(base.hit_tokens(), switched.hit_tokens());

    // 切回去。若切回立刻满命中，说明两个变体各自有缓存、并存不互毁。
    let back = ctx.call_after_pause(&off, "切回 thinking=disabled");
    verdict(base.hit_tokens(), back.hit_tokens());

    let b = base.hit_tokens();
    println!(
        "      → 切换后 {} / 切回后 {} / 基准 {b}",
        switched.hit_tokens(),
        back.hit_tokens()
    );
    // 判读只描述观测，不给"应该怎么做"的结论 —— 那写进 results/ 由人来判。
    println!(
        "      → {}",
        match (switched.hit_tokens() >= b, back.hit_tokens() >= b) {
            (true, _) => "切换后仍满命中 ⇒ thinking.type 不在 prompt 里，可中途开关",
            (false, true) => "切换掉、切回满 ⇒ 两个变体各有缓存，thinking.type 进前缀",
            (false, false) => "切换掉、切回也没回来 ⇒ 不只是前缀问题，需单独复查",
        }
    );
}
