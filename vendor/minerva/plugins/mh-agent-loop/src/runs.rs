//! In-process ownership and cancellation of one turn per session.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

struct Operation {
    id: String,
    cancel: watch::Sender<bool>,
    compacting: bool,
}
type ActiveRuns = Arc<Mutex<HashMap<String, Operation>>>;

#[derive(Default)]
pub struct Runs(ActiveRuns);
pub struct RunGuard {
    runs: ActiveRuns,
    session: String,
    pub cancel: watch::Receiver<bool>,
}
impl Runs {
    pub fn acquire(&self, session: &str, id: &str) -> Result<RunGuard, String> {
        self.acquire_operation(session, id, false)
    }
    pub fn reserve_compaction(&self, session: &str, id: &str) -> Result<RunGuard, String> {
        self.acquire_operation(session, id, true)
    }
    pub fn is_compacting(&self, session: &str) -> Result<bool, String> {
        Ok(self.0.lock().map_err(|e| e.to_string())?.get(session).is_some_and(|run| run.compacting))
    }
    fn acquire_operation(&self, session: &str, id: &str, compacting: bool) -> Result<RunGuard, String> {
        let mut runs = self.0.lock().map_err(|e| e.to_string())?;
        if runs.contains_key(session) {
            return Err("В этом диалоге уже выполняется запрос.".into());
        }
        let (tx, rx) = watch::channel(false);
        runs.insert(session.into(), Operation { id: id.into(), cancel: tx, compacting });
        Ok(RunGuard { runs: self.0.clone(), session: session.into(), cancel: rx })
    }
    pub fn cancel_all(&self) {
        if let Ok(runs) = self.0.lock() {
            for run in runs.values() {
                run.cancel.send_replace(true);
            }
        }
    }
    pub fn cancel(&self, session: &str, id: &str) -> Result<bool, String> {
        let runs = self.0.lock().map_err(|e| e.to_string())?;
        if let Some(active) = runs.get(session) {
            if active.id == id {
                active.cancel.send_replace(true);
                return Ok(true);
            }
        }
        Ok(false)
    }
}
impl Drop for RunGuard {
    fn drop(&mut self) {
        if let Ok(mut runs) = self.runs.lock() {
            runs.remove(&self.session);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn excludes_duplicates_cancels_only_matching_run_and_releases() {
        let runs = Runs::default();
        let mut first = runs.acquire("a", "one").unwrap();
        assert!(runs.acquire("a", "two").is_err());
        let _other = runs.acquire("b", "one").unwrap();
        assert!(!runs.cancel("a", "old").unwrap());
        assert!(runs.cancel("a", "one").unwrap());
        first.cancel.changed().await.unwrap();
        assert!(*first.cancel.borrow());
        drop(first);
        assert!(runs.acquire("a", "two").is_ok());
    }
}
