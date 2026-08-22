//! Studio's rendering — state and `update` are `studio.rs`.
//!
//! Three blocks: the backend card (only when something is wrong with it), the
//! composer, and the gallery. The backend card is the one worth reading twice:
//! **a missing ComfyUI is not an error state**, it is an install this app does
//! not do for you, so it renders as an informational card naming the port it
//! looked at and where to get the thing — never a red banner, and never in
//! place of the composer (ADR 0009).

use crate::studio::{Kind, Message, State, IMAGE_SIZES, PRESETS, VIDEO_LENGTHS, VIDEO_SIZES};
use crate::ui::{self, space, Icon, Tone};
use agent_platform_client::types::MediaJob;
use iced::widget::{container, image, Row};
use iced::{Element, Length};

pub fn view(state: &State) -> Element<'_, Message> {
    let mut blocks: Vec<Element<'_, Message>> = Vec::new();

    if let Some(error) = &state.error {
        blocks.push(ui::error_bar(error, Message::TraceLogs, Message::Dismiss, Vec::new()));
    }
    if let Some(card) = backend_card(state) {
        blocks.push(card);
    }
    if let Some(card) = video_models_card(state) {
        blocks.push(card);
    }
    if let Some(card) = unsupported_kind_card(state) {
        blocks.push(card);
    }
    blocks.push(composer(state));
    blocks.push(gallery(state));

    ui::page(
        "Studio",
        Some(ui::muted(
            "Images and video generated on this machine, from a sentence. Nothing leaves the \
             computer.",
        )),
        Some(ui::button_secondary(Icon::Refresh, "Refresh", Message::Refresh)),
        ui::stack_lg(blocks),
    )
}

/// Shown only when the backend needs something. A working install gets its
/// screen back — a permanent "everything is fine" card is a row of pixels
/// that teaches the user to stop reading the top of the page.
fn backend_card(state: &State) -> Option<Element<'_, Message>> {
    let status = state.status.as_ref()?;
    if status.reachable && status.image_model.is_some() {
        return None;
    }

    // The sd-server backend manages itself (ADR 0011), so "not reachable" is
    // never the whole story there — it may be downloading, unpacking, or
    // waiting for a model. Offering "Get ComfyUI" in that state would be
    // pointing at the wrong app entirely.
    if status.backend == "sdcpp" {
        let stage = status.backend_stage.as_deref().unwrap_or("");
        let detail = status.backend_detail.clone();
        let (tone, title, body): (Tone, &str, String) = match stage {
            // Two different missing things, and telling them apart is the
            // difference between "press Generate" and "go install something":
            // `not_installed` is the 39 MB binary, which the first generation
            // fetches on its own, while `unconfigured` is the weights, which
            // it cannot.
            "not_installed" => (
                Tone::Info,
                "Generator not fetched yet",
                "stable-diffusion.cpp is not on disk yet. The first generation downloads                  it — about 39 MB — and starts it for you."
                    .to_string(),
            ),
            "unconfigured" => (
                Tone::Info,
                "No model installed yet",
                "Generation runs on stable-diffusion.cpp, which this app fetches and starts                  for you — but it needs a model to load first."
                    .to_string(),
            ),
            "downloading" | "extracting" => (
                Tone::Info,
                "Setting up the generator",
                "Fetching what stable-diffusion.cpp needs. This runs in the background; \n                 press Refresh for progress."
                    .to_string(),
            ),
            "starting" => (
                Tone::Info,
                "Loading the model",
                "stable-diffusion.cpp is reading the weights into VRAM. Large models take \n                 a few minutes on a cold start."
                    .to_string(),
            ),
            "stopped" => (
                Tone::Info,
                "Generator is idle",
                "The model was unloaded to free VRAM. The next generation starts it again."
                    .to_string(),
            ),
            "failed" => (
                Tone::Warning,
                "The generator could not start",
                "stable-diffusion.cpp exited instead of serving.".to_string(),
            ),
            "external" => (
                Tone::Info,
                "No generator answering",
                format!(
                    "MEDIA_API_BASE points at {}, which this app does not manage. Start it \n                     there, then press Refresh.",
                    status.base
                ),
            ),
            // `ready` with no model, or a stage a newer server added.
            _ => (
                Tone::Warning,
                "The generator has no model loaded",
                "It is running but has nothing to draw with.".to_string(),
            ),
        };
        let mut rows = vec![ui::muted(body)];
        // The backend's own words — a failed model load says which file it
        // could not read, and that is the only actionable sentence available.
        if let Some(detail) = detail {
            rows.push(ui::caption(detail));
        }
        return Some(ui::alert(tone, title, Some(ui::stack(rows).into())));
    }
    let (tone, title, detail): (Tone, &str, Element<'_, Message>) = if !status.reachable {
        (
            Tone::Info,
            "ComfyUI is not running",
            ui::stack(vec![
                ui::muted(format!(
                    "Generation runs through ComfyUI on this machine — nothing was found at {}. \
                     Install it, start it, then press Refresh.",
                    status.base
                )),
                ui::caption(
                    "Already have it on another port? Set MEDIA_API_BASE in the server's .env.",
                ),
                ui::cluster(vec![ui::button_secondary(
                    Icon::Download,
                    "Get ComfyUI",
                    Message::OpenSite,
                )])
                .into(),
            ])
            .into(),
        )
    } else {
        (
            Tone::Warning,
            "ComfyUI has no image models installed",
            ui::stack(vec![
                ui::muted(
                    "The backend is running but has no checkpoint to draw with. Put a \
                     text-to-image model (Flux, Z-Image, SDXL) in ComfyUI/models/checkpoints \
                     and press Refresh.",
                ),
                ui::caption(
                    "Video uses the Wan 2.2 family and its own files — see ComfyUI's \
                     text-to-video template for what it needs.",
                ),
            ])
            .into(),
        )
    };

    Some(ui::alert(tone, title, Some(detail)))
}

/// Shown when the running backend cannot serve the *selected* kind.
///
/// `sd-server` binds one model at startup, so a machine set up for images
/// genuinely cannot make a video until it restarts onto a video model — and it
/// says so through `modes` before a job is submitted rather than after three
/// minutes of rendering nothing.
///
/// **A sentence, not a greyed-out button.** Disabling the Video toggle would
/// leave the user with a dead control and no reason for it; the toggle stays
/// live, the card explains, and pressing Generate installs what is missing
/// rather than failing. An older server sends no `modes` at all, and
/// [`MediaStatus::supports`] reads that as "yes" — refusing on missing
/// information would break a working install.
fn unsupported_kind_card(state: &State) -> Option<Element<'_, Message>> {
    let status = state.status.as_ref()?;
    let wanted = if state.kind == Kind::Video { "video" } else { "image" };
    if !status.reachable || status.supports(wanted) {
        return None;
    }
    let loaded = status.image_model.as_deref().unwrap_or("the current model");
    Some(ui::alert(
        Tone::Info,
        if state.kind == Kind::Video {
            "This model makes images, not video"
        } else {
            "This model makes video, not images"
        },
        Some(
            ui::stack(vec![
                ui::muted(format!(
                    "{loaded} is loaded, and one model is loaded at a time. Generating will                      swap to a {wanted} model — installing it first if it is not on disk."
                )),
                ui::caption(
                    "Both cannot be resident at once on a consumer card, so the swap is the                      cost of switching, not a fault.",
                ),
            ])
            .into(),
        ),
    ))
}

/// What video is missing, and the button that fetches it.
///
/// Only while Video is the selected kind: these files are several gigabytes
/// and someone making images does not need to be told about them. Hidden
/// entirely once ComfyUI reports all three, so a working install gets its
/// screen back — the same rule [`backend_card`] follows.
///
/// The download arms before it fires. The first press swaps the button for
/// the size, the destination and a confirm, because ten gigabytes written
/// into another application's install directory is not a thing to start on a
/// stray click.
fn video_models_card(state: &State) -> Option<Element<'_, Message>> {
    if state.kind != Kind::Video {
        return None;
    }
    let reqs = state.requirements.as_ref()?;
    let install = &state.install;
    if reqs.missing().next().is_none() && !install.running() {
        return None;
    }

    let mut rows: Vec<Element<'_, Message>> = vec![ui::muted(
        "Video needs the Wan 2.2 files by name — ComfyUI rejects the workflow until each one \
         is in place.",
    )];

    // Every required file, present or not, so the list reads as a checklist
    // rather than only naming what went wrong.
    for item in &reqs.items {
        rows.push(ui::caption(format!(
            "{} {}/{} · {}",
            if item.installed { "✓" } else { "—" },
            item.folder,
            item.file_name,
            if item.installed {
                "installed".to_string()
            } else {
                crate::studio::format_bytes(item.size_bytes)
            }
        )));
    }

    if install.running() {
        let current = install.current.as_ref();
        let name = current.map(|c| c.file_name.as_str()).unwrap_or("");
        let got = crate::model_download::human(current.map(|c| c.received).unwrap_or(0));
        rows.push(ui::body(match current.and_then(|c| c.bytes) {
            // A length the server knew or the transfer reported; without one a
            // bare byte count still shows movement.
            Some(total) => format!(
                "{name} — {got} of {} · file {} of {} · {}% overall",
                crate::model_download::human(total),
                install.done + 1,
                install.total,
                (install.fraction() * 100.0).round() as i64
            ),
            None => format!("{name} — {got} so far · file {} of {}", install.done + 1, install.total),
        }));
        rows.push(
            ui::cluster(vec![ui::button_ghost(Icon::X, "Cancel", Message::InstallCancel)]).into(),
        );
    } else if !reqs.can_install() {
        // Something is missing but there is nowhere safe to put it: say so
        // plainly rather than showing a button that cannot work.
        rows.push(ui::caption(
            "ComfyUI's models folder could not be located from here, so these cannot be \
             installed automatically — drop them into the folders above by hand.",
        ));
    } else if install.armed {
        rows.push(ui::body(format!(
            "Download {} into {}?",
            crate::studio::format_bytes(reqs.missing_bytes()),
            reqs.models_root.as_deref().unwrap_or_default()
        )));
        rows.push(
            ui::cluster(vec![
                ui::button_default(Icon::Download, "Yes, download", Message::InstallConfirm),
                ui::button_ghost(Icon::X, "Not now", Message::InstallCancel),
            ])
            .into(),
        );
    } else {
        rows.push(
            ui::cluster(vec![
                ui::button_secondary(
                    Icon::Download,
                    format!("Download the missing files ({})", crate::studio::format_bytes(reqs.missing_bytes())),
                    Message::InstallArm,
                ),
                ui::caption("From Hugging Face, straight into ComfyUI. The app stays usable."),
            ])
            .into(),
        );
    }

    Some(ui::alert(Tone::Warning, "Video models are missing", Some(ui::stack(rows).into())))
}

/// The prompt and everything that shapes it. The `enhance` toggle is spelled
/// out rather than labelled "enhance": it sends the prompt to a language model
/// first, and a user should know that before pressing it.
fn composer(state: &State) -> Element<'_, Message> {
    let is_video = state.kind == Kind::Video;
    let sizes: &[(&str, i64, i64)] = if is_video { &VIDEO_SIZES } else { &IMAGE_SIZES };

    let mut rows: Vec<Element<'_, Message>> = vec![
        ui::segmented([
            (Kind::Image.label(), !is_video, Message::KindChanged(Kind::Image)),
            (Kind::Video.label(), is_video, Message::KindChanged(Kind::Video)),
        ]),
        ui::input(
            if is_video {
                "A paper boat drifting down a rain-filled gutter, slow push in"
            } else {
                "A paper boat on a rain-filled gutter, late afternoon light"
            },
            &state.prompt,
            Message::PromptChanged,
        ),
        ui::field(
            "Avoid",
            ui::input("blurry, extra fingers, watermark", &state.negative, Message::NegativeChanged),
        ),
        preset_field(state),
        ui::field(
            "Size",
            ui::chips(
                sizes
                    .iter()
                    .enumerate()
                    .map(|(i, (label, _, _))| (*label, state.size == i, Message::SizeChanged(i)))
                    .collect::<Vec<_>>(),
            ),
        ),
    ];

    if is_video {
        rows.push(ui::field(
            "Length",
            ui::chips(
                VIDEO_LENGTHS
                    .iter()
                    .map(|(label, frames)| {
                        (*label, state.length == *frames, Message::LengthChanged(*frames))
                    })
                    .collect::<Vec<_>>(),
            ),
        ));
    }

    rows.push(ui::toggle(
        if state.enhance { Icon::Sparkles } else { Icon::Pencil },
        if state.enhance {
            "Let a model expand the prompt first"
        } else {
            "Use my words exactly"
        },
        state.enhance,
        Message::EnhanceToggled(!state.enhance),
    ));

    rows.push(
        ui::cluster(vec![
            ui::button_default(
                if is_video { Icon::Film } else { Icon::Image },
                if state.busy { "Starting…" } else { "Generate" },
                Message::Generate,
            ),
            // Beside Generate rather than by the prompt box: it is the other
            // way to start, for when the empty box is the thing in the way.
            ui::button_secondary(
                Icon::Sparkles,
                if state.suggesting { "Thinking…" } else { "Surprise me" },
                Message::Suggest,
            ),
            ui::caption(if is_video {
                "A few minutes on a 16 GB card. The app stays usable while it renders."
            } else {
                "Usually under a minute, depending on the model and the card."
            }),
        ])
        .into(),
    );

    ui::card_with_header(
        "New generation",
        // Naming the renderer is the point of the line, so it has to be the one
        // that will actually render: sd-server takes flat parameters and there
        // is no workflow graph on that path at all.
        Some(ui::muted(match state.status.as_ref().map(|s| s.backend.as_str()) {
            Some("sdcpp") => "Describe it. The server runs stable-diffusion.cpp on this machine.",
            _ => "Describe it. The server builds the workflow and ComfyUI renders it.",
        })),
        None,
        ui::stack(rows),
    )
}

/// The specialised styles, for the current kind only. "Freeform" is a chip
/// rather than a clear button because deselecting is the same size of
/// decision as selecting, and the caption underneath spells out the words the
/// chosen preset will append — a style that shapes the picture invisibly is a
/// style the user cannot correct.
fn preset_field(state: &State) -> Element<'_, Message> {
    let mut chips: Vec<(&str, bool, Message)> =
        vec![("Freeform", state.preset.is_none(), Message::PresetChanged(None))];
    chips.extend(
        PRESETS
            .iter()
            .enumerate()
            .filter(|(_, preset)| preset.kind == state.kind)
            .map(|(i, preset)| {
                (preset.label, state.preset == Some(i), Message::PresetChanged(Some(i)))
            }),
    );

    let mut rows: Vec<Element<'_, Message>> = vec![ui::chips(chips)];
    if let Some(preset) = state.preset.and_then(|i| PRESETS.get(i)) {
        rows.push(ui::caption(format!("Adds to your prompt: {}", preset.style)));
    }
    ui::field("Style", ui::stack(rows))
}

fn gallery(state: &State) -> Element<'_, Message> {
    if state.jobs.is_empty() {
        return ui::section(
            "Gallery",
            None,
            ui::empty_state_icon(
                Icon::Image,
                "Nothing generated yet. What you make shows up here, newest first.",
            ),
        );
    }

    let cards: Vec<Element<'_, Message>> = state.jobs.iter().map(|job| card(state, job)).collect();
    // Wraps rather than a column: the cards are fixed-width tiles, so a wide
    // window shows a row of them and a narrow one stacks without clipping.
    ui::section("Gallery", None, Row::with_children(cards).spacing(space::MD).wrap())
}

/// Fixed enough to tile, tall enough that a portrait preview and a landscape
/// one leave the row the same height.
const CARD_W: f32 = 300.0;
const PREVIEW_H: f32 = 300.0;

fn card<'a>(state: &'a State, job: &'a MediaJob) -> Element<'a, Message> {
    let (label, tone) = match job.status.as_str() {
        "completed" => ("Done", Tone::Success),
        "failed" => ("Failed", Tone::Danger),
        _ => ("Rendering…", Tone::Info),
    };

    let mut rows: Vec<Element<'_, Message>> = vec![ui::cluster(vec![
        ui::badge(label, tone),
        ui::badge(if job.is_video() { "Video" } else { "Image" }, Tone::Neutral),
        ui::spacer(),
        ui::caption(format!("{}×{}", job.width, job.height)),
    ])
    .into()];

    rows.push(preview(state, job));
    if let Some(line) = timing(state, job) {
        rows.push(ui::caption(line));
    }
    rows.push(ui::body(job.prompt.clone()));

    // What the model actually drew from, when it was not what the user typed
    // — a card that hides the rewrite makes a surprising picture unexplainable.
    if let Some(enhanced) = &job.enhanced_prompt {
        rows.push(ui::caption(format!("Expanded to: {enhanced}")));
    }
    if let Some(error) = &job.error {
        rows.push(ui::caption(error.clone()));
    }
    if job.is_done() {
        rows.push(
            ui::cluster(vec![ui::button_ghost(
                if job.is_video() { Icon::Play } else { Icon::FolderOpen },
                if job.is_video() { "Play" } else { "Open" },
                Message::OpenFile(job.id),
            )])
            .into(),
        );
    }

    container(ui::tile(ui::stack(rows))).width(CARD_W).into()
}

/// `1m 20s elapsed · ~40s left` while it renders, `Took 1m 20s` once it is
/// done. The estimate is dropped rather than faked when this machine has not
/// finished enough of the same kind to have a median (see `State::eta_secs`),
/// so the elapsed half always shows and the guess only appears when it is one.
fn timing(state: &State, job: &MediaJob) -> Option<String> {
    let elapsed = crate::studio::elapsed_secs(job)?;
    if !job.is_running() {
        return job.is_done().then(|| format!("Took {}", crate::studio::format_secs(elapsed)));
    }
    let elapsed = format!("{} elapsed", crate::studio::format_secs(elapsed));
    Some(match state.eta_secs(job) {
        Some(0) => format!("{elapsed} · any moment now"),
        Some(left) => format!("{elapsed} · ~{} left", crate::studio::format_secs(left)),
        None => elapsed,
    })
}

/// The picture, or the honest stand-in for one. A finished video gets a
/// placeholder rather than a frame: iced has no video decoder, so there is
/// nothing to show until the user opens it in a real player (ADR 0009).
fn preview<'a>(state: &'a State, job: &'a MediaJob) -> Element<'a, Message> {
    let placeholder = |glyph, message: &'a str| -> Element<'a, Message> {
        container(ui::empty_state_icon(glyph, message))
            .height(PREVIEW_H)
            .width(Length::Fill)
            .center_y(PREVIEW_H)
            .into()
    };

    match job.status.as_str() {
        "failed" => placeholder(Icon::XCircle, "This one did not render."),
        "completed" if job.is_video() => {
            placeholder(Icon::Film, "Video — press Play to open it in your player.")
        }
        "completed" => match state.images.get(&job.id) {
            Some(handle) => container(
                image(handle.clone()).width(Length::Fill).height(PREVIEW_H).content_fit(iced::ContentFit::Contain),
            )
            .height(PREVIEW_H)
            .into(),
            None => placeholder(Icon::Image, "Loading the picture…"),
        },
        _ => placeholder(Icon::Clock, "Rendering. This card fills in by itself."),
    }
}
