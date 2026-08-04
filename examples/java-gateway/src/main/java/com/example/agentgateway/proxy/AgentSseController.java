package com.example.agentgateway.proxy;

import com.fasterxml.jackson.databind.JsonNode;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
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

    private static final Logger log = LoggerFactory.getLogger(AgentSseController.class);

    private final AgentSessionClient sessions;
    private final ChatSubscribers subscribers;

    public AgentSseController(AgentSessionClient sessions, ChatSubscribers subscribers) {
        this.sessions = sessions;
        this.subscribers = subscribers;
    }

    @GetMapping(value = "/agent/sessions/{chatid}/events", produces = MediaType.TEXT_EVENT_STREAM_VALUE)
    public Flux<ServerSentEvent<String>> events(
            @PathVariable String chatid,
            @RequestHeader(value = "Last-Event-ID", required = false) String lastEventId,
            ServerHttpRequest request) {
        Long browserCursor = parseCursor(lastEventId);
        HttpHeaders browserHeaders = request.getHeaders();
        // POST 是幂等 getOrCreate：活会话接上、磁盘有历史就恢复、都没有才新建。
        // 每一条浏览器 SSE 连接各自 poll；不要在网关把多个观察者 share 成一条
        // 上游 poll，否则 cursor 与取消语义会混在一起。
        return sessions.getOrCreate(chatid, browserHeaders)
                .thenMany(Flux.defer(() -> pollForever(chatid, new PollCursor(browserCursor), browserHeaders)))
                .doOnSubscribe(subscription -> subscribers.attach(chatid))
                .doFinally(signal -> cancelWhenNoViewerLeft(chatid, browserHeaders));
    }

    /**
     * 浏览器这一条流结束（关 tab = CANCEL、上游报错 = ON_ERROR）时的显式出路。
     * 本网关在这个 chatid 上还有别的连接就什么都不做——Rust 的引用计数还没归零，
     * 取消会误伤别的 tab。真的一个不剩才发 cancel，把「不白烧 token」变成主动
     * 动作；Rust 侧的宽限继续兜「网关自己崩了没人发」那一路。
     */
    private void cancelWhenNoViewerLeft(String chatid, HttpHeaders browserHeaders) {
        if (!subscribers.detachIsLast(chatid)) {
            return;
        }
        sessions.cancel(chatid, browserHeaders)
                .subscribe(
                        ignored -> {},
                        error -> log.debug("显式 cancel {} 没成功，交给 Rust 侧宽限兜底", chatid, error));
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
