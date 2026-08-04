package com.example.agentgateway.runtime;

import com.example.agentgateway.config.AgentRuntimeProperties;
import com.fasterxml.jackson.databind.ObjectMapper;
import jakarta.annotation.PostConstruct;
import jakarta.annotation.PreDestroy;
import java.io.BufferedReader;
import java.io.IOException;
import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.time.Instant;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.springframework.stereotype.Component;

/** Starts one loopback agent-server child process and publishes its negotiated base URI. */
@Component
public final class AgentServerProcess {

    private static final Logger log = LoggerFactory.getLogger(AgentServerProcess.class);
    private static final Duration READY_POLL_INTERVAL = Duration.ofMillis(50);

    private final AgentRuntimeProperties properties;
    private final ObjectMapper objectMapper;
    private volatile Process process;
    private volatile URI baseUri;
    private Path readyDirectory;

    public AgentServerProcess(AgentRuntimeProperties properties, ObjectMapper objectMapper) {
        this.properties = properties;
        this.objectMapper = objectMapper;
    }

    @PostConstruct
    void start() {
        Path workingDirectory = properties.workingDir().toAbsolutePath().normalize();
        Path providersConfig = resolve(workingDirectory, properties.providersConfig());
        Path sessionsDirectory = resolve(workingDirectory, properties.sessionsDir());
        requireConfiguredKey();

        try {
            if (!Files.isRegularFile(providersConfig)) {
                throw new IllegalStateException("agent.runtime.providers-config 不是可读文件");
            }
            Files.createDirectories(sessionsDirectory);
            readyDirectory = Files.createTempDirectory("agent-server-ready-");
            Path readyFile = readyDirectory.resolve("ready.json");
            ProcessBuilder builder = new ProcessBuilder(
                    properties.command(),
                    "--config", providersConfig.toString(),
                    "--sessions-dir", sessionsDirectory.toString(),
                    "--port", "0",
                    "--ready-file", readyFile.toString());
            builder.directory(workingDirectory.toFile());
            builder.redirectErrorStream(true);
            // ProcessBuilder 默认继承环境；显式放回所选变量使配置语义一目了然。
            // 绝不记录这个值，也不把它放进 argv、providers.toml 或 ready file。
            builder.environment().put(properties.keyEnv(), System.getenv(properties.keyEnv()));
            process = builder.start();
            copyChildOutput(process);
            baseUri = awaitReadyFile(process, readyFile);
        } catch (IOException error) {
            stopProcess();
            deleteReadyDirectory();
            throw new IllegalStateException("agent-server 无法启动", error);
        } catch (RuntimeException error) {
            stopProcess();
            deleteReadyDirectory();
            throw error;
        }
    }

    /** Returns the child base URI only after the ready-file handshake completed. */
    public URI resolve(String path) {
        URI readyBaseUri = baseUri;
        if (readyBaseUri == null) {
            throw new IllegalStateException("agent-server 尚未完成 ready-file 握手");
        }
        return readyBaseUri.resolve(path);
    }

    @PreDestroy
    void stop() {
        stopProcess();
        deleteReadyDirectory();
    }

    private void requireConfiguredKey() {
        String key = System.getenv(properties.keyEnv());
        if (key == null || key.isBlank()) {
            throw new IllegalStateException("agent.runtime.key-env 指向的环境变量未设置");
        }
    }

    private URI awaitReadyFile(Process child, Path readyFile) {
        Instant deadline = Instant.now().plus(properties.startupTimeout());
        while (Instant.now().isBefore(deadline)) {
            if (!child.isAlive()) {
                throw new IllegalStateException("agent-server 在 ready-file 握手前退出");
            }
            if (Files.isRegularFile(readyFile)) {
                try {
                    ReadyFile ready = objectMapper.readValue(Files.readString(readyFile), ReadyFile.class);
                    if (ready.port() < 1 || ready.port() > 65_535 || ready.pid() != child.pid()
                            || ready.version() == null || ready.version().isBlank()) {
                        throw new IllegalStateException("agent-server ready-file 内容无效");
                    }
                    return URI.create("http://127.0.0.1:" + ready.port() + "/");
                } catch (IOException ignored) {
                    // Rust 原子发布文件；此处容忍短暂的文件系统可见性延迟。
                }
            }
            sleepUntilReady();
        }
        throw new IllegalStateException("等待 agent-server ready-file 超时");
    }

    private void copyChildOutput(Process child) {
        Thread outputThread = new Thread(() -> {
            try (BufferedReader reader = child.inputReader(StandardCharsets.UTF_8)) {
                for (String line; (line = reader.readLine()) != null;) {
                    log.info("[agent-server] {}", line);
                }
            } catch (IOException error) {
                if (child.isAlive()) {
                    log.warn("读取 agent-server 输出失败", error);
                }
            }
        }, "agent-server-output");
        outputThread.setDaemon(true);
        outputThread.start();
    }

    private void stopProcess() {
        Process child = process;
        if (child == null || !child.isAlive()) {
            return;
        }
        child.destroy(); // Unix 上是 SIGTERM；Rust 收到后 close_all 并落盘快照。
        try {
            if (!child.waitFor(properties.shutdownTimeout().toMillis(), java.util.concurrent.TimeUnit.MILLISECONDS)) {
                log.warn("agent-server 未在关闭宽限内退出，强制结束子进程");
                child.destroyForcibly();
            }
        } catch (InterruptedException error) {
            Thread.currentThread().interrupt();
            child.destroyForcibly();
        }
    }

    private void deleteReadyDirectory() {
        if (readyDirectory == null) {
            return;
        }
        try {
            Files.deleteIfExists(readyDirectory.resolve("ready.json"));
            Files.deleteIfExists(readyDirectory);
        } catch (IOException error) {
            log.debug("无法删除 agent-server ready-file 临时目录", error);
        }
    }

    private static Path resolve(Path workingDirectory, Path path) {
        return (path.isAbsolute() ? path : workingDirectory.resolve(path)).normalize();
    }

    private static void sleepUntilReady() {
        try {
            Thread.sleep(READY_POLL_INTERVAL.toMillis());
        } catch (InterruptedException error) {
            Thread.currentThread().interrupt();
            throw new IllegalStateException("等待 agent-server ready-file 时被中断", error);
        }
    }

    private record ReadyFile(int port, long pid, String version) {
    }
}
