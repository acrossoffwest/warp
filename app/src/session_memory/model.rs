use std::sync::mpsc::SyncSender;
use std::sync::Arc;

use warpui::{Entity, SingletonEntity};

use crate::persistence::{self, ModelEvent};

use super::types::{SessionMemoryRecord, SessionMemoryStatus};

pub type SessionMemoryEventSink = Arc<dyn Fn(SessionMemoryModelEvent) + Send + Sync + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionMemoryModelEvent {
    UpsertRecord { record: SessionMemoryRecord },
    DeleteRecord { id: String },
}

pub struct SessionMemoryModel {
    records: Vec<SessionMemoryRecord>,
    event_sink: Option<SessionMemoryEventSink>,
}

impl SessionMemoryModel {
    pub fn new(
        mut records: Vec<SessionMemoryRecord>,
        event_sink: Option<SessionMemoryEventSink>,
    ) -> Self {
        for record in &mut records {
            record.status = record
                .status
                .classify_startup(record.closed_intentionally_at);
        }

        Self {
            records,
            event_sink,
        }
    }

    pub fn from_persisted_records(
        records: Vec<persistence::SessionMemoryRecord>,
        event_sink: Option<SessionMemoryEventSink>,
    ) -> Self {
        Self::new(
            records.into_iter().map(SessionMemoryRecord::from).collect(),
            event_sink,
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

    pub fn filtered_records(&self, query: &str) -> Vec<SessionMemoryRecord> {
        self.records
            .iter()
            .filter(|record| record.matches_query(query))
            .cloned()
            .collect()
    }

    pub fn upsert(&mut self, record: SessionMemoryRecord) {
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

    pub fn delete(&mut self, id: &str) {
        self.records.retain(|record| record.id != id);

        if let Some(event_sink) = &self.event_sink {
            event_sink(SessionMemoryModelEvent::DeleteRecord { id: id.to_string() });
        }
    }
}

impl Entity for SessionMemoryModel {
    type Event = SessionMemoryModelEvent;
}

impl SingletonEntity for SessionMemoryModel {}
