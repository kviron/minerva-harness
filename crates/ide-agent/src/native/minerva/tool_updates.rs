use serde_json::{Value, json};

pub(super) fn call_update(call: &Value, known: bool) -> Value {
    let shell = call["name"] == "shell";
    let mut update = json!({
        "sessionUpdate":if known {"tool_call_update"} else {"tool_call"},
        "toolCallId":call["callId"],
        "title":if shell {call["arguments"]["command"].clone()} else {call["name"].clone()},
        "kind":if shell {"execute"} else {"other"},
        "status":match call["status"].as_str() {
            Some("completed")=>"completed",Some("failed"|"cancelled")=>"failed",_=>"in_progress",
        },
        "rawInput":call["arguments"],
    });
    if shell && call["meta"].is_object() {
        update["rawOutput"] = call["meta"].clone();
    }
    update
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_progress_reaches_the_existing_tool_card_before_settlement() {
        let call = json!({"name":"shell","callId":"one","arguments":{"command":"npm run dev"},
            "status":"running","meta":{"stdout":"ready","stderr":"","truncated":false}});
        let update = call_update(&call, true);
        assert_eq!(update["rawOutput"]["stdout"], "ready");
        assert_eq!(update["kind"], "execute");
        assert_eq!(update["status"], "in_progress");
        assert_eq!(update["title"], "npm run dev");
    }
}
