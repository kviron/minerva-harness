//! Project-wide bound on confirmed idle native delegation connections.
use super::attempt;
use crate::{Live, Task, provider::Driver};
use std::{
    collections::VecDeque,
    sync::{Arc, Weak, atomic::Ordering},
};

const MAX_IDLE: usize = 8;

#[derive(Default)]
pub(crate) struct IdleExecutors(tokio::sync::Mutex<VecDeque<Entry>>);

struct Entry {
    task: Weak<Task>,
    turn: u64,
}

impl IdleExecutors {
    pub(crate) async fn retain(&self, task: &Arc<Task>) {
        let mut entries = self.0.lock().await;
        entries.retain(|entry| {
            entry
                .task
                .upgrade()
                .is_some_and(|candidate| candidate.id != task.id && entry.matches_generation(&candidate))
        });
        let entry = Entry { task: Arc::downgrade(task), turn: task.turns.load(Ordering::Acquire) };
        if !entry.matches_generation(task) {
            return;
        }
        entries.push_back(entry);
        Self::enforce_locked(&mut entries, task).await;
    }

    async fn enforce_locked(entries: &mut VecDeque<Entry>, task: &Task) {
        if entries.len() <= MAX_IDLE {
            return;
        }
        let Ok(store) = attempt::open_for_turn(&task.shared.paths.store_dir).await else { return };
        let eligible: Vec<_> = entries
            .iter()
            .filter_map(|entry| {
                let task = entry.task.upgrade()?;
                if !entry.matches_idle(&task) {
                    return None;
                }
                let session = task.state.lock().session_id.clone()?;
                if !store.settled_for_task(&task.id, &session) {
                    return None;
                }
                Some((task, entry.turn, session))
            })
            .collect();
        drop(store);
        let mut remaining = eligible.len();
        for (task, turn, session) in eligible {
            if remaining <= MAX_IDLE {
                break;
            }
            let _settings = task.setting_changes.lock().await;
            let _connecting = task.connecting.lock().await;
            if let Some(live) = detach_idle(&task, turn, &session) {
                live.peer.kill();
                remaining -= 1;
            }
        }
        entries.retain(|entry| entry.task.upgrade().is_some_and(|task| entry.matches_generation(&task)));
    }
}

impl Entry {
    fn matches_generation(&self, task: &Task) -> bool {
        task.state.lock().delegated_read_only && task.turns.load(Ordering::Acquire) == self.turn
    }

    fn matches_idle(&self, task: &Task) -> bool {
        let state = task.state.lock();
        eligible(task, &state, self.turn)
            && task.live.try_lock().is_some_and(|live| live.as_ref().is_some_and(resumable_native))
    }
}

fn eligible(task: &Task, state: &crate::TaskState, turn: u64) -> bool {
    state.delegated_read_only
        && !state.busy
        && state.background_tasks.is_empty()
        && state.background_writer.is_none()
        && task.permissions.try_lock().is_some_and(|pending| pending.is_empty())
        && task.scoped_native.try_lock().is_some_and(|scope| scope.is_none())
        && task.turns.load(Ordering::Acquire) == turn
}

fn resumable_native(live: &Live) -> bool {
    live.resumable && matches!(live.driver, Driver::Codex | Driver::Claude) && !live.peer.is_closed()
}

fn detach_idle(task: &Task, turn: u64, session: &str) -> Option<Live> {
    let state = task.state.lock();
    if !eligible(task, &state, turn) || state.session_id.as_deref() != Some(session) {
        return None;
    }
    let mut live = task.live.try_lock()?;
    if live.as_ref().is_some_and(|live| resumable_native(live) && live.session == session) { live.take() } else { None }
}
