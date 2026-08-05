# einfach-agent

> English is the primary project language. [简体中文](README.zh-CN.md)

An embeddable agent runtime that lets each host application supply its own tools and skills—without
turning the model context or the Rust core into an integration dump.

The standout feature is a complete host-capability loop:

```text
Browser / Desktop / Java capabilities
                 │
          compact skill index
                 │
          AI activates one bundle
                 │
      instructions + tools appear
                 │
       one host claims execution
                 │
       Rust commits one final result
```

## Why It Is Different

### Host applications can teach the agent new capabilities

A host supplies session-scoped tools and skills when it creates a conversation. The same Rust agent
can therefore operate inside a finance application, an admin console, a design product, or a desktop
shell without hard-coding each product integration into the core.

- `web:` tools execute in a browser or web host.
- `desk:` tools execute in a desktop host.
- Skills package domain instructions together with related tools.
- Built-in server tools can be disabled per session.

Declarations are validated, deterministically ordered, journaled with the session, and restored with
the conversation. A later deployment change cannot silently rewrite the capability surface of an
existing conversation.

### Large capability catalogs stay lazy

Large tool surfaces can be grouped into skills. Initially, the model sees only a compact index of
skill names and descriptions. It explicitly activates the relevant skill before the full instructions
and its tool schemas enter the request.

This keeps unrelated domain knowledge out of the prompt, avoids context growth proportional to the
entire catalog, and preserves stable prefixes for provider prompt caching. Small, always-available
host tools may still be declared directly.

### Remote tools have a real execution protocol

When several tabs or host processes observe the same tool call, they do not race to perform the side
effect. Rust atomically grants one claim inside the session actor. Only the winner may execute.

Result delivery is strongly acknowledged:

- `committed` means the actor verified and applied the terminal result—not merely queued it;
- retrying an identical submission returns `duplicate` without advancing the model twice;
- changing an already-used submission returns `result_conflict`;
- cancellation, expiry, stale calls, and claim mismatches have distinct structured outcomes.

The protocol also refuses to lie about distributed uncertainty. If nobody claimed a call, it ends as
`unclaimed_timeout`. If a host claimed it and then disappeared, it ends as `outcome_unknown`, because
the external side effect may already have happened and must not be retried silently.

### State is the source of truth

Every piece of agent state lives in one atomic dependency graph. Undo, redo, crash recovery, and
audit replay are four projections of the same mechanism rather than four loosely synchronized
features.

A turn removed by `/undo` is genuinely absent from the model's reconstructed memory. Reversibility
barriers prevent accidental rollback across irreversible work, while explicit override remains
available when the operator intends it.

### Provider differences stay outside the core

The core contains no provider-specific branching. Adapters translate intent for DeepSeek, Kimi, and
GLM and report every unavoidable adjustment as observable data. Prefix-byte checks, response
accounting, and rolling cache telemetry make prompt-cache regressions visible during the turn rather
than on the next invoice.

## Real-World Verification

The host execution path has been exercised end to end, not only through mocks:

- 100 real-TCP concurrent claim races produced exactly one winner per round;
- two real browsers connected through the Java gateway with the same `chatid`;
- only the winning browser performed the side effect;
- retry after a simulated lost HTTP response returned `duplicate`;
- disconnect after claim produced `outcome_unknown`, while the observing browser and session
  continued normally.

The detailed protocol and evidence are recorded in
[Host-Native Tools and Skills](docs/issues/092-remote-tool-result-protocol.md).

## Runtime Surfaces

| Surface | Entry point | Purpose |
|---|---|---|
| CLI | `cargo run -p agent-cli` | Conversation, tools, agents, undo/redo, and crash recovery |
| Standalone server | `cargo run -p agent-server-bin -- --sessions-dir ./sessions` | HTTP/SSE runtime behind an application gateway |
| Web | `pnpm --filter web dev` | Streaming UI and browser-hosted tool execution |
| Desktop | `pnpm --filter desktop tauri build` | Tauri host using the same server library and Web UI |
| Java gateway | `examples/java-gateway/` | Spring WebFlux embedding and proxy reference |

## Run Locally

```bash
cp providers.example.toml providers.toml
# Add a DeepSeek, Kimi, or GLM API key to providers.toml.

cargo run -p agent-cli
```

To run the standalone HTTP/SSE server:

```bash
cargo run -p agent-server-bin -- --sessions-dir ./sessions
```

This repository intentionally has no hosted build pipeline. Tests and invariant checks are run
locally when changing the relevant component.

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [State model](docs/STATE-MODEL.md)
- [Provider adapter contract](docs/ADAPTER.md)
- [Hard invariants](docs/INVARIANTS.md)
- [Roadmap and decisions](docs/ROADMAP.md)
- [Implementation issues](docs/issues/README.md)

The state engine originated as a fork of the Rust atomic engine in
[einfach](https://github.com/allroad88888888/einfach) and now evolves independently.
