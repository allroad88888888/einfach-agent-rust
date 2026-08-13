# I built a skill-activation subsystem, measured it, and deleted 1,945 lines of it

*The measurement that killed it was one I ran to make it better.*

---

## The problem

An agent runtime that hosts a growing catalog of capabilities has a prompt-size problem. If every skill's full instructions sit in the system prompt, the prompt grows with the catalog, and you pay for all of it on every single turn regardless of relevance.

The obvious fix is laziness. Show the model a compact index — names and one-line descriptions — and load the full text only when it's needed.

That much is still what the system does today. Everything else about it is gone.

---

## What I built

Skills were an **activation subsystem**:

- Model calls `srv:skill/activate` with a skill id
- That writes to an `active_skills` slot in session state
- The slot is journaled, so activation participates in undo, crash recovery, and audit replay
- On each subsequent turn, the assembly step expands active skills and **injects their body text into the system segment**
- `srv:skill/deactivate` reverses it

Injection needed its own machinery — a `late_system` field threaded from the core through to each provider adapter, plus per-provider handling of *where* in the request that late text should go.

Which is where the measuring started.

---

## The measurement that made it look good

Different providers handle mid-conversation additions to the system prompt very differently. I ran a probe against all three I support (DeepSeek, Kimi, GLM), appending an instruction mid-conversation and checking three things: does it arrive, does the model obey it, and what does it cost in cache hits?

The interesting result was DeepSeek. Appending a *message* zeroed its cache — even though every byte before the insertion point was unchanged. But folding the same text onto the **tail of the top-level system content** — which intuitively should be worse, since you're rewriting an earlier part of the prompt — retained about **91%** of the cache.

So I built exactly that. Per-provider injection strategy: append-to-system-tail on DeepSeek because it measured at 91%, message-level on the other two because they were free there.

This is good engineering, in the narrow sense. I found a real 91%-vs-0% difference, encoded it in the adapter layer where provider differences belong, and left the core untouched.

It's also the reason I didn't notice the whole thing was unnecessary for another two weeks.

---

## The measurement that killed it

The industry converged on a different shape while I was building mine: put the *index* in the system prompt, and deliver skill **bodies as ordinary tool results** — the model calls a `read` tool, and the text arrives in the conversation as a tool result message.

My first reaction was that this had to be worse. Bodies would land at the end of the message history, where they'd bloat the conversation. Surely a tidy system-prompt injection is cleaner than appending kilobytes of instructions into the transcript.

Then I measured it, and the reasoning reversed completely.

**Appending to the tail of the message history is the one thing prompt caching is explicitly built for.** Every provider's cache is a prefix cache. Growing the conversation at the end is the access pattern they're all optimized for — it's what happens on literally every normal turn. Injecting into the system segment mid-conversation is the opposite: it modifies something *before* the cached region, which is the one operation these caches handle badly, and each one handles badly in its own way.

I'd spent the probe budget measuring how badly each provider handles the thing I shouldn't have been doing.

Ten turns on DeepSeek with the tool-result approach — 13 provider calls, three of which were skill-body reads:

**Cache hit rate 97.5%–99.8%, mean 98.5%. Not one call below 90%, reads included.**

The reads cost essentially nothing, because they're the access pattern the cache wants.

---

## What deleting it looked like

**65 files, +607 / −2552 — net 1,945 lines removed.** Eight files deleted whole, carrying 34 tests with them.

Gone from the core: `activate_skill`, `deactivate_skill`, and their error type. Gone from the provider layer: the `late_system` field, the `LateSystemReshapedPrefix` adjustment variant (and its generated TypeScript), the cost-multiple constant, two message-assembly helpers, and the per-provider injection branches in all three adapters. Gone from the runtime: the activation tool, the injection path, and the shadowing rules that existed because injected content could collide with the tool table.

And gone from the probe results: **the entire per-provider injection strategy.** The 91% finding, the thing I was proudest of, described the least-bad way to do something there was no longer any reason to do.

What replaced all of it: two ordinary tools. One returns an index at session start. One reads a body on demand. Neither is special-cased anywhere in the core.

---

## The part that couldn't be deleted

One thing had to stay: the `SkillsActive` slot in the state schema.

Not because anything reads it — nothing does. Because **sessions persisted before the deletion have real activation entries in their journals**, and the journal is replayed on recovery. Remove the variant and every one of those sessions fails to deserialize.

So the variant is still there, marked deprecated, with all its write paths removed. It's a shape in the schema that exists only so that history stays readable.

I want to be precise about the trade this represents, because it's the most interesting thing in the whole episode.

The system's central design claim is that all agent state lives in one atomic dependency graph with a command log, which is why undo, redo, crash recovery, and audit replay are the same mechanism rather than four features that drift apart. That's a strong property and it's most of the reason the system exists.

**The bill for that property is that you cannot quietly un-ship a state shape.** If your ledger is real, it contains real history, and real history constrains you. A system that faked its undo — deleted some UI state and moved on — could have removed the variant cleanly, because it was never keeping a durable record of anything.

The vestigial enum variant *is* the ledger working. It's not debt, it's the receipt.

There's an honest behavior change recorded alongside it: an old session that had a skill activated will recover fine, and will no longer see that skill's body in its context. The state is there; nothing reads it. That's a real difference in behavior, and it's written down rather than smoothed over.

---

## What I'd take from this

**The precision of a measurement tells you nothing about whether the thing measured should exist.** My 91%-vs-0% finding was correct, reproducible, and load-bearing for a design decision. It was also a detailed characterization of a code path that shouldn't have been in the system.

I think the specific trap is this: **investigating something makes it feel more necessary.** Once I'd run a three-provider probe, found a real asymmetry, and encoded it as a per-provider strategy, that subsystem had become the thing I understood best in the entire codebase. That's exactly backwards from how it should feel. The parts you've measured most carefully are the parts you're least willing to delete, and there is no reason for those two things to be related.

The measurement that actually mattered took ten turns and one number. It didn't require understanding the injection machinery at all — which is, in hindsight, the tell. **The question "should this exist?" is usually cheaper to answer than the question "how do I make this better?", and I answered them in the wrong order.**

---

*The runtime is [einfach-agent](https://github.com/allroad88888888/einfach-agent-rust). The deletion is issue 141; the decision that caused it is 27 in the roadmap, which supersedes decision 21 and keeps 21's reasoning on file — because "why we changed our mind" is worth more than the conclusion.*
