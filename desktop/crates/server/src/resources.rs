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

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use axum::extract::State;
use axum::routing::put;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sysinfo::{Disk, Disks, ProcessesToUpdate, System};
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
// Host sample
// ---------------------------------------------------------------------------

/// What the machine itself is doing, alongside what this server is doing with
/// it. Read by the Performance page's meters, and by the assistant through
/// `api_get /api/v1/system/resources`.
///
/// **Sampled on demand, never on a timer.** Same rule the sidebar monitor lives
/// by: a resource readout that wakes the machine up to report that the machine
/// is busy has become the thing it reports on. The desktop was already polling
/// this route every 5-20 s (see `App::resource_poll_every`), so the sample rides
/// that request and costs nothing when nobody is looking. `sysinfo` keeps the
/// previous CPU tick internally, so a poll that far apart yields the average
/// over the gap rather than an instant that happened to catch a spike.
///
/// GPUs are NVIDIA-only and may be an empty list; see [`GpuView`] for why.
#[derive(Serialize, Clone, Debug)]
pub struct HostView {
    /// Whole machine, 0-100 across all cores.
    pub cpu_percent: f32,
    /// One entry per logical core, in a stable order - the strip of bars under
    /// the CPU meter. What it adds over the average: eight cores at 12% and one
    /// core pinned at 100% are the same average and very different machines.
    pub cpu_per_core: Vec<f32>,
    pub mem_used_bytes: u64,
    pub mem_total_bytes: u64,
    /// Zero total on a machine with swap turned off, which the page draws as
    /// "off" rather than as an empty meter.
    pub swap_used_bytes: u64,
    pub swap_total_bytes: u64,
    /// The volume the workspaces live on - not a sum over every mount, because
    /// a total across disks is a number no path is actually on.
    pub disk_used_bytes: u64,
    pub disk_total_bytes: u64,
    pub disk_mount: String,
    /// `agent-platformd`'s own slice, normalised the same way as `cpu_percent`
    /// (sysinfo reports a process against one core, so 400% is possible before
    /// this divides by the core count). Here so "the machine is busy" can be
    /// told apart from "we are busy", which is the difference between a setting
    /// the user should change and one they should not.
    pub process_cpu_percent: f32,
    pub process_mem_bytes: u64,
    /// Machine uptime, not process uptime - `/system/status` already carries
    /// ours.
    pub uptime_seconds: u64,
    pub os: String,
    /// Empty on a machine with no NVIDIA driver, which is the ordinary
    /// case rather than an error - see [`GpuView`].
    pub gpus: Vec<GpuView>,
}

/// One NVIDIA GPU, as NVML reports it.
///
/// **NVIDIA only, on purpose.** The vendor-neutral answer is three more SDKs
/// (ROCm SMI, Level Zero, DXGI's adapter counters) each with its own runtime that
/// may not be installed; NVML at least fails cleanly, because `nvml-wrapper`
/// `dlopen`s `nvml.dll`/`libnvidia-ml.so` at runtime rather than linking it. So
/// nothing about the build depends on a GPU being present, and a machine without
/// an NVIDIA driver reports an empty list instead of failing to start. A second
/// vendor is a second branch in [`sample_gpus`] when someone has one to test on.
#[derive(Serialize, Clone, Debug)]
pub struct GpuView {
    pub name: String,
    /// Percent of the last sampling period the GPU had work on it. NVML's own
    /// utilisation counter, not a derived number.
    pub utilization_percent: f32,
    pub mem_used_bytes: u64,
    pub mem_total_bytes: u64,
    /// Absent on cards that do not expose a sensor, which is rarer than it
    /// sounds but not impossible on laptop hybrids.
    pub temperature_c: Option<u32>,
}

/// NVML's handle, initialised once. `Err` is the ordinary case on a machine with
/// no NVIDIA driver, and it is cached as such: retrying `Nvml::init()` on every
/// poll would be a failing `dlopen` every five seconds forever.
static NVML: OnceLock<Option<nvml_wrapper::Nvml>> = OnceLock::new();

fn sample_gpus() -> Vec<GpuView> {
    let Some(nvml) = NVML.get_or_init(|| nvml_wrapper::Nvml::init().ok()) else {
        return Vec::new();
    };
    let count = nvml.device_count().unwrap_or(0);
    (0..count)
        .filter_map(|i| {
            let device = nvml.device_by_index(i).ok()?;
            let memory = device.memory_info().ok();
            Some(GpuView {
                name: device.name().unwrap_or_else(|_| format!("GPU {i}")),
                // A card that cannot report utilisation still reports its memory,
                // and the memory bar is the half that matters when a local model
                // is loaded — so a missing counter is a 0, not a dropped card.
                utilization_percent: device
                    .utilization_rates()
                    .map(|u| u.gpu as f32)
                    .unwrap_or(0.0),
                mem_used_bytes: memory.as_ref().map(|m| m.used).unwrap_or(0),
                mem_total_bytes: memory.as_ref().map(|m| m.total).unwrap_or(0),
                temperature_c: device
                    .temperature(nvml_wrapper::enum_wrappers::device::TemperatureSensor::Gpu)
                    .ok(),
            })
        })
        .collect()
}

/// The one `System` in the process. Held across calls because CPU percentages
/// are a *diff* against the previous refresh: a fresh `System` per request would
/// report 0% forever.
static SAMPLER: OnceLock<Mutex<Sampler>> = OnceLock::new();

struct Sampler {
    sys: System,
    disks: Disks,
}

/// Blocking. Call it from `spawn_blocking` - refreshing the CPU and disk tables
/// is syscalls, and on Windows enumerating volumes is not fast.
fn sample_host() -> HostView {
    let cell = SAMPLER.get_or_init(|| {
        let mut sys = System::new();
        // The first CPU read has no previous tick to diff against and reports
        // 0% - a load meter's single most misleading answer. Prime it here, at
        // the cost of one 200 ms sleep once per process, on a blocking thread.
        sys.refresh_cpu_usage();
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        Mutex::new(Sampler { sys, disks: Disks::new_with_refreshed_list() })
    });
    let mut guard = cell.lock().unwrap_or_else(|e| e.into_inner());
    let Sampler { sys, disks } = &mut *guard;

    sys.refresh_cpu_usage();
    sys.refresh_memory();
    let pid = sysinfo::get_current_pid().ok();
    if let Some(pid) = pid {
        // `Some(&[pid])`: refreshing every process on the machine to read one of
        // them is the expensive mistake this route would make by default.
        sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    }
    // `false` - sizes and free space, without re-listing the volumes. A USB
    // stick appearing mid-session is not worth an enumeration every 5 s.
    disks.refresh(false);

    let cores = sys.cpus().len().max(1) as f32;
    let (process_cpu_percent, process_mem_bytes) = pid
        .and_then(|p| sys.process(p))
        .map(|p| (p.cpu_usage() / cores, p.memory()))
        .unwrap_or((0.0, 0));

    let root = crate::workspace_files::workspace_root();
    // Canonicalised, or a relative `data/workspaces` matches no mount point and
    // every machine falls through to "largest disk".
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    let disk = pick_disk(disks, &root);

    HostView {
        cpu_percent: sys.global_cpu_usage(),
        cpu_per_core: sys.cpus().iter().map(|c| c.cpu_usage()).collect(),
        mem_used_bytes: sys.used_memory(),
        mem_total_bytes: sys.total_memory(),
        swap_used_bytes: sys.used_swap(),
        swap_total_bytes: sys.total_swap(),
        disk_used_bytes: disk
            .map(|d| d.total_space().saturating_sub(d.available_space()))
            .unwrap_or(0),
        disk_total_bytes: disk.map(|d| d.total_space()).unwrap_or(0),
        disk_mount: disk.map(|d| d.mount_point().display().to_string()).unwrap_or_default(),
        process_cpu_percent,
        process_mem_bytes,
        uptime_seconds: System::uptime(),
        os: System::long_os_version().unwrap_or_else(|| std::env::consts::OS.to_string()),
        gpus: sample_gpus(),
    }
}

/// The disk `path` sits on: the mount point that is its longest prefix.
///
/// Falls back to the largest disk rather than to nothing, because "no mount
/// matched" means the path was odd (a UNC share, a symlink chain), not that the
/// machine has no disks - and a machine with disks must not draw an empty meter.
fn pick_disk<'a>(disks: &'a Disks, path: &Path) -> Option<&'a Disk> {
    disks
        .list()
        .iter()
        .filter(|d| path.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .or_else(|| disks.list().iter().max_by_key(|d| d.total_space()))
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
    /// The machine underneath, when a sample was taken. `None` rather than
    /// zeroes if sampling ever fails: a CPU meter at 0% is a claim, and "we did
    /// not look" is not that claim.
    pub host: Option<HostView>,
}

fn view(limits: &Limits, host: Option<HostView>) -> ResourcesView {
    let (interactive, background) = limits.in_flight();
    ResourcesView {
        mode: limits.mode().as_str(),
        resolved: limits.resolved().as_str(),
        background_limit: limits.background_width(),
        background_in_flight: background,
        interactive_in_flight: interactive,
        cpus: limits.cpus,
        host,
    }
}

/// One host sample, off the async workers. `None` only if the blocking task
/// itself was cancelled or panicked, which is the case the `Option` is for.
async fn host_sample() -> Option<HostView> {
    tokio::task::spawn_blocking(sample_host).await.ok()
}

async fn get_resources(State(state): State<Arc<AppState>>) -> Json<ResourcesView> {
    Json(view(&state.limits, host_sample().await))
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
    // Sampled here too, not just on GET: the desktop keeps whichever answer
    // came back last, so a PUT that returned `host: None` would blank the
    // Performance page's meters every time the window changed focus.
    Ok(Json(view(&state.limits, host_sample().await)))
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

    /// The disk the workspaces are on, not the first one listed and not the sum
    /// of all of them. `Disk` cannot be constructed outside sysinfo, so this
    /// asserts against the real machine: whatever it picks must be a mount the
    /// workspace root is actually under, unless nothing matched at all.
    #[test]
    fn the_sample_reports_a_disk_the_workspaces_are_on() {
        let host = sample_host();
        assert!(host.mem_total_bytes > 0, "a machine with no memory is not running this test");
        assert!(!host.cpu_per_core.is_empty(), "one bar per core, and there is at least one core");
        assert!(host.disk_used_bytes <= host.disk_total_bytes, "used cannot exceed the volume");
        if !host.disk_mount.is_empty() {
            let root = crate::workspace_files::workspace_root();
            let root = std::fs::canonicalize(&root).unwrap_or(root);
            let matched = root.starts_with(&host.disk_mount);
            let only_fallback = Disks::new_with_refreshed_list()
                .list()
                .iter()
                .all(|d| !root.starts_with(d.mount_point()));
            assert!(matched || only_fallback, "picked {} for {}", host.disk_mount, root.display());
        }
    }

    /// The second sample is the one that matters: the first primes the CPU diff,
    /// and a percentage that stayed pinned at 0 would mean the `System` is being
    /// rebuilt per call instead of held.
    #[test]
    fn a_sample_is_bounded_and_repeatable() {
        let _ = sample_host();
        let host = sample_host();
        assert!((0.0..=100.5).contains(&host.cpu_percent), "cpu was {}", host.cpu_percent);
        for (i, core) in host.cpu_per_core.iter().enumerate() {
            assert!((0.0..=100.5).contains(core), "core {i} was {core}");
        }
        assert!(host.mem_used_bytes <= host.mem_total_bytes);
        assert!(host.process_mem_bytes > 0, "this process has memory");
    }

    /// Cannot assert a GPU exists — CI has none, and an empty list is the
    /// documented answer there. What it can assert is that a card NVML *did*
    /// report comes back whole, rather than as a named row of zeroes.
    #[test]
    fn a_listed_gpu_is_a_complete_reading() {
        for gpu in sample_gpus() {
            assert!(!gpu.name.is_empty(), "a card NVML listed has a name");
            assert!(
                (0.0..=100.0).contains(&gpu.utilization_percent),
                "{} reported {}%",
                gpu.name,
                gpu.utilization_percent
            );
            assert!(gpu.mem_used_bytes <= gpu.mem_total_bytes, "{} VRAM", gpu.name);
            assert!(gpu.mem_total_bytes > 0, "{} has memory", gpu.name);
        }
    }
}
