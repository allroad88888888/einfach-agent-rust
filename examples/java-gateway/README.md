# Java gateway reference

Copy this example into your service and adapt it; do not consume it as a Maven
dependency. It starts one local Rust agent-server process, converts Rust pull
responses into browser SSE, and keeps the Rust process lifetime tied to the
Java application.

## Honest boundary

This machine has no JDK installed. The Maven command below has **not** been run
for this repository, so this reference is source-reviewed only, not
build-verified. Do not report it as a successful Maven build until it has been
compiled and exercised in the target environment.

## Runtime shape

    Browser EventSource
            |
            | GET /agent/sessions/{chatid}/events
            v
    Java gateway: creates browser SSE
            |
            | POST /sessions { "id": chatid } once per SSE connection
            | GET /sessions/{chatid}/events/poll (long poll, 25 s)
            v
    child agent-server on 127.0.0.1:<random port>

Java creates the child with ProcessBuilder using port 0, a persistent
sessions directory, and an exclusive ready-file path. After the Rust process
has successfully bound its loopback socket, it atomically publishes:

    {"port":43127,"pid":12345,"version":"..."}

Java validates that the reported pid is its child, then uses that random
loopback port as the Rust base URI. It never parses a human-readable startup
log to discover the port. Child stdout and stderr are forwarded to the Java
logger; neither this example nor the Rust process should print API keys.

At application shutdown, PreDestroy calls Process.destroy(). On Unix this is
SIGTERM; agent-server closes all sessions and writes their snapshots before
exiting. Java waits for the configured shutdown timeout, then uses a forcible
kill only as the zombie-process fallback. The ready-file temporary directory
is removed afterwards.

## Provider, model, and API-key configuration

The Java properties choose the Rust executable, providers TOML, session data
directory, and the *name* of the environment variable carrying the secret.
They do not contain a secret:

    agent:
      runtime:
        command: ${AGENT_SERVER_BIN:agent-server}
        working-dir: ${AGENT_WORKING_DIR:.}
        providers-config: ${AGENT_PROVIDERS_CONFIG:providers.toml}
        sessions-dir: ${AGENT_SESSIONS_DIR:agent-sessions}
        key-env: ${AGENT_API_KEY_ENV:DEEPSEEK_API_KEY}
        startup-timeout: 20s
        shutdown-timeout: 30s
        poll-wait-ms: 25000

Put provider selection, model, and the matching key variable name in the TOML
referenced by providers-config. For example:

    [default]
    provider = "deepseek"

    [providers.deepseek]
    base_url = "https://api.deepseek.com"
    model = "deepseek-v4-pro"
    api_key_env = "DEEPSEEK_API_KEY"

This example follows the repository default, DeepSeek. Switch to another
supported provider (currently Kimi or GLM) by changing its TOML section,
`[default].provider`, and both key-variable names; Java code does not change.

Before starting Java, set the secret in its environment and make
AGENT_API_KEY_ENV exactly match api_key_env:

    export DEEPSEEK_API_KEY='set-this-in-your-secret-manager-or-shell'
    export AGENT_API_KEY_ENV=DEEPSEEK_API_KEY
    export AGENT_PROVIDERS_CONFIG=/absolute/path/providers.toml
    export AGENT_SESSIONS_DIR=/var/lib/my-service/agent-sessions
    export AGENT_SERVER_BIN=/absolute/path/agent-server
    mvn -q package
    java -jar target/agent-gateway-0.0.0-reference.jar

The Maven line is the target-environment command; it was not run here. Java
checks only that the selected environment variable is nonempty, passes it to
the child environment, and never adds its value to an argument, ready file,
property, or Java-authored log line. Keep providers.toml out of source control
when it contains deployment-specific configuration; use api_key_env rather
than an inline key.

The child is intentionally loopback-only. This reference does not turn
agent-server into a network service and must not publish its random port.
Package the Rust binary alongside the Java service for each target platform
(or extract a bundled binary to an executable private directory before
startup). The binary is part of the deployment artifact, not a Java dependency.

## Browser API and pull cursor

Open the browser stream with:

    const source = new EventSource(
      "/agent/sessions/" + encodeURIComponent(chatid) + "/events"
    );

For every browser SSE connection the gateway first sends the idempotent Rust
request POST /sessions with { "id": chatid }. A live session is reused, a
persisted session is recovered, and an absent one is created. It then calls:

    GET /sessions/{chatid}/events/poll
    Last-Event-ID: <optional cursor>
    X-Poll-Wait-Ms: 25000

Each JSON frame becomes a browser SSE message with the same frame id and the
full event JSON as its data. The browser's Last-Event-ID is used for the first
Rust poll. Thereafter, the gateway sends response.next as the next
Last-Event-ID **without adding one**: next is already the last delivered frame
id, and an empty response preserves the submitted cursor.

Each browser connection owns an independent polling loop. Do not share one
loop between tabs: cursor, reconnect, and cancellation behavior would then
become ambiguous. A 20--25 second Rust long poll keeps SubscriberGuard held
for the request lifetime and avoids short polling gaps relative to the Rust
cancellation grace period.

All other short Rust endpoints remain available below /agent, including
input, tool_result, undo, redo, session status, and cancel. The browser should
explicitly call POST /agent/sessions/{chatid}/cancel for an intentional normal
end; cancellation grace is a safety net for an accidental disconnect, not the
primary close path. Do not make the SSE controller cancel automatically on
every connection close: EventSource reconnects and multiple tabs can observe
the same chat.

## Required ownership check

chatid is an authority boundary, not merely a transport parameter. This
minimal reference deliberately has no authentication, so it cannot establish
that the caller owns a supplied chatid. Add an authentication filter before
these controllers and enforce one of the following deployment contracts:

- Generate chatid values with an unguessable UUID component.
- Store and verify a user-to-chatid ownership mapping before proxying or
  creating a session.

Without one of these checks, one user can request another user's chatid and
receive its events. Passing identity headers onward is not an authorization
check by itself.

## Why this remains WebFlux, but is no longer required by Rust SSE proxying

The example keeps WebFlux and ServerSentEvent because it is a compact,
nonblocking way to serve many browser streams. Rust is no longer exposing a
long-lived SSE response to Java, though: Java makes bounded long-poll requests
and constructs the downstream SSE itself. Consequently the former
upstream-SSE-proxy requirements (byte-buffer streaming, compression disabling,
and upstream cancellation propagation) are gone. A Spring MVC service may
implement the same pull loop with an asynchronous streaming mechanism of its
choice; it must still avoid blocking a request thread for every connected
browser.

## Source map

    AgentServerProcess        child start, ready-file handshake, graceful stop
    AgentRuntimeProperties    non-secret runtime settings and validation
    AgentSessionClient        idempotent session creation and Rust polling
    AgentSseController        Rust poll response -> browser SSE
    AgentProxyController      other short /agent requests

The sample intentionally omits authentication, distributed session routing,
metrics, secret-manager integration, and binary packaging mechanics. Add those
in the copied application at the boundary appropriate to your deployment.
