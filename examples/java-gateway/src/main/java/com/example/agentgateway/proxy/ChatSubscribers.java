package com.example.agentgateway.proxy;

import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import org.springframework.stereotype.Component;

/**
 * 本网关当前在每个 chatid 上还挂着几条浏览器 SSE 连接。
 *
 * <p>只回答一个问题：这条连接断开时，本进程还有没有别的观察者？有就什么都不做，
 * 没有才发显式 {@code POST /sessions/{id}/cancel}。多个 tab 看同一个 chat 很常见，
 * 少了这个计数，关掉其中一个 tab 会把整个 chat 在飞的轮次取消掉。</p>
 *
 * <p>这不是 Rust 订阅计数的副本：Rust 那一份还包含直连 SSE 的客户端和别的网关实例，
 * 并且仍然是取消的权威（引用计数归零 → 宽限 → 取消）。这一份只决定本进程要不要
 * 主动说一声，让「不白烧 token」走显式出路而不是等宽限兜底。</p>
 */
@Component
final class ChatSubscribers {

    private final Map<String, Integer> counts = new ConcurrentHashMap<>();

    void attach(String chatId) {
        counts.merge(chatId, 1, Integer::sum);
    }

    /**
     * 减一并回答「刚断开的是不是本网关在这个 chatid 上的最后一条连接」。
     *
     * <p>{@code computeIfPresent} 返回 null 有两种情况：计数被摘掉（真的是最后一条），
     * 或者这个 chatid 根本没在表里。后者只会出现在 attach 没跑过的路径上，此时发一次
     * 多余的 cancel 也是安全的（没有在飞轮次时 Rust 侧就是个空操作）。</p>
     */
    boolean detachIsLast(String chatId) {
        return counts.computeIfPresent(chatId, (id, count) -> count <= 1 ? null : count - 1) == null;
    }
}
