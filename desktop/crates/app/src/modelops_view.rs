//! Model ops rendering: projects, build launcher, job monitor, local models
//! and the adapter registry.

use crate::domain;
use crate::modelops::{Message, State, STAGES};
use crate::ui::{self, space, Icon, Tone};
use iced::widget::{checkbox, column, container, scrollable};
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
            checkbox(on)
                .label(*stage)
                .on_toggle(move |v| Message::ToggleStage(stage.to_string(), v))
                .size(16)
                .text_size(ui::font::SM)
                .into()
        })
        .collect();

    let launcher = ui::stack(vec![
        ui::caption("PIPELINE STAGES"),
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
            ui::badge_icon(ui::tone_icon(tone), job.status.clone(), tone),
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
                        cells.push(ui::badge("active", Tone::Success));
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
