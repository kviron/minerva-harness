use super::CodexShim;
use serde_json::{Value, json};

const WRITE: &str = "openidea_write_file";
const PATCH: &str = "openidea_patch_file";

pub(super) fn requested(params: &Value) -> bool {
    super::delegated_session(params) && params.pointer("/_meta/openidea/delegatedWritable") == Some(&json!(true))
}

pub(super) fn configure_thread(request: &mut Value) {
    request["sandbox"] = json!("read-only");
    request["approvalPolicy"] = json!("never");
    request["developerInstructions"] = json!(
        "OpenIDEA exposes openidea_write_file and openidea_patch_file as its authorized workspace editing tools. Use these function tools directly for requested file changes, subject to OpenIDEA approval. Codex's own filesystem and shell remain read-only."
    );
}

pub(super) fn definitions() -> Value {
    json!([
        {"type":"function","deferLoading":false,"name":WRITE,"description":"Write a UTF-8 file within the OpenIDEA workspace",
         "inputSchema":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},
                        "required":["path","content"],"additionalProperties":false}},
        {"type":"function","deferLoading":false,"name":PATCH,"description":"Replace unique text within a UTF-8 workspace file",
         "inputSchema":{"type":"object","properties":{"path":{"type":"string"},"oldText":{"type":"string"},"newText":{"type":"string"}},
                        "required":["path","oldText","newText"],"additionalProperties":false}}
    ])
}

fn answer(success: bool, text: &str) -> Value {
    json!({"success":success,"contentItems":[{"type":"inputText","text":text}]})
}

impl CodexShim {
    pub(super) fn scoped_file_call(&self, params: &Value) -> Value {
        let denied = || answer(false, "No active OpenIDEA file write grant");
        let (Some(thread), Some(turn), Some(call)) =
            (params["threadId"].as_str(), params["turnId"].as_str(), params["callId"].as_str())
        else {
            return denied();
        };
        if call.is_empty() || !params["namespace"].is_null() {
            return denied();
        }
        let tool = match params["tool"].as_str() {
            Some(WRITE) => "write_file",
            Some(PATCH) => "patch_file",
            _ => return denied(),
        };
        let pending_ready = self.state.lock().threads.get(thread).and_then(|state| {
            let prompt = state.prompt.as_ref()?;
            prompt.file_turn_ready.as_ref().map(|ready| ready.subscribe())
        });
        if let Some(mut readiness) = pending_ready {
            // Codex can call a dynamic tool before OpenIDEA acknowledges the turn ledger entry.
            self.runtime.block_on(async {
                let _ = tokio::time::timeout(std::time::Duration::from_secs(3), readiness.changed()).await;
            });
        }
        let grant = self.state.lock().threads.get(thread).and_then(|state| {
            let prompt = state.prompt.as_ref()?;
            if !state.scoped_files
                || !prompt.delegated_read_only
                || prompt.cancel
                || prompt.turn.as_deref() != Some(turn)
            {
                return None;
            }
            prompt.host_file_grant.clone()
        });
        let Some(grant) = grant else { return denied() };
        let Some(client) = self.client.get() else { return denied() };
        let outcome = self.runtime.block_on(client.request::<_, Value>(
            "_openidea/scoped_file_tool",
            json!({"sessionId":thread,"turnId":grant,"toolUseId":call,"name":tool,"arguments":params["arguments"]}),
            None,
        ));
        match outcome {
            Ok(result) => answer(true, &result.to_string()),
            Err(error) => answer(false, &error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_file_catalog_contains_only_scoped_file_tools() {
        let catalog = definitions();
        let specs = catalog.as_array().unwrap();
        assert_eq!(specs.len(), 2);
        for (tool, name) in specs.iter().zip([WRITE, PATCH]) {
            assert_eq!(tool["type"], "function");
            assert_eq!(tool["name"], name);
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        }
    }
}
