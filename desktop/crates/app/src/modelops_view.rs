//! Model ops rendering: projects, build launcher, job monitor, local models
//! and the adapter registry.

use crate::domain;
use crate::modelops::{Message, State, STAGES};
use crate::ui::{self, space, Icon, Tone};
use iced::widget::{column, container, scrollable};
use iced::{Element, Length};

pub fn view(state: &State) -> Element<'_, Message> {
    let mut blocks: Vec<Element<'_, Message>> = Vec::new();

    if let Some(err) = &state.error {
        blocks.push(ui::error_bar(err, Message::TraceLogs, Message::DismissNotice, Vec::new()));
    }
    if let Some(draft) = &state.new_project {
        blocks.push(ui::card_with_header(
            "New model project",
            None,
            Some(
                ui::cluster(vec![
                    ui::button_default(Icon::Plus, "Create", Message::CreateProject),
                    ui::button_ghost(Icon::X, "Cancel", Message::CancelNewProject),
                ])
                .into(),
            ),
            ui::stack(vec![
                ui::field("Name", ui::input("my-adapter", &draft.name, Message::NewNameChanged)),
                ui::field(
                    "Description",
                    ui::input("What is it for?", &draft.description, Message::NewDescriptionChanged),
                ),
                ui::field(
                    "Base model",
                    ui::input("qwen2.5:7b", &draft.base_model, Message::NewBaseModelChanged),
                ),
            ]),
        ));
    }

    blocks.push(projects_card(state));
    if let Some(job) = &state.job {
        blocks.push(job_card(state, job));
    }
    blocks.push(ollama_card(state));
    blocks.push(registry_card(state));

    ui::page(
        "Model ops",
        Some(ui::muted("Fine-tune adapters, watch build jobs, and manage local models.")),
        Some(
            ui::cluster(vec![
                ui::button_secondary(Icon::Plus, "New project", Message::NewProject),
                ui::button_outline(Icon::Refresh, "Refresh", Message::Refresh),
            ])
            .into(),
        ),
        {
            let body: Element<'_, Message> =
                scrollable(ui::stack_lg(blocks)).height(Length::Fill).into();
            body
        },
    )
}

fn projects_card(state: &State) -> Element<'_, Message> {
    if state.projects.is_empty() {
        // Keep the header the populated card has, so the empty page is not a
        // nameless box between two titled ones.
        return ui::card_with_header(
            "Model projects",
            Some(ui::muted("Pick a project, choose stages, and run the pipeline.")),
            None,
            if !state.loaded {
                ui::empty_state_icon(Icon::Clock, "Loading…")
            } else {
                ui::empty_state_icon(Icon::Cpu, "No model projects yet.")
            },
        );
    }

    let rows: Vec<Element<'_, Message>> = state
        .projects
        .iter()
        .map(|p| {
            let selected = state.selected.as_deref() == Some(p.name.as_str());
            ui::list_item(
                ui::cluster(vec![
                    ui::body(p.name.clone()),
                    ui::caption(p.description.clone().unwrap_or_else(|| "—".into())),
                    ui::spacer(),
                    ui::badge(
                        ui::count(p.registry_entries.len(), "version", "versions"),
                        Tone::Neutral,
                    ),
                ]),
                selected,
                Message::Select(p.name.clone()),
            )
        })
        .collect();

    let stage_toggles: Vec<Element<'_, Message>> = STAGES
        .iter()
        .map(|stage| {
            let on = state.stages.iter().any(|s| s == stage);
            ui::checkbox(*stage, on, move |v| Message::ToggleStage(stage.to_string(), v))
        })
        .collect();

    let launcher = ui::stack(vec![
        ui::caption("Pipeline stages"),
        ui::cluster(stage_toggles).into(),
        ui::cluster(vec![
            container(ui::input(
                "Register as (optional alias)",
                &state.register_alias,
                Message::AliasChanged,
            ))
            .width(280)
            .into(),
            if state.busy {
                ui::badge("starting…", Tone::Info)
            } else {
                ui::button_default(Icon::Play, "Start build", Message::StartBuild)
            },
            if state.uploading {
                ui::badge("uploading…", Tone::Info)
            } else {
                ui::button_secondary(Icon::Upload, "Upload dataset file…", Message::PickDatasetFile)
            },
        ])
        .into(),
    ]);

    ui::card_with_header(
        "Model projects",
        Some(ui::muted("Pick a project, choose stages, and run the pipeline.")),
        None,
        column![ui::stack(rows), ui::separator(), launcher].spacing(space::MD),
    )
}

/// `93m`, `2h 14m`. Only used here, so it lives here rather than in `domain`.
fn short_duration(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

/// The bar and the numbers under it, when the running stage is reporting any.
///
/// Everything is optional because it comes from a stage's own marker line:
/// `prepare` prints a phase and a sentence, `train` prints a step count, a loss
/// and an ETA. Rendering only what arrived keeps a short stage from showing a
/// row of dashes, and lets a stage start reporting more without a change here.
fn progress_rows<'a>(
    job: &'a agent_platform_client::types::ModelBuildJob,
    tone: Tone,
) -> Vec<Element<'a, Message>> {
    let progress = &job.progress;
    let Some(fields) = progress.as_object().filter(|f| !f.is_empty()) else {
        return Vec::new();
    };
    let number = |key: &str| fields.get(key).and_then(serde_json::Value::as_f64);
    let step = number("step").unwrap_or(0.0) as usize;
    let total = number("total_steps").unwrap_or(0.0) as usize;

    let mut rows: Vec<Element<'a, Message>> = Vec::new();
    if total > 0 {
        rows.push(ui::meter(step, total, tone));
    }

    let mut parts: Vec<String> = Vec::new();
    if total > 0 {
        parts.push(format!("step {step}/{total} ({:.0}%)", (step as f64 / total as f64) * 100.0));
    }
    if let Some(loss) = number("loss") {
        parts.push(format!("loss {loss:.4}"));
    }
    if let Some(epoch) = number("epoch") {
        parts.push(format!("epoch {epoch:.2}"));
    }
    if let Some(eta) = number("eta_s") {
        parts.push(format!("eta {}", short_duration(eta)));
    }
    if let Some(from) = number("resumed_from") {
        parts.push(format!("resumed at {}", from as usize));
    }
    if !parts.is_empty() {
        rows.push(ui::caption(parts.join(" · ")));
    }
    if let Some(message) = fields.get("message").and_then(serde_json::Value::as_str) {
        rows.push(ui::caption(message.to_string()));
    }
    rows
}

fn job_card<'a>(
    _state: &'a State,
    job: &'a agent_platform_client::types::ModelBuildJob,
) -> Element<'a, Message> {
    let tone = match job.status.as_str() {
        "completed" => Tone::Success,
        "failed" | "cancelled" => Tone::Danger,
        _ => Tone::Info,
    };

    let mut body = vec![
        ui::cluster(vec![
            ui::badge_icon(ui::tone_icon(tone), job_status_label(&job.status), tone),
            ui::caption(format!("job #{} · {}", job.id, job.job_type)),
            ui::spacer(),
            ui::caption(job.stages.join(" → ")),
            ui::caption(job_age(job)),
        ])
        .into(),
    ];
    if let Some(stage) = &job.current_stage {
        body.push(ui::field("Current stage", ui::body(stage.clone())));
    }
    body.extend(progress_rows(job, tone));
    if let Some(error) = &job.error_message {
        body.push(ui::alert(Tone::Danger, "Job failed", Some(ui::mono(error.clone()))));
    }
    if let Some(log) = &job.log_tail {
        body.push(ui::caption("LOG TAIL"));
        body.push(ui::code(ui::mono(log.clone())));
    }

    ui::card_with_header(
        "Build job",
        None,
        Some(ui::button_ghost(Icon::X, "Close", Message::CloseJob)),
        ui::stack(body),
    )
}

fn ollama_card(state: &State) -> Element<'_, Message> {
    let list: Element<'_, Message> = if state.ollama.is_empty() {
        if !state.loaded {
            ui::empty_state_icon(Icon::Clock, "Loading…")
        } else {
            ui::empty_state_icon(Icon::Cpu, "No local models found (is Ollama running?).")
        }
    } else {
        ui::stack(
            state
                .ollama
                .iter()
                .map(|m| {
                    ui::cluster(vec![
                        ui::mono(m.name.clone()),
                        ui::spacer(),
                        ui::caption(m.size.map(domain::format_size).unwrap_or_default()),
                    ])
                    .into()
                })
                .collect::<Vec<_>>(),
        )
        .into()
    };

    ui::card_with_header(
        "Local models",
        Some(ui::muted("Models Ollama has on this machine.")),
        Some(
            ui::cluster(vec![
                container(ui::input("qwen2.5:7b", &state.pull_name, Message::PullNameChanged))
                    .width(220)
                    .into(),
                if state.busy {
                    ui::badge("pulling…", Tone::Info)
                } else {
                    ui::button_secondary(Icon::Download, "Pull", Message::PullModel)
                },
            ])
            .into(),
        ),
        list,
    )
}

fn registry_card(state: &State) -> Element<'_, Message> {
    let list: Element<'_, Message> = if state.registry.is_empty() {
        if !state.loaded {
            ui::empty_state_icon(Icon::Clock, "Loading…")
        } else {
            ui::empty_state_icon(Icon::Inbox, "Nothing registered yet.")
        }
    } else {
        ui::stack(
            state
                .registry
                .iter()
                .map(|e| {
                    let mut cells = vec![
                        ui::body(e.project_name.clone().unwrap_or_else(|| format!("#{}", e.project_id))),
                        ui::caption(format!("v{}", e.version)),
                        ui::mono(e.ollama_tag.clone()),
                        ui::spacer(),
                    ];
                    if let Some(score) = e.eval_score {
                        cells.push(ui::caption(format!("eval {score:.3}")));
                    }
                    if e.is_active {
                        cells.push(ui::badge("Active", Tone::Success));
                    }
                    ui::cluster(cells).into()
                })
                .collect::<Vec<_>>(),
        )
        .into()
    };

    ui::card_with_header("Registry", Some(ui::muted("Adapters registered from builds.")), None, list)
}

/// Bytes as GB/MB, matching how the web UI labels model sizes.
/// Kept beside the registry table so job timestamps read the same as elsewhere.
pub fn job_age(job: &agent_platform_client::types::ModelBuildJob) -> String {
    domain::relative_time(&job.created_at).unwrap_or_default()
}

fn job_status_label(status: &str) -> &str {
    match status {
        "completed" => "Completed",
        "failed" => "Failed",
        "cancelled" => "Cancelled",
        "running" => "Running",
        "queued" => "Queued",
        other => other,
    }
}
