package com.example.agentgateway.proxy;

import com.example.agentgateway.config.AgentRuntimeProperties;
import com.example.agentgateway.runtime.AgentServerProcess;
import java.net.URI;
import java.nio.charset.StandardCharsets;
import org.springframework.http.HttpHeaders;
import org.springframework.http.MediaType;
import org.springframework.stereotype.Component;
import org.springframework.web.reactive.function.client.WebClient;
import org.springframework.web.util.UriUtils;
import reactor.core.publisher.Mono;

/** Calls the Rust session creation and pull endpoints used by browser SSE connections. */
@Component
final class AgentSessionClient {

    private final WebClient webClient;
    private final AgentServerProcess agentServer;
    private final AgentRuntimeProperties properties;

    AgentSessionClient(WebClient agentWebClient, AgentServerProcess agentServer, AgentRuntimeProperties properties) {
        this.webClient = agentWebClient;
        this.agentServer = agentServer;
        this.properties = properties;
    }

    Mono<Void> getOrCreate(String chatId, HttpHeaders browserHeaders) {
        return webClient.post()
                .uri(agentServer.resolve("/sessions"))
                .headers(headers -> headers.addAll(HopByHopHeaders.strip(browserHeaders)))
                .contentType(MediaType.APPLICATION_JSON)
                .bodyValue(new CreateSessionRequest(chatId))
                .retrieve()
                .toBodilessEntity()
                .then();
    }

    Mono<PollResponse> poll(String chatId, Long lastEventId, HttpHeaders browserHeaders) {
        return webClient.get()
                .uri(sessionPath(chatId) + "/events/poll")
                .headers(headers -> {
                    headers.addAll(HopByHopHeaders.strip(browserHeaders));
                    headers.set("X-Poll-Wait-Ms", Integer.toString(properties.pollWaitMs()));
                    headers.remove("Last-Event-ID");
                    if (lastEventId != null) {
                        headers.set("Last-Event-ID", Long.toString(lastEventId));
                    }
                })
                .accept(MediaType.APPLICATION_JSON)
                .retrieve()
                .bodyToMono(PollResponse.class);
    }

    private URI sessionPath(String chatId) {
        String encodedId = UriUtils.encodePathSegment(chatId, StandardCharsets.UTF_8);
        return agentServer.resolve("/sessions/" + encodedId);
    }

    private record CreateSessionRequest(String id) {
    }
}
