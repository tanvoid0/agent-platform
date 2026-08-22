//! Design tokens: shadcn/ui's structure, LM Studio's dark palette.
//!
//! Semantic names (`background`, `card`, `muted_foreground`, `border`, …) and the
//! spacing/radius/type scales below are shadcn's, not iced's — screens use these,
//! never raw colors. The light block is still shadcn's default (zinc) `:root`
//! verbatim. Dark fills still step up (canvas → card → popover) so a panel has
//! somewhere to sit; [`card`] lifts off the canvas with a shadow, not a hairline.
//! Inputs and separators keep the hairline. See [`dark_tokens`].

use iced::widget::{button, container, text, text_input};
use iced::{Background, Border, Color, Shadow, Theme, Vector};

/// shadcn spacing: Tailwind's 4px scale.
pub mod space {
    pub const XS: f32 = 4.0; // gap-1
    pub const SM: f32 = 8.0; // gap-2
    pub const MD: f32 = 16.0; // gap-4
    pub const LG: f32 = 24.0; // gap-6
    pub const XL: f32 = 32.0; // gap-8
}

/// shadcn `--radius: 0.5rem`, with the derived sm/lg steps.
pub mod radius {
    pub const SM: f32 = 4.0; // calc(var(--radius) - 4px)
    pub const MD: f32 = 6.0; // calc(var(--radius) - 2px)
    pub const LG: f32 = 8.0; // var(--radius)
    pub const XL: f32 = 12.0; // elevated surfaces (cards)
    pub const PILL: f32 = 999.0;
}

/// Tailwind type scale used by shadcn components.
pub mod font {
    pub const XS: f32 = 12.0; // text-xs — badges, captions
    pub const SM: f32 = 14.0; // text-sm — body, buttons, inputs
    pub const BASE: f32 = 16.0;
    pub const LG: f32 = 18.0; // card titles
    pub const XL2: f32 = 24.0; // page titles

    /// Page and card titles. Body stays `Font::DEFAULT` so weight, not size,
    /// is what separates a heading from a sentence.
    pub const SEMIBOLD: iced::Font = iced::Font {
        family: iced::font::Family::SansSerif,
        weight: iced::font::Weight::Semibold,
        stretch: iced::font::Stretch::Normal,
        style: iced::font::Style::Normal,
    };
}

fn hsl(h: f32, s: f32, l: f32) -> Color {
    // shadcn tokens are authored as `H S% L%`; convert once here.
    let (h, s, l) = (h / 360.0, s / 100.0, l / 100.0);
    if s == 0.0 {
        return Color::from_rgb(l, l, l);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |mut t: f32| {
        if t < 0.0 {
            t += 1.0
        }
        if t > 1.0 {
            t -= 1.0
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    Color::from_rgb(f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0))
}

/// One shadcn theme block (`:root` or `.dark`).
pub struct Tokens {
    pub background: Color,
    pub foreground: Color,
    pub card: Color,
    pub card_foreground: Color,
    pub popover: Color,
    pub primary: Color,
    pub primary_foreground: Color,
    pub secondary: Color,
    pub secondary_foreground: Color,
    pub muted: Color,
    pub muted_foreground: Color,
    pub accent: Color,
    pub destructive: Color,
    pub destructive_foreground: Color,
    pub success: Color,
    pub warning: Color,
    pub info: Color,
    pub border: Color,
    pub input: Color,
    pub ring: Color,
    pub dark: bool,
}

/// shadcn default (zinc) — `:root`.
fn light_tokens() -> Tokens {
    Tokens {
        // Off-white canvas so a white card can actually lift. Pure white-on-white
        // made the shadow the only edge, and it vanished in daylight.
        background: hsl(240.0, 6.0, 95.0),
        foreground: hsl(240.0, 10.0, 3.9),
        card: hsl(0.0, 0.0, 100.0),
        card_foreground: hsl(240.0, 10.0, 3.9),
        popover: hsl(0.0, 0.0, 100.0),
        primary: hsl(240.0, 5.9, 10.0),
        primary_foreground: hsl(0.0, 0.0, 98.0),
        secondary: hsl(240.0, 5.0, 92.0),
        secondary_foreground: hsl(240.0, 5.9, 10.0),
        muted: hsl(240.0, 5.0, 92.0),
        muted_foreground: hsl(240.0, 4.0, 40.0),
        accent: hsl(240.0, 5.0, 90.0),
        destructive: hsl(0.0, 72.0, 40.0),
        destructive_foreground: hsl(0.0, 0.0, 98.0),
        success: hsl(142.1, 76.2, 36.3),
        warning: hsl(37.7, 92.1, 50.2),
        info: hsl(221.2, 83.2, 53.3),
        border: hsl(240.0, 5.9, 50.0),
        input: hsl(240.0, 5.9, 50.0),
        ring: hsl(221.2, 83.2, 53.3),
        dark: false,
    }
}

/// Dark canvas with stepped fills. Cards lift with a shadow; inputs still use
/// the hairline (`border` lighter than every fill) so a field does not look
/// like a panel.
///
/// shadcn's `.dark` collapses `border`, `muted`, `secondary` and `accent` onto a
/// single value. That is fine when cards cast shadows, but a hovered row and an
/// input edge would then be the same color. Hence the split, and the test below
/// that keeps it.
fn dark_tokens() -> Tokens {
    Tokens {
        background: hsl(240.0, 5.0, 7.0), // canvas
        foreground: hsl(0.0, 0.0, 98.0),
        card: hsl(240.0, 5.0, 11.0), // panel — one clear step off the canvas
        card_foreground: hsl(0.0, 0.0, 98.0),
        popover: hsl(240.0, 5.0, 14.0), // floats above a panel
        primary: hsl(0.0, 0.0, 98.0),
        primary_foreground: hsl(240.0, 5.9, 10.0),
        secondary: hsl(240.0, 4.0, 15.0),
        secondary_foreground: hsl(0.0, 0.0, 98.0),
        muted: hsl(240.0, 4.0, 15.0),
        muted_foreground: hsl(240.0, 5.0, 64.9),
        accent: hsl(240.0, 4.0, 16.0), // hover / selected fill
        destructive: hsl(0.0, 62.8, 30.6),
        destructive_foreground: hsl(0.0, 0.0, 98.0),
        success: hsl(142.1, 70.6, 45.3),
        warning: hsl(47.9, 95.8, 53.1),
        // Restrained blue rather than Tailwind's blue-500: at this canvas
        // lightness a fully saturated accent is the only thing the eye lands on.
        info: hsl(217.0, 78.0, 62.0),
        // 20% L was a 1.3:1 ghost line on the card. 44% L is the UI 3:1 floor
        // and still sits above every fill, so depth stays a hairline not a shadow.
        border: hsl(240.0, 5.0, 44.0),
        input: hsl(240.0, 5.0, 44.0),
        // Focus reads as the accent. shadcn's near-white ring is a second bright
        // value competing with `foreground` on every focused field.
        ring: hsl(217.0, 78.0, 62.0),
        dark: true,
    }
}

/// The two iced themes the app runs; tokens are keyed off which one is active.
/// (An iced `Theme` cannot carry our token struct, so it is matched by name.)
pub const LIGHT_NAME: &str = "shadcn-light";
pub const DARK_NAME: &str = "shadcn-dark";

pub fn light_theme() -> Theme {
    let t = light_tokens();
    Theme::custom(
        LIGHT_NAME.to_string(),
        iced::theme::Palette {
            background: t.background,
            text: t.foreground,
            primary: t.info,
            success: t.success,
            warning: t.warning,
            danger: t.destructive,
        },
    )
}

pub fn dark_theme() -> Theme {
    let t = dark_tokens();
    Theme::custom(
        DARK_NAME.to_string(),
        iced::theme::Palette {
            background: t.background,
            text: t.foreground,
            primary: t.info,
            success: t.success,
            warning: t.warning,
            danger: t.destructive,
        },
    )
}

pub fn tokens(theme: &Theme) -> Tokens {
    if theme.to_string() == LIGHT_NAME {
        light_tokens()
    } else {
        dark_tokens()
    }
}

/// Semantic color roles for domain state. Screens pick a `Tone`, never a color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Info,
    Success,
    Warning,
    Danger,
}

pub(crate) fn tone_color(t: &Tokens, tone: Tone) -> Color {
    match tone {
        Tone::Neutral => t.muted_foreground,
        Tone::Info => t.info,
        Tone::Success => t.success,
        Tone::Warning => t.warning,
        // dark destructive is a button *background* token, unreadable as text
        // on near-black; buttons keep using t.destructive directly.
        Tone::Danger => if t.dark { hsl(0.0, 91.0, 71.0) } else { t.destructive },
    }
}

fn alpha(color: Color, a: f32) -> Color {
    Color { a, ..color }
}

// -- text -------------------------------------------------------------------

pub fn text_default(theme: &Theme) -> text::Style {
    text::Style { color: Some(tokens(theme).foreground) }
}

pub fn text_muted(theme: &Theme) -> text::Style {
    text::Style { color: Some(tokens(theme).muted_foreground) }
}

pub fn text_tone(tone: Tone) -> impl Fn(&Theme) -> text::Style {
    move |theme| text::Style { color: Some(tone_color(&tokens(theme), tone)) }
}

// -- surfaces ---------------------------------------------------------------

/// A card sits on the canvas: fill one step up, 12px corners, a real drop
/// shadow (offset + blur). No 1px border — that plus a shadow is the ghost card.
pub fn card(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(Background::Color(t.card)),
        text_color: Some(t.card_foreground),
        border: Border { color: Color::TRANSPARENT, width: 0.0, radius: radius::XL.into() },
        shadow: Shadow {
            color: alpha(Color::BLACK, if t.dark { 0.50 } else { 0.10 }),
            offset: Vector::new(0.0, 6.0),
            blur_radius: if t.dark { 20.0 } else { 16.0 },
        },
        ..container::Style::default()
    }
}

/// A stacked list row: same fill as [`card`], quieter shadow so a column of
/// them does not look like a pile of floating panels.
pub fn tile(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(Background::Color(t.card)),
        text_color: Some(t.card_foreground),
        border: Border { color: Color::TRANSPARENT, width: 0.0, radius: radius::LG.into() },
        shadow: Shadow {
            color: alpha(Color::BLACK, if t.dark { 0.28 } else { 0.06 }),
            offset: Vector::new(0.0, 2.0),
            blur_radius: if t.dark { 10.0 } else { 8.0 },
        },
        ..container::Style::default()
    }
}

/// App background (page canvas behind cards).
pub fn app_background(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(Background::Color(t.background)),
        text_color: Some(t.foreground),
        ..container::Style::default()
    }
}

/// Sidebar surface: shadcn uses `bg-muted/40` with a right border.
pub fn sidebar(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(Background::Color(alpha(t.muted, 0.4))),
        ..container::Style::default()
    }
}

/// shadcn `Badge` variants: rounded-full, border, subtle tinted background.
pub fn badge(tone: Tone) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let t = tokens(theme);
        let color = tone_color(&t, tone);
        container::Style {
            background: Some(Background::Color(alpha(color, if t.dark { 0.18 } else { 0.12 }))),
            border: Border { color: alpha(color, 0.35), width: 1.0, radius: radius::PILL.into() },
            ..container::Style::default()
        }
    }
}

/// One segment of [`crate::ui::meter`]. `on` is the tone at full strength; `off`
/// is the same hue at the alpha a badge uses for its fill, so an idle meter
/// reads as a track rather than as five empty boxes.
pub fn meter_cell(tone: Tone, on: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let t = tokens(theme);
        let color = tone_color(&t, tone);
        container::Style {
            background: Some(Background::Color(if on {
                color
            } else {
                alpha(t.muted_foreground, if t.dark { 0.20 } else { 0.15 })
            })),
            border: Border { radius: 1.0.into(), ..Border::default() },
            ..container::Style::default()
        }
    }
}

/// The empty half of a [`crate::ui::gauge`] or [`crate::ui::core_bars`] bar.
/// Neutral, not a tinted version of the fill: the track has to read as "this is
/// how much there is" at every tone, including the red one.
pub fn gauge_track(corner: f32) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        container::Style {
            background: Some(Background::Color(track_color(&tokens(theme)))),
            border: Border { radius: corner.into(), ..Border::default() },
            ..container::Style::default()
        }
    }
}

/// The unfilled part of any meter, bar or dial. One definition so a dial's arc
/// and the bar under it sit on the same grey — two hand-tuned alphas is how the
/// same page ends up with two greys nobody chose.
pub(crate) fn track_color(t: &Tokens) -> Color {
    alpha(t.muted_foreground, if t.dark { 0.14 } else { 0.12 })
}

/// The filled half. Takes the track's radius, so a full bar and an empty
/// one have the same silhouette and only the colour moves.
pub fn gauge_fill(tone: Tone, corner: f32) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let t = tokens(theme);
        container::Style {
            background: Some(Background::Color(tone_color(&t, tone))),
            border: Border { radius: corner.into(), ..Border::default() },
            ..container::Style::default()
        }
    }
}

/// [`badge`] that is a button — a trace id in a log row, which filters to its
/// request when clicked.
pub fn badge_button(tone: Tone) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let t = tokens(theme);
        let color = tone_color(&t, tone);
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: Some(Background::Color(alpha(
                color,
                match (t.dark, hovered) {
                    (true, false) => 0.18,
                    (true, true) => 0.32,
                    (false, false) => 0.12,
                    (false, true) => 0.24,
                },
            ))),
            text_color: color,
            border: Border { color: alpha(color, 0.35), width: 1.0, radius: radius::PILL.into() },
            ..button::Style::default()
        }
    }
}

/// shadcn `Alert` variants.
pub fn alert(tone: Tone) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let t = tokens(theme);
        let color = tone_color(&t, tone);
        container::Style {
            background: Some(Background::Color(alpha(color, if t.dark { 0.12 } else { 0.08 }))),
            border: Border { color: alpha(color, 0.3), width: 1.0, radius: radius::MD.into() },
            ..container::Style::default()
        }
    }
}

/// Dialog scrim (shadcn `DialogOverlay`: bg-black/80).
pub fn scrim(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(alpha(Color::BLACK, 0.8))),
        ..container::Style::default()
    }
}

/// Code/pre block.
pub fn code_block(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(Background::Color(t.muted)),
        border: Border { color: t.border, width: 1.0, radius: radius::MD.into() },
        ..container::Style::default()
    }
}

/// Backdrop for the HUD canvas, which paints a fixed dark palette in either
/// theme — a light backdrop would swallow its white/cyan strokes.
pub fn hud_backdrop(theme: &Theme) -> container::Style {
    // Deep space in dark, cold paper in light — the HUD inverts with the app
    // instead of staying a black box in a light window.
    let bg = if tokens(theme).dark {
        Color::from_rgb(0.04, 0.05, 0.08)
    } else {
        Color::from_rgb(0.94, 0.95, 0.98)
    };
    container::Style { background: Some(Background::Color(bg)), ..code_block(theme) }
}

// -- buttons ----------------------------------------------------------------

/// shadcn button variants. Hover/press follow shadcn's `/90` and `/80` opacity steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonVariant {
    Default,
    Secondary,
    Outline,
    Ghost,
    Destructive,
}

pub fn button_style(variant: ButtonVariant) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let t = tokens(theme);
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let disabled = matches!(status, button::Status::Disabled);

        let (bg, fg, border_color) = match variant {
            ButtonVariant::Default => (Some(t.primary), t.primary_foreground, Color::TRANSPARENT),
            ButtonVariant::Secondary => {
                (Some(t.secondary), t.secondary_foreground, Color::TRANSPARENT)
            }
            ButtonVariant::Outline => (
                if hovered { Some(t.accent) } else { None },
                t.foreground,
                t.border,
            ),
            ButtonVariant::Ghost => (
                if hovered { Some(t.accent) } else { None },
                t.foreground,
                Color::TRANSPARENT,
            ),
            ButtonVariant::Destructive => {
                (Some(t.destructive), t.destructive_foreground, Color::TRANSPARENT)
            }
        };

        let fade = |c: Color| {
            if disabled {
                alpha(c, 0.5)
            } else if hovered && !matches!(variant, ButtonVariant::Outline | ButtonVariant::Ghost) {
                alpha(c, 0.9)
            } else {
                c
            }
        };

        button::Style {
            background: bg.map(|c| Background::Color(fade(c))),
            text_color: if disabled { alpha(fg, 0.5) } else { fg },
            border: Border { color: border_color, width: 1.0, radius: radius::MD.into() },
            ..button::Style::default()
        }
    }
}

/// Sidebar nav entry: selected/hover use the info accent so "here" is not
/// another grey fill in a grey shell. Hairline, not a 2px rail — the kit
/// already owns 1px borders.
pub fn nav_item(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let t = tokens(theme);
        let hovered = matches!(status, button::Status::Hovered);
        let fill = |a: f32| Some(Background::Color(alpha(t.info, a)));
        button::Style {
            background: match (selected, hovered, t.dark) {
                (true, _, true) => fill(0.18),
                (true, _, false) => fill(0.12),
                (false, true, true) => fill(0.10),
                (false, true, false) => fill(0.08),
                _ => None,
            },
            text_color: if selected { t.foreground } else { t.muted_foreground },
            border: Border {
                color: if selected { alpha(t.info, 0.40) } else { Color::TRANSPARENT },
                width: 1.0,
                radius: radius::MD.into(),
            },
            ..button::Style::default()
        }
    }
}

/// Selectable row in a list (run lists, rosters). Same selection language as
/// [`nav_item`] so a highlighted run and a highlighted tab agree.
pub fn list_item(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let t = tokens(theme);
        let hovered = matches!(status, button::Status::Hovered);
        let fill = |a: f32| Some(Background::Color(alpha(t.info, a)));
        button::Style {
            background: match (selected, hovered, t.dark) {
                (true, _, true) => fill(0.18),
                (true, _, false) => fill(0.12),
                (false, true, true) => fill(0.10),
                (false, true, false) => fill(0.08),
                _ => None,
            },
            text_color: t.foreground,
            border: Border {
                color: if selected { alpha(t.info, 0.40) } else { Color::TRANSPARENT },
                width: 1.0,
                radius: radius::MD.into(),
            },
            ..button::Style::default()
        }
    }
}

// -- inputs -----------------------------------------------------------------

/// shadcn `Input`: h-9, rounded-md, border-input, ring on focus.
pub fn input(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let t = tokens(theme);
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: Background::Color(t.background),
        border: Border {
            color: if focused { t.ring } else { t.input },
            width: 1.0,
            radius: radius::MD.into(),
        },
        icon: t.muted_foreground,
        placeholder: t.muted_foreground,
        value: t.foreground,
        selection: alpha(t.info, 0.35),
    }
}

/// shadcn `Checkbox`: 16px, info fill when on, hairline when off.
pub fn checkbox(theme: &Theme, status: iced::widget::checkbox::Status) -> iced::widget::checkbox::Style {
    use iced::widget::checkbox::Status;
    let t = tokens(theme);
    let (checked, hovered, disabled) = match status {
        Status::Active { is_checked } => (is_checked, false, false),
        Status::Hovered { is_checked } => (is_checked, true, false),
        Status::Disabled { is_checked } => (is_checked, false, true),
    };
    iced::widget::checkbox::Style {
        background: Background::Color(if checked {
            t.info
        } else if hovered {
            t.accent
        } else {
            t.background
        }),
        icon_color: if t.dark { t.foreground } else { Color::WHITE },
        border: Border {
            color: if checked { t.info } else { t.input },
            width: 1.0,
            radius: radius::SM.into(),
        },
        text_color: Some(if disabled { alpha(t.foreground, 0.5) } else { t.foreground }),
    }
}

// -- select (shadcn `Select`) -----------------------------------------------

pub fn select(theme: &Theme, status: iced::widget::pick_list::Status) -> iced::widget::pick_list::Style {
    let t = tokens(theme);
    let active = matches!(
        status,
        iced::widget::pick_list::Status::Hovered | iced::widget::pick_list::Status::Opened { .. }
    );
    iced::widget::pick_list::Style {
        text_color: t.foreground,
        placeholder_color: t.muted_foreground,
        handle_color: t.muted_foreground,
        background: Background::Color(t.background),
        border: Border {
            color: if active { t.ring } else { t.input },
            width: 1.0,
            radius: radius::MD.into(),
        },
    }
}

/// shadcn `SelectContent`: popover surface, rounded, shadowed.
pub fn select_menu(theme: &Theme) -> iced::widget::overlay::menu::Style {
    let t = tokens(theme);
    iced::widget::overlay::menu::Style {
        background: Background::Color(t.popover),
        border: Border { color: t.border, width: 1.0, radius: radius::MD.into() },
        text_color: t.foreground,
        selected_text_color: t.foreground,
        selected_background: Background::Color(t.accent),
        shadow: Shadow {
            color: alpha(Color::BLACK, if t.dark { 0.5 } else { 0.12 }),
            offset: Vector::new(0.0, 4.0),
            blur_radius: 12.0,
        },
    }
}

/// shadcn `Tooltip`: bg-primary text-primary-foreground, floats over
/// everything so it gets the same shadow as [`select_menu`].
pub fn tooltip(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(Background::Color(t.primary)),
        text_color: Some(t.primary_foreground),
        border: Border { color: t.primary, width: 0.0, radius: radius::MD.into() },
        shadow: Shadow {
            color: alpha(Color::BLACK, if t.dark { 0.5 } else { 0.12 }),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 8.0,
        },
        ..container::Style::default()
    }
}

// -- separators -------------------------------------------------------------

pub fn separator(theme: &Theme) -> iced::widget::rule::Style {
    let t = tokens(theme);
    iced::widget::rule::Style {
        color: t.border,
        radius: 0.0.into(),
        fill_mode: iced::widget::rule::FillMode::Full,
        snap: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsl_matches_shadcn_values() {
        // `--background: 0 0% 100%` is white; `240 10% 3.9%` is near-black.
        let white = hsl(0.0, 0.0, 100.0);
        assert!((white.r - 1.0).abs() < 1e-6 && (white.b - 1.0).abs() < 1e-6);
        let near_black = hsl(240.0, 10.0, 3.9);
        assert!(near_black.r < 0.06 && near_black.b < 0.07);
        // `0 84.2% 60.2%` (destructive) is a saturated red: r >> g, b.
        let red = hsl(0.0, 84.2, 60.2);
        assert!(red.r > 0.9 && red.g < 0.4 && red.b < 0.4);
    }

    /// The dark ramp is the whole look, and `card()` gave up its shadow on the
    /// strength of it: fills that step upward, under a border lighter than all of
    /// them. A border that sinks back into a fill leaves every panel edgeless.
    #[test]
    fn dark_fills_step_up_under_a_lighter_border() {
        let t = dark_tokens();
        // All near-neutral at one hue, so channel sum orders them by lightness.
        let level = |c: Color| c.r + c.g + c.b;
        assert!(level(t.background) < level(t.card), "a panel must lift off the canvas");
        assert!(level(t.card) < level(t.popover), "an overlay must lift off a panel");
        assert!(level(t.popover) < level(t.muted));
        assert!(level(t.muted) <= level(t.accent), "hover must not darken a muted fill");
        assert!(level(t.accent) < level(t.border), "the border must read as a hairline over every fill");
    }

    #[test]
    fn cards_elevate_without_a_hairline() {
        let dark = card(&dark_theme());
        assert_eq!(dark.border.width, 0.0);
        assert!(dark.shadow.offset.y > 0.0, "shadow needs an offset, not a glow");
        assert!(dark.shadow.blur_radius >= 12.0);
        let light = card(&light_theme());
        assert_eq!(light.border.width, 0.0);
        assert!(light.shadow.offset.y > 0.0);
        assert!(light.shadow.blur_radius >= 12.0);
    }

    #[test]
    fn tiles_sit_quieter_than_cards() {
        let dark_card = card(&dark_theme());
        let dark_tile = tile(&dark_theme());
        assert_eq!(dark_tile.border.width, 0.0);
        assert!(dark_tile.shadow.offset.y > 0.0);
        assert!(dark_tile.shadow.blur_radius < dark_card.shadow.blur_radius);
        assert!(dark_tile.shadow.offset.y < dark_card.shadow.offset.y);
        let light_tile = tile(&light_theme());
        assert_eq!(light_tile.border.width, 0.0);
        assert!(light_tile.shadow.offset.y > 0.0);
    }

    #[test]
    fn light_canvas_lets_white_cards_lift() {
        let t = light_tokens();
        assert!(rel_lum(t.background) < 0.97, "canvas must not be pure white");
        assert!(rel_lum(t.card) > rel_lum(t.background));
        assert_eq!(t.ring, t.info, "focus ring is the info blue, not near-black");
    }

    #[test]
    fn tokens_follow_active_theme() {
        assert!(!tokens(&light_theme()).dark);
        assert!(tokens(&dark_theme()).dark);
    }

    fn rel_lum(c: Color) -> f32 {
        let lin = |ch: f32| {
            if ch <= 0.04045 {
                ch / 12.92
            } else {
                ((ch + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
    }

    fn contrast(a: Color, b: Color) -> f32 {
        let (l1, l2) = (rel_lum(a), rel_lum(b));
        let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn hairlines_and_destructive_meet_aa() {
        let dark = dark_tokens();
        assert!(
            contrast(dark.border, dark.card) >= 3.0,
            "dark hairline vs card {}",
            contrast(dark.border, dark.card)
        );
        assert!(
            contrast(dark.destructive_foreground, dark.destructive) >= 4.5,
            "dark destructive {}",
            contrast(dark.destructive_foreground, dark.destructive)
        );
        let light = light_tokens();
        assert!(
            contrast(light.border, light.background) >= 3.0,
            "light hairline vs canvas {}",
            contrast(light.border, light.background)
        );
        assert!(
            contrast(light.destructive_foreground, light.destructive) >= 4.5,
            "light destructive {}",
            contrast(light.destructive_foreground, light.destructive)
        );
        assert!(
            contrast(light.muted_foreground, light.background) >= 4.5,
            "light muted {}",
            contrast(light.muted_foreground, light.background)
        );
    }
}
