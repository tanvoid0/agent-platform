//! Design tokens: shadcn/ui's structure, LM Studio's dark palette.
//!
//! Semantic names (`background`, `card`, `muted_foreground`, `border`, …) and the
//! spacing/radius/type scales below are shadcn's, not iced's — screens use these,
//! never raw colors. The light block is still shadcn's default (zinc) `:root`
//! verbatim; the dark block is not, because the two design languages disagree
//! about depth. shadcn leans on shadows, LM Studio on flat fills separated by a
//! hairline — so the dark ramp is retuned for the latter and [`card`] casts no
//! shadow at all. See [`dark_tokens`].

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
    pub const PILL: f32 = 999.0;
}

/// Tailwind type scale used by shadcn components.
pub mod font {
    pub const XS: f32 = 12.0; // text-xs — badges, captions
    pub const SM: f32 = 14.0; // text-sm — body, buttons, inputs
    pub const BASE: f32 = 16.0;
    pub const LG: f32 = 18.0; // card titles
    pub const XL2: f32 = 24.0; // page titles
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
        background: hsl(0.0, 0.0, 100.0),
        foreground: hsl(240.0, 10.0, 3.9),
        card: hsl(0.0, 0.0, 100.0),
        card_foreground: hsl(240.0, 10.0, 3.9),
        popover: hsl(0.0, 0.0, 100.0),
        primary: hsl(240.0, 5.9, 10.0),
        primary_foreground: hsl(0.0, 0.0, 98.0),
        secondary: hsl(240.0, 4.8, 95.9),
        secondary_foreground: hsl(240.0, 5.9, 10.0),
        muted: hsl(240.0, 4.8, 95.9),
        muted_foreground: hsl(240.0, 3.8, 46.1),
        accent: hsl(240.0, 4.8, 95.9),
        destructive: hsl(0.0, 84.2, 60.2),
        destructive_foreground: hsl(0.0, 0.0, 98.0),
        // Not in shadcn's base set; taken from the Tailwind ramp it ships with.
        success: hsl(142.1, 76.2, 36.3),
        warning: hsl(37.7, 92.1, 50.2),
        info: hsl(221.2, 83.2, 53.3),
        border: hsl(240.0, 5.9, 90.0),
        input: hsl(240.0, 5.9, 90.0),
        ring: hsl(240.0, 10.0, 3.9),
        dark: false,
    }
}

/// LM Studio-style dark: a near-neutral canvas, flat elevation steps, and a
/// border lighter than every fill so panels are separated by a hairline instead
/// of a shadow.
///
/// shadcn's `.dark` collapses `border`, `muted`, `secondary` and `accent` onto a
/// single value. That is fine when cards cast shadows, but this language has no
/// shadows to fall back on — a card's edge and a hovered row would be the same
/// color, leaving panels with no visible boundary. Hence the split, and the test
/// below that keeps it.
fn dark_tokens() -> Tokens {
    Tokens {
        background: hsl(240.0, 5.0, 7.0), // canvas
        foreground: hsl(0.0, 0.0, 98.0),
        card: hsl(240.0, 5.0, 10.0), // panel on the canvas
        card_foreground: hsl(0.0, 0.0, 98.0),
        popover: hsl(240.0, 5.0, 12.0), // floats above a panel, so one step up again
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
        info: hsl(217.0, 78.0, 58.0),
        border: hsl(240.0, 4.0, 20.0), // the hairline — lighter than any fill
        input: hsl(240.0, 4.0, 20.0),
        // Focus reads as the accent. shadcn's near-white ring is a second bright
        // value competing with `foreground` on every focused field.
        ring: hsl(217.0, 78.0, 58.0),
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

/// shadcn `Card`: bg-card, rounded-lg, border — but no shadow. Depth comes from
/// the fill stepping up off the canvas and the border catching the edge; a card
/// is inline, not floating, so it has nothing to cast onto. Overlays that really
/// do float ([`select_menu`]) keep theirs.
pub fn card(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(Background::Color(t.card)),
        text_color: Some(t.card_foreground),
        border: Border { color: t.border, width: 1.0, radius: radius::LG.into() },
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

/// Sidebar nav entry: active = `bg-accent`, hover = `bg-accent/50`.
pub fn nav_item(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let t = tokens(theme);
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: match (selected, hovered) {
                (true, _) => Some(Background::Color(t.accent)),
                (false, true) => Some(Background::Color(alpha(t.accent, 0.5))),
                _ => None,
            },
            text_color: if selected { t.foreground } else { t.muted_foreground },
            border: Border { radius: radius::MD.into(), ..Border::default() },
            ..button::Style::default()
        }
    }
}

/// Selectable row in a list (run lists, rosters).
pub fn list_item(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let t = tokens(theme);
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: match (selected, hovered) {
                (true, _) => Some(Background::Color(t.accent)),
                (false, true) => Some(Background::Color(alpha(t.accent, 0.5))),
                _ => None,
            },
            text_color: t.foreground,
            border: Border {
                color: if selected { t.border } else { Color::TRANSPARENT },
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
    fn tokens_follow_active_theme() {
        assert!(!tokens(&light_theme()).dark);
        assert!(tokens(&dark_theme()).dark);
    }
}
