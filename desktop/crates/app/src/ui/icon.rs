//! Icons — lucide (the set shadcn/ui ships with) via the `iced_fonts` crate.
//!
//! Icons are glyphs in a bundled font, so they are `Text` widgets: inside a
//! button or nav item they inherit that widget's `text_color` for free, and no
//! per-variant color plumbing is needed. The enum names the handful the app
//! uses; the font carries ~1600 more.

use super::theme;
use iced::widget::Text;
use iced::Element;
use iced_fonts::lucide;

/// Matches `font::SM` text so an icon sits on the same line as its label.
pub const SIZE: f32 = 16.0;

/// The font bytes the app must load at startup (see `main.rs`).
pub const FONT_BYTES: &[u8] = iced_fonts::LUCIDE_FONT_BYTES;

/// The loaded font, for widgets that take a font + code point.
pub const FONT: iced::Font = iced_fonts::LUCIDE_FONT;

macro_rules! icons {
    ($($variant:ident => $glyph:ident),+ $(,)?) => {
        /// The app's icon vocabulary — names follow <https://lucide.dev/icons>.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Icon {
            $($variant),+
        }

        impl Icon {
            /// The raw glyph, unsized and unstyled.
            pub fn glyph<'a>(self) -> Text<'a> {
                match self {
                    $(Icon::$variant => lucide::$glyph()),+
                }
            }

            /// The glyph's char, for widgets that take a code point rather than
            /// a child element (`text_input`'s leading icon).
            pub fn code_point(self) -> char {
                let (content, _, _) = match self {
                    $(Icon::$variant => lucide::advanced_text::$glyph()),+
                };
                content.chars().next().unwrap_or(' ')
            }
        }
    };
}

icons! {
    // navigation
    Activity => activity,
    Folder => folder,
    Users => users,
    Cpu => cpu,
    Message => message_square,
    Sparkles => sparkles,
    Plug => plug,
    Gauge => gauge,
    Scroll => scroll_text,
    Zap => zap,

    // actions
    Refresh => refresh_cw,
    RotateCcw => rotate_ccw,
    Play => play,
    Stop => square,
    Pause => pause,
    Check => check,
    X => x,
    Plus => plus,
    Trash => trash_two,
    Pencil => pencil,
    Save => save,
    Upload => upload,
    Download => download,
    Copy => copy,
    Eye => eye,
    EyeOff => eye_off,
    FolderOpen => folder_open,
    Send => send,
    Mic => mic,
    MicOff => mic_off,
    Volume => volume_two,
    VolumeOff => volume_x,
    Search => search,
    Settings => settings,
    ChevronLeft => chevron_left,
    ChevronRight => chevron_right,
    ListChecks => list_checks,
    ArrowLeft => arrow_left,
    ArrowRight => arrow_right,
    Globe => globe,

    // theme
    Sun => sun,
    Moon => moon,
    Monitor => monitor,

    // state
    Lock => lock,
    Server => server,
    CheckCircle => circle_check,
    XCircle => circle_x,
    Alert => triangle_alert,
    Info => info,
    Clock => clock_four,
    Terminal => terminal,
    Inbox => inbox,
}

/// Icon inheriting the surrounding widget's text color (buttons, nav items).
pub fn glyph<'a, M: 'a>(i: Icon) -> Element<'a, M> {
    i.glyph().size(SIZE).into()
}

/// `text-muted-foreground` icon — captions, list rows.
pub fn icon_muted<'a, M: 'a>(i: Icon) -> Element<'a, M> {
    i.glyph().size(SIZE).style(theme::text_muted).into()
}

/// Icon in a domain [`Tone`](theme::Tone) — badges, alerts, status rows.
pub fn icon_tone<'a, M: 'a>(i: Icon, tone: theme::Tone) -> Element<'a, M> {
    i.glyph().size(SIZE).style(theme::text_tone(tone)).into()
}

/// Oversized muted icon for empty states.
pub fn icon_large<'a, M: 'a>(i: Icon, size: f32) -> Element<'a, M> {
    i.glyph().size(size).style(theme::text_muted).into()
}
