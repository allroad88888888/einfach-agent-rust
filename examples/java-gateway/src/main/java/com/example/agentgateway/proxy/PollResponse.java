package com.example.agentgateway.proxy;

import com.fasterxml.jackson.databind.JsonNode;
import java.util.List;

/** Wire shape returned by Rust {@code GET /sessions/{id}/events/poll}. */
record PollResponse(List<PollFrame> frames, Long next) {

    List<PollFrame> framesOrEmpty() {
        return frames == null ? List.of() : frames;
    }

    record PollFrame(long id, JsonNode event) {
    }
}
