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

impl From<persistence::SessionMemorySource> for super::types::SessionMemorySource {
    fn from(source: persistence::SessionMemorySource) -> Self {
        match source {
            persistence::SessionMemorySource::WarpTerminal => Self::WarpTerminal,
            persistence::SessionMemorySource::ClaudeCode => Self::ClaudeCode,
            persistence::SessionMemorySource::Codex => Self::Codex,
        }
    }
}

impl From<super::types::SessionMemorySource> for persistence::SessionMemorySource {
    fn from(source: super::types::SessionMemorySource) -> Self {
        match source {
            super::types::SessionMemorySource::WarpTerminal => Self::WarpTerminal,
            super::types::SessionMemorySource::ClaudeCode => Self::ClaudeCode,
            super::types::SessionMemorySource::Codex => Self::Codex,
        }
    }
}

impl From<persistence::SessionMemoryKind> for super::types::SessionMemoryKind {
    fn from(kind: persistence::SessionMemoryKind) -> Self {
        match kind {
            persistence::SessionMemoryKind::Terminal => Self::Terminal,
            persistence::SessionMemoryKind::AgentChat => Self::AgentChat,
        }
    }
}

impl From<super::types::SessionMemoryKind> for persistence::SessionMemoryKind {
    fn from(kind: super::types::SessionMemoryKind) -> Self {
        match kind {
            super::types::SessionMemoryKind::Terminal => Self::Terminal,
            super::types::SessionMemoryKind::AgentChat => Self::AgentChat,
        }
    }
}

impl From<persistence::SessionMemoryStatus> for SessionMemoryStatus {
    fn from(status: persistence::SessionMemoryStatus) -> Self {
        match status {
            persistence::SessionMemoryStatus::Live => Self::Live,
            persistence::SessionMemoryStatus::Blocked => Self::Blocked,
            persistence::SessionMemoryStatus::Success => Self::Success,
            persistence::SessionMemoryStatus::UserClosed => Self::UserClosed,
            persistence::SessionMemoryStatus::Interrupted => Self::Interrupted,
            persistence::SessionMemoryStatus::Stale => Self::Stale,
            persistence::SessionMemoryStatus::Unknown => Self::Unknown,
        }
    }
}

impl From<SessionMemoryStatus> for persistence::SessionMemoryStatus {
    fn from(status: SessionMemoryStatus) -> Self {
        match status {
            SessionMemoryStatus::Live => Self::Live,
            SessionMemoryStatus::Blocked => Self::Blocked,
            SessionMemoryStatus::Success => Self::Success,
            SessionMemoryStatus::UserClosed => Self::UserClosed,
            SessionMemoryStatus::Interrupted => Self::Interrupted,
            SessionMemoryStatus::Stale => Self::Stale,
            SessionMemoryStatus::Unknown => Self::Unknown,
        }
    }
}

impl From<persistence::AgentPermissionMode> for super::types::AgentPermissionMode {
    fn from(permission_mode: persistence::AgentPermissionMode) -> Self {
        match permission_mode {
            persistence::AgentPermissionMode::Normal => Self::Normal,
            persistence::AgentPermissionMode::Dangerous => Self::Dangerous,
            persistence::AgentPermissionMode::Unknown => Self::Unknown,
        }
    }
}

impl From<super::types::AgentPermissionMode> for persistence::AgentPermissionMode {
    fn from(permission_mode: super::types::AgentPermissionMode) -> Self {
        match permission_mode {
            super::types::AgentPermissionMode::Normal => Self::Normal,
            super::types::AgentPermissionMode::Dangerous => Self::Dangerous,
            super::types::AgentPermissionMode::Unknown => Self::Unknown,
        }
    }
}

impl From<persistence::SessionMemoryRecord> for SessionMemoryRecord {
    fn from(record: persistence::SessionMemoryRecord) -> Self {
        Self {
            id: record.id,
            source: record.source.into(),
            kind: record.kind.into(),
            status: record.status.into(),
            title: record.title,
            summary: record.summary,
            cwd: record.cwd,
            project: record.project,
            native_session_id: record.native_session_id,
            transcript_path: record.transcript_path,
            terminal_pane_uuid: record.terminal_pane_uuid,
            app_window_fingerprint: record.app_window_fingerprint,
            app_tab_fingerprint: record.app_tab_fingerprint,
            last_command: record.last_command,
            last_exit_code: record.last_exit_code,
            launch_argv: record.launch_argv,
            permission_mode: record.permission_mode.into(),
            last_seen_at: record.last_seen_at,
            started_at: record.started_at,
            completed_at: record.completed_at,
            closed_intentionally_at: record.closed_intentionally_at,
            restore_payload: record.restore_payload,
        }
    }
}

impl From<SessionMemoryRecord> for persistence::SessionMemoryRecord {
    fn from(record: SessionMemoryRecord) -> Self {
        Self {
            id: record.id,
            source: record.source.into(),
            kind: record.kind.into(),
            status: record.status.into(),
            title: record.title,
            summary: record.summary,
            cwd: record.cwd,
            project: record.project,
            native_session_id: record.native_session_id,
            transcript_path: record.transcript_path,
            terminal_pane_uuid: record.terminal_pane_uuid,
            app_window_fingerprint: record.app_window_fingerprint,
            app_tab_fingerprint: record.app_tab_fingerprint,
            last_command: record.last_command,
            last_exit_code: record.last_exit_code,
            launch_argv: record.launch_argv,
            permission_mode: record.permission_mode.into(),
            last_seen_at: record.last_seen_at,
            started_at: record.started_at,
            completed_at: record.completed_at,
            closed_intentionally_at: record.closed_intentionally_at,
            restore_payload: record.restore_payload,
        }
    }
}
