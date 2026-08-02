//! E6：末尾追加 vs 中间改写 —— 直接量化压缩的代价。
//!
//! 这是 docs/STATE-MODEL.md 里那条「压缩和 undo 打架」的另一半：压缩重写消息历史
//! 中段，等于把变更点之后的缓存全部作废。到底作废多少，之前只有推断没有数字。
//!
//! 两个探针共用同一段已建缓存的对话前段，唯一区别是改动的位置。

use crate::client::{Ctx, verdict};
use crate::fixture;

const RECORDS: usize = 120;
const TURNS: usize = 6;
const MUTATE_AT: usize = 2;

pub fn run(ctx: &mut Ctx) {
    println!("\n  E6 末尾追加 vs 中间改写（压缩代价）");

    let provider = ctx.provider.clone();
    let model = ctx.model.clone();
    let nonce = format!("{}-mut", ctx.nonce);
    let tools = vec![fixture::tool_a()];

    let mut warm_msgs = fixture::conversation(&nonce, RECORDS, TURNS, None);
    warm_msgs.push(fixture::user(fixture::ASK));
    let warm_body = fixture::body(
        &provider,
        &model,
        serde_json::Value::Array(warm_msgs.clone()),
        tools.clone(),
    );

    ctx.call_after_pause(&warm_body, "对话前段 冷");
    let base = ctx.call_after_pause(&warm_body, "对话前段 复发（基准）");

    // 严格追加：前面每一个字节都没动。
    let mut appended = warm_msgs.clone();
    appended.push(fixture::assistant("好的，已确认。"));
    appended.push(fixture::user("再问一个：Record 0088 的 quota？"));
    let append_body = fixture::body(
        &provider,
        &model,
        serde_json::Value::Array(appended),
        tools.clone(),
    );
    let a = ctx.call_after_pause(&append_body, "末尾追加两条");
    verdict(base.hit_tokens(), a.hit_tokens());

    // 中间改写：只动第 MUTATE_AT 轮，长度接近，位置在中段。
    let mut mutated = fixture::conversation(&nonce, RECORDS, TURNS, Some(MUTATE_AT));
    mutated.push(fixture::user(fixture::ASK));
    let mutate_body = fixture::body(
        &provider,
        &model,
        serde_json::Value::Array(mutated),
        tools,
    );
    let m = ctx.call_after_pause(&mutate_body, &format!("改写第 {MUTATE_AT} 轮"));
    verdict(base.hit_tokens(), m.hit_tokens());

    println!(
        "      → 追加保留 {}，改写保留 {}（基准 {}）",
        a.hit_tokens(),
        m.hit_tokens(),
        base.hit_tokens()
    );
    println!("      → 改写若仍有残留，说明命中止于变更点，而非整条归零");
}
