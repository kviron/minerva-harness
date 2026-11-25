//! Retain a bounded last-known sample only across transient errors for the same account.
use super::UsageProvider;
use std::time::Duration;

const MAX_STALE_AGE: Duration = Duration::from_secs(15 * 60);

pub(super) fn retain_samples(rows: &mut [UsageProvider], previous: Option<&[UsageProvider]>, now: u64) {
    let Some(previous) = previous else { return };
    for row in rows {
        if row.status != "unavailable"
            || !matches!(row.error_code.as_deref(), Some("network" | "timeout" | "rate-limited" | "invalid-response"))
        {
            continue;
        }
        let Some(sample) = previous.iter().find(|sample| sample.provider_id == row.provider_id) else { continue };
        let Some(updated_at) = sample.updated_at else { continue };
        if sample.windows.is_empty() || now < updated_at || now - updated_at > MAX_STALE_AGE.as_millis() as u64 {
            continue;
        }
        row.windows = sample.windows.clone();
        row.updated_at = Some(updated_at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::UsageWindow;
    const NOW: u64 = 1_700_000_000_000;
    fn sample(id: &str) -> UsageProvider {
        UsageProvider::empty(id, id).result(Ok(vec![UsageWindow {
            id: "primary".into(),
            window_seconds: Some(18000),
            used_percent: 42.0,
            reset_at: None,
        }]))
    }
    fn failed(id: &str, code: &'static str) -> UsageProvider {
        UsageProvider::empty(id, id).result(Err(code))
    }
    #[test]
    fn transient_errors_retain_original_sample_time_without_extending_its_life() {
        for code in ["network", "timeout", "rate-limited", "invalid-response"] {
            let mut previous = sample("codex");
            previous.updated_at = Some(NOW - 1000);
            let mut rows = vec![failed("codex", code)];
            retain_samples(&mut rows, Some(&[previous]), NOW);
            assert_eq!(rows[0].status, "unavailable");
            assert_eq!(rows[0].windows[0].used_percent, 42.0);
            assert_eq!(rows[0].updated_at, Some(NOW - 1000));
            let mut next = vec![failed("codex", code)];
            retain_samples(&mut next, Some(&rows), NOW + MAX_STALE_AGE.as_millis() as u64);
            assert!(next[0].windows.is_empty());
            assert!(next[0].updated_at.is_none());
        }
    }
    #[test]
    fn authentication_and_account_changes_clear_samples() {
        let mut previous = sample("codex");
        previous.updated_at = Some(NOW);
        let mut auth = vec![failed("codex", "auth"), UsageProvider::empty("codex", "Codex")];
        retain_samples(&mut auth, Some(&[previous.clone()]), NOW);
        assert!(auth.iter().all(|row| row.windows.is_empty()));
        let mut changed = vec![failed("codex", "network")];
        retain_samples(&mut changed, None, NOW);
        assert!(changed[0].windows.is_empty());
    }
    #[test]
    fn providers_are_independent_and_success_accepts_real_resets() {
        let mut previous = sample("codex");
        previous.updated_at = Some(NOW);
        let mut reset = sample("codex");
        reset.windows[0].used_percent = 0.0;
        let mut rows = vec![failed("go", "network"), reset];
        retain_samples(&mut rows, Some(&[previous]), NOW);
        assert!(rows[0].windows.is_empty());
        assert_eq!(rows[1].windows[0].used_percent, 0.0);
    }
}
