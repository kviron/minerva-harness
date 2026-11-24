use crate::sessions::ContentPart;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteSelection {
    pub revision: i64,
    pub model: String,
    pub reasoning_effort: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteDispatch {
    pub run_id: String,
    pub effect_id: String,
    pub turn: u64,
    pub selection: RouteSelection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchAdmission {
    Recorded { changed: bool },
    Duplicate,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[derive(ts_rs::TS)]
pub struct RunMessage {
    pub id: String,
    pub text: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[derive(ts_rs::TS)]
pub struct StartRunRequest {
    pub session_id: String,
    pub request_id: String,
    pub parent_id: Option<String>,
    pub message: RunMessage,
    pub model: String,
    #[serde(default)]
    #[ts(type = "JsonValue")]
    pub options: serde_json::Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[derive(ts_rs::TS)]
pub struct RunHandle {
    pub session_id: String,
    pub run_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[derive(ts_rs::TS)]
pub struct RunSnapshot {
    pub session_id: String,
    pub run_id: String,
    pub status: String,
    pub parts: Vec<ContentPart>,
    pub error: Option<String>,
}
