# 093 — Vision Delegation for Non-Visual Agents

> Canonical English issue. A short Chinese companion is available in
> [093-vision-subagent-delegation.zh-CN.md](./093-vision-subagent-delegation.zh-CN.md).

**Milestone:** M12 · **Status:** in progress · **Lead model:** `gpt-5.6-sol`

## Outcome

A non-visual root agent such as DeepSeek can inspect user-attached images through a narrowly
scoped Kimi child agent and then continue the original turn with the child's textual findings.

The model calls one purpose-built tool:

```text
DeepSeek root
  -> srv:vision/inspect(selected image handles, question)
  -> trusted vision execution profile
  -> Kimi child with no parent history and no tools
  -> textual observation returned as the tool result
  -> DeepSeek completes the answer
```

The important capability is not a provider fallback. It is an explicit, observable delegation:
the parent selects the images and question, the server selects the trusted model route, and the
child receives the minimum data required for that inspection.

## Decisions

1. **Use a dedicated Tool, not a Skill, for V1.** `srv:vision/inspect` has a small stable schema and
   one clear behavior. A Skill would add an activation round trip without adding knowledge the
   model needs. If vision later becomes a catalog of OCR, chart, document, and UI workflows, that
   catalog may be wrapped in a lazily activated Skill without changing this Tool's contract.
2. **Reuse a generic child-launch substrate, not the public spawn request verbatim.** Execution
   profile selection, context policy, selected attachments, and child tool grants are common launch
   inputs. The vision facade fixes them to safe values.
3. **The model never chooses a provider, endpoint, API key, or raw model name.** It can only invoke
   the vision capability. The server resolves an allowlisted `execution_profile_id`.
4. **Context and attachments are independent.** The vision child receives no parent conversation
   history, only the explicit question and selected images. It receives an empty tool table.
5. **Retain non-visual ingress images in a provider-neutral vault.** Today their bytes are discarded
   before a child can use them. V1 keeps bounded session-scoped attachments in server memory.
6. **Handles are references, not credentials.** The model sees stable `img_*` handles; every lookup
   is still scoped to the owning session. Bytes, local paths, and provider references never enter
   prompts, SSE, errors, journals, or snapshots.
7. **V1 attachment recovery is explicitly ephemeral.** Session close, expiry, capacity eviction, or
   process restart can make a handle unavailable. The Tool returns `attachment_unavailable`; it
   never silently asks the non-visual provider to continue as if inspection succeeded.
8. **Child routing is durable; secrets are not.** Core persists only the resolved opaque execution
   profile ID in the same spawn entry as the child grant. Runtime resolves that ID to live clients,
   adapters, endpoints, keys, model settings, and timeouts. Missing routes fail closed.
9. **Inspection is model-demand in V1.** The non-visual placeholder names the available image
   handles and tells the model to call `srv:vision/inspect` when visual evidence is needed. Guaranteed
   automatic inspection is a separate product mode, not an implicit fallback.

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

Rules:

- `images` contains 1–8 distinct handles from the current session;
- `question` is non-empty and bounded;
- the tool is pure from the agent timeline's perspective;
- a successful result is the child's final text plus non-secret observation metadata;
- a failure is a terminal tool result with a stable code and `is_error=true`;
- `retryable` is information, never authorization for an automatic retry.

Stable failure codes:

| Code | Retryable | Meaning |
|---|---:|---|
| `invalid_input` | no | Invalid handles, count, or question |
| `attachment_not_found` | no | Handle never belonged to this session |
| `attachment_unavailable` | no | Known handle expired, was evicted, or was lost on restart |
| `image_unsupported` | no | MIME or image payload cannot be used |
| `vision_profile_unavailable` | yes | Configured vision execution profile is absent |
| `vision_upload_failed` | yes | Selected image could not be materialized for Kimi |
| `vision_timeout` | yes | The child exceeded its deadline |
| `vision_rejected` | no | Provider or policy rejected the inspection |
| `vision_child_failed` | depends | Sanitized child/provider failure |
| `vision_cancelled` | no | Parent turn or session cancelled the inspection |

## State Protocol

```text
registered -> available -> leased -> available
     |            |          |
     |            |          +-> unavailable (close/expiry/eviction/restart)
     |            +------------> unavailable
     +-------------------------> rejected

vision call:
accepted -> resolving -> uploading -> running_child -> succeeded
    |           |            |              |
    +-----------+------------+--------------+-> failed | cancelled
```

- Registration finishes before the user event is accepted by the session actor.
- A lease prevents expiry/eviction while an inspection is actively reading an attachment.
- Cancellation propagates parent → child → upload/read lease.
- Every path reaches one terminal Tool result; no pending call is left waiting for a frontend.
- Repeated inspection may reuse a route-scoped materialized reference while it remains valid, but
  that provider reference is private runtime state and is never accepted from the model.

## Work Tree

Each leaf is intended to be independently reviewable. `SOL` means `gpt-5.6-sol`; `TERRA` means
`gpt-5.6-terra`.

```text
093 Vision delegation [SOL, root]
├─ A. Freeze contracts [SOL, root]
│  ├─ A1. Dedicated Tool vs Skill decision
│  ├─ A2. Execution-profile trust boundary
│  ├─ A3. Attachment lifecycle and error vocabulary
│  └─ A4. Context/attachment isolation invariants
├─ B. Durable child execution identity [SOL, Plato]
│  ├─ B1. Move ChildConfig to one-purpose module
│  ├─ B2. Add private ExecutionProfile slot
│  ├─ B3. Write grant + resolved profile in one spawn entry
│  ├─ B4. Add read API with legacy Null behavior
│  └─ B5. Prove spawn/undo/redo/restore identity
├─ C. Runtime profile routing [SOL, root after B]
│  ├─ C1. Define immutable execution binding
│  ├─ C2. Build deterministic allowlisted registry
│  ├─ C3. Resolve profile per agent before provider IO
│  ├─ C4. Snapshot adapter and route for in-flight calls
│  ├─ C5. Partition guard/cache accounting by profile
│  └─ C6. Fail closed when a durable ID is missing
├─ D. Spawn request cleanup [TERRA, Herschel]
│  ├─ D1. Extract schema and parser from oversized spawn_tool.rs
│  ├─ D2. Preserve byte-stable existing schema
│  ├─ D3. Preserve task/tools/background validation
│  └─ D4. Bring spawn_tool.rs below 300 lines
├─ E. Provider-neutral attachment vault [SOL, Boole]
│  ├─ E1. Define session-scoped image handle
│  ├─ E2. Register validated bytes with quotas
│  ├─ E3. Lease bytes during inspection
│  ├─ E4. Mark close/expiry/eviction as unavailable
│  ├─ E5. Reject cross-session and unknown handles
│  └─ E6. Prove no byte/path/reference leakage
├─ F. Non-visual ingress handoff [SOL, root after E]
│  ├─ F1. Register bytes before actor input
│  ├─ F2. Persist only safe handles in UserImage references
│  ├─ F3. Include handles in non-visual placeholders
│  ├─ F4. Preserve direct Kimi vision via request-time materialization
│  └─ F5. Preserve byte-identical text-only requests
├─ G. Vision facade and child launch [SOL, root after B/C/E/F]
│  ├─ G1. Add strict vision Tool spec and parser
│  ├─ G2. Resolve and lease selected handles
│  ├─ G3. Materialize images through the vision route
│  ├─ G4. Spawn child with no history and no tools
│  ├─ G5. Return child final text as Tool result
│  └─ G6. Propagate timeout/cancel/failure terminally
├─ H. Configuration [TERRA, root]
│  ├─ H1. Load all named provider sections
│  ├─ H2. Map capability `vision` to one trusted profile
│  ├─ H3. Validate vision capability at startup
│  └─ H4. Keep endpoint/key/model out of public schemas
├─ I. Verification [SOL, agents split by ownership]
│  ├─ I1. Core persistence and undo matrix
│  ├─ I2. Vault ownership/quota/expiry/concurrency matrix
│  ├─ I3. Fake DeepSeek parent -> fake Kimi child E2E
│  ├─ I4. Assert parent request contains no image reference
│  ├─ I5. Assert child request contains only selected images
│  ├─ I6. Restore with missing profile fails before HTTP
│  ├─ I7. Cancel/timeout/repeated-inspection matrix
│  └─ I8. Real Kimi dogfood, one paid call, explicit approval
└─ J. Follow-ups, not blockers [TERRA]
   ├─ J1. Persistent encrypted artifact store across restart
   ├─ J2. Optional parent-summary context policy
   ├─ J3. Lazy vision Skill catalog if workflows expand
   └─ J4. Optional automatic-inspection product mode
```

## Dependencies and Commit Batches

```text
Batch 1: A + B + D + E       contracts and independent foundations
Batch 2: C + H               trusted multi-provider runtime routing
Batch 3: F                   non-visual attachment preservation
Batch 4: G                   end-to-end vision delegation
Batch 5: I                   recovery, security, and E2E proof
```

Each batch gets its own commit. Parallel agents must not commit shared work; the lead integrates,
runs focused tests, checks `wc -l`, and commits explicit paths so the pre-existing dirty worktree is
not accidentally included.

## Acceptance Criteria

1. A DeepSeek-root session with an image can call `srv:vision/inspect`; a Kimi child sees the selected
   image and its final observation returns to DeepSeek in the same turn.
2. The Kimi child receives no parent history, no unselected image, and no tools.
3. The model can select only session image handles. It cannot select or observe provider, endpoint,
   model, key, local path, raw bytes, or provider upload reference.
4. Root and child calls can use different fake providers concurrently without route, adapter,
   timeout, guard, or cache-accounting cross-talk.
5. Child execution profile identity survives snapshot, recovery, undo, and redo. A missing configured
   profile fails before any HTTP request and never falls back to the root provider.
6. Attachment close, expiry, eviction, restart loss, malformed payload, upload failure, timeout,
   cancellation, and provider rejection all produce explicit terminal Tool results.
7. Kimi-root image input and all text-only input remain regression-free.
8. No touched regular file exceeds 300 physical lines; any strongly cohesive exception stays at or
   below 500 lines with a written reason.

## Non-Goals

- Giving arbitrary model/provider selection to the LLM.
- Copying the complete parent history into the vision child.
- Allowing the child to call shell, network, spawn, MCP, host, or vision tools.
- Treating `ms://`, URLs, filesystem paths, or browser object URLs as durable attachment IDs.
- Silently retrying a paid or externally observable provider call.
- Building a general lazy-detail protocol for every standalone Tool in this issue.
