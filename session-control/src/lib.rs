//! Desired session state, independent of a UI poll or network operation.
pub mod storage;
use std::sync::Mutex;
use std::time::{Duration, Instant};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Stopped,
    Starting,
    Running,
    Reconnecting,
    Stopping,
    Failed,
}
impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Reconnecting => "reconnecting",
            Self::Stopping => "stopping",
            Self::Failed => "failed",
        }
    }
}
#[derive(Clone)]
pub struct Snapshot<P> {
    pub generation: u64,
    pub plan: Option<P>,
    pub phase: Phase,
    pub error: String,
}
struct State<P> {
    snapshot: Snapshot<P>,
    in_flight: bool,
    retry_at: Instant,
    failures: u32,
}
pub struct Controller<P> {
    state: Mutex<State<P>>,
}
impl<P: Clone> Default for Controller<P> {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                snapshot: Snapshot {
                    generation: 0,
                    plan: None,
                    phase: Phase::Stopped,
                    error: String::new(),
                },
                in_flight: false,
                retry_at: Instant::now(),
                failures: 0,
            }),
        }
    }
}
impl<P: Clone> Controller<P> {
    pub fn snapshot(&self) -> Snapshot<P> {
        self.state.lock().unwrap().snapshot.clone()
    }
    pub fn request(&self, plan: P) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.snapshot.plan.is_some() {
            return false;
        }
        state.snapshot.generation += 1;
        state.snapshot.plan = Some(plan);
        state.snapshot.phase = Phase::Starting;
        state.snapshot.error.clear();
        state.in_flight = false;
        state.failures = 0;
        state.retry_at = Instant::now();
        true
    }
    pub fn cancel(&self) {
        let mut state = self.state.lock().unwrap();
        state.snapshot.generation += 1;
        state.snapshot.plan = None;
        state.snapshot.phase = Phase::Stopping;
        state.snapshot.error.clear();
        state.in_flight = false;
    }
    pub fn stopped(&self) {
        let mut state = self.state.lock().unwrap();
        if state.snapshot.plan.is_none() && state.snapshot.phase == Phase::Stopping {
            state.snapshot.phase = Phase::Stopped;
        }
    }
    pub fn current(&self, generation: u64) -> bool {
        let state = self.state.lock().unwrap();
        state.snapshot.generation == generation && state.snapshot.plan.is_some()
    }
    pub fn attempt(&self, now: Instant) -> Option<Snapshot<P>> {
        let mut state = self.state.lock().unwrap();
        if state.in_flight
            || state.snapshot.plan.is_none()
            || !matches!(state.snapshot.phase, Phase::Starting | Phase::Reconnecting)
            || now < state.retry_at
        {
            return None;
        }
        state.in_flight = true;
        Some(state.snapshot.clone())
    }
    /// Install the ready runtime atomically against Stop. The closure must not block.
    pub fn publish(&self, generation: u64, install: impl FnOnce()) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.snapshot.generation != generation || state.snapshot.plan.is_none() {
            return false;
        }
        install();
        state.in_flight = false;
        state.snapshot.phase = Phase::Running;
        state.snapshot.error.clear();
        state.failures = 0;
        true
    }
    pub fn failed(&self, generation: u64, error: String, retry: Option<Duration>, now: Instant) {
        let mut state = self.state.lock().unwrap();
        if state.snapshot.generation != generation || state.snapshot.plan.is_none() {
            return;
        }
        state.in_flight = false;
        state.snapshot.error = error;
        state.failures = state.failures.saturating_add(1);
        if let Some(base) = retry {
            // Bounded backoff + deterministic small jitter; never a blocking sleep.
            let base = base.as_millis().min(30000) as u64;
            let delay =
                (base.saturating_mul(1u64 << state.failures.min(6).saturating_sub(1))).min(30000);
            let jitter = (generation.wrapping_mul(37) + state.failures as u64 * 17) % 251;
            state.retry_at = now + Duration::from_millis(delay + jitter);
            state.snapshot.phase = Phase::Reconnecting;
        } else {
            state.snapshot.plan = None;
            state.snapshot.phase = Phase::Failed;
        }
    }
}
pub fn retryable(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    ![
        "authentication failed",
        "public key authentication",
        "permission denied",
        "unknown server key",
        "rejected the server host key",
        "host key verification failed",
        "stmux003 server agent required",
        "cannot load ssh key",
        "ssh key file not found",
        "encrypted private key",
        "invalid",
        "already occupied",
        "cannot bind",
    ]
    .iter()
    .any(|word| error.contains(word))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stop_cancels_backoff_and_stale_completion() {
        let control = Controller::default();
        assert!(control.request("original"));
        let attempt = control.attempt(Instant::now()).unwrap();
        control.failed(
            attempt.generation,
            "network".into(),
            Some(Duration::from_secs(1)),
            Instant::now(),
        );
        control.cancel();
        assert!(control
            .attempt(Instant::now() + Duration::from_secs(90))
            .is_none());
        assert!(!control.publish(attempt.generation, || panic!("Stale runtime installed")));
        control.failed(
            attempt.generation,
            "late error".into(),
            None,
            Instant::now(),
        );
        control.stopped();
        assert_eq!(control.snapshot().phase, Phase::Stopped);
        assert!(control.request("new"));
        assert!(!control.publish(attempt.generation, || panic!(
            "Old generation replaced a new session"
        )));
    }
    #[test]
    fn retries_continue_and_keep_the_original_profile_without_polling() {
        let control = Controller::default();
        assert!(control.request(String::from("profile-a")));
        assert!(!control.request(String::from("profile-b")));
        let start = Instant::now();
        for n in 0..8 {
            let attempt = control
                .attempt(start + Duration::from_secs(n * 60))
                .unwrap();
            assert_eq!(attempt.plan.as_deref(), Some("profile-a"));
            control.failed(
                attempt.generation,
                "network".into(),
                Some(Duration::from_secs(1)),
                start + Duration::from_secs(n * 60),
            );
        }
        let attempt = control.attempt(start + Duration::from_secs(600)).unwrap();
        let mut installed = false;
        assert!(control.publish(attempt.generation, || installed = true));
        assert!(installed);
        assert_eq!(control.snapshot().phase, Phase::Running);
    }
    #[test]
    fn terminal_errors_do_not_retry_until_an_explicit_start() {
        let control = Controller::default();
        control.request(());
        let attempt = control.attempt(Instant::now()).unwrap();
        control.failed(
            attempt.generation,
            "key changed".into(),
            None,
            Instant::now(),
        );
        assert_eq!(control.snapshot().phase, Phase::Failed);
        assert!(control
            .attempt(Instant::now() + Duration::from_secs(90))
            .is_none());
        assert!(control.request(()));
        assert!(!retryable("Embedded SSH public key authentication failed"));
        assert!(!retryable("STMUX003 server agent required: old agent"));
        assert!(!retryable("Host key verification failed."));
        assert!(retryable("Connection reset by peer"));
    }
}
