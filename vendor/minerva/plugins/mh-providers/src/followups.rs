use crate::FollowupControl;
use mh_sessions::delegation::PreferredFollowup;
use rig_core::message::Message;
use std::sync::{Arc, Mutex};

type History = Vec<Arc<Message>>;

struct Inserted {
    position: usize,
    followup: PreferredFollowup,
    confirmed: bool,
}

pub(super) struct StepFollowups {
    control: Option<Arc<dyn FollowupControl>>,
    inserted: Mutex<Vec<Inserted>>,
}

impl StepFollowups {
    pub(super) fn new(control: Option<Arc<dyn FollowupControl>>) -> Self {
        Self { control, inserted: Mutex::new(Vec::new()) }
    }

    pub(super) fn prepare(&self, history: &[Arc<Message>]) -> Result<(History, Option<PreferredFollowup>), String> {
        let mut history = history.to_vec();
        let Some(control) = &self.control else {
            return Ok((history, None));
        };
        for (offset, entry) in self.inserted.lock().map_err(|error| error.to_string())?.iter().enumerate() {
            let position = entry.position.saturating_add(offset).min(history.len());
            history.insert(position, Arc::new(Message::user(&entry.followup.prompt)));
        }
        let pending = control.preferred()?;
        if let Some(entry) = &pending {
            history.push(Arc::new(Message::user(&entry.prompt)));
        }
        Ok((history, pending))
    }

    pub(super) fn claim(
        &self,
        position: usize,
        pending: Option<PreferredFollowup>,
        history: &crate::rig_history::HistoryHook,
    ) -> Result<(), String> {
        if let (Some(control), Some(followup)) = (&self.control, pending) {
            let mut inserted = self.inserted.lock().map_err(|error| error.to_string())?;
            if !control.claim(&followup.work_item_id)? {
                return Err("Subagent follow-up is no longer available for this request".into());
            }
            history.stage_followup(&followup.work_item_id, &followup.prompt)?;
            inserted.push(Inserted { position, followup, confirmed: false });
        }
        Ok(())
    }

    pub(super) fn confirm(&self) -> Result<(), String> {
        if let Some(control) = &self.control {
            for entry in
                self.inserted.lock().map_err(|error| error.to_string())?.iter_mut().filter(|entry| !entry.confirmed)
            {
                if !control.confirm(&entry.followup.work_item_id)? {
                    return Err("Could not confirm subagent follow-up delivery".into());
                }
                entry.confirmed = true;
            }
        }
        Ok(())
    }
}
