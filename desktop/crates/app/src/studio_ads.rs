//! Studio → Ads: social advertisements from a project's brand brief (ADR 0017).
//! State and `update` only — rendering is `studio_ads_view.rs`, per the root
//! `CLAUDE.md` split.
//!
//! The server does the work (`/api/v1/ads/*`): one model round-trip writes
//! every variant's caption, hashtags, call to action and picture prompt, then
//! starts an ordinary media job per variant. So this screen has **no poll and
//! no image cache of its own** — the pictures are [`crate::studio`]'s jobs, in
//! its gallery, fetched by its cache, and the ads view reads them from the
//! parent state. A card here and a card in the gallery are the same picture.
//!
//! Three things it does own:
//!
//! **The brand brief**, per project, edited in place and saved explicitly. Not
//! autosaved: it is prompt input a user rewrites mid-thought, and a save on
//! every keystroke would store half-sentences and race the campaign that reads
//! them.
//!
//! **The platform list**, fetched rather than hard-coded. The server's sizes
//! are the ones its media seam will actually honour — a preset invented here
//! could be silently rewritten on the way through.
//!
//! **What is on the clipboard.** The whole point of the screen is copyable
//! post text, so the button that copied last says so until another one does.

use agent_platform_client::types::{
    AdBrand, AdCampaign, AdCampaignCreate, AdPlatform, ProjectSummary, TeamTemplateSummary,
};
use agent_platform_client::Client;
use iced::Task;

use crate::domain::err_string;

/// Which line of the brand brief an edit lands on. One message with a field
/// tag rather than six near-identical messages — they all do the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Company,
    Product,
    Audience,
    Voice,
    Link,
    Avoid,
}

impl Field {
    pub fn label(self) -> &'static str {
        match self {
            Field::Company => "Company",
            Field::Product => "What it is",
            Field::Audience => "Who it is for",
            Field::Voice => "Voice",
            Field::Link => "Link",
            Field::Avoid => "Never do this",
        }
    }

    pub fn placeholder(self) -> &'static str {
        match self {
            Field::Company => "Devstrail",
            Field::Product => "A software studio that ships internal tools for small teams",
            Field::Audience => "Founders and ops leads at 5–50 person companies",
            Field::Voice => "Plain and specific. No exclamation marks, no hype words.",
            Field::Link => "https://devstrail.com",
            Field::Avoid => "Never claim we are the cheapest, never name a competitor",
        }
    }

    pub fn get(self, brand: &AdBrand) -> &str {
        match self {
            Field::Company => &brand.company,
            Field::Product => &brand.product,
            Field::Audience => &brand.audience,
            Field::Voice => &brand.voice,
            Field::Link => &brand.link,
            Field::Avoid => &brand.avoid,
        }
    }

    fn set(self, brand: &mut AdBrand, value: String) {
        match self {
            Field::Company => brand.company = value,
            Field::Product => brand.product = value,
            Field::Audience => brand.audience = value,
            Field::Voice => brand.voice = value,
            Field::Link => brand.link = value,
            Field::Avoid => brand.avoid = value,
        }
    }
}

/// The order the editor renders them in — the order someone would actually
/// fill them, not the order the struct declares.
pub const FIELDS: [Field; 6] =
    [Field::Company, Field::Product, Field::Audience, Field::Voice, Field::Link, Field::Avoid];

/// How many ads one press writes. Three is enough to choose from and cheap
/// enough to wait for; the server clamps to 1–6 whatever is asked.
pub const VARIANT_CHOICES: [i64; 3] = [1, 3, 5];
pub const DEFAULT_VARIANTS: i64 = 3;

/// The largest edge a checkpoint family composes without repeating the
/// subject, or `None` when the family is not one we recognise.
///
/// Only SD 1.x is listed, and it is listed because it is what a bare ComfyUI
/// install actually ends up holding: `media::choose_checkpoint` prefers Flux,
/// Z-Image, SDXL and SD3, then falls back to *whatever is there*. A machine
/// with only `v1-5-pruned-emaonly.safetensors` therefore draws every ad on a
/// model trained at 512², and past roughly 768 that model duplicates heads and
/// limbs rather than composing a taller frame.
///
/// An unrecognised name yields `None` — no warning. Refusing on missing
/// information would put a scary card over a working install, which is the
/// same rule `MediaStatus::supports` follows.
fn model_edge_ceiling(model: &str) -> Option<i64> {
    let name = model.to_ascii_lowercase();
    const SD15: [&str; 5] = ["v1-5", "v1.5", "sd15", "sd-v1", "sd_v1"];
    SD15.iter().any(|m| name.contains(m)).then_some(768)
}

/// The sentence to show when the installed checkpoint cannot draw the size this
/// platform needs — `None` when it can, or when we do not recognise the model.
///
/// Every ad preset is at least 1088 on its long edge, so on SD 1.5 this fires
/// for all of them. That is the honest answer: the fix is a bigger model, not a
/// smaller platform, and the message says so rather than offering a choice that
/// does not exist.
pub fn undersized_model(image_model: Option<&str>, platform: Option<&AdPlatform>) -> Option<String> {
    let model = image_model?;
    let platform = platform?;
    // `image_model` is the checkpoint the *image* templates load. A video
    // platform runs the Wan graph, which names its own files and never sees
    // this one — warning about it there would be pointing at the wrong model.
    if platform.is_video() {
        return None;
    }
    let ceiling = model_edge_ceiling(model)?;
    let longest = platform.width.max(platform.height);
    (longest > ceiling).then(|| {
        format!(
            "{model} is the checkpoint ComfyUI would use, and it draws well up to about \
             {ceiling}px. {} needs {}×{}, where this model tends to repeat the subject rather \
             than fill the frame. Put an SDXL, Flux or Z-Image checkpoint in \
             ComfyUI/models/checkpoints and press Refresh.",
            platform.label, platform.width, platform.height
        )
    })
}

#[derive(Default)]
pub struct State {
    pub projects: Vec<ProjectSummary>,
    pub project: Option<i64>,
    /// The open project's brief. Empty for a project that has none yet, which
    /// is the ordinary starting state and not an error.
    pub brand: AdBrand,
    /// Which project `brand` belongs to. Guards against a slow fetch landing
    /// after the user has already moved to another project and writing its
    /// brief over theirs.
    brand_for: Option<i64>,
    /// Edited since it was loaded or saved — what the Save button reacts to.
    pub brand_dirty: bool,
    pub saving: bool,
    pub platforms: Vec<AdPlatform>,
    pub platform: Option<String>,
    pub brief: String,
    /// Teams the user has built. Empty is fine — the server has its own
    /// social media marketing roster and uses it when none is named.
    pub teams: Vec<TeamTemplateSummary>,
    pub team: Option<i64>,
    pub variants: i64,
    pub campaigns: Vec<AdCampaign>,
    pub busy: bool,
    /// `(campaign id, variant index)` of the last text copied — the button
    /// that did it says "Copied" until another one takes over.
    pub copied: Option<(i64, usize)>,
    pub error: Option<String>,
}

impl State {
    /// The brief the picker is showing, if it is still in the list.
    pub fn selected_project(&self) -> Option<&ProjectSummary> {
        self.project.and_then(|id| self.projects.iter().find(|p| p.id == id))
    }

    pub fn selected_platform(&self) -> Option<&AdPlatform> {
        self.platform.as_deref().and_then(|id| self.platforms.iter().find(|p| p.id == id))
    }

    /// Whether Generate can do anything. Checked here rather than only in the
    /// view so the press has one rule, and the view can explain the same one.
    pub fn blocker(&self) -> Option<&'static str> {
        if self.project.is_none() {
            return Some("Pick the project this ad is for.");
        }
        if self.brand.is_empty() {
            return Some(
                "This project has no brand brief yet. Company, what it is, and who it is for is \
                 enough to start.",
            );
        }
        if self.platform.is_none() {
            return Some("Pick where the ad is going.");
        }
        if self.brief.trim().is_empty() {
            return Some("Say what this particular ad is about.");
        }
        None
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    /// "View logs" on a traced error banner. Forwarded up through
    /// `studio::update` so `main::update` can intercept it the way it does for
    /// every other screen.
    TraceLogs(String),
    Dismiss,
    /// Screen entry: everything that does not depend on the chosen project.
    Refresh,
    ProjectsLoaded(Result<Vec<ProjectSummary>, String>),
    PlatformsLoaded(Result<Vec<AdPlatform>, String>),
    TeamsLoaded(Result<Vec<TeamTemplateSummary>, String>),
    ProjectPicked(Option<i64>),
    BrandLoaded(i64, Result<Box<AdBrand>, String>),
    BrandFieldChanged(Field, String),
    SaveBrand,
    BrandSaved(Result<Box<AdBrand>, String>),
    PlatformPicked(String),
    BriefChanged(String),
    TeamPicked(Option<i64>),
    VariantsChanged(i64),
    Generate,
    Generated(Result<Box<AdCampaign>, String>),
    CampaignsLoaded(Result<Vec<AdCampaign>, String>),
    /// Put one variant's post text on the clipboard.
    CopyPost(i64, usize),
    /// Copy only the hashtag line — the half people paste into a first comment.
    CopyTags(i64, usize),
    /// Hand a finished clip to the desktop's player — iced cannot decode video
    /// (ADR 0009), so this is the only way to watch one. Intercepted in
    /// `studio::update`, which already owns the write-then-open path.
    OpenMedia(i64),
    Delete(i64),
    Deleted(Result<i64, String>),
}

pub fn update(state: &mut State, client: &Client, message: Message) -> Task<Message> {
    match message {
        Message::TraceLogs(_) => Task::none(),
        Message::Dismiss => {
            state.error = None;
            Task::none()
        }
        Message::Refresh => {
            if state.variants == 0 {
                state.variants = DEFAULT_VARIANTS;
            }
            let mut tasks = vec![load_projects(client), load_platforms(client), load_teams(client)];
            // A project already chosen keeps its brief and its campaigns across
            // a refresh; without this, switching tabs blanks the editor.
            if let Some(id) = state.project {
                tasks.push(load_brand(client, id));
                tasks.push(load_campaigns(client, id));
            }
            Task::batch(tasks)
        }
        Message::ProjectsLoaded(Ok(projects)) => {
            state.error = None;
            // One project is not a choice — open it rather than making the user
            // pick from a list of one.
            let first = (state.project.is_none() && projects.len() == 1).then(|| projects[0].id);
            state.projects = projects;
            match first {
                Some(id) => update(state, client, Message::ProjectPicked(Some(id))),
                None => Task::none(),
            }
        }
        Message::ProjectsLoaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Message::PlatformsLoaded(Ok(platforms)) => {
            state.error = None;
            if state.platform.is_none() {
                state.platform = platforms.first().map(|p| p.id.clone());
            }
            state.platforms = platforms;
            Task::none()
        }
        Message::PlatformsLoaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        // Not a banner: no teams at all is a working state, because the server
        // has its own roster. Failing this costs the picker, not the screen.
        Message::TeamsLoaded(Ok(teams)) => {
            state.teams = teams;
            Task::none()
        }
        Message::TeamsLoaded(Err(_)) => {
            state.teams.clear();
            Task::none()
        }
        Message::ProjectPicked(id) => {
            if state.project == id {
                return Task::none();
            }
            state.project = id;
            // Everything below the picker belongs to the old project.
            state.brand = AdBrand::default();
            state.brand_for = None;
            state.brand_dirty = false;
            state.campaigns.clear();
            state.copied = None;
            match id {
                Some(id) => Task::batch([load_brand(client, id), load_campaigns(client, id)]),
                None => Task::none(),
            }
        }
        Message::BrandLoaded(for_project, Ok(brand)) => {
            // A fetch that landed after the user moved on, or after they
            // started typing, must not overwrite what is on screen.
            if state.project != Some(for_project) || state.brand_dirty {
                return Task::none();
            }
            state.error = None;
            state.brand = *brand;
            state.brand_for = Some(for_project);
            Task::none()
        }
        Message::BrandLoaded(_, Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Message::BrandFieldChanged(field, value) => {
            field.set(&mut state.brand, value);
            state.brand_dirty = true;
            Task::none()
        }
        Message::SaveBrand => {
            let Some(project_id) = state.project else {
                return Task::none();
            };
            if state.saving {
                return Task::none();
            }
            state.saving = true;
            state.error = None;
            let (client, brand) = (client.clone(), state.brand.clone());
            Task::perform(
                async move {
                    err_string(client.set_project_brand(project_id, &brand).await).map(Box::new)
                },
                Message::BrandSaved,
            )
        }
        Message::BrandSaved(Ok(brand)) => {
            state.saving = false;
            state.brand_dirty = false;
            // What the server kept, not what we hoped it stored — the reply is
            // the truth about a field it may have trimmed.
            state.brand = *brand;
            Task::none()
        }
        Message::BrandSaved(Err(e)) => {
            state.saving = false;
            state.error = Some(e);
            Task::none()
        }
        Message::PlatformPicked(id) => {
            state.platform = Some(id);
            Task::none()
        }
        Message::BriefChanged(v) => {
            state.brief = v;
            Task::none()
        }
        Message::TeamPicked(id) => {
            state.team = id;
            Task::none()
        }
        Message::VariantsChanged(n) => {
            state.variants = n;
            Task::none()
        }
        Message::Generate => generate(state, client),
        Message::Generated(Ok(campaign)) => {
            state.busy = false;
            state.error = None;
            // A campaign whose every picture was refused still arrives; say so
            // once here rather than leaving a row of empty cards to interpret.
            if let Some(why) = every_picture_refused(&campaign) {
                state.error = Some(format!(
                    "The copy was written, but no picture could be started: {why}"
                ));
            }
            state.campaigns.insert(0, *campaign);
            Task::none()
        }
        Message::Generated(Err(e)) => {
            state.busy = false;
            state.error = Some(e);
            Task::none()
        }
        Message::CampaignsLoaded(Ok(campaigns)) => {
            state.error = None;
            state.campaigns = campaigns;
            Task::none()
        }
        Message::CampaignsLoaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Message::CopyPost(campaign_id, index) => match variant_text(state, campaign_id, index, false)
        {
            Some(text) => {
                state.copied = Some((campaign_id, index));
                iced::clipboard::write(text)
            }
            None => Task::none(),
        },
        Message::CopyTags(campaign_id, index) => {
            match variant_text(state, campaign_id, index, true) {
                Some(text) => {
                    state.copied = Some((campaign_id, index));
                    iced::clipboard::write(text)
                }
                None => Task::none(),
            }
        }
        // Handled by `studio::update` before it reaches here.
        Message::OpenMedia(_) => Task::none(),
        Message::Delete(id) => {
            let client = client.clone();
            Task::perform(
                async move { err_string(client.delete_ad_campaign(id).await).map(|_| id) },
                Message::Deleted,
            )
        }
        Message::Deleted(Ok(id)) => {
            state.campaigns.retain(|c| c.id != id);
            if state.copied.is_some_and(|(c, _)| c == id) {
                state.copied = None;
            }
            Task::none()
        }
        Message::Deleted(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
    }
}

/// Every variant refused its picture, and the first reason why — `None` when at
/// least one is rendering, because then the screen shows progress instead.
fn every_picture_refused(campaign: &AdCampaign) -> Option<String> {
    if campaign.variants.is_empty() || campaign.variants.iter().any(|v| v.media_job_id.is_some()) {
        return None;
    }
    campaign
        .variants
        .iter()
        .find_map(|v| v.media_error.clone())
        .or_else(|| Some("the media backend did not accept the job".to_string()))
}

fn variant_text(state: &State, campaign_id: i64, index: usize, tags_only: bool) -> Option<String> {
    let variant = state
        .campaigns
        .iter()
        .find(|c| c.id == campaign_id)
        .and_then(|c| c.variants.get(index))?;
    let text = if tags_only { variant.hashtags.join(" ") } else { variant.post_text() };
    (!text.trim().is_empty()).then_some(text)
}

fn generate(state: &mut State, client: &Client) -> Task<Message> {
    if let Some(blocker) = state.blocker() {
        state.error = Some(blocker.to_string());
        return Task::none();
    }
    // Saving first would be a second failure mode on the way to the one the
    // user pressed; the server reads the *stored* brief, so unsaved edits
    // simply are not in this campaign, and the view says so beside the button.
    let (Some(project_id), Some(platform)) = (state.project, state.platform.clone()) else {
        return Task::none();
    };
    state.busy = true;
    state.error = None;

    let body = AdCampaignCreate {
        project_id,
        platform,
        brief: state.brief.trim().to_string(),
        team_template_id: state.team,
        variants: Some(state.variants.max(1)),
    };
    let client = client.clone();
    Task::perform(
        async move { err_string(client.create_ad_campaign(&body).await).map(Box::new) },
        Message::Generated,
    )
}

fn load_projects(client: &Client) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move { err_string(client.projects().await).map(|r| r.projects) },
        Message::ProjectsLoaded,
    )
}

fn load_platforms(client: &Client) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move { err_string(client.ad_platforms().await).map(|r| r.platforms) },
        Message::PlatformsLoaded,
    )
}

fn load_teams(client: &Client) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move { err_string(client.teams().await).map(|r| r.teams) },
        Message::TeamsLoaded,
    )
}

fn load_brand(client: &Client, project_id: i64) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move {
            (project_id, err_string(client.project_brand(project_id).await).map(Box::new))
        },
        |(id, result)| Message::BrandLoaded(id, result),
    )
}

fn load_campaigns(client: &Client, project_id: i64) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move { err_string(client.ad_campaigns(Some(project_id)).await).map(|r| r.campaigns) },
        Message::CampaignsLoaded,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn client() -> Client {
        Client::new("http://127.0.0.1:1", "k")
    }

    fn project(id: i64, name: &str) -> ProjectSummary {
        serde_json::from_value(json!({
            "id": id, "name": name, "description": null, "color": null,
            "created_at": "2026-08-30T00:00:00", "updated_at": "2026-08-30T00:00:00"
        }))
        .unwrap()
    }

    fn brand() -> AdBrand {
        AdBrand { company: "Devstrail".into(), product: "internal tools".into(), ..AdBrand::default() }
    }

    fn campaign(id: i64, job: Option<i64>, media_error: Option<&str>) -> AdCampaign {
        serde_json::from_value(json!({
            "id": id, "project_id": 1, "platform": "ig_feed", "brief": "launch",
            "variants": [{
                "caption": "Ship it.", "hashtags": ["#a", "#b"], "cta": "See example.com",
                "image_prompt": "p", "negative": "", "media_job_id": job,
                "media_error": media_error
            }],
            "created_at": "2026-08-30T00:00:00", "updated_at": "2026-08-30T00:00:00"
        }))
        .unwrap()
    }

    /// The gate the Generate button reads. Each blocker names the next thing to
    /// do, and a complete form has none.
    #[test]
    fn the_blocker_walks_the_form_in_order() {
        let (mut s, c) = (State::default(), client());
        assert!(s.blocker().unwrap().contains("project"));

        let _ = update(&mut s, &c, Message::ProjectsLoaded(Ok(vec![project(1, "Devstrail")])));
        assert_eq!(s.project, Some(1), "a single project opens itself");
        assert!(s.blocker().unwrap().contains("brand brief"));

        s.brand = brand();
        assert!(s.blocker().unwrap().contains("where the ad is going"));

        s.platform = Some("ig_feed".into());
        assert!(s.blocker().unwrap().contains("what this particular ad is about"));

        s.brief = "launching the audit tool".into();
        assert_eq!(s.blocker(), None);
    }

    /// Pressing Generate with the form incomplete must not reach the network,
    /// and must say which part is missing.
    #[test]
    fn generate_refuses_an_incomplete_form_without_a_request() {
        let (mut s, c) = (State::default(), client());
        let _ = update(&mut s, &c, Message::Generate);
        assert!(s.error.is_some());
        assert!(!s.busy, "nothing was started, so nothing is in flight");
    }

    /// Two projects, two briefs. Switching must drop the old one entirely —
    /// generating against project B with project A's brief on screen would be
    /// an ad about the wrong company.
    #[test]
    fn switching_project_drops_the_previous_brief_and_campaigns() {
        let (mut s, c) = (State::default(), client());
        s.projects = vec![project(1, "A"), project(2, "B")];
        let _ = update(&mut s, &c, Message::ProjectPicked(Some(1)));
        let _ = update(&mut s, &c, Message::BrandLoaded(1, Ok(Box::new(brand()))));
        s.campaigns = vec![campaign(7, Some(3), None)];
        assert_eq!(s.brand.company, "Devstrail");

        let _ = update(&mut s, &c, Message::ProjectPicked(Some(2)));
        assert!(s.brand.is_empty(), "project B starts with its own blank brief");
        assert!(s.campaigns.is_empty());
        assert!(!s.brand_dirty);
    }

    /// A brief fetched for a project the user has already left, or one that
    /// lands while they are typing, must not overwrite the editor.
    #[test]
    fn a_late_brand_fetch_never_overwrites_what_is_on_screen() {
        let (mut s, c) = (State::default(), client());
        s.projects = vec![project(1, "A"), project(2, "B")];
        let _ = update(&mut s, &c, Message::ProjectPicked(Some(2)));

        let _ = update(&mut s, &c, Message::BrandLoaded(1, Ok(Box::new(brand()))));
        assert!(s.brand.is_empty(), "project 1's brief is not project 2's");

        let _ = update(&mut s, &c, Message::BrandFieldChanged(Field::Company, "Typed".into()));
        assert!(s.brand_dirty);
        let _ = update(&mut s, &c, Message::BrandLoaded(2, Ok(Box::new(brand()))));
        assert_eq!(s.brand.company, "Typed", "an in-flight fetch must not eat an edit");
    }

    /// Saving clears the dirty flag and takes the server's stored copy.
    #[test]
    fn saving_takes_what_the_server_kept() {
        let (mut s, c) = (State::default(), client());
        s.project = Some(1);
        let _ = update(&mut s, &c, Message::BrandFieldChanged(Field::Company, " Devstrail ".into()));
        assert!(s.brand_dirty);

        let _ = update(&mut s, &c, Message::SaveBrand);
        assert!(s.saving);
        let stored = AdBrand { company: "Devstrail".into(), ..AdBrand::default() };
        let _ = update(&mut s, &c, Message::BrandSaved(Ok(Box::new(stored))));
        assert!(!s.brand_dirty && !s.saving);
        assert_eq!(s.brand.company, "Devstrail");
    }

    /// A campaign whose pictures were all refused still lands — the copy is
    /// worth keeping — but the screen says why there are no pictures.
    #[test]
    fn a_campaign_with_no_pictures_lands_with_an_explanation() {
        let (mut s, c) = (State::default(), client());
        let _ = update(
            &mut s,
            &c,
            Message::Generated(Ok(Box::new(campaign(1, None, Some("ComfyUI is not running"))))),
        );
        assert_eq!(s.campaigns.len(), 1, "the words are kept");
        assert!(s.error.as_deref().is_some_and(|e| e.contains("ComfyUI is not running")));

        // One that is rendering is not an error at all.
        let mut s2 = State::default();
        let _ = update(&mut s2, &c, Message::Generated(Ok(Box::new(campaign(2, Some(9), None)))));
        assert!(s2.error.is_none());
    }

    /// Copy marks the button that did it, and asking for text that is not there
    /// changes nothing rather than marking an empty copy.
    #[test]
    fn copy_marks_only_a_variant_that_had_something_to_copy() {
        let (mut s, c) = (State::default(), client());
        s.campaigns = vec![campaign(4, Some(1), None)];

        let _ = update(&mut s, &c, Message::CopyPost(4, 0));
        assert_eq!(s.copied, Some((4, 0)));

        let _ = update(&mut s, &c, Message::CopyPost(99, 0));
        assert_eq!(s.copied, Some((4, 0)), "a campaign that is not there copies nothing");

        // Deleting the campaign clears the mark, so no button claims a copy of
        // something that is gone.
        let _ = update(&mut s, &c, Message::Deleted(Ok(4)));
        assert!(s.campaigns.is_empty());
        assert_eq!(s.copied, None);
    }

    fn platform(id: &str, w: i64, h: i64) -> AdPlatform {
        serde_json::from_value(json!({
            "id": id, "label": "Instagram story", "kind": "image", "width": w, "height": h,
            "caption_limit": 2200, "hashtag_max": 5, "note": "9:16"
        }))
        .unwrap()
    }

    /// The case a bare ComfyUI install actually lands in: SD 1.5 is the only
    /// checkpoint, so `media::choose_checkpoint` falls back to it and every ad
    /// preset asks it for a size it was never trained for. Silence there means
    /// the user generates mush and concludes the feature is broken.
    #[test]
    fn sd15_is_named_as_too_small_for_every_ad_size() {
        let story = platform("ig_story", 1088, 1920);
        let warning = undersized_model(Some("v1-5-pruned-emaonly.safetensors"), Some(&story))
            .expect("SD 1.5 cannot compose 1088x1920");
        assert!(warning.contains("v1-5-pruned-emaonly"), "name the model: {warning}");
        assert!(warning.contains("1088×1920"), "name the size it cannot do: {warning}");
        assert!(warning.contains("checkpoints"), "name the fix: {warning}");

        // The smallest preset is still 1088, so the warning is not specific to
        // the tall one — the fix really is a bigger model.
        let square = platform("ig_feed", 1088, 1088);
        assert!(undersized_model(Some("v1-5-pruned-emaonly.safetensors"), Some(&square)).is_some());
    }

    /// An unrecognised checkpoint must not raise a card over a working install,
    /// and a model that can do the size must not either.
    #[test]
    fn an_unknown_or_capable_model_says_nothing() {
        let story = platform("ig_story", 1088, 1920);
        assert_eq!(undersized_model(Some("flux1-dev.safetensors"), Some(&story)), None);
        assert_eq!(undersized_model(Some("sd_xl_base_1.0.safetensors"), Some(&story)), None);
        assert_eq!(
            undersized_model(Some("some-model-nobody-has-heard-of.safetensors"), Some(&story)),
            None,
            "unknown means no warning, not a guess"
        );
        assert_eq!(undersized_model(None, Some(&story)), None, "no model yet is not a warning");

        // SD 1.5 inside its own range is fine; nothing here asks for that, but
        // the predicate must be about the size and not about the name alone.
        let small = platform("tiny", 512, 512);
        assert_eq!(undersized_model(Some("v1-5-pruned-emaonly.safetensors"), Some(&small)), None);
    }

    /// The video preset loads the Wan graph, not the image checkpoint, so the
    /// SD 1.5 warning must not follow it there.
    #[test]
    fn the_image_model_warning_stays_out_of_the_video_preset() {
        let reel: AdPlatform = serde_json::from_value(json!({
            "id": "ig_reel", "label": "Reel", "kind": "video", "width": 720, "height": 1280,
            "caption_limit": 2200, "hashtag_max": 5, "length": 49, "note": "9:16"
        }))
        .unwrap();
        assert_eq!(
            undersized_model(Some("v1-5-pruned-emaonly.safetensors"), Some(&reel)),
            None,
            "the image checkpoint has nothing to do with the Wan graph"
        );
    }

    /// A failed team fetch costs the picker, not the screen: the server has its
    /// own roster and a campaign without a named team is the normal case.
    #[test]
    fn teams_failing_is_not_a_banner() {
        let (mut s, c) = (State::default(), client());
        let _ = update(&mut s, &c, Message::TeamsLoaded(Err("boom".into())));
        assert!(s.teams.is_empty());
        assert!(s.error.is_none());
    }
}
