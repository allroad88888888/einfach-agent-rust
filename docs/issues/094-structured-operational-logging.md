# 094 — Structured Operational Logging

**Status:** complete · **Lead:** `gpt-5.6-sol`

## Outcome

The Rust HTTP server has one structured operational-log pipeline. A request, server lifecycle
event, or safe failure can be searched by stable fields instead of reconstructed from scattered
`eprintln!` text. Session JSONL and SSE remain product/persistence protocols, not log sinks.

```text
agent-server-bin / desktop host
  -> install one tracing subscriber
  -> agent-server spans and events
  -> stderr (human locally, JSON when configured)
```

## Contract

1. New Rust operational logs use `tracing`; `tracing-subscriber` is initialized only by executable
   hosts. Library crates emit events and spans but never install a global subscriber.
2. Every HTTP request produces one span with a generated request ID, HTTP method, matched route,
   status, and elapsed time. Raw URI/query strings and headers are excluded.
3. Safe server lifecycle events include bind address, provider/model identifiers already selected by
   the host, session count, and sanitized error classification. They exclude prompts, messages,
   thinking, tool arguments/results, image names/paths/bytes, provider references, capabilities,
   authorization material, and raw upstream bodies.
4. `RUST_LOG` controls level/filtering. Host output supports compact human-readable text by default
   and JSON through one documented host environment switch. This issue adds no file rotation,
   OpenTelemetry exporter, collector, or persistence changes.
5. CLI `println!` output that is intentionally user-facing remains UI output; this issue only
   replaces process diagnostics and adds operational request spans.

## Execution Ledger

```text
094 Structured operational logging [active | lead/SOL]
├─ A. Contract and issue ledger
│  └─ A1 [done | lead/SOL | W0 | -] Freeze safe field/output boundary — this issue
├─ B. HTTP observability
│  ├─ B1 [done | HTTP/SOL | W1 | A1] Add tracing dependencies and safe request layer —
│  │    exclusive: crates/agent-server/Cargo.toml, crates/agent-server/src/http/observability.rs,
│  │    crates/agent-server/src/http/mod.rs, crates/agent-server/src/http/routes/mod.rs
│  └─ B2 [done | HTTP/SOL | W1 | B1] Prove request logs use matched routes, not raw request data —
│       exclusive: crates/agent-server/src/http/observability_tests.rs
├─ C. Standalone server host
│  ├─ C1 [done | bin/TERRA | W1 | A1] Install configurable subscriber once at process entry —
│  │    exclusive: crates/agent-server-bin/Cargo.toml,
│  │    crates/agent-server-bin/src/observability.rs,
│  │    crates/agent-server-bin/src/main.rs, Cargo.lock
│  └─ C2 [done | bin/TERRA | W1 | C1] Convert lifecycle diagnostics to structured events —
│       exclusive: crates/agent-server-bin/src/run.rs
├─ D. Desktop host
│  ├─ D1 [done | desktop/TERRA | W1 | A1] Install the same tracing pipeline without double global
│  │    logger initialization — exclusive: apps/desktop/src-tauri/Cargo.toml,
│  │    apps/desktop/src-tauri/src/observability.rs, apps/desktop/src-tauri/src/lib.rs
│  └─ D2 [done | desktop/TERRA | W1 | D1] Convert desktop host diagnostics to tracing —
│       exclusive: apps/desktop/src-tauri/src/server.rs
├─ E. Verification and integration
│  ├─ E1 [done | verifier/SOL | W2 | B,C,D] Focused server/bin/desktop checks passed under
│  │    `--locked`; no staging or commit
│  └─ E2 [done | audit/SOL | W2 | E1] Independent field, global-init, scope, and line-size review
└─ F. Deferred deliberately
   ├─ F1 [ready | future | - | -] Local rolling-file retention via tracing-appender
   └─ F2 [ready | future | - | -] OpenTelemetry export and distributed trace propagation
```

`Cargo.lock` belongs to C1 after B1's manifest edit is visible; the independent desktop lock belongs
to D1. No other leaf may regenerate either lock.

## Acceptance Criteria

1. `agent-server` has request spans that carry only safe structured fields and do not change API
   responses or SSE behavior.
2. The standalone binary initializes a subscriber once before startup and emits structured startup,
   shutdown, and failure events.
3. The desktop host emits `tracing` events without competing global logger setup.
4. JSONL/session persistence and SSE frames remain unchanged as product behavior.
5. Focused tests and relevant cargo checks pass; new or materially changed source/test files stay
   at or below 300 physical lines.
