//! E9：会话中途追加一条 `role:"system"` 消息——三家收不收、听不听、保不保前缀。
//!
//! 背景见 docs/issues/038-skill-injection-probe.md：039 的注入策略押在这上面。
//! PROVIDERS.md 只测过 Kimi 的消息级 **tools**（零缓存代价），消息级 **system**
//! 是空白，且「收」≠「听」——有的家可能 200 但把中途 system 当普通文本弱化，
//! 所以听没听必须是行为级断言（回答里有没有标记），不是状态码断言。
//!
//! 四问对应四步：
//! - 轮 1：普通对话建立前缀基线
//! - 轮 2：历史尾部插入 `role:system` 指令 + 新用户消息，跑 3 次取多数——
//!   收（HTTP 不 4xx）、听（回答带不带标记）、保前缀（`cached_tokens`）
//! - 轮 3：注入后再来普通一轮，验证新前缀继续命中
//! - 对照组：同一条指令改为并入顶层 system 再发，量化「注入 vs 重建」的 cached 差值

use crate::client::{Ctx, verdict};
use crate::fixture;
use serde_json::{Value, json};

const RECORDS: usize = 140;
const TURNS: usize = 4;
const MARKER: &str = "【标记X】";
const INSTRUCTION: &str =
    "从现在起，你的每一次回答都必须以【标记X】结尾，一个字都不能省略这个标记。";
const ASK2: &str = "请用一句话确认 Record 0021 的 tier，格式：tier=N。";
const ASK3: &str = "再用一句话确认 Record 0028 的 quota，格式：quota=N。";
/// 轮 3 的合成助手回复——**不用模型真实输出**，理由同 fixture::conversation：
/// 前缀延长要求逐字节确定，模型输出本身不确定（尤其加了标记指令之后）。
const SYNTHETIC_REPLY: &str = "确认：tier=1。【标记X】";
/// 要留够空间让思考 + 正文 + 标记都能输出完，避免「未听」其实是「被截断」。
/// 实测过 400：DeepSeek 默认开思考，一次就吃掉 314 个 reasoning_tokens，
/// 正文被截成空字符串——「未听」其实是「被截断」，不能用小预算，加大到 1000。
const ANSWER_MAX_TOKENS: u64 = 1000;

fn with_max_tokens(mut body: Value, max_tokens: u64) -> Value {
    body["max_tokens"] = json!(max_tokens);
    body
}

pub fn run(ctx: &mut Ctx) {
    println!("\n  E9 消息级 system 注入（收/听/前缀/对照）");

    let provider = ctx.provider.clone();
    let model = ctx.model.clone();
    let nonce = format!("{}-inj", ctx.nonce);
    let tools = vec![fixture::tool_a()];

    // 轮 1：普通对话建立前缀基线。
    let warm_msgs = fixture::conversation(&nonce, RECORDS, TURNS, None);
    let warm_body = fixture::body(&provider, &model, Value::Array(warm_msgs.clone()), tools.clone());
    ctx.call_after_pause(&warm_body, "轮1 建立前缀 冷");
    let base = ctx.call_after_pause(&warm_body, "轮1 建立前缀 复发（基准）");

    // 轮 2：历史尾部插入 role:system 指令 + 新用户消息。跑 3 次取多数——
    // 防单次抽样噪声，「收」「听」都按行为/传输结果记，不猜。
    let mut injected_msgs = warm_msgs.clone();
    injected_msgs.push(json!({ "role": "system", "content": INSTRUCTION }));
    injected_msgs.push(fixture::user(ASK2));
    let inject_body = with_max_tokens(
        fixture::body(&provider, &model, Value::Array(injected_msgs.clone()), tools.clone()),
        ANSWER_MAX_TOKENS,
    );

    let mut accepted = 0u32;
    let mut heard = 0u32;
    let mut first_hit: Option<u64> = None;
    let mut first_prompt: Option<u64> = None;
    let mut last_hit: Option<u64> = None;
    let mut last_prompt: Option<u64> = None;
    for i in 1..=3 {
        let o = ctx.call_after_pause(&inject_body, &format!("轮2 消息级注入 第{i}次"));
        let ok_http = o.error.is_none();
        let ok_marker = o.heard(MARKER);
        if ok_http {
            accepted += 1;
        }
        if ok_marker {
            heard += 1;
        }
        println!(
            "      → 第{i}次：{}／{}，cached={}",
            if ok_http { "收" } else { "拒" },
            if ok_marker { "听" } else { "未听" },
            o.hit_tokens()
        );
        if first_hit.is_none() {
            first_hit = Some(o.hit_tokens());
            first_prompt = Some(o.prompt());
        }
        last_hit = Some(o.hit_tokens());
        last_prompt = Some(o.prompt());
    }
    let majority_accepted = accepted >= 2;
    let majority_heard = heard >= 2;
    println!(
        "      → 收 {accepted}/3（多数={majority_accepted}），听 {heard}/3（多数={majority_heard}）"
    );
    if let Some(f) = first_hit {
        verdict(base.hit_tokens(), f);
    }

    // 轮 3：注入后再来普通一轮，验证新前缀（含注入消息）继续命中。
    let (extend_hit, extend_prompt) = if majority_accepted {
        let mut extended = injected_msgs.clone();
        extended.push(fixture::assistant(SYNTHETIC_REPLY));
        extended.push(fixture::user(ASK3));
        let extend_body = with_max_tokens(
            fixture::body(&provider, &model, Value::Array(extended), tools.clone()),
            ANSWER_MAX_TOKENS,
        );
        let r3 = ctx.call_after_pause(&extend_body, "轮3 注入后继续一轮");
        let prior_prompt = last_prompt.unwrap_or(0);
        println!(
            "      → 轮3 prompt={} cached={}（轮2 末次 prompt={prior_prompt}）",
            r3.prompt(),
            r3.hit_tokens()
        );
        verdict(prior_prompt, r3.hit_tokens());
        (Some(r3.hit_tokens()), Some(r3.prompt()))
    } else {
        println!("      → 轮2 多数被拒绝，跳过轮3（没有可延长的注入前缀）");
        (None, None)
    };

    // 对照组：同一条指令并入顶层 system 再发，同一条 warm 历史、同一个问题——
    // 唯一变量是「追加消息」还是「重建顶层」，量化两者的 cached 差值。
    let rebuilt_system = format!("{}{INSTRUCTION}\n", fixture::system(&nonce, RECORDS));
    let mut rebuilt_msgs = warm_msgs.clone();
    rebuilt_msgs[0] = json!({ "role": "system", "content": rebuilt_system });
    rebuilt_msgs.push(fixture::user(ASK2));
    let rebuilt_body = with_max_tokens(
        fixture::body(&provider, &model, Value::Array(rebuilt_msgs), tools),
        ANSWER_MAX_TOKENS,
    );
    let ctrl = ctx.call_after_pause(&rebuilt_body, "对照组 并入顶层 system 重建");
    println!(
        "      → 对照：注入首次命中 {}，重建首次命中 {}（基准 {}）",
        first_hit.unwrap_or(0),
        ctrl.hit_tokens(),
        base.hit_tokens()
    );
    verdict(base.hit_tokens(), ctrl.hit_tokens());

    // 汇总一条便于人工判读、抄进 PROVIDERS.md 的记录——原始逐次 usage 已经在
    // ctx.observations 里了，这条只是加个总览，不是额外下结论。
    ctx.observations.push(json!({
        "summary": "system-injection",
        "accepted_of_3": accepted,
        "heard_of_3": heard,
        "majority_accepted": majority_accepted,
        "majority_heard": majority_heard,
        "baseline_hit": base.hit_tokens(),
        "baseline_prompt": base.prompt(),
        "inject_first_hit": first_hit,
        "inject_first_prompt": first_prompt,
        "inject_last_hit": last_hit,
        "inject_last_prompt": last_prompt,
        "extend_hit": extend_hit,
        "extend_prompt": extend_prompt,
        "control_rebuild_hit": ctrl.hit_tokens(),
        "control_rebuild_prompt": ctrl.prompt(),
        "control_vs_inject_delta": ctrl.hit_tokens() as i64 - first_hit.unwrap_or(0) as i64,
    }));
}
