//! Studio → Ads rendering — state and `update` are `studio_ads.rs`.
//!
//! Takes the **parent** [`crate::studio::State`] rather than the ads state
//! alone, because the pictures on these cards are Studio's media jobs, in
//! Studio's image cache. Reading them here is what keeps this screen free of a
//! second poll and a second copy of every finished picture in memory.
//!
//! Three blocks: the brand brief for the chosen project, the composer, and the
//! campaigns. Each campaign is a row of variant cards — picture on top, the
//! post text under it, and the copy buttons that are the point of the screen.

use crate::studio::State;
use crate::studio_ads::{Message, FIELDS, VARIANT_CHOICES};
use crate::ui::{self, space, Icon, Tone};
use agent_platform_client::types::{AdCampaign, AdVariant, MediaJob};
use iced::widget::{container, image, Row};
use iced::{Element, Length};

pub fn view(state: &State) -> Element<'_, Message> {
    let ads = &state.ads;
    let mut blocks: Vec<Element<'_, Message>> = Vec::new();

    if let Some(error) = &ads.error {
        blocks.push(ui::error_bar(error, Message::TraceLogs, Message::Dismiss, Vec::new()));
    }
    if let Some(card) = undersized_model_card(state) {
        blocks.push(card);
    }
    blocks.push(project_picker(state));
    if ads.project.is_some() {
        blocks.push(brand_card(state));
        blocks.push(composer(state));
    }
    blocks.push(campaigns(state));

    ui::stack_lg(blocks).into()
}

/// Shown when the checkpoint ComfyUI would pick cannot draw the size this
/// platform needs — in practice, an install whose only model is SD 1.5.
///
/// A card and not a disabled button: the generation will *work*, it will just
/// look wrong, and the user is entitled to try it anyway. Same rule the backend
/// cards on the Create tab follow — explain, never block.
fn undersized_model_card(state: &State) -> Option<Element<'_, Message>> {
    let model = state.status.as_ref()?.image_model.as_deref();
    let warning =
        crate::studio_ads::undersized_model(model, state.ads.selected_platform())?;
    Some(ui::alert(
        Tone::Warning,
        "This model is smaller than the ad size",
        Some(ui::stack(vec![
            ui::muted(warning),
            ui::caption(
                "The words are unaffected — only the picture is. Generating anyway is fine if \
                 you are testing the copy.",
            ),
        ])
        .into()),
    ))
}

fn project_picker(state: &State) -> Element<'_, Message> {
    let ads = &state.ads;
    if ads.projects.is_empty() {
        return ui::alert(
            Tone::Info,
            "No projects yet",
            Some(
                ui::muted(
                    "An ad is written from a project's brand brief, so there has to be a project \
                     first. Make one in Library, then come back.",
                )
                .into(),
            ),
        );
    }

    let names: Vec<String> = ads.projects.iter().map(|p| p.name.clone()).collect();
    let selected = ads.selected_project().map(|p| p.name.clone());
    let by_name: Vec<(String, i64)> = ads.projects.iter().map(|p| (p.name.clone(), p.id)).collect();

    ui::field(
        "Advertising",
        ui::select("Which project?", names, selected, move |name: String| {
            Message::ProjectPicked(by_name.iter().find(|(n, _)| *n == name).map(|(_, id)| *id))
        }),
    )
}

/// The standing facts, per project. Saved explicitly: it is prompt input a
/// person rewrites mid-thought, and the campaign reads the *stored* copy — so
/// the header says plainly when there are edits that would not be used.
fn brand_card(state: &State) -> Element<'_, Message> {
    let ads = &state.ads;
    let rows: Vec<Element<'_, Message>> = FIELDS
        .iter()
        .map(|field| {
            let field = *field;
            ui::field(
                field.label(),
                ui::input(field.placeholder(), field.get(&ads.brand), move |v| {
                    Message::BrandFieldChanged(field, v)
                }),
            )
        })
        .chain(std::iter::once(
            ui::cluster(vec![
                ui::button_secondary(
                    Icon::Save,
                    if ads.saving { "Saving…" } else { "Save brief" },
                    Message::SaveBrand,
                ),
                ui::caption(if ads.brand_dirty {
                    "Unsaved. An ad is written from the saved brief, so save before generating."
                } else {
                    "Saved. Every ad for this project is written from it."
                }),
            ])
            .into(),
        ))
        .collect();

    ui::card_with_header(
        "Brand brief",
        Some(ui::muted(
            "Who you are and how you sound. Written once per project, then used by every ad it \
             produces.",
        )),
        None,
        ui::stack(rows),
    )
}

fn composer(state: &State) -> Element<'_, Message> {
    let ads = &state.ads;

    let platform_chips: Vec<(&str, bool, Message)> = ads
        .platforms
        .iter()
        .map(|p| {
            (
                p.label.as_str(),
                ads.platform.as_deref() == Some(p.id.as_str()),
                Message::PlatformPicked(p.id.clone()),
            )
        })
        .collect();

    let mut rows: Vec<Element<'_, Message>> = vec![
        ui::input(
            "Launching the compliance audit tool — free for the first month",
            &ads.brief,
            Message::BriefChanged,
        ),
        ui::field("Where it is going", ui::chips(platform_chips)),
    ];

    // The size and the platform's own limits, so a caption that comes back long
    // is a surprise the user was warned about rather than one they discover in
    // the post box.
    if let Some(platform) = ads.selected_platform() {
        rows.push(ui::caption(format!(
            "{}×{} · caption up to {} characters · {}",
            platform.width,
            platform.height,
            platform.caption_limit,
            ui::count(platform.hashtag_max as usize, "hashtag", "hashtags")
        )));
        rows.push(ui::caption(platform.note.clone()));
    }

    rows.push(team_field(state));
    rows.push(ui::field(
        "How many",
        ui::chips(
            VARIANT_CHOICES
                .iter()
                .map(|n| {
                    (
                        if *n == 1 { "1 ad" } else if *n == 3 { "3 ads" } else { "5 ads" },
                        ads.variants == *n,
                        Message::VariantsChanged(*n),
                    )
                })
                .collect::<Vec<_>>(),
        ),
    ));

    rows.push(
        ui::cluster(vec![
            ui::button_default(
                Icon::Sparkles,
                if ads.busy { "Writing…" } else { "Generate ads" },
                Message::Generate,
            ),
            ui::caption(match ads.blocker() {
                Some(blocker) => blocker,
                None => "The words come back in seconds; the pictures render after, in the gallery.",
            }),
        ])
        .into(),
    );

    ui::card_with_header(
        "New campaign",
        Some(ui::muted(
            "One line about this particular ad. The standing facts come from the brief above.",
        )),
        None,
        ui::stack(rows),
    )
}

/// Which roster writes the copy. "Default team" is a chip rather than an empty
/// select, because no team named is the ordinary case and the server has a
/// social media marketing roster of its own for exactly it.
fn team_field(state: &State) -> Element<'_, Message> {
    let ads = &state.ads;
    let mut chips: Vec<(&str, bool, Message)> =
        vec![("Social media marketing", ads.team.is_none(), Message::TeamPicked(None))];
    chips.extend(
        ads.teams
            .iter()
            .map(|t| (t.name.as_str(), ads.team == Some(t.id), Message::TeamPicked(Some(t.id)))),
    );

    let mut rows: Vec<Element<'_, Message>> = vec![ui::chips(chips)];
    if ads.team.is_none() {
        rows.push(ui::caption(
            "A strategist, a copywriter, an art director and a social lead. Build your own in \
             Library to write in a different voice.",
        ));
    }
    ui::field("Written by", ui::stack(rows))
}

fn campaigns(state: &State) -> Element<'_, Message> {
    let ads = &state.ads;
    if ads.campaigns.is_empty() {
        return ui::section(
            "Campaigns",
            None,
            ui::empty_state_icon(
                Icon::Send,
                "No ads yet. What you generate shows up here, newest first, with the text ready \
                 to copy.",
            ),
        );
    }

    let blocks: Vec<Element<'_, Message>> =
        ads.campaigns.iter().map(|c| campaign_block(state, c)).collect();
    ui::section("Campaigns", None, ui::stack_lg(blocks))
}

fn campaign_block<'a>(state: &'a State, campaign: &'a AdCampaign) -> Element<'a, Message> {
    let header = ui::cluster(vec![
        ui::badge(campaign.platform_label.as_deref().unwrap_or(campaign.platform.as_str()), Tone::Neutral),
        ui::badge(ui::count(campaign.variants.len(), "ad", "ads"), Tone::Info),
        ui::spacer(),
        ui::icon_tip(Icon::Trash, "Delete this campaign", Message::Delete(campaign.id)),
    ]);

    let cards: Vec<Element<'a, Message>> = campaign
        .variants
        .iter()
        .enumerate()
        .map(|(i, v)| variant_card(state, campaign, i, v))
        .collect();

    ui::card(ui::stack(vec![
        header.into(),
        ui::body(campaign.brief.clone()),
        Row::with_children(cards).spacing(space::MD).wrap().into(),
    ]))
}

/// Fixed enough to tile beside its siblings, and the same preview height
/// whatever aspect the platform asked for.
const CARD_W: f32 = 320.0;
const PREVIEW_H: f32 = 260.0;

fn variant_card<'a>(
    state: &'a State,
    campaign: &'a AdCampaign,
    index: usize,
    variant: &'a AdVariant,
) -> Element<'a, Message> {
    let copied = state.ads.copied == Some((campaign.id, index));
    let mut rows: Vec<Element<'a, Message>> = vec![preview(state, variant)];

    rows.push(ui::body(variant.caption.clone()));
    if !variant.cta.trim().is_empty() {
        rows.push(ui::muted(variant.cta.clone()));
    }
    if !variant.hashtags.is_empty() {
        rows.push(ui::caption(variant.hashtags.join(" ")));
    }

    let mut buttons = Vec::new();
    // Play first: on a video card it is the only way to see what was made.
    if let Some(job) = variant
        .media_job_id
        .and_then(|id| state.jobs.iter().find(|j| j.id == id))
        .filter(|j| j.is_video() && j.is_done())
    {
        buttons.push(ui::button_secondary(Icon::Play, "Play", Message::OpenMedia(job.id)));
    }
    buttons.push(ui::button_secondary(
        if copied { Icon::Check } else { Icon::Copy },
        if copied { "Copied" } else { "Copy post" },
        Message::CopyPost(campaign.id, index),
    ));
    // The tags on their own: plenty of people put them in the first comment
    // rather than the caption, and re-selecting them by hand is the fiddliest
    // part of posting.
    if !variant.hashtags.is_empty() {
        buttons.push(ui::button_ghost(
            Icon::Copy,
            "Tags only",
            Message::CopyTags(campaign.id, index),
        ));
    }
    rows.push(ui::cluster(buttons).into());

    container(ui::tile(ui::stack(rows))).width(CARD_W).into()
}

/// The picture, or the honest stand-in. The job is looked up in Studio's own
/// gallery, so a picture that has finished there has finished here.
fn preview<'a>(state: &'a State, variant: &'a AdVariant) -> Element<'a, Message> {
    let placeholder = |glyph, message: String| -> Element<'a, Message> {
        container(ui::empty_state_icon(glyph, message))
            .height(PREVIEW_H)
            .width(Length::Fill)
            .center_y(PREVIEW_H)
            .into()
    };

    let Some(job_id) = variant.media_job_id else {
        return placeholder(
            Icon::XCircle,
            variant
                .media_error
                .clone()
                .unwrap_or_else(|| "No picture was started for this one.".to_string()),
        );
    };
    let Some(job) = state.jobs.iter().find(|j| j.id == job_id) else {
        // Studio keeps the last hundred jobs; an older campaign's picture has
        // aged out of that list rather than gone wrong.
        return placeholder(Icon::Image, "The picture is no longer in the gallery.".to_string());
    };

    match job.status.as_str() {
        "failed" => placeholder(
            Icon::XCircle,
            job.error.clone().unwrap_or_else(|| "This one did not render.".to_string()),
        ),
        // Studio never fetches video bytes and iced cannot decode them, so a
        // finished clip is a placeholder plus the button that opens it.
        "completed" if job.is_video() => {
            placeholder(Icon::Film, "Clip ready — press Play to open it.".to_string())
        }
        "completed" => match state.images.get(&job.id) {
            Some(handle) => container(
                image(handle.clone())
                    .width(Length::Fill)
                    .height(PREVIEW_H)
                    .content_fit(iced::ContentFit::Contain),
            )
            .height(PREVIEW_H)
            .into(),
            None => placeholder(Icon::Image, "Loading the picture…".to_string()),
        },
        _ => placeholder(Icon::Clock, rendering_line(state, job)),
    }
}

/// "Rendering · 40s elapsed", reusing Studio's own measured estimate rather
/// than inventing a second one.
fn rendering_line(state: &State, job: &MediaJob) -> String {
    match crate::studio::elapsed_secs(job) {
        Some(elapsed) => {
            let elapsed = crate::studio::format_secs(elapsed);
            match state.eta_secs(job) {
                Some(0) => format!("Rendering · any moment now ({elapsed} elapsed)"),
                Some(left) => {
                    format!("Rendering · ~{} left ({elapsed} elapsed)", crate::studio::format_secs(left))
                }
                None => format!("Rendering · {elapsed} elapsed"),
            }
        }
        None => "Rendering. This card fills in by itself.".to_string(),
    }
}
