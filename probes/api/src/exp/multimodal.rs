//! E10：图片入参——三家收不收 OpenAI 的 content 数组 + `data:` URL、模型看不看得见、
//! 图片进不进缓存前缀。
//!
//! `probes/PROVIDERS.md` §未测 的第二条原文就是「多模态入参形状」。这组回答四问，
//! 前两问决定**能不能做**，后两问决定**做了多贵**：
//!
//! 1. **形状**：content 数组本身收不收（先不带图，只放一个 text 块）；带图收不收。
//!    这条不是形式主义——产品现在的 `wire::messages` 把所有 text 块拼成一个
//!    **字符串**塞进 `content`。如果数组形态只在带图时才用，同一条历史消息就会在
//!    两种形态之间切换，而那正是红线 11 最怕的事。所以要先知道「不带图也能用数组」
//!    成不成立。
//! 2. **看没看见**：图里印一个四位数，模型答不出来就是「收了但没看」。
//!    200 不等于支持——E9 在消息级 system 上踩过同一个坑（收 ≠ 听）。
//! 3. **多贵**：同一条 prompt 带图 vs 不带图的 `prompt_tokens` 差。
//! 4. **缓存**（最贵的一条）：原样重发命中多少；末尾追加一轮之后，历史里那张图
//!    还认不认。图片若不进缓存块、或每轮编码字节不稳，就是**每轮全价重发一张图**，
//!    而且完全静默——功能全对，只有账单不对。
//!
//! 探针只报观测到的数字，**不下结论**——结论人工判读后写进 PROVIDERS.md。

use crate::client::{Ctx, verdict};
use crate::{caps, fixture};
use serde_json::{Value, json};

/// 本组在 `ctx.nonce` 之上再加的后缀。`bin/multimodal --dump` 要复刻**同一张**图，
/// 两处必须共用这一个常量——分叉了，dump 出来的就不是真发出去的那张，验了等于没验。
pub const NONCE_SUFFIX: &str = "-img";

const RECORDS: usize = 140;
/// 同 E9：思考常开的家用小预算会把正文截成空串，「没看见」其实是「被截断」。
const ANSWER_MAX_TOKENS: u64 = 1000;
const ASK: &str = "图片里印着一个四位数。只回答那四个数字，不要任何其它字。";
const ASK2: &str = "刚才那张图里的四位数，最后一位是几？只回答一个数字。";
/// 末尾追加那一轮用的合成助手回复——**不用模型真实输出**，理由同
/// `fixture::conversation`：前缀延长要求逐字节确定，模型输出本身不确定。
const SYNTHETIC_REPLY: &str = "好的，我已经看过那张图。";

pub fn run(ctx: &mut Ctx) {
    println!("\n  E10 图片入参（形状/看见/计价/缓存）");

    let provider = ctx.provider.clone();
    let model = ctx.model.clone();
    let nonce = format!("{}{NONCE_SUFFIX}", ctx.nonce);
    let (digits, data_url) = fixture::image_digits(&nonce);
    println!(
        "      图内数字={digits}  data URL={} 字符（约 {} KB PNG）",
        data_url.len(),
        data_url.len() * 3 / 4 / 1024
    );

    let sys = json!({ "role": "system", "content": fixture::system(&nonce, RECORDS) });
    let text_block = json!({ "type": "text", "text": ASK });
    let image_block = json!({ "type": "image_url", "image_url": { "url": data_url } });

    // 一、数组形态本身收不收（不带图）。
    let array_only = body(&provider, &model, json!([sys, { "role": "user", "content": [text_block] }]));
    let arr = ctx.call_after_pause(&array_only, "数组形态·纯文本块");

    // 二、带图。冷发一次 → 原样重发一次（判缓存）→ 末尾追加一轮（判仅扩展匹配）。
    let with_image = json!({ "role": "user", "content": [text_block, image_block] });
    let image_body = body(&provider, &model, json!([sys, with_image]));
    let cold = ctx.call_after_pause(&image_body, "带图·冷");
    let seen = cold.heard(&digits);
    let repeat = ctx.call_after_pause(&image_body, "带图·原样重发");
    let extend_body = body(
        &provider,
        &model,
        json!([sys, with_image, fixture::assistant(SYNTHETIC_REPLY), fixture::user(ASK2)]),
    );
    let extend = ctx.call_after_pause(&extend_body, "带图·末尾追加一轮");

    // 三、基准：同一条 system、同一句问题，content 用**字符串**（产品今天的形态）。
    // 放最后发，前面几次都不会污染它的 prompt_tokens 读数。
    let plain = ctx.call_after_pause(
        &body(&provider, &model, json!([sys, fixture::user(ASK)])),
        "纯文本·基准（字符串形态）",
    );

    // ── 判读（只描述观测，不替人下结论）──
    println!(
        "      → 数组形态：{}；带图：{}",
        accepted(&arr.error),
        accepted(&cold.error)
    );
    println!(
        "      → 看没看见：{}（图内 {digits}，模型答「{}」）",
        if seen { "看见" } else { "没看见/答错" },
        cold.content.as_deref().unwrap_or("<空>").trim().chars().take(40).collect::<String>()
    );
    let image_cost = cold.prompt() as i64 - plain.prompt() as i64;
    println!(
        "      → 图片计价：带图 prompt={}，纯文本 prompt={}，差 {image_cost}",
        cold.prompt(),
        plain.prompt()
    );
    println!(
        "      → 数组形态本身的开销：数组纯文本 prompt={}，字符串 prompt={}，差 {}",
        arr.prompt(),
        plain.prompt(),
        arr.prompt() as i64 - plain.prompt() as i64
    );
    println!("      → 原样重发 cached={} / prompt={}", repeat.hit_tokens(), repeat.prompt());
    verdict(cold.prompt(), repeat.hit_tokens());
    println!(
        "      → 末尾追加 cached={} / prompt={}（图仍在历史里）",
        extend.hit_tokens(),
        extend.prompt()
    );
    verdict(repeat.prompt(), extend.hit_tokens());

    // 四、尺寸扫描。只有收图的家才跑——被拒的家再打三次也只是三个同样的 400。
    // 默认那张只有 270×110，量出来的 token 不能外推到用户真会传的照片上；
    // 换两个倍数各打一次，才分得清「固定开销」还是「随面积长」。
    let sweep = if cold.error.is_none() {
        size_sweep(ctx, &provider, &model, &sys, &text_block, &nonce, cold.prompt(), plain.prompt())
    } else {
        println!("      → 尺寸扫描：跳过（这家不收图，再打也只是同样的 400）");
        Value::Null
    };

    ctx.observations.push(json!({
        "summary": "multimodal",
        "size_sweep": sweep,
        "digits_in_image": digits,
        "array_form_accepted": arr.error.is_none(),
        "image_accepted": cold.error.is_none(),
        "image_seen": seen,
        "image_answer": cold.content,
        "array_error": arr.error,
        "image_error": cold.error,
        "prompt_image_cold": cold.prompt(),
        "prompt_plain_baseline": plain.prompt(),
        "prompt_array_text_only": arr.prompt(),
        "image_token_cost": image_cost,
        "repeat_hit": repeat.hit_tokens(),
        "repeat_prompt": repeat.prompt(),
        "extend_hit": extend.hit_tokens(),
        "extend_prompt": extend.prompt(),
    }));
}

/// 同一个数字换两个放大倍数各打一次，连同默认那档一起报出 token 成本。
///
/// 报的是**每档的绝对 token 数**，不替人拟合曲线——三个点看得出是常数还是随面积长，
/// 而拟合出来的系数会被当成承诺，实际各家怎么切 tile 我们并不知道。
#[allow(clippy::too_many_arguments)]
fn size_sweep(
    ctx: &mut Ctx,
    provider: &str,
    model: &str,
    sys: &Value,
    text_block: &Value,
    nonce: &str,
    default_prompt: u64,
    plain_prompt: u64,
) -> Value {
    let mut rows = vec![json!({
        "scale": fixture::DEFAULT_SCALE,
        "prompt_tokens": default_prompt,
        "image_cost": default_prompt as i64 - plain_prompt as i64,
    })];
    for scale in [4usize, 24] {
        let (_, url) = fixture::image_digits_scaled(nonce, scale);
        let msg = json!({
            "role": "user",
            "content": [text_block, { "type": "image_url", "image_url": { "url": url } }]
        });
        let o = ctx.call_after_pause(&body(provider, model, json!([sys, msg])), &format!("尺寸扫描·{scale}x"));
        rows.push(json!({
            "scale": scale,
            "data_url_chars": url.len(),
            "prompt_tokens": o.prompt(),
            "image_cost": o.prompt() as i64 - plain_prompt as i64,
            "error": o.error,
        }));
    }
    println!("      → 尺寸扫描（图片 token 成本 = 本档 prompt − 纯文本基准 {plain_prompt}）：");
    for r in &rows {
        println!(
            "          {}x  prompt={}  图片成本={}",
            r["scale"], r["prompt_tokens"], r["image_cost"]
        );
    }
    Value::Array(rows)
}

/// 请求骨架。**刻意不带 `tools`**：这组问的是 messages 的形状，工具是噪声，
/// 而空的 `tools: []` 本身在有的家会 400，会把「图不收」误判成「形状不收」。
fn body(provider: &str, model: &str, messages: Value) -> Value {
    json!({
        "model": model,
        "messages": messages,
        "max_tokens": ANSWER_MAX_TOKENS,
        "temperature": caps::temperature(provider),
        "stream": false
    })
}

fn accepted(error: &Option<String>) -> String {
    match error {
        None => "收".to_string(),
        Some(e) => format!("拒（{}）", e.chars().take(120).collect::<String>()),
    }
}
