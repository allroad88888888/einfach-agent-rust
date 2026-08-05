package com.example.agentgateway.proxy;

import static org.assertj.core.api.Assertions.assertThat;

import com.example.agentgateway.config.AgentRuntimeProperties;
import com.example.agentgateway.runtime.AgentServerProcess;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.BlockingQueue;
import java.util.concurrent.LinkedBlockingQueue;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.springframework.http.MediaType;
import org.springframework.test.web.reactive.server.WebTestClient;
import org.springframework.test.util.ReflectionTestUtils;
import org.springframework.web.reactive.function.client.WebClient;

/** Verifies that the generic gateway preserves every remote-tool v2 wire contract. */
class AgentProxyRemoteToolProtocolTest {

    private final BlockingQueue<CapturedRequest> requests = new LinkedBlockingQueue<>();
    private HttpServer upstream;
    private WebTestClient client;

    @BeforeEach
    void startUpstream() throws IOException {
        upstream = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        upstream.createContext("/", this::respond);
        upstream.start();

        URI baseUri = URI.create("http://127.0.0.1:" + upstream.getAddress().getPort() + "/");
        AgentRuntimeProperties properties = new AgentRuntimeProperties(
                null, null, null, null, null, null, null, 0);
        AgentServerProcess process = new AgentServerProcess(properties, new ObjectMapper());
        ReflectionTestUtils.setField(process, "baseUri", baseUri);
        AgentProxyController controller = new AgentProxyController(WebClient.create(), process);
        client = WebTestClient.bindToController(controller).build();
    }

    @AfterEach
    void stopUpstream() {
        upstream.stop(0);
    }

    @Test
    void forwardsClaimStatusAndV2ResultWithoutProtocolKnowledge() throws InterruptedException {
        exchangeJson("/agent/sessions/chat-1/tool_claim", "claim-accepted", """
                {"agent":"root","tool_call_id":"call-1","claim_id":"browser-a"}
                """);
        CapturedRequest claim = requests.take();
        assertThat(claim.method()).isEqualTo("POST");
        assertThat(claim.pathAndQuery()).isEqualTo("/sessions/chat-1/tool_claim");
        assertThat(claim.traceparent()).isEqualTo("00-trace-parent");
        assertThat(claim.body()).contains("\"claim_id\":\"browser-a\"");

        client.get()
                .uri("/agent/sessions/chat-1/tool_status?agent=root&tool_call_id=call-1")
                .header("X-Tool-Claim-Id", "browser-a")
                .exchange()
                .expectStatus().isOk()
                .expectHeader().valueEquals("X-Remote-Tool-Protocol", "v2")
                .expectBody().json("{\"kind\":\"status-observed\"}");
        CapturedRequest status = requests.take();
        assertThat(status.method()).isEqualTo("GET");
        assertThat(status.pathAndQuery())
                .isEqualTo("/sessions/chat-1/tool_status?agent=root&tool_call_id=call-1");
        assertThat(status.claimId()).isEqualTo("browser-a");

        exchangeJson("/agent/sessions/chat-1/tool_result", "result-committed", """
                {"agent":"root","tool_call_id":"call-1","claim_id":"browser-a",
                 "submission_id":"submission-1",
                 "outcome":{"status":"succeeded","content":"done"}}
                """);
        CapturedRequest result = requests.take();
        assertThat(result.method()).isEqualTo("POST");
        assertThat(result.pathAndQuery()).isEqualTo("/sessions/chat-1/tool_result");
        assertThat(result.body()).contains("\"submission_id\":\"submission-1\"");
    }

    private void exchangeJson(String path, String responseKind, String body) {
        client.post()
                .uri(path)
                .contentType(MediaType.APPLICATION_JSON)
                .header("traceparent", "00-trace-parent")
                .bodyValue(body)
                .exchange()
                .expectStatus().isOk()
                .expectHeader().valueEquals("X-Remote-Tool-Protocol", "v2")
                .expectBody().json("{\"kind\":\"" + responseKind + "\"}");
    }

    private void respond(HttpExchange exchange) throws IOException {
        byte[] requestBody = exchange.getRequestBody().readAllBytes();
        URI uri = exchange.getRequestURI();
        String pathAndQuery = uri.getRawQuery() == null
                ? uri.getRawPath()
                : uri.getRawPath() + "?" + uri.getRawQuery();
        requests.add(new CapturedRequest(
                exchange.getRequestMethod(),
                pathAndQuery,
                exchange.getRequestHeaders().getFirst("traceparent"),
                exchange.getRequestHeaders().getFirst("X-Tool-Claim-Id"),
                new String(requestBody, StandardCharsets.UTF_8)));

        String kind = switch (uri.getRawPath()) {
            case "/sessions/chat-1/tool_claim" -> "claim-accepted";
            case "/sessions/chat-1/tool_status" -> "status-observed";
            case "/sessions/chat-1/tool_result" -> "result-committed";
            default -> "unexpected";
        };
        byte[] response = ("{\"kind\":\"" + kind + "\"}").getBytes(StandardCharsets.UTF_8);
        exchange.getResponseHeaders().add("Content-Type", "application/json");
        exchange.getResponseHeaders().add("X-Remote-Tool-Protocol", "v2");
        exchange.sendResponseHeaders(200, response.length);
        exchange.getResponseBody().write(response);
        exchange.close();
    }

    private record CapturedRequest(
            String method,
            String pathAndQuery,
            String traceparent,
            String claimId,
            String body) {
    }
}
