# Capability flags are `match provider` wearing a hat

*If adding a fourth vendor means editing your core, the abstraction didn't work. Here's a test for that, and a way out.*

> This is one rule from a set of twelve. I wrote about the set as a whole in
> [The bugs that don't fail](185-bugs-that-dont-fail.en.md) — this post is the long
> version of the last one in it,
> because it's the only one whose violation stays invisible until the day you add a
> fourth vendor, rather than the day you hit undo.

---

You're supporting three LLM providers. They mostly agree, and disagree in irritating specifics: one can force a named tool call, one can't; one has thinking mode on by default and refuses forced tool calls while it's on; one accepts any temperature and one only accepts `1`.

You know better than to write `match provider` in your core. So you abstract:

```rust
struct Capabilities {
    supports_forced_tool: bool,
    supports_named_tool: bool,
    thinking_toggleable: bool,
    temperature_adjustable: bool,
    // ...
}
```

and the core branches on the flags instead of the names. Vendor names now appear in exactly one place. That feels like the right shape, and every code review will approve it.

I want to argue it's the same mistake with better manners, and give you a single question that tells you which side of the line you're on.

---

## The test

> **When you add a fourth provider, do you edit the core?**

With capability flags, yes — and not as an oversight. Structurally.

The new provider arrives with a combination of flags that no existing provider has. `supports_named_tool: false` combined with `thinking_toggleable: true`, say, when your existing three only ever produced that pair as `false/false` and `true/true`. That combination now takes a path through your core **that has never executed.**

Four flags is sixteen combinations. Six is sixty-four. You have three providers, so you have tested at most three of them, and you found out which three by running them.

The flags didn't remove the branching. They removed the vendor *names* from the branching, which made it harder to see which vendors you're actually branching on — and made it feel like the problem was solved when the branch count had actually gone up.

There's a second symptom that shows up earlier, if you're watching for it: **you keep adding flags.** Every integration surfaces one more thing that differs, each addition is individually justified, and the combination space doubles each time. A struct that grows a field per vendor integration is not an abstraction over vendors. It's a list of them, transposed.

---

## Invert the direction

The alternative is to stop asking beforehand.

**Before:** the core asks "can you force a named tool call?" and branches on the answer.

**After:** the core says "this turn must call `fs/read`." The adapter does the best it can. If it can't do exactly that, it does the nearest thing and **attaches a record of the substitution to the response.**

```
Adjustment::ToolChoiceDowngraded { wanted: "fs/read", used: "required" }
Adjustment::ThinkingDisabledForToolChoice
Adjustment::TemperatureOverridden { wanted: 0.0, used: 1.0 }
```

The core has one path. It sends intent, gets a response, and — because it was going to have to anyway — checks whether what it wanted actually happened.

That last clause is what makes the whole thing work, and it's easy to skip past. **Forced tool calls are not a guarantee on any provider.** Even where the API accepts your `tool_choice` without complaint, the model can come back with something else. So the core already needs the "verify the result" path. It exists, it's tested, it runs on every turn.

Once you have that path, the pre-flight capability check is buying you nothing. It's a second, less reliable mechanism for a question the first mechanism already answers — with the added downside that it answers it *before* the fact, using a static description of the provider, rather than *after* the fact, using what actually came back.

---

## What this buys, concretely

**Test combinations go from 2^N to 1.** The core has one path. Provider-specific behavior lives in adapters, each of which is a pure function you can test against recorded response frames with no network.

**Adding a provider doesn't touch the core.** New directory, four functions, done. This is the property the abstraction was supposed to deliver and the flag version doesn't.

**Degradations become visible.** This is the one I'd underweight if I hadn't seen it work.

With capability flags, the core silently takes the weaker path. Nothing is wrong; nothing is reported; the behavior is simply different from what you'd get elsewhere, and there is no artifact anywhere saying so. Six months later someone asks "why does this work on provider A but not B?" and the answer is a boolean somewhere that nobody has looked at.

With adjustments, every substitution is a **piece of data attached to the turn it happened in.** It goes into the log, into the CLI output, into the audit trail. "This turn's forced tool call was downgraded to `required` because thinking mode can't be disabled on this provider" is a sentence the system can produce about itself.

And it gives you a property worth stating explicitly: **an empty adjustment list means the request was executed as intended.** That's a strong claim, and it's only available because the adapter is obliged to report rather than permitted to decide.

Which points at the actual failure mode this design is defending against: **a silent compromise.** Not "we couldn't do it" — that's fine, it's honest. But "we did something else and told no one," which is what capability-flag branching produces by construction, because the branch is taken before there's a turn to attach anything to.

---

## Where the line actually falls

"Put provider stuff in adapters" is too vague to act on. Two rules make it concrete.

**Request assembly belongs to the adapter, not the core.** The core supplies raw materials — system chunks, message history, tool specs, intent — unmerged and unformatted. The adapter assembles. This isn't an aesthetic preference: every assembly decision depends on a vendor difference (where do late-added tools go, does the thinking field enter the cached prefix, can temperature be set at all). Assembling in the core means writing a function that makes those decisions without being allowed to know anything — which means it can't make them, which means it's just a courier.

**But adapters may not change the world.** They're pure functions. And that's the constraint that decides the genuinely hard cases.

Take context compaction — dropping old turns when the conversation approaches the window limit. Three sub-questions, three different answers:

- **When to compact** — core. It's arithmetic: token count versus configured window size. No vendor branching, just a parameter. (Parameters are fine. The rule prohibits *branching* on vendor behavior, not accepting numbers that differ per vendor.)
- **How to compact** — core. Compaction is a state change; it must go through the command layer so it enters the undo log like everything else.
- **How the compacted result is laid out in the request** — adapter. One provider does real prefix-tree matching and can preserve a shared branch; the others match extensions only and lose everything after the edit point, so they eat the cost and report an adjustment.

That third one is the interesting split, and here's why it has to be that way: **if the adapter compacted on its own, the prompt and the recorded state would disagree.** Undo, audit, and the cached-prefix mirror would all be describing a conversation that was never sent. The adapter is allowed to decide *how to render* the world. It is not allowed to change it.

That's the line: **adapters translate, they don't mutate.** Everything else follows.

---

## The honest cost

Two, and neither is free.

**You find out later.** A capability check fails before you spend anything. An adjustment arrives after a request you already paid for. When the substitution is something you'd rather have avoided entirely, you've paid to learn that.

I think this is the right trade anyway, because the pre-flight check is only as good as your static description of the provider — and vendor documentation is, in my measured experience, wrong in both directions. I've found APIs that document four supported values and reject two of them, and APIs that document one and accept four. A capability flag is a claim about a provider that you maintain by hand and verify never. An adjustment is an observation about a request that actually happened.

**Someone has to look at adjustments.** They're data, and data nobody reads is worse than a branch, because at least the branch does something. If they only ever land in a log file, you have built a very tidy mechanism for ignoring problems. They need to surface where a human sees them — in the turn output, in a status view, somewhere with eyes on it.

---

## The one-line version

If your provider abstraction has a struct of booleans that the core reads, count the combinations, count how many you've run, and ask what happens when the fourth vendor arrives.

Then try the inversion: **state intent, let the adapter substitute, and require it to say so.** One path, one test combination, and every compromise becomes a visible event in the turn where it happened — instead of a boolean nobody has read since the day it was added.

---

*This is red line 12 of twelve in [einfach-agent](https://github.com/allroad88888888/einfach-agent-rust); the seam is `docs/ADAPTER.md`. It's enforced by a grep for vendor names and `caps.` in the core crates — which catches the obvious violations and none of the clever ones, so it's also on the design-review list.*
