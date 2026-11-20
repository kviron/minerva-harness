//! Embedded model/API engine. Uses the same in-process event normalization as native drivers.
use ide_jsonrpc::{Handler, Peer, codes};
use ide_minerva::{
    Service,
    minerva_runtime::{RunMessage, Runtime, StartRunRequest},
};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub(crate) async fn discover(
    shared: &Arc<crate::Shared>,
    spec: &crate::provider::ProviderSpec,
    model: Option<&str>,
) -> Result<crate::catalogs::Discovered, String> {
    let peer =
        (shared.connector)(spec, &shared.paths.project_root, Arc::new(ide_jsonrpc::NoopHandler), Box::new(|_| {}))
            .map_err(|e| e.to_string())?;
    let _kill = crate::catalogs::KillPeer(peer.clone());
    let reply: Value =
        tokio::time::timeout(Duration::from_secs(20), peer.request("_minerva/config", json!({"model":model}), None))
            .await
            .map_err(|_| "Minerva model discovery timed out")?
            .map_err(|e| e.to_string())?;
    Ok(crate::catalogs::Discovered {
        config: crate::config::options_of(&reply),
        commands: None,
        levels: Default::default(),
    })
}

pub(crate) fn connect(
    service: Arc<Service>,
    cwd: &Path,
    client: Arc<dyn Handler>,
    model_prefix: Option<String>,
) -> Peer {
    super::serve(
        Arc::new(Engine {
            service,
            model_prefix,
            cwd: cwd.to_path_buf(),
            client: OnceLock::new(),
            runs: Arc::new(Mutex::new(HashMap::new())),
            early_cancel: Mutex::new(HashMap::new()),
            runtime: tokio::runtime::Handle::current(),
        }),
        client,
    )
}

struct Engine {
    service: Arc<Service>,
    model_prefix: Option<String>,
    cwd: PathBuf,
    client: OnceLock<Peer>,
    runs: Arc<Mutex<HashMap<String, ActiveRun>>>,
    early_cancel: Mutex<HashMap<String, Instant>>,
    runtime: tokio::runtime::Handle,
}

#[derive(Clone)]
struct ActiveRun {
    id: String,
    cancelled: Arc<AtomicBool>,
}

impl Engine {
    fn update(&self, session: &str, update: Value) -> Result<(), String> {
        self.client
            .get()
            .ok_or("Minerva connection is closed")?
            .notify("session/update", json!({"sessionId":session,"update":update}))
            .map_err(|e| e.to_string())
    }

    async fn options(&self, runtime: &Runtime, current: Option<&str>) -> Result<Value, String> {
        let providers = serde_json::to_value(runtime.plugins.providers()?.list().await).map_err(|e| e.to_string())?;
        let mut models = Vec::new();
        for provider in providers.as_array().ok_or("Invalid provider catalog")? {
            let id = provider["id"].as_str().ok_or("Provider has no ID")?;
            for model in provider["models"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                models.push(json!({"value":format!("api:{id}/{model}"),"name":format!("{} · {model}",provider["name"].as_str().unwrap_or(id))}));
            }
        }
        for model in runtime.plugins.models().await? {
            models.push(json!({"value":model.id,"name":format!("{} · {}",model.group,model.name)}));
        }
        if let Some(prefix) = &self.model_prefix {
            models.retain(|model| model["value"].as_str().is_some_and(|value| value.starts_with(prefix)));
        }
        let selected = current
            .map(str::to_owned)
            .or_else(|| models.first().and_then(|v| v["value"].as_str()).map(str::to_owned))
            .unwrap_or_default();
        Ok(json!([
            {"id":"model","name":"Model","category":"model","type":"select","currentValue":selected,"options":models},
            {"id":"mode","name":"Permissions","category":"mode","type":"select","currentValue":"ask","options":[
                {"value":"ask","name":"Ask before changes"},{"value":"allow","name":"Allow changes"}
            ]}
        ]))
    }

    async fn check_session(&self, runtime: &Arc<Runtime>, id: &str) -> Result<(), String> {
        let workspace = self.service.workspace(&self.cwd).await?;
        if !workspace.session_ids.iter().any(|session| session == id) {
            return Err("Session belongs to another project".into());
        }
        let session = runtime.sessions.get(id)?;
        if let (Some(prefix), Some(model)) = (&self.model_prefix, session.model.as_deref()) {
            if !model.starts_with(prefix) {
                return Err("Session belongs to another model provider".into());
            }
        }
        Ok(())
    }

    async fn session_options(&self, runtime: &Arc<Runtime>, id: &str) -> Result<Value, String> {
        let session = runtime.sessions.get(id)?;
        let mut options = self.options(runtime, session.model.as_deref()).await?;
        let settings = runtime.harness_call("workspace.get", json!({"sessionId":id})).await?;
        options[1]["currentValue"] = json!(if settings["permissionPolicy"] == "allow" { "allow" } else { "ask" });
        Ok(options)
    }

    async fn handle(&self, method: &str, params: Value) -> Result<Value, String> {
        if method == "initialize" {
            return Ok(
                json!({"protocolVersion":1,"agentInfo":{"name":"minerva","title":"Minerva","version":env!("CARGO_PKG_VERSION")},"authMethods":[],"agentCapabilities":{"loadSession":true,"promptCapabilities":{"image":true,"embeddedContext":true}}}),
            );
        }
        let runtime = self.service.runtime().await?;
        match method {
            "_minerva/config" => Ok(json!({"configOptions":self.options(&runtime,params["model"].as_str()).await?})),
            "session/new" => {
                let workspace = self.service.workspace(&self.cwd).await?;
                let id = uuid::Uuid::new_v4().to_string();
                runtime.create_session(&id, Some(&workspace.id))?;
                let options = self.options(&runtime, None).await?;
                let model = options[0]["currentValue"].as_str().unwrap_or_default();
                if !model.is_empty() {
                    runtime.sessions.set_model(&id, model)?;
                }
                Ok(json!({"sessionId":id,"configOptions":options}))
            }
            "session/load" | "session/resume" => {
                let id = string(&params, "sessionId")?;
                self.check_session(&runtime, id).await?;
                if method == "session/load" {
                    for message in runtime.sessions.load(id)? {
                        for part in message.parts {
                            let kind = match (message.role.as_str(), part.kind.as_str()) {
                                ("user", "text") => "user_message_chunk",
                                (_, "reasoning") => "agent_thought_chunk",
                                (_, "text") => "agent_message_chunk",
                                _ => continue,
                            };
                            if let Some(text) = part.data["text"].as_str() {
                                self.update(id, json!({"sessionUpdate":kind,"content":{"type":"text","text":text}}))?;
                            }
                        }
                    }
                }
                Ok(json!({"configOptions":self.session_options(&runtime,id).await?}))
            }
            "session/set_config_option" => {
                let id = string(&params, "sessionId")?;
                self.check_session(&runtime, id).await?;
                match string(&params, "configId")? {
                    "model" => {
                        let model = string(&params, "value")?;
                        let options = self.options(&runtime, None).await?;
                        if !options[0]["options"].as_array().into_iter().flatten().any(|v| v["value"] == model) {
                            return Err("Unknown API model".into());
                        }
                        runtime.sessions.set_model(id, model)?;
                    }
                    "mode" => {
                        let policy = string(&params, "value")?;
                        if !matches!(policy, "ask" | "allow") {
                            return Err("Unknown permission mode".into());
                        }
                        let mut settings = runtime.harness_call("workspace.get", json!({"sessionId":id})).await?;
                        settings["permissionPolicy"] = json!(policy);
                        runtime.harness_call("workspace.save", json!({"sessionId":id,"settings":settings})).await?;
                    }
                    _ => return Err("Unsupported Minerva option".into()),
                }
                Ok(json!({"configOptions":self.session_options(&runtime,id).await?}))
            }
            "session/prompt" => self.prompt(runtime, params).await,
            _ => Err(format!("Unsupported Minerva method: {method}")),
        }
    }

    async fn prompt(&self, runtime: Arc<Runtime>, params: Value) -> Result<Value, String> {
        let id = string(&params, "sessionId")?;
        self.check_session(&runtime, id).await?;
        if self.early_cancel.lock().remove(id).is_some_and(|time| time.elapsed() < Duration::from_secs(2)) {
            return Ok(json!({"stopReason":"cancelled"}));
        }
        let content = params["prompt"].as_array().ok_or("Missing prompt")?;
        let mut text = String::new();
        let mut attachments = Vec::new();
        for block in content {
            match block["type"].as_str() {
                Some("text") => text.push_str(block["text"].as_str().ok_or("Invalid text block")?),
                Some("resource") => text
                    .push_str(block["resource"]["text"].as_str().ok_or("Only embedded text resources are supported")?),
                Some("image") => {
                    if attachments.len() >= 8 {
                        return Err("At most eight images are supported".into());
                    }
                    let mime = string(block, "mimeType")?;
                    if !matches!(mime, "image/png" | "image/jpeg" | "image/gif" | "image/webp") {
                        return Err("Unsupported image type".into());
                    }
                    attachments.push(runtime.harness_call("attachments.put",json!({"sessionId":id,"name":format!("image-{}",attachments.len()),"mimeType":mime,"data":string(block,"data")?})).await?);
                }
                _ => return Err("Unsupported prompt content".into()),
            }
            text.push('\n');
            if text.len() > 2_000_000 {
                return Err("Prompt exceeds 2 MB".into());
            }
        }
        if text.trim().is_empty() && attachments.is_empty() {
            return Err("Prompt is empty".into());
        }
        let model =
            self.session_options(&runtime, id).await?[0]["currentValue"].as_str().unwrap_or_default().to_owned();
        if model.is_empty() {
            return Err("Configure an API model in Minerva settings first".into());
        }
        runtime.sessions.set_model(id, &model)?;
        let run = uuid::Uuid::new_v4().to_string();
        let active = ActiveRun { id: run.clone(), cancelled: Arc::new(AtomicBool::new(false)) };
        {
            let mut runs = self.runs.lock();
            if runs.contains_key(id) {
                return Err("A turn is already running".into());
            }
            runs.insert(id.into(), active.clone());
        }
        if self.early_cancel.lock().remove(id).is_some_and(|time| time.elapsed() < Duration::from_secs(2)) {
            active.cancelled.store(true, Ordering::Release);
        }
        let result = self.run_prompt(&runtime, id, &active, model, text, attachments).await;
        if result.is_err() {
            let _ = runtime.cancel_run(id, &run);
        }
        self.runs.lock().remove(id);
        if runtime.get_run(id, &run).is_ok_and(|snapshot| snapshot.status == "cancelled") {
            return Ok(json!({"stopReason":"cancelled"}));
        }
        result
    }

    async fn run_prompt(
        &self,
        runtime: &Arc<Runtime>,
        id: &str,
        active: &ActiveRun,
        model: String,
        text: String,
        attachments: Vec<Value>,
    ) -> Result<Value, String> {
        let run = active.id.as_str();
        let baseline = runtime.sessions.baseline(id)?;
        runtime
            .start_run(StartRunRequest {
                session_id: id.into(),
                request_id: run.into(),
                parent_id: baseline.messages.last().map(|m| m.id.clone()),
                message: RunMessage { id: uuid::Uuid::new_v4().to_string(), text },
                model,
                options: json!({"attachments":attachments}),
            })
            .await?;
        let mut rendered = HashMap::<usize, String>::new();
        let mut tools = HashMap::<String, Value>::new();
        loop {
            if active.cancelled.load(Ordering::Acquire) {
                let _ = runtime.cancel_run(id, run);
            }
            let snapshot = runtime.get_run(id, run)?;
            for (index, part) in snapshot.parts.iter().enumerate() {
                match part.kind.as_str() {
                    "text" | "reasoning" => {
                        let text = part.data["text"].as_str().unwrap_or_default();
                        let previous = rendered.entry(index).or_default();
                        if !text.starts_with(previous.as_str()) {
                            return Err("Non-append stream update".into());
                        }
                        let suffix = &text[previous.len()..];
                        if !suffix.is_empty() {
                            self.update(id,json!({"sessionUpdate":if part.kind=="text" {"agent_message_chunk"}else{"agent_thought_chunk"},"content":{"type":"text","text":suffix}}))?;
                            *previous = text.into();
                        }
                    }
                    "tool-call" => {
                        let call = &part.data;
                        let call_id = call["callId"].as_str().unwrap_or_default();
                        if tools.get(call_id) != Some(call) {
                            self.update(id,json!({"sessionUpdate":if tools.contains_key(call_id){"tool_call_update"}else{"tool_call"},"toolCallId":call_id,"title":call["name"],"kind":"other","status":match call["status"].as_str(){Some("completed")=>"completed",Some("failed")=>"failed",_=>"in_progress"},"rawInput":call["arguments"]}))?;
                            tools.insert(call_id.into(), call.clone());
                        }
                    }
                    "tool-result" => {
                        let result = &part.data;
                        let key = format!("result:{}", result["callId"]);
                        if tools.get(&key) != Some(result) {
                            self.update(id,json!({"sessionUpdate":"tool_call_update","toolCallId":result["callId"],"status":if result["isError"]==true{"failed"}else{"completed"},"rawOutput":result["output"]}))?;
                            tools.insert(key, result.clone());
                        }
                    }
                    _ => {}
                }
            }
            if snapshot.status != "running" {
                if let Some(error) = snapshot.error {
                    return Err(error);
                }
                return Ok(json!({"stopReason":if snapshot.status=="cancelled"{"cancelled"}else{"end_turn"}}));
            }
            let interactions = runtime.harness_call("interactions.list", json!({"sessionId":id})).await?;
            for pending in interactions.as_array().into_iter().flatten() {
                self.interaction(runtime, id, pending).await?;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn interaction(&self, runtime: &Arc<Runtime>, id: &str, pending: &Value) -> Result<(), String> {
        let mut tool =
            json!({"toolCallId":pending["callId"],"title":pending["prompt"],"kind":"other","status":"pending"});
        let question = pending["kind"] == "question";
        let options = if question {
            tool["_meta"] = json!({"questions":[{"answerKey":"answer","header":pending["title"],"question":pending["prompt"],"multiSelect":false,"options":pending["options"].as_array().into_iter().flatten().map(|label|json!({"label":label})).collect::<Vec<_>>()}]});
            json!([{"optionId":"submit","name":"Submit","kind":"allow_once"}])
        } else {
            json!([{"optionId":"allow","name":"Allow once","kind":"allow_once"},{"optionId":"deny","name":"Deny","kind":"reject_once"}])
        };
        let peer = self.client.get().ok_or("Connection closed")?;
        let request = peer.request::<_, Value>(
            "session/request_permission",
            json!({"sessionId":id,"toolCall":tool,"options":options}),
            None,
        );
        tokio::pin!(request);
        let deadline = tokio::time::sleep(Duration::from_secs(600));
        tokio::pin!(deadline);
        let response: Value = loop {
            tokio::select! {
                response = &mut request => break response.map_err(|e|e.to_string())?,
                _ = &mut deadline => return Err("Minerva interaction timed out".into()),
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    let run = pending["runId"].as_str().ok_or("Interaction has no run ID")?;
                    if runtime.get_run(id,run)?.status != "running" { return Err("Minerva turn stopped".into()); }
                }
            }
        };
        let choice = response.pointer("/outcome/optionId").and_then(Value::as_str).unwrap_or("deny");
        let answer = if question {
            serde_json::from_str::<Value>(choice).ok().and_then(|v| v["answer"][0].as_str().map(str::to_owned))
        } else {
            None
        };
        if question && answer.is_none() {
            let run = string(pending, "runId")?;
            let _ = runtime.cancel_run(id, run);
            return Err("Minerva question cancelled".into());
        }
        let accepted = runtime.harness_call("interactions.answer",json!({"sessionId":id,"id":pending["id"],"decision":if question{"answer"}else if choice=="allow"{"allow"}else{"deny"},"answer":answer})).await?;
        if accepted != true {
            return Err("Minerva interaction is no longer pending".into());
        }
        Ok(())
    }
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key].as_str().ok_or_else(|| format!("Missing {key}"))
}

impl Handler for Engine {
    fn on_notification(&self, method: &str, params: Value) {
        if method == "session/cancel"
            && let Some(id) = params["sessionId"].as_str()
        {
            let run = self.runs.lock().get(id).cloned();
            if let Some(run) = run {
                run.cancelled.store(true, Ordering::Release);
                let service = self.service.clone();
                let id = id.to_owned();
                self.runtime.spawn(async move {
                    if let Ok(runtime) = service.runtime().await {
                        let _ = runtime.cancel_run(&id, &run.id);
                    }
                });
            } else {
                let mut cancelled = self.early_cancel.lock();
                cancelled.retain(|_, time| time.elapsed() < Duration::from_secs(2));
                cancelled.insert(id.into(), Instant::now());
            }
        }
    }
    fn on_request(&self, method: &str, params: Value) -> Result<Value, (i64, String)> {
        self.runtime.block_on(self.handle(method, params)).map_err(|error| (codes::INTERNAL_ERROR, error))
    }
}

impl super::Shim for Engine {
    fn attach(&self, client: Peer) {
        let _ = self.client.set(client.clone());
        let runs = self.runs.clone();
        let service = self.service.clone();
        self.runtime.spawn(async move {
            client.closed().await;
            let pending: Vec<_> = runs.lock().drain().collect();
            if !pending.is_empty()
                && let Ok(runtime) = service.runtime().await
            {
                for (id, run) in pending {
                    run.cancelled.store(true, Ordering::Release);
                    let _ = runtime.cancel_run(&id, &run.id);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Client(Arc<Mutex<Vec<Value>>>);
    impl Handler for Client {
        fn on_notification(&self, _: &str, params: Value) {
            self.0.lock().push(params);
        }
        fn on_request(&self, _: &str, _: Value) -> Result<Value, (i64, String)> {
            Ok(json!({"outcome":{"outcome":"selected","optionId":"deny"}}))
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn each_connection_only_offers_and_accepts_its_own_models() {
        let dir = tempfile::tempdir().unwrap();
        let service = Arc::new(Service::new(dir.path().join("data"), dir.path().join("config")));
        let runtime = service.runtime().await.unwrap();
        for id in ["one", "two"] {
            runtime.plugins.providers().unwrap().save(serde_json::from_value(json!({
                "id":id,"name":id,"baseUrl":"http://localhost:1234/v1","models":["model"],"apiKey":null,"isNew":true,"protocol":"ollama"
            })).unwrap()).await.unwrap();
        }
        let peer = connect(service.clone(), dir.path(), Arc::new(Client(Arc::default())), Some("api:one/".into()));
        let new: Value = peer.request("session/new", json!({}), None).await.unwrap();
        assert_eq!(new["configOptions"][0]["options"].as_array().unwrap().len(), 1);
        assert_eq!(new["configOptions"][0]["currentValue"], "api:one/model");
        assert!(
            peer.request::<_, Value>(
                "session/set_config_option",
                json!({"sessionId":new["sessionId"],"configId":"model","value":"api:two/model"}),
                None
            )
            .await
            .is_err()
        );
        peer.kill();
        service.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn embedded_sessions_are_project_scoped_and_models_are_validated() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let service = Arc::new(Service::new(dir.path().join("data"), dir.path().join("config")));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let peer = connect(service.clone(), &a, Arc::new(Client(seen.clone())), None);
        let init: Value = peer.request("initialize", json!({}), None).await.unwrap();
        assert_eq!(init["agentInfo"]["name"], "minerva");
        let _: Value = peer.request("_minerva/config", json!({}), None).await.unwrap();
        assert!(service.runtime().await.unwrap().sessions.list().unwrap().is_empty());
        let new: Value = peer.request("session/new", json!({}), None).await.unwrap();
        let id = new["sessionId"].as_str().unwrap();
        assert!(
            peer.request::<_, Value>(
                "session/set_config_option",
                json!({"sessionId":id,"configId":"model","value":"api:missing/model"}),
                None
            )
            .await
            .is_err()
        );
        let other = connect(service.clone(), &b, Arc::new(Client(seen.clone())), None);
        assert!(other.request::<_, Value>("session/load", json!({"sessionId":id}), None).await.is_err());
        let _: Value = peer.request("session/load", json!({"sessionId":id}), None).await.unwrap();
        let _: Value = peer
            .request("session/set_config_option", json!({"sessionId":id,"configId":"mode","value":"allow"}), None)
            .await
            .unwrap();
        let loaded: Value = peer.request("session/load", json!({"sessionId":id}), None).await.unwrap();
        assert_eq!(loaded["configOptions"][1]["currentValue"], "allow");
        peer.notify("session/cancel", json!({"sessionId":id})).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let cancelled: Value = peer
            .request(
                "session/prompt",
                json!({"sessionId":id,"prompt":[{"type":"text","text":"cancel before start"}]}),
                None,
            )
            .await
            .unwrap();
        assert_eq!(cancelled["stopReason"], "cancelled");
        assert!(
            peer.request::<_, Value>(
                "session/prompt",
                json!({"sessionId":id,"prompt":[{"type":"image","data":"x"}]}),
                None
            )
            .await
            .is_err()
        );
        peer.kill();
        other.kill();
        service.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn direct_api_turn_streams_into_the_existing_agent_chat() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut input = Vec::new();
            let mut chunk = [0; 4096];
            loop {
                let n = socket.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                input.extend_from_slice(&chunk[..n]);
                if let Some(end) = input.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&input[..end]);
                    let length = header
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|n| n.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if input.len() >= end + 4 + length {
                        break;
                    }
                }
                assert!(input.len() < 2_000_000);
            }
            assert!(String::from_utf8_lossy(&input).contains("fixture question"));
            assert!(String::from_utf8_lossy(&input).contains("data:image/png;base64,"));
            let body = "data: {\"id\":\"test\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"test\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"fixture answer\"},\"finish_reason\":null}]}\n\ndata: {\"id\":\"test\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"test\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        std::fs::create_dir(&root).unwrap();
        let service = Arc::new(Service::new(dir.path().join("data"), dir.path().join("config")));
        let runtime = service.runtime().await.unwrap();
        runtime.plugins.providers().unwrap().save(serde_json::from_value(json!({"id":"fixture","name":"Fixture","baseUrl":format!("http://{address}/v1"),"models":["test"],"apiKey":null,"isNew":true,"protocol":"ollama"})).unwrap()).await.unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let peer = connect(service.clone(), &root, Arc::new(Client(seen.clone())), None);
        let new: Value = peer.request("session/new", json!({}), None).await.unwrap();
        assert_eq!(new["configOptions"][0]["currentValue"], "api:fixture/test");
        let reply: Value = peer
            .request(
                "session/prompt",
                json!({"sessionId":new["sessionId"],"prompt":[{"type":"text","text":"fixture question"},{"type":"image","mimeType":"image/png","data":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg=="}]}),
                None,
            )
            .await
            .unwrap();
        assert_eq!(reply["stopReason"], "end_turn");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(seen.lock().iter().any(|e| e["update"]["content"]["text"] == "fixture answer"));
        server.await.unwrap();
        peer.kill();
        service.shutdown().await.unwrap();
    }
}
