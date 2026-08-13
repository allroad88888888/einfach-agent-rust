# State model

> Translated from [STATE-MODEL.md](STATE-MODEL.md) as of commit `064126a`.
> **The Chinese version is authoritative** — development happens in Chinese, so this file
> can lag. If the two disagree, the Chinese one is right and this one is a bug.
> Terminology follows the table at the bottom of [INVARIANTS.en.md](INVARIANTS.en.md).
>
> To find out whether it has lagged, and by how much:
> `git log --oneline 064126a..HEAD -- docs/STATE-MODEL.md`. Empty output means this
> translation is current. If you update the translation, move the hash.

The heart of this repository. Undo, redo, crash recovery, and audit replay are four
projections of one mechanism.

## Why this works at all

In a conventional agent framework, state is scattered across object fields, closure
captures, and local variables — you can't actually say what "the complete state" *is*, so
persistence means hand-picking fields, and missing one is a whole category of recovery bug.

Here you can say it: **complete state = the values of all primitive atoms.** Everything
derived is recomputable. Therefore:

- a snapshot is the serialization of every primitive atom
- recovery is rebuilding the atom graph, pouring the primitive values back in, and letting
  derived values recompute themselves
- the volume undo has to record is proportional to **source** state, not all state
- after a rollback every derived value is automatically consistent — you never get
  "message history rolled back but the token count didn't follow"

That last point is the one that matters. The hand-written-state-machine bug where a
rollback misses a field **cannot structurally occur here.**

## The atom graph

**Every slot is a family keyed by `AgentId`.** There are no singleton atoms — the root
agent is simply the one whose id is `root`, and it takes no special path. The entire agent
tree lives in one store; see §"sub-agents".

### Source atoms (primitive, entered in the undo log)

`Slot::ALL` currently has twelve members, all of which have write paths:

| slot | meaning |
|---|---|
| `Messages` | message history |
| `Status` | `Idle` / `Thinking` / `ToolsPending` / `Done{truncated}` / `Failed(_)` |
| `ToolSlots` | this turn's tool slots, ordered as the model requested them; each is `Pending` or `Finished{content,is_error}` |
| `PrevPrefix` | the previous request's prefix mirror (the adapter's comparison material); `Null` before the first turn |
| `NextMessageId` | the next `MessageId` to mint (strictly increasing from 1) |
| `TurnsUsed` / `MaxTurns` | how many `CallProvider`s this turn has issued, and the ceiling |
| `RetriesUsed` / `MaxRetries` | consecutive failures in the current failure-retry chain, and the ceiling |
| `ToolsAllowed` | **the tool subset snapshotted at spawn time**, doubling as the **live roster**: `Null` means this agent isn't on it |
| `SkillsActive` | ids of skills once activated (a sorted, deduplicated array — red line 11). **Kept as a shell, read-only** (decision 27 / [issue 141](issues/141-remove-activation-subsystem.md)): the write paths are deleted and no production code reads it to build a prompt any more — the variant itself survives because of red line 4 (old session journals really do contain entries that wrote this slot) |
| `HostTools` | **tools the host declared at session creation**: a name-sorted array of `{name, description, schema, reversibility}`. It comes back verbatim on recovery, so the host doesn't have to re-declare on reconnect |

`ToolsAllowed` carrying two jobs isn't a shortcut. It means **"this agent was spawned, and
it carries this tool subset"** — and `Null` is the *absence* of that fact, not a second
field. Which is why "never spawned," "the spawn was undone," and "already despawned" are
indistinguishable in state: they **are** the same state.

**Three that exist in the design and still have no write path**: `config` (model /
temperature / max_tokens), `system_base` (the base system prompt), and
`tools_registry_version` (a u64 bumped when the registry changes). Not an oversight — a
deliberate call: a slot that has never been validated by real use is the same as one that
was never written, except that it *looks* finished. When one is added, its direction in
`graph/visibility.rs` has to be declared explicitly at the same time (red line 10 is
guarded by an exhaustive match there).

`tools_registry_version` will only ever hold a version number; the registry itself lives
outside the store. The reason is red line 5: `store.get()` returns an owned value, so every
read clones, and putting an entire tool table in an atom means copying it over and over.

**In-flight tool calls are not per-call atoms**: the key `AtomKey::ToolCall(agent, call_id,
Result)` exists (the variant set of an on-disk key type can't be changed after the fact, so
it's kept), but **no production code writes it** — which tools are in flight during a turn
lives in that agent's single `ToolSlots` slot, as `SlotState::Pending`.

### Derived atoms (not logged; replayed by the engine after an undo)

**Exactly one exists today**:

```
ToolsConverged(agent) = f(this agent's ToolSlots)   // is nothing Pending any more?
```

Its shape is deliberate: **it scans the slots rather than maintaining a counter.** A
counter is the thing most likely to disagree after an undo — roll back the slots but not
the counter and the convergence condition is permanently off by one, silently. Moving it
into the atom graph makes "forgot to maintain it" impossible: there is no state to
maintain, only a recomputation. It answers `Pending` rather than `Bool(false)` when
unconverged, precisely so that pending-ness propagates downstream through the graph.

Several more are in the design and **have not landed** (written here so nobody assumes they
already work):

```
prompt.system     = f(system_base, skills_active, tools_registry_version)
prompt.payload    = f(prompt.system, messages, config)
turn.pending      = f(this agent's in-flight tool slots + all sub-agent Status)
turn.can_submit   = f(Status, turn.pending)
ui.token_estimate = f(prompt.payload)
ui.timeline       = f(messages, tool slot results)
```

As it stands, `Ingredients` is assembled fresh by the host **every turn**. The conclusion
("don't hand-write a counter of how many haven't come back") is unchanged; it's only prompt
assembly that doesn't currently go through the dependency graph — so ARCHITECTURE's claim
that "ingredients are maintained incrementally by the engine" is, today, only redeemed for
`messages`.

**The `skills_active` input above is obsolete**: that path was deleted along with decision
27 / [issue 141](issues/141-remove-activation-subsystem.md). `Slot::SkillsActive` remains
a shell (variant kept, no write path) but is no longer an input to any prompt-assembly
formula. Skill bodies today are read on demand by an ordinary tool (`srv:skill/read`) whose
result enters the conversation — an entirely different path from the "system/payload derived
from a set of primitives" table above, and one that neither needs nor should be folded back
into that formula.

### Where `Pending` comes from

Upstream einfach-core has a `#BUSY!` mechanism: while an async formula is in flight the
cell holds `Value::Error(Busy)`, which short-circuits along the error path to propagate
pending-ness to everything downstream; the host settles it later and dependents recompute.

That is exactly the semantics of "a tool call is in flight → the whole downstream UI goes
pending → the result arrives and it refreshes itself." At fork time every Excel error code
was deleted except this one, renamed to `Pending`.

## Writes must funnel

`store.set()` is bare and anyone can call it. **`agent-core` exposes only the command API,
and business code must not touch `store.set`** (red line 2), so every primitive write
explicitly leaves behind `(AtomKey, prev, next)`. Derived values are not recorded.

This isn't "log it while we're here," it's the only workable option. Capturing changes
automatically would require a standing subscription and a baseline value per tracked atom —
O(tracked atoms) — and here every slot of every agent is a family atom with sub-agents
added dynamically. That cost doesn't hold. The upstream TypeScript `createHistory` hit this
and wrote it into its comments; the conclusion is taken as-is.

One `store.batch(|s| {…})` is one undo step. Transaction boundaries reuse batch rather than
inventing a second concept.

## The command log

One flat log, one cursor. **Undo pops the top** — the log is ordered by time, and what gets
popped is the most recent step regardless of which agent performed it.

"Roll back only this agent's entries" means skipping entries in the middle of the log, and
a middle entry's `prev` was captured against the world as it was *then*, so skipping
doesn't work. That's selective undo, a problem of a different magnitude, and this project
doesn't do it.

```rust
// agent-store/src/history/log.rs — knows nothing of agent vocabulary
pub struct Entry<K, V, M> {
    pub seq: u64,                    // minted by History, strictly increasing, never reused
    pub meta: M,                     // filled by the layer above
    pub changes: Vec<Change<K, V>>,  // Change { key, prev, next }; prev captured at write time
}

// agent-core/src/command/meta.rs — what M is on the agent side
pub struct EntryMeta {
    pub turn_id: u64,          // groups the two undo granularities; allocated by the root agent
    pub epoch: Epoch,          // which generation this step happened in (red line 6's credential)
    pub label: &'static str,   // "user_input" / "provider_done" / "tool_result" / …
    pub barrier: bool,         // an uncrossable barrier: this step recorded an Irreversible tool call
}
```

**The three-parameter generic isn't decoration**: `History` lives in `agent-store`, and that
crate may not import `agent-core`, so agent vocabulary like `turn_id` / `epoch` / `label`
becomes the generic `M` as a group. On disk `M` is `PersistedMeta`, field for field, except
that `label` becomes a `String` — in-process labels are a finite set of compile-time
constants, while on-disk labels are historical data and may contain values this version
doesn't recognize.

`barrier` is the **only** on-disk basis for the undo barrier: before dispatching an
irreversible tool the host calls `Session::mark_irreversible`, the resulting `tool_result`
entry carries the bit, and an undo that hits it returns `UndoOutcome::Blocked`. It still
stops you after a crash and restart — the bit is in the file.

**There is no `agent` field**: attribution lives inside each `Change.key` (via
`AtomKey::agent()`), and one entry may span keys belonging to several agents. Undo never
looks at it anyway (one flat log, ordered by time); the UI timeline and the audit trail
read it out of `changes`.

**There is also no `owner` (tenant) field.** It once said "keep it now so multi-tenancy
later needs no migration" — which is backwards: real multi-tenancy means adding a field to
`EntryMeta` and `PersistedMeta`, and that *is* an on-disk schema change (old rows lack the
key and `Deserialize` fails unless you also add `#[serde(default)]`). Either add it now or
admit honestly that a migration is coming.

- `undo(turn)` — pop from the top until `turn_id` changes (the default UI granularity)
- `undo(batch)` — pop one entry (developer/advanced mode, an expandable timeline)
- `redo` — replay `next` in the forward direction

`turn_id` is **allocated by the root agent, and every sub-agent entry inherits the turn_id
of the root turn it happens in.** Sub-agents do not create turn boundaries. So `undo(turn)`
steps back one entire root turn, taking with it all sub-agent work from that turn — which is
what "one store, undo rolls back everything" ought to mean.

**⚠️ Anything that writes to the store during session creation must call `begin_turn()`
itself to push the boundary forward.** `TurnStatus::Idle` **is not a terminal status**, so
`handle_input` doesn't open a new turn for the **first** turn — meaning whatever was written
at creation time **shares turn 1** with the user's first message, and `/undo`-ing that first
message takes it along. Issue 073 hit exactly this (host-injected tool declarations), and
the symptom is **silent**: you see nothing at the time, and only discover the tool table is
missing entries the next time the session is reopened, which is nowhere near the scene of the
crime. On a freshly created session `begin_turn()` produces no `Change` at all
(`History::append` rejects empty steps), so it **writes no entry** and does nothing but move
the boundary. The cost is zero. Don't skip it.

### Cap and branching

The log is bounded, **defaulting to 100 entries at the session layer**, overflowing from the
oldest end. Note the layering: the `History` structure itself defaults to **unbounded** — it
knows nothing about how large a session ought to be, just as it knows nothing about
`AtomId` or `turn_id`. The 100 is a session-layer policy, not a constant of the log
structure. Truncatability is exactly the advantage a transaction log has over a snapshot
log: every entry carries its own complete inverse, so dropping the oldest doesn't affect
whether the rest can roll back. A snapshot-style log would have to scan backwards through
prior history to find an atom's previous value, and truncation would lose it permanently.

Writing a new entry while the cursor is not at the top **discards the redo tail by default**
(everything at index >= cursor). Branching from a historical point is an explicit operation,
not the default.

## On-disk keys must be `AtomKey`

`AtomId` is an incrementing u64, entirely dependent on creation order. If a snapshot stored
`(AtomId, Value)`, then anyone inserting a `create_atom` into the middle of a graph-
construction function would shift every value in every old snapshot — **and it wouldn't
error; it would be a silent misalignment.** That's red line 4.

```rust
enum AtomKey {
    Agent(AgentId, Slot),
    ToolCall(AgentId, ToolCallId, ToolCallSlot),   // ToolCallSlot currently has only Result
}
```

Two variants only. **There is no `Skill(SkillId)`** — skill content lives in a registry
outside the store, and the store holds only which ones were activated, which is
`Agent(_, Slot::SkillsActive)`.

`ToolCallSlot` **has only the `Result` variant today, and even that branch has no production
write path** (in-flight tool slots live in `Agent(_, Slot::ToolSlots)`). The variant set is
kept because `AtomKey` is the on-disk key type: you can add to `Slot` (old snapshots simply
lack the key and take a default), but you cannot change `AtomKey`'s variant set after the
fact — the two have very different stability requirements.

The design also calls for a `ToolCallSlot::Request`, storing a snapshot taken when a call is
dispatched, including the `Location` and `Reversibility` **as of that moment**. The reasoning
still holds: recovery must decide using the semantics in force when the call was made, not
by re-reading a tool table that may have changed since — otherwise a call marked
`Irreversible` at the time might be re-sent as though it were `Pure`. **But it has never
landed, and that is deliberate**: `agent-core` has no tool table, so fabricating a
placeholder snapshot would be making things up, and a *false* `Irreversible` would make undo
block a harmless `fs/read` — precisely the silent-wrong-value failure this repo fears most.
To add it, the **host that owns the tool table** has to record it. The upper half of the
§"interruption semantics" table below is blocked on exactly this.

`Slot` says "how to restore"; `AgentId` says "which one to restore." This is the same shape
as upstream TypeScript's `HistoryOp { key, scope }` — already proven there, so copied.

A snapshot is `Snapshot { values: Vec<(AtomKey, AgentValue)> }`, primitives only.
`Entry.changes` uses `AtomKey` on disk as well. Source slots are **not created lazily**,
specifically so that "complete state = all primitives" holds immediately on the
`Session::primitives()` side — with lazy creation, a slot never written wouldn't be in the
family, the snapshot would be missing an entry, recovery would give it a default, and if the
default happened to equal its actual value at the time, nothing would ever error until the
day someone changed the default.

Schema evolution comes free as a side effect: a new slot simply isn't found in an old
snapshot and takes its default; a deleted slot is a surplus entry in the snapshot and is
ignored with a warning. No migration scripts.

## Unserializable things stay outside primitives

In-flight HTTP streams, MCP stdio child processes, SSE senders, tool-execution
`JoinHandle`s — these are not state, they are state's **execution site**.

The rule (red line 3): **every primitive atom's value must be serializable.** Live objects
go in a runtime registry outside the store, and the atom holds only a serializable handle:

```
atom:      ToolSlots[i] = Pending                 serializable
registry:  call_id → JoinHandle / oneshot sender  not snapshotted, rebuilt
```

`AgentValue` therefore **does not offer** an `Opaque(Arc<dyn Any>)`-style variant. Offer one
and somebody will use it, and then the snapshot has a hole — a hole you only find out about
during recovery.

## Epoch

A tool call is in flight (its slot holds `Pending`) and the user hits undo; the result comes
home and would be written into a world that has been rolled back. So (red line 6):

- undo bumps the session epoch
- every effect carries the epoch it was dispatched with
- write-back compares first, and on mismatch discards and cancels

The landing point is the first gate in `Session::step`: an event whose epoch doesn't equal
the current generation is **discarded whole**, returning an empty `Vec` without writing a
single primitive (epochs only increase, so "not equal to current" is equivalent to "stale").
The credential for the remote-tool path is held **by the server**: a client `POST
/tool_result` neither carries nor can forge an epoch, and can only exactly match a
`(agent, call_id)` that is still waiting.

Skip this and it will eventually blow up — in the intermittent, hard-to-reproduce way.

## Sub-agents

**The entire agent tree lives in one store**, with family instances distinguished by
`AgentId`. Not one store per agent. That buys three things separate stores can't:

1. A child reading its parent is a single `get`, tracked and invalidated automatically
   through the dependency graph, with no message-passing mechanism at all.
2. "Wait until all sub-agents finish" is a derived atom, with `Pending` aggregating up the
   graph automatically — no scheduler to write.
3. **Cross-agent undo is consistent for free**: when a parent rolls back a step, the
   sub-agents spawned by that step are in the same command log and roll back with it. With
   separate stores this would be a distributed transaction.

### `AgentId` encodes a path

`root/a1/a1.2`. Ancestor/descendant checks are prefix matches, computable without reading
the store.

Don't store a parent pointer in an atom — then the read-boundary decision would depend on
store state, while undo is in the middle of rolling store state back. That ties a knot.

### Read boundaries: up and down only, never sideways

Which keeps the dependency graph a tree. There are exactly two APIs and no third:

```rust
fn read_ancestor  (&self, reader: &AgentId, target: &AgentId, slot: Slot) -> Result<AgentValue, ReadDenied>;
fn read_descendant(&self, reader: &AgentId, target: &AgentId, slot: Slot) -> Result<AgentValue, ReadDenied>;
```

**Crossing the boundary is explicitly refused, not defaulted** — `ReadDenied` has four
variants: `NotAnAncestor` / `NotADescendant` (wrong direction; sideways reads die here),
`NotVisible` (right direction, but this slot doesn't open that way), and `NoSuchAtom` (no
such atom in the graph — **and it does not helpfully create one**). This layer is red line
10's runtime landing point; the structural half is in `graph/visibility.rs`, which is an
**exhaustive match with not a single `_` wildcard**: adding a slot without declaring its
direction doesn't compile.

Today's two directions: upward — `Messages`, `SkillsActive`, `HostTools` (context a
sub-agent needs to work; the latter two are "what capabilities does this session have,"
which belongs to the session rather than to any one agent). Downward — `Status`,
`ToolsAllowed` (a parent needs to know whether a child is done; `ToolsAllowed` doubles as
the live roster, and an aggregating derived value has to know which children are alive
first). Everything else is `Private` — **opening a direction requires a reason; closing one
doesn't.**

The slot sets readable in the two directions are **disjoint**, and combined with the graph
being a tree that makes a cycle structurally impossible — no reliance on the runtime
`CyclicRef` backstop. (The argument: cross-agent edges are only "a descendant reads an
ancestor's `Upward` slot" or "an ancestor reads a descendant's `Downward` slot," a cycle must
contain both kinds, therefore some slot on the cycle is read in both directions, which
requires the two sets to intersect. So the test asserts **the set property itself**, not a
handful of cases.) Siblings exchanging data go via a common ancestor.

### Eviction and undo

**Three hard constraints pinned down by measurement in issue 019**:

1. **Evict leaf-to-root**: `destroy_atom` panics outright if a reverse edge exists, and
   `family.evict` refuses while downstream remains — an order hard-coded by the engine:
   destroy derived before primitive.
2. **Eviction is state-driven**: "removed from the live roster → the aggregating derived
   recomputes → the edge disappears → only then can it be evicted." Not a timer casually
   evicting things.
3. **Rebuilding guarantees the atom comes back, not the value**: eviction produces no
   `Change`, and undo only pours in the values carried by entries. A despawn's teardown
   command must record the live value as `prev`, or undo restores a default — chain intact,
   value wrong, no error.

Sub-agents are short-lived and one root session may spawn hundreds, so not reclaiming atoms
is a leak. But evict an agent's atoms when it finishes and a later undo back to the moment
it was running finds its target gone.

The solution is another dividend of `AtomKey`: **undo/redo paths recreate a missing atom on
demand** (create with a default, then pour in `prev`). Upstream's applier already does this —
`resolve(op.scope)` is a family get-or-create. This lazily-recreate path must be written into
the applier; miss it and you get "undo halfway and discover you can't get back."

### Concurrency

**Sub-agent concurrency is IO concurrency, not state concurrency.** LLM calls and tool
execution run as concurrent futures on a single thread (`FuturesUnordered`), and write-back
must return to the actor thread and serialize — the same write-back path as `tool_result`,
with no new mechanism.

The session boundary follows naturally: **one root agent + its entire subtree = one session
= one actor thread = one store.** Stores are not shared across roots.

### The cost of aggregating atoms

A derived value that "reads every sub-agent's status" is O(sub-agents) and recomputes when
any one of them changes. Where short-circuiting is possible, use `Pending` short-circuiting
(return on the first `Pending`, don't read the rest); where it isn't (a total token count,
say), either accept O(n) or make it incremental. **Pick one explicitly when you write such
an atom; don't default into it.**

## Persistence

### The interface

```rust
pub trait SessionStore<K, V, M> {
    fn append(&self, entry: &Entry<K, V, M>);
    fn drop_oldest(&self, count: usize);                    // cap overflow, from the oldest end
    fn drop_after(&self, first_seq: u64, count: usize);     // a new branch overwrites the redo tail
    fn set_cursor(&self, cursor: usize);
    fn snapshot(&self, snap: &Snapshot<K, V>);
    fn load(&self) -> LoadOutcome<K, V, M>;
}
```

**One instance per session**, with no `SessionId` parameter on the methods (the original
draft had one; it was removed during implementation). §"sub-agents" already fixes "one root
agent + subtree = one session = one actor thread = one store," so multiple sessions means
one `SessionStore` instance each, not one instance routing by id — which file or which table
something routes to is the host's business, not this port's.

**`load` is three-valued, not an `Option`** (found by an independent test in issue 027; the
`Option` design was rejected):

```rust
pub enum LoadOutcome<K, V, M> {
    Absent,                              // this identity has never written anything → starting fresh is correct
    Refused { reason: String },          // a session exists but this data can't be safely loaded → must fail hard
    Loaded(LoadedSession<K, V, M>),      // { snapshot, entries, cursor, next_seq }
}
```

An `Option` compresses "the file doesn't exist" and "a session exists but loading it is
refused (corruption in the middle)" into the same `None`, leaving the host one path —
treat it as brand new, and then **the first snapshot overwrites the user's original file**,
destroying data that was still manually recoverable a moment earlier. "No session" and "a
session that can't be read" are entirely different situations for a host, so they must be
different values. `reason` carries only diagnostics like a category and a line number and
**never K/V content** — that could be the user's conversation.

**Writes are all fire-and-forget with no return value.** A failure doesn't roll back
in-memory state, it's reported through an `on_error` callback — otherwise one IO hiccup would
wedge undo permanently. That's a lesson from the upstream TypeScript version, adopted
directly.

**The synchronous trait is deliberate.** The actor is single-threaded; writes go over an
mpsc to a dedicated IO thread, so the actor doesn't block and `agent-core` never has to
become async.

Implementations plug in freely: `Memory` (tests), `Jsonl` (file append), `Sqlite`, `Redis`,
`Postgres`, or the enterprise's own. They can be layered — snapshots and logs on different
backends, even chosen per session (a scratch session in `Memory`, an important one on disk).
It's a matter of which `Arc<dyn SessionStore>` you pass when constructing the session.

`Memory` and `Jsonl` are the first two (both landed). Where they live is decided by red line
7: `Memory` does zero IO, like the port definition itself, so it lives in `agent-store`;
`Jsonl` does real file IO and lives in `agent-runtime` — `agent-core` and `agent-store` are
both forbidden IO, so the runtime layer is the only place it can go. The two share one body
of "how cursors translate and how snapshots compact" logic (`SessionLog`): fork that
algorithm once and "write → load → replay is consistent" becomes two separately-maintained
derivations that will eventually disagree.

### Recovery *is* redo

Load the most recent snapshot, then push every subsequent `Entry` forward by its `next`.
**That is the redo loop, the same function** — there is no second loading path.

This is the justification for "derived read fns must be pure" (red line 1): a replay must
arrive at the same answer. Read the clock, take a random number, or read a mutable global
inside a read fn, and the derived values after recovery differ from those before the crash —
without erroring.

**Recovery is faithful replay, not "rebuild using today's configuration"** (issue 073 turned
this sentence into a decidable acceptance criterion). Tool declarations a host injects at
`POST /sessions` are therefore **session state** (`Slot::HostTools`, journaled once at
creation), not deployment configuration re-reported on every connection: a recovered session
comes back with **the tool table it had at the time**, not the host's current list.
Otherwise the historical conversation would contradict itself (the model once said "I called
`web:crm/lookup`," which may not exist in today's list), and since the tool table sits at
the very front of the prompt, swapping it means the recovered session's first turn has a
completely broken prefix (red line 11). The same reasoning explains why skills store
**activated ids** rather than body text: that asset has another owner in a registry outside
the store, whereas an injected declaration has **no second copy** anywhere outside it.

### Interruption semantics

Restoring state is the easy part; the hard part is what to do about things that were in
flight. The design's answer reuses the `ToolCallRequest.reversibility` snapshotted at
dispatch — the same judgment undo makes when it hits an irreversible operation:

| state at crash time | recovery strategy | today |
|---|---|---|
| tool call in flight, `Pure` | re-send it | ⛔ input missing |
| tool call in flight, `Reversible` | run the compensating action, then re-send | ⛔ input missing |
| tool call in flight, `Irreversible` | **must not re-send**; mark `Unknown` and ask the user "this may already have executed" | ⛔ input missing |
| LLM stream cut mid-generation | roll back the whole turn — that's `undo(turn)`, the same function | ✅ |
| MCP connections, SSE senders | not snapshotted; just reconnect | ✅ |

**The upper half of that table currently has no on-disk basis**: what it needs is the
`Reversibility` as of dispatch, which is exactly the `ToolCallSlot::Request` that never
landed (above). Today that lives only in host memory (`RunnerCtx`'s `PendingRemoteTool`) and
dies with the process. `Session::restore` only pours in the snapshot and pushes entries
forward, so a tool slot holding `Pending` comes back still `Pending` while its execution site
is gone.

**This does not mean red line 6 or the undo barrier has a hole** — the barrier bit
`EntryMeta.barrier` is on disk, and the epoch resumes from the log's maximum plus one. The
hole is only on the crash-recovery path.

There are two ways to fix it, and **you should verify before choosing rather than
implementing the table above directly**: (1) have the host that owns the tool table record a
`ToolCallSlot::Request` snapshot, or (2) take the route in the next paragraph — erase the
incomplete turn entirely, which would make the upper half of the table **a superfluous design
that should be deleted rather than built**. Run a `kill -9` while a tool call is in flight
first and see what actually happens (does it wedge in `ToolsPending` forever?), then decide.

Second-to-last row: **an incomplete turn is erased by a turn-granularity undo.** The turn
layer of the two undo granularities turns out to be exactly the atomicity boundary for crash
recovery — not a coincidence, the same concept.

## Message history uses a persistent vector

The `messages` slot must **not** be `Arc<Vec<Message>>`. Appending requires `make_mut`,
cloning the whole Vec each time; a thousand-message session appending several times per turn
is O(n) copying over and over.

Use `imbl::Vector` (`im` is unmaintained; `imbl` is a maintained fork): append is O(log n)
with structural sharing, so keeping old versions in the undo log costs almost nothing —
exactly what this undo design wants. `PartialEq` can take a structurally-shared pointer fast
path too.

By the same reasoning, every `AgentValue` variant that could grow large is `Arc`-wrapped
(red line 5). `store.set` uses `PartialEq` to decide whether anything changed and therefore
whether to propagate; without a `ptr_eq` fast path, every write is a deep comparison.

## What you get for free

Once the command log exists, these require no additional code:

- branching from any point in history ("what if I'd asked it differently back there")
- replaying a session for someone else to watch
- reproducing a bug exactly
- auditing
