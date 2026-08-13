# The bugs that don't fail: rules whose violations only surface during undo or crash recovery

*Some defects announce themselves. These don't. They pass every test, ship, work correctly for weeks, and then hand you a wrong answer with no error attached.*

---

There's a category of bug whose defining characteristic is that **nothing goes wrong**. No panic, no error return, no failing test. The feature works. The tests are green. The behavior is correct in every scenario you thought to check.

And then, three weeks later, in one specific circumstance — someone hits undo, or the process is killed and restarts, or you add a fourth vendor — the system produces a value that is simply wrong, and it does so silently.

I maintain a list of twelve rules for one such system. Six of them exist entirely to prevent this category. Here are the four that best illustrate why "just be careful in code review" isn't a strategy.

---

## 1. A derived value that reads the clock

The system keeps all state in an atomic dependency graph. Some values are stored; others are computed from stored ones by a read function. Undo works by replaying a command log and recomputing.

**The rule:** a read function may not read the clock, take a random number, read a mutable global, or do IO.

**Break it and:** you call undo, the derived value recomputes, and it comes back different — because `Instant::now()` returns something else than it did the first time. Redo doesn't match either. Recover from a crash and you get a session that isn't the one you had.

Nothing errors. The value is just... not the value.

What makes this treacherous is that it's *locally reasonable*. A read function that stamps "last updated" onto a computed record looks like exactly the kind of small convenience you'd wave through in review. It's correct on first evaluation. It's correct on every evaluation until something replays history.

**The fix isn't discipline, it's placement:** if you need the current time, make it a stored value written by the command layer at write time. Then the clock is read exactly once, and the reading is recorded.

---

## 2. In-flight work that outlives the world it was launched into

A tool call is dispatched. It takes four seconds. At second two, the user hits undo.

At second four, the result comes back and gets written into a world where the call it belongs to no longer exists.

**The rule:** every in-flight effect carries the session epoch it was launched with; results are compared against the current epoch before being written, and undo bumps the epoch.

**Break it and:** a ghost result lands in rolled-back state. Intermittent, timing-dependent, essentially impossible to reproduce on purpose.

This one can't be grep'd for. There's no forbidden token — the mistake is an *absent* comparison, and absence doesn't grep. It's on the review checklist, and the checklist names the specific call sites: every path where a tool result or a provider callback comes home.

**A rule you can't check is a rule you should assume is being broken somewhere.** Knowing which of your rules are in that category is itself useful information.

---

## 3. The one that costs money instead of correctness

Prompt caching works on **byte-exact prefix matching**. If the tools list at the front of your prompt serializes identically to last time, you pay a fraction. If a single byte moved, you pay full price for the whole thing.

Rust randomizes `HashMap` iteration order. Serialize the same tool table twice and you may get two different byte sequences.

**The rule:** anything that gets rendered into a prompt uses ordered containers — `BTreeMap`, `BTreeSet`, `Vec`. No `HashMap`, no `HashSet`. No timestamps or request ids in the system prompt either.

**Break it and:** nothing fails. Not one thing. Every request succeeds, every response is correct, every test passes, and the product behaves exactly as designed.

You simply pay full price on every turn, forever. On one provider I measured, the cached rate is **120× cheaper** than the uncached rate. So the bug is a two-orders-of-magnitude cost multiplier that is *invisible from inside the system*. The only channel that reports it is the invoice, which means you find out at the end of the month, and the way you find out is by paying.

I want to be direct about what this implies: **"the tests pass" is not evidence of anything here.** There is no test you would naturally write that fails. You'd have to already suspect the problem to test for it.

Two things catch it. A static check — same file has a `Serialize` derive and a `HashMap<` — which is crude and misses indirection. And a runtime check that compares the actual prefix bytes against last turn's before sending, which catches the general case. The second one exists precisely because the first one is not sufficient, and I'd rather have a redundant check than trust a grep with a two-orders-of-magnitude bill behind it.

---

## 4. The one that stays correct until the fourth vendor

The core of the system may not contain vendor-specific branching. No `match provider`. And — this is the part people push back on — **no capability flags either**. No `if caps.supports_prefix()`.

The capability-flag version looks like the clean solution. You've abstracted the vendor names away; the core doesn't mention anyone by name. That feels like the right shape.

It isn't, and here's the test that shows it: **when you add a fourth vendor, do you have to edit the core?**

With capability flags, yes. The new vendor has a combination of flags nobody's produced before, which means it takes a path through your core that has never executed. N flags means 2^N combinations and you have tested maybe four of them. The flags didn't remove the branching — they just made it harder to see which vendors you're actually branching on.

**Break this rule and everything works perfectly** — with the three vendors you built it against. It stays working for months. The failure arrives on the day someone integrates a fourth one and discovers that "add a provider" means opening the core, which is exactly the property the abstraction was supposed to buy.

The alternative is to invert the direction. The core states *intent* — "this turn must call `fs/read`" — and the adapter figures out how to express it. When the adapter can't, it doesn't ask permission beforehand; it does the nearest thing and **reports the substitution afterward** as data attached to the response.

Combinations to test: one. Cost of adding a vendor: one new adapter, core untouched. And every degradation becomes a visible event in the turn where it happened, instead of a silent difference in behavior you notice later, if ever.

---

## The thing all four share

None of these produce an error. All four produce **a wrong value, or a wrong bill, at a delay**, through a path where the code is doing exactly what it was written to do.

That has a practical consequence: for this category, testing is a weak instrument. Not useless — but you can only write a test for a failure you've already imagined, and the whole problem is that these failures are unimaginative-proof. They occur at the seams between features (state × undo, effects × cancellation, serialization × billing, core × vendor count), and seams are where nobody's test coverage lives.

So the countermeasure isn't more tests. It's **structural constraints checked by something that isn't a person.**

---

## Which brings me to the part I keep re-learning

The rules document has a line near the top:

> A rule that's written down but that nothing checks is waste paper within six months.

So each rule is classified: mechanically checkable rules are wired into a script that runs on every file save and in CI. Rules requiring judgment go into a design guide that gets read when you're making the relevant decision — deliberately not into the same script, because a checker that cries wolf gets disabled, and then you have neither.

I wrote that line. I believe it. And I still managed, this week, to diagnose an unrelated problem in the same codebase, fix it thoroughly, write it up carefully — and put no check in place. Eight days later it had come back through a different mechanism, and I found out by accident while writing about the first occurrence.

The second time I fixed it, I added the check. That's the whole lesson and it took me two rounds to apply it to myself.

**If you take one thing from this: go through your own conventions and sort them into "something verifies this" and "we intend to remember this."** The second list is longer than you expect, and the items on it are not being followed. Not because anyone is careless — because a convention with no verification isn't a constraint, it's a hope.

---

*The twelve rules are `docs/INVARIANTS.md` in [einfach-agent](https://github.com/allroad88888888/einfach-agent-rust); the checker is `scripts/check-invariants.sh`. Six of the twelve are in the silent-failure category, which is why they're rules rather than suggestions.*
