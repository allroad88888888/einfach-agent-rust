# Architecture

> Translated from [ARCHITECTURE.md](ARCHITECTURE.md) as of commit `5e45a2a`.
> **The Chinese version is authoritative** — development happens in Chinese, so this file
> can lag. If the two disagree, the Chinese one is right and this one is a bug.
> Terminology follows the table at the bottom of [INVARIANTS.en.md](INVARIANTS.en.md).
>
> To find out whether it has lagged, and by how much:
> `git log --oneline 5e45a2a..HEAD -- docs/ARCHITECTURE.md`. Empty output means this
> translation is current. If you update the translation, move the hash.

## In one sentence

All agent state lives in one atomic dependency graph. Source state is held in primitive
atoms; everything else — prompt assembly, pending aggregation, UI projections — is derived
and recomputed by the engine. Therefore **recording changes to source state is the same
thing as recording all state**, and undo, redo, crash recovery, and audit replay share that
one record.

## Five load-bearing decisions

**1. The state engine is not `Send`/`Sync`, deliberately.**
`Store` is `Rc<RefCell<Inner>>` and listeners are `Rc<dyn CellListener>`. That's the price
of its synchronous re-entrant semantics — a listener can write again synchronously and
propagation stays glitch-free. Switching to `Arc<Mutex>` would turn re-entrancy into a
deadlock risk for a benefit that isn't real. So: **each session owns one thread and its
store lives there**, with the outside world going in via `mpsc<Command>` and out via
`broadcast<Event>`. Only `agent-server` knows threads and tokio exist; `agent-core` is
always single-threaded from the inside.

A session's boundary is **one root agent plus its entire subtree** — every sub-agent shares
that one store and that one thread. Sub-agent concurrency is IO concurrency (LLM calls and
tool execution run as concurrent futures on the same thread via `FuturesUnordered`); state
write-back is always serialized onto the actor thread. See
[STATE-MODEL.md](STATE-MODEL.md) §"sub-agents".

**2. Prompt ingredients are derived atoms; wire assembly happens in the adapter.**
Swapping a skill recomputes only the system segment; adding a message doesn't re-run skill
work — the ingredients (the fields of `Ingredients`) are maintained incrementally by the
engine. Turning ingredients into wire JSON is a model-related decision and belongs to the
adapter's `encode` (decision 15, red line 12), which is a pure function whose inputs are
exactly those atom values. **If you use the store as a HashMap with callbacks, this
architecture has no reason to exist** — writing a plain struct would be faster.

**3. Persistence isn't a feature added later; it's the same code as undo.**
Recovery means starting from a snapshot and pushing the command log's `next` values
forward — which is precisely the redo loop. Once undo/redo was written, recovery was
already written. See [STATE-MODEL.md](STATE-MODEL.md).

**4. Tool calls are location-transparent.**
`agent-core` emits a `ToolCall` and knows nothing about "frontend" or "backend." The router
reads `location` off the descriptor and decides whether to execute locally or push it over
SSE and wait for the client to POST a result back. The two paths are isomorphic from the
core's perspective: dispatch, mark `Pending`, await write-back. See [TOOLS.md](TOOLS.md).

**5. The server does no authentication, log formatting, or clustering.**
Those are the enterprise's edge concerns, every organization's conventions differ, and
anything we build there they'd have to undo. The server reads identity headers without
verifying them and honors W3C `traceparent` without integrating any APM SDK. Enterprises
add the rest at their own gateway.

## Package layout

```
einfach-agent-rust/
  crates/                 ten of them (the workspace members in Cargo.toml are authoritative)
    agent-store/        atom engine + history + snapshot. Forked from einfach-core
    agent-core/         AgentValue, the atom graph, loop orchestration, registry/port traits. Zero IO
    agent-tools/        minimal built-in tools (srv:fs/read, fs/list, shell/exec) + local executor
    agent-providers/    LLM adapters (DeepSeek / Kimi / GLM, one directory each)
    agent-transport/    blocking HTTP (the only crate in the repo allowed to depend on ureq) + providers.toml parsing
    agent-mcp/          MCP adapter, produces (ToolSpec, Reversibility)
    agent-runtime/      wires the loop to real IO: tool table, dispatch, runner pump, skill registry
    agent-cli/          CLI host (lib + main — split into a lib so integration tests go through the library surface)
    agent-server/       library crate: axum + session actor + HTTP surface
    agent-server-bin/   the default host binary: a twenty-line main.rs plus arguments and the startup protocol (--ready-file)
  apps/
    desktop/            Tauri shell embedding agent-server (src-tauri carries its own Cargo workspace)
  packages/
    protocol/           TS types generated from Rust
    web/                browser app: transport, state binding, components, MCP client, all under src/
  examples/
    java-gateway/       Spring Boot reference implementation — copy it and edit, not a released artifact
  probes/api/           independent workspace, not in the main dependency graph
```

`agent-runtime` is the seam between IO and pure logic: `agent-core` emits an `Effect`,
runtime actually performs it, and translates the result back into an `Event`. **The tool
table and dispatch live here, not in the core** — the core deliberately has no tool table
(`Reversibility` is metadata on a descriptor, so a core that invented one would be making
it up).

The frontend was once planned as two packages, `client` (transport + state binding) and
`ui` (components). **That never happened**: both live under `packages/web/src/`. Split it
when a second frontend host actually appears — splitting now means maintaining three
packages for one consumer.

### Package boundaries

**`agent-store`** — knows only atoms, the dependency graph, and the command log. It does
not know about agents, messages, or tools. The concrete value type is defined by
`agent-core`; the store only requires `Clone + PartialEq + Serialize`.

**`agent-core`** — does no IO. The reason is *not* "so it can compile to wasm" (that's an
incidental benefit of decision 26, not the justification — the constraint should hold even
if we stopped targeting wasm). The reason is that **the entire agent loop must be unit-
testable with no network**: mock a provider, mock a tool executor, and test state
transitions, undo, and recovery. Once IO seeps in, those become integration tests, and then
nobody writes them.

**`agent-server`** — is a **library**, not a binary. The desktop app embeds it, enterprise
internal services embed it, and `agent-server-bin` is just one host among several. Ship
only a binary and enterprises are forced to wrap it in a proxy.

```rust
AgentServer::new(config).serve(addr).await
```

## Transport

SSE downstream, plain POST upstream. The application layer is full-duplex without needing
WebSockets.

```
POST /sessions                        create/attach a session (chatid, idempotent three-way, INTEGRATION.md §3)
GET  /sessions/{id}                   session state
GET  /sessions/{id}/agents            live agent-tree snapshot (a derived read, not new state)
GET  /sessions/{id}/events            SSE downstream
GET  /sessions/{id}/events/poll       pull-based downstream (a second projection of the same ring, decision 25)
POST /sessions/{id}/input             user message
POST /sessions/{id}/tool_result       { agent, tool_call_id, result }
POST /sessions/{id}/undo              { granularity: "turn" | "step", force: bool }
POST /sessions/{id}/redo
POST /sessions/{id}/cancel
```

The `tool_result` body **carries no epoch, and the client cannot supply one**: the server
records the epoch when it dispatches, and validation happens inside the actor's `RunnerCtx`
by exactly matching a `(agent, call_id)` that is still waiting. Red line 6 still holds; it
just isn't expressed on the wire — **if a client could forge the generation number, red
line 6 would be self-certifying.**

`granularity: "step"` does not accept `force: true` (only turn granularity has a
barrier-crossing mode). That combination is rejected with a 400 at the HTTP layer rather
than being silently ignored deeper in.

**Event names are not maintained in this document.** The authority is
`packages/protocol/src/generated/SessionEvent.ts`, generated from Rust by ts-rs. See
§"protocol types": two hand-written copies of a live protocol is the most common source of
rot, and a copy in the docs would be the third.

Every wire frame is an **envelope**:

```
id: 42
data: {"agent":"<AgentId>","event":{"type":"text_delta","data":"…"}}
```

`agent` says which agent the frame belongs to (the whole tree shares one stream); `event`
is the `SessionEvent` (adjacently tagged, `tag = "type"`, `content = "data"`). **All frames
use SSE's default `message` event type** — there's no `event:` field routing, so clients
use `onmessage` and dispatch on `event.type`.

Why SSE and not WebSockets: easier to get through enterprise proxies, better to observe,
and reconnection can replay via `Last-Event-ID`. The bounded ring buffer backing that
replay **lives in the per-session hub at the HTTP layer** (`http/hub/ring.rs`, 256 frames
by default), not in the session actor. **Location determines semantics**: the ring is
in-process, so after a restart `Last-Event-ID` cannot replay old frames — an honest boundary
of this shape, not a defect. The same ring is also the source of truth for `events/poll`
(SSE / polling / long-polling are three projections of one ring, see
[INTEGRATION.md](INTEGRATION.md) §4).

**The server must emit these two headers on SSE responses**:

```
X-Accel-Buffering: no
Cache-Control: no-cache
```

In enterprise environments there may be an nginx, an Ingress controller, or an internal LB
between server and browser, and any layer's default buffering turns a stream into one
big delivery at the end. Get it right once at the server and every intermediary behaves,
with zero code on the gateway side.

### Cancellation propagation

A client disconnecting from SSE **must** cancel in-flight LLM requests and tool calls. This
isn't an ops nicety, it's correctness: dropping it burns tokens for nothing and leaves
ghost sessions writing into channels nobody is reading.

The mechanism is subscriber refcounting plus a grace countdown: the countdown starts only
at zero, and cancellation happens only if a second check at expiry still reads zero.
**Pull-based transport shares the same mechanism** — each poll holds a subscription for its
whole duration (so a long-poll in progress keeps the count non-zero and won't be
misjudged as a disconnect; a client that walks away simply never polls again, and the
countdown runs the same cancellation path). One SSE viewer plus one pull gateway on the
same session means either can leave without killing the other.

## Deployment shapes

Two shapes, one binary/library:

1. **Standalone**, starting at `replicas: 1`. The enterprise gateway sits in front; the
   server has only a ClusterIP and no Ingress.
2. **As a child process of a host** (recommended since M9): a Java gateway or Tauri starts
   it with `--port 0` and `--ready-file`, making the two **a single deployment unit** where
   start/stop/restart/health belong to the host.

`bind` defaults to `127.0.0.1`; listening on `0.0.0.0` requires explicitly setting
`AGENT_BIND`. There is currently no authentication at all, so the default is safe and
exposure is a deliberate act — running it by accident on a bare machine doesn't put it on
the network. The child-process shape is local-only for free.

**`--ready-file` is the stable startup protocol between host and Rust — do not parse the
startup banner.** The host supplies a path that **does not yet exist**, and after a
successful bind Rust atomically publishes `{"port","pid","version"}` there (using
`hard_link`, not `rename` — `rename` silently overwrites, while `hard_link` requires the
target to be absent, which makes it impossible for a stale file from a previous launch to
be read as this launch's success). **Failure to publish means a non-zero exit**, so there's
no "process is running but the parent waits forever" state.

For integration work the server can host the frontend same-origin: point
`AGENT_STATIC_DIR` at `packages/web`'s `dist/`, which saves a dev server and CORS setup.

### Sticky routing across replicas (**design sketch, not implemented**)

> **Status**: there is not one line of code behind this section. `PodAddr` /
> `LocalRegistry` / `RedisRegistry` / cross-pod forwarding are undefined anywhere in the
> repo. The existing `crates/agent-server/src/registry/` is a **single-replica in-memory
> table** (`SessionId → SessionHandle`, with `open`/`get`/`close` and a dead marker on
> crash), and its own module docs say that `trait SessionRegistry` is "the seam that will
> grow when `RedisRegistry` is actually built." **Multi-replica does not work today** —
> don't do capacity planning from this section. It's kept because it's a valid design
> intent: if multi-replica is ever scheduled, resume thinking from here instead of
> re-deriving it.

Multiple replicas break here: `GET /events` lands on Pod-1 (where the session actor is)
while `POST /tool_result` lands on Pod-3 (which has never heard of that session).

The envisioned answer is **self-routing on the server side**: any pod asks the registry who
owns the session and, if it isn't itself, forwards one hop inside the cluster, reverse-
proxying the SSE stream too. The gateway stays completely unaware; it just hits the Service.

```rust
// sketch; does not exist in the code
trait SessionRegistry {
    fn owner(&self, id: SessionId) -> Option<PodAddr>;
}
```

`LocalRegistry` would always resolve to itself (single replica, so the forwarding branch is
dead code) and multi-replica would swap in `RedisRegistry` with zero changes to the gateway
or frontend — which is the part of this shape worth keeping: **going multi-replica should
not make the gateway and frontend change protocol.**

The original plan said "write the forwarding logic now, keep the registry abstraction now."
**That wasn't done, and in hindsight that was right**: under a single replica the
forwarding branch is pure dead code that can't be verified, and M9's pull-based transport
made the whole thing easier (no long connection to stick). The cost is that "swap the
registry implementation and you have multi-replica" is not true — cross-pod forwarding,
including SSE reverse-proxying, is entirely unbuilt.

## Edge-agnostic

| Concern | Server side | Enterprise side |
|---|---|---|
| Auth | none; reads identity headers without verifying | gateway verifies and writes headers |
| Logs | `tracing` → stderr | their collector |
| Tracing | honors W3C `traceparent` only | SkyWalking / Sleuth / OTel all pass through |
| Config | environment variables | ConfigMap / Secret |

**No authentication ≠ no identity.** The server still needs to know whose session this is,
for isolation and audit attribution. The **intent** is to trust `X-Agent-Tenant-Id` /
`X-Agent-User-Id` from upstream, falling back to `anonymous`.

> **Status (not implemented, not scheduled)**: the server **never reads those two headers**,
> and `EntryMeta` has **no `owner` field** (it is currently
> `{ turn_id, epoch, label, barrier }`). The original plan's line — "keep an `Entry.owner`
> field now so multi-tenancy later needs no schema migration" — **is a promise in reverse**:
> the field doesn't exist, and `EntryMeta` is a `Serialize`d on-disk structure, so adding
> it later **is a snapshot/log schema change** and will require migration. Planning
> multi-tenancy from the original wording would step into a hole.

The only landed form of identity today is **chatid**: `POST /sessions` accepts a
client-specified id and **ownership is guaranteed by the gateway** (guess someone's chatid
and you attach to their session). That is a **deployment contract**, not something code can
enforce; full constraints in [INTEGRATION.md](INTEGRATION.md) §3.

Two things to decide together whenever multi-tenancy is actually scheduled: which atom the
headers land in (or whether they only enter log metadata), and the **on-disk compatibility
strategy** for adding `owner` to `EntryMeta` (old logs lack the field, so recovery must read
the old format).

## Java gateway reference implementation

`examples/java-gateway/`. **Not published to Maven, no Spring Boot 2/3 dual compatibility,
no version tracking.** The first line of its README is "copy it and edit; don't depend on
it." It has been build-verified with OpenJDK 21 and Maven 3.9.15 (`mvn -q package`), and at
M9 sign-off it ran a full live chain (real DeepSeek upstream plus curl, with the gateway
starting the Rust child process itself, 67 frames arriving one by one, and Rust exiting
cleanly when Java stopped).

### Shape: pull from upstream, produce SSE yourself (decision 25)

```
browser ──SSE──> Java gateway ──long-poll (HTTP)──> Rust agent-server
                      └────── child process; lifecycle owned by Java ──────┘
```

**The core judgment: the complexity of SSE should appear only in the hop that *produces*
SSE, never in the hop that *proxies* it.** Producing SSE is a tutorial-level Spring
operation; proxying it is where the pits are (below). So the Java↔Rust hop is pull-based,
and the browser hop's SSE protocol is **unchanged down to the byte** (`EventSource`
auto-reconnect, `Last-Event-ID`, identical frame envelopes). Full derivation in
[INTEGRATION.md](INTEGRATION.md).

What's in it now:

- **`AgentServerProcess`** — `@PostConstruct` starts the Rust child via `ProcessBuilder`
  (`--port 0 --ready-file <exclusive temp path>`), polls the ready file for the actual port
  and verifies the pid, and **never parses the startup banner**. `@PreDestroy` does
  `destroy()` (SIGTERM) → `waitFor` → `destroyForcibly()` only on timeout. On SIGTERM the
  Rust side **persists every session before exiting**.
- **`AgentSseController`** — a `@GetMapping` that produces SSE itself, looping internally
  against upstream `GET /sessions/{id}/events/poll` with `Last-Event-ID: <cursor>` and
  `X-Poll-Wait-Ms: 25000`. **The server computes `next`; the gateway must not add one**
  (the ring returns only `id > Last-Event-ID`, so incrementing yourself skips a frame).
- **`AgentProxyController`** — a catch-all `@RequestMapping("/agent/**")` forwarding the
  remaining short requests, with a `// your filter goes here` comment.
- **`ChatSubscribers`** — how many browser connections this gateway still has per chatid,
  **used only to decide whether to proactively `POST /cancel`.** It is not a copy of Rust's
  subscriber count: that one also includes directly-connected clients and other gateway
  instances, and it remains the authority for cancellation (refcount → grace → `cancel()`).

**Long-polling rather than short-polling**: while pulling, the gateway holds Rust's
`SubscriberGuard`. Under short-polling the guard is released as soon as the response is
sent, so the polling interval would have to stay under the grace period (5s) or the client
gets judged disconnected. Long-polling holds it throughout, which removes that constraint
and minimizes idle requests.

**Headers are forwarded wholesale** (except hop-by-hop: `Connection` / `Keep-Alive` /
`Transfer-Encoding` / `Upgrade` / `TE` / `Trailer` / `Proxy-*`) — no per-header allowlist.
Wholesale forwarding means whatever headers an enterprise's auth filter writes reach the
server automatically, and `traceparent` passes through automatically. The code written to
achieve that is zero lines. **Pass-through isn't a feature; it's what happens when you
don't filter.**

**Deployment contract (the part code can't solve)**: chatid *is* session identity, so
guessing someone's chatid attaches you to their session. The gateway must guarantee
ownership — either chatids contain a uuid, or the gateway enforces `user → chatid`
authorization.

### The four pits of proxying SSE (**structurally absent on this chain**, but recognize them before copying)

These four belong to *forwarding* someone else's stream, not to *producing* your own. The
pull-based design deletes the stream on the Java↔Rust hop, so all four vanish: no stream to
buffer, no chunk boundaries to worry about, no long connection whose timeout must be
loosened, no cancellation signal to propagate upstream. **They're documented here because
an enterprise may well still proxy SSE somewhere else** (another layer in front of the
gateway, or a direct `GET /events`):

1. **No buffering** — forward via `bodyToFlux(DataBuffer)`, never `bodyToMono(String)`, or
   nothing goes out until the upstream stream ends.
2. **No compression** — disable gzip on that path or send `Accept-Encoding: identity`;
   compression accumulates events to chunk boundaries.
3. **Loosen timeouts** — WebClient's default response timeout will cut a long connection.
   (Under pull-based this returns in another form: giving the polling client a response
   timeout shorter than the long-poll ceiling turns **normal waiting** into a failure. The
   reference implementation therefore sets no `responseTimeout`.)
4. **Propagate disconnects** — when the frontend closes SSE, the upstream subscription must
   be cancelled too. WebFlux propagates Flux cancellation automatically, but don't insert
   `.cache()` / `.share()` and block the signal.

**"You must use WebFlux" is no longer forced by the Rust side.** The old reason was
proxying SSE: Spring MVC's `SseEmitter` occupies one Tomcat thread per connection, so a
default `max-threads: 200` means two hundred concurrent sessions exhaust the application's
thread pool. Under pull-based transport, **MVC can implement this protocol too** — the only
remaining constraint is not to occupy a request thread long-term per browser connection
(configure a separate pool via `spring.mvc.async`, or run just this path on WebFlux). The
reference implementation is **still** WebFlux; that is now an implementation choice, not a
requirement.

## Desktop

Tauri embeds the same `agent-server` (bound to a random loopback port, no gateway, no
auth). The frontend code is unchanged; only the base URL differs. This pattern — embed,
random loopback port, swap the base URL — was later reused verbatim by M9's Java gateway;
desktop validated it first.

Desktop-only capabilities (fs, shell) are **designed** to be registered as tools with
`location: Desktop`, travelling the exact same remote channel as `web:` tools
(`Location::is_remote()` is true for both, prefix `desk:`, recognized by `ToolTable`).

> **Status: not one `desk:` tool is registered.** The routing half works (shared with
> `web:`), but no assembly path registers desktop tools and there is no end-to-end test
> (end-to-end exists only on the web side). `bootstrap.rs` says plainly: "add it to
> `BootstrapOptions` when something actually needs to call it; don't build it early." Not a
> defect — just not needed yet. But don't mistake "designed" for "working."

**wasm is a third host shape** (decision 26, superseding the earlier "no wasm target"):
the core compiles into the browser and runs there with no `agent-server` process at all.
All three shapes coexist, and decision 12's "`agent-server` is a library" is unchanged.

What's trimmed in the browser shape: `agent-mcp` isn't compiled (stdio doesn't exist there;
MCP servers a browser can reach are connected by the frontend itself), `agent-tools`'
`srv:` shell/fs specs aren't declared (they're pure data — simply don't declare them), and
`agent-transport` swaps in a fetch implementation — `fetch`'s streaming body plus
`AbortController` neatly replaces the machinery `read_loop.rs` had to grow because ureq
offers no interrupt handle.

**The one structural change** is `RunnerCtx.fs: ToolExecutor`: it's a concrete struct whose
`new()` canonicalizes a real directory, which doesn't exist in a browser. It needs an
injection seam — and note that where this document says "mock a tool executor" above, that
seam **did not actually exist yet** under the then-current structure; it was opened as part
of this work.

## Provider adaptation

Model-side differences (cache semantics, `tool_choice` support, stream framing, error-code
assignment) are absorbed by the adapters in `agent-providers`, and **the architecture
should not know about them**.

**The full definition of the seam is in [ADAPTER.md](ADAPTER.md)** — how ingredients are
divided, how an `Adjustment` is reported, what the trait looks like, and what it looks like
when something is in the wrong place. Only the test is repeated here:

> To decide where a piece of code goes: **is it a model-related decision?** Yes → adapter.
> No → core. **Not one of them is allowed in the core** (red line 12): no `match provider`,
> and no `if caps.xxx()`.

Two corollaries:

1. **Request assembly belongs to the adapter** (decision 15). Every assembly decision is
   model-related: where late-added tools go, where skill content is injected, whether
   thinking enters the prefix, whether temperature can be changed at all.
2. **Replace "ask about capabilities beforehand" with "report adjustments afterward"**
   (decision 17). The core states intent ("this turn must call `fs/read`"); if the adapter
   can't, it downgrades and attaches an `Adjustment` to the response. The core runs one
   path end to end.

**Cache invalidation is silent** — no error, just more expensive, and on some providers two
orders of magnitude more expensive. [Issue 024](issues/024-cache-guard.md) catches it in
three layers, split along the same line as red line 12: **judgment** belongs to the adapter
(how much should have hit, which segment drifted — that requires knowing the matching
semantics and block size), **comparison** belongs to the core (predicted vs actual, rolling
window — pure arithmetic).

Measured data and the complete record of per-vendor differences are in
[probes/PROVIDERS.md](../probes/PROVIDERS.md). That file is internal evidence for the
adapters; **the main design should not cite a word of it.**

## Protocol types

The TS types in `packages/protocol` are generated from the Rust side with **ts-rs** and
**not hand-maintained**. Two hand-written copies of a live protocol is the most common
source of rot in enterprise projects. Any protocol change must, as part of finishing the
work locally, run the protocol-consistency test with the `ts` feature enabled — it fails
when the generated output disagrees with the source, so forgetting to regenerate the TS
turns red right there.

This is also the only reason for a single monorepo with two workspaces: **a protocol change
can be completed atomically in one commit.**
