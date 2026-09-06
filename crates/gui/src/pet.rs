use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use kernel::event::StopReason;
use serde::Serialize;

const DEFAULT_SESSION_TITLE: &str = "Untitled session";
const MAX_SEEN_EVENTS: usize = 2_048;
pub const PET_IDLE_TIMEOUT: Duration = Duration::from_mins(1);
pub const PET_NOTICE_DURATION: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PetConnectionStatus {
    Connected,
    Disconnected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PetRequest {
    Permission {
        req_id: String,
        session_id: String,
        title: String,
    },
    AskUser {
        req_id: String,
        session_id: String,
        title: String,
    },
}

impl PetRequest {
    fn req_id(&self) -> &str {
        match self {
            Self::Permission { req_id, .. } | Self::AskUser { req_id, .. } => req_id,
        }
    }

    fn session_id(&self) -> &str {
        match self {
            Self::Permission { session_id, .. } | Self::AskUser { session_id, .. } => session_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PetNoticeKind {
    Completed,
    Cancelled,
    Failed,
    MaxIterations,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PetNotice {
    pub event_id: String,
    pub session_id: String,
    pub title: String,
    pub kind: PetNoticeKind,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PetMood {
    Idle,
    Working,
    Happy,
    Curious,
    Alert,
    Worried,
    Sleepy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PetSnapshot {
    pub revision: u64,
    pub connection_status: PetConnectionStatus,
    pub running_count: usize,
    pub mood: PetMood,
    pub request: Option<PetRequest>,
    pub notice: Option<PetNotice>,
}

#[derive(Debug, Clone)]
struct PetSessionState {
    title: String,
    running: bool,
}

#[derive(Debug, Clone)]
struct TimedPetNotice {
    value: PetNotice,
    expires_at: Instant,
}

#[derive(Debug)]
pub struct PetRuntime {
    revision: u64,
    connection_status: PetConnectionStatus,
    sessions: HashMap<String, PetSessionState>,
    last_activity_at: Instant,
    requests: Vec<PetRequest>,
    notice: Option<TimedPetNotice>,
    seen_events: HashSet<String>,
    seen_event_order: VecDeque<String>,
}

impl Default for PetRuntime {
    fn default() -> Self {
        Self::new(Instant::now())
    }
}

impl PetRuntime {
    pub fn new(now: Instant) -> Self {
        Self {
            revision: 0,
            connection_status: PetConnectionStatus::Connected,
            sessions: HashMap::new(),
            last_activity_at: now,
            requests: Vec::new(),
            notice: None,
            seen_events: HashSet::new(),
            seen_event_order: VecDeque::new(),
        }
    }

    pub fn snapshot(&self, now: Instant) -> PetSnapshot {
        let running_count = self.running_count();
        let request = self
            .requests
            .iter()
            .find(|request| matches!(request, PetRequest::Permission { .. }))
            .or_else(|| self.requests.first())
            .cloned();
        let notice = self.active_notice(now).cloned();
        let mood = self.mood(now, running_count, request.as_ref(), notice.as_ref());
        PetSnapshot {
            revision: self.revision,
            connection_status: self.connection_status,
            running_count,
            mood,
            request,
            notice,
        }
    }

    pub fn record_activity(&mut self, now: Instant) {
        self.last_activity_at = now;
        if self.connection_status == PetConnectionStatus::Disconnected {
            self.connection_status = PetConnectionStatus::Connected;
        }
        self.bump_revision();
    }

    pub fn expire(&mut self, now: Instant) -> bool {
        let expired = self
            .notice
            .as_ref()
            .is_some_and(|notice| notice.expires_at <= now);
        if expired {
            self.notice = None;
            self.bump_revision();
        }
        expired
    }

    fn mood(
        &self,
        now: Instant,
        running_count: usize,
        request: Option<&PetRequest>,
        notice: Option<&PetNotice>,
    ) -> PetMood {
        match request {
            Some(PetRequest::Permission { .. }) => return PetMood::Alert,
            Some(PetRequest::AskUser { .. }) => return PetMood::Curious,
            None => {}
        }
        if let Some(notice) = notice {
            return match notice.kind {
                PetNoticeKind::Completed => PetMood::Happy,
                PetNoticeKind::Cancelled | PetNoticeKind::Failed | PetNoticeKind::MaxIterations => {
                    PetMood::Worried
                }
            };
        }
        if self.connection_status == PetConnectionStatus::Disconnected {
            return PetMood::Sleepy;
        }
        if running_count > 0 {
            return PetMood::Working;
        }
        if now.duration_since(self.last_activity_at) >= PET_IDLE_TIMEOUT {
            return PetMood::Sleepy;
        }
        PetMood::Idle
    }

    fn active_notice(&self, now: Instant) -> Option<&PetNotice> {
        self.notice
            .as_ref()
            .filter(|notice| notice.expires_at > now)
            .map(|notice| &notice.value)
    }

    fn running_count(&self) -> usize {
        self.sessions
            .values()
            .filter(|session| session.running)
            .count()
    }

    pub fn set_connection_status(&mut self, status: PetConnectionStatus) -> bool {
        if self.connection_status == status {
            return false;
        }
        self.connection_status = status;
        self.bump_revision();
        true
    }

    #[allow(dead_code)]
    pub fn set_session(&mut self, session_id: &str, title: Option<&str>, running: bool) -> bool {
        let title = normalized_title(title);
        match self.sessions.get_mut(session_id) {
            Some(session) if session.title == title && session.running == running => false,
            Some(session) => {
                session.title = title;
                session.running = running;
                self.bump_revision();
                true
            }
            None => {
                self.sessions
                    .insert(session_id.to_string(), PetSessionState { title, running });
                self.bump_revision();
                true
            }
        }
    }

    #[allow(dead_code)]
    pub fn remove_session(&mut self, session_id: &str) -> bool {
        let removed = self.sessions.remove(session_id).is_some();
        let requests_removed = self.requests.iter().any(|request| match request {
            PetRequest::Permission { session_id: id, .. }
            | PetRequest::AskUser { session_id: id, .. } => id == session_id,
        });
        if removed || requests_removed {
            self.requests.retain(|request| match request {
                PetRequest::Permission { session_id: id, .. }
                | PetRequest::AskUser { session_id: id, .. } => id != session_id,
            });
            self.bump_revision();
            return true;
        }
        false
    }

    pub fn reconcile_running_sessions<'a>(
        &mut self,
        sessions: impl IntoIterator<Item = (&'a str, Option<&'a str>, bool)>,
    ) -> bool {
        let old_running_count = self
            .sessions
            .values()
            .filter(|session| session.running)
            .count();
        for session in self.sessions.values_mut() {
            session.running = false;
        }
        let mut changed = false;
        let session_ids: HashSet<String> = sessions
            .into_iter()
            .map(|(session_id, title, running)| {
                let title = normalized_title(title);
                match self.sessions.get_mut(session_id) {
                    Some(session) => {
                        changed |= session.title != title || session.running != running;
                        session.title = title;
                        session.running = running;
                    }
                    None => {
                        self.sessions
                            .insert(session_id.to_string(), PetSessionState { title, running });
                        changed = true;
                    }
                }
                session_id.to_string()
            })
            .collect();
        let stale_ids: Vec<String> = self
            .sessions
            .keys()
            .filter(|session_id| !session_ids.contains(*session_id))
            .cloned()
            .collect();
        for session_id in stale_ids {
            self.sessions.remove(&session_id);
            self.requests.retain(|request| match request {
                PetRequest::Permission { session_id: id, .. }
                | PetRequest::AskUser { session_id: id, .. } => id != &session_id,
            });
            changed = true;
        }
        let new_running_count = self
            .sessions
            .values()
            .filter(|session| session.running)
            .count();
        changed |= old_running_count != new_running_count;
        if changed {
            self.bump_revision();
        }
        changed
    }

    pub fn update_session_running(&mut self, session_id: &str, running: bool) -> bool {
        let changed = self.set_running_without_revision(session_id, running);
        if changed {
            self.bump_revision();
        }
        changed
    }

    pub fn clear_session_requests(&mut self, session_id: &str) -> bool {
        let old_len = self.requests.len();
        self.requests
            .retain(|request| request.session_id() != session_id);
        let changed = self.requests.len() != old_len;
        if changed {
            self.bump_revision();
        }
        changed
    }

    pub fn update_session_title(&mut self, session_id: &str, title: Option<&str>) -> bool {
        let title = normalized_title(title);
        let session = self
            .sessions
            .entry(session_id.to_string())
            .or_insert_with(|| PetSessionState {
                title: DEFAULT_SESSION_TITLE.to_string(),
                running: false,
            });
        if session.title == title {
            return false;
        }
        session.title = title;
        self.bump_revision();
        true
    }

    pub fn process_activity(
        &mut self,
        source_session_id: &str,
        event_id: &str,
        activity: &kernel::notification::AgentActivity,
        now: Instant,
    ) -> bool {
        use kernel::notification::AgentActivity;

        let event_key = format!("{source_session_id}:{event_id}");
        if !self.remember_event(event_key) {
            return false;
        }
        let title = self.sessions.get(source_session_id).map_or_else(
            || DEFAULT_SESSION_TITLE.to_string(),
            |session| session.title.clone(),
        );

        let changed = match activity {
            AgentActivity::PermissionRequested {
                req_id,
                target_session_id,
            } => {
                self.requests.retain(|request| request.req_id() != req_id);
                self.requests.push(PetRequest::Permission {
                    req_id: req_id.clone(),
                    session_id: target_session_id.clone(),
                    title,
                });
                true
            }
            AgentActivity::AskUserRequested {
                req_id,
                target_session_id,
            } => {
                self.requests.retain(|request| request.req_id() != req_id);
                self.requests.push(PetRequest::AskUser {
                    req_id: req_id.clone(),
                    session_id: target_session_id.clone(),
                    title,
                });
                true
            }
            AgentActivity::RequestResolved { req_id } => {
                let old_len = self.requests.len();
                self.requests.retain(|request| request.req_id() != req_id);
                self.requests.len() != old_len
            }
            AgentActivity::Started => self.set_running_without_revision(source_session_id, true),
            AgentActivity::Stopped { reason } => {
                self.set_running_without_revision(source_session_id, false);
                self.requests
                    .retain(|request| request.session_id() != source_session_id);
                let (kind, message) = notice_reason(reason);
                self.notice = Some(TimedPetNotice {
                    value: PetNotice {
                        event_id: event_id.to_string(),
                        session_id: source_session_id.to_string(),
                        title,
                        kind,
                        message,
                    },
                    expires_at: now + PET_NOTICE_DURATION,
                });
                true
            }
        };
        if changed {
            self.bump_revision();
        }
        changed
    }

    fn set_running_without_revision(&mut self, session_id: &str, running: bool) -> bool {
        let session = self
            .sessions
            .entry(session_id.to_string())
            .or_insert_with(|| PetSessionState {
                title: DEFAULT_SESSION_TITLE.to_string(),
                running: false,
            });
        if session.running == running {
            return false;
        }
        session.running = running;
        true
    }

    fn remember_event(&mut self, event_key: String) -> bool {
        if !self.seen_events.insert(event_key.clone()) {
            return false;
        }
        self.seen_event_order.push_back(event_key);
        while self.seen_event_order.len() > MAX_SEEN_EVENTS {
            if let Some(oldest) = self.seen_event_order.pop_front() {
                self.seen_events.remove(&oldest);
            }
        }
        true
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }
}

fn normalized_title(title: Option<&str>) -> String {
    title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or(DEFAULT_SESSION_TITLE)
        .to_string()
}

fn notice_reason(reason: &StopReason) -> (PetNoticeKind, Option<String>) {
    match reason {
        StopReason::Completed { finish_reason } => (
            PetNoticeKind::Completed,
            finish_reason.map(|reason| format!("{reason:?}")),
        ),
        StopReason::Cancelled { operation } => (PetNoticeKind::Cancelled, operation.clone()),
        // daemon 关停打断：桌宠通知与用户取消同显示（"已停止"）。
        StopReason::Shutdown => (
            PetNoticeKind::Cancelled,
            Some("daemon shutdown".to_string()),
        ),
        StopReason::Failed { error } => (PetNoticeKind::Failed, Some(error.clone())),
        StopReason::MaxIterations { reached } => (
            PetNoticeKind::MaxIterations,
            Some(format!("Reached {reached} iterations")),
        ),
    }
}
