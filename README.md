# einfach-agent

**An embeddable agent runtime with a real ledger.** Undo, redo, crash recovery, and audit
replay are one mechanism, not four features. Runs on a server, in a desktop app, or
entirely in a browser tab.

### [▶ Try it in your browser](https://allroad88888888.github.io/einfach-agent-rust/) — no install, no server, bring your own key

There is no backend. That page **is** the agent runtime, compiled to wasm and running in
your tab. Your key goes straight from your browser to the provider — open DevTools →
Network and check.

![Undo removing a turn from the model's memory](docs/assets/undo-demo.gif)

[![CI](https://github.com/allroad88888888/einfach-agent-rust/actions/workflows/ci.yml/badge.svg)](https://github.com/allroad88888888/einfach-agent-rust/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/einfach-store.svg?label=einfach-store)](https://crates.io/crates/einfach-store)
[![docs.rs](https://img.shields.io/docsrs/einfach-store?label=docs.rs)](https://docs.rs/einfach-store)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

> English is the primary project language. [简体中文](README.zh-CN.md)

---

## This is not another agent framework

The Rust ecosystem has good libraries for *building LLM applications* — chains, RAG,
embeddings, tool loops. If that's what you need, use one of those.

This is a different thing: **a runtime you embed in a product**, whose defining property is
that all agent state lives in one atomic dependency graph with a command log. The nearest
comparisons aren't agent libraries — they're LangGraph's time travel and Temporal's durable
execution.

Everything below follows from that one decision.

### Undo actually removes the turn

Every chat UI has an undo button that deletes a bubble. The model still remembers, because
what got deleted was a view.

Here the conversation is reconstructed from the ledger, so a turn removed by `/undo` is
**genuinely absent from the memory the next prompt is built from.** You can check this in
about thirty seconds in the demo above: tell it a passphrase, ask for it back, undo twice,
then ask again without using the word "undo." It says it doesn't know.

It stays gone after a refresh, because the undo went to storage too.

**Reversibility barriers** stop an undo at an irreversible operation rather than silently
rolling past it — you get told which tool blocked it, and can override explicitly.

### `kill -9` and continue

Recovery is loading the last snapshot and pushing the log forward — which is literally the
redo loop, the same function. There is no second code path for "loading a session," so
there is no second code path to drift.

### The same core runs in a browser with no server at all

Not a demo shim: the event pump, turn state machine, provider adapters, and the state graph
compile to wasm and run in a tab. The page hosting it serves three kinds of bytes and takes
part in zero model requests.

Five shapes, one library: CLI, standalone server, embedded server (desktop / Java gateway),
and the browser host. [Architecture →](docs/ARCHITECTURE.en.md)

### Host applications teach the agent their own tools

**[Second demo →](https://allroad88888888.github.io/einfach-agent-rust/roles.html)** Same
wasm binary, two roles: a viewer gets one read-only tool, an operator additionally gets a
refund tool declared `irreversible`. Ask the viewer to refund something and it answers
"I only have a read-only order search tool." One deployment, one agent, capability surface
per caller — there is no fixed tool list in a Rust core that expresses that.

![Two roles, two tool tables, and an undo that stops at the irreversible call](docs/assets/roles-demo.gif)

The last frame is the barrier from the previous section, seen from the other side: the
refund already left the page's memory, so undo names the tool that blocked it instead of
rolling back a payment the ledger cannot reach.

A host declares session-scoped tools and skills when it creates a conversation, so the same
runtime works inside a finance app, an admin console, or a desktop shell without any of
those integrations reaching the core. `web:` tools execute in the browser, `desk:` in the
desktop host.

Declarations are validated, deterministically ordered, journaled with the session, and
restored with it — **a later deployment change cannot silently rewrite what an existing
conversation could do.**

Large catalogs stay lazy: the model sees a compact index at session start and reads a
skill's body on demand through an ordinary tool call, so the body arrives as a tool result
in the conversation. Nothing is ever injected into the system prompt mid-conversation —
bodies land at the tail of the message history, which is the path prompt caching is built
for. Measured over ten turns against DeepSeek including the turns that read a skill body:
**97.5%–99.8% cache hit rate, mean 98.5%.**

### Provider differences never reach the core

No `match provider` in the core, and no capability flags either — capability flags are the
same branching with the vendor names filed off, and they still force you to edit the core
when a fourth vendor arrives.

Instead the core states intent and adapters report what they had to change, as data
attached to the turn. **An empty adjustment list means the request ran as intended.**

Cache regressions are caught during the turn — by prefix-byte comparison and usage
reconciliation — rather than on next month's invoice, where a two-orders-of-magnitude
mistake would otherwise surface.

## Quickstart

```bash
cp providers.example.toml providers.toml
# Add a DeepSeek, Kimi, or GLM API key. Any OpenAI-compatible endpoint also works:
# set `adapter = "openai"` on the section (see that file). The generic adapter is
# run against all three endpoints above — tool calls, streaming, cache accounting,
# auth failures, undo.

cargo run -p agent-cli
```

Then `/undo`, kill the process, restart, and continue.

The standalone HTTP/SSE server:

```bash
cargo run -p agent-server-bin -- --sessions-dir ./sessions
```

| Surface | Entry point |
|---|---|
| CLI | `cargo run -p agent-cli` |
| Standalone server | `cargo run -p agent-server-bin -- --sessions-dir ./sessions` |
| Web UI | `pnpm --filter web dev` |
| Desktop | `pnpm --filter desktop tauri build` |
| Browser (wasm, no server) | `scripts/build-wasm.sh`, or [the hosted demo](https://allroad88888888.github.io/einfach-agent-rust/) |
| Java gateway | `examples/java-gateway/` |

Every push and pull request runs the same gates used locally: invariant checks,
`clippy -D warnings`, the workspace test suite, the protocol-consistency test that
regenerates the TypeScript types, the frontend typecheck, and a browser-host wasm build.

## Documentation

- [State model](docs/STATE-MODEL.en.md) — why undo, redo, crash recovery and audit replay
  are one mechanism, and which parts are honestly not built yet
- [Architecture](docs/ARCHITECTURE.en.md) — packages, transport, deployment shapes, and
  what is *not* built yet
- [Hard invariants](docs/INVARIANTS.en.md) — twelve rules whose violations produce no error
- [Provider adapter contract](docs/ADAPTER.md) *(Chinese)*
- [Measured provider differences](probes/PROVIDERS.md) *(Chinese)* — DeepSeek / Kimi / GLM,
  measured rather than documented, with the raw observations in `probes/results/`
- [Roadmap and decisions](docs/ROADMAP.md) *(Chinese)* — every decision with its reasoning,
  including the ones that were later overturned
- [Implementation issues](docs/issues/README.md) *(Chinese)* — one file per task, each with
  acceptance criteria and an implementation record

Documents marked *(Chinese)* have no translation yet. Development happens in Chinese; the
translated documents say so at the top and name the Chinese original as authoritative.

The state engine originated as a fork of the Rust atomic engine in
[einfach](https://github.com/allroad88888888/einfach) and now evolves independently.

## Status

Usable and exercised end to end, but young — the first commit landed 2026-08-03. Each
milestone closed with a live-provider run rather than mocks, and those runs are recorded
issue by issue.

**One crate is on crates.io so far**: [`einfach-store`](https://crates.io/crates/einfach-store),
the state engine underneath everything above — the atomic dependency graph plus the command
log, with no agent, provider, or IO code in it (it is forbidden from doing IO at all). The
runtime crates are not published yet; to use those, depend on this repository directly.

Not built, and marked as such wherever it comes up: multi-replica deployment,
multi-tenancy, and MCP's OAuth / resources / prompts. The API is not stable yet.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your
option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this
work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
