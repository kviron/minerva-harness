use mh_tool_runtime::ToolDefinition;
use serde_json::{json, Value};

pub(super) fn definitions() -> Vec<ToolDefinition> {
    let lookup = json!({"type":"object","additionalProperties":false,"required":["id"],"properties":{"id":{"type":"string","minLength":1},"timeoutMs":{"type":"integer","minimum":0,"maximum":30000}}});
    let target = json!({
        "type":"object",
        "oneOf":[
            {
                "type":"object",
                "additionalProperties":false,
                "required":["kind","route"],
                "properties":{
                    "kind":{"enum":["minerva"]},
                    "route":{"type":"string","minLength":1},
                    "effort":{"type":["string","null"]}
                }
            },
            {
                "type":"object",
                "additionalProperties":false,
                "required":["kind","agentId","options"],
                "properties":{
                    "kind":{"enum":["ide-agent"]},
                    "agentId":{"type":"string","minLength":1},
                    "options":{
                        "type":"object",
                        "additionalProperties":false,
                        "description":"Optional settings for supported native agents. Use catalog values for model and effort.",
                        "properties":{
                            "model":{"type":"string","minLength":1},
                            "effort":{"type":"string","minLength":1}
                        }
                    }
                }
            }
        ]
    });
    vec![
        definition("subagent_discover", "List available delegation targets. Use an exact returned target when starting a child.", json!({"type":"object","additionalProperties":false})),
        definition("subagent_start", "Start a separate read-only child. Returns an admission receipt; use get/wait for its result. The child continues independently. Set writable true for a target with delegated write support when it needs to edit files; the call waits for the child and temporarily pauses parent writes.", json!({"type":"object","additionalProperties":false,"required":["operationId","target","prompt"],"properties":{"operationId":{"type":"string","minLength":1,"maxLength":200},"target":target,"writable":{"type":"boolean"},"prompt":{"type":"string","minLength":1,"maxLength":32768}}})),
        definition("subagent_list", "List children belonging to this conversation tree.", json!({"type":"object","additionalProperties":false})),
        definition("subagent_interrupt_tree", "Close delegation admission for the root conversation and request cancellation of its active descendants.", json!({"type":"object","additionalProperties":false})),
        definition("subagent_send", "Give a child another task in its existing conversation. Read-only work is queued while the child is active. Set writable true for a supported child after successful completion and when it has no older queued work. This waits for the child with a new write grant.", json!({"type":"object","additionalProperties":false,"required":["id","operationId","prompt"],"properties":{"id":{"type":"string","minLength":1},"operationId":{"type":"string","minLength":1,"maxLength":200},"prompt":{"type":"string","minLength":1,"maxLength":32768},"writable":{"type":"boolean"}}})),
        definition("subagent_get", "Read the child's current state and result.", lookup.clone()),
        definition("subagent_wait", "Wait up to 30 seconds for a child's result or a pending question. Cancelling this wait does not stop the child.", lookup.clone()),
        definition("subagent_questions", "List a child's pending questions.", lookup.clone()),
        definition("subagent_answer", "Answer a child's specific question with an answer from the user or the current task context.", json!({"type":"object","additionalProperties":false,"required":["id","questionId","operationId","answer"],"properties":{"id":{"type":"string","minLength":1},"questionId":{"type":"string","minLength":1},"operationId":{"type":"string","minLength":1,"maxLength":200},"answer":{"type":"string","minLength":1,"maxLength":65536}}})),
        definition("subagent_interrupt", "Request that a child stop. Stopping is not proof that execution has ended.", lookup),
    ]
}

fn definition(name: &str, description: &str, input_schema: Value) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        input_schema,
        output_schema: json!({}),
        side_effect: false,
    }
}
