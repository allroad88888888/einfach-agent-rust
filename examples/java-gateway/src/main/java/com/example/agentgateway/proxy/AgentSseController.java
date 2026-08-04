package com.example.agentgateway.proxy;

import com.fasterxml.jackson.databind.JsonNode;
import org.springframework.http.HttpHeaders;
import org.springframework.http.MediaType;
import org.springframework.http.codec.ServerSentEvent;
import org.springframework.http.server.reactive.ServerHttpRequest;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.RequestHeader;
import org.springframework.web.bind.annotation.RestController;
import reactor.core.publisher.Flux;

/** Produces browser SSE from Rust pull responses for one business chatid. */
@RestController
public class AgentSseController {

    private final AgentSessionClient sessions;

    public AgentSseController(AgentSessionClient sessions) {
        this.sessions = sessions;
    }

    @GetMapping(value = "/agent/sessions/{chatid}/events", produces = MediaType.TEXT_EVENT_STREAM_VALUE)
    public Flux<ServerSentEvent<String>> events(
            @PathVariable String chatid,
            @RequestHeader(value = "Last-Event-ID", required = false) String lastEventId,
            ServerHttpRequest request) {
        Long browserCursor = parseCursor(lastEventId);
        // POST 是幂等 getOrCreate：活会话接上、磁盘有历史就恢复、都没有才新建。
        // 每一条浏览器 SSE 连接各自 poll；不要在网关把多个观察者 share 成一条
        // 上游 poll，否则 cursor 与取消语义会混在一起。
        return sessions.getOrCreate(chatid, request.getHeaders())
                .thenMany(Flux.defer(() -> pollForever(chatid, new PollCursor(browserCursor), request.getHeaders())));
    }

    private Flux<ServerSentEvent<String>> pollForever(
            String chatid, PollCursor cursor, HttpHeaders browserHeaders) {
        return Flux.defer(() -> sessions.poll(chatid, cursor.lastEventId(), browserHeaders)
                        .flatMapMany(response -> {
                            cursor.advance(response.next());
                            return Flux.fromIterable(response.framesOrEmpty())
                                    .map(AgentSseController::toSse);
                        }))
                .repeat();
    }

    private static ServerSentEvent<String> toSse(PollResponse.PollFrame frame) {
        JsonNode event = frame.event();
        if (event == null) {
            throw new IllegalStateException("Rust poll 响应缺少 frame.event");
        }
        // Rust 原 SSE 的 data 也是整个 Frame 信封；保留 id/data 形状后浏览器
        // 的 EventSource、Last-Event-ID 与现有渲染逻辑不用为拉取式改协议。
        return ServerSentEvent.builder(event.toString()).id(Long.toString(frame.id())).build();
    }

    private static Long parseCursor(String value) {
        if (value == null) {
            return null;
        }
        try {
            long cursor = Long.parseLong(value);
            return cursor < 0 ? null : cursor;
        } catch (NumberFormatException ignored) {
            return null;
        }
    }

    private static final class PollCursor {
        private Long lastEventId;

        private PollCursor(Long lastEventId) {
            this.lastEventId = lastEventId;
        }

        private Long lastEventId() {
            return lastEventId;
        }

        private void advance(Long next) {
            if (next == null || next < 0) {
                throw new IllegalStateException("Rust poll 响应缺少合法 next cursor");
            }
            if (lastEventId != null && next < lastEventId) {
                throw new IllegalStateException("Rust poll 响应的 next cursor 倒退");
            }
            // `next` 就是下一次请求要带的 Last-Event-ID，不能再做 +1。
            lastEventId = next;
        }
    }
}
