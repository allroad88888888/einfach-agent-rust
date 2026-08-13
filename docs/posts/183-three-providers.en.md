# What actually differs between DeepSeek, Kimi and GLM

*Measured, not documented. Both of those words are load-bearing.*

---

All three of these providers advertise an OpenAI-compatible API. All three deliver one, in the sense that a naive request gets a sensible response. The differences show up in the places you only reach after you've committed to an architecture: prompt caching, forced tool calls, where `usage` lands in a stream, what a 404 means.

I needed those answers to build a provider abstraction, couldn't get them from the docs, and measured them instead. This is what came back.

**Everything below is from live requests on 2026-07-31** against `deepseek-v4-pro`, `kimi-k3`, and `glm-5.2`, with two follow-ups on 08-01 and 08-03. Raw JSON is in the repo. Numbers move; the method is the part worth keeping.

---

## First: the documentation is wrong in both directions

Not "incomplete." Wrong, and wrong in both directions, which is worse — you can't even apply a consistent correction.

- **DeepSeek's docs** say `tool_choice` supports all four values. Two of them return **400**.
- **GLM's docs** say `tool_choice` only supports `auto`. All four work.

If you build from documentation you get a system that's broken on one provider and needlessly degraded on another, and both failures look like your bug.

That finding is why the rest of this exists.

---

## The difference that changes your architecture

Prompt caching splits into **two different matching semantics**, and this is the single most consequential bit:

- **GLM does real prefix-tree matching.** Any request matches against the longest common prefix, block-aligned.
- **DeepSeek and Kimi only match extensions.** A new request must be a strict continuation of one they've already seen. Any divergence mid-prompt and the hit rate goes to zero.

Two independent experiments agree. Keeping system+tools fixed and changing only the trailing user message: GLM hit 5440 tokens on first request, the other two hit **0**. Rewriting the middle of a conversation: GLM retained **96.7%**, the other two retained **0**.

What that costs you:

| | Loss on mid-conversation rewrite | One compaction ≈ how many turns of hits | Sibling agents sharing a prefix |
|---|---|---|---|
| DeepSeek | 100% | **~120 turns** | no benefit |
| Kimi | 100% | ~10 turns | no benefit |
| GLM | ~3% | negligible | works |

Read the middle column again. **On DeepSeek, compacting a conversation once costs roughly what 120 cached turns would have cost.** Context compaction is a routine feature. On DeepSeek it is a routine feature with a 120× price tag attached, and nothing in the API tells you that.

The right column kills a design that sounds clever: "give all sibling agents the same tool set so they share a cached prefix." That only pays on GLM. On the other two the benefit is exactly zero, so you should trim each agent's tools to what it actually needs.

**One clarification, because I got this wrong at first.** A cold 0% hit rate looked like "changing the tool table destroyed the cache." Re-running the same request hit fully — the old prefix was never gone. Zero just means *this variant is new*. The real cost is one full-price re-encode of that turn, and then you're cached on the new prefix. "Invalidated" and "not yet seen" produce the same number and mean very different things.

### Where the tool list sits in the prompt

All three drop to 0% on the turn after you change the top-level `tools`. GLM does real prefix matching — so if `system` came *before* `tools`, it would at least keep the system blocks. It doesn't; it's also 0.

**So `tools` precedes `system` in the rendered prompt on all three.** That's not documented anywhere I could find, and it determines the segment order your cache-mirror logic has to use: `[tools][system][history]`.

### Caching doesn't pay on small prompts

Coverage (`cached / prompt`):

| prompt tokens | DeepSeek | Kimi | GLM |
|---|---|---|---|
| ~330 | 88.5% | 77.6% | **0%** |
| ~460 | 89.2% | 54.5% | **0%** |
| ~860 | 92.8% | 89.1% | 98.1% |
| ~3100 | 97.5% | 92.4% | 99.1% |

Block sizes are 128 / 256 / 64 respectively. **GLM caches nothing below roughly 860 tokens** and then jumps to 98%. Kimi at 470 tokens caches exactly one 256-block and wastes the remaining 214.

Small sub-agents are not worth optimizing for prefix reuse, on GLM especially.

---

## The difference that will silently break you

Kimi puts `usage` in a frame **after** the finish frame, and that frame has an **empty `choices` array**:

```
data: {"choices":[{"delta":{},"finish_reason":"stop"}]}
data: {"choices":[],"usage":{"prompt_tokens":110,"cached_tokens":110,...}}
data: [DONE]
```

A decoder that assumes every frame has `choices[0]` either panics or — much more likely, because most people write defensive index access — **silently drops the usage frame.**

And then everything looks fine. The conversation streams correctly. The user gets their answer. You've just lost every token count and cache statistic on that provider, which means any cache-regression guard you built is now blind, permanently, with no error anywhere.

This is the failure mode I care most about in this whole comparison: **the ones that break loudly are cheap. The ones that keep working while quietly disabling your instrumentation are the expensive ones.**

Two smaller ones in the same family:

- **Empty content is expressed differently.** DeepSeek sends an explicit `"content": null`; the other two omit the field. You cannot use "field is present" to test for content.
- **GLM repeats `role: "assistant"` on every frame.** Harmless if you ignore it, garbage in your output if you concatenate blindly.

---

## Forced tool calls fight with thinking mode

| | `none` | `required` | named function |
|---|---|---|---|
| DeepSeek (default) | ✅ | **400** | **400** |
| DeepSeek (thinking explicitly off) | ✅ | ✅ | ✅ |
| Kimi | ✅ | ✅ | **400** |
| GLM (either way) | ✅ | ✅ | ✅ |

Verbatim errors: DeepSeek `Thinking mode does not support this tool_choice`; Kimi `tool_choice 'specified' is incompatible with thinking enabled`.

- **DeepSeek v4-pro has thinking on by default.** To use `required` or a named function you must send `thinking.type=disabled` in the same request. Your adapter can do that automatically — but it should record that it did, because it just changed the model's behavior on the user's behalf.
- **On Kimi, naming a specific function is permanently unavailable.** Thinking is always on and there's no field to turn it off (no `thinking` in the parameter list, and every response carries `reasoning_tokens`). The best available behavior is to degrade to `required` and say so.

That last one is the general shape: **when you can't do what was asked, the honest move is to do the nearest thing and report the substitution.** Not to fail, and not to pretend.

---

## Adding tools mid-conversation

| | Channel | Cost |
|---|---|---|
| Kimi | Append a `role:system` message carrying `tools` | **Zero.** Measured: prompt 5276→5382, hits stayed at 5120 |
| GLM | Top-level only | Full re-encode of one turn, ~2× |
| DeepSeek | Top-level only | Full re-encode of one turn, **~120×** |

Kimi is the only one with a message-level tool channel, and it's free. On the other two, mid-conversation tool changes are a pricing event.

---

## Mid-conversation system messages: DeepSeek is backwards

Later experiment (08-03). Build a ~4400-token conversation, then append a `{"role":"system"}` message with a new instruction, and compare against folding the same text into the top-level system content instead.

| | Accepted | Obeyed | Prefix retained | vs. rebuilding the top-level system |
|---|---|---|---|---|
| DeepSeek | 3/3 | 3/3 | **0%** | **rebuilding is cheaper by 3968 tokens** |
| Kimi | 3/3 | 3/3 | 100% | injecting is cheaper by 4352 |
| GLM | 3/3 | 3/3 | 100% | injecting is cheaper by 4352 |

All three accept it and all three obey it. But DeepSeek zeroes the cache on an appended system message **even though every byte before the insertion point is unchanged** — while *rewriting the top-level system block*, which intuitively should be worse, keeps the cache.

That is exactly backwards from the other two, and backwards from intuition.

> **A methodology note that cost me an hour.** On the first run DeepSeek appeared not to obey — the answer came back empty. It wasn't disobedience: `max_tokens` was 400, thinking is on by default, and 314 tokens went to reasoning, truncating the visible answer to nothing. **When you're testing whether a model obeys an instruction, give it enough `max_tokens`, or you'll measure truncation and call it behavior.**

---

## Errors: don't classify by status code

The error body shape is consistent — `{"error": {"message", "type", ...}}` — but status codes are not:

| | DeepSeek | Kimi | GLM |
|---|---|---|---|
| Model name doesn't exist | 400 | **404** | 400 |
| Invalid key | 401 | 401 | 401 |
| Overloaded | 503 | **429** | — |
| Out of credit | **402** | — | — |

Kimi returns **404 for a nonexistent model name**, and 404 elsewhere usually means an unrecoverable routing problem. Classify on `error.type` first and fall back to status codes, not the other way around.

**And give "out of credit" its own class.** Backing off and retrying a 402 accomplishes nothing, and it needs a human immediately. Folded into your rate-limit bucket, it becomes a system that backs off politely forever.

Also: **none of the three return rate-limit headers.** No `Retry-After`, no `X-RateLimit-*`, nothing. Your backoff schedule is yours to invent.

---

## Two more things worth knowing

**GLM's server-side tools are invisible to you.** A `web_search` call returns 200 with no `tool_calls`, no trace of a search, `finish_reason: stop`, and no extra top-level fields. It happens inside the model. Your router never sees it, which means it can't enter your command log, can't be rolled back, can't be assigned a side-effect level, and leaves a hole in your audit trail. The correct way to model it isn't "a fourth execution location" — it's **a session-level switch that trades away auditability for that portion of the conversation**, and the user should be told that's the trade.

(Kimi's `formulas` are different: declared as ordinary functions, you receive `tool_calls` and execute them yourself. Visible, auditable, normal. Don't conflate the two.)

**Cache writes are asynchronous.** Measured in production on 08-01: a back-to-back tool call, milliseconds after the previous one completed, hit 640 instead of the expected 2304 — and 640 was exactly the block-rounded size of a mirror from *several turns earlier*. The next normally-paced call was back to 97%. So cache ingestion lags by seconds to tens of seconds, and a guard that compares only against the immediately preceding request will fire false positives under tight call sequences.

**GLM thinks a lot.** "Reply with exactly one word: OK" cost `completion_tokens: 194`, of which 191 were reasoning. DeepSeek spent 15 on the same prompt. GLM's per-token output price is low, but if every trivial call reasons for two hundred tokens, recompute your actual cost before choosing a default.

---

## The summary table

| | DeepSeek | Kimi | GLM |
|---|---|---|---|
| **Cache matching** | extension-only | extension-only | **real prefix tree** |
| Block size | 128 | 256 | 64 |
| Minimum to take effect | ~380 | 256 | **~860** |
| Hit discount | **120×** (flash 50×) | 10× | 2× |
| Cache field | `prompt_cache_hit_tokens` | `prompt_tokens_details.cached_tokens` | same as Kimi |
| On a miss | field is 0 | **field absent** | field is 0 |
| Message-level tools | ✗ | **✓ (free)** | ✗ |
| Message-level system | **✗ zeroes cache** | ✓ free | ✓ free |
| Thinking | toggleable, **on by default** | **always on, can't disable** | toggleable |
| `thinking.type` enters prefix | ✗ | — | **✓** |
| `tool_choice` named function | needs thinking off | **never available** | works |
| Stream `usage` position | same frame as finish | **separate frame after finish** | same frame as finish |
| Server-side tools | ✗ | formulas (visible) | retrieval/web_search (**invisible**) |
| `temperature` | free | **only accepts 1** | free |
| Tool count limit | 128 | undocumented | 128 |
| Rate-limit headers | none | none | none |

---

## Where all this ends up

None of these differences appear in the core of the system I was building. They live in adapters, and the core states intent — "call this specific tool," "keep this prefix" — while the adapter decides how to express it and **reports back anything it had to change.**

That direction matters more than it sounds. The alternative is a capability-flag struct the core branches on, and that's the same `match provider` wearing a hat: N flags means 2^N combinations, most of which have never been executed, and adding a fourth provider still means editing the core. Reporting adjustments after the fact keeps the combination count at one and makes every degradation visible in the turn where it happened, instead of on next month's invoice.

The measurements above are the argument for that design. Almost every one of them is a place where the API will do something other than what you asked, and tell you nothing.

---

*Raw observations and the probe code are in [einfach-agent](https://github.com/allroad88888888/einfach-agent-rust) under `probes/`. If you re-run them and get different numbers, the numbers changed — open an issue, I'd like to know.*
