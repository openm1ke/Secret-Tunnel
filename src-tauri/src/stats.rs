//! History of tunnel sessions and the traffic each application sent through
//! them, kept in `sessions.json` next to the configuration. Only names, times
//! and byte counts are stored; no addresses or contents.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const MAX_SESSIONS: usize = 1000;
/// Seconds between writes of a running session, so a crash loses little.
const SAVE_EVERY: u64 = 30;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct AppUsage {
    pub name: String,
    pub rx: u64,
    pub tx: u64,
    pub flows: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct SessionRecord {
    pub id: String,
    pub started_at: u64,
    /// Last time the session was seen running; its end once it stopped.
    pub ended_at: u64,
    pub server: String,
    pub route: Vec<String>,
    pub country: String,
    pub rx: u64,
    pub tx: u64,
    pub apps: Vec<AppUsage>,
    pub active: bool,
}

#[derive(Default)]
pub struct SessionStore {
    path: Option<PathBuf>,
    sessions: Vec<SessionRecord>,
    saved_at: u64,
}

pub struct Observation<'a> {
    pub id: &'a str,
    pub started_at: u64,
    pub server: &'a str,
    pub route: &'a [String],
    pub country: &'a str,
    pub rx: u64,
    pub tx: u64,
    pub apps: Vec<AppUsage>,
}

impl SessionStore {
    /// A store backed by `path`. A session left active by a crash is closed at
    /// the last time it was seen.
    pub fn load(path: PathBuf) -> Self {
        let mut sessions: Vec<SessionRecord> = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        for session in &mut sessions {
            session.active = false;
        }
        Self { path: Some(path), sessions, saved_at: 0 }
    }

    /// Record the running session's totals; a new id closes the previous one.
    pub fn observe(&mut self, seen: Observation<'_>, now: u64) {
        let current = self.sessions.last_mut().filter(|session| session.active && session.id == seen.id);
        let session = match current {
            Some(session) => session,
            None => {
                self.finish(now);
                self.sessions.push(SessionRecord {
                    id: seen.id.to_owned(),
                    started_at: seen.started_at,
                    active: true,
                    ..Default::default()
                });
                if self.sessions.len() > MAX_SESSIONS {
                    let overflow = self.sessions.len() - MAX_SESSIONS;
                    self.sessions.drain(..overflow);
                }
                self.saved_at = 0;
                self.sessions.last_mut().unwrap()
            }
        };
        session.ended_at = now;
        session.server = seen.server.to_owned();
        session.route = seen.route.to_vec();
        if !seen.country.is_empty() {
            session.country = seen.country.to_owned();
        }
        session.rx = seen.rx;
        session.tx = seen.tx;
        let mut apps = seen.apps;
        apps.retain(|app| app.rx + app.tx > 0 || app.flows > 0);
        apps.sort_by(|a, b| (b.rx + b.tx).cmp(&(a.rx + a.tx)));
        session.apps = apps;
        if now.saturating_sub(self.saved_at) >= SAVE_EVERY {
            self.save(now);
        }
    }

    /// Close the running session, if any.
    pub fn finish(&mut self, now: u64) {
        if let Some(session) = self.sessions.last_mut().filter(|session| session.active) {
            session.active = false;
            session.ended_at = session.ended_at.max(session.started_at).min(now.max(session.started_at));
            self.save(now);
        }
    }

    pub fn list(&self) -> Vec<SessionRecord> {
        self.sessions.clone()
    }

    /// Forget finished sessions; a running one keeps counting.
    pub fn clear(&mut self, now: u64) {
        self.sessions.retain(|session| session.active);
        self.save(now);
    }

    fn save(&mut self, now: u64) {
        self.saved_at = now;
        let Some(path) = &self.path else { return };
        let Ok(bytes) = serde_json::to_vec(&self.sessions) else { return };
        let temporary = path.with_extension("json.tmp");
        if std::fs::write(&temporary, bytes).is_ok() {
            let _ = std::fs::rename(&temporary, path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen<'a>(id: &'a str, route: &'a [String], rx: u64, apps: Vec<AppUsage>) -> Observation<'a> {
        Observation { id, started_at: 100, server: "France", route, country: "FR", rx, tx: rx / 2, apps }
    }

    #[test]
    fn sessions_are_updated_closed_and_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("secret-tunnel-stats-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sessions.json");
        let route = vec!["RU".to_owned(), "France".to_owned()];
        let mut store = SessionStore::load(path.clone());
        let telegram = AppUsage { name: "Telegram".into(), rx: 10, tx: 5, flows: 2 };
        let idle = AppUsage { name: "Idle".into(), ..Default::default() };
        store.observe(seen("a", &route, 100, vec![idle.clone(), telegram.clone()]), 110);
        store.observe(seen("a", &route, 300, vec![telegram.clone()]), 140);
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list()[0].rx, 300);
        assert_eq!(store.list()[0].ended_at, 140);
        // A new session closes the previous one.
        store.observe(seen("b", &route, 50, vec![]), 200);
        let list = store.list();
        assert_eq!(list.len(), 2);
        assert!(!list[0].active && list[1].active);
        // The running session is saved; a restart closes it.
        let reloaded = SessionStore::load(path.clone()).list();
        assert_eq!(reloaded.len(), 2);
        assert!(reloaded.iter().all(|session| !session.active));
        assert_eq!(reloaded[0].apps, vec![telegram]);
        // Clearing keeps only a running session.
        store.clear(210);
        assert_eq!(store.list().len(), 1);
        store.finish(220);
        store.clear(230);
        assert!(SessionStore::load(path).list().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
