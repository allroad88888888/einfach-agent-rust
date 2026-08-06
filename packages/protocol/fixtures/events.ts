// issue 032：本文件由 Rust 生成，勿手改。重新生成：cargo run -p agent-server --features ts --example gen_protocol_ts
export const events = [
  {
    "agent": "root",
    "event": {
      "type": "text_delta",
      "data": "streamed answer chunk"
    }
  },
  {
    "agent": "root",
    "event": {
      "type": "thinking_delta",
      "data": "considering which tool to call"
    }
  },
  {
    "agent": "root",
    "event": {
      "type": "tool_call_started",
      "data": {
        "name": "srv:fs/read"
      }
    }
  },
  {
    "agent": "root",
    "event": {
      "type": "preflight_drift_alert",
      "data": {
        "Unexpected": {
          "segment": "Tools"
        }
      }
    }
  },
  {
    "agent": "root",
    "event": {
      "type": "transport_trouble",
      "data": "post_stream ended without a stop reason"
    }
  },
  {
    "agent": "root",
    "event": {
      "type": "tool_executing",
      "data": {
        "call_id": "call_1",
        "request": {
          "tool": "srv:fs/read",
          "input": {
            "path": "/tmp/a.txt"
          },
          "location": "Server",
          "reversibility": "Pure"
        }
      }
    }
  },
  {
    "agent": "root",
    "event": {
      "type": "tool_executed",
      "data": {
        "call_id": "call_1",
        "tool": "srv:fs/read",
        "output_len": 128,
        "is_error": false
      }
    }
  },
  {
    "agent": "root",
    "event": {
      "type": "turn_guard",
      "data": {
        "usage": {
          "prompt": 1000,
          "completion": 64,
          "cached": 900
        },
        "report": {
          "drift": "Clean",
          "reconcile": {
            "Match": {
              "predicted": 900,
              "actual": 900
            }
          },
          "window": {
            "Healthy": {
              "turns": 4,
              "hit_percent": 92,
              "low_streak": 0
            }
          }
        },
        "adjustments": [
          {
            "TemperatureOverridden": {
              "wanted": 0.7,
              "used": 1.0
            }
          }
        ]
      }
    }
  },
  {
    "agent": "root",
    "event": {
      "type": "notice",
      "data": {
        "TurnStatusChanged": {
          "status": "Idle"
        }
      }
    }
  },
  {
    "agent": "root",
    "event": {
      "type": "undo",
      "data": {
        "type": "blocked",
        "data": {
          "entries": 1,
          "barrier_seq": 5,
          "label": "tool_result",
          "tool": "srv:shell/exec",
          "call_id": "call_1"
        }
      }
    }
  },
  {
    "agent": "root",
    "event": {
      "type": "redo",
      "data": {
        "type": "nothing"
      }
    }
  },
  {
    "agent": "root",
    "event": {
      "type": "lagged",
      "data": {
        "skipped": 7
      }
    }
  },
  {
    "agent": "root",
    "event": {
      "type": "session_died",
      "data": {
        "reason": "actor panicked: boom"
      }
    }
  },
  {
    "agent": "root",
    "event": {
      "type": "gap",
      "data": {
        "skipped": 3
      }
    }
  },
  {
    "agent": "root",
    "event": {
      "type": "agent_tree",
      "data": {
        "nodes": [
          {
            "id": "root",
            "parent": null,
            "depth": 0,
            "task": "帮我查一下今天的天气",
            "activity": {
              "Working": {
                "tools": [
                  "srv:agent/spawn"
                ]
              }
            }
          },
          {
            "id": "root/a1",
            "parent": "root",
            "depth": 1,
            "task": "查天气",
            "activity": {
              "Done": {
                "truncated": false
              }
            }
          }
        ]
      }
    }
  },
  {
    "agent": "root",
    "event": {
      "type": "orphaned_child",
      "data": {
        "child": "root/a1",
        "fate": {
          "type": "discarded",
          "data": {
            "bytes": 128,
            "is_error": false
          }
        }
      }
    }
  },
  {
    "agent": "root",
    "event": {
      "type": "transient_source_failure",
      "data": {
        "epoch": 7,
        "cause": {
          "type": "transport_http",
          "data": {
            "status": 502,
            "body": "upstream diagnostic"
          }
        }
      }
    }
  }
] as const;
