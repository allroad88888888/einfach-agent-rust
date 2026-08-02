package com.example.agentgateway.proxy;

import java.util.Locale;
import java.util.Set;
import org.springframework.http.HttpHeaders;

/**
 * hop-by-hop 头部：只对相邻两跳有意义，原样转发给下一跳会产生错误语义
 * （比如把浏览器发给网关的 Connection: keep-alive 转给 agent-server，
 * 干扰的是网关和上游之间本该独立的连接管理）。
 *
 * ARCHITECTURE.md「header 做全量转发」一节点名的清单：除这几个之外一律
 * 全量转发，不要在这里加白名单。identity 头（X-Agent-User-Id 等）、
 * traceparent（决策 11）都不在这份名单里，靠「不过滤」自动到达上游——
 * 这也是为什么两个控制器里都找不到专门写 identity/traceparent 转发的
 * 代码：全量转发就是那段代码，0 行。
 */
final class HopByHopHeaders {

    private HopByHopHeaders() {
    }

    private static final Set<String> EXACT_NAMES = Set.of(
            "connection",
            "keep-alive",
            "transfer-encoding",
            "upgrade",
            "te",
            "trailer"
    );

    /** Proxy-* 是前缀族（Proxy-Authenticate/Proxy-Authorization 等），不是单个名字。 */
    private static final String PROXY_PREFIX = "proxy-";

    static HttpHeaders strip(HttpHeaders source) {
        HttpHeaders filtered = new HttpHeaders();
        source.forEach((name, values) -> {
            if (!isHopByHop(name)) {
                filtered.addAll(name, values);
            }
        });
        return filtered;
    }

    private static boolean isHopByHop(String name) {
        String lower = name.toLowerCase(Locale.ROOT);
        return EXACT_NAMES.contains(lower) || lower.startsWith(PROXY_PREFIX);
    }
}
