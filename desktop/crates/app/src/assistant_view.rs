//! E.V. screen: suit-AI heads-up display — a live spectrum analyzer wearing a
//! spider-web, a rotating targeting reticle, an orbiting sensor pair and a
//! reactor core that thumps on the bass of whoever is talking — transcript
//! below, composer at the bottom. The HUD is the only custom canvas outside
//! the DAG; everything else composes the ui kit.
//!
//! Everything here is driven by real audio: `assistant::Tick` runs a Goertzel
//! band pass over the mic (or over E.V.'s own playback) at 60 fps, so nothing
//! on screen is a canned loop — the web spokes *are* the spectrum bins.

use crate::assistant::{Message, Mode, State, BANDS, WAVE};
use crate::ui::{self, space, theme, Icon, Tone};
use iced::widget::canvas::{self, Frame, Geometry, LineCap, Path, Stroke, Text};
use iced::widget::{canvas as canvas_widget, column, container, markdown, scrollable};
use iced::{mouse, Color, Element, Length, Point, Radians, Rectangle, Renderer, Theme};

/// The live HUD canvas alone, for embedding outside this screen (the
/// Dashboard). Whoever shows it must also run the `assistant::Tick`
/// subscription, or the canvas freezes.
pub fn hud(state: &State, height: f32) -> Element<'_, Message> {
    container(
        canvas_widget(Hud {
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
        .height(height),
    )
    .style(theme::code_block)
    .width(Length::Fill)
    .into()
}

pub fn view<'a>(state: &'a State, iced_theme: &Theme) -> Element<'a, Message> {
    let mode = state.mode();
    let hud = hud(state, 224.0);

    let status = ui::cluster(vec![
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
        ui::caption(format!("VOICE {}", if state.voice { "ON" } else { "MUTED" })),
        // Voice ID is a filter on who gets answered, so its state is never
        // hidden: learning, locked on, or told this was someone else.
        match (state.voice_enrolled(), state.voice_sim) {
            (false, _) if state.armed() => ui::caption("Learning your voice…".to_string()),
            (true, Some(sim)) if sim < crate::assistant::VOICE_MATCH => {
                ui::caption("Different voice — parked, not sent".to_string())
            }
            (true, _) => ui::caption("Voice ID locked".to_string()),
            _ => ui::caption(String::new()),
        },
        // Hands-free answers whatever it hears, so say out loud what gets it to
        // answer: being named, or replying inside the follow-up window.
        ui::caption(if state.armed() {
            "Say “E.V., …” — or just talk right after a reply".to_string()
        } else {
            "Hands-free is off".to_string()
        }),
    ])
    .into();

    let transcript: Element<'_, Message> = if state.messages.is_empty() {
        ui::empty_state("Web-shooters primed. What do you need?")
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
                    // The terminal's answer to a run_command call — fenced output.
                    "tool" => ("TERMINAL", Tone::Info),
                    _ => ("E.V.", Tone::Danger),
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
        scrollable(
            ui::stack_lg(turns)
                .padding(iced::Padding { right: 12.0, ..Default::default() }),
        )
        .id(crate::assistant::transcript_id())
        .height(Length::Fill)
        .into()
    };

    let composer_row: Element<'_, Message> = ui::cluster(vec![
            container(ui::input_submit(
                "Talk to E.V.…",
                &state.draft,
                Message::DraftChanged,
                Message::Send,
            ))
            .width(Length::Fill)
            .into(),
            if state.armed() {
                ui::button_destructive(Icon::MicOff, "Mic off", Message::Listen)
            } else {
                ui::button_secondary(Icon::Mic, "Hands-free", Message::Listen)
            },
            if state.voice {
                ui::button_ghost(Icon::Volume, "Mute", Message::ToggleVoice)
            } else {
                ui::button_ghost(Icon::VolumeOff, "Unmute", Message::ToggleVoice)
            },
            // Only offered once there is something to forget — wrong person
            // enrolled, or a new mic that changed how you sound.
            if state.voice_enrolled() {
                ui::button_ghost(Icon::XCircle, "Forget voice", Message::ForgetVoice)
            } else {
                iced::widget::Space::new().into()
            },
            if state.sending {
                ui::badge("…", Tone::Warning)
            } else {
                ui::button_default(Icon::Send, "Send", Message::Send)
            },
    ])
    .into();
    let composer = ui::card(composer_row);

    let mut blocks: Vec<Element<'_, Message>> = vec![hud.into(), status];
    if let Some(err) = &state.error {
        let mut row = vec![container(ui::alert_error(err.clone())).width(Length::Fill).into()];
        if err.contains("Privacy → Microphone") {
            row.push(ui::button_secondary(Icon::Settings, "Open Settings", Message::OpenMicSettings));
        }
        row.push(ui::button_ghost(Icon::X, "Dismiss", Message::DismissError));
        blocks.push(ui::cluster(row).into());
    }
    blocks.push(container(transcript).height(Length::Fill).into());
    blocks.push(composer);

    ui::page_fixed(
        "E.V.",
        Some(ui::muted("Onboard suit AI. Replies are spoken unless muted.")),
        Some(ui::button_outline(Icon::Trash, "Clear", Message::Clear)),
        {
            let body: Element<'_, Message> =
                column(blocks).spacing(space::MD).height(Length::Fill).into();
            body
        },
    )
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
fn band_at(bands: &[f32], x: f32) -> f32 {
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
        // Colour crosses the mode change instead of cutting to it.
        let accent = mix_color(hue(self.prev), hue(self.mode), self.mix);
        let holo = fade(HOLO_CYAN, 0.75);
        let web = Color { a: if t.dark { 0.40 } else { 0.55 }, ..SPIDEY_RED };

        let mut frame = Frame::new(renderer, bounds.size());
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
                Stroke::default().with_color(fade(SPIDEY_RED, e * boot)).with_width(1.0 + e),
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
            glow(&mut frame, &glint, fade(HOT, hot_e * (1.0 - travel)), 1.6);
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
                .with_color(fade(mix_color(accent, HOLO_CYAN, 0.35), 0.85 * boot))
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
            glow(&mut frame, &bar, fade(mix_color(accent, HOT, e - 0.4), boot), 1.8);
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
            frame.fill(&Path::circle(orbit_point(theta), 3.0), fade(HOT, lead * boot));
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
            Stroke::default().with_color(fade(HOLO_CYAN, 0.7 * boot)).with_width(1.6),
        );
        frame.fill(&Path::circle(center, core_r), fade(accent, boot));
        frame.fill(&Path::circle(center, core_r * 0.45), fade(HOT, (0.5 + self.beat * 0.5) * boot));
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
                    .with_color(fade(HOT, self.beat * 0.5))
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
                Stroke::default().with_color(fade(t.warning, 0.12)).with_width(1.0),
            );
            let edge = Path::line(center, at(r_max * 1.05, spin));
            glow(&mut frame, &edge, fade(t.warning, 0.8), 1.4);
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
                let hot = if self.mode == Mode::Listening { SPIDEY_RED } else { t.success };
                frame.fill(&bar, if i < lit { hot } else { fade(web, 0.5) });
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
            match self.mode {
                Mode::Idle => "E.V. // STANDING BY".to_string(),
                Mode::Armed => "E.V. // MIC LIVE · GATE SHUT".to_string(),
                Mode::Listening => "E.V. // LISTENING".to_string(),
                Mode::Thinking => "E.V. // ANALYZING…".to_string(),
                Mode::Speaking => "E.V. // TRANSMITTING".to_string(),
            },
            // Above the ribbon: the status line is the one thing that must stay
            // readable at a glance.
            Point::new(m + 6.0, h - m - 38.0),
            mix_color(holo, accent, 0.35),
            12.0,
        ));

        vec![frame.into_geometry()]
    }
}
