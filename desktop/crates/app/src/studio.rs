//! Studio: local image and video generation (ADR 0009). State and `update`
//! only — rendering is `studio_view.rs`, per the root `CLAUDE.md` split.
//!
//! The server does all the work (`/api/v1/media/*` → ComfyUI over loopback);
//! this screen is a prompt box, a size row and a gallery of jobs. Two things
//! it owns that the server cannot:
//!
//! **The poll.** A generation is a background job, so the screen ticks while
//! any job is still running and stops when none is — [`State::polling`] is
//! that predicate, and `main.rs` gates the subscription on it. Nothing polls
//! on a screen where every job has settled.
//!
//! **The image cache.** `iced::widget::image::Handle` wants bytes, and the
//! bytes come from `GET /media/jobs/{id}/file`. Fetched once per finished
//! image job and kept in [`State::images`], because a view runs every frame
//! and re-fetching a picture sixty times a second is not a cache miss, it is
//! a denial of service against your own server. Videos are never fetched —
//! iced cannot decode one, so a finished video offers *Open* and *Reveal*
//! instead (the ceiling the ADR names).

use std::collections::HashMap;

use agent_platform_client::types::{
    MediaGenerateRequest, MediaJob, MediaRequirement, MediaRequirements, MediaStatus,
    MediaSuggestion,
};
use agent_platform_client::Client;
use iced::widget::image;
use iced::Task;

use crate::domain::err_string;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Image,
    Video,
}

impl Default for Kind {
    fn default() -> Self {
        Kind::Image
    }
}

impl Kind {
    pub fn wire(self) -> &'static str {
        match self {
            Kind::Image => "image",
            Kind::Video => "video",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Image => "Image",
            Kind::Video => "Video",
        }
    }
}

/// The size presets, per kind. Video is deliberately small and short: Wan 2.2
/// 5B at 832×480 is minutes on a 16 GB card, and offering 1080p here would be
/// offering a render nobody waits out.
pub const IMAGE_SIZES: [(&str, i64, i64); 3] =
    [("Square 1024", 1024, 1024), ("Portrait 832×1216", 832, 1216), ("Landscape 1216×832", 1216, 832)];
pub const VIDEO_SIZES: [(&str, i64, i64); 2] = [("480p 832×480", 832, 480), ("Square 640", 640, 640)];

/// A specialised job: a style the model gets appended to whatever the user
/// typed, plus the form settings that style needs. Everything a preset does
/// is visible in the composer afterwards and editable — it fills the fields,
/// it does not hide a second prompt. Only `style` is applied at generate
/// time, and the composer names it under the chips.
pub struct Preset {
    pub label: &'static str,
    pub kind: Kind,
    /// Appended to the prompt, comma-joined, at generate time.
    pub style: &'static str,
    pub negative: &'static str,
    /// Index into that kind's size table — a logo wants a square, a photo a
    /// landscape, and picking the preset should not leave that to the user.
    pub size: usize,
}

/// The catalogue. Keywords, not sentences: this is what a diffusion model
/// reads, and the user's own subject stays the head of the prompt.
pub const PRESETS: &[Preset] = &[
    Preset {
        label: "Pixel art",
        kind: Kind::Image,
        style: "pixel art, 16-bit sprite, limited palette, crisp square pixels, hard black                 outline, no anti-aliasing",
        negative: "blurry, smooth gradients, photorealistic, anti-aliased, jpeg artifacts",
        size: 0,
    },
    Preset {
        label: "Logo mark",
        kind: Kind::Image,
        style: "flat vector logo mark, bold simple geometric shapes, solid colours, centred,                 plain white background, no lettering",
        negative: "photograph, 3d render, gradient mesh, text, letters, watermark, busy                    background, drop shadow",
        size: 0,
    },
    Preset {
        label: "Icon",
        kind: Kind::Image,
        style: "minimal line-art icon, uniform stroke weight, monochrome, flat, centred,                 generous margin, plain white background",
        negative: "shading, gradient, colour fill, texture, 3d, text, watermark",
        size: 0,
    },
    Preset {
        label: "Sticker",
        kind: Kind::Image,
        style: "die-cut sticker illustration, thick white border, bold flat colours, glossy,                 centred on a plain background",
        negative: "photorealistic, muted colours, complex background, text, watermark",
        size: 0,
    },
    Preset {
        label: "Isometric",
        kind: Kind::Image,
        style: "isometric 3/4 view game asset, clean edges, soft ambient occlusion, muted                 palette, plain background",
        negative: "perspective distortion, photograph, motion blur, text, watermark",
        size: 0,
    },
    Preset {
        label: "Photo",
        kind: Kind::Image,
        style: "photograph, 50mm lens, shallow depth of field, natural light, fine detail,                 realistic colour",
        negative: "illustration, cartoon, cgi, oversaturated, extra fingers, text, watermark",
        size: 2,
    },
    Preset {
        label: "Cinematic",
        kind: Kind::Video,
        style: "cinematic footage, shallow depth of field, slow dolly in, natural light,                 film grain",
        negative: "static frame, jitter, warped faces, text, watermark",
        size: 0,
    },
    Preset {
        label: "Seamless loop",
        kind: Kind::Video,
        style: "seamless looping motion, subject centred, static camera, even lighting",
        negative: "camera shake, hard cuts, scene change, text, watermark",
        size: 1,
    },
];

/// Frame counts at 24 fps, as seconds. Wan's latent wants `4n+1` frames.
pub const VIDEO_LENGTHS: [(&str, i64); 3] = [("~2s", 49), ("~3s", 73), ("~4s", 97)];

/// Where the backend comes from, for the card's "Get ComfyUI" button.
const COMFY_SITE: &str = "https://www.comfy.org/download";

#[derive(Default)]
pub struct State {
    pub prompt: String,
    pub negative: String,
    pub kind: Kind,
    /// Index into [`IMAGE_SIZES`] / [`VIDEO_SIZES`] for the current kind.
    pub size: usize,
    /// Index into [`PRESETS`], or `None` for freeform. Only presets of the
    /// current kind are offered, so this never disagrees with `kind`.
    pub preset: Option<usize>,
    pub length: i64,
    pub enhance: bool,
    pub status: Option<MediaStatus>,
    pub jobs: Vec<MediaJob>,
    /// Decoded output bytes per finished image job — see the module doc for
    /// why this is a cache and not a per-frame fetch.
    pub images: HashMap<i64, image::Handle>,
    /// Job ids already asked for, so a fetch in flight (or one that failed)
    /// is not started again on the next tick.
    fetched: std::collections::HashSet<i64>,
    pub busy: bool,
    /// A suggestion is in flight — the button says so, and a second press
    /// while it is out would race two prompts into the same box.
    pub suggesting: bool,
    /// What video needs and what ComfyUI has, once asked. `None` before the
    /// first answer, which reads as "not known yet" and offers nothing.
    pub requirements: Option<MediaRequirements>,
    pub install: Install,
    pub error: Option<String>,
}

/// Fetching the video models ComfyUI is missing.
///
/// Ten gigabytes is not something a single click should start, so the button
/// arms before it fires: [`Install::armed`] is the state between the first
/// press and the confirm, and it is where the size and the destination are
/// shown. Idle → armed → running, and any press of Cancel goes back to idle.
#[derive(Default)]
pub struct Install {
    pub armed: bool,
    /// The file being fetched right now, and how far in — `None` when nothing
    /// is running.
    pub current: Option<InstallProgress>,
    /// Still to fetch after the current one. Drained in order, because the
    /// stream engine does one file at a time.
    pub queue: Vec<MediaRequirement>,
    pub handle: Option<iced::task::Handle>,
    /// The half-written file, swept by hand on cancel — the transfer is
    /// dropped mid-write, exactly as in [`crate::model_download`].
    pub part: Option<std::path::PathBuf>,
    pub done: usize,
    pub total: usize,
}

pub struct InstallProgress {
    pub file_name: String,
    pub received: u64,
    pub bytes: Option<u64>,
}

impl Install {
    pub fn running(&self) -> bool {
        self.current.is_some()
    }

    /// `0.0`–`1.0` across the whole set, not just the file in flight: a bar
    /// that restarts at zero for each of three files reports three downloads,
    /// which is not what the user pressed.
    pub fn fraction(&self) -> f32 {
        let Some(current) = &self.current else {
            return 0.0;
        };
        let within = match current.bytes {
            Some(total) if total > 0 => current.received as f32 / total as f32,
            _ => 0.0,
        };
        ((self.done as f32 + within) / self.total.max(1) as f32).clamp(0.0, 1.0)
    }
}

/// `9.3 GB`, `1.3 GB`, `812 MB`. Decimal units, because that is what a model
/// card and a download manager both quote.
pub fn format_bytes(bytes: i64) -> String {
    const GB: f64 = 1_000_000_000.0;
    const MB: f64 = 1_000_000.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else {
        format!("{:.0} MB", b / MB)
    }
}

/// Wall-clock seconds a job has been going: to `updated_at` once it settled,
/// to now while it is still running.
///
/// Both stamps are naive UTC, written by the server's `sql_now`
/// (`%Y-%m-%d %H:%M:%S%.6f`); the `T`-separated ISO form is accepted too
/// because that is what the fixtures and any hand-written row use. An
/// unparseable stamp yields `None` rather than a wrong number — a card with
/// no timer beats a card counting from the wrong epoch.
pub fn elapsed_secs(job: &MediaJob) -> Option<i64> {
    let start = parse_stamp(&job.created_at)?;
    let end = if job.is_running() {
        chrono::Utc::now().naive_utc()
    } else {
        parse_stamp(&job.updated_at)?
    };
    Some((end - start).num_seconds().max(0))
}

fn parse_stamp(raw: &str) -> Option<chrono::NaiveDateTime> {
    let text = raw.trim();
    ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"]
        .iter()
        .find_map(|f| chrono::NaiveDateTime::parse_from_str(text, f).ok())
}

/// `1m 20s`, `45s`. Minutes and seconds only — nothing here runs for hours,
/// and "0h 1m 20s" is three units to read a two-unit number.
pub fn format_secs(secs: i64) -> String {
    let (m, s) = (secs / 60, secs % 60);
    if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

impl State {
    /// Whether anything is still cooking — `main.rs` gates the tick on this,
    /// so a settled gallery costs nothing.
    pub fn polling(&self) -> bool {
        self.jobs.iter().any(MediaJob::is_running)
    }

    pub fn dimensions(&self) -> (i64, i64) {
        let table: &[(&str, i64, i64)] =
            if self.kind == Kind::Video { &VIDEO_SIZES } else { &IMAGE_SIZES };
        let (_, w, h) = table.get(self.size).copied().unwrap_or(table[0]);
        (w, h)
    }

    /// Seconds still to go on a running job, or `None` when there is nothing
    /// to base that on.
    ///
    /// The estimate is the median of what finished jobs of the same kind
    /// actually took on this machine — a card and a model this app cannot
    /// inspect make any a-priori number a guess, whereas "the last few took
    /// 90s" is measured. Median, not mean, because one job that was queued
    /// behind another would drag an average up and never come back down.
    ///
    /// `None` until two have finished: a countdown built from a single sample
    /// is a number with a decimal point and no information in it.
    pub fn eta_secs(&self, job: &MediaJob) -> Option<i64> {
        if !job.is_running() {
            return None;
        }
        let mut past: Vec<i64> = self
            .jobs
            .iter()
            .filter(|j| j.is_done() && j.kind == job.kind)
            .filter_map(elapsed_secs)
            .collect();
        if past.len() < 2 {
            return None;
        }
        past.sort_unstable();
        let typical = past[past.len() / 2];
        // Saturates at zero rather than going negative: a job running past
        // the estimate says "any moment now", it does not owe time back.
        Some((typical - elapsed_secs(job)?).max(0))
    }

    /// The backend is there but has nothing to draw with — a different state
    /// from "not installed", and the view says so differently.
    pub fn needs_model(&self) -> bool {
        self.status.as_ref().is_some_and(|s| s.reachable && s.image_model.is_none())
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    /// "View logs" on a traced error banner — intercepted in `main::update`,
    /// so this arm exists only for exhaustiveness (as on every other screen).
    TraceLogs(String),
    PromptChanged(String),
    NegativeChanged(String),
    KindChanged(Kind),
    SizeChanged(usize),
    /// A specialised style, or `None` for freeform.
    PresetChanged(Option<usize>),
    LengthChanged(i64),
    EnhanceToggled(bool),
    Generate,
    Generated(Result<Box<MediaJob>, String>),
    /// "Surprise me" — ask the local model for a prompt to try.
    Suggest,
    Suggested(Result<Box<MediaSuggestion>, String>),
    RequirementsLoaded(Result<Box<MediaRequirements>, String>),
    /// First press: show what it would cost. Second: start fetching.
    InstallArm,
    InstallConfirm,
    InstallCancel,
    InstallProgressed(crate::model_download::Progress),
    /// Screen entry and the running-job tick both land here.
    Refresh,
    StatusLoaded(Result<Box<MediaStatus>, String>),
    JobsLoaded(Result<Vec<MediaJob>, String>),
    ImageLoaded(i64, Result<Vec<u8>, String>),
    /// A finished video (or any output) in the desktop's own default viewer.
    OpenFile(i64),
    /// "Get ComfyUI" on the backend card — the download page, in the user's
    /// own browser, the same way Providers hands out a "Get API key" link.
    OpenSite,
    /// The write-then-open half of [`Message::OpenFile`]. A failure here is a
    /// banner, unlike a failed thumbnail: the user pressed a button and is
    /// owed an answer either way.
    FileOpened(Result<(), String>),
    Dismiss,
}

pub fn update(state: &mut State, client: &Client, message: Message) -> Task<Message> {
    match message {
        Message::TraceLogs(_) => Task::none(),
        Message::PromptChanged(v) => {
            state.prompt = v;
            Task::none()
        }
        Message::NegativeChanged(v) => {
            state.negative = v;
            Task::none()
        }
        Message::KindChanged(kind) => {
            // The size tables differ in length, so an index carried across
            // would silently land on a different preset — or out of range.
            state.kind = kind;
            state.size = 0;
            // The chips only ever offer this kind's presets, and the size
            // index a preset picked belongs to the other table anyway.
            state.preset = None;
            if kind == Kind::Video && state.length == 0 {
                state.length = VIDEO_LENGTHS[0].1;
            }
            Task::none()
        }
        Message::SizeChanged(i) => {
            state.size = i;
            Task::none()
        }
        Message::PresetChanged(index) => {
            state.preset = index;
            // A preset fills the form rather than overriding it at generate
            // time — the user sees what it did and can edit any of it.
            if let Some(preset) = index.and_then(|i| PRESETS.get(i)) {
                state.size = preset.size;
                state.negative = preset.negative.to_string();
            }
            Task::none()
        }
        Message::LengthChanged(frames) => {
            state.length = frames;
            Task::none()
        }
        Message::EnhanceToggled(on) => {
            state.enhance = on;
            Task::none()
        }
        Message::Generate => generate(state, client),
        Message::Suggest => {
            if state.suggesting {
                return Task::none();
            }
            state.suggesting = true;
            state.error = None;
            let (client, kind) = (client.clone(), state.kind.wire());
            Task::perform(
                async move { err_string(client.suggest_media_prompt(kind).await).map(Box::new) },
                Message::Suggested,
            )
        }
        Message::Suggested(Ok(s)) => {
            state.suggesting = false;
            // Straight into the box, replacing whatever was there: the button
            // says "Surprise me", and one that appended would build a paragraph.
            state.prompt = s.prompt;
            Task::none()
        }
        Message::Suggested(Err(e)) => {
            state.suggesting = false;
            state.error = Some(e);
            Task::none()
        }
        Message::Generated(Ok(job)) => {
            state.busy = false;
            state.error = None;
            state.jobs.insert(0, *job);
            Task::none()
        }
        Message::Generated(Err(e)) => {
            state.busy = false;
            state.error = Some(e);
            Task::none()
        }
        Message::Refresh => {
            // Requirements ride along with the other two rather than waiting
            // for the user to pick Video: the card that offers the download
            // should already be there when they get to it.
            let (a, b, c) = (load_status(client), load_jobs(client), load_requirements(client));
            Task::batch([a, b, c])
        }
        Message::RequirementsLoaded(Ok(reqs)) => {
            state.requirements = Some(*reqs);
            state.install.armed = false;
            Task::none()
        }
        Message::RequirementsLoaded(Err(_)) => {
            // Not a banner. This is a side question the screen asked on its
            // own; failing it costs the download button, not the screen.
            state.requirements = None;
            Task::none()
        }
        Message::InstallArm => {
            state.install.armed = true;
            Task::none()
        }
        Message::InstallCancel => cancel_install(state),
        Message::InstallConfirm => start_install(state),
        Message::InstallProgressed(progress) => install_progressed(state, client, progress),
        Message::StatusLoaded(Ok(status)) => {
            state.status = Some(*status);
            state.error = None;
            Task::none()
        }
        Message::StatusLoaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Message::JobsLoaded(Ok(jobs)) => {
            state.error = None;
            // Fetch the bytes of any finished image the cache has not seen.
            // Videos are skipped on purpose — see the module doc.
            let wanted: Vec<i64> = jobs
                .iter()
                .filter(|j| j.is_done() && !j.is_video() && !state.fetched.contains(&j.id))
                .map(|j| j.id)
                .collect();
            state.jobs = jobs;
            let tasks: Vec<Task<Message>> = wanted
                .into_iter()
                .map(|id| {
                    state.fetched.insert(id);
                    let client = client.clone();
                    Task::perform(
                        async move { (id, err_string(client.media_file(id).await)) },
                        |(id, result)| Message::ImageLoaded(id, result),
                    )
                })
                .collect();
            Task::batch(tasks)
        }
        Message::JobsLoaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Message::ImageLoaded(id, Ok(bytes)) => {
            state.images.insert(id, image::Handle::from_bytes(bytes));
            Task::none()
        }
        Message::ImageLoaded(id, Err(_)) => {
            // Not a banner: one thumbnail that would not load says nothing
            // about the screen, and the card still shows the job as finished.
            // Dropped from `fetched` so a later refresh may retry it.
            state.fetched.remove(&id);
            Task::none()
        }
        Message::OpenFile(id) => open_output(state, client, id),
        Message::OpenSite => {
            crate::shell::open_url(COMFY_SITE);
            Task::none()
        }
        Message::FileOpened(Ok(())) => {
            state.error = None;
            Task::none()
        }
        Message::FileOpened(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Message::Dismiss => {
            state.error = None;
            Task::none()
        }
    }
}

fn generate(state: &mut State, client: &Client) -> Task<Message> {
    let mut prompt = state.prompt.trim().to_string();
    if prompt.is_empty() {
        state.error = Some("Describe what you want generated.".into());
        return Task::none();
    }
    if state.status.as_ref().is_some_and(|s| !s.reachable) {
        state.error = Some(
            "ComfyUI is not running — start it, then press Refresh. See the card above.".into(),
        );
        return Task::none();
    }
    if let Some(preset) = state.preset.and_then(|i| PRESETS.get(i)) {
        prompt = format!("{prompt}, {}", preset.style);
    }
    state.busy = true;
    state.error = None;

    let (width, height) = state.dimensions();
    let negative = state.negative.trim().to_string();
    let req = MediaGenerateRequest {
        kind: state.kind.wire().to_string(),
        prompt,
        negative: (!negative.is_empty()).then_some(negative),
        width: Some(width),
        height: Some(height),
        length: (state.kind == Kind::Video).then_some(state.length.max(49)),
        enhance: state.enhance,
    };
    let client = client.clone();
    Task::perform(
        async move { err_string(client.generate_media(&req).await).map(Box::new) },
        Message::Generated,
    )
}

/// Begin the queue the confirm step just approved. One file at a time: the
/// download engine streams a single URL, and three parallel multi-gigabyte
/// transfers over one connection finish no sooner and report far worse.
fn start_install(state: &mut State) -> Task<Message> {
    let Some(reqs) = &state.requirements else {
        return Task::none();
    };
    let Some(root) = reqs.models_root.clone() else {
        state.error =
            Some("ComfyUI's models folder could not be located, so there is nowhere to put these \
                  files. Install them by hand, or set MEDIA_API_BASE to the instance you mean."
                .into());
        return Task::none();
    };

    let mut queue: Vec<MediaRequirement> = reqs.missing().cloned().collect();
    if queue.is_empty() {
        state.install = Install::default();
        return Task::none();
    }
    queue.reverse(); // popped from the back, so the first listed goes first.

    state.install = Install {
        armed: false,
        total: queue.len(),
        queue,
        ..Install::default()
    };
    next_download(state, &root)
}

/// Start the next file in the queue, or finish. `root` is threaded in rather
/// than re-read because the requirements are refreshed at the end, and reading
/// it from a half-updated state is how the last file lands in the wrong place.
fn next_download(state: &mut State, root: &str) -> Task<Message> {
    let Some(item) = state.install.queue.pop() else {
        return Task::none();
    };
    let dir = std::path::PathBuf::from(root).join(&item.folder);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        state.error = Some(format!("Could not create {}: {e}", dir.display()));
        state.install = Install::default();
        return Task::none();
    }

    state.install.part = Some(crate::model_download::part_path(&dir, &item.file_name));
    state.install.current = Some(InstallProgress {
        file_name: item.file_name.clone(),
        received: 0,
        bytes: u64::try_from(item.size_bytes).ok(),
    });

    let (task, handle) = Task::stream(crate::model_download::download(
        item.url.clone(),
        dir,
        item.file_name.clone(),
    ))
    .map(Message::InstallProgressed)
    .abortable();
    state.install.handle = Some(handle);
    task
}

fn install_progressed(
    state: &mut State,
    client: &Client,
    progress: crate::model_download::Progress,
) -> Task<Message> {
    use crate::model_download::Progress;
    match progress {
        Progress::Downloading { received, total } => {
            if let Some(current) = state.install.current.as_mut() {
                current.received = received;
                // The server's size is the one shown before the press; the
                // transfer's own is better once it exists.
                current.bytes = total.or(current.bytes);
            }
            Task::none()
        }
        Progress::Done(_) => {
            state.install.done += 1;
            state.install.part = None;
            state.install.current = None;
            let root = state
                .requirements
                .as_ref()
                .and_then(|r| r.models_root.clone())
                .unwrap_or_default();
            if state.install.queue.is_empty() {
                // Re-ask rather than assume: ComfyUI has to see the file for
                // the card to go away, and that is the thing worth confirming.
                state.install = Install::default();
                return load_requirements(client);
            }
            next_download(state, &root)
        }
        Progress::Failed(e) => {
            state.error = Some(e);
            sweep_part(state);
            state.install = Install::default();
            Task::none()
        }
    }
}

fn cancel_install(state: &mut State) -> Task<Message> {
    if let Some(handle) = state.install.handle.take() {
        handle.abort();
    }
    sweep_part(state);
    state.install = Install::default();
    Task::none()
}

/// The transfer is dropped mid-write, so the partial file is ours to remove —
/// the same hand-sweep [`crate::model_download`] documents.
fn sweep_part(state: &mut State) {
    if let Some(part) = state.install.part.take() {
        let _ = std::fs::remove_file(part);
    }
}

fn load_requirements(client: &Client) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move { err_string(client.media_requirements().await).map(Box::new) },
        Message::RequirementsLoaded,
    )
}

fn load_status(client: &Client) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move { err_string(client.media_status().await).map(Box::new) },
        Message::StatusLoaded,
    )
}

fn load_jobs(client: &Client) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move { err_string(client.media_jobs().await).map(|r| r.jobs) },
        Message::JobsLoaded,
    )
}

/// Hands a finished output to the desktop's default viewer — the only way to
/// watch a video, since iced cannot decode one (ADR 0009), and the "see it
/// full size" path for an image.
///
/// The bytes live in the server's media folder, but this app does not know
/// where that folder is — the daemon may even be a separate install it merely
/// attached to — so the file comes over the same HTTP route the gallery uses
/// and is written to the temp dir before being opened.
fn open_output(state: &State, client: &Client, id: i64) -> Task<Message> {
    let Some(name) = state.jobs.iter().find(|j| j.id == id).and_then(|j| j.file_name.clone())
    else {
        return Task::none();
    };
    let client = client.clone();
    Task::perform(
        async move {
            let bytes = err_string(client.media_file(id).await)?;
            let path = std::env::temp_dir().join(format!("agent-platform-media-{name}"));
            std::fs::write(&path, bytes)
                .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
            crate::shell::reveal_path(&path.display().to_string());
            Ok(())
        },
        Message::FileOpened,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn client() -> Client {
        Client::new("http://127.0.0.1:1", "k")
    }

    fn job(id: i64, kind: &str, status: &str, file: Option<&str>) -> MediaJob {
        serde_json::from_value(json!({
            "id": id, "kind": kind, "prompt": "a cat", "enhanced_prompt": null,
            "status": status, "error": null, "width": 1024, "height": 1024, "length": 0,
            "seed": 1, "file_name": file,
            "created_at": "2026-08-19T00:00:00", "updated_at": "2026-08-19T00:00:00"
        }))
        .unwrap()
    }

    /// The tick has to stop. A gallery where everything has settled must not
    /// keep asking, and one still running must.
    #[test]
    fn polling_follows_whether_anything_is_still_running() {
        let mut s = State::default();
        assert!(!s.polling(), "an empty gallery polls nothing");
        s.jobs = vec![job(1, "image", "completed", Some("1_a.png"))];
        assert!(!s.polling());
        s.jobs.push(job(2, "video", "running", None));
        assert!(s.polling());
        s.jobs = vec![job(2, "video", "failed", None)];
        assert!(!s.polling(), "a failed job is settled, not pending");
    }

    /// The cache's whole point: a finished image is fetched once, and a
    /// second refresh carrying the same job must not ask again.
    #[test]
    fn a_finished_image_is_fetched_once_and_a_video_never() {
        let mut s = State::default();
        let jobs = vec![
            job(1, "image", "completed", Some("1_a.png")),
            job(2, "video", "completed", Some("2_a.mp4")),
            job(3, "image", "running", None),
        ];
        let _ = update(&mut s, &client(), Message::JobsLoaded(Ok(jobs.clone())));
        assert_eq!(s.fetched.iter().copied().collect::<Vec<_>>(), vec![1]);

        let _ = update(&mut s, &client(), Message::JobsLoaded(Ok(jobs)));
        assert_eq!(s.fetched.len(), 1, "the same image must not be fetched twice");
    }

    fn reqs(models_root: Option<&str>, installed: [bool; 2]) -> MediaRequirements {
        serde_json::from_value(json!({
            "models_root": models_root,
            "items": [
                { "folder": "diffusion_models", "file_name": "unet.safetensors",
                  "url": "https://example/unet.safetensors", "size_bytes": 10_000_000_000i64,
                  "installed": installed[0] },
                { "folder": "vae", "file_name": "vae.safetensors",
                  "url": "https://example/vae.safetensors", "size_bytes": 1_400_000_000i64,
                  "installed": installed[1] },
            ]
        }))
        .unwrap()
    }

    /// The confirm step is the whole point: one press must not start ten
    /// gigabytes, and backing out must leave no trace.
    #[test]
    fn the_download_arms_before_it_fires() {
        let mut s = State::default();
        s.requirements = Some(reqs(Some("C:/comfy/models"), [false, false]));

        let _ = update(&mut s, &client(), Message::InstallArm);
        assert!(s.install.armed, "the first press only arms");
        assert!(!s.install.running(), "nothing may be fetched before the confirm");

        let _ = update(&mut s, &client(), Message::InstallCancel);
        assert!(!s.install.armed);
        assert!(!s.install.running());
    }

    /// Only what is missing gets queued, and the size quoted is the sum of
    /// exactly those files.
    #[test]
    fn only_the_missing_files_are_queued_and_priced() {
        let all = reqs(Some("C:/comfy/models"), [false, false]);
        assert_eq!(all.missing().count(), 2);
        assert_eq!(all.missing_bytes(), 11_400_000_000);
        assert!(all.can_install());

        let one = reqs(Some("C:/comfy/models"), [true, false]);
        assert_eq!(one.missing().count(), 1);
        assert_eq!(one.missing_bytes(), 1_400_000_000);

        let none = reqs(Some("C:/comfy/models"), [true, true]);
        assert!(!none.can_install(), "nothing missing is nothing to install");
    }

    /// No verified directory means no download, however much is missing.
    #[test]
    fn without_a_models_root_nothing_may_be_installed() {
        let blind = reqs(None, [false, false]);
        assert!(!blind.can_install());

        let mut s = State::default();
        s.requirements = Some(blind);
        let _ = update(&mut s, &client(), Message::InstallConfirm);
        assert!(!s.install.running(), "a confirm without a destination starts nothing");
        assert!(s.error.is_some(), "and says why");
    }

    /// The bar counts the whole set. Half of the first of two files is a
    /// quarter of the job, not half of it.
    #[test]
    fn progress_spans_the_whole_queue() {
        let mut i = Install { total: 2, ..Install::default() };
        assert_eq!(i.fraction(), 0.0, "nothing running is no progress");

        i.current = Some(InstallProgress { file_name: "a".into(), received: 50, bytes: Some(100) });
        assert!((i.fraction() - 0.25).abs() < f32::EPSILON, "got {}", i.fraction());

        i.done = 1;
        i.current = Some(InstallProgress { file_name: "b".into(), received: 50, bytes: Some(100) });
        assert!((i.fraction() - 0.75).abs() < f32::EPSILON, "got {}", i.fraction());

        // An unknown length must not divide by zero or exceed the whole.
        i.current = Some(InstallProgress { file_name: "b".into(), received: 50, bytes: None });
        assert!((0.0..=1.0).contains(&i.fraction()));
    }

    #[test]
    fn byte_sizes_read_as_a_model_card_writes_them() {
        assert_eq!(format_bytes(9_999_658_848), "10.0 GB");
        assert_eq!(format_bytes(1_409_400_960), "1.4 GB");
        assert_eq!(format_bytes(812_000_000), "812 MB");
    }

    /// A failed requirements fetch is a missing button, not a red banner over
    /// a screen that is otherwise fine.
    #[test]
    fn a_failed_requirements_fetch_is_not_a_banner() {
        let mut s = State::default();
        let _ = update(&mut s, &client(), Message::RequirementsLoaded(Err("boom".into())));
        assert!(s.requirements.is_none());
        assert!(s.error.is_none());
    }

    /// A failed fetch clears its mark so a later refresh can retry, and does
    /// not raise a banner over an otherwise fine screen.
    #[test]
    fn a_failed_image_fetch_is_retryable_and_not_a_banner() {
        let mut s = State::default();
        s.fetched.insert(7);
        let _ = update(&mut s, &client(), Message::ImageLoaded(7, Err("boom".into())));
        assert!(!s.fetched.contains(&7));
        assert!(s.error.is_none());
    }

    #[test]
    fn switching_kind_resets_the_size_index_into_the_other_table() {
        let mut s = State::default();
        s.size = 2; // valid for IMAGE_SIZES, out of range for VIDEO_SIZES
        let _ = update(&mut s, &client(), Message::KindChanged(Kind::Video));
        assert_eq!(s.size, 0);
        assert_eq!(s.dimensions(), (832, 480));
        assert_eq!(s.length, VIDEO_LENGTHS[0].1, "a video needs a frame count");
    }

    fn timed(id: i64, kind: &str, status: &str, created: &str, updated: &str) -> MediaJob {
        let mut j = job(id, kind, status, Some("f.png"));
        j.created_at = created.into();
        j.updated_at = updated.into();
        j
    }

    /// Both stamp dialects parse, a settled job measures to `updated_at`, and
    /// a stamp that makes no sense yields nothing rather than a wrong number.
    #[test]
    fn elapsed_reads_both_stamp_formats_and_refuses_nonsense() {
        let sql = timed(1, "image", "completed", "2026-08-19 00:00:00.000000", "2026-08-19 00:01:20.000000");
        assert_eq!(elapsed_secs(&sql), Some(80));

        let iso = timed(2, "image", "completed", "2026-08-19T00:00:00", "2026-08-19T00:00:45");
        assert_eq!(elapsed_secs(&iso), Some(45));

        let junk = timed(3, "image", "completed", "not a date", "also not");
        assert_eq!(elapsed_secs(&junk), None);

        assert_eq!(format_secs(45), "45s");
        assert_eq!(format_secs(80), "1m 20s");
        assert_eq!(format_secs(3), "3s");
    }

    /// The estimate needs two finished jobs of the same kind before it says
    /// anything, uses their median, and never counts below zero.
    #[test]
    fn eta_waits_for_history_and_never_goes_negative() {
        let mut s = State::default();
        let running = timed(9, "video", "running", "2026-08-19 00:00:00.000000", "2026-08-19 00:00:00.000000");

        s.jobs = vec![running.clone()];
        assert_eq!(s.eta_secs(&running), None, "no history, no guess");

        // One finished job is still not enough to call anything typical.
        s.jobs.push(timed(1, "video", "completed", "2026-08-19 00:00:00.000000", "2026-08-19 00:01:00.000000"));
        assert_eq!(s.eta_secs(&running), None);

        // Three finished: median of 60/120/600 is 120. The running job started
        // "now", so the estimate is essentially the whole median.
        s.jobs.push(timed(2, "video", "completed", "2026-08-19 00:00:00.000000", "2026-08-19 00:02:00.000000"));
        s.jobs.push(timed(3, "video", "completed", "2026-08-19 00:00:00.000000", "2026-08-19 00:10:00.000000"));
        let eta = s.eta_secs(&running).expect("three finished videos is a median");
        assert!(eta <= 120, "the estimate is the median minus elapsed, got {eta}");

        // An image's history must not be read as a video's.
        let img = timed(10, "image", "running", "2026-08-19 00:00:00.000000", "2026-08-19 00:00:00.000000");
        assert_eq!(s.eta_secs(&img), None, "kinds do not share a median");

        // A job that has outrun the estimate reports zero, not a debt.
        let overdue = timed(11, "video", "running", "2000-01-01 00:00:00.000000", "2000-01-01 00:00:00.000000");
        s.jobs.push(overdue.clone());
        assert_eq!(s.eta_secs(&overdue), Some(0));
    }

    /// A settled job has no countdown at all.
    #[test]
    fn a_finished_job_has_no_eta() {
        let mut s = State::default();
        let done = timed(1, "image", "completed", "2026-08-19 00:00:00.000000", "2026-08-19 00:00:30.000000");
        s.jobs = vec![done.clone(), timed(2, "image", "completed", "2026-08-19 00:00:00.000000", "2026-08-19 00:00:30.000000")];
        assert_eq!(s.eta_secs(&done), None);
    }

    /// The suggestion replaces the box and clears the in-flight flag; a second
    /// press while one is out must not fire a second request.
    #[test]
    fn a_suggestion_lands_in_the_prompt_box_once() {
        let mut s = State::default();
        s.prompt = "old words".into();
        let _ = update(&mut s, &client(), Message::Suggest);
        assert!(s.suggesting);

        let before = s.prompt.clone();
        let _ = update(&mut s, &client(), Message::Suggest);
        assert_eq!(s.prompt, before, "a second press while in flight changes nothing");

        let reply = serde_json::from_value(json!({ "kind": "image", "prompt": "a red door" })).unwrap();
        let _ = update(&mut s, &client(), Message::Suggested(Ok(Box::new(reply))));
        assert_eq!(s.prompt, "a red door");
        assert!(!s.suggesting);
    }

    #[test]
    fn an_empty_prompt_is_rejected_without_a_network_call() {
        let mut s = State::default();
        let _ = update(&mut s, &client(), Message::Generate);
        assert!(s.error.is_some());
        assert!(!s.busy);
    }

    /// The unreachable case is caught before the request, so the user gets
    /// the sentence that names the fix rather than a transport error.
    #[test]
    fn generating_with_no_backend_names_the_fix() {
        let mut s = State::default();
        s.prompt = "a cat".into();
        s.status = Some(
            serde_json::from_value(json!({
                "reachable": false, "base": "http://127.0.0.1:8188",
                "checkpoints": [], "image_model": null
            }))
            .unwrap(),
        );
        let _ = update(&mut s, &client(), Message::Generate);
        assert!(s.error.as_deref().is_some_and(|e| e.contains("ComfyUI is not running")));
        assert!(!s.busy);
    }

    #[test]
    fn a_new_job_lands_at_the_top_and_clears_a_stale_error() {
        let mut s = State::default();
        s.error = Some("boom".into());
        s.jobs = vec![job(1, "image", "completed", Some("1_a.png"))];
        let _ = update(
            &mut s,
            &client(),
            Message::Generated(Ok(Box::new(job(2, "image", "running", None)))),
        );
        assert_eq!(s.jobs.first().map(|j| j.id), Some(2));
        assert!(s.error.is_none());
        assert!(!s.busy);
    }

    /// A preset is only ever offered for its own kind, and its size index
    /// belongs to that kind's table — a stale index from the other one would
    /// land on a different preset or out of range.
    #[test]
    fn every_preset_indexes_its_own_size_table() {
        for preset in PRESETS {
            let table = if preset.kind == Kind::Video { VIDEO_SIZES.len() } else { IMAGE_SIZES.len() };
            assert!(preset.size < table, "{} points past its size table", preset.label);
            assert!(!preset.style.is_empty(), "{} adds nothing", preset.label);
        }
    }

    /// Picking a preset fills the form; changing kind drops it, because the
    /// chips never offer a preset of the other kind.
    #[test]
    fn a_preset_fills_the_form_and_a_kind_change_drops_it() {
        let (mut s, c) = (State::default(), client());
        let i = PRESETS.iter().position(|p| p.kind == Kind::Image).unwrap();
        let _ = update(&mut s, &c, Message::PresetChanged(Some(i)));
        assert_eq!(s.preset, Some(i));
        assert_eq!(s.negative, PRESETS[i].negative, "the avoid box shows what the preset asks for");
        assert_eq!(s.size, PRESETS[i].size);

        let _ = update(&mut s, &c, Message::KindChanged(Kind::Video));
        assert_eq!(s.preset, None);
        assert_eq!(s.size, 0);
    }
}
