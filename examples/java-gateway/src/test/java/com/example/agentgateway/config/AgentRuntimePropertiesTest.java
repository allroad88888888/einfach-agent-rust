package com.example.agentgateway.config;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;

import java.time.Duration;
import org.junit.jupiter.api.Test;

class AgentRuntimePropertiesTest {

    @Test
    void defaultsRemoteToolTimeoutToTenMinutes() {
        AgentRuntimeProperties properties = propertiesWith(null);

        assertThat(properties.remoteToolTimeout()).isEqualTo(Duration.ofMinutes(10));
    }

    @Test
    void acceptsMillisecondRemoteToolTimeout() {
        AgentRuntimeProperties properties = propertiesWith(Duration.ofMillis(250));

        assertThat(properties.remoteToolTimeout()).isEqualTo(Duration.ofMillis(250));
    }

    @Test
    void rejectsSubMillisecondRemoteToolTimeout() {
        assertThatThrownBy(() -> propertiesWith(Duration.ofNanos(1)))
                .isInstanceOf(IllegalArgumentException.class)
                .hasMessageContaining("remote-tool-timeout");
    }

    private static AgentRuntimeProperties propertiesWith(Duration remoteToolTimeout) {
        return new AgentRuntimeProperties(
                null, null, null, null, null, null, null, remoteToolTimeout, 0);
    }
}
