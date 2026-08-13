# einfach-agent

[![CI](https://github.com/allroad88888888/einfach-agent-rust/actions/workflows/ci.yml/badge.svg)](https://github.com/allroad88888888/einfach-agent-rust/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

> English is the primary project language. [简体中文](README.zh-CN.md)

An embeddable agent runtime that lets each host application supply its own tools and skills—without
turning the model context or the Rust core into an integration dump.

The standout feature is a complete host-capability loop:

```text
Browser / Desktop / Java declares its capabilities
                 │
   session start: compact skill index enters the prefix
                 │
      AI reads one skill body on demand (a normal tool call)
                 │
     the body arrives as a tool result, in the conversation
                 │
        AI calls a host tool; the host executes it
                 │
             result continues the turn
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

Large tool surfaces can be grouped into skills. At session start the model sees only a compact index
of skill names and descriptions. It reads the full body of a skill on demand, through an ordinary
tool call, and the body arrives as a tool result inside the conversation.

Nothing is ever injected into the system prompt mid-conversation. Bodies are appended at the tail of
the message history — the path prompt caching is designed for — so the cached prefix survives every
read. Measured over ten turns against DeepSeek, including the turns where a skill body was read:
97.5%–99.8% cache hit rate, mean 98.5%.

This keeps unrelated domain knowledge out of the prompt and avoids context growth proportional to the
entire catalog. Small, always-available host tools may still be declared directly.

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

Every push and pull request runs the same gates used locally: invariant checks, `clippy -D warnings`,
the workspace test suite, the protocol-consistency test that regenerates the TypeScript types, the
frontend typecheck, and a browser-host wasm build.

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [State model](docs/STATE-MODEL.md)
- [Provider adapter contract](docs/ADAPTER.md)
- [Hard invariants](docs/INVARIANTS.en.md) — the twelve rules whose violations produce no error
- [Roadmap and decisions](docs/ROADMAP.md)
- [Implementation issues](docs/issues/README.md)

The state engine originated as a fork of the Rust atomic engine in
[einfach](https://github.com/allroad88888888/einfach) and now evolves independently.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your
option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this
work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
