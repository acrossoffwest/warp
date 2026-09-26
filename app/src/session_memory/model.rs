use std::collections::HashSet;
use std::sync::Arc;
use std::sync::mpsc::SyncSender;

use warpui::{Entity, ModelContext, SingletonEntity};

use crate::persistence::{self, ModelEvent};

use super::types::{SessionMemoryRecord, SessionMemoryRunState, SessionMemoryStatus};

pub type SessionMemoryEventSink = Arc<dyn Fn(SessionMemoryModelEvent) + Send + Sync + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionMemoryModelEvent {
    UpsertRecord {
        record: SessionMemoryRecord,
    },
    DeleteRecord {
        id: String,
    },
    MarkRecordsOffered {
        ids: Vec<String>,
        app_run_id: String,
        offered_run_id: String,
    },
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
        }

        Self {
            records,
            event_sink,
            run_state,
        }
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
                    SessionMemoryModelEvent::MarkRecordsOffered {
                        ids,
                        app_run_id,
                        offered_run_id,
                    } => ModelEvent::MarkSessionMemoryRecordsOffered {
                        ids,
                        app_run_id,
                        offered_run_id,
                    },
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

    pub fn startup_restore_candidates(&self) -> Vec<SessionMemoryRecord> {
        let Some(previous_run_id) = self.run_state.previous_run_id.as_deref() else {
            return Vec::new();
        };

        let mut candidates: Vec<SessionMemoryRecord> = Vec::new();
        for record in self.records.iter().filter(|record| {
            record.is_agent()
                && record.app_run_id.as_deref() == Some(previous_run_id)
                && record.completed_at.is_none()
                && record.closed_intentionally_at.is_none()
                && record.recovery_offered_run_id.is_none()
        }) {
            let duplicate = record
                .native_session_id
                .as_ref()
                .and_then(|native_session_id| {
                    candidates.iter().position(|candidate| {
                        candidate.native_session_id.as_ref() == Some(native_session_id)
                    })
                });
            match duplicate {
                Some(index) if candidates[index].last_seen_at >= record.last_seen_at => {}
                Some(index) => candidates[index] = record.clone(),
                None => candidates.push(record.clone()),
            }
        }
        candidates
    }

    pub fn previous_run_native_session_ids(&self) -> HashSet<String> {
        let Some(previous_run_id) = self.run_state.previous_run_id.as_deref() else {
            return HashSet::new();
        };
        self.records
            .iter()
            .filter(|record| record.app_run_id.as_deref() == Some(previous_run_id))
            .filter_map(|record| record.native_session_id.clone())
            .collect()
    }

    pub fn filtered_records(&self, query: &str) -> Vec<SessionMemoryRecord> {
        self.records
            .iter()
            .filter(|record| record.matches_query(query))
            .cloned()
            .collect()
    }

    pub fn upsert(&mut self, record: SessionMemoryRecord) {
        let mut record = record;
        if record.app_run_id.is_none() {
            record.app_run_id = Some(self.run_state.current_run_id.clone());
        }

        if let Some(existing) = self
            .records
            .iter_mut()
            .find(|existing| existing.id == record.id)
        {
            *existing = record.clone();
        } else {
            self.records.push(record.clone());
        }

        if let Some(event_sink) = &self.event_sink {
            event_sink(SessionMemoryModelEvent::UpsertRecord { record });
        }
    }

    pub fn upsert_and_notify(&mut self, record: SessionMemoryRecord, ctx: &mut ModelContext<Self>) {
        self.upsert(record.clone());
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

    pub fn mark_startup_recovery_offered(&mut self, ids: &[String]) {
        let Some(previous_run_id) = self.run_state.previous_run_id.clone() else {
            return;
        };
        let offered_run_id = self.run_state.current_run_id.clone();
        for record in &mut self.records {
            if ids.iter().any(|id| id == &record.id)
                && record.app_run_id.as_deref() == Some(previous_run_id.as_str())
            {
                record.recovery_offered_run_id = Some(offered_run_id.clone());
            }
        }

        if let Some(event_sink) = &self.event_sink {
            event_sink(SessionMemoryModelEvent::MarkRecordsOffered {
                ids: ids.to_vec(),
                app_run_id: previous_run_id,
                offered_run_id,
            });
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
