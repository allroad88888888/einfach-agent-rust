# Invariants

> Translated from [INVARIANTS.md](INVARIANTS.md) as of commit `9ae84d5`.
> **The Chinese version is authoritative** — development happens in Chinese, so this file
> can lag. If the two disagree, the Chinese one is right and this one is a bug.
>
> To find out whether it has lagged, and by how much:
> `git log --oneline 9ae84d5..HEAD -- docs/INVARIANTS.md`. Empty output means this
> translation is current. If you update the translation, move the hash.

Break any of the rules below and undo / crash recovery will go wrong **silently** — no
error, no panic, just a recovered state that isn't the one you had. This is the most
expensive class of bug in this repository, which is why these are red lines and not
suggestions.

Each rule gives: the rule, why, what breaking it looks like, and how it's checked.

**Checks come in two kinds.** Rules a grep can decide are wired into
`scripts/check-invariants.sh` (a PostToolUse hook on Edit/Write, plus the local
end-of-session check). Rules requiring judgment go into the `agent-state-design` skill,
read when you're designing an atom, assigning a reversibility level, or choosing a value
type. **A rule that's written down but that nothing checks is waste paper within six
months.**

---

## 1. A derived atom's read fn must be a pure function

**Rule**: the closure passed to `create_derived` may not read the clock, take a random
number, read a mutable global, or do IO. Its only inputs are `ReadArgs::get` / `peek`.

**Why**: recovery means replaying the command log from a snapshot, and a replay has to
arrive at the same answer.

**Breaking it**: after an undo, the recomputed derived value differs from the original,
and redo doesn't line up either; a session recovered from a crash is a different session
than the one that crashed. Nothing errors at any point.

**Check**: the hook does a coarse grep for `Instant::now` / `SystemTime::now` / `rand::` /
`thread_rng` under `agent-core/src/atoms/`. Cases that slip past it (reaching the clock
through a helper, say) are on review. When you need "the current time," make it a
primitive atom whose value the command layer writes at write time.

---

## 2. Business code must never call `store.set()` directly

**Rule**: primitive writes go through `agent-core`'s command API. Bare `store.set` is
allowed only in `agent-store/src/` and `agent-core/src/command/`.

**Why**: undo needs every write to leave behind `(AtomKey, prev, next)`, and **explicit
declaration is the only workable way to get that**. Capturing changes automatically would
require a standing subscription and a baseline value for every tracked atom — a cost of
O(tracked atoms), and in this system every slot of every agent is a family atom, with
sub-agents added dynamically. That cost doesn't hold up. The upstream TypeScript
`createHistory` hit this and wrote it into its comments.

**Breaking it**: that write doesn't enter the undo log. When undo passes over it, that one
atom stays at its new value while everything else rolls back — a self-contradictory state,
and the kind that passes every test and shows up intermittently in production.

**Check**: the hook greps for `\bstore\.set\(` (by convention the variable is named
`store`), whitelisting the two directories above plus `*/tests/*` — manipulating the store
directly is a test's job; the rule is about business writes bypassing the undo log.

---

## 3. Every primitive atom's value must be serializable

**Rule**: every variant of `AgentValue` must serde. Live objects (`JoinHandle`,
`oneshot::Sender`, an HTTP stream, an MCP child-process handle) go in a runtime registry
outside the store; the atom holds only a serializable handle to them.

**Why**: a snapshot *is* the serialization of every primitive atom. One that can't
serialize means the snapshot has a hole in it.

**Breaking it**: the snapshot is missing a piece, so on recovery that atom takes its
default value and every derived value downstream computes something wrong. You find out
the first time you actually recover from a crash.

**Check**: `AgentValue` deliberately **does not offer** an `Opaque(Arc<dyn Any>)`-style
variant, which lets the type system catch most of it. The hook additionally greps the
value-definition files for `dyn Any`.

---

## 4. Snapshots and logs are keyed by `AtomKey`, never `AtomId`

**Rule**: the keys in `Snapshot` and `Entry.changes` are `AtomKey` (logical keys).
`AtomId` is only meaningful within a process.

**Why**: `AtomId` is an incrementing u64 (`inner.next_id += 1`) — entirely dependent on
creation order.

**Breaking it**: anyone who inserts a `create_atom` line into the middle of a graph-
construction function shifts **every value in every old snapshot by one position**. It
doesn't error, because the `Value` types may happen to be compatible — the values are just
attached to the wrong atoms.

**Twin clause** (pinned down by a real measurement in issue 019): a derived read fn must
**not capture an `AtomId`** — always look the family up by logical key at read time. A
derived value that captured an id panics outright once its dependency has been evicted and
rebuilt (ids are monotonic and never reused — which is lucky, because a panic beats a
silent wrong value). Rule 4 governs keys that go to disk; this clause governs keys captured
in closures. Same disease.

**Check**: the hook greps for a file containing both `AtomId` and `derive(...Serialize`;
`*/tests/*` is exempt (integration tests inherently hold both sides of the seam at once,
and this rule is about the production serialization path — decided in issue 010). The twin
clause needs judgment → skill / review; grep can't see closure captures.

---

## 5. Large values must be wrapped in `Arc`, with `PartialEq` taking a `ptr_eq` fast path

**Rule**: any `AgentValue` variant that might exceed a few hundred bytes is wrapped in
`Arc`. The first branch of `PartialEq` is `Arc::ptr_eq`. Message history uses
`imbl::Vector` rather than `Arc<Vec>`.

**Why**: `store.get()` returns an owned value — **every read is a clone** — and
`store.set` uses `PartialEq` to decide whether anything changed and therefore whether to
propagate.

**Breaking it**: not a correctness problem, a performance cliff. In a session with a
thousand messages, every prompt read deep-copies the whole history and every write deep-
compares it. Upstream's `Array` / `Lambda` are already `Arc`-wrapped; copy that.

**Check**: needs judgment → skill `agent-state-design`.

---

## 6. In-flight effects must carry an epoch, verified before write-back

**Rule**: an effect carries the session epoch it was dispatched with; before its result is
written back, the epochs are compared, and a mismatch means discard and cancel. Undo bumps
the epoch.

**Why**: a tool call is in flight and the user hits undo — the result comes home and would
be written into a world that has been rolled back.

**Breaking it**: a "ghost result" lands in rolled-back state. Intermittent, timing-
dependent, hard to reproduce.

**Check**: needs judgment → skill. On review, look for an epoch comparison on every
`POST /tool_result` path and every provider callback path.

---

## 7. `agent-core` / `agent-store` must not do IO

**Rule**: neither crate's `Cargo.toml` may contain `reqwest` / `hyper` / `axum` / `tokio`
or their ecosystems; their sources may not `use std::fs` / `std::net` / `std::process`.

**Why**: the entire agent loop must be unit-testable with no network — mock provider, mock
tool executor, with state transitions / undo / recovery all testable.

**Breaking it**: those tests become integration tests, then nobody writes them, and then
rules 1–6 have no regression protection at all.

**Check**: the hook decides it from `Cargo.toml` dependencies and source `use` statements.

---

## 8. `bind` defaults to `127.0.0.1`

**Rule**: bind loopback by default; listening on `0.0.0.0` requires explicitly setting
`AGENT_BIND`.

**Why**: there is no authentication at all right now, and that's deliberate — enterprises
add it at their own gateway.

**Breaking it**: an agent that can run shell tools, exposed to the network.

**Check**: the hook greps for a hardcoded `0.0.0.0` under `agent-server`.

---

## 9. File length: ≤300 lines normally, ≤500 for complex files

**Rule**: measured by `wc -l`. "Complex" is limited to a single tightly-cohesive
algorithm / state machine / engine core, and you must be able to articulate why splitting
it would make it *harder* to read. If you can't, the limit is 300.

**Why**: each file does one thing — you should be able to say what it does in one sentence
containing no "and".

**Breaking it**: if your change pushes a file over the limit, splitting it is part of that
change. No "we'll split it next time."

**Check**: decided by the hook. Over 500 blocks; 300–500 warns that a reason is required.

**Exempt**: `tests/`, `benches/`, generated code, fixtures, snapshots.

**Note**: Rust convention puts `#[cfg(test)] mod tests` inline at the bottom of the file,
which inflates line counts considerably. This repo's stance is to **move integration tests
into `tests/`** and keep only the most closely-bound unit tests in source files. Upstream
`einfach-core`'s `store.rs` was 1297 lines including inline tests; at fork time it was
split by responsibility into five files: graph structure and records / the read evaluation
path / flush and pending scheduling / subscription dispatch / debug introspection.

---

## 10. Agents may read up and down, never sideways

**Rule**: cross-agent reads go through `read_ancestor` (reading `messages` / `config` /
`skills` upward) and `read_descendant` (reading `status` / `result` / `usage` downward).
There is no third API. Siblings exchanging data do it via a common ancestor.

**Why**: the whole agent tree lives in one store, so everything is physically reachable.
The dependency graph has to be kept a tree by API constraint. The slot sets readable in the
two directions are disjoint, which makes a cycle structurally impossible.

**Breaking it**: a dependency cycle. Upstream has `CyclicRef` detection and a 256-depth
budget, so this surfaces as a runtime error rather than a silent wrong value — but that's a
backstop, not a design. Sideways reads also let an O(n) "read all my siblings" aggregation
sneak in.

**Check**: needs judgment → skill `agent-state-design`. Exposing exactly two functions is
itself the primary constraint.

---

## 11. Anything that enters a prompt must serialize byte-deterministically

**Rule**: the tool table, the skill list, and any collection that gets rendered into a
prompt uses ordered containers (`BTreeMap` / `BTreeSet` / `Vec`). No `HashMap`, no
`HashSet`. No timestamps, request ids, or random ids in the system prompt.

**Why**: prefix caching works on **byte-exact equality**. `HashMap` iteration order is
randomized in Rust, so the same tool table can serialize to two different byte sequences —
and the top-level `tools` sit at the very front of the prompt (confirmed by measurement
against all three providers), so every request becomes a brand-new prefix.

**Breaking it**: no error, no panic, everything works perfectly. **You just pay full price
on every single turn.** On DeepSeek v4-pro that is a 120× difference. Diagnosing it means
reading the invoice, i.e. paying the tuition first, every time.

**Check**: decided by the hook — a file containing both a `Serialize` derive and
`HashMap<` / `HashSet<`. There is also a runtime layer that compares the actual prefix
bytes before sending; see [probes/PROVIDERS.md](../probes/PROVIDERS.md) and
[issue 024](issues/024-cache-guard.md).

---

## 12. No model-related decisions in the core

**Rule**: `agent-core` / `agent-store` may not contain vendor names, **and may not contain
capability-flag branching either**. No `match provider`, no `if caps.xxx()`. The core has
exactly one path.

Model-related decisions live **entirely** in `agent-providers`: the core says "this turn
must call `fs/read`," and the adapter decides whether that becomes `tool_choice: {...}`,
whether thinking has to be switched off first, or whether this vendor simply can't do it
and the request gets downgraded.

**Why**: capability-flag branching looks cleaner than `match provider` but is the same
disease wearing a hat — every flag adds a branch in the core, N flags means 2^N
combinations, and most of them have never been executed. When adding a new provider means
editing the core rather than just an adapter, the seam was never sealed.

**The alternative: replace "ask about capabilities beforehand" with "report adjustments
afterward."**

The core doesn't ask "can you force a named tool call," it states the intent. The adapter
does its best and, if it falls short, attaches an `Adjustment` to the response:
"`MustUse(fs/read)` was downgraded to `required`." The core runs one path end to end and
checks the result afterward — which it has to do anyway, because **forced tool calls are
not a guarantee on any provider**.

Inverting the direction buys three things: adjustments are **visible** (they enter the log,
the CLI output, the audit trail) where a pre-flight branch is invisible; adding a provider
doesn't touch the core; and the test combinations drop from 2^N to 1.

**Breaking it**: no error. Everything works — until you add a fourth provider and discover
you have to edit the core, or until some capability-flag combination is taken for the first
time in production.

**Check**: decided by the hook — vendor names, `Capabilities`, or `caps.` appearing in
`agent-core` / `agent-store`, or a `Cargo.toml` there depending on `agent-providers`.

The full definition of the seam is in [ADAPTER.md](ADAPTER.md).

---

## About these rules

Rules 1–6 are preconditions for this architecture working at all, not coding style. What
they have in common: **breaking them produces no error**, and surfaces only as a wrong
value during undo or crash recovery — the two paths that get tested least.

So: when you add an atom, add a tool, or change the command layer, walk this document.
Automated checks cover only what a grep can decide; the rest rests on this document and the
skill.

---

## Terminology

Used consistently across the English translations of these documents.

| Chinese | English | Note |
|---|---|---|
| 红线 | invariant / red line | "red line" when referring to the numbered rules |
| 原子 / atom | atom | a unit of state in the dependency graph |
| 派生 / derived | derived atom | computed from other atoms by a pure read fn |
| primitive atom | primitive atom | stored, not computed; must be serializable |
| 命令层 | command layer | the only legal write path; produces undo-log entries |
| 日志 / journal | command log (as a verb: *journaled*) | the on-disk record of every command; recovery replays it. Note 日志 also means ordinary logging in [ARCHITECTURE.en.md](ARCHITECTURE.en.md) §deployment — that sense is *logs*, not this one |
| 快照 | snapshot | serialized primitive atoms |
| 可逆性屏障 | reversibility barrier | stops an undo at an irreversible operation |
| 前缀块 | prefix chunk | a cache-aligned segment of the assembled prompt |
| 料单 | ingredients | raw materials the core supplies to an adapter, unassembled |
| 接缝 | seam | a deliberate boundary where a class of difference is absorbed |
| 调整 / `Adjustment` | adjustment | a compromise the adapter made, reported after the fact |
| 宿主 | host | the application embedding this runtime |
| 时机 / `CallTiming` | call timing | when a tool is invoked: model-driven, session start, or turn end |
