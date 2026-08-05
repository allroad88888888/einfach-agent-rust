# Host-Native Tools and Skills

> Canonical English overview. A short Chinese summary is available in
> [092-remote-tool-result-protocol.zh-CN.md](./092-remote-tool-result-protocol.zh-CN.md).

## The Core Idea

The frontend is not a passive chat UI. It can teach the agent new tools and skills at session
creation time, while Rust keeps model context small and turns frontend execution into a reliable,
observable protocol.

This creates one end-to-end capability system:

```text
Frontend-defined tools and skills
              │
              ▼
        Compact skill index
              │
       AI selects what it needs
              │
              ▼
 Selected bundle loaded on demand
              │
              ▼
 Atomic execution through the host
              │
              ▼
 Strongly acknowledged final result
```

## 1. Bring Your Own Tools and Skills

A frontend can attach its own capabilities when it creates a session:

- tools implemented by the browser, desktop shell, or Java host;
- skills containing domain instructions and related tool declarations;
- capability metadata scoped to that session rather than installed globally.

This means the same Rust agent core can immediately operate inside different products without
hard-coding every product integration into Rust. A finance UI, design tool, internal admin system,
or desktop application can each expose a different native capability surface.

The declarations are validated, deterministically ordered, journaled with the session, and restored
with the conversation. Historical behavior therefore does not silently change because a deployment
later changes its local files.

## 2. Skill Bundles Keep the AI Context Lean

Large host capability surfaces can be packaged as skills. At the beginning of a conversation, the
model receives only a compact, stable skill index rather than every skill body and every tool schema
inside those skills.

The index tells the model what exists, what each bundle is for, and how to activate it. The model
selects the relevant skill, and only then does the agent load its complete instructions and tool
definitions.

This is the important difference between capability discovery and capability injection:

```text
At session start          After explicit activation
----------------          -------------------------
names                     full skill instructions
short descriptions        complete tool schemas
activation entrypoints    execution-ready definitions
```

Standalone host tools remain the eager path for a small always-available surface. Skill bundles are
the lazy path for a large catalog. The result is a much better scaling model:

- adding capabilities does not linearly bloat every AI request;
- unrelated business instructions do not distract the model;
- stable indexes preserve provider prompt-cache efficiency;
- the model can discover a large host surface while paying detail cost only for what it uses.

## 3. Exactly One Connected Executor Starts the Work

A tool call may be visible to multiple browser tabs or host processes. Before causing a side effect,
an executor must atomically claim the call.

```text
                 one atomic claim
Browser A ─────────────┐
                      ├──► Rust actor ───► one winner
Browser B ─────────────┘                  one explicit conflict
```

Only a client receiving `claimed` or `already_claimed_by_you` may execute. Every other client gets
`tool_claimed_by_other` and must remain an observer.

The claim is serialized inside the session actor, so it is not a best-effort frontend convention.
It is the server-side gate immediately before external execution. This prevents two live tabs from
placing the same order, sending the same message, or applying the same mutation.

## 4. HTTP Success Means the Result Was Really Committed

Tool result submission is a request/reply operation with a strong acknowledgement. HTTP 200 does
not mean “accepted into a queue.” It means the actor verified the claim, passed the current-session
epoch gate, and committed the terminal result to the active tool slot.

Every final submission has a stable `submission_id`:

- the first valid submission returns `committed`;
- replaying the identical submission returns `duplicate` without advancing the model twice;
- changing the payload or submission identity returns `result_conflict`;
- stale, cancelled, unknown, and credential-mismatched calls return distinct structured errors.

This makes retry safe when the result was committed but the HTTP response was lost. The executor
re-sends the result; it never re-runs the external tool.

## 5. It Never Lies About an Unknown Side Effect

Distributed systems cannot always prove what happened outside their process. This protocol makes
that uncertainty explicit instead of disguising it as a normal timeout.

```text
No executor claimed before deadline  ──► unclaimed_timeout
Claimed, then host disappeared       ──► outcome_unknown
Host reported business failure       ──► failed
Conversation cancelled the call      ──► cancelled
Host committed a successful result   ──► succeeded
```

`outcome_unknown` is deliberately a first-class terminal state. It tells operators and the model
that a side effect may already have happened and must not be retried automatically. Business systems
that need crash-safe exactly-once behavior can use `tool_call_id` as their own idempotency key.

## Observable by Design

Executors and UIs can query the current call state, revision, deadlines, terminal origin, and whether
the supplied claim belongs to them. Every state transition increments a revision. Terminal receipts
are kept in a bounded session ledger, enabling deterministic duplicate detection without leaking
credentials or injecting protocol bookkeeping into the model prompt.

SSE remains useful for live timeline events, but it is not treated as a commit acknowledgement. The
synchronous result response is authoritative.

## Proven End to End

The design is covered beyond unit-level mocks:

- 100 real-TCP races each produced exactly one claim winner and one HTTP 409 loser;
- two real browser clients connected through the Java gateway with the same `chatid`;
- their side-effect counters were 1 and 0, proving the losing browser did not execute;
- replay after a simulated lost response returned `duplicate` without a second state transition;
- after a winner claimed and disconnected, the call became `outcome_unknown` with
  `terminal_origin=deadline`, while the observing browser remained connected and the session
  continued;
- Rust, Web, and Java proxy tests cover claim, result, status, headers, bodies, cancellation, undo,
  timeout, late results, and protocol compatibility.

## The Boundary of the Guarantee

The system guarantees one active protocol winner, idempotent result commitment, and honest state
reporting. It does not claim exactly-once execution inside an arbitrary external business system.
That last guarantee requires the external system to honor an idempotency key.

This boundary is intentional: the agent prevents every duplicate it can prove, refuses unsafe
automatic takeover, and clearly exposes the cases no distributed coordinator can infer.
