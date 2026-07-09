use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use warpui::{Entity, ModelContext, SingletonEntity};

use crate::persistence::{self, ModelEvent};

use super::restore::terminal_restore_plan;
use super::types::{
    SessionMemoryRecord, SessionMemoryRunState, SessionMemorySource, SessionMemoryStatus,
};

pub type SessionMemoryEventSink = Arc<dyn Fn(SessionMemoryModelEvent) + Send + Sync + 'static>;
const RECENT_AGENT_STARTUP_RESTORE_SECONDS: u64 = 30 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionMemoryModelEvent {
    UpsertRecord { record: SessionMemoryRecord },
    DeleteRecord { id: String },
}

pub struct SessionMemoryModel {
    records: Vec<SessionMemoryRecord>,
    event_sink: Option<SessionMemoryEventSink>,
    run_state: SessionMemoryRunState,
}

impl SessionMemoryModel {
    pub fn new(
        records: Vec<SessionMemoryRecord>,
        event_sink: Option<SessionMemoryEventSink>,
    ) -> Self {
        Self::new_with_run_state(records, event_sink, SessionMemoryRunState::test_default())
    }

    pub fn new_with_run_state(
        mut records: Vec<SessionMemoryRecord>,
        event_sink: Option<SessionMemoryEventSink>,
        run_state: SessionMemoryRunState,
    ) -> Self {
        for record in &mut records {
            record.status = record
                .status
                .classify_startup(record.closed_intentionally_at);
            record.normalize_terminal_agent_command();
        }

        let mut model = Self {
            records,
            event_sink,
            run_state,
        };
        model.dedupe_native_session_duplicates();
        model
    }

    /// An agent chat is identified by its native session id, not by the pane
    /// that happened to host it. Panes change across restores, so without
    /// dedupe every restore leaves a stale copy of the same chat behind.
    /// Keeps the most recently seen record per native session id.
    fn dedupe_native_session_duplicates(&mut self) {
        let mut keep: std::collections::HashMap<String, (i64, String)> =
            std::collections::HashMap::new();
        for record in &self.records {
            let Some(native_session_id) = &record.native_session_id else {
                continue;
            };
            let candidate = (record.last_seen_at, record.id.clone());
            match keep.get(native_session_id) {
                Some(best) if *best >= candidate => {}
                _ => {
                    keep.insert(native_session_id.clone(), candidate);
                }
            }
        }

        let removed = self
            .records
            .iter()
            .filter(|record| {
                record
                    .native_session_id
                    .as_ref()
                    .and_then(|native_session_id| keep.get(native_session_id))
                    .is_some_and(|(_, keep_id)| *keep_id != record.id)
            })
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        for id in removed {
            self.delete(&id);
        }
    }

    /// Removes stale records describing the same native agent session as
    /// `record` (which just got fresh data and wins regardless of timestamps).
    /// Returns the removed record ids.
    fn dedupe_native_session_duplicates_of(&mut self, record: &SessionMemoryRecord) -> Vec<String> {
        let Some(native_session_id) = &record.native_session_id else {
            return Vec::new();
        };
        let removed = self
            .records
            .iter()
            .filter(|existing| {
                existing.id != record.id
                    && existing.native_session_id.as_deref() == Some(native_session_id.as_str())
            })
            .map(|existing| existing.id.clone())
            .collect::<Vec<_>>();
        for id in &removed {
            self.delete(id);
        }
        removed
    }

    pub fn from_persisted_records(
        records: Vec<persistence::SessionMemoryRecord>,
        event_sink: Option<SessionMemoryEventSink>,
    ) -> Self {
        Self::from_persisted_records_with_run_state(
            records,
            event_sink,
            SessionMemoryRunState::test_default(),
        )
    }

    pub fn from_persisted_records_with_run_state(
        records: Vec<persistence::SessionMemoryRecord>,
        event_sink: Option<SessionMemoryEventSink>,
        run_state: SessionMemoryRunState,
    ) -> Self {
        Self::new_with_run_state(
            records.into_iter().map(SessionMemoryRecord::from).collect(),
            event_sink,
            run_state,
        )
    }

    pub fn persistence_event_sink(
        sender: Option<SyncSender<ModelEvent>>,
    ) -> Option<SessionMemoryEventSink> {
        sender.map(|sender| {
            Arc::new(move |event| {
                let model_event = match event {
                    SessionMemoryModelEvent::UpsertRecord { record } => {
                        ModelEvent::UpsertSessionMemoryRecord {
                            record: record.into(),
                        }
                    }
                    SessionMemoryModelEvent::DeleteRecord { id } => {
                        ModelEvent::DeleteSessionMemoryRecord { id }
                    }
                };

                if let Err(err) = sender.send(model_event) {
                    log::error!("Error sending session memory model event to persistence: {err:?}");
                }
            }) as SessionMemoryEventSink
        })
    }

    pub fn records(&self) -> &[SessionMemoryRecord] {
        &self.records
    }

    pub fn current_run_id(&self) -> &str {
        &self.run_state.current_run_id
    }

    pub fn interrupted_count(&self) -> usize {
        self.records
            .iter()
            .filter(|record| record.status == SessionMemoryStatus::Interrupted)
            .count()
    }

    pub fn interrupted_records(&self) -> Vec<SessionMemoryRecord> {
        self.records
            .iter()
            .filter(|record| record.status == SessionMemoryStatus::Interrupted)
            .cloned()
            .collect()
    }

    pub fn startup_auto_restore_records(&self) -> Vec<SessionMemoryRecord> {
        let previous_run_id = self
            .run_state
            .previous_run_id
            .as_deref()
            .or(self.run_state.recoverable_run_id.as_deref());
        let now = now_unix_seconds();

        self.records
            .iter()
            .filter(|record| {
                record.status == SessionMemoryStatus::Interrupted
                    && record.app_run_id.as_deref() != Some(self.run_state.current_run_id.as_str())
                    && record.recovery_offered_run_id.as_deref()
                        != Some(self.run_state.current_run_id.as_str())
                    && Self::should_auto_restore_on_startup(record)
                    && (record.app_run_id.as_deref() == previous_run_id
                        || Self::is_recent_agent_startup_restore_candidate(record, now))
            })
            .cloned()
            .collect()
    }

    fn should_auto_restore_on_startup(record: &SessionMemoryRecord) -> bool {
        match record.source {
            SessionMemorySource::ClaudeCode | SessionMemorySource::Codex => {
                // A set `completed_at` means the agent exited inside a live
                // pane — the user ended the chat, so don't resurrect it.
                record.native_session_id.is_some() && record.completed_at.is_none()
            }
            SessionMemorySource::WarpTerminal => {
                terminal_restore_plan(record, false).auto_run() == Some(true)
            }
        }
    }

    fn is_recent_agent_startup_restore_candidate(record: &SessionMemoryRecord, now: i64) -> bool {
        if !matches!(
            record.source,
            SessionMemorySource::ClaudeCode | SessionMemorySource::Codex
        ) {
            return false;
        }

        now.abs_diff(record.last_seen_at) <= RECENT_AGENT_STARTUP_RESTORE_SECONDS
    }

    pub fn filtered_records(&self, query: &str) -> Vec<SessionMemoryRecord> {
        self.records
            .iter()
            .filter(|record| record.matches_query(query))
            .cloned()
            .collect()
    }

    /// Returns the ids of stale duplicate records removed by the upsert.
    pub fn upsert(&mut self, record: SessionMemoryRecord) -> Vec<String> {
        let mut record = record;
        if record.app_run_id.is_none() {
            record.app_run_id = Some(self.run_state.current_run_id.clone());
        }
        record.normalize_terminal_agent_command();

        if let Some(existing) = self
            .records
            .iter_mut()
            .find(|existing| existing.id == record.id)
        {
            *existing = record.clone();
        } else {
            self.records.push(record.clone());
        }

        let removed = self.dedupe_native_session_duplicates_of(&record);

        if let Some(event_sink) = &self.event_sink {
            event_sink(SessionMemoryModelEvent::UpsertRecord { record });
        }
        removed
    }

    pub fn upsert_and_notify(&mut self, record: SessionMemoryRecord, ctx: &mut ModelContext<Self>) {
        let removed = self.upsert(record.clone());
        for id in removed {
            ctx.emit(SessionMemoryModelEvent::DeleteRecord { id });
        }
        ctx.emit(SessionMemoryModelEvent::UpsertRecord { record });
    }

    pub fn delete(&mut self, id: &str) {
        self.records.retain(|record| record.id != id);

        if let Some(event_sink) = &self.event_sink {
            event_sink(SessionMemoryModelEvent::DeleteRecord { id: id.to_string() });
        }
    }

    pub fn delete_and_notify(&mut self, id: &str, ctx: &mut ModelContext<Self>) {
        self.delete(id);
        ctx.emit(SessionMemoryModelEvent::DeleteRecord { id: id.to_string() });
    }

    /// True when a layout-restored tab should be skipped because every
    /// terminal pane in it was already closed intentionally by the user. The
    /// window snapshot can be stale after a crash or force-kill, while close
    /// markers are written immediately — trust the markers.
    pub fn should_suppress_restored_tab(&self, terminal_pane_uuids: &[Vec<u8>]) -> bool {
        if terminal_pane_uuids.is_empty() {
            return false;
        }
        terminal_pane_uuids.iter().all(|uuid| {
            self.records.iter().any(|record| {
                record.terminal_pane_uuid.as_deref() == Some(uuid.as_slice())
                    && record.closed_intentionally_at.is_some()
            })
        })
    }

    /// Marks the record's CLI agent session as ended by the user (the agent
    /// process exited while the pane stayed open). Such records are kept on
    /// the board but excluded from startup auto-restore.
    pub fn mark_agent_session_ended(&mut self, id: &str) {
        let Some(record) = self.records.iter_mut().find(|record| record.id == id) else {
            return;
        };
        if record.completed_at.is_some() {
            return;
        }
        record.completed_at = Some(now_unix_seconds());
        let record = record.clone();

        if let Some(event_sink) = &self.event_sink {
            event_sink(SessionMemoryModelEvent::UpsertRecord { record });
        }
    }

    pub fn mark_agent_session_ended_and_notify(&mut self, id: &str, ctx: &mut ModelContext<Self>) {
        self.mark_agent_session_ended(id);
        if let Some(record) = self.records.iter().find(|record| record.id == id) {
            ctx.emit(SessionMemoryModelEvent::UpsertRecord {
                record: record.clone(),
            });
        }
    }

    /// Marks every record bound to this native agent session as ended.
    pub fn mark_agent_session_ended_for_native_session_and_notify(
        &mut self,
        native_session_id: &str,
        ctx: &mut ModelContext<Self>,
    ) {
        let ids = self
            .records
            .iter()
            .filter(|record| {
                record.native_session_id.as_deref() == Some(native_session_id)
                    && record.completed_at.is_none()
            })
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            self.mark_agent_session_ended_and_notify(&id, ctx);
        }
    }

    pub fn mark_startup_recovery_offered(&mut self, ids: &[String]) {
        let mut changed_records = Vec::new();
        for record in &mut self.records {
            if ids.iter().any(|id| id == &record.id) {
                record.recovery_offered_run_id = Some(self.run_state.current_run_id.clone());
                changed_records.push(record.clone());
            }
        }

        if let Some(event_sink) = &self.event_sink {
            for record in changed_records {
                event_sink(SessionMemoryModelEvent::UpsertRecord { record });
            }
        }
    }

    pub fn mark_startup_recovery_offered_and_notify(
        &mut self,
        ids: &[String],
        ctx: &mut ModelContext<Self>,
    ) {
        self.mark_startup_recovery_offered(ids);
        for record in self
            .records
            .iter()
            .filter(|record| ids.iter().any(|id| id == &record.id))
            .cloned()
        {
            ctx.emit(SessionMemoryModelEvent::UpsertRecord { record });
        }
    }
}

impl Entity for SessionMemoryModel {
    type Event = SessionMemoryModelEvent;
}

impl SingletonEntity for SessionMemoryModel {}

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}
