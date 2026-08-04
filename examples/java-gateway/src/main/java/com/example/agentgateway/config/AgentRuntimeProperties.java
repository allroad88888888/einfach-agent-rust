package com.example.agentgateway.config;

import java.nio.file.Path;
import java.time.Duration;
import java.util.regex.Pattern;
import org.springframework.boot.context.properties.ConfigurationProperties;

/**
 * Java 宿主启动 agent-server 所需的非机密配置。
 *
 * <p>provider、base URL 与模型都由 {@code providers-config} 指向的 TOML 决定；
 * 真正的 key 不属于 Spring 配置。{@code key-env} 只是那个 key 所在环境变量的名字，
 * 子进程从继承的环境里读取其值。</p>
 */
@ConfigurationProperties(prefix = "agent.runtime")
public record AgentRuntimeProperties(
        String command,
        Path workingDir,
        Path providersConfig,
        Path sessionsDir,
        String keyEnv,
        Duration startupTimeout,
        Duration shutdownTimeout,
        int pollWaitMs) {

    private static final Pattern ENVIRONMENT_VARIABLE = Pattern.compile("[A-Za-z_][A-Za-z0-9_]*");

    public AgentRuntimeProperties {
        command = defaultIfBlank(command, "agent-server");
        workingDir = workingDir == null ? Path.of(".") : workingDir;
        providersConfig = providersConfig == null ? Path.of("providers.toml") : providersConfig;
        sessionsDir = sessionsDir == null ? Path.of("agent-sessions") : sessionsDir;
        keyEnv = defaultIfBlank(keyEnv, "DEEPSEEK_API_KEY");
        startupTimeout = defaultIfNull(startupTimeout, Duration.ofSeconds(20));
        shutdownTimeout = defaultIfNull(shutdownTimeout, Duration.ofSeconds(30));
        pollWaitMs = pollWaitMs == 0 ? 25_000 : pollWaitMs;

        if (!ENVIRONMENT_VARIABLE.matcher(keyEnv).matches()) {
            throw new IllegalArgumentException("agent.runtime.key-env 必须是环境变量名");
        }
        if (startupTimeout.isNegative() || startupTimeout.isZero()) {
            throw new IllegalArgumentException("agent.runtime.startup-timeout 必须大于 0");
        }
        if (shutdownTimeout.isNegative() || shutdownTimeout.isZero()) {
            throw new IllegalArgumentException("agent.runtime.shutdown-timeout 必须大于 0");
        }
        if (pollWaitMs < 1 || pollWaitMs > 60_000) {
            throw new IllegalArgumentException("agent.runtime.poll-wait-ms 必须在 1 到 60000 之间");
        }
    }

    private static String defaultIfBlank(String value, String fallback) {
        return value == null || value.isBlank() ? fallback : value;
    }

    private static <T> T defaultIfNull(T value, T fallback) {
        return value == null ? fallback : value;
    }
}
