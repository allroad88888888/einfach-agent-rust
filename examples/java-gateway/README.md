# Java gateway reference

Copy this example into your service and adapt it; do not consume it as a Maven
dependency. It starts one local Rust agent-server process, converts Rust pull
responses into browser SSE, and keeps the Rust process lifetime tied to the
Java application.

## Deployment contract you must satisfy first: chatid ownership

**Read this before anything else.** chatid is an authority boundary, not merely
a transport parameter: whoever can name a chatid can attach to that
conversation. This minimal reference deliberately has no authentication, so it
cannot establish that the caller owns a supplied chatid. Add an authentication
filter in front of these controllers and enforce one of:

- Generate chatid values with an unguessable UUID component.
- Store and verify a user-to-chatid ownership mapping before creating a session
  or proxying anything under that chatid.

Without one of these, one user can request another user's chatid and receive
its events. Passing identity headers onward is not an authorization check by
itself, and **no amount of code in this example can fix it for you** — the Rust
server is unauthenticated by design and trusts the gateway to have decided who
the caller is.

## Build verification

`mvn -q package` has been run for this reference in this repository with
Temurin/Homebrew OpenJDK 21 and Maven 3.9.15, and it succeeds (Spring Boot
3.3.4 supports Java 17-21; the compile target here is 17). This supersedes the
earlier "no JDK on this machine, source-reviewed only" note. It is still only a
compile-and-package result: it does not prove any runtime behavior. Real
end-to-end behavior is exercised separately with a live provider.

If you maintain this file in an environment without a JDK, keep the old rule
from issue 037: state the missing build verification honestly, never report a
Maven build that did not run.

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

Owning the child process has a price, and this example only shows the shape of
paying it:

- **Binary distribution.** The Rust executable is part of your deployment
  artifact, not a Maven dependency. Either install it on the host and point
  `AGENT_SERVER_BIN` at it, or bundle one binary per target platform under
  `src/main/resources`, extract the matching one to a private directory at
  startup, and `chmod +x` it (a file extracted from a jar is not executable).
  A jar built on macOS will not carry a Linux binary unless you put it there.
- **Zombie fallback.** `destroy()` then `waitFor(shutdown-timeout)` then
  `destroyForcibly()` covers an orderly JVM exit. It does not cover `kill -9`
  on the JVM: the child would survive. If your platform can hard-kill the JVM,
  supervise the child externally as well (systemd cgroup, Kubernetes pod
  lifetime, or a PID file checked at startup).
- **Child output.** `redirectErrorStream(true)` plus one daemon reader thread
  puts every child line into the Java logger under `[agent-server]`. Without a
  reader the child eventually blocks on a full pipe buffer, so this thread is
  load-bearing, not decoration.

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
        remote-tool-timeout: ${AGENT_REMOTE_TOOL_TIMEOUT:10m}
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

Java checks only that the selected environment variable is nonempty, passes it
to the child environment, and never adds its value to an argument, ready file,
property, or Java-authored log line. Keep providers.toml out of source control
when it contains deployment-specific configuration; use api_key_env rather
than an inline key.

`remote-tool-timeout` is the maximum time between a browser/desktop host
claiming a remote tool call and committing its result. Keep the 10-minute
default for interactive tools; use `AGENT_REMOTE_TOOL_TIMEOUT` (for example
`750ms` or `2m`) when the host needs a different operational deadline.

If `java` reports "Unable to locate a Java Runtime" on macOS you are hitting
the `/usr/bin/java` stub. Point JAVA_HOME at a real JDK, for example
`export JAVA_HOME=/opt/homebrew/opt/openjdk@21`.

The child is intentionally loopback-only. This reference does not turn
agent-server into a network service and must not publish its random port.
Package the Rust binary alongside the Java service for each target platform
(or extract a bundled binary to an executable private directory before
startup). The binary is part of the deployment artifact, not a Java dependency.

### Smoke-checking the gateway with curl

    curl --noproxy '*' -i http://127.0.0.1:8080/agent/sessions/chat-local-1

`--noproxy '*'` is not optional if your shell exports `http_proxy` /
`https_proxy` (common on corporate machines): curl would otherwise send a
loopback request through the proxy and report a 502 that has nothing to do with
this gateway. The same applies to any curl against the Rust child's port.

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

## Closing a stream: explicit cancel, with grace as the safety net

When a browser stream ends, the gateway does not just walk away. It keeps a
per-chatid count of its own live browser connections (ChatSubscribers) and,
when the last one for a chatid goes away, sends

    POST /sessions/{chatid}/cancel

so an in-flight model turn stops immediately instead of burning tokens until
the Rust cancellation grace expires. The count is what keeps multiple tabs on
one chat safe: closing one tab while another is still attached sends nothing.

Rust's own reference count plus grace period remains the authority and the
fallback. It covers the case this gateway cannot signal — the gateway process
itself dying — and it is also what protects a chat that a *different* SSE
client or gateway instance is still watching, since the explicit cancel above
only reflects this process's view.

One accepted cost: if the last browser connection drops mid-turn and the
browser's EventSource reconnects a moment later, the turn is already cancelled
rather than still running. That is the deliberate trade of making the explicit
exit the primary path. The browser may also call
POST /agent/sessions/{chatid}/cancel itself for a user-initiated stop; that
request is proxied unchanged.

All other short Rust endpoints remain available below /agent: input,
tool_result, undo, redo, session status, and cancel.

## Why this remains WebFlux, but is no longer required by Rust SSE proxying

Earlier revisions of this example proxied one long-lived Rust SSE response
straight through to the browser, and that single decision is what forced
WebFlux. Proxying a stream has four requirements, and all four belong to
*relaying* a stream, not to *producing* one:

1. **Do not buffer.** A relay must forward each chunk as it arrives; any
   aggregating body (`bodyToMono(String.class)`, a servlet response wrapper, an
   nginx `proxy_buffering on`) holds the stream until it ends.
2. **Do not compress.** Response compression re-frames the byte stream and
   delays event boundaries.
3. **Do not time out.** Every read/idle/response timeout on the relay path has
   to be lifted, because the upstream response legitimately never ends.
4. **Propagate cancellation.** When the browser disconnects, the relay must
   tear down the upstream request too, or the upstream keeps producing.

Java now issues bounded long-poll requests (each one a normal request/response
that completes within about 25 seconds) and builds the downstream SSE itself,
so all four disappear: nothing to un-buffer, nothing to un-compress, no
infinite response to keep alive, and no upstream stream to cancel. What remains
is producing SSE toward the browser — the textbook Spring case.

The example keeps WebFlux because ServerSentEvent plus a nonblocking WebClient
is the most compact way to write it, **not** because Rust requires it any more.
A Spring MVC service can implement the same pull loop with whatever async
streaming mechanism it already uses; the one rule that still holds is not to
park a request thread per connected browser, which is the reason MVC's
SseEmitter does not scale to hundreds of concurrent chats.

## Source map

    AgentServerProcess        child start, ready-file handshake, graceful stop
    AgentRuntimeProperties    non-secret runtime settings and validation
    AgentSessionClient        idempotent session creation, polling, cancel
    AgentSseController        Rust poll response -> browser SSE
    ChatSubscribers           per-chatid browser connection count for cancel
    AgentProxyController      other short /agent requests
    HopByHopHeaders           headers that must not be relayed to the next hop
    PollResponse              wire shape of the Rust poll response

The sample intentionally omits authentication, distributed session routing,
metrics, secret-manager integration, and binary packaging mechanics. Add those
in the copied application at the boundary appropriate to your deployment.
