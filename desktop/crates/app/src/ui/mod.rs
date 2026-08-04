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
    button, column, container, row, rule, scrollable, space as space_widget, text, text_input,
    Column, Row,
};
use iced::{Element, Length, Padding};

pub use icon::{icon, icon_muted, Icon};
pub use theme::{font, space, ButtonVariant, Tone};

// ---------------------------------------------------------------------------
// Typography (shadcn's Tailwind type scale)
// ---------------------------------------------------------------------------

/// `text-2xl font-semibold` — page title.
pub fn title<'a, M: 'a>(content: impl text::IntoFragment<'a>) -> Element<'a, M> {
    text(content).size(font::XL2).style(theme::text_default).into()
}

/// `text-lg font-semibold` — card title.
pub fn heading<'a, M: 'a>(content: impl text::IntoFragment<'a>) -> Element<'a, M> {
    text(content).size(font::LG).style(theme::text_default).into()
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
pub fn button_sized<'a, M: 'a + Clone>(
    glyph: Option<Icon>,
    label: &'a str,
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
    label: &'a str,
    on_press: M,
) -> Element<'a, M> {
    button_sized(Some(glyph), label, ButtonVariant::Default, Size::Sm, Some(on_press))
}

/// `<Button variant="secondary">`
pub fn button_secondary<'a, M: 'a + Clone>(
    glyph: Icon,
    label: &'a str,
    on_press: M,
) -> Element<'a, M> {
    button_sized(Some(glyph), label, ButtonVariant::Secondary, Size::Sm, Some(on_press))
}

/// `<Button variant="outline">`
pub fn button_outline<'a, M: 'a + Clone>(
    glyph: Icon,
    label: &'a str,
    on_press: M,
) -> Element<'a, M> {
    button_sized(Some(glyph), label, ButtonVariant::Outline, Size::Sm, Some(on_press))
}

/// `<Button variant="ghost">`
pub fn button_ghost<'a, M: 'a + Clone>(
    glyph: Icon,
    label: &'a str,
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

/// `<Button variant="destructive">`
pub fn button_destructive<'a, M: 'a + Clone>(
    glyph: Icon,
    label: &'a str,
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

// ---------------------------------------------------------------------------
// Card / layout
// ---------------------------------------------------------------------------

/// `<Card>` — bordered surface with shadow.
pub fn card<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Element<'a, M> {
    container(content)
        .padding(space::MD)
        .width(Length::Fill)
        .style(theme::card)
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
    page_fixed(title_text, description, actions, scrollable(content.into()).height(Length::Fill))
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
    container(
        column![head, container(content.into()).height(Length::Fill)]
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
pub fn cluster<'a, M: 'a>(children: Vec<Element<'a, M>>) -> Row<'a, M> {
    Row::with_children(children)
        .spacing(space::SM)
        .align_y(iced::Alignment::Center)
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
    container(
        column![
            icon::icon_large(glyph, 28.0),
            text(message).size(font::SM).style(theme::text_muted),
        ]
        .spacing(space::SM)
        .align_x(iced::Alignment::Center),
    )
    .padding(space::XL)
    .width(Length::Fill)
    .center_x(Length::Fill)
    .into()
}
