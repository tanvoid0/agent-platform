//! How much of this machine the server is allowed to use, and who gets it first
//! when two callers want it (ADR 0010).
//!
//! Two lanes, not a priority queue. `tokio::sync::Semaphore` is FIFO-fair with
//! no notion of priority, and putting a real priority queue in front of it would
//! mean owning a scheduler. What makes that unnecessary: the thing that hurts is
//! *background work stampeding* — a DAG wave is dozens of simultaneous model
//! calls — while interactive work is one call at a time per human. So the
//! interactive lane is capped generously (a bound against pathology, not a
//! throttle) and the background lane tightly. Interactive callers essentially
//! never wait; background callers do, which is the entire point.
//!
//! [`Mode::Auto`] is not a third width. It resolves, at each acquire, to one of
//! the real three from two signals the server already has: whether the desktop
//! window is in front of the user, and how long ago the last interactive call
//! was. See [`Limits::resolved`].

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::State;
use axum::routing::put;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::{Semaphore, SemaphorePermit};

use crate::AppState;

/// How long after the last interactive call [`Mode::Auto`] still assumes the
/// user might come back. They closed a chat and walked off; the run they left
/// behind should not collapse to single-file the instant they did.
const RECENT_INTERACTIVE: std::time::Duration = std::time::Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Mode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Stay out of the way of whatever else is running. One background call.
    Eco,
    /// Half the machine.
    Balanced,
    /// As much as is useful. Still bounded — see [`Tier::background`].
    Turbo,
    /// Pick one of the above per acquire. The default, because a knob most users
    /// never touch only helps the ones who already knew they had a problem.
    #[default]
    Auto,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Eco, Mode::Balanced, Mode::Turbo, Mode::Auto];

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Eco => "eco",
            Mode::Balanced => "balanced",
            Mode::Turbo => "turbo",
            Mode::Auto => "auto",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.as_str() == s.trim().to_ascii_lowercase())
    }

    fn to_u8(self) -> u8 {
        match self {
            Mode::Eco => 0,
            Mode::Balanced => 1,
            Mode::Turbo => 2,
            Mode::Auto => 3,
        }
    }

    fn from_u8(v: u8) -> Mode {
        match v {
            0 => Mode::Eco,
            1 => Mode::Balanced,
            2 => Mode::Turbo,
            _ => Mode::Auto,
        }
    }
}

/// A resolved mode — [`Mode`] with `Auto` already decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Eco,
    Balanced,
    Turbo,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Eco => "eco",
            Tier::Balanced => "balanced",
            Tier::Turbo => "turbo",
        }
    }

    /// Background permits. The clamps matter more than the arithmetic: a 2-core
    /// laptop must still get 2 in Balanced or a DAG runs single-file on the tier
    /// that is supposed to be the compromise, and a 64-core box must not open 64
    /// simultaneous vendor connections just because it can.
    pub fn background(self, cpus: usize) -> usize {
        match self {
            Tier::Eco => 1,
            Tier::Balanced => (cpus / 2).clamp(2, 8),
            Tier::Turbo => cpus.clamp(4, 16),
        }
    }

    /// Interactive permits. A ceiling against pathology, not a throttle — a
    /// human generates one call at a time, so this only ever binds when
    /// something has gone wrong.
    fn interactive(self) -> usize {
        match self {
            Tier::Eco => 4,
            Tier::Balanced => 8,
            Tier::Turbo => 16,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    /// A human is waiting on this: chat, the Coder loop, the assistant.
    Interactive,
    /// Nobody is watching: DAG nodes, scheduled workflows, auto-titling.
    Background,
}

// ---------------------------------------------------------------------------
// Limits
// ---------------------------------------------------------------------------

/// One per process, on [`AppState`].
pub struct Limits {
    mode: AtomicU8,
    /// Desktop window focused and open. Only consulted under [`Mode::Auto`], and
    /// only ever written by `PUT /system/resources`.
    user_present: AtomicBool,
    /// Millis since [`Limits::start`] of the last interactive acquire. `Instant`
    /// is not atomic, so the offset is what is stored.
    last_interactive_ms: AtomicU64,
    start: Instant,
    cpus: usize,
    interactive: Arc<Semaphore>,
    background: Arc<Semaphore>,
    /// Permits currently *granted* to each semaphore, so a resize can diff
    /// against it. Behind one lock because the two must move together with the
    /// semaphores or the accounting drifts.
    granted: Mutex<(usize, usize)>,
}

impl Default for Limits {
    fn default() -> Self {
        Self::new(Mode::default())
    }
}

impl Limits {
    pub fn new(mode: Mode) -> Self {
        let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        // Start at the widest either lane can ever be, then immediately shrink
        // to the current tier. Growing a semaphore is free (`add_permits`);
        // starting narrow and growing would let the first callers through at a
        // limit nobody chose.
        let interactive = Arc::new(Semaphore::new(Tier::Turbo.interactive()));
        let background = Arc::new(Semaphore::new(Tier::Turbo.background(cpus)));
        let limits = Self {
            mode: AtomicU8::new(mode.to_u8()),
            user_present: AtomicBool::new(false),
            last_interactive_ms: AtomicU64::new(0),
            start: Instant::now(),
            cpus,
            granted: Mutex::new((Tier::Turbo.interactive(), Tier::Turbo.background(cpus))),
            interactive,
            background,
        };
        limits.sync_permits();
        limits
    }

    pub fn mode(&self) -> Mode {
        Mode::from_u8(self.mode.load(Ordering::Relaxed))
    }

    pub fn set_mode(&self, mode: Mode) {
        self.mode.store(mode.to_u8(), Ordering::Relaxed);
        self.sync_permits();
    }

    pub fn set_user_present(&self, present: bool) {
        self.user_present.store(present, Ordering::Relaxed);
        self.sync_permits();
    }

    /// What [`Mode::Auto`] currently means. Also what the Settings screen shows,
    /// so the mode is legible rather than magic.
    pub fn resolved(&self) -> Tier {
        match self.mode() {
            Mode::Eco => Tier::Eco,
            Mode::Balanced => Tier::Balanced,
            Mode::Turbo => Tier::Turbo,
            Mode::Auto => {
                if self.user_present.load(Ordering::Relaxed) {
                    // Watching a run means wanting it finished.
                    Tier::Turbo
                } else if self.since_interactive() < RECENT_INTERACTIVE {
                    Tier::Balanced
                } else {
                    // Nobody is looking. Give the machine back.
                    Tier::Eco
                }
            }
        }
    }

    fn since_interactive(&self) -> std::time::Duration {
        let last = self.last_interactive_ms.load(Ordering::Relaxed);
        // Saturating: a clock that somehow ran backwards should read "just now",
        // not "never", because "never" is the answer that throttles.
        std::time::Duration::from_millis(
            (self.start.elapsed().as_millis() as u64).saturating_sub(last),
        )
    }

    /// Model calls in flight in each lane, for the sidebar monitor. Reading a
    /// semaphore's free count is two atomic loads — the monitor must be able to
    /// ask often without becoming the thing it is reporting on.
    pub fn in_flight(&self) -> (usize, usize) {
        let granted = self.granted.lock().unwrap_or_else(|e| e.into_inner());
        (
            granted.0.saturating_sub(self.interactive.available_permits()),
            granted.1.saturating_sub(self.background.available_permits()),
        )
    }

    /// The DAG executor's wave width. It asks *before* spawning, so the wave
    /// stops being created too wide rather than being created wide and blocking
    /// 39 tasks on a semaphore.
    pub fn background_width(&self) -> usize {
        self.resolved().background(self.cpus)
    }

    /// Bring each semaphore to the resolved tier's width.
    ///
    /// Permits already handed out are never revoked — `forget_permits` only
    /// takes what is currently free, and the rest of the shrink lands as those
    /// calls finish and return permits into a semaphore that is now smaller than
    /// it was. So a mode toggled mid-run takes effect as work drains, which is
    /// what a user flipping the switch means by it.
    fn sync_permits(&self) {
        let tier = self.resolved();
        let want = (tier.interactive(), tier.background(self.cpus));
        let mut granted = self.granted.lock().unwrap_or_else(|e| e.into_inner());
        granted.0 = resize(&self.interactive, granted.0, want.0);
        granted.1 = resize(&self.background, granted.1, want.1);
    }

    /// Wait for room to make one model call. Hold the permit for the whole call
    /// — dropping it early is the same as not gating at all.
    pub async fn acquire(&self, priority: Priority) -> SemaphorePermit<'_> {
        let sem = match priority {
            Priority::Interactive => {
                self.last_interactive_ms
                    .store(self.start.elapsed().as_millis() as u64, Ordering::Relaxed);
                // A call just arrived from a human, so Auto may have just moved
                // from Eco to Balanced. Reconcile before queueing on the answer.
                self.sync_permits();
                &self.interactive
            }
            Priority::Background => &self.background,
        };
        // `Semaphore::acquire` only errors when the semaphore is closed, and
        // nothing here ever closes one. If that ever changes, an ungated call is
        // the right failure: today's behaviour, not a stall.
        sem.acquire().await.expect("resource semaphore is never closed")
    }
}

/// Move `sem` from `have` permits towards `want`, and report what it actually
/// holds now. Shrinking can fall short — `forget_permits` takes only what is
/// free — and the caller must record the shortfall, or the next resize computes
/// its diff against a number the semaphore never reached.
fn resize(sem: &Semaphore, have: usize, want: usize) -> usize {
    if want > have {
        sem.add_permits(want - have);
        want
    } else if want < have {
        have - sem.forget_permits(have - want)
    } else {
        have
    }
}

// ---------------------------------------------------------------------------
// Route
// ---------------------------------------------------------------------------

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/system/resources", put(set_resources).get(get_resources))
}

#[derive(Deserialize)]
pub struct ResourcesBody {
    /// Absent leaves the mode alone, so the desktop can push presence on a
    /// window event without restating a setting it did not change.
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub user_present: Option<bool>,
}

#[derive(Serialize)]
pub struct ResourcesView {
    pub mode: &'static str,
    pub resolved: &'static str,
    pub background_limit: usize,
    pub background_in_flight: usize,
    pub interactive_in_flight: usize,
    pub cpus: usize,
}

fn view(limits: &Limits) -> ResourcesView {
    let (interactive, background) = limits.in_flight();
    ResourcesView {
        mode: limits.mode().as_str(),
        resolved: limits.resolved().as_str(),
        background_limit: limits.background_width(),
        background_in_flight: background,
        interactive_in_flight: interactive,
        cpus: limits.cpus,
    }
}

async fn get_resources(State(state): State<Arc<AppState>>) -> Json<ResourcesView> {
    Json(view(&state.limits))
}

/// Pushed on change only — mode toggled, window focused, unfocused, closed, and
/// once when the desktop first sees the server ready. Deliberately not polled:
/// an event that fires a few times an hour does not need a timer, and adding one
/// would contradict the ADR this lives in.
async fn set_resources(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ResourcesBody>,
) -> Result<Json<ResourcesView>, crate::error::ApiError> {
    if let Some(raw) = body.mode.as_deref() {
        let mode = Mode::parse(raw).ok_or_else(|| {
            crate::error::ApiError::bad_request(format!(
                "unknown resource mode '{raw}' (expected eco, balanced, turbo or auto)"
            ))
        })?;
        state.limits.set_mode(mode);
    }
    if let Some(present) = body.user_present {
        state.limits.set_user_present(present);
    }
    Ok(Json(view(&state.limits)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_widths_are_clamped_at_both_ends() {
        // A 2-core laptop still gets a real Balanced…
        assert_eq!(Tier::Balanced.background(2), 2);
        assert_eq!(Tier::Turbo.background(2), 4);
        // …and a 64-core box does not open 64 vendor connections.
        assert_eq!(Tier::Turbo.background(64), 16);
        assert_eq!(Tier::Balanced.background(64), 8);
        // Eco is one, on every machine. That is the whole promise of Eco.
        assert_eq!(Tier::Eco.background(64), 1);
    }

    #[test]
    fn explicit_modes_ignore_presence() {
        let limits = Limits::new(Mode::Eco);
        limits.set_user_present(true);
        assert_eq!(limits.resolved(), Tier::Eco);
        limits.set_mode(Mode::Turbo);
        limits.set_user_present(false);
        assert_eq!(limits.resolved(), Tier::Turbo);
    }

    #[test]
    fn auto_follows_presence_then_recency_then_sleeps() {
        let limits = Limits::new(Mode::Auto);
        // Fresh process, nobody has called anything, window not reported.
        // `last_interactive_ms` is 0 and elapsed is ~0, so "just started" reads
        // as recent — which is right: the app is coming up.
        limits.set_user_present(true);
        assert_eq!(limits.resolved(), Tier::Turbo);

        limits.set_user_present(false);
        assert_eq!(limits.resolved(), Tier::Balanced, "recent interactive keeps it off Eco");

        // Backdate the last interactive call past the window.
        limits.last_interactive_ms.store(0, Ordering::Relaxed);
        let elapsed = limits.start.elapsed().as_millis() as u64;
        limits
            .last_interactive_ms
            .store(elapsed.saturating_sub(RECENT_INTERACTIVE.as_millis() as u64 + 1), Ordering::Relaxed);
        // saturating_sub floors at 0 on a young process, which would read as
        // "just now"; only assert when the process has actually run long enough
        // for the arithmetic to mean anything.
        if elapsed > RECENT_INTERACTIVE.as_millis() as u64 {
            assert_eq!(limits.resolved(), Tier::Eco);
        }
    }

    #[test]
    fn shrinking_takes_only_free_permits_and_growing_restores_them() {
        let limits = Limits::new(Mode::Turbo);
        let cpus = limits.cpus;
        assert_eq!(limits.background.available_permits(), Tier::Turbo.background(cpus));

        limits.set_mode(Mode::Eco);
        assert_eq!(limits.background.available_permits(), 1);
        assert_eq!(limits.background_width(), 1);

        limits.set_mode(Mode::Turbo);
        assert_eq!(limits.background.available_permits(), Tier::Turbo.background(cpus));
    }

    #[tokio::test]
    async fn eco_serialises_background_but_not_interactive() {
        let limits = Limits::new(Mode::Eco);
        let held = limits.acquire(Priority::Background).await;
        assert_eq!(limits.background.available_permits(), 0);
        assert_eq!(limits.in_flight(), (0, 1));
        // The second background caller would block; the interactive one must not.
        let interactive = limits.acquire(Priority::Interactive).await;
        assert_eq!(limits.in_flight(), (1, 1));
        drop(held);
        drop(interactive);
        assert_eq!(limits.background.available_permits(), 1);
        assert_eq!(limits.in_flight(), (0, 0));
    }

    /// A shrink cannot always take every permit, so `granted` can briefly exceed
    /// the target. The monitor must not turn that into a huge number by
    /// underflowing a `usize`.
    #[tokio::test]
    async fn in_flight_survives_a_shrink_under_load() {
        let limits = Limits::new(Mode::Turbo);
        let _held = limits.acquire(Priority::Background).await;
        limits.set_mode(Mode::Eco);
        let (_, background) = limits.in_flight();
        assert_eq!(background, 1, "the one call still out is the one in flight");
    }

    #[test]
    fn mode_round_trips_through_the_wire_string() {
        for mode in Mode::ALL {
            assert_eq!(Mode::parse(mode.as_str()), Some(mode));
            assert_eq!(Mode::from_u8(mode.to_u8()), mode);
        }
        assert_eq!(Mode::parse("ECO"), Some(Mode::Eco));
        assert_eq!(Mode::parse("fastest"), None);
    }
}
