//! E.V. screen: suit-AI heads-up display — a live spectrum analyzer wearing a
//! spider-web, a rotating targeting reticle, an orbiting sensor pair and a
//! reactor core that thumps on the bass of whoever is talking — transcript
//! below, composer at the bottom. The HUD is the only custom canvas outside
//! the DAG; everything else composes the ui kit.
//!
//! Everything here is driven by real audio: `assistant::Tick` runs a Goertzel
//! band pass over the mic (or over E.V.'s own playback) at 60 fps, so nothing
//! on screen is a canned loop — the web spokes *are* the spectrum bins.

use crate::assistant::{Message, Mode, State};
use crate::assistant_gate::{BANDS, WAVE};
use crate::shell::HudStyle;
use crate::ui::{self, space, theme, Icon, Tone};
use iced::widget::canvas::{self, Frame, Geometry, LineCap, Path, Stroke, Text};
use iced::widget::{canvas as canvas_widget, column, container, markdown};
use iced::{mouse, Color, Element, Length, Point, Radians, Rectangle, Renderer, Theme};

/// The live HUD canvas alone, for embedding outside this screen (the
/// Dashboard). Whoever shows it must also run the `assistant::Tick`
/// subscription, or the canvas freezes.
pub fn hud<'a>(
    state: &'a State,
    height: impl Into<Length>,
    style: HudStyle,
    iced_theme: &Theme,
) -> Element<'a, Message> {
    let height = height.into();
    let body: Element<'a, Message> = match style {
        HudStyle::Bubble => {
            let orb = iced::widget::shader(crate::bubble_shader::Bubble::new(state, iced_theme))
                .width(Length::Fill)
                .height(height);
            // A shader cannot draw text, so the status line the reference types
            // out under its orb is a real text widget stacked over it. Same
            // reveal, same `elapsed` clock as the canvas styles use.
            let shown = typed(mode_label(state.mode()), state.elapsed);
            iced::widget::stack![
                orb,
                container(iced::widget::text(shown.to_string()).size(17))
                    .width(Length::Fill)
                    .height(height)
                    .align_x(iced::Center)
                    .align_y(iced::alignment::Vertical::Bottom)
                    .padding(space::LG),
            ]
            .into()
        }
        _ => canvas_widget(Hud {
            style,
            phase: state.phase,
            mode: state.mode(),
            prev: state.mode_prev,
            mix: ease_out(state.mode_t),
            level: state.mic_level,
            energy: state.energy,
            beat: state.beat,
            boot: ease_out(state.boot),
            elapsed: state.elapsed,
            floor: state.floor,
            voice_sim: state.voice_sim,
            bands: &state.bands,
            wave: &state.wave,
        })
        .width(Length::Fill)
        .height(height)
        .into(),
    };
    container(body).style(theme::hud_backdrop).width(Length::Fill).into()
}

/// What the orb says it is doing. Shared by both bubble styles so the GPU and
/// canvas versions never drift apart on wording.
pub fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Idle => "Standing by",
        Mode::Armed => "Mic live",
        Mode::Listening => "Listening",
        Mode::Thinking => "Thinking",
        Mode::Speaking => "Speaking",
    }
}

/// The conversation itself: HUD, error banner, transcript, composer — everything
/// except the provider/model header. Split out from [`view`] so the floating
/// panel ([`crate::screen::assistant_overlay`]) is the same widget tree rather
/// than a second copy that drifts: one composer, one mic button, one transcript.
pub fn panel<'a>(state: &'a State, iced_theme: &Theme, style: HudStyle) -> Element<'a, Message> {
    let mode = state.mode();

    // Only meaningful in voice mode: every line of it reports the mic.
    let status = || -> Element<'_, Message> { ui::cluster(vec![
        ui::badge(
            match mode {
                Mode::Idle => "SYSTEMS NOMINAL",
                Mode::Armed => "MIC LIVE · MONITORING",
                Mode::Listening => "LISTENING",
                Mode::Thinking => "ANALYZING",
                Mode::Speaking => "TRANSMITTING",
            },
            match mode {
                Mode::Idle => Tone::Success,
                Mode::Armed => Tone::Info,
                Mode::Listening => Tone::Danger,
                Mode::Thinking => Tone::Warning,
                Mode::Speaking => Tone::Info,
            },
        ),
        // Voice ID is a filter on who gets answered, so its state is never
        // hidden: learning, locked on, or told this was someone else.
        match (state.voice_enrolled(), state.voice_sim) {
            (false, _) if state.armed() => ui::caption("Learning your voice…".to_string()),
            (true, Some(sim)) if sim < crate::assistant_gate::VOICE_MATCH => {
                ui::caption("Different voice — parked, not sent".to_string())
            }
            (true, _) => ui::caption("Voice ID locked".to_string()),
            _ => ui::caption(String::new()),
        },
        // Voice mode answers whatever it hears, so say out loud what gets it to
        // answer: being named, or replying inside the follow-up window.
        ui::caption(if state.armed() {
            format!("Say “{}, …” — or just talk right after a reply", crate::assistant::name())
        } else {
            "Mic closed".to_string()
        }),
        // Repair, not a mode — it belongs beside the voice-ID readout it
        // undoes, and only once there is something to forget.
        if state.voice_enrolled() {
            ui::button_ghost(Icon::XCircle, "Forget voice", Message::ForgetVoice)
        } else {
            ui::caption(String::new())
        },
    ])
    .into() };

    let transcript: Element<'_, Message> = if state.messages.is_empty() {
        ui::empty_state(if state.voice {
            "Web-shooters primed. What do you need?".to_string()
        } else {
            format!("Ask {} anything.", crate::assistant::name())
        })
    } else {
        // Open flow, not boxes: role tag over content, markdown for E.V.
        // Snap-to-end happens per new message; free scrolling in between.
        let turns: Vec<Element<'_, Message>> = state
            .messages
            .iter()
            .zip(&state.md)
            .enumerate()
            .map(|(i, (m, items))| {
                let is_user = m.role == "user";
                let (label, tone) = match m.role.as_str() {
                    "user" => ("YOU", Tone::Neutral),
                    // What a tool answered — fenced output. Not "TERMINAL": most
                    // of these are an API read or a screen change now, and the
                    // call row directly above already names which tool it was.
                    "tool" => ("TOOL", Tone::Info),
                    _ => (crate::assistant::name(), Tone::Danger),
                };
                let mut parts: Vec<Element<'_, Message>> = Vec::new();
                // A reasoning model's chain-of-thought: open while it streams
                // (before the answer starts), collapsed behind a toggle after.
                // Displayed only — the voice never reads it.
                let reasoning = state.reasoning.get(i).map(String::as_str).unwrap_or("");
                if !reasoning.is_empty() {
                    let open = state.reasoning_live(i) || state.reasoning_open.contains(&i);
                    parts.push(ui::thinking(reasoning, open, Message::ToggleReasoning(i)));
                }
                parts.push(if is_user {
                    ui::body(m.content.clone())
                } else {
                    markdown::view(items, markdown::Settings::from(iced_theme))
                        .map(Message::LinkClicked)
                });
                ui::turn(label, tone, is_user, column(parts).spacing(space::XS).into())
            })
            .collect();
        ui::transcript(crate::assistant::transcript_id(), turns)
    };

    let composer = ui::card(ui::composer(
        if state.voice { crate::assistant::composer_hint() } else { "Message…" },
        &state.draft,
        Message::DraftChanged,
        Message::Send,
        vec![
            // One control for the whole mode. The mic button is the wake signal,
            // the way it is in every other assistant: press to hand the thread
            // to the voice, press again to get the keyboard back.
            if state.voice {
                ui::button_destructive(Icon::MicOff, "Exit voice", Message::Listen)
            } else {
                ui::button_secondary(Icon::Mic, "Voice", Message::Listen)
            },
            if state.sending {
                ui::badge("thinking…", Tone::Info)
            } else {
                ui::button_default(Icon::Send, "Send", Message::Send)
            },
        ],
    ));
    // Standby is the one state with an open mic and no HUD over it. It does not
    // get to be invisible: this row is the whole disclosure.
    let composer = match state.standby && !state.voice && state.armed() {
        false => composer,
        true => {
            let note: Element<'_, Message> = ui::cluster(vec![
                ui::badge_icon(
                    Icon::Mic,
                    format!(
                        "MIC LIVE · WAITING FOR “{}”",
                        crate::assistant::name().to_uppercase()
                    ),
                    Tone::Warning,
                ),
                ui::caption("Anything else it hears is dropped.".to_string()),
            ])
            .into();
            ui::card(column![composer, note].spacing(space::XS))
        }
    };

    // Text mode is the same conversation without the theatre: no HUD canvas, no
    // mic telemetry, so nothing on screen implies audio is running.
    let mut blocks: Vec<Element<'_, Message>> = Vec::new();
    if state.voice {
        blocks.push(hud(state, 224.0, style, iced_theme));
        blocks.push(status());
    }
    if let Some(err) = &state.error {
        let extra: Vec<Element<'_, Message>> = if err.contains("Privacy → Microphone") {
            vec![ui::button_secondary(Icon::Settings, "Open Settings", Message::OpenMicSettings)]
        } else {
            Vec::new()
        };
        blocks.push(ui::error_bar(err, Message::TraceLogs, Message::DismissError, extra));
    }
    blocks.push(container(transcript).height(Length::Fill).into());
    // Between the transcript and the composer, where Coder puts its own: the
    // change is the thing to read before typing anything else.
    if let Some(write) = &state.pending {
        blocks.push(approval(write));
    }
    blocks.push(composer);
    column(blocks).spacing(space::MD).height(Length::Fill).into()
}

/// The one gate between a model and anything outside this app — the user's data
/// through the API, or their whole machine through the shell.
fn approval<'a>(proposal: &'a crate::assistant_tools::Pending) -> Element<'a, Message> {
    let mut body: Vec<Element<'_, Message>> = vec![ui::code(ui::mono(proposal.summary()))];
    body.extend(proposal.detail().map(|d| ui::code(ui::mono(d))));
    ui::approval(
        proposal.heading(),
        Tone::Warning,
        body,
        "No",
        Message::Decide(false),
        Some(Message::Decide(true)),
    )
}

pub fn view<'a>(state: &'a State, iced_theme: &Theme, style: HudStyle) -> Element<'a, Message> {
    // Who is answering leads the page. The old header put a 24px "E.V." title and
    // a line of flavour text here and pushed these two into a trailing cluster of
    // five equal-weight widgets — but the tab strip above already names the
    // assistant, and the HUD says which mode it is in. Both were repeating
    // something on screen; the model was not.
    let mut head: Vec<Element<'_, Message>> = vec![ui::model_pickers(
        state.provider_ids(),
        &state.provider,
        Message::ProviderChanged,
        state.model_options(),
        &state.model,
        Message::ModelChanged,
    )
    .into()];
    // pick_list cannot deselect, so going back to the server default needs its
    // own button — shown only while an override is active.
    if !state.provider.is_empty() || !state.model.is_empty() {
        head.push(ui::button_ghost(Icon::X, "Default", Message::UseDefaults));
    }
    // No mode segment up here: the composer's mic button is the mode, and two
    // controls for one state is how you end up in voice mode with a shut mic.
    head.push(ui::spacer());
    head.push(ui::button_ghost(Icon::Trash, "Clear", Message::Clear));

    ui::page_custom(ui::cluster(head), panel(state, iced_theme, style))
}

// ---------------------------------------------------------------------------
// HUD canvas — a live analyzer dressed as a suit AI.
//
// Layers, back to front: grid + drifting dust, the waveform ribbon, the web
// (whose spokes are the spectrum bins), the spectrum rim with peak hold, the
// rotating reticle, orbiting sensors, the reactor core, then the chrome —
// brackets, telemetry and the input meter. Nothing loops on a timer alone;
// every radius, brightness and thickness is a function of live audio.
// ---------------------------------------------------------------------------

/// Web spokes are the spectrum bins, one to one.
const SPOKES: usize = BANDS;
/// Bars around the rim. More than there are bins — interpolated, so the rim
/// reads as a continuous edge instead of two dozen teeth.
const BARS: usize = 96;
/// Motes of drifting dust; parallax is what stops a flat canvas reading flat.
const DUST: usize = 26;

// Suit palette, deliberately not theme tokens: the HUD is E.V.'s territory and
// keeps the classic red/blue in both light and dark themes.
const SPIDEY_RED: Color = Color::from_rgb(0.902, 0.169, 0.180); // #E62B2E
const SPIDEY_BLUE: Color = Color::from_rgb(0.263, 0.451, 0.918); // #4373EA
const HOLO_CYAN: Color = Color::from_rgb(0.208, 0.816, 1.0); // #35D0FF
const HOT: Color = Color::from_rgb(0.94, 0.98, 1.0); // filament white

struct Hud<'a> {
    style: HudStyle,
    /// Seconds of animation time.
    phase: f32,
    mode: Mode,
    /// Mode the crossfade is coming from, and how far along it is (eased).
    prev: Mode,
    mix: f32,
    /// Smoothed mic level — the explicit INPUT meter, unchanged semantics.
    level: f32,
    /// Broadband energy of whoever is talking.
    energy: f32,
    /// Transient flash on an attack.
    beat: f32,
    /// Power-on sweep, 0..1 (eased).
    boot: f32,
    /// Seconds in the current mode.
    elapsed: f32,
    /// The room's learned noise floor (raw RMS) — reported so a gate that
    /// refuses to open has a visible reason.
    floor: f32,
    /// How close the last utterance was to the enrolled voice, if there is one.
    voice_sim: Option<f32>,
    bands: &'a [f32],
    wave: &'a [f32],
}

fn ease_out(t: f32) -> f32 {
    let t = 1.0 - t.clamp(0.0, 1.0);
    1.0 - t * t * t
}

fn mix_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

fn fade(c: Color, a: f32) -> Color {
    Color { a: (c.a * a).clamp(0.0, 1.0), ..c }
}

/// Band energy at a fractional position across the spectrum, interpolated so
/// `BARS` bars can ride `BANDS` bins without stair-stepping.
pub fn band_at(bands: &[f32], x: f32) -> f32 {
    if bands.is_empty() {
        return 0.0;
    }
    let pos = x.clamp(0.0, 1.0) * (bands.len() - 1) as f32;
    let i = pos as usize;
    let a = bands[i];
    let b = *bands.get(i + 1).unwrap_or(&a);
    a + (b - a) * (pos - i as f32)
}

/// Deterministic 0..1 from an index — the dust needs to look scattered, not to
/// actually be random (a fresh RNG each frame would make it boil).
fn hash01(i: u32) -> f32 {
    let x = i.wrapping_mul(2_654_435_761) ^ (i << 7);
    ((x >> 8) & 0xFF_FFFF) as f32 / 16_777_216.0
}

/// Poor man's bloom: the canvas has no blur, so stroke the same path wide and
/// faint, then narrow and bright. Three passes is where it stops paying off.
fn glow(frame: &mut Frame, path: &Path, color: Color, width: f32) {
    for (w, a) in [(width * 4.5, 0.10), (width * 2.0, 0.22), (width, 1.0)] {
        frame.stroke(
            path,
            Stroke::default()
                .with_color(fade(color, a))
                .with_width(w)
                .with_line_cap(LineCap::Round),
        );
    }
}

/// How much of `label` has been typed after `elapsed` seconds in this state.
/// Split on character boundaries, not bytes: this runs inside `draw`, where a
/// slice through the middle of a multi-byte character is a panic mid-render, and
/// the day someone writes "Écoute" here is the day it fires.
fn typed(label: &str, elapsed: f32) -> &str {
    const CPS: f32 = 18.0;
    let n = (elapsed.max(0.0) * CPS) as usize;
    match label.char_indices().nth(n) {
        Some((byte, _)) => &label[..byte],
        None => label,
    }
}

/// Three colours as a closed wheel: `u` turns once around and lands back on `a`.
/// The band's colour is read off this per segment, so a discontinuity anywhere
/// in it — including the wrap — is a hard seam across the ring.
fn wheel(a: Color, b: Color, c: Color, u: f32) -> Color {
    let u = u.rem_euclid(1.0) * 3.0;
    let f = u.fract();
    match u as usize {
        0 => mix_color(a, b, f),
        1 => mix_color(b, c, f),
        _ => mix_color(c, a, f),
    }
}

fn mono(content: String, position: Point, color: Color, size: f32) -> Text {
    Text { content, position, color, size: size.into(), font: iced::Font::MONOSPACE, ..Text::default() }
}

impl canvas::Program<Message> for Hud<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        iced_theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        match self.style {
            // `Bubble` never reaches the canvas — it is the shader widget. Kept
            // total rather than unreachable so a new style cannot slip through.
            HudStyle::Bubble | HudStyle::BubbleCanvas => {
                self.draw_bubble(&mut frame, iced_theme, bounds)
            }
            HudStyle::Suit => self.draw_suit(&mut frame, iced_theme, bounds),
        }
        vec![frame.into_geometry()]
    }
}

impl Hud<'_> {
    fn draw_suit(&self, frame: &mut Frame, iced_theme: &Theme, bounds: Rectangle) {
        use std::f32::consts::{PI, TAU};

        let t = theme::tokens(iced_theme);
        let hue = |m: Mode| match m {
            Mode::Idle => SPIDEY_BLUE,
            // Armed is its own colour on purpose: an open mic must never look
            // like an idle one.
            Mode::Armed => t.success,
            Mode::Listening => SPIDEY_RED,
            Mode::Thinking => t.warning,
            Mode::Speaking => HOLO_CYAN,
        };
        // Every stroke here is additive light on a dark field. On paper that
        // reads as haze, so in light mode the palette is pushed toward ink:
        // same hues, enough darkness to survive a white backdrop.
        let ink = |c: Color| if t.dark { c } else { mix_color(c, Color::BLACK, 0.45) };
        // Colour crosses the mode change instead of cutting to it.
        let accent = ink(mix_color(hue(self.prev), hue(self.mode), self.mix));
        let holo = fade(ink(HOLO_CYAN), 0.75);
        // Filament white is the brightest thing on a dark HUD; on light it has
        // to be the darkest.
        let hot = if t.dark { HOT } else { Color::from_rgb(0.05, 0.07, 0.12) };
        let web = Color { a: if t.dark { 0.40 } else { 0.55 }, ..ink(SPIDEY_RED) };

        let (w, h) = (bounds.width, bounds.height);
        let center = Point::new(w / 2.0, h / 2.0);
        // Power-on: everything sweeps out from the core and fades up once.
        let boot = self.boot;
        let r_max = ((h / 2.0) - 14.0) * (0.30 + 0.70 * boot);
        let at = |radius: f32, angle: f32| {
            Point::new(center.x + radius * angle.cos(), center.y + radius * angle.sin())
        };
        // Bands read loudest at the bottom of the rim and the top of the web,
        // so the two never move in lockstep.
        let bass = band_at(self.bands, 0.08);
        let energy = self.energy;
        let spin = self.phase * if self.mode == Mode::Thinking { 2.6 } else { 1.0 };

        // -- Backdrop: grid, drifting dust, and a slow vertical scan ---------
        let grid = Path::new(|b| {
            let mut y = (h % 12.0) / 2.0;
            while y < h {
                b.move_to(Point::new(0.0, y));
                b.line_to(Point::new(w, y));
                y += 12.0;
            }
            let mut x = (w % 44.0) / 2.0;
            while x < w {
                b.move_to(Point::new(x, 0.0));
                b.line_to(Point::new(x, h));
                x += 44.0;
            }
        });
        frame.stroke(
            &grid,
            Stroke::default().with_color(fade(holo, 0.06 + 0.05 * energy)).with_width(1.0),
        );

        let dust = Path::new(|b| {
            for i in 0..DUST {
                let seed = i as u32 * 7 + 3;
                // Slow drift up and to the right, wrapping — the parallax layer.
                let sx = (hash01(seed) * w + self.phase * (6.0 + hash01(seed + 1) * 14.0)) % w;
                let sy = (hash01(seed + 2) * h - self.phase * (4.0 + hash01(seed + 3) * 9.0))
                    .rem_euclid(h);
                b.circle(Point::new(sx, sy), 0.6 + hash01(seed + 4) * 1.3);
            }
        });
        frame.fill(&dust, fade(holo, (0.18 + energy * 0.5) * boot));

        // One scan bar every 6 s, with a short bright trail behind it.
        let scan = (self.phase / 6.0).fract() * (w + 60.0) - 30.0;
        for k in 0..4 {
            let x = scan - k as f32 * 7.0;
            if x < 0.0 || x > w {
                continue;
            }
            frame.fill_rectangle(
                Point::new(x, 0.0),
                iced::Size::new(1.5, h),
                fade(holo, 0.16 / (k as f32 + 1.0)),
            );
        }

        // -- Waveform ribbon: the last two seconds of loudness ---------------
        if self.wave.len() > 2 {
            let base = h - 12.0;
            let amp = 11.0;
            let step = w / (WAVE - 1) as f32;
            let ribbon = Path::new(|b| {
                b.move_to(Point::new(0.0, base));
                for (i, v) in self.wave.iter().enumerate() {
                    b.line_to(Point::new(i as f32 * step, base - v * amp));
                }
                for (i, v) in self.wave.iter().enumerate().rev() {
                    b.line_to(Point::new(i as f32 * step, base + v * amp));
                }
                b.close();
            });
            frame.fill(&ribbon, fade(accent, 0.18 * boot));
            frame.stroke(
                &ribbon,
                Stroke::default().with_color(fade(accent, 0.55 * boot)).with_width(1.0),
            );
        }

        // -- Web: spokes are bins, rings sag like real strands ----------------
        // Louder voice tenses the strands: they surge outward, brighten, thicken.
        let strands = Path::new(|b| {
            for k in 0..SPOKES {
                let a = k as f32 * TAU / SPOKES as f32 - PI / 2.0;
                let e = self.bands.get(k).copied().unwrap_or(0.0);
                b.move_to(at(r_max * 0.16, a));
                b.line_to(at(r_max * (0.98 + e * 0.14), a));
            }
        });
        frame.stroke(
            &strands,
            Stroke::default()
                .with_color(fade(web, (0.55 + energy * 0.45) * boot))
                .with_width(1.0 + energy * 1.2),
        );
        // Per-spoke hot overlay so an individual bin can flare on its own.
        for k in 0..SPOKES {
            let e = self.bands.get(k).copied().unwrap_or(0.0);
            if e < 0.35 {
                continue;
            }
            let a = k as f32 * TAU / SPOKES as f32 - PI / 2.0;
            let line = Path::line(at(r_max * 0.16, a), at(r_max * (0.98 + e * 0.14), a));
            frame.stroke(
                &line,
                Stroke::default().with_color(fade(ink(SPIDEY_RED), e * boot)).with_width(1.0 + e),
            );
        }
        for (i, ring) in [0.34_f32, 0.55, 0.76, 0.97].iter().enumerate() {
            // Inner rings react harder — the voice radiates from the core.
            let surge = 1.0 + energy * (0.14 - 0.03 * i as f32);
            let r = r_max * ring * surge;
            // Louder pulls the sag flatter, like a strand under tension.
            let sag = 0.93 + energy * 0.05;
            let path = Path::new(|b| {
                b.move_to(at(r, -PI / 2.0));
                for k in 1..=SPOKES {
                    let a1 = k as f32 * TAU / SPOKES as f32 - PI / 2.0;
                    let a_mid = a1 - PI / SPOKES as f32;
                    b.quadratic_curve_to(at(r * sag, a_mid), at(r, a1));
                }
            });
            frame.stroke(
                &path,
                Stroke::default()
                    .with_color(fade(web, (0.5 + energy * 0.4) * boot))
                    .with_width(1.0 + energy * 0.8),
            );
        }
        // A glint runs outward along the hottest strand — the web "catches" the
        // loudest frequency in the room.
        if energy > 0.08 {
            let (hot_k, hot_e) = self
                .bands
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(k, e)| (k, *e))
                .unwrap_or((0, 0.0));
            let a = hot_k as f32 * TAU / SPOKES as f32 - PI / 2.0;
            let travel = (self.phase * 1.6).fract();
            let r0 = r_max * (0.16 + travel * 0.82);
            let glint = Path::line(at(r0, a), at(r0 + r_max * 0.12, a));
            glow(frame, &glint, fade(hot, hot_e * (1.0 - travel)), 1.6);
        }

        // -- Spectrum rim: mirrored bars, bass at 12 o'clock -----------------
        let r_in = r_max * 0.42;
        // With nobody talking the bins are flat, and a flat rim looks dead. A
        // shallow travelling ripple keeps it alive; it fades out the moment
        // there is real signal, so it never dresses up measured data.
        let idle_ripple = |x: f32| {
            let quiet = (0.03 - self.energy).max(0.0) / 0.03;
            quiet * 0.055 * (0.5 + 0.5 * (self.phase * 2.2 + x * 9.0).sin())
        };
        let peaks = Path::new(|b| {
            for i in 0..BARS {
                let f = i as f32 / BARS as f32;
                // Triangle map: identical left and right halves read as designed
                // rather than noisy.
                let x = if f <= 0.5 { f * 2.0 } else { (1.0 - f) * 2.0 };
                let e = band_at(self.bands, x);
                let a = f * TAU - PI / 2.0;
                b.move_to(at(r_in, a));
                b.line_to(at(r_in + r_max * (0.05 + 0.34 * e + idle_ripple(x)), a));
            }
        });
        frame.stroke(
            &peaks,
            Stroke::default()
                .with_color(fade(mix_color(accent, ink(HOLO_CYAN), 0.35), 0.85 * boot))
                .with_width(2.0)
                .with_line_cap(LineCap::Round),
        );
        // Only the loud bars get the expensive treatment, and only every third
        // one of those: three strokes each adds up fast at 60 fps.
        for i in (0..BARS).step_by(3) {
            let f = i as f32 / BARS as f32;
            let x = if f <= 0.5 { f * 2.0 } else { (1.0 - f) * 2.0 };
            let e = band_at(self.bands, x);
            if e < 0.55 {
                continue;
            }
            let a = f * TAU - PI / 2.0;
            let bar = Path::line(at(r_in, a), at(r_in + r_max * (0.05 + 0.34 * e), a));
            glow(frame, &bar, fade(mix_color(accent, hot, e - 0.4), boot), 1.8);
        }
        // Peak-hold: a dashed ring parked at the loudest bin.
        const DASH: [f32; 2] = [3.0, 7.0];
        let hold = self.bands.iter().copied().fold(0.0_f32, f32::max);
        let hold_ring = Path::circle(center, r_in + r_max * (0.05 + 0.34 * hold));
        frame.stroke(
            &hold_ring,
            Stroke {
                line_dash: canvas::LineDash { segments: &DASH, offset: (self.phase * 30.0) as usize },
                ..Stroke::default().with_color(fade(holo, 0.45 * boot)).with_width(1.0)
            },
        );

        // -- Reticle: three arc groups at different radii, speeds, directions -
        for (radius_f, sweep, width, dir, color) in [
            (0.86_f32, 0.9_f32, 1.6_f32, 0.45_f32, holo),
            (0.62, 1.15, 2.4, -1.0, accent),
            (0.30, 0.75, 1.4, 1.8, holo),
        ] {
            let r = r_max * radius_f;
            for half in [0.0, PI] {
                let start = spin * dir + half;
                let arc = Path::new(|b| {
                    b.arc(canvas::path::Arc {
                        center,
                        radius: r,
                        start_angle: Radians(start),
                        end_angle: Radians(start + sweep),
                    });
                });
                frame.stroke(
                    &arc,
                    Stroke::default().with_color(fade(color, boot)).with_width(width),
                );
                // Chevron cap at the leading end sells the rotation direction.
                let tip = start + if dir > 0.0 { sweep } else { 0.0 };
                let chev = Path::new(|b| {
                    b.move_to(at(r - 4.0, tip - 0.05 * dir.signum()));
                    b.line_to(at(r + 4.0, tip));
                    b.line_to(at(r - 4.0, tip + 0.05 * dir.signum()));
                });
                frame.stroke(
                    &chev,
                    Stroke::default().with_color(fade(color, boot)).with_width(1.2),
                );
            }
        }
        // Fixed crosshair ticks, length modulated by the nearest bin.
        for (q, a) in [0.0_f32, 0.5, 1.0, 1.5].map(|q| q * PI).into_iter().enumerate() {
            let e = band_at(self.bands, q as f32 / 3.0);
            let tick = Path::line(at(r_max * 0.20, a), at(r_max * (0.26 + 0.05 * e), a));
            frame.stroke(
                &tick,
                Stroke::default().with_color(fade(holo, boot)).with_width(1.5),
            );
        }

        // -- Orbiting sensors on a tilted ring, with comet trails -------------
        let tilt = 0.42 + 0.1 * (self.phase * 0.31).sin();
        let (ta, tb) = (r_max * 1.02, r_max * 1.02 * tilt);
        let roll = self.phase * 0.23;
        let orbit_point = |theta: f32| {
            let (x, y) = (ta * theta.cos(), tb * theta.sin());
            Point::new(
                center.x + x * roll.cos() - y * roll.sin(),
                center.y + x * roll.sin() + y * roll.cos(),
            )
        };
        let ellipse = Path::new(|b| {
            b.move_to(orbit_point(0.0));
            for k in 1..=64 {
                b.line_to(orbit_point(k as f32 * TAU / 64.0));
            }
        });
        frame.stroke(
            &ellipse,
            Stroke::default().with_color(fade(holo, 0.22 * boot)).with_width(1.0),
        );
        for (n, lead) in [(0.0_f32, 1.0_f32), (PI, 0.7)] {
            let theta = spin * 0.6 + n;
            let trail = Path::new(|b| {
                for k in 0..9 {
                    let p = orbit_point(theta - k as f32 * 0.055);
                    b.circle(p, (3.2 - k as f32 * 0.32).max(0.4));
                }
            });
            frame.fill(&trail, fade(accent, 0.20 * lead * boot));
            frame.fill(&Path::circle(orbit_point(theta), 3.0), fade(hot, lead * boot));
        }

        // -- Core: halo stack, dashed iris, bass-driven disc ------------------
        let breathe = (self.phase * 1.7).sin() * 0.5 + 0.5;
        let amp = match self.mode {
            Mode::Speaking | Mode::Listening => 0.10 * breathe + bass * 1.1 + self.beat * 0.35,
            _ => 0.10 * breathe + self.beat * 0.2,
        };
        let core_r = r_max * 0.13 * (1.0 + amp);
        for (scale, a) in [(3.6, 0.05), (2.6, 0.08), (1.8, 0.14), (1.25, 0.22)] {
            frame.fill(&Path::circle(center, core_r * scale), fade(accent, a * boot));
        }
        // Iris: short dashes on a counter-rotating ring, gapped like an aperture.
        let iris_r = core_r * 1.6;
        let iris = Path::new(|b| {
            for k in 0..18 {
                let a = k as f32 * TAU / 18.0 - spin * 0.8;
                b.move_to(at(iris_r, a));
                b.line_to(at(iris_r + 3.5 + 6.0 * band_at(self.bands, k as f32 / 18.0), a));
            }
        });
        frame.stroke(
            &iris,
            Stroke::default().with_color(fade(ink(HOLO_CYAN), 0.7 * boot)).with_width(1.6),
        );
        frame.fill(&Path::circle(center, core_r), fade(accent, boot));
        frame.fill(&Path::circle(center, core_r * 0.45), fade(hot, (0.5 + self.beat * 0.5) * boot));
        // Anamorphic flare on a transient — the "it heard that" tell.
        if self.beat > 0.02 {
            let f = self.beat * r_max * 1.5;
            let flare = Path::new(|b| {
                b.move_to(Point::new(center.x - f, center.y));
                b.line_to(Point::new(center.x + f, center.y));
                b.move_to(Point::new(center.x, center.y - f * 0.35));
                b.line_to(Point::new(center.x, center.y + f * 0.35));
            });
            frame.stroke(
                &flare,
                Stroke::default()
                    .with_color(fade(hot, self.beat * 0.5))
                    .with_width(1.2)
                    .with_line_cap(LineCap::Round),
            );
        }

        // -- Thinking: a radar wedge sweeps the field ------------------------
        if self.mode == Mode::Thinking {
            let sweep = Path::new(|b| {
                for k in 0..14 {
                    let a = spin - k as f32 * 0.045;
                    b.move_to(center);
                    b.line_to(at(r_max * 1.05, a));
                }
            });
            frame.stroke(
                &sweep,
                Stroke::default().with_color(fade(ink(t.warning), 0.12)).with_width(1.0),
            );
            let edge = Path::line(center, at(r_max * 1.05, spin));
            glow(frame, &edge, fade(ink(t.warning), 0.8), 1.4);
        }

        // -- Chrome: brackets, telemetry, input meter -------------------------
        let m = 8.0;
        // The frame twitches a pixel while thinking; a perfectly still HUD
        // reads as a screenshot.
        let jitter = if self.mode == Mode::Thinking {
            ((self.phase * 37.0).sin() * 1.2).round()
        } else {
            0.0
        };
        let len = 18.0 + 6.0 * energy;
        let corners = [
            (m + jitter, m, 1.0, 1.0),
            (w - m + jitter, m, -1.0, 1.0),
            (m - jitter, h - m, 1.0, -1.0),
            (w - m - jitter, h - m, -1.0, -1.0),
        ];
        let brackets = Path::new(|b| {
            for (x, y, dx, dy) in corners {
                b.move_to(Point::new(x + len * dx, y));
                b.line_to(Point::new(x, y));
                b.line_to(Point::new(x, y + len * dy));
            }
        });
        frame.stroke(
            &brackets,
            Stroke::default().with_color(fade(holo, boot)).with_width(1.5),
        );

        // Mic live: explicit input meter, bottom-right — a row of bars that
        // light up with mic level, so "is it hearing me" has one obvious answer.
        // Shown whenever the mic is open, not only mid-utterance: a live mic
        // the user cannot see is the one thing this screen must never do.
        if matches!(self.mode, Mode::Listening | Mode::Armed) {
            const METER: usize = 12;
            let lit = (self.level * METER as f32).ceil() as usize;
            let (bw, gap, bh) = (6.0, 3.0, 14.0);
            let x0 = w - m - METER as f32 * (bw + gap);
            let y0 = h - m - bh - 22.0;
            for i in 0..METER {
                let x = x0 + i as f32 * (bw + gap);
                let bar_h = bh * (0.4 + 0.6 * (i as f32 / METER as f32));
                let bar =
                    Path::rectangle(Point::new(x, y0 + (bh - bar_h)), iced::Size::new(bw, bar_h));
                let lit_c = ink(if self.mode == Mode::Listening { SPIDEY_RED } else { t.success });
                frame.fill(&bar, if i < lit { lit_c } else { fade(web, 0.5) });
            }
            frame.fill_text(mono("INPUT".into(), Point::new(x0, y0 - 14.0), holo, 10.0));
        }

        // Telemetry. Real numbers, not set dressing: level in dB, the loudest
        // band's centre frequency, and time in the current mode.
        let db = 20.0 * (self.energy.max(1e-4)).log10();
        let peak_bin = self
            .bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map_or(0, |(i, _)| i);
        let peak_hz = crate::stt::band_freq(peak_bin, BANDS);
        let secs = self.elapsed as u32;
        // The noise floor is telemetry, not decoration: when the gate refuses to
        // open in a loud room, this is the number that explains why.
        let nf = 20.0 * (self.floor.max(1e-4)).log10();
        let readouts = [
            format!("LVL {db:>6.1} dB"),
            if self.energy > 0.02 {
                format!("PK  {peak_hz:>6.0} Hz")
            } else {
                "PK       — Hz".to_string()
            },
            format!("NF  {nf:>6.1} dB"),
            // Voice ID: the number the speaker check actually decided on, so
            // its threshold can be judged against your own voice.
            match self.voice_sim {
                Some(sim) => format!("VID {sim:>6.2}"),
                None => "VID      —".to_string(),
            },
            format!("T+  {:02}:{:02}", secs / 60, secs % 60),
        ];
        for (i, line) in readouts.iter().enumerate() {
            frame.fill_text(mono(
                line.clone(),
                Point::new(w - m - 96.0, m + 8.0 + i as f32 * 13.0),
                fade(holo, 0.85),
                10.0,
            ));
        }
        // A packet counter that actually counts: frames of audio analyzed.
        frame.fill_text(mono(
            format!("PKT {:06X}", (self.phase * 60.0) as u32 & 0xFF_FFFF),
            Point::new(m + 6.0, m + 8.0),
            fade(holo, 0.55),
            10.0,
        ));
        frame.fill_text(mono(
            format!("SPECTRUM {BANDS}CH · GOERTZEL · 60HZ"),
            Point::new(m + 6.0, m + 21.0),
            fade(holo, 0.35),
            10.0,
        ));

        frame.fill_text(mono(
            format!(
                "{} // {}",
                crate::assistant::name().to_uppercase(),
                match self.mode {
                    Mode::Idle => "STANDING BY",
                    Mode::Armed => "MIC LIVE · GATE SHUT",
                    Mode::Listening => "LISTENING",
                    Mode::Thinking => "ANALYZING…",
                    Mode::Speaking => "TRANSMITTING",
                }
            ),
            // Above the ribbon: the status line is the one thing that must stay
            // readable at a glance.
            Point::new(m + 6.0, h - m - 38.0),
            mix_color(holo, accent, 0.35),
            12.0,
        ));
    }

    /// The default animation: a pastel disc inside a thin ring with one hot arc,
    /// and the state typed out underneath.
    ///
    /// Built against measurements of a reference loop rather than by eye, and
    /// three of those measurements overturned the obvious guesses. The band is
    /// *one* hue whose intensity falls off around the circumference — not a
    /// rainbow travelling around it; the hue turns over time instead. The
    /// interior is a near-opaque pastel wash cycling pink → lavender → mint, not
    /// a white centre. And the band is thin: half-max width lands near a tenth
    /// of the radius.
    fn draw_bubble(&self, frame: &mut Frame, iced_theme: &Theme, bounds: Rectangle) {
        use std::f32::consts::TAU;

        let t = theme::tokens(iced_theme);
        let hue = |m: Mode| match m {
            Mode::Idle => mix_color(SPIDEY_BLUE, HOLO_CYAN, 0.35),
            // An open mic is never the same colour as an idle one, here either.
            Mode::Armed => t.success,
            Mode::Listening => SPIDEY_RED,
            Mode::Thinking => t.warning,
            Mode::Speaking => HOLO_CYAN,
        };
        let ink = |c: Color| if t.dark { c } else { mix_color(c, Color::BLACK, 0.22) };
        let accent = ink(mix_color(hue(self.prev), hue(self.mode), self.mix));
        // The two the hue turns through. Measured, the reference sits around a
        // vivid blue-violet and drifts to pink; the mode's own colour anchors it
        // so Listening still reads red rather than always violet.
        let cool = ink(mix_color(accent, HOLO_CYAN, 0.5));
        let warm = ink(mix_color(accent, Color::from_rgb(1.0, 0.42, 0.82), 0.6));
        // What the interior washes toward: the backdrop this canvas sits on.
        let paper =
            if t.dark { Color::from_rgb(0.04, 0.05, 0.08) } else { Color::from_rgb(0.99, 0.99, 1.0) };
        let pastel = |c: Color| mix_color(c, paper, if t.dark { 0.55 } else { 0.60 });

        let (w, h) = (bounds.width, bounds.height);
        let center = Point::new(w / 2.0, h / 2.0 - h * 0.05);
        let boot = self.boot;
        let energy = self.energy.clamp(0.0, 1.0);
        let bass = band_at(self.bands, 0.08);
        let breathe = 0.5 + 0.5 * (self.phase * 0.55).sin();
        // Thinking turns the hue and the hot arc faster, rather than bolting a
        // spinner onto a still shape.
        let ph = self.phase * if self.mode == Mode::Thinking { 1.9 } else { 1.0 };
        // Leaves room under the disc for the status line, which is part of the
        // composition rather than a caption bolted beneath it.
        let unit = (w.min(h) * 0.5 - 10.0).max(10.0);
        let r = unit
            * (0.60 + 0.02 * breathe + 0.05 * energy + 0.03 * bass + 0.02 * self.beat).min(0.70)
            * (0.7 + 0.3 * boot);

        // Barely there: measured, the reference's edge stays within about a tenth
        // of its radius. The life is in the colour, not in the outline.
        let edge = |a: f32| {
            let amp = 0.010 + 0.025 * energy + 0.012 * bass;
            1.0 + amp
                * ((a * 2.0 + ph * 0.29).sin()
                    + 0.6 * (a * 3.0 - ph * 0.43).sin()
                    + 0.3 * (a * 5.0 + ph * 0.19).sin())
        };
        // Spectrum around the band, mirrored left to right so it reads as
        // designed rather than as noise.
        let swell = |f: f32| {
            let x = if f <= 0.5 { f * 2.0 } else { (1.0 - f) * 2.0 };
            band_at(self.bands, x)
        };

        // -- Interior: a pastel wash, near-opaque -------------------------------
        // Continuous gradients, never stacked fills: alpha stacked in steps is
        // what puts countable contour rings inside a soft shape. The two hues are
        // one turn of the wheel apart and both crawl, so the disc keeps shifting
        // without ever landing on a colour it just held.
        let wash_a = pastel(wheel(accent, cool, warm, ph * 0.035));
        let wash_b = pastel(wheel(accent, cool, warm, ph * 0.035 + 0.28));
        let axis = ph * 0.07;
        let (ax, ay) = (r * axis.cos(), r * axis.sin());
        frame.fill(
            &Path::circle(center, r * 0.99),
            iced::advanced::graphics::gradient::Linear::new(
                Point::new(center.x - ax, center.y - ay),
                Point::new(center.x + ax, center.y + ay),
            )
            .add_stop(0.0, fade(wash_a, 0.90 * boot))
            .add_stop(0.5, fade(mix_color(wash_a, wash_b, 0.5), 0.80 * boot))
            .add_stop(1.0, fade(wash_b, 0.90 * boot)),
        );
        // Two soft masses drifting inside it. This is the liquid: the interior
        // sliding under a still rim, which is what the reference does and what a
        // deforming outline does not.
        for n in 0..2 {
            let n = n as f32;
            let a = ph * (0.13 + n * 0.08) + n * 2.7;
            let d = r * 0.26 * (0.5 + 0.5 * (ph * 0.11 + n * 1.9).sin());
            let c = Point::new(center.x + d * a.cos(), center.y + d * a.sin());
            let rad = r * (0.55 + 0.12 * (ph * 0.17 + n * 2.0).sin());
            let tint = pastel(wheel(accent, cool, warm, ph * 0.035 + 0.55 + n * 0.2));
            let blob_axis = ph * 0.09 + n * 2.2;
            let (bx, by) = (rad * blob_axis.cos(), rad * blob_axis.sin());
            frame.fill(
                &Path::circle(c, rad),
                iced::advanced::graphics::gradient::Linear::new(
                    Point::new(c.x - bx, c.y - by),
                    Point::new(c.x + bx, c.y + by),
                )
                .add_stop(0.0, fade(tint, 0.0))
                .add_stop(0.5, fade(tint, (0.40 + 0.25 * energy) * boot))
                .add_stop(1.0, fade(tint, 0.0)),
            );
        }

        // -- The band -----------------------------------------------------------
        // One hue, one hot arc. Measured, the reference's chroma runs about 5:1
        // between its brightest point and the far side — so this is an intensity
        // gradient around the ring, and the hue itself turns on the clock.
        let live = wheel(accent, cool, warm, ph * 0.045);
        let faint = mix_color(live, paper, 0.72);
        let hot = ph * 0.06;
        const SEGS: usize = 160;
        let seg = |i: usize, of: usize, spread: f32| {
            let a0 = i as f32 * TAU / of as f32;
            let step = TAU / of as f32;
            Path::new(|b| {
                // Overlapping into the next segment, so the seams close.
                for k in 0..=3 {
                    let a = a0 + step * k as f32 / 3.0 * spread;
                    let rr = r * edge(a);
                    let p = Point::new(center.x + rr * a.cos(), center.y + rr * a.sin());
                    if k == 0 {
                        b.move_to(p);
                    } else {
                        b.line_to(p);
                    }
                }
            })
        };
        for pass in 0..2 {
            // Haze first, then the band on top of it. The haze is what stops the
            // ring sitting on the page like something drawn with a compass.
            let (wide, dim) = if pass == 0 { (3.2, 0.16) } else { (1.0, 1.0) };
            for i in 0..SEGS {
                let f = i as f32 / SEGS as f32;
                let d = (f - hot).rem_euclid(1.0);
                // 1 at the hot point, 0 at the far side, tightened so the arc
                // stays an arc rather than a slow global brightening.
                let heat = (1.0 - d.min(1.0 - d) * 2.0).powf(1.6);
                let width = r * (0.045 + 0.085 * heat + 0.075 * swell(f) + 0.02 * self.beat);
                frame.stroke(
                    &seg(i, SEGS, 1.6),
                    Stroke::default()
                        .with_color(fade(
                            mix_color(faint, live, heat),
                            (0.18 + 0.82 * heat) * dim * boot,
                        ))
                        .with_width(width * wide)
                        .with_line_cap(LineCap::Round),
                );
            }
        }

        // Talking sheds the band outward — the tell that this is driven by sound
        // and not by a timer.
        if energy > 0.02 && matches!(self.mode, Mode::Speaking | Mode::Listening) {
            for k in 0..3 {
                let p = (ph * 0.45 + k as f32 / 3.0).fract();
                let out = 1.0 - p;
                frame.stroke(
                    &Path::circle(center, r * (1.03 + p * 0.5)),
                    Stroke::default()
                        .with_color(fade(live, out * out * 0.30 * energy * boot))
                        .with_width(1.0 + 2.5 * out),
                );
            }
        }

        // -- The status line ------------------------------------------------------
        // Typed out left to right at its final centred position, the way the
        // reference reveals its caption. `elapsed` resets on every mode change,
        // so each new state types itself in without any extra state to keep.
        let shown = typed(mode_label(self.mode), self.elapsed);
        if !shown.is_empty() {
            let size = (h * 0.085).clamp(13.0, 26.0);
            frame.fill_text(Text {
                content: shown.to_string(),
                position: Point::new(center.x, (center.y + r * 1.42).min(h - size)),
                color: fade(ink(mix_color(live, Color::BLACK, if t.dark { 0.0 } else { 0.45 })), boot),
                size: size.into(),
                align_x: iced::alignment::Horizontal::Center.into(),
                align_y: iced::alignment::Vertical::Center,
                ..Text::default()
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The band reads its colour off `wheel` per segment, so any jump in it —
    /// at a stop or across the wrap — draws a hard seam across the ring.
    #[test]
    fn the_colour_wheel_closes_on_itself() {
        let (a, b, c) = (
            Color::from_rgb(0.9, 0.1, 0.2),
            Color::from_rgb(0.1, 0.8, 0.3),
            Color::from_rgb(0.2, 0.3, 1.0),
        );
        let near = |x: Color, y: Color, what: &str| {
            let d = (x.r - y.r).abs() + (x.g - y.g).abs() + (x.b - y.b).abs();
            assert!(d < 0.02, "{what}: {x:?} vs {y:?}");
        };
        // Wraps: just under 1.0 is just under a full turn, so it lands on `a`.
        near(wheel(a, b, c, 0.9999), a, "wrap");
        near(wheel(a, b, c, 0.0), a, "start");
        // And the same turn one lap along is the same colour.
        near(wheel(a, b, c, 0.37), wheel(a, b, c, 1.37), "periodic");
        // Each stop is hit exactly, from both sides.
        near(wheel(a, b, c, 1.0 / 3.0), b, "stop b");
        near(wheel(a, b, c, 2.0 / 3.0), c, "stop c");
        near(wheel(a, b, c, 0.3333 - 0.0005), b, "approach b");
    }

    /// The status line types itself in, and it is sliced every frame — on
    /// characters, never bytes, or a multi-byte label panics mid-render.
    #[test]
    fn the_status_line_types_in_without_splitting_a_character() {
        assert_eq!(typed("Listening", 0.0), "");
        assert_eq!(typed("Listening", 0.1), "L");
        assert_eq!(typed("Listening", 10.0), "Listening");
        // Past the end stays put rather than running off it.
        assert_eq!(typed("Hi", 1e6), "Hi");
        // Accented characters advance one character at a time, not one byte.
        let s = "\u{c9}coute";
        for n in 0..=6 {
            let out = typed(s, n as f32 / 18.0);
            assert_eq!(out.chars().count(), n.min(6), "{out:?}");
            assert!(s.starts_with(out));
        }
    }
}
