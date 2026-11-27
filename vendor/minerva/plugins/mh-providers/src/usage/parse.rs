use super::UsageWindow;
use serde_json::Value;
fn number(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str()?.parse().ok()).filter(|v| v.is_finite())
}
fn timestamp(v: &Value) -> Option<u64> {
    if let Some(n) = number(v).filter(|n| *n > 0.0 && *n < 1e15) {
        return Some(if n < 1e12 { n * 1000.0 } else { n } as u64);
    }
    chrono::DateTime::parse_from_rfc3339(v.as_str()?).ok().and_then(|d| u64::try_from(d.timestamp_millis()).ok())
}
fn window(id: &str, v: &Value, percent: &str, reset: &str) -> Option<UsageWindow> {
    Some(UsageWindow {
        id: id.into(),
        used_percent: number(&v[percent])?.clamp(0.0, 100.0),
        reset_at: timestamp(&v[reset]),
        window_seconds: number(&v["limit_window_seconds"]).filter(|n| *n > 0.0 && *n < 1e10).map(|n| n as u64),
    })
}
pub(crate) fn codex(v: &Value) -> Result<Vec<UsageWindow>, &'static str> {
    let rows: Vec<_> = [("primary", "primary_window"), ("secondary", "secondary_window")]
        .into_iter()
        .filter_map(|(id, field)| window(id, &v["rate_limit"][field], "used_percent", "reset_at"))
        .collect();
    if rows.is_empty() {
        Err("invalid-response")
    } else {
        Ok(rows)
    }
}
pub(super) fn go(v: &Value) -> Result<Vec<UsageWindow>, &'static str> {
    let rows: Vec<_> = [("5h", "rolling", 18000), ("weekly", "weekly", 604800), ("monthly", "monthly", 2592000)]
        .into_iter()
        .filter_map(|(id, field, seconds)| {
            let mut row = window(id, &v["usage"][field], "percent", "resetsAt")?;
            row.window_seconds = if id == "monthly" { None } else { Some(seconds) };
            Some(row)
        })
        .collect();
    if rows.is_empty() {
        Err("invalid-response")
    } else {
        Ok(rows)
    }
}
pub(super) fn token_harbor(v: &Value) -> Result<Vec<UsageWindow>, &'static str> {
    let mut rows = Vec::new();
    for (id, enabled, percent) in [("pass", "on_plan", "plan_used_pct"), ("free", "models_enabled", "used_pct")] {
        if v[enabled].as_bool().ok_or("invalid-response")? {
            let used = v[percent].as_f64().filter(|n| (0.0..=100.0).contains(n)).ok_or("invalid-response")?;
            rows.push(UsageWindow {
                id: id.into(),
                used_percent: used,
                reset_at: timestamp(&v["reset_at"]),
                window_seconds: Some(604800),
            });
        }
    }
    Ok(rows)
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn codex_windows_validate_without_inventing_zero() {
        assert!(codex(&json!({"rate_limit":{"primary_window":{"used_percent":null}}})).is_err());
        let rows=codex(&json!({"rate_limit":{"primary_window":{"used_percent":125,"reset_at":1700000000,"limit_window_seconds":18000}}})).unwrap();
        assert_eq!(rows[0].used_percent, 100.0);
        assert_eq!(rows[0].reset_at, Some(1700000000000));
    }
    #[test]
    fn go_accepts_seconds_millis_and_iso() {
        for value in [json!(1700000000), json!(1700000000000u64), json!("2023-11-14T22:13:20Z")] {
            assert_eq!(timestamp(&value), Some(1700000000000));
        }
        assert!(go(&json!({"usage":{"rolling":{"percent":"bad"}}})).is_err());
        assert_eq!(
            go(&json!({"usage":{"rolling":{"percent":0,"resetsAt":1700000000000u64}}})).unwrap()[0].used_percent,
            0.0
        );
    }
}
