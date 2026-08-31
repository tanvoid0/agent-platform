//! Reusable UI kit — shadcn/ui's component vocabulary rendered natively in iced.
//!
//! Component names, variants, sizes and spacing mirror shadcn (`Button`,
//! `Card`, `Badge`, `Alert`, `Input`, `Separator`, `Tabs`, `Skeleton`), so the
//! app reads as the same design system without a DOM. Tokens live in
//! [`theme`]; screens compose these functions and never style raw widgets.
//!
//! The kit is deliberately complete ahead of its callers (Phase 3/4 screens),
//! hence the blanket allow.
#![allow(dead_code)]

pub mod icon;
pub mod theme;

use iced::widget::{
    button, canvas, column, container, row, rule, scrollable, space as space_widget, text,
    text_input, Column, Row,
};
use iced::{Element, Length, Padding};

pub use icon::{icon_muted, Icon};
pub use theme::{font, space, ButtonVariant, Tone};

// ---------------------------------------------------------------------------
// Typography (shadcn's Tailwind type scale)
// ---------------------------------------------------------------------------

/// `text-2xl font-semibold` — page title.
pub fn title<'a, M: 'a>(content: impl text::IntoFragment<'a>) -> Element<'a, M> {
    text(content).size(font::XL2).font(font::SEMIBOLD).style(theme::text_default).into()
}

/// `text-lg font-semibold` — card title.
pub fn heading<'a, M: 'a>(content: impl text::IntoFragment<'a>) -> Element<'a, M> {
    text(content).size(font::LG).font(font::SEMIBOLD).style(theme::text_default).into()
}

/// `text-sm` — body copy. Fills its parent so long strings wrap instead of
/// overflowing the card (iced text lays out at intrinsic width otherwise).
pub fn body<'a, M: 'a>(content: impl text::IntoFragment<'a>) -> Element<'a, M> {
    text(content).size(font::SM).width(Length::Fill).style(theme::text_default).into()
}

/// `text-sm text-muted-foreground` — descriptions, helper copy.
pub fn muted<'a, M: 'a>(content: impl text::IntoFragment<'a>) -> Element<'a, M> {
    text(content).size(font::SM).width(Length::Fill).style(theme::text_muted).into()
}

/// `text-xs text-muted-foreground` — captions.
pub fn caption<'a, M: 'a>(content: impl text::IntoFragment<'a>) -> Element<'a, M> {
    text(content).size(font::XS).style(theme::text_muted).into()
}

/// `font-mono text-xs` — paths, ids, log lines.
pub fn mono<'a, M: 'a>(content: impl text::IntoFragment<'a>) -> Element<'a, M> {
    text(content)
        .size(font::XS)
        .width(Length::Fill)
        .font(iced::Font::MONOSPACE)
        .style(theme::text_default)
        .into()
}

/// Colored text for domain state.
pub fn toned<'a, M: 'a>(content: impl text::IntoFragment<'a>, tone: Tone) -> Element<'a, M> {
    text(content).size(font::SM).style(theme::text_tone(tone)).into()
}

/// [`mono`], colored by `tone` — an error or warning log line's message, so
/// the two rows worth stopping for read differently from the wall of neutral
/// ones around them, not just their level pill.
pub fn mono_toned<'a, M: 'a>(content: impl text::IntoFragment<'a>, tone: Tone) -> Element<'a, M> {
    text(content)
        .size(font::XS)
        .width(Length::Fill)
        .font(iced::Font::MONOSPACE)
        .style(theme::text_tone(tone))
        .into()
}

// ---------------------------------------------------------------------------
// Button (shadcn variants + sizes)
// ---------------------------------------------------------------------------

/// shadcn sizes: `sm` (h-8), `default` (h-9), `lg` (h-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Sm,
    Default,
    Lg,
}

impl Size {
    fn padding(self) -> Padding {
        match self {
            Size::Sm => Padding::from([4.0, 12.0]),
            Size::Default => Padding::from([8.0, 16.0]),
            Size::Lg => Padding::from([10.0, 20.0]),
        }
    }
}

/// Full-control button; the helpers below cover the common cases. Buttons carry
/// a leading icon (shadcn's `<Button><Icon /> Label</Button>`); pass `None` only
/// where a glyph would add noise.
///
/// `label` takes `impl text::IntoFragment` (as `heading`/`body`/`badge` already
/// do), not a bare `&'a str`, so a computed label ("Open in Google") works here
/// the same as a literal one — the only difference from a plain `&str` call
/// site is that `text(label)` accepts an owned `String` too.
pub fn button_sized<'a, M: 'a + Clone>(
    glyph: Option<Icon>,
    label: impl text::IntoFragment<'a>,
    variant: ButtonVariant,
    size: Size,
    on_press: Option<M>,
) -> Element<'a, M> {
    let content: Element<'a, M> = match glyph {
        Some(g) => row![icon::glyph(g), text(label).size(font::SM)]
            .spacing(space::XS + 2.0)
            .align_y(iced::Alignment::Center)
            .into(),
        None => text(label).size(font::SM).into(),
    };
    let b = button(content).padding(size.padding()).style(theme::button_style(variant));
    match on_press {
        Some(msg) => b.on_press(msg).into(),
        None => b.into(), // no handler = disabled styling
    }
}

/// `<Button>` — primary action.
pub fn button_default<'a, M: 'a + Clone>(
    glyph: Icon,
    label: impl text::IntoFragment<'a>,
    on_press: M,
) -> Element<'a, M> {
    button_sized(Some(glyph), label, ButtonVariant::Default, Size::Default, Some(on_press))
}

/// `<Button variant="secondary">`
pub fn button_secondary<'a, M: 'a + Clone>(
    glyph: Icon,
    label: impl text::IntoFragment<'a>,
    on_press: M,
) -> Element<'a, M> {
    button_sized(Some(glyph), label, ButtonVariant::Secondary, Size::Sm, Some(on_press))
}

/// `<Button variant="outline">`
pub fn button_outline<'a, M: 'a + Clone>(
    glyph: Icon,
    label: impl text::IntoFragment<'a>,
    on_press: M,
) -> Element<'a, M> {
    button_sized(Some(glyph), label, ButtonVariant::Outline, Size::Sm, Some(on_press))
}

/// `<Button variant="ghost">`
pub fn button_ghost<'a, M: 'a + Clone>(
    glyph: Icon,
    label: impl text::IntoFragment<'a>,
    on_press: M,
) -> Element<'a, M> {
    button_sized(Some(glyph), label, ButtonVariant::Ghost, Size::Sm, Some(on_press))
}

/// Square icon-only ghost button (`<Button variant="ghost" size="icon">`).
pub fn icon_button<'a, M: 'a + Clone>(glyph: Icon, on_press: M) -> Element<'a, M> {
    button(container(icon::glyph(glyph)).center(Length::Fill))
        .width(28)
        .height(28)
        .padding(0)
        .style(theme::button_style(ButtonVariant::Ghost))
        .on_press(on_press)
        .into()
}

/// [`icon_button`] with a tooltip — the default for any unlabeled control.
pub fn icon_tip<'a, M: 'a + Clone>(glyph: Icon, label: &'a str, on_press: M) -> Element<'a, M> {
    tooltip(icon_button(glyph, on_press), label)
}

/// shadcn `Tooltip`: a small label that appears above `content` on hover.
/// Wrap any icon-only control that has no visible label with this.
pub fn tooltip<'a, M: 'a>(content: Element<'a, M>, label: &'a str) -> Element<'a, M> {
    iced::widget::tooltip(
        content,
        container(text(label).size(font::XS)).padding(Padding::from([4.0, 8.0])).style(theme::tooltip),
        iced::widget::tooltip::Position::Top,
    )
    .gap(6)
    .into()
}

/// Icon-only nav control (no label): same selected/hover styling as
/// [`nav_item`], sized like [`icon_button`]. Used where the icon alone is
/// self-explanatory, e.g. Settings sitting next to the theme/refresh icons.
pub fn nav_icon_button<'a, M: 'a + Clone>(
    glyph: Icon,
    label: &'a str,
    selected: bool,
    on_press: M,
) -> Element<'a, M> {
    let btn = button(container(icon::glyph(glyph)).center(Length::Fill))
        .width(28)
        .height(28)
        .padding(0)
        .style(theme::nav_item(selected))
        .on_press(on_press);
    tooltip(btn.into(), label)
}

/// shadcn `Toggle` — stays highlighted while its state is on, so a switch
/// (Files pane open, Plan step on) reads as state rather than as a one-shot
/// action the way [`button_ghost`] does.
pub fn toggle<'a, M: 'a + Clone>(
    glyph: Icon,
    // Owned as well as borrowed: a label that names the assistant is built per
    // frame, and the caller has nowhere to keep it.
    label: impl text::IntoFragment<'a>,
    selected: bool,
    on_press: M,
) -> Element<'a, M> {
    button(
        row![icon::glyph(glyph), text(label).size(font::SM)]
            .spacing(space::XS + 2.0)
            .align_y(iced::Alignment::Center),
    )
    .padding(Size::Sm.padding())
    .style(theme::nav_item(selected))
    .on_press(on_press)
    .into()
}

/// `<Button variant="destructive">`
pub fn button_destructive<'a, M: 'a + Clone>(
    glyph: Icon,
    label: impl text::IntoFragment<'a>,
    on_press: M,
) -> Element<'a, M> {
    button_sized(Some(glyph), label, ButtonVariant::Destructive, Size::Sm, Some(on_press))
}

/// Sidebar nav entry: icon + label, the icon tracking the label's color.
pub fn nav_item<'a, M: 'a + Clone>(
    glyph: Icon,
    label: &'a str,
    selected: bool,
    on_press: M,
) -> Element<'a, M> {
    button(
        row![icon::glyph(glyph), text(label).size(font::SM)]
        .spacing(space::SM)
        .align_y(iced::Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([8.0, 12.0]))
    .style(theme::nav_item(selected))
    .on_press(on_press)
    .into()
}

/// [`nav_item`] carrying a count of what happened on that screen while the user
/// was elsewhere. A count of zero renders the plain item — an empty inbox shows
/// no badges at all.
pub fn nav_item_counted<'a, M: 'a + Clone>(
    glyph: Icon,
    label: &'a str,
    selected: bool,
    count: usize,
    tone: Tone,
    on_press: M,
) -> Element<'a, M> {
    if count == 0 {
        return nav_item(glyph, label, selected, on_press);
    }
    button(
        row![
            icon::glyph(glyph),
            text(label).size(font::SM).width(Length::Fill),
            badge(count.to_string(), tone),
        ]
        .spacing(space::SM)
        .align_y(iced::Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([8.0, 12.0]))
    .style(theme::nav_item(selected))
    .on_press(on_press)
    .into()
}

/// The notification bell and its unseen count. Zero renders the bell alone, so
/// an empty inbox is quiet; the tone is the caller's, because "waiting on you"
/// and "finished" are not the same news.
pub fn bell<'a, M: 'a + Clone>(count: usize, tone: Tone, on_press: M) -> Element<'a, M> {
    let mut children: Vec<Element<'a, M>> = vec![icon::glyph(Icon::Bell)];
    if count > 0 {
        children.push(badge(count.to_string(), tone));
    }
    let btn = button(Row::with_children(children).spacing(space::XS).align_y(iced::Alignment::Center))
        .height(28)
        .padding(Padding::from([0.0, space::XS]))
        .style(theme::nav_item(count > 0))
        .on_press(on_press);
    tooltip(
        btn.into(),
        if count > 0 { "Notifications" } else { "Notifications (nothing waiting)" },
    )
}

/// Heading above a run of [`nav_item`]s, so nine destinations read as three
/// short groups instead of one flat list.
pub fn nav_group<'a, M: 'a>(label: &'a str) -> Element<'a, M> {
    container(text(label).size(font::XS).style(theme::text_muted))
        .padding(Padding { top: space::SM, right: 0.0, bottom: 2.0, left: 12.0 })
        .into()
}

/// A nav entry the app cannot open yet (the server it needs is not up). Kept
/// visible rather than hidden so the shape of the app does not change while it
/// starts — it just cannot be pressed.
pub fn nav_item_locked<'a, M: 'a>(glyph: Icon, label: &'a str) -> Element<'a, M> {
    container(
        row![
            icon_muted(glyph),
            text(label).size(font::SM).width(Length::Fill).style(theme::text_muted),
            icon_muted(Icon::Lock),
        ]
        .spacing(space::SM)
        .align_y(iced::Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([8.0, 12.0]))
    .into()
}

/// Segmented control (shadcn `Tabs`), used for theme mode and view switching.
pub fn segmented<'a, M: 'a + Clone>(
    options: impl IntoIterator<Item = (&'a str, bool, M)>,
) -> Element<'a, M> {
    let children: Vec<Element<'a, M>> = options
        .into_iter()
        .map(|(label, selected, msg)| {
            button(text(label).size(font::XS))
                .padding(Padding::from([4.0, 10.0]))
                .style(theme::nav_item(selected))
                .on_press(msg)
                .into()
        })
        .collect();
    container(Row::with_children(children).spacing(2.0))
        .padding(2.0)
        .style(theme::code_block)
        .into()
}

/// [`segmented`] for a set where more than one option can be on, and which
/// therefore has no fixed width. It wraps: five equipment options in a 460px
/// pane are one clipped line otherwise, and the clipped one is unpickable.
pub fn chips<'a, M: 'a + Clone>(
    options: impl IntoIterator<Item = (&'a str, bool, M)>,
) -> Element<'a, M> {
    let children: Vec<Element<'a, M>> = options
        .into_iter()
        .map(|(label, selected, msg)| {
            button(text(label).size(font::XS))
                .padding(Padding::from([4.0, 10.0]))
                .style(theme::nav_item(selected))
                .on_press(msg)
                .into()
        })
        .collect();
    Row::with_children(children).spacing(space::XS).wrap().into()
}

// ---------------------------------------------------------------------------
// Card / layout
// ---------------------------------------------------------------------------

/// `<Card>` — elevated surface, shadow instead of a hairline.
pub fn card<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Element<'a, M> {
    container(content)
        .padding(space::MD)
        .width(Length::Fill)
        .style(theme::card)
        .into()
}

/// A quieter card for stacked rows (team list, plan items). Same fill, less lift.
pub fn tile<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Element<'a, M> {
    container(content)
        .padding(space::MD)
        .width(Length::Fill)
        .style(theme::tile)
        .into()
}

/// `<Card>` with `<CardHeader>` (title + optional description + actions).
pub fn card_with_header<'a, M: 'a>(
    title_text: impl text::IntoFragment<'a>,
    description: Option<Element<'a, M>>,
    actions: Option<Element<'a, M>>,
    content: impl Into<Element<'a, M>>,
) -> Element<'a, M> {
    let title_row: Element<'a, M> = match actions {
        Some(actions) => row![heading(title_text), space_widget::horizontal(), actions]
            .spacing(space::SM)
            .align_y(iced::Alignment::Center)
            .into(),
        None => heading(title_text),
    };
    let mut header = column![title_row].spacing(space::XS);
    if let Some(desc) = description {
        header = header.push(desc);
    }
    card(column![header, separator(), content.into()].spacing(space::MD))
}

/// Convenience: card with a title and no description.
pub fn section<'a, M: 'a>(
    title_text: impl text::IntoFragment<'a>,
    actions: Option<Element<'a, M>>,
    content: impl Into<Element<'a, M>>,
) -> Element<'a, M> {
    card_with_header(title_text, None, actions, content)
}

/// Page scaffold: title, optional description, optional header actions, scrolling body.
pub fn page<'a, M: 'a>(
    title_text: impl text::IntoFragment<'a>,
    description: Option<Element<'a, M>>,
    actions: Option<Element<'a, M>>,
    content: impl Into<Element<'a, M>>,
) -> Element<'a, M> {
    page_fixed(
        title_text,
        description,
        actions,
        scrollable(content.into()).spacing(space::SM).height(Length::Fill),
    )
}

/// Same scaffold for screens that scroll their own body — a chat transcript
/// scrolls while its composer stays pinned, which an outer scrollable breaks
/// (nested scrollables, and the composer scrolling off the page).
pub fn page_fixed<'a, M: 'a>(
    title_text: impl text::IntoFragment<'a>,
    description: Option<Element<'a, M>>,
    actions: Option<Element<'a, M>>,
    content: impl Into<Element<'a, M>>,
) -> Element<'a, M> {
    let title_row: Element<'a, M> = match actions {
        Some(actions) => row![title(title_text), space_widget::horizontal(), actions]
            .spacing(space::SM)
            .align_y(iced::Alignment::Center)
            .into(),
        None => title(title_text),
    };
    let mut head = column![title_row].spacing(space::XS);
    if let Some(desc) = description {
        head = head.push(desc);
    }
    page_custom(head, content)
}

/// The same scaffold for a screen whose header is a *control* rather than a
/// title — the assistant, where the model you are talking to is the thing worth
/// the top of the page and the title only repeated the tab above it.
pub fn page_custom<'a, M: 'a>(
    head: impl Into<Element<'a, M>>,
    content: impl Into<Element<'a, M>>,
) -> Element<'a, M> {
    container(
        column![head.into(), container(content.into()).height(Length::Fill)]
            .spacing(space::LG)
            .padding(space::LG),
    )
    .style(theme::app_background)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// `<Separator />`
pub fn separator<'a, M: 'a>() -> Element<'a, M> {
    rule::horizontal(1).style(theme::separator).into()
}

pub fn separator_vertical<'a, M: 'a>() -> Element<'a, M> {
    rule::vertical(1).style(theme::separator).into()
}

pub fn spacer<'a, M: 'a>() -> Element<'a, M> {
    space_widget::horizontal().into()
}

/// Vertical stack (`space-y-2`).
pub fn stack<'a, M: 'a>(children: Vec<Element<'a, M>>) -> Column<'a, M> {
    Column::with_children(children).spacing(space::SM)
}

/// Vertical stack with card-level gap (`space-y-4`).
pub fn stack_lg<'a, M: 'a>(children: Vec<Element<'a, M>>) -> Column<'a, M> {
    Column::with_children(children).spacing(space::MD)
}

/// Horizontal group (`flex items-center gap-2`).
/// [`cluster`] that wraps instead of overflowing. A row of badges whose count
/// depends on data (a board card's facts, the sidebar's utility strip) has no
/// width it is guaranteed to fit in, and an overflowing `Row` clips its tail
/// silently — the clipped control is simply unreachable.
pub fn wrap_row<'a, M: 'a>(children: Vec<Element<'a, M>>) -> Element<'a, M> {
    Row::with_children(children)
        .spacing(space::XS)
        .align_y(iced::Alignment::Center)
        .wrap()
        .into()
}

pub fn cluster<'a, M: 'a>(children: Vec<Element<'a, M>>) -> Row<'a, M> {
    Row::with_children(children)
        .spacing(space::SM)
        .align_y(iced::Alignment::Center)
}

// ---------------------------------------------------------------------------
// Dialog (shadcn `Dialog` / `AlertDialog`)
// ---------------------------------------------------------------------------

/// Draw `dialog` centered over `base` on a scrim — shadcn's `DialogOverlay` +
/// `DialogContent`. The app owns its modals rather than handing them to the OS,
/// so a confirmation looks like the rest of the app and not like Windows.
pub fn modal<'a, M: 'a>(
    base: impl Into<Element<'a, M>>,
    dialog: impl Into<Element<'a, M>>,
    max_width: f32,
) -> Element<'a, M> {
    let overlay = container(container(dialog.into()).max_width(max_width))
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .padding(space::LG)
        .style(theme::scrim);
    iced::widget::stack![base.into(), overlay].into()
}

/// `<Toaster>` — pins `toast` to the bottom-right corner over `base`. Unlike
/// [`modal`] it lays no scrim, so the page underneath stays readable *and*
/// clickable: [`opaque`] is only as large as the toast, and [`bottom_right`]
/// just places it. A Fill positioning layer used to swallow every click
/// (measured on an earlier iced); that is why the E.V. panel is still a real
/// column of the shell row rather than a layer (see `screen::view`).
pub fn toast_layer<'a, M: 'a>(
    base: impl Into<Element<'a, M>>,
    toast: impl Into<Element<'a, M>>,
) -> Element<'a, M> {
    iced::widget::stack![
        base.into(),
        iced::widget::bottom_right(iced::widget::opaque(
            container(toast.into()).max_width(420),
        ))
        .padding(space::LG),
    ]
    .into()
}

/// `<Toast>` — one transient message: tone glyph, text, close button. The
/// success half of what used to be an inline banner; errors stay in the page
/// because they are not transient.
pub fn toast<'a, M: 'a + Clone>(
    message: impl text::IntoFragment<'a>,
    tone: Tone,
    on_dismiss: M,
) -> Element<'a, M> {
    container(
        row![
            tone_icon(tone).glyph().size(font::SM).style(theme::text_tone(tone)),
            text(message).size(font::SM).width(Length::Fill).style(theme::text_default),
            tooltip(icon_button(Icon::X, on_dismiss), "Dismiss"),
        ]
        .spacing(space::SM)
        .align_y(iced::Alignment::Center),
    )
    .padding(space::MD)
    .style(theme::card)
    .into()
}

/// `<AlertDialog>` — title, one line of prose, right-aligned buttons. Pair with
/// [`modal`]; the last action reads as the primary one, as in shadcn.
pub fn confirm_dialog<'a, M: 'a>(
    title_text: impl text::IntoFragment<'a>,
    description: impl text::IntoFragment<'a>,
    actions: Vec<Element<'a, M>>,
) -> Element<'a, M> {
    let mut buttons = vec![spacer()];
    buttons.extend(actions);
    card(
        column![heading(title_text), muted(description), cluster(buttons)]
            .spacing(space::MD),
    )
}

// ---------------------------------------------------------------------------
// Data display
// ---------------------------------------------------------------------------

/// Label/value row with a fixed label column so stacked rows align.
pub fn field<'a, M: 'a>(label: &'a str, value: impl Into<Element<'a, M>>) -> Element<'a, M> {
    row![
        text(label).size(font::SM).style(theme::text_muted).width(150),
        container(value.into()).width(Length::Fill),
    ]
    .spacing(space::SM)
    .align_y(iced::Alignment::Start)
    .into()
}

/// `3 steps` / `1 step` — a count with its noun, so a badge never reads
/// "1 steps". Both forms are passed in; English plurals are not guessable
/// ("memory" → "memories").
pub fn count(n: usize, singular: &str, plural: &str) -> String {
    format!("{n} {}", if n == 1 { singular } else { plural })
}

/// `<Badge>` — rounded-full status pill.
pub fn badge<'a, M: 'a>(label: impl text::IntoFragment<'a>, tone: Tone) -> Element<'a, M> {
    container(text(label).size(font::XS).style(theme::text_tone(tone)))
        .padding(Padding::from([2.0, space::SM]))
        .style(theme::badge(tone))
        .into()
}

/// `<Badge>` with a leading glyph — state that reads faster as a shape.
/// [`badge`] that is a button. The label is owned, not `&'a str` like the
/// `button_*` family: these are built from data (a trace id off a log line),
/// not from a literal in the view.
pub fn badge_button<'a, M: 'a + Clone>(
    label: impl text::IntoFragment<'a>,
    tone: Tone,
    on_press: M,
) -> Element<'a, M> {
    button(text(label).size(font::XS).font(iced::Font::MONOSPACE))
        .padding(Padding::from([2.0, space::SM]))
        .style(theme::badge_button(tone))
        .on_press(on_press)
        .into()
}

pub fn badge_icon<'a, M: 'a>(
    glyph: Icon,
    label: impl text::IntoFragment<'a>,
    tone: Tone,
) -> Element<'a, M> {
    container(
        row![
            glyph.glyph().size(font::XS).style(theme::text_tone(tone)),
            text(label).size(font::XS).style(theme::text_tone(tone)),
        ]
        .spacing(space::XS)
        .align_y(iced::Alignment::Center),
    )
    .padding(Padding::from([2.0, space::SM]))
    .style(theme::badge(tone))
    .into()
}

/// A segmented bar: `filled` of `total` cells lit, in `tone`.
///
/// Cells rather than a continuous bar because the number it reports is small and
/// discrete — model calls in flight against a lane's limit — and four filled
/// blocks out of eight is a count you can read at a glance, where 50% of a
/// smooth bar is not. Containers with a background, so it costs one quad per
/// cell and nothing per frame; a canvas here would mean a redraw loop for a
/// widget that changes every few seconds.
///
/// `total` of 0 draws nothing rather than dividing by it.
pub fn meter<'a, M: 'a>(filled: usize, total: usize, tone: Tone) -> Element<'a, M> {
    // A wide lane on a big machine would draw sixteen 2px slivers in a 208px
    // sidebar. Past this the cells stop being countable, so the bar switches to
    // proportional: same widget, `filled` scaled into the cells there is room for.
    let Some((cells, lit)) = meter_cells(filled, total) else {
        return space_widget::horizontal().into();
    };
    Row::with_children((0..cells).map(|i| {
        container(space_widget::vertical().height(4.0))
            .width(Length::Fill)
            .style(theme::meter_cell(tone, i < lit))
            .into()
    }))
    .spacing(2.0)
    .into()
}

/// `(cells, lit)` for [`meter`], or `None` when there is nothing to draw.
///
/// Past `MAX_CELLS` the bar stops being a count and becomes proportional: a wide
/// lane on a big machine would otherwise draw sixteen 2px slivers in a 208px
/// sidebar, which is neither countable nor a bar. The scaling rounds *up*, so
/// one call in flight is never drawn as an idle track.
fn meter_cells(filled: usize, total: usize) -> Option<(usize, usize)> {
    const MAX_CELLS: usize = 10;
    if total == 0 {
        return None;
    }
    let cells = total.min(MAX_CELLS);
    let filled = filled.min(total);
    Some((cells, if total <= MAX_CELLS { filled } else { (filled * cells).div_ceil(total) }))
}

/// A continuous bar for a ratio, where [`meter`] draws cells for a count.
///
/// The distinction is what the number *is*. "3 of 8 calls" has three real things
/// in it and reads best as three lit blocks; 61% of memory has no things in it
/// at all, and rounding it into ten cells answers a question nobody asked. So
/// this is one rounded track with a rounded fill, sized by `Length::FillPortion`
/// — no canvas, no per-frame work, the same two quads whatever the value.
///
/// `fraction` is clamped to 0–1; anything above zero draws at least a sliver, or
/// a machine at 0.2% is indistinguishable from a machine nobody sampled.
pub fn gauge<'a, M: 'a>(fraction: f32, tone: Tone) -> Element<'a, M> {
    gauge_sized(fraction, tone, GAUGE_HEIGHT)
}

/// [`gauge`]'s track height. Thicker than [`meter`]'s 4px cells because this one
/// is a page's headline number rather than a sidebar footnote.
const GAUGE_HEIGHT: f32 = 8.0;

/// Scale for the `FillPortion` split. 1000 makes a 0.1% step visible on a wide
/// card without the rounding showing up as a jump.
const GAUGE_SCALE: u16 = 1000;

fn gauge_sized<'a, M: 'a>(fraction: f32, tone: Tone, height: f32) -> Element<'a, M> {
    let lit = gauge_portion(fraction);
    let mut bar = Row::new();
    if lit > 0 {
        bar = bar.push(
            container(space_widget::vertical().height(height))
                .width(Length::FillPortion(lit))
                .style(theme::gauge_fill(tone, theme::radius::PILL)),
        );
    }
    if lit < GAUGE_SCALE {
        // A zero-portion child would divide the row's width by zero, so the
        // empty half is pushed only when there is an empty half.
        bar = bar.push(space_widget::horizontal().width(Length::FillPortion(GAUGE_SCALE - lit)));
    }
    container(bar).width(Length::Fill).height(height).style(theme::gauge_track(theme::radius::PILL)).into()
}

/// `fraction` as a `FillPortion` share of [`GAUGE_SCALE`].
///
/// The `max(12)` is the sliver rule: a real-but-tiny reading must still be
/// visible, because a bar that rounds 0.4% down to nothing makes "barely busy"
/// and "not measured" look identical. Exactly zero still draws an empty track.
fn gauge_portion(fraction: f32) -> u16 {
    if !fraction.is_finite() || fraction <= 0.0 {
        return 0;
    }
    let raw = (fraction.min(1.0) * GAUGE_SCALE as f32).round() as u16;
    raw.max(12).min(GAUGE_SCALE)
}

/// One thin vertical bar per core — the strip under the CPU gauge.
///
/// Worth its own widget because the average hides the shape: eight cores at 12%
/// and one core pinned with seven idle are the same 12% and very different
/// machines, and the second one is what a stuck single-threaded job looks like.
pub fn core_bars<'a, M: 'a>(usage: &[f32], tone: Tone) -> Element<'a, M> {
    const HEIGHT: f32 = 34.0;
    if usage.is_empty() {
        return space_widget::horizontal().into();
    }
    Row::with_children(usage.iter().map(|u| {
        let lit = gauge_portion(u / 100.0);
        let mut col = Column::new();
        // Grown from the bottom, so the strip reads as a bar chart rather than
        // as a column of blocks that happen to be different lengths.
        if lit < GAUGE_SCALE {
            col = col.push(space_widget::vertical().height(Length::FillPortion(GAUGE_SCALE - lit)));
        }
        if lit > 0 {
            col = col.push(
                container(space_widget::horizontal().width(Length::Fill))
                    .height(Length::FillPortion(lit))
                    .style(theme::gauge_fill(tone, theme::radius::SM)),
            );
        }
        container(col.height(Length::Fill))
            .width(Length::Fill)
            .height(HEIGHT)
            .style(theme::gauge_track(theme::radius::SM))
            .into()
    }))
    .spacing(2.0)
    .height(HEIGHT)
    .into()
}

/// A labelled [`gauge`]: name on the left, value on the right, bar underneath,
/// and an optional line of detail below that.
///
/// The value is `mono` so a column of these does not jitter as digits change
/// width — the meter is meant to be watched, and text that reflows while you
/// watch it reads as the number changing more than it did.
pub fn gauge_row<'a, M: 'a>(
    label: impl text::IntoFragment<'a>,
    value: impl text::IntoFragment<'a>,
    fraction: f32,
    tone: Tone,
    note: Option<Element<'a, M>>,
) -> Element<'a, M> {
    let mut col = column![
        row![
            text(label).size(font::XS).width(Length::Fill).style(theme::text_muted),
            mono_toned(value, tone),
        ]
        .spacing(space::SM)
        .align_y(iced::Alignment::Center),
        gauge(fraction, tone),
    ]
    .spacing(space::XS);
    if let Some(note) = note {
        col = col.push(note);
    }
    col.into()
}

/// `0-1` as a whole-number percent. Whole numbers on purpose: a decimal place on
/// a figure that is resampled every few seconds is precision the sample does not
/// have, and it makes the tile flicker.
pub fn percent(fraction: f32) -> String {
    if !fraction.is_finite() {
        return "--".to_string();
    }
    format!("{}%", (fraction.clamp(0.0, 1.0) * 100.0).round())
}

/// `used / total` as a fraction, with the divide-by-zero answered rather than
/// produced. A total of zero is a real state here — swap turned off — and the
/// caller draws it as "off", so it must not arrive as `NaN`.
pub fn fraction_of(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (used as f32 / total as f32).clamp(0.0, 1.0)
    }
}

/// A radial gauge: a 270° arc with the percentage in the middle of it.
///
/// The one widget here that is worth a canvas. [`gauge`]'s bar answers "how full
/// is this" in a row of many; a dial answers "how hard is this machine working"
/// as the thing you look at first, and the arc's sweep reads at a glance from
/// across a desk in a way a 6px bar does not. Cost is one geometry per redraw of
/// a widget that changes every 5–20 s, which is why the *rows* below it are
/// still bars — a dial per number would be a page of clocks.
///
/// Opening at the bottom, which is where a dial's opening goes; the arc runs
/// clockwise from lower-left, and the canvas's y-down axis is what makes an
/// increasing angle turn that way.
pub fn dial<'a, M: 'a>(fraction: f32, tone: Tone) -> Element<'a, M> {
    canvas(Dial { fraction, tone }).width(DIAL_SIZE).height(DIAL_SIZE).into()
}

const DIAL_SIZE: f32 = 116.0;
const DIAL_STROKE: f32 = 10.0;
/// Lower-left, in canvas radians (y down, so this points down and to the left).
const DIAL_START: f32 = std::f32::consts::PI * 0.75;
/// 270°, leaving the quarter at the bottom open.
const DIAL_SWEEP: f32 = std::f32::consts::PI * 1.5;

struct Dial {
    fraction: f32,
    tone: Tone,
}

impl<M> canvas::Program<M> for Dial {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        theme: &iced::Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let t = theme::tokens(theme);
        let color = theme::tone_color(&t, self.tone);
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let center = iced::Point::new(bounds.width / 2.0, bounds.height / 2.0);
        // Inset by half the stroke, or the round caps clip against the bounds.
        let radius = (bounds.width.min(bounds.height) - DIAL_STROKE) / 2.0 - 1.0;
        let arc = |end: f32| {
            canvas::Path::new(|p| {
                p.arc(canvas::path::Arc {
                    center,
                    radius,
                    start_angle: iced::Radians(DIAL_START),
                    end_angle: iced::Radians(end),
                })
            })
        };
        let stroke = |c: iced::Color| {
            canvas::Stroke::default()
                .with_color(c)
                .with_width(DIAL_STROKE)
                .with_line_cap(canvas::LineCap::Round)
        };

        frame.stroke(
            &arc(DIAL_START + DIAL_SWEEP),
            stroke(theme::track_color(&t)),
        );
        if self.fraction.is_finite() && self.fraction > 0.0 {
            // The floor is the round cap's own width: below it the value arc is
            // a dot that reads as an artefact rather than as a reading.
            let filled = self.fraction.clamp(0.0, 1.0).max(0.015);
            frame.stroke(&arc(DIAL_START + DIAL_SWEEP * filled), stroke(color));
        }
        frame.fill_text(canvas::Text {
            content: percent(self.fraction),
            position: center,
            color,
            size: 24.0.into(),
            font: font::SEMIBOLD,
            align_x: iced::alignment::Horizontal::Center.into(),
            align_y: iced::alignment::Vertical::Center,
            ..canvas::Text::default()
        });
        vec![frame.into_geometry()]
    }
}

/// A [`dial`] with its name and one line of detail: the page's headline unit.
pub fn dial_tile<'a, M: 'a>(
    glyph: Icon,
    label: &'a str,
    fraction: f32,
    note: impl text::IntoFragment<'a>,
) -> Element<'a, M> {
    let tone = load_tone(fraction);
    container(
        column![
            row![
                glyph.glyph().size(font::XS).style(theme::text_tone(tone)),
                text(label).size(font::XS).style(theme::text_muted),
            ]
            .spacing(space::XS)
            .align_y(iced::Alignment::Center),
            dial(fraction, tone),
            text(note).size(font::XS).style(theme::text_muted),
        ]
        .spacing(space::SM)
        .align_x(iced::Alignment::Center),
    )
    .padding(space::MD)
    .width(Length::Fill)
    .style(theme::card)
    .into()
}

/// The tone a load reads at. Green until the machine has real headroom gone,
/// amber where another job would start to hurt, red where it already does.
///
/// Not a warning system — nothing here is broken at 95% CPU, it is a machine
/// doing what it was told. The colour is the same "how loud" scale the resource
/// tiers use, one step further along.
pub fn load_tone(fraction: f32) -> Tone {
    if !fraction.is_finite() {
        Tone::Neutral
    } else if fraction >= 0.90 {
        Tone::Danger
    } else if fraction >= 0.70 {
        Tone::Warning
    } else {
        Tone::Success
    }
}

/// Braille dots — the classic CLI spinner, cycled by an ever-incrementing
/// frame counter (see `coder::Message::AnimTick`). Text, not an icon glyph: it
/// draws in any monospace font, so it costs nothing beyond what [`badge_icon`]
/// already costs.
const SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// The spinner's glyph for a given frame. `frame` wraps freely; only its
/// remainder mod the frame count is ever read.
pub fn spinner_char(frame: u8) -> char {
    SPINNER_FRAMES[frame as usize % SPINNER_FRAMES.len()]
}

/// [`badge_icon`], but the leading glyph spins instead of sitting still — the
/// difference between "this is broken" and "this is working" when a step can
/// take minutes and the badge's text does not change in between.
pub fn badge_spinner<'a, M: 'a>(
    frame: u8,
    label: impl text::IntoFragment<'a>,
    tone: Tone,
) -> Element<'a, M> {
    container(
        row![
            text(spinner_char(frame))
                .size(font::XS)
                .font(iced::Font::MONOSPACE)
                .style(theme::text_tone(tone)),
            text(label).size(font::XS).style(theme::text_tone(tone)),
        ]
        .spacing(space::XS)
        .align_y(iced::Alignment::Center),
    )
    .padding(Padding::from([2.0, space::SM]))
    .style(theme::badge(tone))
    .into()
}

/// The glyph that stands for a tone: used by [`alert`] and status badges.
pub fn tone_icon(tone: Tone) -> Icon {
    match tone {
        Tone::Success => Icon::CheckCircle,
        Tone::Warning => Icon::Alert,
        Tone::Danger => Icon::XCircle,
        Tone::Info | Tone::Neutral => Icon::Info,
    }
}

/// Big-number tile for counts (shadcn dashboard card). Sized to share a row:
/// the value wraps inside the tile rather than spilling past its border.
pub fn stat<'a, M: 'a>(
    glyph: Icon,
    label: &'a str,
    value: impl text::IntoFragment<'a>,
) -> Element<'a, M> {
    container(
        column![
            row![
                glyph.glyph().size(font::XS).style(theme::text_muted),
                text(label).size(font::XS).width(Length::Fill).style(theme::text_muted),
            ]
            .spacing(space::XS)
            .align_y(iced::Alignment::Center),
            text(value).size(font::LG).width(Length::Fill).style(theme::text_default),
        ]
        .spacing(space::XS),
    )
    .padding(space::MD)
    .width(Length::Fill)
    .style(theme::card)
    .into()
}

/// `<Alert>` — inline banner.
pub fn alert<'a, M: 'a>(
    tone: Tone,
    title_text: impl text::IntoFragment<'a>,
    body_text: Option<Element<'a, M>>,
) -> Element<'a, M> {
    let mut col = column![row![
        tone_icon(tone).glyph().size(font::SM).style(theme::text_tone(tone)),
        text(title_text).size(font::SM).style(theme::text_tone(tone)),
    ]
    .spacing(space::SM)
    .align_y(iced::Alignment::Center)]
    .spacing(space::XS);
    if let Some(b) = body_text {
        col = col.push(b);
    }
    container(col)
        .padding(space::MD)
        .width(Length::Fill)
        .style(theme::alert(tone))
        .into()
}

pub fn alert_error<'a, M: 'a>(message: impl text::IntoFragment<'a>) -> Element<'a, M> {
    alert(Tone::Danger, message, None)
}

/// [`alert_error`], but for a message the client may have suffixed with
/// `" · trace {id}"` ([`agent_platform_client::Error`]'s `Display`) — the
/// trace id a failed request's server-side log line carries. When present, a
/// "View logs" button jumps to that request's log lines. The id stays in the
/// text: it is what goes in a bug report, and the Logs screen may no longer
/// hold the line if the ring has wrapped.
pub fn alert_error_traced<'a, M: 'a + Clone>(
    message: &str,
    on_view_logs: impl Fn(String) -> M + 'a,
) -> Element<'a, M> {
    match message.rsplit_once(" · trace ") {
        Some((_, trace_id)) => alert(
            Tone::Danger,
            message.to_string(),
            Some(button_ghost(Icon::Scroll, "View logs", on_view_logs(trace_id.to_string()))),
        ),
        None => alert(Tone::Danger, message.to_string(), None),
    }
}

pub fn alert_warning<'a, M: 'a>(message: impl text::IntoFragment<'a>) -> Element<'a, M> {
    alert(Tone::Warning, message, None)
}

/// `<Input>`
pub fn input<'a, M: 'a + Clone>(
    placeholder: &'a str,
    value: &'a str,
    on_input: impl Fn(String) -> M + 'a,
) -> Element<'a, M> {
    text_input(placeholder, value)
        .on_input(on_input)
        .size(font::SM)
        .padding(Padding::from([8.0, 12.0]))
        .style(theme::input)
        .into()
}

/// `<Checkbox>` — 16px box, body-sized label, tokens instead of iced defaults.
pub fn checkbox<'a, M: 'a + Clone>(
    label: impl text::IntoFragment<'a>,
    checked: bool,
    on_toggle: impl Fn(bool) -> M + 'a,
) -> Element<'a, M> {
    iced::widget::checkbox(checked)
        .label(label)
        .on_toggle(on_toggle)
        .size(16)
        .text_size(font::SM)
        .style(theme::checkbox)
        .into()
}

/// `<Input>` that submits on Enter — the composer of any chat box.
pub fn input_submit<'a, M: 'a + Clone>(
    placeholder: &'a str,
    value: &'a str,
    on_input: impl Fn(String) -> M + 'a,
    on_submit: M,
) -> Element<'a, M> {
    text_input(placeholder, value)
        .on_input(on_input)
        .on_submit(on_submit)
        .size(font::SM)
        .padding(Padding::from([8.0, 12.0]))
        .style(theme::input)
        .into()
}

/// [`input_submit`], set in the monospace font — the one editable surface in
/// the app that shows a string the user might type straight over (the Search
/// screen's dork box). Same style otherwise, so it reads as an input and not
/// as a code block someone forgot to make editable.
pub fn input_mono_submit<'a, M: 'a + Clone>(
    placeholder: &'a str,
    value: &'a str,
    on_input: impl Fn(String) -> M + 'a,
    on_submit: M,
) -> Element<'a, M> {
    text_input(placeholder, value)
        .on_input(on_input)
        .on_submit(on_submit)
        .size(font::XS)
        .font(iced::Font::MONOSPACE)
        .padding(Padding::from([8.0, 12.0]))
        .style(theme::input)
        .into()
}

/// One chat turn: role tag over the content, the user's own turns on a tinted
/// surface so a thread reads as a conversation instead of a stack of cards.
pub fn turn<'a, M: 'a>(
    label: &'a str,
    tone: Tone,
    is_user: bool,
    content: Element<'a, M>,
) -> Element<'a, M> {
    let inner = Column::with_children(vec![badge(label, tone), content]).spacing(space::XS);
    let c = container(inner).padding(space::SM).width(Length::Fill);
    if is_user { c.style(theme::code_block).into() } else { c.into() }
}

/// The scrolling half of a chat surface: a thread of [`turn`]s that snaps to the
/// end as it grows. `id` is what `operation::snap_to_end` addresses, so each
/// surface passes its own.
///
/// The right padding is not decoration — iced 0.14's scrollbar floats over the
/// content and clips the trailing edge of every card without it (see
/// `desktop/CLAUDE.md`). Getting that wrong twice is why this is one function.
pub fn transcript<'a, M: 'a>(
    id: iced::widget::Id,
    turns: Vec<Element<'a, M>>,
) -> Element<'a, M> {
    scrollable(stack_lg(turns).padding(Padding { right: 12.0, ..Default::default() }))
        .id(id)
        .height(Length::Fill)
        .into()
}

/// The typing half: one submitting input, plus whatever trailing controls say
/// what happens next — Send, a spinner, a mic. Returned uncarded, because
/// callers differ on what goes *under* the row (E.V. nothing, Coder a clock).
pub fn composer<'a, M: 'a + Clone>(
    placeholder: &'a str,
    value: &'a str,
    on_input: impl Fn(String) -> M + 'a,
    on_submit: M,
    trailing: Vec<Element<'a, M>>,
) -> Row<'a, M> {
    let mut children: Vec<Element<'a, M>> =
        vec![container(input_submit(placeholder, value, on_input, on_submit))
            .width(Length::Fill)
            .into()];
    children.extend(trailing);
    cluster(children)
}

/// The gate between a model and anything outside the app — a write through the
/// API, or a command on the machine. Shows what will happen verbatim (a
/// friendlier summary would mean agreeing to something other than what runs)
/// and defaults to nothing happening.
///
/// `run` is `None` when there is nothing runnable to approve: a model that
/// leaks its tool syntax as prose gets salvaged with whatever arguments
/// survived, and an empty command under a live Run button is the one thing this
/// card must never be.
pub fn approval<'a, M: 'a + Clone>(
    heading: impl text::IntoFragment<'a>,
    tone: Tone,
    body: Vec<Element<'a, M>>,
    no_label: &'a str,
    on_no: M,
    run: Option<M>,
) -> Element<'a, M> {
    approval_extra(heading, tone, body, no_label, on_no, run, None)
}

/// [`approval`] with one more control between No and Run — the standing answer
/// ("always allow this"), which is a third decision and not a variant of either.
pub fn approval_extra<'a, M: 'a + Clone>(
    heading: impl text::IntoFragment<'a>,
    tone: Tone,
    body: Vec<Element<'a, M>>,
    no_label: &'a str,
    on_no: M,
    run: Option<M>,
    extra: Option<Element<'a, M>>,
) -> Element<'a, M> {
    let mut head: Vec<Element<'a, M>> = vec![
        badge_icon(Icon::Alert, heading, tone),
        spacer(),
        button_ghost(Icon::X, no_label, on_no),
    ];
    head.extend(extra);
    if let Some(run) = run {
        head.push(button_default(Icon::Play, "Run", run));
    }
    let mut lines: Vec<Element<'a, M>> = vec![cluster(head).into()];
    lines.extend(body);
    card(Column::with_children(lines).spacing(space::SM))
}

/// The provider + model pair every chat surface puts in its header. Empty means
/// the server's default, which is why neither picker gets a required marker —
/// and why deselecting needs its own button beside this (`pick_list` cannot).
pub fn model_pickers<'a, M: 'a + Clone>(
    providers: Vec<String>,
    provider: &str,
    on_provider: impl Fn(String) -> M + 'a,
    models: Vec<String>,
    model: &str,
    on_model: impl Fn(String) -> M + 'a,
) -> Row<'a, M> {
    let some = |s: &str| (!s.is_empty()).then(|| s.to_string());
    cluster(vec![
        container(select("Provider", providers, some(provider), on_provider)).width(180).into(),
        container(select("Model", models, some(model), on_model)).width(260).into(),
    ])
}

/// Any banner with a Dismiss on its right. `extra` sits between the two, for
/// repairs specific to what the banner says (opening mic settings, say), and is
/// usually empty.
pub fn dismissible<'a, M: 'a + Clone>(
    inner: Element<'a, M>,
    on_dismiss: M,
    extra: Vec<Element<'a, M>>,
) -> Element<'a, M> {
    let mut row: Vec<Element<'a, M>> = vec![container(inner).width(Length::Fill).into()];
    row.extend(extra);
    row.push(button_ghost(Icon::X, "Dismiss", on_dismiss));
    cluster(row).into()
}

/// A turn's error, over the composer that will retry it: the message, the way
/// into the logs behind it, and the way to dismiss it.
pub fn error_bar<'a, M: 'a + Clone>(
    message: &'a str,
    on_trace: impl Fn(String) -> M + 'a,
    on_dismiss: M,
    extra: Vec<Element<'a, M>>,
) -> Element<'a, M> {
    dismissible(alert_error_traced(message, on_trace), on_dismiss, extra)
}

/// Collapsible chain-of-thought section above a reasoning model's reply: a
/// ghost toggle, and the thought stream in muted text while open. Shown only
/// when the model actually streamed reasoning — callers skip it otherwise.
pub fn thinking<'a, M: 'a + Clone>(reasoning: &'a str, open: bool, toggle: M) -> Element<'a, M> {
    let head = button_ghost(
        Icon::Sparkles,
        if open { "Hide thinking" } else { "Thinking" },
        toggle,
    );
    if !open {
        return head;
    }
    Column::with_children(vec![head, muted(reasoning)]).spacing(space::XS).into()
}

/// `<Input>` with a leading glyph (search fields, filters).
pub fn input_icon<'a, M: 'a + Clone>(
    glyph: Icon,
    placeholder: &'a str,
    value: &'a str,
    on_input: impl Fn(String) -> M + 'a,
) -> Element<'a, M> {
    text_input(placeholder, value)
        .on_input(on_input)
        .size(font::SM)
        .icon(text_input::Icon {
            font: icon::FONT,
            code_point: glyph.code_point(),
            size: Some(font::SM.into()),
            spacing: space::SM,
            side: text_input::Side::Left,
        })
        .padding(Padding::from([8.0, 12.0]))
        .style(theme::input)
        .into()
}

/// `<Select>` — styled dropdown.
pub fn select<'a, T, M>(
    placeholder: &'a str,
    options: Vec<T>,
    selected: Option<T>,
    on_select: impl Fn(T) -> M + 'a,
) -> Element<'a, M>
where
    T: ToString + PartialEq + Clone + 'a,
    M: 'a + Clone,
{
    iced::widget::pick_list(options, selected, on_select)
        .placeholder(placeholder)
        .width(Length::Fill)
        // Extra right padding reserves room for the caret; pick_list does not
        // shorten its label, so a long option would otherwise run under it.
        .padding(Padding { top: 8.0, right: 30.0, bottom: 8.0, left: 12.0 })
        .text_size(font::SM)
        .style(theme::select)
        .menu_style(theme::select_menu)
        .into()
}

/// Selectable list row.
pub fn list_item<'a, M: 'a + Clone>(
    content: impl Into<Element<'a, M>>,
    selected: bool,
    on_press: M,
) -> Element<'a, M> {
    button(content)
        .width(Length::Fill)
        .padding(space::SM)
        .style(theme::list_item(selected))
        .on_press(on_press)
        .into()
}

/// Selectable list row for dense lists (log lines), where [`list_item`]'s
/// padding would halve how many rows fit on screen.
pub fn list_item_compact<'a, M: 'a + Clone>(
    content: impl Into<Element<'a, M>>,
    selected: bool,
    on_press: M,
) -> Element<'a, M> {
    button(content)
        .width(Length::Fill)
        .padding(Padding::from([1.0, space::XS]))
        .style(theme::list_item(selected))
        .on_press(on_press)
        .into()
}

/// Monospace block for logs, code, curl samples.
pub fn code<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Element<'a, M> {
    container(content)
        .padding(space::SM)
        .width(Length::Fill)
        .style(theme::code_block)
        .into()
}

/// Placeholder for an empty list or unfetched pane.
pub fn empty_state<'a, M: 'a>(message: impl text::IntoFragment<'a>) -> Element<'a, M> {
    empty_state_icon(Icon::Inbox, message)
}

/// Empty state with a chosen glyph (e.g. a clock while waiting).
pub fn empty_state_icon<'a, M: 'a>(
    glyph: Icon,
    message: impl text::IntoFragment<'a>,
) -> Element<'a, M> {
    empty_state_body(glyph, message, None)
}

/// Empty state that names the next action, not just the absence.
pub fn empty_state_action<'a, M: 'a>(
    glyph: Icon,
    message: impl text::IntoFragment<'a>,
    action: Element<'a, M>,
) -> Element<'a, M> {
    empty_state_body(glyph, message, Some(action))
}

fn empty_state_body<'a, M: 'a>(
    glyph: Icon,
    message: impl text::IntoFragment<'a>,
    action: Option<Element<'a, M>>,
) -> Element<'a, M> {
    let mut col = column![
        icon::icon_large(glyph, 28.0),
        text(message).size(font::SM).style(theme::text_muted),
    ]
    .spacing(space::SM)
    .align_x(iced::Alignment::Center);
    if let Some(action) = action {
        col = col.push(action);
    }
    container(col)
        .padding(space::XL)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::{fraction_of, gauge_portion, meter_cells, percent, GAUGE_SCALE};

    #[test]
    fn the_meter_stays_countable_and_never_hides_live_work() {
        // Small lanes are one cell per unit — the count you can read at a glance.
        assert_eq!(meter_cells(0, 4), Some((4, 0)));
        assert_eq!(meter_cells(3, 4), Some((4, 3)));
        // Wide lanes go proportional at the cell cap.
        assert_eq!(meter_cells(16, 16), Some((10, 10)));
        assert_eq!(meter_cells(8, 16), Some((10, 5)));
        // The reason it rounds up: one call in flight must not read as idle.
        assert_eq!(meter_cells(1, 16), Some((10, 1)));
        // More in flight than the limit is possible for one beat after a shrink;
        // it clamps rather than overflowing the row.
        assert_eq!(meter_cells(9, 4), Some((4, 4)));
        // Nothing to draw is nothing drawn, not an empty track.
        assert_eq!(meter_cells(0, 0), None);
    }
    /// The gauge's two edge cases: a real-but-tiny reading has to be visible, and
    /// nothing-at-all has to stay an empty track. Between them is the difference
    /// between "barely busy" and "not measured", which the page draws as the same
    /// thing if this rounds a sliver away.
    #[test]
    fn a_gauge_shows_a_sliver_for_a_live_reading_and_nothing_for_none() {
        assert_eq!(gauge_portion(0.0), 0, "an idle machine is an empty track");
        assert_eq!(gauge_portion(1.0), GAUGE_SCALE);
        assert_eq!(gauge_portion(0.5), GAUGE_SCALE / 2);
        assert!(gauge_portion(0.0004) >= 12, "0.04% still has to draw");
        // Out of range at both ends, and the value a divide-by-zero would leave.
        assert_eq!(gauge_portion(2.0), GAUGE_SCALE);
        assert_eq!(gauge_portion(-1.0), 0);
        assert_eq!(gauge_portion(f32::NAN), 0);
    }

    /// Swap turned off is a total of zero, and it reaches this as a division.
    #[test]
    fn a_zero_total_is_answered_rather_than_divided_by() {
        assert_eq!(fraction_of(0, 0), 0.0);
        assert_eq!(fraction_of(1, 4), 0.25);
        assert_eq!(fraction_of(9, 4), 1.0, "used past total clamps rather than overflowing");
        assert_eq!(percent(0.615), "62%");
        assert_eq!(percent(f32::NAN), "--");
    }
}
