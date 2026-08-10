# 093 — Vision Delegation for Non-Visual Agents

> ⚠️ **Superseded by s5**: the vision sub-agent delegation pipeline described here was
> removed by the s5 refactor. The current implementation is the `srv:vision/inspect` built-in
> tool fed by the `POST /uploads` endpoint. Kept as a historical record only.

> Canonical English issue. A short Chinese companion is available in
> [093-vision-subagent-delegation.zh-CN.md](./093-vision-subagent-delegation.zh-CN.md).

**Milestone:** M12 · **Status:** in progress · **Lead model:** `gpt-5.6-sol`

## Outcome

A non-visual root agent such as DeepSeek can inspect user-attached images through a narrowly
scoped Kimi child and then continue the same turn with the child's textual observation.

```text
DeepSeek root
  -> srv:vision/inspect(selected image handles, question)
  -> trusted vision execution profile
  -> Kimi child with no parent history and no tools
  -> textual Tool result
  -> DeepSeek completes the answer
```

This is explicit, observable delegation rather than provider fallback. The parent selects the
images and question; the server selects the trusted route; the child receives only what that
inspection requires.

## Decisions

1. **Use a dedicated Tool, not a Skill, for V1.** The schema is small and stable. A lazy Skill
   catalog remains possible if OCR, chart, document, and UI workflows later need instructions.
2. **Reuse a generic child-launch substrate, not the public spawn request verbatim.** The vision
   facade fixes execution profile, context policy, attachments, and tool grants to safe values.
3. **The model never selects provider material.** Provider, endpoint, key, raw model name, upload
   reference, and timeout come only from an allowlisted server-side execution profile.
4. **Context and attachments are independent.** The child gets the explicit question, selected
   images, a fixed isolated system prompt, no parent history, and an empty tool table.
5. **Ingress images live in a bounded provider-neutral vault.** Only session-scoped `img_*` handles
   enter agent state; bytes and provider references remain runtime-private.
6. **Handles are references, not credentials.** Every lookup is scoped to the owning session.
   Bytes, paths, names shaped like paths, provider references, and secrets must not leak through
   prompts, SSE, errors, journals, or snapshots.
7. **V1 recovery and ownership memory are ephemeral and bounded.** Close, expiry, eviction, or
   restart makes a retained same-session handle `attachment_unavailable`. Once its bounded
   tombstone is pruned, it is indistinguishable from an unknown handle and returns
   `attachment_not_found`. Inspection never silently continues without visual evidence.
8. **Child routing identity is durable; bindings and secrets are not.** Core persists only the
   opaque execution profile ID in the spawn entry. Runtime resolves it and fails closed when absent.
9. **Inspection is model-demand.** The non-visual placeholder advertises safe handles and the Tool.
   Guaranteed automatic inspection is a separate product mode.
10. **Reuse the stable upload path unchanged.** Request-time materialization calls the existing
    synchronous `Client::upload_image`. `agent-transport` is outside this issue's scope and has no
    change for vision cancellation or timeout.

## Tool Contract

```json
{
  "name": "srv:vision/inspect",
  "arguments": {
    "images": ["img_1"],
    "question": "Read the visible text and explain the error shown."
  }
}
```

- `images` contains 1–8 distinct canonical handles; `question` is non-empty and bounded.
- The model cannot supply a profile, provider, endpoint, model, key, URL, path, or upload reference.
- Success returns final child text plus non-secret metadata.
- Failure returns a stable safe envelope with `is_error=true`; `retryable` never authorizes an
  automatic retry of a paid or externally observable call.

| Code | Retryable | Meaning |
|---|---:|---|
| `invalid_input` | no | Invalid handles, count, fields, or question |
| `attachment_not_found` | no | Not known for this session in the bounded in-memory ownership index |
| `attachment_unavailable` | no | Known same-session handle has no readable bytes while its tombstone remains |
| `image_unsupported` | no | MIME or payload cannot be used |
| `vision_profile_unavailable` | yes | Trusted vision execution profile is absent |
| `vision_upload_failed` | yes | Selected image could not be materialized |
| `vision_timeout` | yes | The child attempt exceeded its deadline |
| `vision_rejected` | no | Provider or policy rejected the inspection |
| `vision_child_failed` | depends | Sanitized child/provider failure |
| `vision_cancelled` | no | The inspection was cancelled while its parent turn remained live |

## State and Cancellation Protocol

```text
registered -> available -> leased -> available
     |            |          |
     |            |          +-> unavailable -> tombstone pruned -> unknown
     |            +------------> unavailable
     +-------------------------> rejected

vision call:
accepted -> resolving -> uploading -> running_child -> succeeded
    |           |            |              |
    +-----------+------------+--------------+-> failed | locally_cancelled
```

- Registration completes before the actor accepts the user event. A lease delays reclamation while
  local preparation reads an attachment.
- Cancellation or timeout irreversibly abandons the exact `(agent, attempt)` credential. The
  call-local latch is checked before upload, between selected images, and before Kimi chat encoding,
  so no later upload or chat starts and late attempt messages cannot land.
- An already-started synchronous `Client::upload_image` may physically finish. Its local preparation
  then observes the latch, exits, and releases its leases; V1 promises neither physical abort nor
  immediate lease release while that upload is blocked.
- Every inspection path except cancellation of the containing parent turn settles a Tool result.
  Parent/session cancellation terminates and erases that turn, so no Tool result is promised after
  erasure. It leaves no resumable logical child or pending frontend receipt; a lease held by an
  already-started preparation is released when that synchronous work returns.
- Every uploaded provider reference is request-local to one inspection. It exists only while
  preparing/sending that provider request and is never reused, public, durable, or model-supplied.

## Execution Ledger

Leaf notation is `[status | owner/model | wave | dependencies] objective — evidence`. Status is one
of `blocked`, `ready`, `active`, `review`, or `done`; `SOL` handles state/security/integration and
`TERRA` handles bounded cleanup/docs/tests. Each leaf owns only the evidence path named on its line
and excludes unrelated production or test modules. Verification leaves own tests only. All leaves
also exclude arbitrary model selection, parent-history copying, and automatic paid-call retries.

```text
093 Vision delegation [in progress | lead/SOL]
├─ A. Contracts
│  ├─ A1 [done|lead/SOL|W0|-] Choose dedicated Tool over Skill — Decisions 1
│  ├─ A2 [done|lead/SOL|W0|-] Freeze profile trust boundary — Decisions 2–3,8
│  ├─ A3 [done|lead/SOL|W0|-] Freeze bounded attachment/error semantics — Decisions 5–7
│  └─ A4 [done|lead/SOL|W0|-] Freeze context/tool isolation — Decision 4
├─ B. Durable child identity
│  ├─ B1 [done|Plato/SOL|W1|A2] Isolate ChildConfig — core/command/child_config.rs
│  ├─ B2 [done|Plato/SOL|W1|B1] Add private ExecutionProfile slot — core/graph/slot.rs
│  ├─ B3 [done|Plato/SOL|W1|B2] Atomically spawn grant/profile/retry — core/command/spawn.rs
│  ├─ B4 [done|Plato/SOL|W1|B2] Read profile with legacy Null — core/command/read.rs
│  └─ B5 [done|Plato/SOL|W1|B3–B4] Prove undo/redo/restore — execution_profile.rs
├─ C. Runtime routing
│  ├─ C1 [done|routing/SOL|W2|B] Define immutable binding — runtime/execution_binding.rs
│  ├─ C2 [done|routing/SOL|W2|C1] Build deterministic allowlist — execution_binding_tests.rs
│  ├─ C3 [done|routing/SOL|W2|C1] Resolve per agent before I/O — runtime/provider_call.rs
│  ├─ C4 [done|routing/SOL|W2|C3] Snapshot in-flight route/adapter — provider_call_tests.rs
│  ├─ C5 [done|routing/SOL|W2|C1] Partition guard/cache scope — execution_binding_tests.rs
│  └─ C6 [done|routing/SOL|W2|C3] Fail closed on missing route — provider_call_tests.rs
├─ D. Spawn cleanup
│  ├─ D1 [done|Herschel/TERRA|W1|-] Extract request schema/parser — runtime/spawn_request.rs
│  ├─ D2 [done|Herschel/TERRA|W1|D1] Preserve schema bytes — spawn_request.rs tests
│  ├─ D3 [done|Herschel/TERRA|W1|D1] Preserve validation — spawn_request.rs tests
│  └─ D4 [done|Herschel/TERRA|W1|D1] Keep facade below 300 lines — spawn_tool.rs (242)
├─ E. Attachment vault
│  ├─ E1 [done|Boole/SOL|W1|A3] Define canonical session handle — attachments/handle.rs
│  ├─ E2 [done|Boole/SOL|W1|E1] Validate/register with quotas — attachments/store.rs tests
│  ├─ E3 [done|Boole/SOL|W1|E2] Hold bytes by read lease — attachments/tests.rs
│  ├─ E4 [done|Boole/SOL|W1|E3] Revoke on close/expiry/eviction — attachments/tests.rs
│  ├─ E5 [done|Boole/SOL|W1|E1] Hide cross-session/unknown handles — attachments/tests.rs
│  └─ E6 [done|Boole/SOL|W1|E4] Bound unavailable tombstones — attachments/index.rs
├─ F. Ingress and direct vision
│  ├─ F1 [done|ingress/SOL|W3|E2] Register before actor input — http/routes/input.rs
│  ├─ F2 [done|ingress/SOL|W3|F1] Persist only attachment handles — image_user_input_jsonl.rs
│  ├─ F3 [done|ingress/SOL|W3|F2] Advertise safe handles to non-visual root — nonvisual_image_input.rs
│  ├─ F4 [review|Maxwell/SOL|W3|E3,C] Materialize direct Kimi images via unchanged upload client — image_materialization.rs
│  ├─ F5 [review|Maxwell/SOL|W3|F4] Preserve existing text-only request shape — http_image_input.rs
│  └─ F6 [review|name-privacy/TERRA|W3|E2] Accept basenames, reject path-shaped names — validation.rs
├─ G. Vision facade and child execution
│  ├─ G1 [done|vision/SOL|W4|A] Parse strict 1–8-handle request — core/vision/request.rs tests
│  ├─ G2 [done|vision/SOL|W4|C,H] Expose canonical root-only facade — vision_tool_tests.rs
│  ├─ G3 [review|Maxwell/SOL|W4|E3,F4] Resolve selected handles under leases — image_resolver.rs
│  ├─ G4 [review|Maxwell/SOL|W4|G3] Upload selected images with request-local refs — image_materialization.rs
│  ├─ G5 [done|vision/SOL|W4|B,C] Launch child with isolated system/history/tools — vision_profile_tests.rs
│  ├─ G6 [done|vision/SOL|W4|G5] Return non-empty child text safely — vision_child_outcome_tests.rs
│  ├─ G7 [review|Maxwell/SOL|W4|G4] Classify upload status into stable codes — image_preparation_failure_tests.rs
│  ├─ G8 [done|vision/SOL|W4|G5] Pin paid child retry budget to zero — vision_tool_tests.rs
│  ├─ G9 [review|Maxwell/SOL|W4|G3] Latch timeout and suppress later work — deadline.rs, provider_call.rs
│  ├─ G10 [review|Maxwell/SOL|W4|G3] Abandon attempt; release leases after sync work — runner.rs
│  ├─ G11 [review|Maxwell/SOL|W4|G9–G10] Ignore late blocking-I/O completion — io_thread.rs
│  ├─ G12 [done|vision/SOL|W4|G6] Redact child/provider failures — vision_child_outcome.rs
│  └─ G13 [active|output-privacy/SOL|W4|G6] Close exact provider-ref echo across public/durable output — vision_output_privacy.rs, output_privacy.rs
├─ H. Configuration
│  ├─ H1 [done|config/TERRA|W2|C1] Load named provider sections — server/bootstrap.rs
│  ├─ H2 [done|config/TERRA|W2|H1] Map capability vision to trusted profile — bootstrap.rs
│  ├─ H3 [done|config/TERRA|W2|H2] Reject non-visual vision binding — bootstrap tests
│  └─ H4 [done|config/TERRA|W2|H1] Keep provider material private — http/config.rs
├─ I. Verification
│  ├─ I1 [done|core-audit/SOL|W5|B] Persistence/undo matrix — execution_profile.rs
│  ├─ I2 [done|vault-audit/SOL|W5|E] Ownership/quota/expiry/concurrency — attachments/tests.rs
│  ├─ I3 [done|e2e/SOL|W5|G] DeepSeek→Kimi happy path — http_vision_delegation/success.rs
│  ├─ I4 [done|privacy/SOL|W5|I3] Parent sees handles, no image/provider ref — success.rs
│  ├─ I5 [done|privacy/SOL|W5|I3] Child sees selected images only — success.rs
│  ├─ I6a [done|core-audit/SOL|W5|B] Restore preserves profile identity — execution_profile.rs
│  ├─ I6b [done|runtime-audit/SOL|W5|C6] Missing profile fails pre-I/O — provider_call_tests.rs
│  ├─ I7a [done|Maxwell/SOL|W5|G9] Blocked-upload timeout and late completion — timeout.rs, timeout/upstream.rs
│  ├─ I7b [done|Maxwell/SOL|W5|G10] Cancel during sync upload; stop batch — image_materialization_tests.rs
│  ├─ I7c [done|e2e/SOL|W5|G4] Two sequential inspections without cross-talk — repeated_inspection.rs
│  ├─ I8 [ready|operator/SOL|W6|I15] Optional paid Kimi dogfood, not run — requires explicit approval; non-blocking
│  ├─ I9 [done|recovery/SOL|W5|E6,G] Restart loss fails before Kimi I/O — restart.rs
│  ├─ I10 [review|error-audit/SOL|W5|G7,G12] Error/status redaction matrix — failures.rs
│  ├─ I11 [review|name-privacy/TERRA|W5|F6] Name validation/privacy E2E — http_image_name_privacy.rs
│  ├─ I12 [done|privacy/SOL|W5|I3] No history/tool/secret/raw-byte leakage — success.rs
│  ├─ I13 [review|direct-vision/SOL|W5|F4] Kimi-root regression — http_image_input.rs
│  ├─ I14 [review|direct-vision/SOL|W5|F5] Text-only request-shape regression — http_image_input.rs
│  └─ I15 [blocked|acceptance/SOL|W6|G13,I1–I7c,I9–I14] Clean committed-worktree verification — expected: focused test transcript
└─ J. Follow-ups, not blockers
   ├─ J1 [ready|future/TERRA|-|-] Persistent encrypted artifact bytes across restart — evidence: deferred by V1 scope
   ├─ J2 [ready|future/TERRA|-|-] Optional parent-summary context policy — evidence: deferred by Decision 4
   ├─ J3 [ready|future/TERRA|-|-] Lazy vision Skill catalog if workflows expand — evidence: deferred by Decision 1
   ├─ J4 [ready|future/TERRA|-|-] Optional automatic-inspection product mode — evidence: deferred by Decision 9
   └─ J5 [ready|future/SOL|-|-] Durable/unbounded ownership and tombstone metadata — evidence: deferred by Decision 7
```

I6a and I6b are intentionally separate proofs; this issue does not claim one combined
restore-plus-HTTP E2E. G13 remains active: it must carry the exact request-local references to one
terminal output gate, suppress unsafely partial raw vision deltas, recursively scrub exact matches
from text/JSON, and prove no leak through SSE, journal, Tool outcome, or the next root request while
preserving normal observations. It must not guess from an `ms://` prefix. I8 has not run and may be
skipped unless an operator explicitly authorizes the paid call; it does not block completion.

## Waves and Delivery

```text
W0 contracts -> W1 core/vault/cleanup -> W2 routing/config -> W3 ingress/materialization
             -> W4 facade/child execution -> W5 focused and E2E verification -> W6 acceptance
```

The lead reviews each returned diff before unlocking dependants. Agents do not stage or commit
shared work. Delivery uses explicit paths and coherent batches; unrelated dirty-worktree files stay
untouched. I15 must validate the committed state in an isolated clean worktree so local edits cannot
mask failures.

## Acceptance Criteria

1. DeepSeek can request inspection of selected session handles; isolated Kimi text returns in-turn.
2. Kimi receives no parent history, unselected image, host system context, or tools.
3. Provider material, including a provider reference echoed by the child, raw bytes, paths, secrets,
   and unsafe names never cross public or durable surfaces.
4. Root and child provider bindings, timeouts, guards, and cache accounting cannot cross-talk.
5. Profile identity survives core restore/undo/redo; separately, a missing route fails before I/O.
6. Non-cancellation failures settle explicit safe Tool results. Parent/session cancellation instead
   ends the containing turn; started synchronous upload may finish, but its late `(agent, attempt)`
   result is ignored and cannot start another upload or chat.
7. Direct Kimi image input and the existing text-only request shape remain regression-free.
8. Required verification leaves pass in a clean committed worktree; optional I8 is not required.
9. New regular/test files stay at or below 300 physical lines unless a written cohesive-engine
   exception applies. `runner.rs` is currently a 331-line cohesive event-pump/state-machine:
   splitting its in-flight table, deadline, and cancellation transition would fragment one state
   machine across files; it remains below the 500-line complex-engine ceiling. Two lightly edited
   legacy tests were already over 300 lines and were not opportunistically refactored:
   `agent-core/tests/it/observe_046.rs` and
   `agent-server/tests/it/http_capabilities_survive_restart.rs`.

## Non-Goals

- Arbitrary provider/model selection by the LLM or copying parent history into the child.
- Child access to shell, network, spawn, MCP, host, or vision tools.
- Treating `ms://`, URLs, filesystem paths, or object URLs as durable attachment IDs.
- Guaranteed physical interruption of already-started blocking HTTP I/O.
- Changes to the stable `agent-transport` upload implementation.
- General lazy Tool-detail loading or automatic inspection in this issue.
