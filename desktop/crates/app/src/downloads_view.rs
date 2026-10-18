//! The Downloads screen, and the same rows embedded wherever a screen started
//! a transfer.
//!
//! One [`row`] draws a job everywhere: the file's own name, its own bar, its
//! own byte count. That is the difference from what this replaced — an
//! aggregate "file 1 of 2 · 10% overall" could not say which file was moving or
//! let you touch the one that was not.
//!
//! [`panel`] is generic over the message so a screen can embed its own
//! transfers without routing through this screen's messages; both callers hand
//! in a mapper for [`Action`].

use crate::downloads::{Action, Downloads, Job, Status, Tag};
use crate::model_download::human;
use crate::ui::{self, Icon, Tone};
use crate::Message;
use iced::widget::{column, row, space as space_widget};
use iced::{Element, Length};

pub fn view(downloads: &Downloads) -> Element<'_, Message> {
    let clearable = downloads.jobs.iter().any(|j| !j.active());
    let actions = clearable
        .then(|| ui::button_ghost(Icon::Trash, "Clear finished", Message::Downloads(Action::Clear)));

    let body: Element<'_, Message> = if downloads.jobs.is_empty() {
        ui::empty_state_icon(
            Icon::Download,
            "Nothing downloading. Model files started from Studio or Settings show up here.",
        )
    } else {
        panel(downloads.jobs.iter(), Message::Downloads)
    };

    ui::page(
        "Downloads",
        Some(ui::muted(format!(
            "Model files, straight to disk. {} run at once — the rest wait their turn, and any \
             one of them can jump the queue.",
            crate::downloads::LIMIT
        ))),
        actions,
        body,
    )
}

/// The rows for some subset of the jobs, as somebody else's message type.
pub fn panel<'a, M: 'a + Clone>(
    jobs: impl Iterator<Item = &'a Job>,
    on: impl Fn(Action) -> M + Copy + 'a,
) -> Element<'a, M> {
    ui::stack(jobs.map(|job| row_for(job, on)).collect()).into()
}

/// Whether a card that only exists to show media transfers should still be on
/// screen — the last file finishing is what takes it away.
pub fn media_running(downloads: &Downloads) -> bool {
    downloads.by_tag(Tag::MediaModel).any(|j| j.active())
}

fn row_for<'a, M: 'a + Clone>(job: &'a Job, on: impl Fn(Action) -> M + 'a) -> Element<'a, M> {
    let (note, tone) = match &job.status {
        Status::Queued => ("waiting for a slot".to_string(), Tone::Neutral),
        Status::Running => (
            match job.total {
                // No `Content-Length` is common enough on a CDN redirect that a
                // bare byte count has to read as progress on its own.
                None => format!("{} so far", human(job.received)),
                Some(total) => format!(
                    "{} of {} · {}",
                    human(job.received),
                    human(total),
                    ui::percent(job.fraction())
                ),
            },
            Tone::Info,
        ),
        Status::Done => ("done".to_string(), Tone::Success),
        Status::Failed(e) => (e.clone(), Tone::Danger),
    };

    let mut controls: Vec<Element<'a, M>> = Vec::new();
    if job.status == Status::Queued {
        controls.push(ui::icon_tip(Icon::ArrowUp, "Download this one next", on(Action::Promote(job.id))));
    }
    if job.active() {
        controls.push(ui::icon_tip(Icon::X, "Cancel", on(Action::Cancel(job.id))));
    } else {
        if matches!(job.status, Status::Failed(_)) {
            controls.push(ui::icon_tip(Icon::Refresh, "Try again", on(Action::Retry(job.id))));
        }
        controls.push(ui::icon_tip(Icon::Trash, "Remove", on(Action::Cancel(job.id))));
    }

    ui::tile(column![
        row![
            ui::body(job.label.as_str()),
            space_widget::horizontal(),
            ui::toned(note, tone),
            ui::cluster(controls),
        ]
        .spacing(ui::space::SM)
        .align_y(iced::Alignment::Center)
        .width(Length::Fill),
        ui::gauge(job.fraction(), tone),
    ]
    .spacing(ui::space::XS))
}
