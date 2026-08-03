//! E.V. screen: suit-AI heads-up display — radial web backdrop, rotating
//! targeting reticle, pulsing core, corner brackets — transcript below,
//! composer at the bottom. The HUD is the only custom canvas outside the DAG;
//! everything else composes the ui kit.

use crate::assistant::{Message, Mode, State};
use crate::ui::{self, space, theme, Tone};
use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke, Text};
use iced::widget::{canvas as canvas_widget, column, container, scrollable};
use iced::{mouse, Element, Length, Point, Radians, Rectangle, Renderer, Theme};

pub fn view(state: &State) -> Element<'_, Message> {
    let mode = state.mode();
    let hud = container(
        canvas_widget(Hud { phase: state.phase, mode })
            .width(Length::Fill)
            .height(170),
    )
    .style(theme::code_block)
    .width(Length::Fill);

    let status = ui::cluster(vec![
        ui::badge(
            match mode {
                Mode::Idle => "SYSTEMS NOMINAL",
                Mode::Listening => "LISTENING",
                Mode::Thinking => "ANALYZING",
                Mode::Speaking => "TRANSMITTING",
            },
            match mode {
                Mode::Idle => Tone::Success,
                Mode::Listening => Tone::Danger,
                Mode::Thinking => Tone::Warning,
                Mode::Speaking => Tone::Info,
            },
        ),
        ui::caption(format!("VOICE {}", if state.voice { "ON" } else { "MUTED" })),
    ])
    .into();

    let transcript: Element<'_, Message> = if state.messages.is_empty() {
        ui::empty_state("Web-shooters primed. What do you need?")
    } else {
        let turns: Vec<Element<'_, Message>> = state
            .messages
            .iter()
            .map(|m| {
                let (label, tone) = match m.role.as_str() {
                    "user" => ("YOU", Tone::Neutral),
                    _ => ("E.V.", Tone::Danger),
                };
                ui::card(ui::stack(vec![
                    ui::badge(label, tone),
                    ui::body(m.content.clone()),
                ]))
            })
            .collect();
        scrollable(ui::stack(turns)).height(Length::Fill).anchor_bottom().into()
    };

    let composer_row: Element<'_, Message> = ui::cluster(vec![
            container(ui::input("Talk to E.V.…", &state.draft, Message::DraftChanged))
                .width(Length::Fill)
                .into(),
            if state.listening {
                ui::badge("listening…", Tone::Danger)
            } else {
                ui::button_secondary("🎤 Talk", Message::Listen)
            },
            ui::button_ghost(if state.voice { "Mute" } else { "Unmute" }, Message::ToggleVoice),
            if state.sending {
                ui::badge("…", Tone::Warning)
            } else {
                ui::button_default("Send", Message::Send)
            },
    ])
    .into();
    let composer = ui::card(composer_row);

    let mut blocks: Vec<Element<'_, Message>> = vec![hud.into(), status];
    if let Some(err) = &state.error {
        let mut row = vec![container(ui::alert_error(err.clone())).width(Length::Fill).into()];
        if err.contains("Privacy → Speech") {
            row.push(ui::button_secondary("Open Settings", Message::OpenSpeechSettings));
        }
        row.push(ui::button_ghost("Dismiss", Message::DismissError));
        blocks.push(ui::cluster(row).into());
    }
    blocks.push(container(transcript).height(Length::Fill).into());
    blocks.push(composer);

    ui::page(
        "E.V.",
        Some(ui::muted("Onboard suit AI. Replies are spoken unless muted.")),
        Some(ui::button_outline("Clear", Message::Clear)),
        {
            let body: Element<'_, Message> =
                column(blocks).spacing(space::MD).height(Length::Fill).into();
            body
        },
    )
}

// ---------------------------------------------------------------------------
// HUD canvas — radial web, rotating reticle, pulsing core, corner brackets.
// ---------------------------------------------------------------------------

const SPOKES: usize = 12;

// Suit palette, deliberately not theme tokens: the HUD is E.V.'s territory and
// keeps the classic red/blue in both light and dark themes.
const SPIDEY_RED: iced::Color = iced::Color::from_rgb(0.902, 0.169, 0.180); // #E62B2E
const SPIDEY_BLUE: iced::Color = iced::Color::from_rgb(0.169, 0.310, 0.686); // #2B4FAF
const HOLO_CYAN: iced::Color = iced::Color::from_rgb(0.208, 0.816, 1.0); // #35D0FF

struct Hud {
    phase: f32,
    mode: Mode,
}

impl canvas::Program<Message> for Hud {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        iced_theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let t = theme::tokens(iced_theme);
        let web = iced::Color { a: if t.dark { 0.45 } else { 0.55 }, ..SPIDEY_RED };
        let holo = iced::Color { a: 0.75, ..HOLO_CYAN };
        let core_color = match self.mode {
            Mode::Idle => SPIDEY_RED,
            Mode::Listening => SPIDEY_RED,
            Mode::Thinking => t.warning,
            Mode::Speaking => HOLO_CYAN,
        };

        let mut frame = Frame::new(renderer, bounds.size());
        let center = Point::new(bounds.width / 2.0, bounds.height / 2.0);
        let r_max = (bounds.height / 2.0) - 10.0;
        let speed = if self.mode == Mode::Thinking { 3.0 } else { 1.0 };
        let p = self.phase * speed;

        let at = |radius: f32, angle: f32| {
            Point::new(center.x + radius * angle.cos(), center.y + radius * angle.sin())
        };

        // Radial web backdrop: spokes plus sagging rings (control point pulled
        // toward center so the strands read as webbing, not circles).
        for k in 0..SPOKES {
            let a = k as f32 * std::f32::consts::TAU / SPOKES as f32;
            let line = Path::line(at(r_max * 0.14, a), at(r_max, a));
            frame.stroke(&line, Stroke::default().with_color(web).with_width(1.0));
        }
        for ring in [0.35_f32, 0.55, 0.75, 0.95] {
            let r = r_max * ring;
            let path = Path::new(|b| {
                b.move_to(at(r, 0.0));
                for k in 1..=SPOKES {
                    let a1 = k as f32 * std::f32::consts::TAU / SPOKES as f32;
                    let a_mid = a1 - std::f32::consts::PI / SPOKES as f32;
                    b.quadratic_curve_to(at(r * 0.93, a_mid), at(r, a1));
                }
            });
            frame.stroke(&path, Stroke::default().with_color(web).with_width(1.0));
        }

        // Targeting reticle: two rotating arc pairs (outer suit-blue, inner
        // holo-cyan) + fixed crosshair ticks.
        for (radius_f, sweep, width, dir, color) in [
            (0.62_f32, 1.1_f32, 2.2_f32, 1.0_f32, SPIDEY_BLUE),
            (0.46, 0.7, 1.4, -1.0, holo),
        ] {
            let r = r_max * radius_f;
            for half in [0.0, std::f32::consts::PI] {
                let start = p * dir + half;
                let arc = Path::new(|b| {
                    b.arc(canvas::path::Arc {
                        center,
                        radius: r,
                        start_angle: Radians(start),
                        end_angle: Radians(start + sweep),
                    });
                });
                frame.stroke(&arc, Stroke::default().with_color(color).with_width(width));
            }
        }
        for a in [0.0_f32, 0.5, 1.0, 1.5].map(|q| q * std::f32::consts::PI) {
            let tick = Path::line(at(r_max * 0.30, a), at(r_max * 0.38, a));
            frame.stroke(&tick, Stroke::default().with_color(holo).with_width(1.5));
        }

        // Core: pulsing filled disc with a soft halo.
        let pulse = (p * 2.0).sin() * 0.5 + 0.5;
        let amp = match self.mode {
            Mode::Speaking => 0.5,
            Mode::Listening => 0.35, // visibly "breathing" while the mic is hot
            _ => 0.15,
        };
        let core_r = r_max * 0.16 * (1.0 + amp * pulse);
        frame.fill(&Path::circle(center, core_r * 2.0), iced::Color { a: 0.12, ..core_color });
        frame.fill(&Path::circle(center, core_r), core_color);

        // HUD corner brackets.
        let m = 8.0;
        let len = 18.0;
        let corners = [
            (m, m, 1.0, 1.0),
            (bounds.width - m, m, -1.0, 1.0),
            (m, bounds.height - m, 1.0, -1.0),
            (bounds.width - m, bounds.height - m, -1.0, -1.0),
        ];
        for (x, y, dx, dy) in corners {
            let bracket = Path::new(|b| {
                b.move_to(Point::new(x + len * dx, y));
                b.line_to(Point::new(x, y));
                b.line_to(Point::new(x, y + len * dy));
            });
            frame.stroke(&bracket, Stroke::default().with_color(holo).with_width(1.5));
        }

        frame.fill_text(Text {
            content: match self.mode {
                Mode::Idle => "E.V. // STANDING BY".to_string(),
                Mode::Listening => "E.V. // LISTENING".to_string(),
                Mode::Thinking => "E.V. // ANALYZING…".to_string(),
                Mode::Speaking => "E.V. // TRANSMITTING".to_string(),
            },
            position: Point::new(m + 6.0, bounds.height - m - 20.0),
            color: holo,
            size: 12.0.into(),
            font: iced::Font::MONOSPACE,
            ..Text::default()
        });

        vec![frame.into_geometry()]
    }
}
