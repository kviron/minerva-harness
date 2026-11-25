use crate::{AgentBackgroundTask, now_ms};
use serde_json::Value;

const OUTPUT_BYTES: usize = 8 * 1024;

pub(crate) fn is_active(task: &Value) -> bool {
    is_active_status(task.get("status").and_then(Value::as_str))
}

fn is_active_status(status: Option<&str>) -> bool {
    !matches!(status, Some("completed" | "failed" | "stopped" | "killed"))
}

impl crate::Task {
    pub(super) fn clear_active_background(&self) {
        let changed = {
            let mut state = self.state.lock();
            let previous = state.task.background.len();
            state.task.background.retain(|task| !is_active_status(task.status.as_deref()));
            previous != state.task.background.len()
        };
        if changed {
            self.emit_task();
        }
    }
}

pub(crate) fn output_tail(text: &str) -> String {
    output_tail_with_limit(text, OUTPUT_BYTES)
}

pub(crate) fn output_tail_with_limit(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.into();
    }
    const NOTICE: &str = "[earlier output omitted]\n";
    if limit <= NOTICE.len() {
        return NOTICE[..limit].into();
    }
    let mut start = text.len() - limit.saturating_sub(NOTICE.len());
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("{NOTICE}{}", &text[start..])
}

pub(crate) fn shown_background(tasks: &[Value], known: &[AgentBackgroundTask]) -> Vec<AgentBackgroundTask> {
    let now = now_ms();
    tasks
        .iter()
        .filter_map(|task| {
            let id = task.get("task_id")?.as_str()?.to_owned();
            let started_ms = known.iter().find(|seen| seen.id == id).map(|seen| seen.started_ms).unwrap_or_else(|| {
                task.get("started_ms")
                    .and_then(Value::as_f64)
                    .filter(|time| time.is_finite() && *time > 0.0)
                    .unwrap_or(now)
            });
            let text = |key: &str| task.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
            Some(AgentBackgroundTask {
                id,
                kind: text("task_type"),
                description: text("description"),
                started_ms,
                output: task.get("output").and_then(Value::as_str).map(output_tail),
                status: task.get("status").and_then(Value::as_str).map(str::to_owned),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn old_agents_keep_their_start_time_and_do_not_offer_output() {
        let rows = [json!({"task_id":"one","task_type":"local_bash","description":"server"})];
        let first = shown_background(&rows, &[]);
        assert!(first[0].output.is_none());
        let next = shown_background(&rows, &first);
        assert_eq!(first, next);
    }

    #[test]
    fn output_is_bounded_without_splitting_unicode_and_process_time_survives_reconnect() {
        let rows = [json!({"task_id":"one","started_ms":1234,"output":"Ж".repeat(OUTPUT_BYTES)})];
        let shown = shown_background(&rows, &[]);
        assert_eq!(shown[0].started_ms, 1234.0);
        let output = shown[0].output.as_ref().unwrap();
        assert!(output.len() <= OUTPUT_BYTES);
        assert!(output.starts_with("[earlier output omitted]"));
        assert!(output.ends_with('Ж'));
        assert!(!output.contains('\u{fffd}'));
    }

    #[test]
    fn recent_outcomes_are_visible_but_only_live_tasks_hold_the_connection() {
        let tasks = [
            json!({"task_id":"old"}),
            json!({"task_id":"stopping","status":"stopping"}),
            json!({"task_id":"done","status":"completed"}),
            json!({"task_id":"bad","status":"failed"}),
        ];
        assert_eq!(tasks.iter().filter(|task| is_active(task)).count(), 2);
        let visible = shown_background(&tasks, &[]);
        assert_eq!(visible.len(), 4);
        assert_eq!(visible[2].status.as_deref(), Some("completed"));
    }
}
