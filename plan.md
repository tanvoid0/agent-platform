# Agent Platform — continuity plan

Handoff/continuity doc for the **agent-platform** repo. History of how we got
here: `docs/native-desktop-migration.md` and `docs/adr/`.

## What it is

Two deliverables, one repo (three, if you count the training worker):

- **API server** (`desktop/crates/server`, bin `agent-platformd`): Rust/axum,
  binds `http://127.0.0.1:18410`, **serves every route itself** — multi-agent
  process orchestration (goal → planner DAG → approval → topological
  execution), the assistant, coder, todos, workflows, workspaces, tokens, and
  an **embedded** OpenAI-compatible LLM proxy on `/v1/*` in the same process.
  All REST is under **`/api/v1/...`**; `GET /`, `/health` and `/openapi.json`
  are the only routes outside it. Also the cloud artifact
  ([ADR 0007](docs/adr/0007-strangler-rust-server.md)).
- **Desktop** (`desktop/`): native Rust **iced 0.14** app — the only UI. It
  spawns (or attaches to) `agent-platformd` on port 18410 and talks to the API.
  The old `web/` React app and Tauri shell are deleted (ADR 0005).
- **Training worker** (`worker/`): the model-ops LoRA pipeline. Python, because
  it is torch and peft, but **not a server** — `agent-platformd` runs each build
  stage as a subprocess and reads results off its stdout.

**The FastAPI server is gone.** `app/` was deleted 2026-08-07 along with
`scripts/start.py`, `scripts/bundle_server.py`, `scripts/sync_contract_enums.py`,
`pytest.ini` and `requirements-dev.txt`. What that cost, and what moved rather
than died, is the section below.

## Where to edit

| Area | Path |
|------|------|
| Router, startup, the 404 fallback | `desktop/crates/server/src/lib.rs` |
| Schema (replaced Alembic) | `desktop/crates/server/migrations/` + `db::ensure_schema` (`sqlx::migrate!`); `db.rs` is also the SQLite/Postgres choke point (placeholder rewriting, pool construction) |
| Auth, tokens, accounts | `auth.rs`, `api_tokens.rs`, `accounts.rs` (magic-link, entitlements); desktop Settings → Account in `account.rs` / `account_view.rs` ([ADR 0013](docs/adr/0013-desktop-local-open-cloud-account.md)) |
| Processes / orchestrator | `{processes,executor,dag_schema}.rs` — the DAG executor and all eleven process routes |
| LLM proxy, BYOK, providers | `llm.rs` (routes), `llm_config.rs`, `byok.rs`, `provider_catalog.rs`, `model_capabilities.rs`, `model_catalog.rs`, `upstream_http.rs`, `usage.rs`; admin surface in `llm_admin.rs`, `config.yaml` validation in `config_schema.rs` |
| Local model (provider `local`) | `llm_llama_process.rs` (fetch/launch/log/stop `llama-server`) over `managed_server.rs` (the mechanism it shares with sd-server); desktop Settings → Status card in `screen.rs` ([ADR 0012](docs/adr/0012-managed-llama-server.md)) |
| Assistant "E.V." + planning chat | `{assistant,assistant_turn,clarifying_form}.rs`; desktop `assistant.rs`/`assistant_view.rs` + `stt.rs`, and `agenda.rs`/`agenda_view.rs` (board) + `agenda_chat.rs`/`agenda_chat_view.rs` (chat pane) |
| Chat | `{chat,chat_usage,chat_thread_title,context_budget}.rs` |
| Coder agent | `{coder,coder_loop,coder_tools}.rs`; desktop `coder.rs` (one session) + `coder_board.rs` (N of them) + `coder_view.rs` + `coder_tools.rs` (the desktop-side executor) + `coder_notes.rs` + `coder_git.rs` (checkpoints) + `coder_files.rs` (tree, viewer) + `coder_term.rs` (PTY terminal) |
| Todos / boards | `todos.rs`; the agent routes in `action_orchestrator.rs` |
| Workflows engine + assist | `workflows.rs` + `workflow_engine.rs` (and its interval scheduler) |
| Teams, projects | `teams.rs`, `projects.rs` |
| Workspaces, files, documents | `workspaces.rs`, `workspace_files.rs`, `documents.rs` (upload ingest + PDF extraction) |
| Model ops (Ollama, registry, build jobs) | `model_ops.rs` — all seventeen routes and the stage runner; the pipeline itself is `worker/model_ops/pipeline/` |
| Image/video generation | `media.rs` (the seam + the ComfyUI adapter) + `media_templates/*.json`, `media_sdcpp.rs` (the sd-server adapter) + `media_sdcpp_process.rs` (its lifecycle, over `managed_server.rs`); desktop `studio.rs`/`studio_view.rs` ([ADR 0009](docs/adr/0009-local-media-generation.md), [ADR 0011](docs/adr/0011-stable-diffusion-cpp-media-backend.md)) |
| Logs, status | `observability.rs` (the ring `logd!` writes to), `system.rs` (`/system/status`, `/system/logs`) |
| Resource modes, AI-call priority, machine meters | `resources.rs` (`Limits`, the two lanes, the host/GPU sampler, `/system/resources`); gated at `llm::complete_internal` and `llm::chat_completions`; desktop Settings → Performance (`machine_view` in `screen.rs`) and the sidebar monitor ([ADR 0010](docs/adr/0010-resource-modes-and-ai-call-priority.md)) |
| Env seeding, correlation ids | `dotenv.rs`, `request_id.rs` |
| Web admin console | `desktop/crates/server/src/admin.html` — one static page, `include_str!`d and served at `/admin` by `lib.rs`; no build step, no assets |
| Cloud deploy | `.github/workflows/deploy-cloud-run.yml` (a `dist` custom job listed in `dist-workspace.toml`'s `post-announce-jobs`), over the root `Dockerfile` + `docker/entrypoint.sh` |
| Shared shapes | `wire.rs`, `error.rs` |
| Desktop HTTP/SSE client | `desktop/crates/client/` (`enums.rs` is hand-maintained now — the generator went with `app/shared_enums.py`) |
| Desktop screens | `desktop/crates/app/src/` — state/update in `x.rs`, rendering in `x_view.rs` |
| API reference (Settings → API) | `desktop/crates/app/src/apidocs.rs` + `apidocs_view.rs` — parsed from `/openapi.json`, which is now a checked-in file (`desktop/crates/server/src/openapi.json`) served verbatim |
| Desktop UI kit | `desktop/crates/app/src/ui/` — shadcn-derived tokens + widgets; screens compose kit fns only |

## Runbook

```bash
cd desktop && cargo run -p agent-platform-server                # the server, alone
cd desktop && cargo run -p agent-platform-desktop               # the app (spawns it)
```

`agent-platformd` is self-contained: no child process, no interpreter. It
creates its own SQLite file and runs `migrations/sqlite/` on startup. Postgres
is opt-in through `DATABASE_URL` (`migrations/postgres/`, one `sqlx::AnyPool`
either way) and is what the cloud deployment runs — note the repo's own `.env`
sets a DSN, so a local checkout run wants it unset or `AGENT_PLATFORM_ROOT`
pointed elsewhere.

- Desktop dev needs cmake + libclang (machine paths in
  `desktop/.cargo/config.toml`). The app spawns `agent-platformd` from its own
  directory, so **build both**.
- Windows installer: `python scripts/build_installer.py` (Inno Setup) — builds
  both binaries and packages them with `worker/`. No Python runtime ships.
- Tests: `cd desktop && cargo test`. Run `cargo build` too: dev-dependencies
  unify features back into the lib, so `cargo test` alone can pass on a lib
  whose own feature list is missing something it uses.
- A running app holds `target/debug/agent-platformd.exe`; build with
  `--target-dir <somewhere outside the repo>` rather than closing it —
  `.gitignore` pins `desktop/target/` exactly, so a sibling dir shows up
  untracked.
- Hygiene: `python scripts/check_repo_hygiene.py`.
- **Schema changes**: add a new `desktop/crates/server/migrations/000N_*.sql`.
  `sqlx::migrate!` replaced Alembic; `0001_initial.sql` is the squashed head and
  is checksummed, so editing an applied migration stops every existing database
  from starting. Forward-only, no `down` scripts.
- **Build jobs** need `MODEL_OPS_PYTHON` pointing at an interpreter with torch;
  `worker/requirements.txt` lists the rest. Without it a job fails on the first
  stage with the spawn error, naming the interpreter it tried.
- **SSE** (`GET /api/v1/processes/{id}/stream`): tails `EventLog` (~0.8s poll),
  closes with a `terminal` event on terminal status or a human gate
  (`approval_required` / `task_review_required`) — clients refresh via
  `GET /processes/{id}`. Desktop gates its detail polling on the stream.

## Backlog

- **Job-pipeline task models — first project scaffolded 2026-08-28.**
  [ADR 0015](docs/adr/0015-job-pipeline-task-model.md). `jobhunt-screener` is a
  bundled scaffold under `worker/model_ops/data/projects/` — manifest, input
  schema, system prompt — installed into the live data dir by
  `worker/install_project.py`. `ensure_data_scaffold` copies `_template` and
  nothing else on purpose, so a shipped install never grows somebody else's
  projects; that script is the manual half. **Nothing is trained.** The corpus
  is the blocker, not the pipeline: the portfolio captures `scoring` only, and
  the distillation sweep of §2.2 is what fills the rest.
  - **A knowledge row is exactly two messages, user first.**
    `build_dataset._validate_example` parses `messages[0]` as the input JSON and
    `eval.py` reads `[0]` and `[1]` as prompt and expected answer, so a row
    carrying its own system message is dropped *silently* — as a schema failure
    — and the job dies a stage later on "No training examples", pointing at the
    knowledge dir. The system prompt belongs in `export/system.txt`, which
    `export_ollama` bakes into the Modelfile `SYSTEM` block.
    `worker/test_model_ops.py` asserts both halves.
  - **A `train` stage reports progress and can be picked up again — landed
    2026-08-28.** The stage prints `@@AGP:progress@@ {json}` on the same stdout
    channel as `registry_hook`; `model_ops.rs::handle_marker` keeps the newest
    payload on `model_build_jobs.progress_json` (migration `0007`) and puts it
    on both the job row and the SSE frame, so a client attaching halfway through
    a two-hour run gets step, loss, ETA and VRAM instead of a blank bar. The
    admin page's Training tab and the desktop Model ops card both render it.
    `resume` (default **true**) picks up the last checkpoint, but only when
    `checkpoints.resolve` finds the run's fingerprint — hyperparameters plus the
    dataset's SHA-256 — unchanged; `init_from` starts a new adapter version from
    an existing one's weights, which is what `incremental_train` was already
    asking for and silently not getting.
    - **`current_stage` was being read as `0` for every stage.** It is a
      `VARCHAR(32)` that `JOB_COLUMNS` cast to `BIGINT`; SQLite gives `"train"`
      integer affinity and returns `0`, so every client showed stage one for the
      whole build. The cast is gone.
    - **`adapter_version` and `init_from` are `^[a-zA-Z0-9_-]+$`**, the same as
      a project name. Both become a directory the worker creates or reads, and
      escaping them into the stage script's Python literal stops a quote but not
      a `..`.
  - **The dataset gate refuses personal data — landed 2026-08-28.** ADR 0015
    §2.4 L4. `pii_scan.require_clean` runs in `build_dataset` before the split
    and before anything is written, matches on shape rather than on literals,
    and never quotes what it found — the job log is the file it exists to keep
    clean. Salary is deliberately not a pattern: it is the training signal here,
    and is checked upstream where the exporter can compare literals. Off per
    project with `pii_scan: false` in `project.yaml`.
  - **Loss lands on the answer, not the advert.** SFT was flattening a chat row
    to one string, so the gradient covered the prompt too — on a corpus whose
    adverts are as long as real ones that was 99.2% of it. Rows now arrive as
    prompt/completion columns, which TRL masks; `max_seq_len` default is 4096
    because SFT truncates from the *end*, which is where the answer is, and
    94.3% of rows crossed 2048. A trl too old to mask is refused, not run.
  - **A trained model reaches the picker only after `export`.**
    `model_catalog.rs` lists live Ollama tags, not registry rows, and the
    `config.yaml` alias is written only when the build request carries
    `register_alias` — blank by default in the desktop Model ops form. A
    train-only build leaves a registry entry and no visible model.
- **User-owned data, local and cloud — landed 2026-08-23.**
  [ADR 0014](docs/adr/0014-user-owned-data-local-and-cloud.md). Identity is
  always a `users` row: local startup registers the OS user (`kind = local`)
  and backfills every ownerless row to it; magic-link does the same for an
  email. Migration `0006` adds `users.username` / `users.kind` and `user_id` on
  workspace, coder threads, media jobs, workflows, action sets and search
  history. Cross-user reads 404 rather than 401; the master key still sees
  every tenant.
  - **Auth errors name the failure.** A missing Bearer on a keyed server is
    `AUTH_REQUIRED`, an expired JWT is `TOKEN_EXPIRED` with a refresh hint, and
    `/health` carries `auth.required` / `auth.mode` / `auth.hint` so another app
    can see why `/api/v1/*` started 401ing. `tests/auth_and_routing.rs` asserts
    the split — a *wrong* token is still `TOKEN_INVALID`.
  - **`AuthMode::UserSession` is a tenant, not the master key.**
    `require_ai_entitlement` applies to that mode only; workspace tokens, the
    master key and the local machine user skip billing as before.
  - **The accounts page is one `include_str!`'d HTML file**, not a build
    artifact. A SvelteKit version of the same four screens shipped beside it and
    was never reachable — `/accounts` always served the embedded one — so it was
    deleted rather than finished. `AGENT_PLATFORM_ACCOUNTS_DIST` is gone with it.
  - **The Stripe webhook enforces a 300 s replay window**
    (`STRIPE_WEBHOOK_TOLERANCE_SECS`). The header's timestamp is signed, so
    without an age check a captured webhook stays valid forever. The signature
    and the event-kind table are unit-tested; an unknown kind writes nothing.
  - **Magic-link verify GET answers 303** (`Redirect::to`), not 302 — the
    desktop follows any 3xx to its loopback callback, and the IPv6 loopback
    literal `http://[::1]:…` is accepted (bracket-stripped before `IpAddr`).

- **Desktop: open loopback + optional cloud account — landed 2026-08-23.**
  [ADR 0013](docs/adr/0013-desktop-local-open-cloud-account.md). The iced app
  no longer injects a master key when it spawns `agent-platformd` — local API
  is open on loopback like Ollama. Settings → Account magic-links against a
  hosted origin; the session file is provider `platform` on the local daemon.
  SQLite and local routes never require a Portal login.
  - **Spawn sets `AGENT_PLATFORM_MASTER_KEY` empty** so a developer `.env`
    cannot re-arm it. `host_guard` and off-loopback-requires-key stay.
  - **Attach-if-running** tries unauthenticated status first, then the leftover
    install key, so an older keyed daemon is still adopted.
  - **Magic-link `redirect_uri`** must be loopback; verify GET 303s tokens to
    the desktop callback. Paste-the-link is the fallback.
  - **Allowlist** includes `agent-platform-desktop`. Cloud `/v1` still gates on
    entitlement; master / `agp_` still skip billing.

- **Cloud Run deploy — written, verified, and parked 2026-08-23.** Nothing is
  deployed and nothing calls the workflow: the `post-announce-jobs` line is
  commented out in `dist-workspace.toml`, `release.yml` is regenerated without a
  deploy job, and no `gcloud` command has ever run against this project. Reviving
  it is that one line plus `dist generate` and six repo variables. **The fixes it
  turned up are not parked** — they were bugs in their own right and are listed
  below.
  `agent-platformd` would deploy as a **service, not a "Cloud Run function"**:
  functions have no Rust runtime, the root `Dockerfile` already builds a
  server-only image, and Cloud Run bills a container exactly like a function —
  per request, scaling to zero.
  - **`.github/workflows/deploy-cloud-run.yml` is a `dist` custom job.** Wired,
    it runs *after* the GitHub Release exists so a build that failed on any of
    the four platforms never reaches production; `dist generate` gives it
    `needs: [plan, announce]` and passes a `plan` input, which is why the job
    declares one it never reads — a called workflow errors on an input it has
    not declared. Auth is Workload Identity Federation; no service-account JSON
    key exists. Config is repo `vars` + Secret Manager, nothing in the file.
    `--revision-suffix` carries the run number because `gcloud run deploy`
    refuses a revision name that already exists, which is what redeploying a tag
    (the rollback path) asks for.
  - **The image was built and measured**, not assumed: 179 MB, `agent-platformd`
    and nothing else, from the existing `Dockerfile` unchanged.
  - **`--max-instances=1` is correctness, not thrift.** `AppState` holds three
    process-memory maps: `coder_pending` (a `/chat/tool-result` must be served
    by the process that served `/chat/stream`), `model_jobs`, and the
    fixed-window rate limiter. Two instances behind one URL break the first,
    orphan the second and double every token's effective limit.
  - **Private by default, and that is the DDoS gate.** `--no-allow-unauthenticated`
    unless `vars.GCP_ALLOW_PUBLIC` says otherwise: Google rejects an anonymous
    request at its own front door, so a flood never starts a billable container.
    Every alternative — Cloud Armor, a load balancer, IAP — costs more per month
    than the service does. The admin page is reached with
    `gcloud run services proxy`, which authenticates locally and serves it on
    127.0.0.1, so the browser still needs no token. `--max-instances=1` is the
    backstop if it is ever opened: one instance cannot bill more than one vCPU
    however much arrives.
  - **Postgres, not SQLite, and never Cloud SQL.** The container filesystem is
    ephemeral, so scaling to zero would drop the database. `DATABASE_URL` comes
    from Secret Manager and the `any` pool runs `migrations/postgres/` — pointed
    at a serverless Postgres *outside* GCP. **The standing rule is free tier
    only**: Cloud SQL's smallest instance is ~$9/month and bills whether or not
    anything connects, and the same rule rules out a Serverless VPC connector,
    Cloud Build (the image is built on the GitHub runner) and any load balancer.
    What is left has to stay inside Cloud Run's 180k vCPU-s / 360k GiB-s / 2M
    requests, Artifact Registry's 0.5 GB (a cleanup policy, or the bill starts
    around release five), Secret Manager's 6 versions, and Logging's 50 GiB.
  - **`PORT` is Cloud Run's contract.** `docker/entrypoint.sh` now reads it
    between `AGENT_PLATFORM_PORT` and the 18410 default; `Config::from_env`
    already refuses a non-loopback bind without a master key, which is the check
    that makes a public URL safe to hand out.
  - **Six env vars that the desktop defaults get wrong in a container**, found by
    auditing what `serve` starts before it binds: `RESUME_ON_STARTUP=0` (a cold
    start would replay the same interrupted work every time),
    `WORKFLOW_SCHEDULER=0` (`workflow_engine` polls for due workflows and runs
    them, and a workflow can call an LLM), `BACKUP=0` (`db::backup` runs
    `VACUUM INTO` at every start — Postgres rejects the statement outright, and
    the file would land on an ephemeral disk), `LOCAL_LLM_DOCKER_FIX=0`, and
    `ORCHESTRATOR_INTERNAL_URL`/`PROXY_PUBLIC_URL` — both default to
    `127.0.0.1:18410` while the process is on 8080 there, so `llm_admin`'s
    self-calls would hit a closed port. Verified by running the daemon under
    exactly that environment: recovery logged as disabled, no scheduler line, no
    `.bak` written.
  - **sqlx had no TLS, so `DATABASE_URL` could not have reached Neon at all.**
    The features list was `runtime-tokio, sqlite, postgres, any, chrono, macros,
    migrate` — no TLS backend, which makes sqlx answer a server that demands one
    with *"TLS upgrade required by server but SQLx was built without TLS support
    enabled"*. Every hosted Postgres demands one. The suite never caught it
    because `tests/postgres.rs` runs against a local server that does not.
    `tls-rustls-ring` now, matching the provider `reqwest` already pulls —
    rustls installs a process-wide default `CryptoProvider`, and two in one
    binary is a runtime panic rather than a build error. Verified as compiled
    in, not against a live Neon.
  - **No provider key reaches the container.** `--set-env-vars` replaces the
    whole set, `--set-secrets` carries only the master key and the DSN, and the
    baked `config/agent_platform.yaml` holds no keys — every URL in it is
    loopback. The deploy cannot spend at an LLM provider because it cannot
    authenticate to one.
  - **Not in that image**: `worker/` (torch, `Dockerfile.train`) and anything
    `managed_server.rs` fetches — no GPU, ephemeral disk. Provider `local` must
    stay unselected there.
  - **`.dockerignore` was three dead `app/*.db` lines**, so it excluded nothing:
    a local `docker build` shipped 18 GB of `desktop/target` as context, and
    `.env` sat in the context of every image. It now excludes secrets, build
    output and local state — but *not* `worker/`, which `Dockerfile.train` COPYs
    from this same root context.
- **`/admin` — one static page, no build step — landed 2026-08-23.**
  The desktop app is still the UI ([ADR 0005](docs/adr/0005-native-iced-desktop-headless-server.md));
  this is the window into a server nobody can attach it to. `src/admin.html`,
  `include_str!`d and served whole: status, the `/system/logs` ring, the host
  meters, workspace CRUD, that workspace's API tokens (mint / hold / unhold /
  revoke) and projects. Vanilla `fetch` against the same REST surface the app
  uses, from the same origin — nothing to bundle, nothing to version against
  the server, and `AGENT_PLATFORM_CORS_ORIGINS` stays unset.
  - **Served unauthenticated, like `/openapi.json`**: `auth::require_token`
    guards `/api/v1/*` and the document holds no secret. The master key is typed
    in, kept in `sessionStorage` (gone with the tab), and sent as a bearer token
    on every call the page then makes.
  - **Driven, not only compiled.** Against a sandboxed daemon on `:18499`: the
    status and resource tables rendered from the real bodies (which is how the
    first draft's guessed `machine`/`memory_used_bytes` keys were caught — they
    are `host`/`mem_used_bytes`), the log ring rendered flat from its JSON
    records, and a workspace, a project and two tokens were created through the
    page, one of them held.
- **A cross-module env race in the test suite — fixed 2026-08-23.**
  `llm_llama_process`'s launch-flags test sets `LOCAL_MODEL_PATH` to a real
  file; while it holds that, `llm_config`'s capability routing sees `local` as a
  configured provider and resolves chat to it instead of `ollama`. Each module
  had a private lock, which is no lock at all across them. There is one
  `crate::ENV_LOCK` now, and the stale `SAFETY: single-threaded test process`
  comment is gone. It surfaced as an unrelated edit turning the suite red — the
  worst way for a test to fail.
- **Settings → Performance shows the machine, not just the knob — landed 2026-08-22.**
  The page had one meter (background model calls against the lane's limit) and
  said nothing about the desktop it was bounding. It now opens with four dials —
  CPU, memory, GPU, disk — a bar per logical core, and bar rows for swap, VRAM
  and `agent-platformd`'s own slice.
  - **`resources.rs` grew a sampler.** `sysinfo` (system + disk features) for
    CPU/memory/swap/disk/process, `nvml-wrapper` for NVIDIA GPUs. Both are
    sampled inside `spawn_blocking` on the `GET`/`PUT` the desktop was already
    making — no timer, no thread, same reason the sidebar monitor never had one.
    The `System` is a process-wide `OnceLock` because CPU percent is a diff
    against the previous refresh, primed once with a 200 ms sleep so the first
    reading is not a misleading 0%.
  - **GPU is NVIDIA-only and that is the decision.** `nvml-wrapper` `dlopen`s
    the driver's own library, so nothing in the build depends on a GPU and a
    machine without one reports an empty list. ROCm SMI and Level Zero are two
    more SDKs for one meter, neither testable here.
  - **Disk is the volume the workspaces are on**, picked by longest matching
    mount point, not a sum across every mount — a total across disks is a number
    no path is on.
  - **New kit widgets**: `ui::dial` (canvas, 270° arc, the only canvas here),
    `ui::gauge`/`gauge_row` (proportional bar, where `ui::meter`'s cells are for
    counts), `ui::core_bars`. `domain::format_size` gained a TB arm for volumes.
  - **Agents read it through `api_get`** — the path is listed in that tool's
    description in `assistant_tools.rs`; no new tool, the REST surface was
    already the tool.
- **Provider `local` is a managed `llama-server` — landed 2026-08-22.**
  [ADR 0012](docs/adr/0012-managed-llama-server.md). The question behind it was
  "shouldn't the LLM server run internally like the dedicated server, with its
  own logs — same as the image/video one?", and the answer was that the image
  one already did and the LLM one did not. `local` used to mean llama.cpp linked
  in behind `--features local-llm`, off by default, with no log surface and no
  lifecycle; in practice a local model meant an Ollama the user installed.
  - **`managed_server.rs` is the shared mechanism.** Pinned GitHub release,
    `.part` download, unpack, walk for the executable, spawn with stderr drained
    into the `logd!` ring, health-wait that watches the child, bounded stderr
    tail that becomes the error, loopback-only management, Windows job object.
    `media_sdcpp_process.rs` moved onto it and lost 440 lines; policy stayed
    behind (sd-server restarts on a modality change, llama-server on a model
    change).
  - **`llm_llama_process.rs` is the llama policy.** Pin `b10549`, Vulkan asset
    (35 MB), `LOCAL_MODEL_PATH`/`LOCAL_N_CTX` → `-m -c -ngl 999 --jinja -a`,
    `LOCAL_API_BASE` default `http://127.0.0.1:18412`, idle stop at
    `LOCAL_LLM_IDLE_SECS` (600).
  - **`ChatDest` is gone.** `local` resolves to an upstream URL like Ollama
    does, so streaming, retries, usage normalisation and the capability guard
    are the same code. `llm_local.rs` and the server's `local-llm`/`cuda`
    features are deleted — the daemon compiles no C++ now.
  - **The Settings card configures every build.** GGUF picker, Hugging Face
    downloader and context box are unconditional (they set what the daemon
    loads); only the VRAM and last-turn rows stay behind the app's `local-llm`
    feature. ADR 0006's in-process engine is untouched for the app's own chat.
  - **Driven end to end**, not only unit-tested. The ADR's *driven run* section
    has the detail: the daemon fetched b10549 on the first `local` turn and
    answered with that tag in `system_fingerprint`; nineteen `[llama-server]`
    lines landed in `GET /system/logs` beside `[sd-server]`'s; the query was
    sent from E.V. in the app, not curl; killing the daemon reaped both
    children; and one Studio image came out of ComfyUI and one out of an
    `sd-server` this daemon fetched and drove to `ready`.
  - **Known, and not caused by this change:** a local turn on a card someone
    else is filling runs at CPU speed with nothing in the ring to say why
    (llama.cpp fits params to free VRAM silently; `-lv 4` shows it and costs
    ~200 lines a load), and a turn slower than 300 s trips the proxy's upstream
    read timeout — the same ceiling a slow Ollama has always had.

- **stable-diffusion.cpp as a second media backend — seam landed 2026-08-21.**
  [ADR 0011](docs/adr/0011-stable-diffusion-cpp-media-backend.md), which amends
  ADR 0009. `MEDIA_BACKEND` picks between ComfyUI (default, unchanged) and
  **`sd-server`**, the HTTP server that ships in stable-diffusion.cpp's release
  zips. New `media_sdcpp.rs`; `media.rs` keeps every route, the row, the waiter
  and the file writer.

  **Two facts in ADR 0009 were wrong, and both were load-bearing.** ComfyUI is
  **GPL-3.0**, not Apache-2.0 — which does not affect loopback HTTP but does
  affect ever shipping it. And "sd.cpp does images but not video" was a year out
  of date: Wan 2.1/2.2 landed September 2025, LTX-2.3 in May 2026, MiniMax-H3
  this month. Both are marked inline in ADR 0009 rather than quietly edited.

  The comparison that flipped it: **39 MB** (Vulkan build) or 336 MB (CUDA)
  of MIT-licensed native binary, versus ≈3.5 GB of GPL-3.0 Python and torch,
  for the same two modalities. Weights are gigabytes either way; what changes is
  whether a whole interpreter rides along, and whether we would be the ones
  distributing it.

  - **The seam is three functions, not a trait.** An adapter answers `Poll::{
    Pending, Done{bytes, file_name}, Failed}`. `Done` carries **bytes** because
    that is exactly where the backends differ — ComfyUI names a file to fetch
    from `/view`, sd-server returns base64 in the poll body — so there is one
    function that writes a finished file, not two. `watch_job` lost its ComfyUI
    specifics in the process and got shorter.
  - **Sampling parameters are deliberately not sent.** sd-server applies the
    defaults for whatever model it loaded. A distilled model wants `txt_cfg` 1.0
    and ~8 steps, a full one 3.5 and ~28; pinning either would silently wreck
    the other class. Asserted by the integration test, which checks
    `sample_params` is *absent*.
  - **`POST /v1/images/generations` stops answering 501 — with no adapter.**
    sd-server serves that exact OpenAI-shaped route at the same base, so
    `image_api_base()` falls back to the media base when the backend is `sdcpp`
    and the existing capability registry in `llm.rs` does the rest. ComfyUI gets
    no such fallback; a node graph would 404 an OpenAI client. That is the
    "serve our own open-source image API" half of the ask, in five lines.
  - **`GET /status` grew `backend` and `modes`.** sd-server binds one model at
    startup and reports which modes it supports, so "this install cannot do
    video right now" is answerable before a job is submitted rather than minutes
    into one. `MediaStatus::supports` reads an empty list as "yes" so an older
    server is not mistaken for a broken one.
  - **Tests:** six unit tests in `media_sdcpp.rs` (b64 decode, the empty-result
    case that would otherwise hang until the hour deadline, error extraction)
    plus `tests/media_sdcpp_routes.rs`, the whole lifecycle against a stub
    sd-server. Its **own** test binary, not a second test in `media_routes.rs`:
    both drive the module through the `MEDIA_*` process environment and
    `MEDIA_BACKEND` is precisely what they would fight over.

  ### … next steps

  1. ~~Lifecycle~~ — **landed 2026-08-21**, see the entry below.
  2. ~~Model acquisition~~ — **landed 2026-08-21**, see below.
  3. **A/B the video quality on the 5080, then flip the default.** There are
     open complaints upstream about Wan output in sd.cpp. `--rng cpu` exists to
     match ComfyUI's RNG, which is what makes the comparison meaningful. The
     default does not move on a spec sheet.
  4. ~~Desktop~~ — **landed 2026-08-21**. The Studio backend card speaks sd.cpp:
     each `backend_stage` renders as a sentence (no model, setting up, loading,
     idle, failed, external) with the backend's own words underneath on a
     failure. Before this an sdcpp user saw *"ComfyUI is not running"* and a
     **Get ComfyUI** button — pointing at the wrong app while the server was
     busy fetching the right one. A second card covers the selected kind the
     loaded model cannot serve. **Both are sentences, not greyed-out controls**:
     a dead toggle with no reason is worse than a live one with an explanation,
     and generating swaps the model rather than failing.

  ### sd-server lifecycle — landed 2026-08-21

  `media_sdcpp_process.rs`: fetch the pinned release, unpack, spawn,
  health-wait, reap, stop when idle. The user installs nothing. `GET
  /media/status` grew `backend_stage` (`external` | `unconfigured` |
  `not_installed` | `downloading` | `extracting` | `starting` | `ready` |
  `stopped` | `failed`) and a one-sentence `backend_detail`, so the screen can
  distinguish "downloading" from "not installed" — both are `reachable: false`.

  **Two things were wrong until they were measured against the real binary.**

  - **`tar` is a PATH gamble on Windows.** bsdtar in `System32` reads zip; GNU
    tar (git-bash, MSYS) does not — verified on this exact release zip, where
    GNU tar 1.35 answers *"This does not look like a tar archive"*. Now named by
    absolute path, `unzip` elsewhere.
  - **A bad model path kills sd-server in under a second.** Polling the port
    alone would have burned the full 300 s start timeout on a typo. `health_wait`
    watches the child, and quotes its `[ERROR]` lines — preferred over the raw
    tail, which is six lines of Vulkan banner that would put a graphics card in
    front of a file error. Measured end to end: **2.5 s**, with sd-server's own
    reason in the 502.

  Also confirmed on the real 38.8 MB Vulkan zip: it unpacks **flat**
  (`sd-server.exe` plus thirteen DLLs, `ggml-vulkan.dll` alone 50 MB), which is
  why the child runs with `current_dir` set to the install directory; and
  `--listen-ip` / `--listen-port` are real flags with 1234 as the default port.

  **Model flags are `MEDIA_SDCPP_ARGS`, not a table in the crate** — families
  need different ones (`-m` vs `--diffusion-model` + `--vae` + `--llm`) and
  upstream adds families weekly, so a table here would be our own treadmill.
  Unset is a named state with an actionable error, never a silent 39 MB download
  that arrives at the same error.

  Tests: 13 unit, plus `tests/media_sdcpp_spawn.rs` — ignored, gated on
  `AGENT_PLATFORM_TEST_SDSERVER=<path>`, the same shape as the local-inference
  check. It pins the fail-fast path against a **real** sd-server; the success
  path needs multi-gigabyte weights no test should download.

  ### Model catalogue — landed 2026-08-21

  `GET /api/v1/media/models` lists what is worth having, what it costs and
  what is on disk; `POST /api/v1/media/models/{id}/install` fetches the missing
  files. Two entries, one per modality: **Z-Image Turbo** (6.7 GB across three
  files) and **Wan 2.2 TI2V 5B** (18 GB, the same files ComfyUI's template uses).

  **The table fills the launch args; it is not a second way to launch.** That
  distinction is why `media_sdcpp_process.rs` still knows nothing about model
  families: a stale entry costs a catalogue row, not the feature, because
  `MEDIA_SDCPP_ARGS` still overrides everything. And **the files on disk are the
  state** — no setting to persist, no `.env` write. Install the weights and the
  next generate launches with them.

  - **Every URL is ungated, checked rather than assumed.** sd.cpp's own Z-Image
    doc links FLUX.1-schnell for the VAE; that repo answers **401** without a
    token, so the Comfy-Org repackage of the same file is used instead. A
    catalogue entry that walks the user into an auth wall is not an entry.
  - **Sizes are real `content-length` values** from the HuggingFace API, so the
    confirm step can say what it is about to spend.
  - **Installed means size-matched, not present.** A half-written file from a
    killed download would otherwise read as installed and fail at model load.
  - **A kind change is a restart.** One model per process, so an image server
    cannot answer `vid_gen`; `ensure_running` compares the wanted args against
    what the running child was launched with.

  **Two bugs the tests caught, both mine.** The first: asserting catalogue extra
  args come in flag/value pairs — wrong, because sd-server mixes value flags
  (`--cfg-scale 1.0`) with boolean ones (`--diffusion-fa`, `--vae-tiling`), so
  the assertion failed on correct data. The second was worse and is the reason
  the route test exists: requiring launch arguments *before* probing turned an
  already-running, perfectly working backend into a 502, because nothing had
  told us which model to start when nothing needed starting. **A reachable
  server needs no launch args at all.**

  **Not done, and deliberately:** weights are still the user's job on both
  backends, there is no curated per-family table filling `MEDIA_SDCPP_ARGS`
  yet, and ComfyUI is **kept** rather than replaced — it carries an ecosystem
  sd.cpp does not, and new architectures reach it first.

- **Resource modes and AI-call priority — landed 2026-08-19.**
  [ADR 0010](docs/adr/0010-resource-modes-and-ai-call-priority.md).

  **The pitfall was one line.** `executor::max_concurrent_tasks()` returned
  `None` unless an operator set `AGENT_PLATFORM_DAG_MAX_CONCURRENT_TASKS`, and
  `None` means unbounded — while the planner prompt asks the model for "many
  small parallel subagents". A forty-node ready wave was forty simultaneous
  model calls, two concurrent processes were eighty, and a 429 turned each of
  them into six retries. The interactive chat the user was actually watching
  queued behind all of it.

  What landed: `resources.rs` holds a process-wide `Limits` with **two
  semaphore lanes** — interactive (a ceiling against pathology) and background
  (the throttle). `Mode` is `eco | balanced | turbo | auto`; `auto` resolves per
  acquire to Turbo while the desktop window is in front, Balanced for 60 s after
  the last interactive call, Eco once nobody is looking. `max_concurrent_tasks`
  now falls back to the resolved background width, so the wave stops being
  *created* too wide. The gate sits on **`llm::complete_internal`**, which took a
  required `Priority` parameter — eleven internal callers, and the compiler is
  what stops a twelfth from silently joining the wrong lane.

  Desktop: **Settings → Performance** (the picker plus what `Auto` currently
  resolves to) and a **sidebar monitor** above the utility strip. The monitor
  owns no timer and no sampler — it rides the health poll that was already
  running, at a rate that follows the mode (20 s in Eco, 5 s in Turbo,
  `resource_poll_every`), and every number it draws is an atomic read on the
  server. Host CPU/memory is deliberately absent: it needs a per-platform
  dependency and a polling thread, for a number the user cannot act on from
  there.

  Three smaller fixes in the same pass: `coder_tools::search`/`repo_map` moved
  to `spawn_blocking` in **both** crates (a synchronous whole-workspace walk on a
  tokio worker — and on the app side that runtime also draws the UI), the
  model-catalog refresh backs off 30 s → 5 min once both
  local backends come back empty, and `StatusTick` drops to 30 s with no window
  open.

  **Not done, and why:** the SQLite pool keeps sqlx's defaults — 10 connections
  contending was a symptom of the 40-wide wave, not a cause. The tokio runtime
  still starts a worker per core in every mode; it cannot be resized after
  `Runtime::new()` and idle workers park rather than spin.

- **Local image and video generation — landed 2026-08-19.**
  [ADR 0009](docs/adr/0009-local-media-generation.md). A **Studio** screen
  (`studio.rs`/`studio_view.rs`) over a new server domain (`media.rs`,
  migration `0003_media_jobs.sql`), generating on this machine through
  **ComfyUI** over loopback.

  **Why ComfyUI and not Ollama, checked rather than assumed.** Ollama grew
  image generation in January 2026 and `x/flux2-klein` was *already pulled on
  this machine* — but it is **macOS only**. Driven here: `/api/generate`
  answers `"image generation models are not currently supported"` and there is
  no `/v1/images/generations` route at all. LM Studio does not generate.
  ComfyUI's HTTP API (`/prompt`, `/history/{id}`, `/view`) is the whole
  integration. (This entry originally said "sd.cpp does images but not video.
  ComfyUI is the only backend that does both on Windows today" — **wrong on
  both counts**; see ADR 0011 and the entry above.) When Ollama's Windows support lands it slots in behind
  the **already existing** `/v1/images/generations` capability registry in
  `llm.rs` — which is why that route was left alone rather than extended: it is
  synchronous, image-only and base64-in-JSON, and forcing video jobs through it
  would make it OpenAI-shaped in name only.

  - **A generation is a job, not a request.** Diffusion is seconds to minutes,
    so `POST /media/generate` answers once ComfyUI *accepts* the graph and a
    background task polls `/history`. The desktop ticks only while something is
    running (`studio::State::polling` gates the subscription) — a settled
    gallery costs nothing. `media::spawn_startup_recovery` fails orphaned rows
    at boot for the same reason `executor`'s does: the watcher is a task in
    this process, so a restart would leave a spinner nothing will ever stop.
  - **Templates are data.** Two checked-in ComfyUI graphs with `__AGP_*__`
    placeholders (`media_templates/`), and a user file at
    `<media dir>/templates/*.json` overrides either — the escape hatch for a
    node rename or a different model family, without waiting for this crate.
    The image template resolves its checkpoint against `/object_info` rather
    than hard-coding one, preferring known text-to-image families.
    Substitution is textual and quote-aware (`"__AGP_WIDTH__"` → a bare
    number), so a prompt carrying quotes, backslashes or newlines is escaped
    through `serde_json` rather than concatenated — pinned by a test.
  - **The server's first raw-binary route.** `GET /media/jobs/{id}/file`;
    `workspace_files::get_file` is a *text* extractor and could not serve a
    PNG. The desktop caches decoded bytes per finished job — a view runs every
    frame, so re-fetching a picture sixty times a second is a denial of service
    against your own server.
  - **Video does not play in-app, and that ceiling is written where it is
    cut.** iced has no decoder, so a finished clip gets a card with *Play*,
    which writes the bytes to temp and hands them to the default player.
  - **Unconfigured is a first-class state, again.** No ComfyUI means an
    informational card naming the port it looked at and a *Get ComfyUI* button
    — never a red banner, never in place of the composer. Confirmed in the
    running app: the card renders on a machine with no ComfyUI, and **removes
    itself** on Refresh once a backend answers.
  - **E.V. needed no new tool.** `POST /api/v1/media/generate` is a write, so
    `api_write` already parks it behind the one confirm card; reads come free
    through `api_get`. Both are named in the tool spec, along with `studio` in
    `open_screen`'s list — a capability the model is not told about is one it
    never reaches for.

  **One real bug, found by driving it and not by the tests.** The `INSERT`
  named eleven columns and bound ten values: `kind` was never bound, so every
  column shifted by one and the statement died on `NOT NULL prompt`. Nothing
  pure could have caught it — the module's seven unit tests all passed — so the
  regression check is `tests/media_routes.rs`, which runs the whole lifecycle
  (probe → generate → poll → bytes) against a stub ComfyUI built from `axum`
  inside the test binary. It also asserts the *submitted graph*, so a template
  that stops carrying the prompt fails there rather than in a picture nobody
  looks at.

  **Driven end to end** against a stub speaking ComfyUI's API: status probe,
  job created (`width: 1000` snapping to 992), the running → completed
  transition, the PNG copied into the media dir and served back byte-identical
  as `image/png`, plus the video kind and both 400s. **Not** driven: pressing
  *Generate* in the app itself — the window kept moving between screenshot and
  click and the clicker landed on the wrong control twice, so the desktop half
  is proven by its unit tests and by the screen rendering, not by a click.
  604 tests green, hygiene and `check_openapi_request_drift.py` clean.

  **Style presets — landed 2026-08-19.** A `Style` chip row in the composer
  (`studio::PRESETS`): Pixel art, Logo mark, Icon, Sticker, Isometric, Photo
  for images, Cinematic and Seamless loop for video. A preset is client-side
  only — no route, no template, no column. Picking one fills the *Avoid* box
  and the size chip and appends its keyword string to whatever the user typed
  at generate time, with the exact appended words shown under the chips. Only
  the current kind's presets are offered, and a kind change drops the
  selection because the size index belongs to the other table.

  Open, and deliberately: no cancel button (ComfyUI's `/interrupt` exists —
  nothing calls it), no image-to-image or upscale, no inline results in the
  E.V. transcript (chat has no attachment plumbing at all — Studio is the
  surface, and inline is the obvious second slice), and the video template
  hard-codes the Wan 2.2 5B file names, so a user without those files gets
  ComfyUI's own error rather than a check up front.

- **Start at login — landed 2026-08-15.** Settings → Status grows a **Startup**
  card: one toggle writes `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
  with `"<exe>" --minimized`, a second exposes `start_minimized`, which had been
  a settings field with no UI since the Tauri port.
  - **Not a Windows service, deliberately.** This process is the tray icon and
    the server host at once; a service runs in a session with no desktop, so it
    could not show the tray or open the window, and it would want admin rights
    and an installer step to register. The `Run` key is what every always-on
    desktop app uses, and the user can see and remove it from Task Manager's
    Startup tab. If a headless always-on daemon is ever wanted, that is
    `agent-platformd` as its own service — a different feature, and it needs the
    install's `master.key` and data dir passed to it.
  - **The registry is the state, not `settings.json`.** That Startup tab can
    disable the entry behind the app's back, so a second copy of the fact in our
    own file would be a lie; `App::autostart` is a cache of the registry read at
    boot, not a persisted setting. `reg.exe` rather than the registry API, same
    reason `is_our_process` shells out to `tasklist` — no unsafe, no extra
    `windows-sys` feature, twice a session. A failed write re-reads rather than
    flipping the toggle to what was asked for.
  - `--minimized` was already parsed in `boot`; nothing new was needed to make
    the login launch land in the tray.

- **Web search — landed 2026-08-15.**
  [ADR 0008](docs/adr/0008-web-search.md), plan in
  [`docs/web-search-module-plan.md`](docs/web-search-module-plan.md). Natural
  language in, a Google dork out, opened in the user's own browser.

  **The shape, and why it has no provider.** There is no free programmatic web
  search — Google CSE is 100/day behind a Cloud project, Brave's free tier wants
  a card, and scraping `google.com/search` is CAPTCHA'd, against the terms and
  silently fragile. So the server **builds the query and hands it to the
  browser** (`shell::open_url`, already there for Providers' "Get API key"). It
  makes **no outbound HTTP at all**; the only thing it calls is its own
  in-process `/v1`. Zero cost, zero key, zero quota, nothing to maintain.
  - **The line that matters is finding vs comparing.** Finding a document, a
    discussion, a paper, a page on a site is all operator work and is delivered.
    Comparing what came back — "which of these is cheapest", "summarise these
    five" — needs the results in hand and is **not**. The price-comparison
    example that opened the ask lands on the wrong side of that line; the ADR
    records the upgrade path so the deferral is a pause, not a blank.
  - **`DorkQuery` is a struct, not a string the model writes** (`search_dork.rs`).
    The model emits the *fields*; Rust renders the operators. That is what makes
    `site:my site.com` impossible to ship — it has to come through `add_site` or
    `validate`, both of which reject whitespace. `parse` is the inverse, so raw
    dork typed into the box comes back as the same editable chips.
  - **`from_phrases` is a table of named intent recipes**, not ad-hoc patterns —
    `document`, `discussion`, `academic`, `docs`, `shopping`, plus `onsite`,
    `recent`, `exclusion`. Recipes are **additive on the struct**, so composition
    falls out instead of being special-cased: "cheap pdf manual on arxiv.org"
    fires three at once. Each carries a plain sentence that leads the
    explanation ahead of the per-operator lines, because the intent is the
    knowledge the user is missing. `search.rs` reaches for the model **only when
    no recipe fired**, and any model failure — no master key, bad JSON, timeout —
    degrades to the rule output rather than 500ing.
  - **No exposure-hunting recipes.** `intitle:"index of"`, `filetype:env` and
    the exposed-config families are absent from the table. Typing one into `q=`
    still builds it faithfully — that is the caller's own query — but a curated
    table of them makes this a recon tool rather than a search helper. Written
    down because the table is exactly where someone adds them without noticing.
  - **E.V. needs no new read tool.** The route is a `GET` under `/api/v1/`, and
    `assistant_tools::api_get` is already a prefix-guarded GET-only reader for
    any such route. A `POST` would have cost a new tool or put a confirm card in
    front of a *read*, which is the pattern that trains people to click through
    the cards that matter.

  **Review found four, all in the new code, two of them real:**
  - **A bare title marker forced `filetype:pdf`.** "find the article called Foo"
    rendered `filetype:pdf intitle:Foo` — a restriction nobody asked for, quietly
    discarding every non-PDF result. A title marker means match the title; it
    does not mean the thing is a document. `apply_academic` had the same
    assumption for "citation" and "research", now narrowed to paper/study/journal.
  - **The shopping sites were `.com`-only** — `site:amazon.com` does not match
    amazon.co.uk, so the recipe searched the wrong storefronts for a UK user. The
    tell that it was an oversight rather than a decision: the price-range regex
    in the same file already accepts `£`. Both TLDs per retailer now, walmart
    dropped rather than shipped US-only again, `ponytail:` on the ceiling.
  - `used_rules()` was dead — the design moved to the fired-recipe list and only
    its own test still called it.
  - `explanation` rows put a *recipe name* in a field called `operator`. Same bug
    the transcript labels had (see the E.V. console entry below: every tool row
    prefixed `$` after a second tool existed). A `kind` discriminator now tells
    a recipe row from an operator row, changed while no client had been built
    against the contract yet.

  **The screen** is `Screen::Search` (`search.rs`/`search_view.rs`): a sentence
  box, the rendered dork under it **in mono and editable**, removable chips for
  the parts, the explanation, an engine picker and *Open in …*. A caption says
  which mode the next run uses — "translates the sentence above (`ask=`)" or
  "uses this query verbatim (`q=`)" — because editing the dork box silently
  changing what gets sent is the one ambiguity that screen has.

  **E.V. needed one tool after all, and only one.** Reads come free through
  `api_get`, but *opening a browser leaves the app*, so `web_search` parks as
  `Pending::Search` and the card asks "Open this search in your browser?" over
  `Search Google for: <the dork>`. It names the engine rather than showing a
  URL, because the URL does not exist until approval — it is built server-side —
  and a percent-encoded guess would be less honest than what is actually known.
  `Pending::Search` carries a `verbatim` flag: the card is built *before* the
  server is called, so without remembering whether the model sent `q` or `ask`,
  approval could not know which parameter to use, and a sentence sent through
  `q` would silently skip translation.

  **Three rounds of review, and the second one was self-inflicted.** Beyond the
  four findings above:
  - **The client grew its own copy of `render()`** — 44 byte-identical lines of
    operator grammar in a crate that cannot see the server, "pinned" by a test
    asserting a hardcoded string, which would have kept passing through any
    drift. It existed only because removing a chip needs a query string to send.
  - The first fix — a `drop=<token>` parameter, removal logic beside `render`
    on the server — was right but under-specified: it never said *where the
    chips get their tokens*, so the gap got filled with a hand-maintained index
    into `explain()`'s output order, bumped for the collapsed group rows. That
    is a parallel walk of another crate's function in a second crate, correct
    on the day and silently wrong the moment `explain()` reorders — chips would
    hand back the wrong token and removing one would delete another. **Worse
    than the copy it replaced**, because a copy is at least legible.
  - The answer was **the server emitting the chips** (`DorkQuery::chips()`,
    beside `render`/`explain`/`drop_part`): `{token, label, field}`, one per
    removable element, token produced by the same code that renders it. Tone
    stays client-side; nothing else does. `part_chips` is now a twelve-line map
    and **no dork operator is constructed anywhere outside `search_dork.rs`** —
    checked by grep, not by assertion. The invariant the cursor could never
    have: every chip's token must be droppable, dropping each in turn removes
    exactly one element, and the query ends empty but for `terms`.

  Left alone deliberately: `render`/`explain`/`drop_part`/`chips` each spell the
  operators independently *within* `search_dork.rs`. One file, adjacent
  functions, a mismatch visible immediately, and the pair that must agree is
  pinned by the invariant test. Three refactor rounds on one file is enough.

  **Then four more rounds, from one question: "shouldn't the dork have options
  such as search by type, search by website containing?"** It should, and the
  question exposed two different holes.

  - **Operator coverage.** `intext:` (page body contains) was simply absent next
    to `intitle`/`inurl`. `related:` was missing. The numeric range was
    *half-built* — `apply_shopping` appended `"100..200"` into the leftover text
    so it landed in `terms` as a bare string, which meant it was not a field, not
    a chip, not removable and not recoverable by `parse`. It is `range` now.
    `ext:` is accepted on parse as a one-way alias into `filetype`, because a
    dork pasted from elsewhere was silently losing the operator into `terms`.
    Deliberately still absent, with the reason written where someone would reach
    for them: `cache:` and `link:` (**Google removed both** — offering them
    builds queries that cannot work), the `allin*:` family (repeating the
    singular covers it, and a second spelling is a second thing to keep in step
    across all six walks), and `*` (already works, `terms` is verbatim).
  - **The chips were remove-only, which undercut the premise.** There was
    `RemovePart` and no add. So the only ways to *add* an operator were to phrase
    the sentence so a recipe fired, or type the operator — which needs the syntax
    the module exists to remove. `add_field=`+`add_value=` mirrors `drop=`, the
    server builds the operator in both directions, and the picker's labels carry
    both halves ("Page text contains (`intext:`)") so it can be found without the
    syntax and left knowing it. A failed **add** is a 400 naming the problem
    where a failed **drop** is a silent no-op — a drop that matches nothing
    changes nothing visible, but an add that silently does nothing reads as a
    broken button.

  **History is real now** — see the deferral note in the plan doc, which this
  closed. Workspace-scoped because storing changes the tenancy question,
  `opened` as INTEGER because a `bool` on a `FromRow` struct is a latent 500 on
  the `Any` pool, and recording is an explicit POST rather than a side effect of
  `GET /search/dork` — that route is hit by every chip edit, and auto-recording
  would fill history with fragments of a query nobody ran.

  **Results landed, behind a key, and the deferral that blocked them was wrong
  on a fact.** ADR 0008 said there is no free programmatic search; Google CSE is
  100/day free **with no billing account** — the reasoning went from "needs a
  Cloud project" to "needs money", which are different things, and Brave was the
  one that wanted a card. The ADR carries an amendment saying so rather than a
  rewrite, because the original reasoning still governs the no-key path, which is
  every install until someone configures one.
  - **Unconfigured is the default and stays the whole product.** `configured:
    false` with *every other field still populated*, so the screen renders
    exactly what it renders today. Never a 503, never a bare empty list — an
    empty list reads as "nothing matched" and sends the user hunting for a better
    query when the answer is "this install does not do results". A key that
    Google **rejects** is a different problem and surfaces as a real error.
  - `/search` is `/dork` **plus three keys** — same `resolve_dork_query`, same
    `dork_body` — so the results route cannot drift from the builder route.
  - Still deferred, line unmoved: fetching result pages for JSON-LD price
    extraction. CSE returns titles, snippets and URLs; turning those into a price
    table is the SSRF surface and the per-site ceiling. Results are *comparing
    what a search returned*; extraction is *reading what those pages say*.

  **One cross-cutting bug, found because search is the first feature that
  depends on a named 400 reaching the user.** `client.rs`'s `detail_message`
  only read `{"detail": …}` — the **deleted Python server's** error shape.
  `agent-platformd`'s `ApiError` answers `{"error": {"message": …}}`, and
  `sse.rs` already had a reader for that same shape while the plain REST path
  never got one. So **every named error from the Rust server, on every screen,
  had been collapsing to the generic "Request failed"** since the migration.
  Nothing depended on the wording before, which is why it survived. Fixed with
  both shapes read and a test.

  **The stale-banner sweep was finished, and it had been wrong about being
  done.** The 2026-08-07 entry below records this bug found and fixed in seven
  files. Two more turned up in `providers.rs` this week — *a file that sweep had
  already marked fixed* — which said the method had been applied per file and
  not per arm. A full audit of all fifteen files with an `error` field found
  **16 more arms across 7 files**:
  - **`agenda_chat.rs` had all five of its success arms broken.** Its only clear
    site was `DismissError`, so every failure that screen could produce stuck
    until the user dismissed it by hand. That file was not under-swept, it was
    skipped.
  - **`assistant.rs` and `chat.rs` never cleared on `Send`.** `coder.rs` clears
    at the start of every new turn and they did not, so one failed turn banner
    outlived every successful turn after it. `assistant.rs` also stuck on a
    failed transcription (STT runs continuously in voice mode, so *one* bad
    utterance banner survived every good one) and on audio playback.
  - `processes.rs::TeamsLoaded(Ok)`, `workflows.rs::{RunsLoaded,RanNow}(Ok)`,
    `modelops.rs::{JobStarted,JobUpdated}(Ok)`, and four in `coder.rs`
    (terminal open, diff load, checkpoint restore, browser nav) — all the same
    shape: the retry after a transient failure never cleared what the failure
    set.

  **Two shapes were deliberately left alone**, and they are worth knowing
  before the next sweep re-flags them:
  - `coder.rs` and `assistant.rs` each have **one `error` field serving several
    independent subjects** (terminal, checkpoint, thread, turn, tool-post,
    preview; and chat, mic, playback, TTS). Each subject's own success/failure
    pairing is now symmetric, and success in one subject deliberately does *not*
    clear another's banner. Splitting the field is the real fix and is not a
    bug-sweep's job.
  - Synchronous validation errors (empty name, no stage picked) have no paired
    success arm at all — they clear on the next attempt at the same action.

  **Why this keeps happening:** the asymmetry is invisible at the call site.
  Nothing in the type system links "this arm sets a banner" to "that arm must
  clear it", the failure is only reachable through a transient error, and the
  symptom — a banner that is merely *stale* rather than wrong — reads as
  plausible. A sweep that reports only what it changed cannot be audited for
  what it missed, which is how the last one shipped incomplete; this one listed
  its "correct as-is" verdicts too.

  580 tests green across the three crates, hygiene clean.

- **Reuse and hot-path sweep — landed 2026-08-09.** A read of the whole
  tree for duplication, scattered tables and per-request work. Nothing here is a
  feature; it is the drift a year of screens leaves behind. Three passes, in
  this order, each one green on `cargo test` before the next starts.

  **Pass 1 — the copies, and two bugs found while counting them.**
  - `model_ops.rs`'s job runner had the crate's one `sqlx::query*` with a `?`
    placeholder that skipped `crate::db::sql()`. Harmless on SQLite, which takes
    `?` verbatim; the moment the `sqlx::Any` pool runs against Postgres it is a
    syntax error in the middle of a build job. Wrapped.
  - `dotenv::YAML_SECRET_KEYS` listed three keys against
    `llm_admin::SENSITIVE_ENV_KEYS`'s five, so `AIMLAPI_API_KEY` and
    `ANTHROPIC_API_KEY` were masked in `GET /env` while still being *accepted*
    from the committed `config/agent_platform.yaml`. Two lists that must agree
    and no test saying so; `dotenv.rs` reads the one list now.
  - `ui::error_bar` already existed and only `coder_view` called it. Nine other
    screens hand-rolled the same row — five as a private `fn dismissible` copy,
    four as an inline `cluster`. All nine call the kit fn.
  - `err_string` (×7), `non_empty` (×4), `truncate` (×3): byte-identical private
    copies across screens, now single `pub fn`s in `domain.rs` beside
    `format_size`.
  - `temp_db_path`/`start_server` were copied into three integration tests;
    `server/tests/common/mod.rs` holds them.

  **Pass 2 — one table per fact, and the per-request work.**
  - Provider knowledge lived in five places: `llm_config::PROVIDERS`,
    `llm_admin::{ENV_KEYS,SENSITIVE_ENV_KEYS}`, `dotenv::YAML_SECRET_KEYS`,
    literals in `llm.rs`, and the app's `providers::PROVIDER_META`.
    `ProviderSpec` carries `api_key_env`/`base_url_env` now and
    `llm_admin::the_provider_table_is_the_source_of_the_key_lists` walks it: a
    credential named there must be masked, and — for a chat provider — must have
    a field on the Providers screen. `SPEECH_API_KEY` was neither, and is both
    now. **The lists are pinned to the table, not generated from it.** `ENV_KEYS`
    is also the set `write_env_file` owns and *rewrites*, so deriving it would
    have grown the user's `.env` by whatever the table happened to hold; a test
    that fails loudly is the same guard without the blast radius.
  - `registry_list` ran one `SELECT name FROM model_projects` per registry row.
    One map lookup for the page now — not a join, because `REGISTRY_COLUMNS` is
    shared with two queries that do not want the name and a join would need a
    second row struct. `workspaces::archive` ran a SELECT plus a statement per
    token and two per team, only so the response's counts could be `Vec::len()`;
    `rows_affected` is the same number, so it is three set-based statements and
    `archive_rows` is split out with a test pinning the counts and the blast
    radius (the *other* tenant's token and team survive). Recovery's per-process
    `COUNT(*)` is left alone — it runs once at startup over a handful of rows.

  **Pass 3 — `app/src/assistant.rs` was five subjects in one file.** Identity,
  the tool spec and its executor, TTS, the mic gate's DSP, and only then `State`
  + `update`. The two that never touch `State` left: `assistant_gate.rs` (160
  lines — the SNR/onset/hang constants, `voice_like`, `is_ghost`,
  `keep_utterance`, `over_playback`, `since`) and `assistant_voice.rs` (165 —
  the Edge socket, `take_sentence`, `speech_text`, `synthesize`, the rate
  table). `warm_voice` stayed behind because it returns a `Task<Message>`.
  2572 lines left, no behaviour change. While moving it, `keep_utterance`'s doc
  comment got reattached to `keep_utterance` — it had been stacked on
  `over_playback` where it read as one comment about the wrong function.

  **Pass 4 — the three the sweep first argued itself out of.** Each was called
  not worth doing on cost grounds; each turned out to be worth doing on
  readability or correctness grounds instead, which is the better reason.
  - **`clarifying_form`'s twelve `Regex::new` per call** are twelve `LazyLock`
    statics declared in one block behind a `pattern!` macro. The saved
    microseconds are still irrelevant in front of a model round-trip — the point
    is that the patterns can now be read against each other, and a malformed one
    fails on first use rather than on whichever call first reaches its branch.
  - **`require_master_key` was three functions for two meanings.**
    `Principal::require_master_key(denial)` is the tenant check, next to
    `require_scope` where it belongs; `llm_admin` and `workspaces` keep only
    their wording, as a `NOT_A_TENANT` const each. `coder`'s was never the same
    check — it is "this server has no master key, so the loop cannot call its
    own `/v1`", a 503 the operator fixes — and it is
    `require_master_key_configured` now, so the name stops claiming otherwise.
    That mis-naming is what made the three look foldable in the first place.
  - **The tray's 150 ms `try_recv` poll** blocks on the receiver from
    `spawn_blocking`. One parked thread instead of 6.7 wakeups a second forever,
    including while the window is hidden and the app is only a server host.

  Still deliberately not done: **a shared `start_server` for every integration
  test.** Three tests, three genuinely different setups (hand-seeded Alembic
  schema, none, migrated). `tests/common/mod.rs` holds what actually repeats —
  `MASTER`, `temp_db_path`, the plain router-on-an-ephemeral-port — and no more.

- **Providers screen is per-provider now — landed 2026-08-08.** The catalog rows
  were status text and the `.env` fields were one flat list underneath, so the
  user had to match "AI/ML API key" to the "AIMLAPI" row by hand. Each row now
  carries a tick/cross badge, a running/stopped badge for the two local
  backends, and its own dialog (`providers_view::provider_modal`) with that
  provider's key, base URL and default-model dropdown. The flat "Keys and
  endpoints" card is gone; `providers::PROVIDER_META` is the one table saying
  which env keys, which key-mint URL and which launch command belong to a
  provider.
  - Inline actions for the thing each row is missing: **Launch** for a stopped
    local backend (`shell::spawn_detached` — `ollama serve`, `lms server
    start`), **Get API key** for an unconfigured cloud one (`shell::open_url`).
  - Model ops folded in only where it applies: the Ollama dialog carries the
    whole "Local models" surface — the installed models with their sizes
    (`GET /model-ops/ollama/models`, bounded to 180px and scrolling inside
    itself) and the pull field over `POST /model-ops/ollama/models/pull`. No
    other provider grows this half; nothing else can pull. `format_size` moved
    to `domain.rs` rather than being copied. The pull job is **not** polled here
    (this screen has no tick subscription) — the toast says to Refresh, and
    Model ops keeps the job card that actually watches one.
  - `ANTHROPIC_API_KEY` joined `ENV_KEYS`/`SENSITIVE_ENV_KEYS` in `llm_admin.rs`
    and `EnvUpdate`; the Claude row was in the catalog with no way to configure
    it. A test walks `PROVIDER_META` and asserts every field it renders survives
    into the save body, so a sixth provider cannot be added half-wired.

  Still open: "running" is inferred from an empty model list (that is what
  `build_admin` reports for an unreachable local backend), not a probe, and
  Launch does not wait for the port — the toast says to hit Refresh.

- **Notifications, in the app as well as on the desktop — landed 2026-08-08.**
  `notify.rs` already toasted work that finished off-screen, and a toast is gone
  in ten seconds; there was nothing left to come back to. It now also keeps an
  inbox (global `Mutex<Vec<Note>>`, same reason `WATCHING` is global — notes are
  posted from module `update`s that have no `App`), capped at 100.
  - Two kinds. `away()` is *finished*; `review()` is **waiting on you** — the
    Coder approval pause, and the two process statuses (`approval_required`,
    `task_review_required`) that stop the engine dead. Those two never notified
    before: `became_terminal` only fired on completed/failed/cancelled, so the
    one state that actually needs a human was the one that went unannounced.
    It is `settled()` now, keyed on "stopped moving", not "ended".
  - Counts. Per screen on the sidebar entry (`ui::nav_item_counted`), global on
    a bell (`ui::bell`) sharing the sidebar's Connected/Offline line. Warning
    tone when any of what is counted is waiting on the user, Info when it merely
    finished. The bell went on that line and not beside the app name because at
    208px it pushed "Agent Platform" onto two lines — seen in a screenshot, not
    reasoned about.
  - Seen = visited. `main::update` already rewrote `notify::watching` after
    every message; it now calls `notify::seen(key)` on the same line, so a badge
    cannot outlive the visit it was about. `NOTIFY_KEYS` is that key↔screen map
    in both directions — a note's row navigates to where it came from, and a
    badge cannot point somewhere no note came from.
  - Panel behind the bell, Esc closes it (above Abort, unlike the E.V. panel:
    closing it stops nothing).

  Still open: notes are in memory, so a restart forgets them — fine while the
  work they point at is a live run. And a run only notifies while it is the
  *selected* one: the detail poll follows it off-screen, but `ListTick` only
  runs on the Processes screen, so a second background run is silent until you
  look. A global list poll is the fix if that bites.
- **A name and a voice of your own — landed 2026-08-08.** "E.V." was a
  compile-time constant in three places at once: the display name, the wake
  word's spellings, and the Edge voice. All three are settings now
  (`Settings::assistant_name`, `wake_names`, `voice_name`), applied in `boot`
  and on every edit.
  - The name lives in `assistant::NAME_CELL` (`RwLock<&'static str>`, set by
    `set_identity`, read by `assistant::name()`), not threaded through screens.
    It is leaked on write because iced placeholders and button labels want a
    borrow that outlives the view — a few bytes per rename, against a `String`
    in every view signature. `composer_hint()` and `talk_label()` are the two
    composed phrases that need the same treatment.
  - **`assistant::NAME` stays "E.V." and is now the storage key**, not the
    display name: it is the `source` on every chat and memory already filed, and
    renaming it would orphan them. Only `name()` follows the setting.
  - Wake spellings, because speech-to-text writes a spoken name however it
    likes. `resolve_wake` is the rule: the user's comma list wins; empty falls
    back to the name itself, or — while the name is the default — to the
    built-in `NAMES` homophones, so nothing regresses for an install that never
    touches this. `addressed()` matches one token, so a two-word name is spelled
    as one word.
  - The voice is stored as a short id (`en-US-AriaNeural`). `edge_voice()`
    expands it to the long form Edge wants; everything else — including
    `SPEECH_API_BASE` behind `client.speech(text, voice)`, which is where a
    Piper/Kokoro voice someone trained themselves goes — takes the short one as
    written. A malformed id falls back rather than reaching Edge, which answers
    a bad voice with a socket error in front of every sentence.
  - Not done: the tray item keeps the name it was built with until restart
    (`ponytail:` in `main::update`), and a memory's byline still reads "E.V."
    because that is the stored `source`.
- **E.V. as the app's console — 2026-08-08, partly landed.** What shipped:
  - One control for voice. The header `Text`/`Voice` segment is gone; the
    composer's mic button *is* the mode (`Message::Listen` owns both the
    recorder and `State::voice`). Four states became two, and the dead one —
    voice mode with a shut mic, a HUD reporting on nothing — is unreachable.
  - The wake word, which turned out to already exist. `addressed()` and its
    `NAMES` whitelist have always run on every utterance; `follow_up_open()`
    short-circuited on `armed`, so an open mic answered everything it heard.
    That clause is gone and the `armed` field with it. Voice mode now opens the
    window for `FOLLOW_UP` seconds (press the button, just talk), and after that
    it is "E.V., …" or a reply to a reply. Side effect worth knowing: an open
    mic no longer turns a phone call into a dozen turns.
  - **App tools** — `desktop/crates/app/src/assistant_tools.rs`, shaped like
    `memory`'s toolkit (`TOOLS` / `tools_spec` / runner). Two tools, not twelve:
    `api_get(path)` reads any `/api/v1/` route through the new
    `Client::api_get` (GET-only, prefix-guarded — the path comes from a model,
    so it is a trust boundary), and `open_screen(name)` parks a `Screen` in
    `assistant::State::nav` which `main.rs` drains into `Message::Nav`. A tool
    per route would have been a second, staler copy of the REST surface.
  - **E.V. from anywhere** — `assistant_view::panel` is the transcript, HUD and
    composer split out of `view`; `screen::assistant_overlay` floats that same
    widget tree over any screen. Ctrl+K or the sidebar's sparkle toggles it, Esc
    closes it (below Abort — a turn in flight is the more urgent Esc). Same
    `State`, so it is one thread across both surfaces, not a second chat.

  - **The terminal behind the same card — and this was the bigger hole.**
    `run_command` had been an unrestricted shell on the user's machine with
    nothing between it and a model but a persona line asking it not to be
    destructive. It now parks as `Pending::Command` exactly like a write:
    `Settings::confirm_commands` defaults **on**, and its serde default is
    `default_true` rather than `#[serde(default)]` so a settings file written
    before this existed comes back guarded rather than silently open. No
    allowlist of "safe" commands — `git log --pretty=%x00; rm -rf /` is why
    that classifier is unsound, and an unsound guard is worse than an honest
    card. Turning it off is a real choice in Settings, worded as one.
  - **Writes, behind one card.** `api_write(method, path, body)` never reaches
    the network on its own: `run_sync_tool` *parks* it as a `Pending::Write`, the
    turn stalls (the round's other tool results are held in `State::held`, not
    forwarded — a model shown half a round re-asks for the other half), and
    `assistant_view::approval` shows the method, the path and the body verbatim.
    `Message::Decide` runs it or answers the call with a refusal. One card at a
    time: a second write in the same round is answered "one change at a time",
    because a queue of confirmations is a queue of things nobody reads.
  - **One chat UI.** `ui::transcript` and `ui::composer` are the kit now, and
    Coder and E.V. both compose them — including the scrollbar right-padding
    that had already been got wrong twice. Not one `State` for both: Coder's
    turns are a different shape (approval cards, dock, per-tool output) and
    unifying the models would have dragged the mic and the HUD into Coder to no
    end. The pieces are shared; the screens stay their own. Since revisited and
    the sharing widened rather than the screens merged: `ui::approval` (one
    confirmation card — `run: Option<M>` is the unreadable-command case, which
    must never show a live Run button), `ui::model_pickers` and `ui::error_bar`
    are the kit now, and both screens compose them.
  - **The wake word, without a new dependency.** `State::standby` keeps the mic
    open with the HUD down and replies unspoken, and the existing pipeline —
    energy gate → whisper → `addressed()` — does the spotting. Whisper only ever
    sees speech the gate let through, so there is no rolling transcription and
    no model file to ship. Hearing its name flips `voice` on, so the answer is
    spoken and the HUD shows what it heard. Rules that make it liveable: the
    follow-up window does not apply while waking (one wake would otherwise leave
    the room answered for 45s), unaddressed speech is *dropped* rather than
    parked in the composer, `Settings::wake_word` persists it off by default,
    and the composer carries a MIC LIVE row — this is the one state with an open
    mic and no HUD over it, so it does not get to be invisible.

  Driven in the app, not just tested — which is where the next four came from:
  - **`bool` on the `Any` pool 500s at runtime.** `GET /api/v1/workflows` and
    `GET /api/v1/processes/{id}` both died with *"Any driver does not support the
    SQLite type SqliteTypeInfo(Bool)"* / *"Rust type `bool` is not compatible
    with SQL type `BIGINT`"*. Two separate shapes: `workflows.enabled` and
    `model_registry_entries.is_active` are declared `BOOLEAN`, so they now go
    through `crate::BOOL!` (defined in `db.rs`, `#[macro_export]`ed to the crate
    root — a `CAST(CASE … AS BIGINT)`, because Postgres refuses `CAST(bool AS
    BIGINT)` outright and a bare `THEN 1` there is `int4`, not `i64`
    outright); `requires_review` is `INTEGER` on both backends, so it just needed
    an `i64` field and `wire::sql_flag` to keep the wire's boolean. None of this
    is checked at compile time, so **every `bool` on a `FromRow` struct is a
    latent 500** — now written down in `db.rs`. Grep before adding one.
  - **The panel could not be a layer.** It was `ui::modal` (scrim, blocked the
    sidebar), then a scrim-less `stack` layer — which blocked it just as much,
    because the full-window container that positions a layer swallows the click
    either way. Measured, twice. It is now a real column of the shell row, so
    the page beside it stays live; `ui::toast_layer`'s doc claim that it
    "consumes no clicks" is corrected in place.
  - **A global mic needs a global indicator.** Standby holds the mic open across
    every screen, but its only disclosure was the composer, on one screen. There
    is now a mic button in the sidebar footer whenever it is armed, and clicking
    it turns the wake word off — one click from live to shut, from anywhere.
  - **Transcript labels lied.** Every tool-call row was prefixed `$` and every
    tool result labelled `TERMINAL` — both fine when the terminal was the only
    tool, both false the moment `api_get` existed.

  Reviewed after the fact, which found five more — all in the new code:
  - **A tool round could go out half-answered.** Deciding a confirm card while
    the read task was still running sent two requests for one turn, the first
    answering only some of the assistant turn's `tool_calls`. Replaced the
    ad-hoc "hold if pending" branch with `State::tool_waits`: a round names how
    many batches it is waiting on, every batch accumulates into `held`, and the
    request goes out at zero. A batch from an aborted round is dropped rather
    than re-opening the turn.
  - **`CAST(CASE … AS BIGINT)`**, not a bare `CASE` — see above. Would have
    worked on SQLite and 500'd on Postgres, which is the backend nobody runs
    locally and so the one that would have found it in production.
  - **Ctrl+K on a chat screen primed the panel to appear later.** It flipped
    `assistant_open` while the panel was suppressed, so the next navigation
    somewhere else opened it unbidden. It now just navigates.
  - **The wake-word setting could outlive the mic.** A refused microphone left
    `standby` false and `settings.wake_word` true — the toggle read "Listening
    for E.V." with nothing listening, and retried on every launch. `SetWakeWord`
    now mirrors what actually happened, and boot routes through it.
  - Stale doc in `assistant_tools.rs` claiming writes were absent, and a
    zero-width placeholder that left a gap in the sidebar footer.

  Still open:
  - **Waking with the app closed.** Standby runs off the assistant's own tick,
    so the process has to be alive. A true always-on spotter is still a separate
    dependency and a model file.
  - **The panel is E.V. only.** Ctrl+K cannot summon Coder's thread; it renders
    `assistant::State`. Worth doing once there is a reason to have both.
  - **A local model still invents app data.** Asked to open Workflows, it
    volunteered "you have 28 saved workflows" without calling `api_get`; there
    are three. The persona now forbids stating any count, name or status that did
    not come from a tool result in that conversation, and names that exact
    failure. Prompting is the only lever here — worth re-checking on a bigger
    model before trusting a spoken answer about app state.
- **macOS/Linux packaging** — **half closed 2026-08-08.** The *daemon* now
  ships for four platforms (below); the *iced app* is still Windows-only and
  still deferred until there is access to a mac or a Linux box — it links
  whisper.cpp through bindgen and a wgpu surface, neither tried off Windows.
  Compile check first, as before.
- **UX polish pass** — drive the app, audit each screen against the `ui/` kit;
  newer screens landed fast. Rolling, never "done". Swept 2026-08-06 while
  building the planning chat — Agenda, Plans, Providers, Logs, Settings → API:
  - **Plans' columns were cut off at the bottom.** The horizontal scrollable had
    no `spacing`, so iced 0.14's floating scrollbar sat on the last row of every
    column and an empty one rendered as a half-cut `—`. Same one-line remedy as
    `ui::page`'s scrollable; it is in `desktop/CLAUDE.md` and was missed here.
  - **Raw model JSON was reaching the user as prose** — the Agenda review banner
    read <code>```json {"reasoning": …</code>, truncated mid-sentence, and an
    assistant chat turn rendered the same blob. Root cause was one line in
    `action_orchestrator/engine.py`: `parse_decision_response` fell back to
    `response[:200]` for its `thought`, and that field is what `review_service`
    stores as the summary and `assistant_chat` may speak as the reply. A model
    answering the tool-call prompt with a JSON object matched neither
    `<reasoning>` nor `Thought:`, so it hit the fallback *and* lost its actions.
    Now `_decision_from_json` reads that shape, and a `thought` that still looks
    like machine output becomes `None` rather than copy — callers already degrade
    ("Progress review complete.", or a plain chat turn). `_strip_fences` moved
    out of `workflows/assist.py` to `llm_text.py` for the second caller.
  - **Coder's header dropped its own controls off the right edge.** One
    `ui::cluster` with a `space_widget::horizontal()` pushing nine children
    right: the spacer collapses first, and everything past it falls off with no
    scrollbar and no clue. Opening the Files tree takes ~300px of that pane, so
    the *Files* toggle went with it — a tree you opened could not be closed — and
    at the stock window size "Open folder" and "New session" were **already**
    gone, which is the only way to change the workspace from that screen. Now
    `Row::wrap()`, no spacer.
  - **Transport errors were reaching the user as reqwest's own words**:
    Processes showed "error sending request for url (http://…/api/v1/teams/)"
    when the app raced the daemon's startup. Every screen's banner is
    `Error::to_string()`, so `client.rs`'s `Display` is user copy — a refused
    connection now reads "Cannot reach the server at http://127.0.0.1:18410" and
    a timeout says so. Covered by a test against a dead port.
  - Providers, Model ops, Logs, Settings → API and the Coder files pane and
    terminal drawer read clean (the terminal's colour, wrapping and prompt
    redraw were driven, not just opened).
  - **Swept 2026-08-07**, driving the live app rather than reading — Dashboard,
    Processes, Projects, Teams, Workflows, Assistants (E.V. + Memory), Coder:
    - **A stale error banner never cleared.** `Message::Listed(Ok(list))` in
      `processes.rs` was the one arm in the file that didn't clear
      `state.error` on success — every sibling success arm (`Detailed(Ok)`,
      `TeamsLoaded(Ok)`, …) already did. A single request that fails during the
      app-races-the-daemon startup window wedges the banner **forever**: the
      list keeps loading fine underneath (confirmed live — `/health` was 200,
      the process list was populating), but the red "Cannot reach the server"
      banner never goes away, even after leaving the screen and coming back,
      because nothing ever cleared it. Same bug, same missing line, found and
      fixed in six more files by grepping for the asymmetry (`.error = Some`
      with no matching `.error = None` on the paired success arm):
      `workflows.rs` (`Loaded(Ok)`), `todos.rs` (`BoardsLoaded(Ok)`,
      `BoardLoaded(Ok)`), `agenda.rs` (`ProjectsLoaded`/`DashboardLoaded`/
      `ReviewsLoaded(Ok)`), `library.rs` — Projects **and** Teams
      (`ProjectsLoaded`/`TeamsLoaded`/`TeamDetailLoaded(Ok)`), `providers.rs`
      (`EnvLoaded`/`CatalogLoaded(Ok)`), `modelops.rs` (`ProjectsLoaded`/
      `OllamaLoaded`/`RegistryLoaded(Ok)`). `apidocs.rs` and `coder.rs` already
      had it right — that's what made the missing line elsewhere visible as a
      bug rather than a style choice. 194 `cargo test -p agent-platform-desktop`
      cases still pass.
    - **A junk memory with no content.** The Assistants → Memory list had a row
      titled "Remembered:" with nothing under it — `memories.json` confirmed
      the *stored text itself* was the literal string `"Remembered:"`, not a
      rendering bug. The harvester model apparently narrated the save instead
      of stating a fact, and `memory.rs::parse_harvest`'s defensive filter
      (`line.len() > 3`, meant to catch exactly this per its own comment) let
      an 11-character label through. Fixed by also rejecting lines that end in
      `:` — a real memory is `clean()`'s "short third-person statement", never
      a bare label. Regression test added; the stray row itself is still in
      the live store (local user data — left for the user to delete via the
      trash icon rather than hand-edited).
    - Dashboard, Projects, Teams, Workflows, the Assistants E.V. pane and the
      Coder screen (header, transcript, session list) all read clean — no
      cutoffs, no overflow, no orphaned controls.
- **E.V. voice** — `POST /v1/audio/speech` proxies whatever `SPEECH_API_BASE`
  points at and the desktop tries it before its own engines. The self-hosted
  model is picked and stood up: `services/speech-service/` (Piper, CPU ONNX,
  OpenAI-shaped). Open: VAD auto-stop, streaming partials, wake word. Roadmap in
  `docs/ev-voice-roadmap.md`, setup in the service's own README.
- ~~**Assistant roadmap**~~ — `docs/practical-assistant-roadmap.md`. Its phases
  were server-complete and the UI is now ported in full. Todo boards are the
  **Plans** screen (`todos.rs`/`todos_view.rs`), and the assistant dashboard plus
  its review banner are **Agenda** (`agenda.rs`/`agenda_view.rs`): one project's
  assistant board by horizon, complete-from-the-card, and the reviewer's
  suggestions applied or dismissed whole. The last piece — the planning chat that
  *proposes* those actions — shipped as a pane on Agenda rather than a screen of
  its own; see **The planning chat** below. Note the desktop `assistant.rs` is
  the E.V. voice HUD, a different thing.
- **Rust server migration** — [ADR 0007](docs/adr/0007-strangler-rust-server.md):
  `agent-platformd` fronts the Python server and takes it over a domain at a
  time. Auth (including `last_used_at`), `/health`, `/`, projects, teams, todos, workflows (including the
  engine and its scheduler), the whole embedded LLM proxy (`/v1/*`, all nine
  routes), processes, the orchestrators, assistant, chat and coder are Rust —
  the whole of steps 1-4, plus `api_tokens` (6), the LLM-proxy admin surface (7)
  the `action_orchestrator` routes (8), most of model-ops (9) and the
  workspace/document stack (10). **Every Python router now has Rust in front of
  it.** Still proxied by decision, and only these: model-ops' four pipeline-job
  routes, `POST /upload` and `GET /file` for a `.pdf`, `POST /chat/threads`,
  `POST /llm-proxy/config-yaml`, and `system_routes`. **Next steps are listed
  below.**
- **In-process inference** — [ADR 0006](docs/adr/0006-in-process-rust-core.md):
  link llama.cpp into the desktop binary instead of shelling out to Ollama.
  **Half of this was superseded on 2026-08-22 by
  [ADR 0012](docs/adr/0012-managed-llama-server.md):** the *server* side is a
  managed `llama-server` subprocess now, and the in-process engine is the
  desktop app's own chat alone. The steps below are still the app's. The
  full Rust port was reviewed and rejected; the server stays Python. The Phase 0
  spike (`desktop/spike-llama/`) is **answered and closed**: with CUDA Toolkit
  13.3 installed, `llama-cpp-2` builds on Windows and runs 123.5 tok/s against
  Ollama's 112.7 on the same weights, same sitting — parity, kill criteria
  cleared. First slice shipped the same day: the desktop's own chat answers
  in-process behind the `local-llm` feature (off by default; `cuda` implies
  it), with `local_model_path`/`local_n_ctx` in `settings.json`, and KV-cache
  reuse across turns landed after it. **Next steps are listed below.**
  Related defect found while measuring: local chat routes to Ollama's
  OpenAI-compat endpoint, which takes no `options`, so every reply loads at
  131k context (23 GB, 39% spilled to CPU, ~4–5× slower). Not fixable from this
  codebase — see the ADR's results section. Either the Ollama app's own context
  setting changes, or the proxy moves to native `/api/chat`.
- **Coder screen (hearth migration)** — folding the standalone hearth IDE
  (`../hearth`, Tauri + React + Monaco) into this app so coding happens here
  instead of in a second product. The agent half is shipped and driven; the IDE
  surfaces are not, and one of them (preview) has no native path at all.
  **Next steps are listed below, ordered and pickable.**
- **Project sub-groups** — `Project` is a flat folder (no `parent_id`/tags);
  nested groups if career workflows demand it.
- **Document routing** — per-model native PDF/vision vs derived markdown
  (capability flags exist on providers).
- **portal_desktop, the second client** — reviewed 2026-08-08,
  [`docs/portal-desktop-review.md`](docs/portal-desktop-review.md). The
  SvelteKit/Tauri app at `../../../portal/portal_desktop` calls this platform and
  the migration did **not** break it: every route it uses answers, on the same
  port with the same auth (verified live, not read). What broke is its
  *documentation* — it sends users to `/config`, `/tokens` and `/docs`, three
  pages that died with the Python server. Fixed on its side the same day, along
  with a release-notes template that said the same thing. Left open:
  - ~~**Decide the `tools` field.**~~ — **honoured**, same day. Portal had been
    putting a mode-filtered tool list in the stream body since before the
    migration and nothing ever read it (Python's `CoderChatSendRequest` had no
    such field either). Now `SendRequest.tools` → `TurnOptions.tools` →
    `call_llm_step`, with three distinct states: absent is this crate's six
    specs, a non-empty list is the caller's verbatim, and `[]` is a tool-free
    turn rather than a surprise default set. Capped at 64 entries / 64 KB and
    required to be objects — it goes straight into an upstream request body, so
    it is a trust boundary, not a passthrough. **Only meaningful to a delegating
    client**, which runs whatever the model calls; a non-delegating caller gets
    `"Error: unknown tool '…'."` back as the tool result, which is what a
    hallucinated call has always got. Portal needed no change.
  - ~~**Release + updater.**~~ — **shipped**, see below.
  - **Merged? No** — ADR 0005 and 0007 already decided against the webview stack,
    the two UIs cannot share a line of code, and four of portal's two dozen
    domains touch this platform. The split that follows: portal has a webview so
    it is the one that can ever have Monaco/preview/xterm (the three things the
    hearth item closed as impossible here); this app is the platform console.
    What that costs is a contract two clients agree on — hence
    [`docs/coder-delegation-protocol.md`](docs/coder-delegation-protocol.md), and
    it raises `openapi.json`'s silent drift from tidiness to correctness.
- **Deployment hardening** — the gap between "runs on this machine" and
  "survives a container restart". Four landed 2026-08-08 (below); the rest are
  listed there and none is started.

### Releases — 2026-08-08

The repo could build and test and had no way to hand anyone a binary. Two
workflows now, deliberately not one:

- **`.github/workflows/release.yml` — generated by `dist` (cargo-dist 0.32),
  never hand-edited.** Config is `dist-workspace.toml` at the repo root;
  regenerate with `dist generate` after changing it. Tags matching
  `**[0-9]+.[0-9]+.[0-9]+*` publish `agent-platformd` for
  linux-x64 / macos-arm64 / macos-x64 / windows-x64, with shell + PowerShell
  installers, checksums, and an `…-update` self-updater binary per platform.
  `pr-run-mode = "plan"` runs `dist plan` on PRs, so a config that cannot
  release fails before the tag rather than after it.
  - **The config lives at the repo root and the Cargo workspace does not.**
    GitHub only reads `.github/workflows/` from the root, so `dist-workspace.toml`
    bridges the two with `members = ["cargo:desktop"]`. `[profile.dist]` still
    has to be in `desktop/Cargo.toml`, because that is the workspace cargo sees.
  - **`agent-platform-desktop` is `dist = false`.** `dist` has no per-package
    target list, so including it would mean a release that reliably fails on
    three platforms of the four.
- **`.github/workflows/release-desktop.yml` — the app's Windows build, as a
  `dist` custom job rather than a release of its own.** `local-artifacts-jobs`
  makes `dist` call it during build-local-artifacts and fold what it uploads
  into the same release, so **one tag, `v<version>`, ships both**. The zip
  carries **both** exes: the app spawns the daemon from its own directory, so
  shipping one without the other produces an app that starts and cannot reach a
  server. `LIBCLANG_PATH` is set for the same bindgen reason as in `ci.yml`, and
  it builds `--profile dist` so both halves of a release are built the same way.
  - **This was two tag series first, and that was wrong twice over.** A
    separate `desktop-v*` workflow meant two workflows racing to create one
    release — and worse, `dist`'s generated trigger is a *prefix glob*
    (`**[0-9]+.[0-9]+.[0-9]+*`), so `desktop-v0.2.0` matched it too: tagging the
    app would have started a *daemon* release hunting for a package at version
    0.2.0 and finding only the one that is `dist = false`. Namespacing the
    daemon's tags fixed the collision and left the race; one job inside one
    workflow fixes both.
  - **The two crates are versioned in lockstep** (server moved 0.1.0 → 0.2.0 to
    meet the app), because `dist` derives the tag from the version and a skew is
    two tags for one product.
  - The contract with `dist` is the artifact *name*: anything uploaded as
    `artifacts-*` is collected into `target/distrib/`, and a zip must have its
    files at the root rather than nested.

**The app's updater is both halves now** — `update_check.rs` plus a Version
card in Settings → Status. The **check** asks the releases API for the newest
`v*` and compares numerically (so `0.10.0` beats `0.9.0`); it is a button, never
a poll, because this app runs offline by design and should not phone GitHub on
launch. The **install** landed 2026-08-22, once `v0.4.0` existed to point it at:
it downloads that release's Windows zip, verifies the `.sha256` published beside
it, and swaps both exes before reusing `Message::RestartApp`.

- **Windows locks a running `.exe` against deletion but allows renaming it.**
  That is the whole trick: each binary moves to `<name>.old` and the new one
  takes the name it vacated, the process keeps executing the renamed file, and
  `sweep_old` clears the leftovers at the next boot. No `self_update`/
  `axoupdater` dependency was needed for it.
- **Both exes or neither.** The app spawns the daemon from its own directory, so
  a half-swapped install is a version skew across the wire contract. `locate`
  proves both are in the archive before anything is touched, and a failure
  partway rolls back what it moved — two of the module's five tests are that.
- **The daemon goes down first**, before the download rather than between the
  download and the swap: its exe is replaced too and a running child holds the
  handle. A failed install starts it straight back up.
- **Unpacking shells out to `C:\Windows\System32\tar.exe`** by absolute path,
  for the reason `managed_server.rs` documents: a bare `tar` can resolve to the
  GNU tar in a git-bash on PATH, which cannot read a zip.
- The checksum is fetched from the same host as the archive, so it is an
  integrity check and not a defence against a compromised release — it catches
  the truncated download *before* it lands on a working install.
- Still `dist`'s `agent-platform-server-<target>-update` for a daemon installed
  by itself; this is the desktop pair, which moves together.

- **The tag prefix is load-bearing and it changed.** While there were two
  series this filtered for `desktop-v*`; collapsing to one release left that
  prefix matching nothing, which makes the card answer "Up to date." forever —
  the one answer it must never give wrongly. It reads `v*` now, and the test
  keeps a stray `desktop-v9.9.9` in its fixture so a 9.x of the abandoned
  series cannot be read as an upgrade.

Both halves were **driven, not only unit-tested**:

- **The Version card**, on a second app instance launched with
  `AGENT_PLATFORM_PORT=18499` so it never touched the running one: Settings →
  Status renders it under the API-server card, "This build 0.2.0" in mono, and
  pressing *Check for updates* reaches GitHub and settles on "Up to date." — a
  real call against a repo with zero releases, which is the answer it should
  give. Two things worth knowing for the next person who drives this app:
  **`CopyFromScreen` reads the screen, not the window**, so a shot taken
  without foregrounding first captured a *second instance of the same app*
  sitting in front and looked like a rendering bug; and **the first click after
  the ALT-tap is eaten often enough to matter** — clicking twice is the cheap
  remedy, and the calibration click that proved the coordinates were right is
  what separated "wrong offset" from "dropped event".
- **Caller-supplied tools reach the model.** One turn against `devstral:24b`
  with a single tool named `portal_delegate_task` in the body — a name this
  crate does not implement and never advertises. The stream answered
  `event: tool_call {"name": "portal_delegate_task", …}`, which it can only do
  if the list was forwarded. That is also the whole protocol working: the turn
  then parked, waiting for a `tool-result` that was never sent.

**Borrowed from portal_desktop, 2026-08-22.** Two of its three habits are here
now; the third was already true.

- **`scripts/prepare_release.py`** — `patch`/`minor`/`major`/`X.Y.Z`/`current`,
  writes both crates in lockstep, runs `cargo update --workspace --offline`,
  *proves* Cargo.lock agrees, then commits the bump and tags it. **The commit
  comes before the tag**, which portal's own script gets wrong: a tag cut first
  names the commit before the bump, so the release builds a tree still carrying
  the old version.
- **`.github/workflows/release-smoke.yml`** — one `cargo check --locked
  -p agent-platform-server` on ubuntu, wired in as a `dist` `plan-jobs` entry so
  `build-local-artifacts` and the desktop job both wait on it. `dist plan`
  validates config but compiles nothing, so it cannot see a stale lockfile;
  every build job runs `--locked` and all four fail on one several minutes in.
  portal lost v0.7.0 to exactly that, which is why it has this job at all.
  `--locked` is workspace-wide regardless of `-p`, so one package catches a lock
  that disagrees with either manifest. The app is not checked here: it is
  Windows-only and does not build on a Linux runner.
- `fail-fast: false` so one platform's break does not cancel three good builds —
  `dist` already generates that.

**Not borrowed: the installer.** portal ships NSIS/MSI/dmg/AppImage because
`tauri-action` bundles them for free; this app's release is a zip of two exes,
and `scripts/build_installer.py` (Inno Setup) still runs only on a developer's
machine. Wiring it into `release-desktop.yml` means proving `iscc` on the runner
and reconciling the `.iss`'s `target/release` paths with the workflow's
`--profile dist` — worth doing when someone wants Start-menu entries and an
uninstaller, not before.

**What the first tag taught, `v0.2.0`.** It failed in every build job and
published nothing, which is the right way for it to fail. Three separate causes,
and only the first was in the release config:

1. **`dist` builds with `--workspace` by default.** `packages` scopes what is
   *released*, not what is *built*, so every daemon job dragged the iced app in
   and died on `atk-sys` (Linux) or `whisper-rs-sys` (Windows) — GTK and
   whisper.cpp, neither of any use to a headless server. `precise-builds = true`
   is the fix. `dist plan` cannot catch this: planning does not build.
2. **`desktop/.cargo/config.toml` is tracked and hardcodes one machine's path.**
   Its `CMAKE` points inside Visual Studio **Community**; the runners have
   **Enterprise**, so the path does not exist and the cmake crate reports
   "The system cannot find the path specified. (os error 3)" — a message naming
   neither cmake nor whisper. Its own comment says it is "harmless elsewhere …
   when this file is absent", which was never true: it is committed, so it is
   never absent. Both Windows jobs now export `CMAKE=cmake`, and cargo's `[env]`
   yields to a variable already set. **The real fix is still open**: a
   machine-specific absolute path belongs in `~/.cargo/config.toml`, not in the
   repo, and anyone whose VS is not Community 18 at that exact path hits this.
3. **CI had never run at all**, on this branch or any other — it was added
   2026-08-08 and the branch was not pushed until the tag. Its first run failed
   too, and one of the failures is real: `postgres_schema.rs` reports
   `relation "action_sets" does not exist` while applying migration 1. That test
   skips itself without `TEST_DATABASE_URL`, so CI is the first place it has ever
   executed — exactly what the job was added for.

Still unverified, because no job got far enough to try it: whether `dist`
attaches an artifact it did not itself plan. If the app's zip is missing from the
next release, that is where to look, and `gh release upload` is the stopgap.

**Next tag should be `v0.2.1`** — 0.2.0 is spent. Push the fixes and let CI go
green on Windows first; that is the cheap proof of (2), and it costs a push
rather than a public tag.

### Deployment hardening — 2026-08-08

A production-readiness pass over `agent-platformd`. The code was in good shape;
what was missing was everything that only matters once the process is not being
babysat by a developer.

**Landed:**

1. **CI, for the first time** — `.github/workflows/ci.yml`. 490-odd tests
   existed and nothing ran them. Two build jobs because the workspace does not
   build in one place: the server (the deployable artifact, no GUI or audio
   deps) on Linux, the desktop app on Windows with `LIBCLANG_PATH` set for
   whisper's bindgen. `cargo build` runs beside `cargo test` for the reason the
   runbook already gives — dev-dependencies unify features back into the lib.
   Clippy runs without `-D warnings`: gating a never-gated repo in the same
   commit that adds the gate makes the first run red for unrelated reasons.
2. **SIGTERM is handled** — `serve` listened for Ctrl-C only, so `docker stop`,
   `systemctl stop` and a pod eviction were all ignored until the SIGKILL that
   follows. Every in-flight SSE stream, DAG executor step and model-build stage
   died mid-write. `shutdown_signal()` selects over both on unix; Windows has
   no SIGTERM and `ctrl_c` already covers its console events.
3. **An exposed bind with no master key is refused at startup** — auth being
   fully open when `AGENT_PLATFORM_MASTER_KEY` is unset is a *loopback*
   convenience, and `AGENT_PLATFORM_HOST` is an environment variable. The two
   together published every route, with nothing in the startup output saying
   so. `AGENT_PLATFORM_ALLOW_OPEN=1` is the deliberate override.
   `is_loopback` treats anything it does not recognise as exposed.
4. **`/health` touches the database** — it answered `ok` from the fact that the
   handler ran, so a server whose SQLite file was deleted, locked or out of
   disk reported healthy while 500ing every other route. Now `SELECT 1` and a
   503 on failure. It runs on `pool`, not `any`, because `pool` carries
   `busy_timeout(30s)`: the desktop's adopt-or-spawn check reads a non-200 as
   "dead", and an instant `SQLITE_BUSY` would have it start a second server
   against the same file.

5. **Inbound limits.** axum's 2 MB extractor default was the only cap, and it
   applied to the *upload* routes too — Starlette had no limit, so the port
   quietly introduced a ceiling nothing documents. Now a 16 MB general cap
   (`AGENT_PLATFORM_MAX_BODY_MB`) with the four multipart routes raising it to
   512 MB for themselves (`AGENT_PLATFORM_MAX_UPLOAD_MB`); a cap has to exist
   because every handler reads the body into memory before it looks at it. Plus
   `RequestBodyTimeoutLayer` at 60s for the slow-body hold, which the per-call
   reqwest timeouts cannot cover. **Request body only** — a response timeout
   here would cut every SSE stream this server serves at the same mark. The
   test drives the router through `tower::oneshot` rather than a socket: a body
   limit is rejected while the client is still writing, so over a real
   connection reqwest reports the reset instead of the 413 and which one you
   see depends on buffer sizes.
6. **A real migration runner.** `schema.sql` became
   `migrations/0001_initial.sql` under `sqlx::migrate!`, embedded in the binary
   so the artifact is still one file. The squash is not a replay: the thirty
   Alembic revisions are not carried, because every database in existence was
   at head the day Python was deleted. Verified against a **copy of the real
   user database** — 29 tables and no `_sqlx_migrations` before, 30 and one
   after, every row count identical, `/health` 200 and `GET /projects` still
   returning the real row. A schema change is a new `000N_*.sql` from here;
   editing an applied one stops every existing database from starting.
7. **The rate-limit window map is pruned** at 1024 entries. Size-triggered, not
   timed: a sweep of stale minutes costs less than the map that made it needed.

8. **openapi drift is detected**, one direction. `tests/openapi_drift.rs`
   drives every operation the document declares through the router and fails
   if it hits the fallback. A route that exists but is *undocumented* stays
   invisible — an axum `Router` cannot be asked what it serves — and that
   remaining half is what `utoipa` would close, worth its cost the first time
   an undocumented route matters. **The first run found two real things:**
   `GET /api/v1/api-tokens/scopes` was documented and never served (removed
   from the document — nothing calls it, and Python's body cannot be
   reproduced from what is left), and the bare spellings of the collection
   paths had become 404s. FastAPI answered `/api/v1/workspaces` with a 307
   onto `/api/v1/workspaces/`, so `workspaces.rs` and `api_tokens.rs`
   registered only the slashed form and let the bare one fall through to the
   proxy — correct while there was a proxy, a 404 the moment there was not,
   and invisible to the drift test because the document only ever declared the
   slashed spelling. Both are registered now, as `projects.rs` already did.
9. **Three Python-JSON renderers became one.** `dag_schema::PyJson`,
   `workflow_engine::PythonJson` and `todos::EnsureAscii` were the same UTF-16
   surrogate walk written twice and the same separator pair written twice,
   differing only on `json.dumps`'s default separators versus the tight pair.
   That is a field now; `python_json` and `python_json_compact` are the two
   call shapes anything needs.
10. **A startup backup, and a WAL checkpoint at shutdown.** There was no backup
    of any kind: the desktop's SQLite file is a user's whole workspace, on a
    laptop, copied nowhere. `db::backup` takes a `VACUUM INTO` snapshot after
    the listener binds (SQLite's own consistent snapshot — copying `*.db` while
    the WAL holds uncheckpointed pages produces a file missing them) and keeps
    three generations beside the database; `AGENT_PLATFORM_BACKUP=0` skips it.
    `db::checkpoint` runs `PRAGMA wal_checkpoint(TRUNCATE)` past the graceful
    drain, because SQLite checkpoints at 1000 pages but never truncates and the
    sidecar only grows — 4 MB beside a 496 KB database on a real install.
    Verified live against a copy of the real user database: the `.bak` opens as
    a database with every row in it.

11. **DNS rebinding is closed.** The default install binds `127.0.0.1` with no
    master key, and `POST /api/v1/coder/…` runs `run_command` — `cmd /C` or
    `sh -c` with whatever string it is handed, `allow_commands` defaulting to
    true on that route. CORS is off by default and a JSON body forces a
    preflight, so an ordinary page cannot reach it; rebinding is the case those
    two do not cover, because the browser treats a domain re-resolved to
    127.0.0.1 as same-origin and applies no CORS check at all. `host_guard`
    refuses a `Host` that is not this machine's, with a 421, and
    `AGENT_PLATFORM_ALLOWED_HOSTS` adds to the list. **Loopback binds only** — a
    server on `0.0.0.0` is reached by container names and LAN addresses there is
    no list of, and is covered instead by the master key it is now required to
    have, which is exactly what a rebinding attacker does not hold.

12. **The desktop's own state files are written atomically.** `settings.json`,
    `chats.json`, `memories.json` and `master.key` are each rewritten whole on
    every change with `std::fs::write`, which truncates first — and all four
    loaders fall back to a default when parsing fails, so a half-written file is
    not an error anyone sees, it is the user's settings or their entire chat
    history silently gone. Not a remote possibility either: quit is
    `std::process::exit(0)` (`iced::exit()` hangs on Windows), which does not
    wait for a save in flight. `shell::write_atomic` writes a sibling, `sync_all`s
    it and renames over. The agent's own `write_file` tool is left alone — an
    in-place truncate is what editors do and what file watchers expect.

**Not done, and why:**

13. **Postgres has a schema, and it is proven against a real server.** This was
    the blocker in front of the `sqlx::Any` conversion, and it was never the
    280 query sites. `migrations/` is now `migrations/sqlite/` and
    `migrations/postgres/` — same version, same description, two dialects, both
    embedded, the pool's backend picking. **Three differences, every one found
    by running the file rather than reading it:** `DATETIME` is not a Postgres
    type (57 columns); every primary key was `id INTEGER NOT NULL` leaning on
    SQLite's rowid, so without `GENERATED BY DEFAULT AS IDENTITY` every INSERT
    that omits an id fails; and `BOOLEAN DEFAULT 0` is rejected outright
    ("default expression is of type integer"). `tests/postgres_schema.rs`
    applies the schema twice, inserts with and without an id, and exercises
    `db::sql`'s `?`→`$n` rewriting and the `CAST(… AS BIGINT/TEXT)` decoding
    that `db.rs` documents — all against a live Postgres, which is the only
    thing that can check any of it. It skips with a message when there is no
    DSN, and CI now runs a `postgres:16` service so it never skips there. It
    creates a private schema and drops it, refusing to run at all if the name
    already exists, so it never touches real tables.

**Not done, and why:**

14. **`DATABASE_URL` works.** All thirteen domains are off `state.pool`, the
    `SqlitePool` field is deleted, and `Config::from_env` passes a DSN through
    instead of refusing it. `tests/postgres_schema.rs` drives the real router
    against a real Postgres — create a project, read it back — so this is a
    server that answers from Postgres, not a schema that parses.

    **Four incompatibilities, none visible from reading the code.** Each was
    found by running it:

    - **Timestamps are text, and the Postgres schema now says so.** Every
      timestamp in this server is a string end to end — `wire::sql_now()`
      makes one, every INSERT binds one, the scheduler compares them with
      `<=`. Postgres refuses to bind text into a `timestamp` column, so the
      migration declares TEXT. A TIMESTAMP column would have meant casting at
      several hundred write sites to gain a type nothing reads, and SQLite's
      DATETIME is text already. The format is fixed-width, so it still sorts.
    - **Three helpers named a backend in their signatures** while their call
      sites looked converted: `workflows::set_workflow_field` and
      `todos::set_item_column` were generic over `sqlx::Sqlite`, and
      `assistant::purge_todo_board` / `executor::insert_task_node` took a
      `Transaction<'_, Sqlite>`. A domain is not converted until its helpers
      stop naming one.
    - **A computed query's rewritten string has to outlive the query.** Five
      sites build SQL conditionally and then add binds one at a time; the
      `Cow` from `db::sql` dies first. In `list_action_sets` it died at the end
      of a `match` arm.
    - **Postgres type-checks binds and SQLite does not.** A string against an
      `integer` column is `operator does not exist: integer = text` here and a
      silent no-match there.

    Also: the two Postgres tests share a database, and `sqlx::migrate!` takes a
    session advisory lock — running them together reports `deadlock detected`,
    because a scratch schema isolates tables and not the migrator's lock. They
    serialise on a mutex.
- **No global concurrency cap, deliberately.** `chat` has the one that matters
  (`AGENT_PLATFORM_CHAT_MAX_CONCURRENT`), because it is the path that costs an
  upstream call. A cap over the whole router queues rather than sheds, so under
  load it converts a rejection into unbounded latency — worth adding with a
  load-shed policy, not on its own.
- **Foreign keys stay off**, with the reason now measured rather than
  inherited: `PRAGMA foreign_key_check` on a real user database returns 55
  violations, all `eventlog.task_id` pointing at tasknodes a finished DAG
  deleted. SQLAlchemy left the pragma at SQLite's default OFF, so this data was
  never checked in its life. Turning them on needs a migration that rebuilds
  those tables with `ON DELETE` actions — SQLite has no `ALTER TABLE … ADD
  CONSTRAINT` — and clears the orphans. The handlers delete children
  explicitly, which is why nothing is broken today.
- **Rate limiting is per-token and in-process**; the master key is unlimited,
  and an N-up deployment would need a shared store.
- **The backup is on-machine only.** Three generations beside the database
  survive a corrupt file, not a lost disk. Off-machine is a deployment's job.

### Rust server migration — **closed 2026-08-07**

> **The Python server is deleted.** `app/` is gone, `agent-platformd` answers
> every route, and the proxy fallback is a 404. Everything below this box is the
> record of how it got there; nothing in it is outstanding work. What replaced
> the last six proxied shapes:
>
> | Was proxied | Now | Note |
> |---|---|---|
> | `POST /assistant/chat/threads` | `assistant.rs::chat_threads_create` | ~40 LOC; it was only ever proxied because it shares a path with the GET |
> | `POST /llm-proxy/config-yaml` | `config_schema.rs` | a hand-rolled Draft 2020-12 subset that reproduces `jsonschema`'s message wording — verified against the real library on six documents |
> | `/system/status`, `/system/logs` | `system.rs` + `observability.rs` | needed a log ring in Rust first; `logd!` feeds it and every `eprintln!` became one |
> | `POST /upload`, `GET /file` on a `.pdf` | `documents.rs` | extractor is the `pdf-extract` crate, **not** PyMuPDF — see below |
> | model-ops' five job routes | `model_ops.rs` | the pipeline is still Python, as a subprocess worker |
> | `todos agent/step` naming a document | `todos.rs::merge_workspace_documents` | fell out of the PDF port |
>
> **Four things changed behaviour, and none of them is byte-identical.** ADR 0007
> rule 5 ("a domain lands byte-identical or it does not land") governed the
> strangler period; retiring the interpreter overrides it, and these are the
> receipts:
>
> 1. **PDF text differs.** `pdf-extract` is not PyMuPDF. Document *shape* is
>    preserved (title, page count, `## Page N`, the scanned-page notice) because
>    the excerpt and the chat context are built from it, but word and line breaks
>    inside a page differ, and **`### Layout notes` are gone** — they came from
>    per-block bounding boxes the crate does not expose. Re-ingesting converges a
>    workspace on the new extractor. See `documents.rs`.
> 2. **`/system/status` renamed `python` to `server`.** It was
>    `sys.version.split()[0]`; there is no interpreter to ask. It now reports the
>    crate version, and `platform` is `windows-x86_64` where Python said
>    `Windows-11-10.0.26200-SP0` (a build number needs `os_info` or three blocks
>    of `unsafe`). The desktop's Status card and `SystemStatus` moved with it.
> 3. **Alembic is gone.** `schema.sql` + `db::ensure_schema` create the schema
>    from the final head (`e0f1a2b3c4d5`). **It creates; it does not migrate** —
>    the next column change needs a versioned runner built first. Existing
>    databases are unaffected (every statement is `IF NOT EXISTS`, and they are
>    all at head already).
> 4. **`/openapi.json` is a checked-in file.** FastAPI generated it from route
>    declarations; axum cannot enumerate its own router, so the document was
>    captured on the day `app/` was deleted and is now served verbatim from
>    `desktop/crates/server/src/openapi.json`. **It will drift and nothing
>    detects that** — the honest fix is `utoipa` annotations, worth doing the
>    first time a stale entry misleads someone. It is what Settings → API renders.
>
> **Two features died with the interpreter, both already dead in practice.** The
> DAG task tool-calling path (`tool_handlers.py`, 782 LOC) and the MCP client
> that was only reachable through it: `AGENT_PLATFORM_TOOLS_ENABLED` has never
> been set in this deployment, and `executor.rs` already *refused* to run a task
> configured for tools rather than answering without them. That refusal is now
> permanent — the "or run the Python server directly" escape hatch in its error
> message is the part that is no longer true. Coder tools and the assistant's
> action tools are unaffected; they are different code (`coder_tools.rs`,
> `action_orchestrator.rs`).
>
> **What is still Python, and always will be:** `worker/` — the LoRA training
> pipeline. It is torch and peft and there is no porting it. It is no longer a
> *server*, though: `agent-platformd` spawns each stage as a subprocess with
> `MODEL_OPS_PYTHON`, and the stage reports back on stdout with
> `@@AGP:<kind>@@ {json}` markers. That is what closed the two blockers ADR 0007
> named for this domain — `eval`'s result came back through a function return
> (now a marker) and `register_model_entry` wrote the database from inside the
> training child (now a marker the parent persists), so `model_build_jobs` and
> the registry tables have one writer. Cancellation was the third: `runner.py`'s
> module-level `_running` dict is `AppState::model_jobs`, and it works now
> because there is only one process.
>
> **Proven by running it**, not by reading: a fresh database created and
> populated from `schema.sql`; workspace → project → assistant thread created and
> listed; a real PDF uploaded, extracted and read back through `workspace_read`;
> `config-yaml` rejected for bad YAML and for a schema violation with the exact
> sentence `jsonschema` produces, then accepted; a two-stage (`prepare`, `eval`)
> build job run to `succeeded` with its `eval` result parsed off the worker's
> stdout into `result_json`; the job's SSE stream, `cancel` 409/404, and
> `operations/build` including its nested validation `loc`. 468 tests pass.

<details>
<summary>Historical: the strangler migration, 2026-08-05 → 2026-08-07</summary>



> **Read this first if you are picking the work up cold (2026-08-06).**
>
> **Nothing in this migration is committed.** `HEAD` is `93b6e06`, and 88 paths
> are dirty — every Rust module from `llm.rs` through `executor.rs`, `chat.rs`
> and `processes.rs` is **untracked**, not modified. Commit with an explicit
> pathspec (`git commit -- <paths>`), never `-a`: see the next point.
>
> **A second migration is in flight through the same crate.** Postgres support
> (`db.rs`, `sqlx::Any`) is being added by someone else, and `AppState` carries
> `pool` *and* `any` on purpose while domains move across one at a time. Do not
> "tidy" that, and expect `lib.rs` and `Cargo.toml` to move under you. Before
> starting anything large in `desktop/crates/server/src/`, check whether that
> refactor has landed — a 2.5k-LOC port into files being converted query-by-query
> is a merge nobody wants to referee.
>
> **State of play (2026-08-07):** steps 1-10 are all done, and so is the
> todos/workflows split and the `processes`/orchestrator closure. What is left is
> not code, it is Python by deliberate, permanent decision: model-ops' four
> pipeline-job routes (the training subprocess and its `cancel` live in the
> process that started them), `POST /upload` + `GET /file` for a `.pdf`
> (PyMuPDF), `POST /chat/threads` (shares a path with a Rust GET),
> `POST /llm-proxy/config-yaml` (`jsonschema`'s own error text), and
> `system_routes` (two fields — `python`, `platform` — that only Python can
> answer). The verification harness lives in the session scratchpad and is gone —
> the *method* is in the `prove-domain` skill, which now carries the three traps
> that cost time here (fresh DB per run, check the port is free and that the
> daemon actually bound, and coder's suite hardcodes `Bearer test-key`).
>
> **The one open decision is closed.** `/llm/ready`, `/chat/resolved-defaults`
> and `/llm/ui-catalog` had no caller in this repo and named the deleted Flow UI;
> decided 2026-08-07 to delete rather than port. Gone from Python.

Where it stands (2026-08-06). `agent-platformd` binds 18410, spawns Python on an
ephemeral port, and answers these itself; everything else falls through to the
proxy byte-for-byte:

| Surface | File | Left with Python |
|---------|------|------------------|
| Bearer auth: master key, `agp_` tokens, open-when-no-key | `auth.rs` | `last_used_at` writes |
| `/health`, `/` | `lib.rs` | — |
| projects | `projects.rs` | `{id}/processes`, `{id}/workspace/*` |
| teams | `teams.rs` | — |
| todos — **the whole domain**, `agent/step` included | `todos.rs`, `action_orchestrator.rs` | only a step that *names a document*, which is handed back to the proxy whole (see step 3.7) |
| processes — **all eleven routes** — plus `projects/{id}/processes`, the SSE stream, the DAG executor, sub-DAG expansion and startup recovery | `processes.rs`, `executor.rs`, `dag_schema.rs` | — |
| workflows + run history + assist + the engine and its interval scheduler | `workflows.rs`, `workflow_engine.rs` | — **the domain is whole** |
| LLM proxy — **all nine `/v1` routes**, plus the admin surface's fourteen | `llm.rs`, `llm_config.rs`, `byok.rs`, `model_catalog.rs`, `model_capabilities.rs`, `provider_catalog.rs`, `upstream_http.rs`, `usage.rs`, `llm_admin.rs` | `POST /llm-proxy/config-yaml` alone, for its `jsonschema` error text (step 7) |
| `POST /api/v1/chat` + the shared chat helpers | `chat.rs`, `chat_usage.rs`, `chat_thread_title.rs` | — (the three dead Flow-UI GETs are deleted, not proxied) |
| coder — **all ten routes**, the agent loop, both executors and the delegated tool park | `coder.rs`, `coder_loop.rs`, `coder_tools.rs` | — **the domain is whole** |
| Request correlation id on every response and error | `request_id.rs` | — |

**Assistant: 19 of 20 routes now Rust — the domain is whole** (`assistant.rs` +
[`assistant_turn.rs`](desktop/crates/server/src/assistant_turn.rs) +
[`clarifying_form.rs`](desktop/crates/server/src/clarifying_form.rs),
2026-08-06/07) — `GET dashboard`, `GET goals`, `GET`/`POST chat/threads` (POST
proxied), `GET chat/context-usage`, `GET chat/thread`, `POST chat/send`,
`GET`/`PATCH profile/{domain}`, `GET profile`, `GET profile/forms`,
`POST chat/apply`, `POST reviews/run`, `GET reviews/pending`,
`POST reviews/{id}/apply`, `POST reviews/{id}/dismiss`,
`POST items/{id}/complete`, `POST chat/retry`, `POST chat/submit-form`,
`POST reset`. **Only `POST chat/threads` is left with Python, and by choice
not by blocker** — it shares a path with the `GET` Rust owns, so it is
declared to `proxy::forward` explicitly.

**Coder: all ten routes now Rust — the domain is whole** (`coder.rs` +
[`coder_loop.rs`](desktop/crates/server/src/coder_loop.rs) +
[`coder_tools.rs`](desktop/crates/server/src/coder_tools.rs), 2026-08-07). The
five CRUD routes shipped first; `send`, `stream`, `retry`, `approve` and
`tool-result` landed together, as the scope note said they had to.

**The router table, and what is left of it.** This list named six routers with
no Rust in front of them on the morning of 2026-08-07; by the end of that day
every one had been taken or deliberately closed:

| Router (`main.py`) | Surface | Notes |
|---|---|---|
| ~~`api_tokens_router`~~ | ~~`/workspaces/{id}/api-tokens/*`~~ | **done 2026-08-07 (step 6)** — `api_tokens.rs` |
| ~~`workspaces`, `me_workspace`, `workspace`, `workspace_files`~~ | ~~the workspace/document stack~~ | **done 2026-08-07 (step 10)** — `workspaces.rs` + `workspace_files.rs`; `POST /upload` and `GET /file` for a `.pdf` stay, for PyMuPDF |
| ~~`action_orchestrator_router`~~ | ~~`/action-sets`, `/sessions`, `/decide`~~ | **done 2026-08-07 (step 8)** — all eleven routes in `action_orchestrator.rs` |
| `model_ops_router` | `/api/v1/model-ops/*` | **13 of 17 done 2026-08-07 (step 9)** — `model_ops.rs`; the four pipeline-job routes stay, with the runner |
| ~~`llm_proxy_admin_router`~~ | ~~`/api/v1/llm-proxy/*`~~ | **done 2026-08-07 (step 7)** — `llm_admin.rs`, fourteen of fifteen routes |
| `system_router` | `/api/v1/system/{status,logs}` | **deliberately staying — see step 5** |

Python is still not retired — six request shapes are proxied on purpose, listed
under the steps that took the rest — but no router is wholly its own any more.
"Step 5 last" was about ordering within the fan-in, never about finishing.
Playground was deleted rather than ported (step 4½).

**How a domain is proven.** Point the existing pytest file at a running server
and compare failure sets, then cross-render the same rows through both servers
and diff the parsed bodies — the second catches what no test asserts (it is how
the timestamp and foreign-key bugs surfaced):

```bash
AGENT_PLATFORM_TEST_BASE_URL=http://127.0.0.1:18456 \
AGENT_PLATFORM_TEST_KEY=<key> pytest app/tests/test_<domain>_api.py -q
```

The daemon logs its child's origin on startup (`… → http://127.0.0.1:<port>`);
run the same file against that URL and the two failure sets must match.

**Re-measured 2026-08-06, and it holds: 20 of 50 fail, and the two sets are
identical** across `projects`, `teams`, `todos` and `workflows` — every one a
test that mocks in-process or reads the test engine directly (the `assist_*`
LLM mocks, `test_build_item_context`, `test_agent_step_endpoint_mocked`,
`test_trigger_webhook_apply`, the `*_nullifies_*_fk` pair, the colour/default
helpers, `test_scheduler_runs_due_workflows`). The old figure was 19; the extra
one came with suites that have grown since, not with a regression.

Two things that method needs, both learned by getting them wrong:

- **A throwaway instance, not the app's.** These suites write: they create and
  delete projects, teams, boards and workflows. Both halves read
  `AGENT_PLATFORM_DB_PATH`, so one env var isolates them —
  `AGENT_PLATFORM_PORT=18456 AGENT_PLATFORM_DB_PATH=<scratch>.db
  AGENT_PLATFORM_MASTER_KEY=<key> DATABASE_URL= agent-platformd.exe`, then read
  the child's port out of its log.
- **A fresh database per target, not per pair.** Run Rust then Python against
  one database and you get a false divergence:
  `test_workflow_crud_and_validation` asserts `len(workflows) == 1`, so the
  second run sees the first run's rows and fails alone (`assert 19 == 1`). That
  is what a real behavioural difference looks like from the outside, and it is
  not one.

The cross-render scripts live in the session scratchpad rather than the repo, so
they are gone with the session; the shapes they compare are described per step
below. Anything per-request or per-process — `request_id`, `elapsed_ms`,
`model_list_age_sec`, `probed_at`, row ids, timestamps — has to be compared by
type, not value, and pydantic's `input`/`ctx` error detail is a known omission
(see the gaps list).

Ordered by what unblocks what:

1. ~~**`llm_proxy/`**~~ — **done**, all nine routes, **and the switch is thrown.**
   It was the thing blocking the rest: the todo agent routes, workflow
   `run`/`assist`, and moving
   [`local_llm.rs`](desktop/crates/app/src/local_llm.rs) into the daemon so a
   cloud deploy gets in-process inference too. The per-step notes are below.

   The cutover is two lines in [`upstream.rs`](desktop/crates/server/src/upstream.rs):
   the child's `LLM_ORCHESTRATOR_BASE_URL` now points at *us* (via
   `loopback_origin`, because `AGENT_PLATFORM_HOST` may be a bind address like
   `0.0.0.0`, which is not a destination), and it starts with
   `AGENT_PLATFORM_V1_ROUTER=0`. `main.py` mounts its `/v1` router only when that
   is not `0`, so **Python running alone still serves `/v1`** — the runbook's
   uvicorn line, and the five pytest files that exercise it, are unaffected.
   Unmounting outright would have failed those suites; the env gate is the same
   shape as the existing `AGENT_PLATFORM_WORKFLOW_SCHEDULER=0`.

   Everything internal — planner, subagents, coder, assistant, `tool_handlers`'
   `chat_completions` tool — reaches the proxy through `llm_proxy_env`, so all of
   it followed the one variable with no Python change. One loopback hop is the
   price. Two consequences: a Rust `/v1` regression now takes the orchestrator
   with it, and the child is up (and may already be running `startup_recovery`)
   for a few milliseconds before we bind — `llm_client` retries connect failures
   three times with backoff, which covers that window.
2. ~~**Close the todos/workflows split**~~ — **done. Workflows is whole and
   `todo_items` has a single writer.** `workflows/assist`, `todos agent/chat` and
   `spawn-process` all shipped, so Rust owns every write to `workflows`,
   `workflow_runs` and `todo_items`. One todo route is still Python —
   `agent/step`, which needs the whole `action_orchestrator` — and it only
   *appends* to `todo_item_events`, so it is not a hazard. Scoped below.
3. ~~**processes / orchestrator / action_orchestrator**~~ — **done**, all seven
   sub-steps: [`processes.rs`](desktop/crates/server/src/processes.rs) (eleven
   routes plus `projects/{id}/processes` and the SSE stream),
   [`executor.rs`](desktop/crates/server/src/executor.rs) (the wave loop,
   sub-DAG expansion, startup recovery),
   [`dag_schema.rs`](desktop/crates/server/src/dag_schema.rs), and
   `agent/step` over [`action_orchestrator.rs`](desktop/crates/server/src/action_orchestrator.rs).
   FastAPI's `BackgroundTasks` became four fire-and-forget `tokio::spawn` entry
   points; `startup_recovery` got its equivalent and the child is started with
   `AGENT_PLATFORM_RESUME_ON_STARTUP=0`, in the same commit, or both servers
   requeue every stranded process. The scoping first corrected three things this
   list had wrong: the closure was ~2.9k LOC rather than ~6.3k, four of the seven
   sub-steps needed no tokio task at all, and the two-writer hazard is not on
   `process` — it is on the API-token counters, and it is now live (below).
4. **assistant, chat, coder** — largest and highest-churn, **in progress**.
   Scoped below in two notes, and the scoping moved four things: the assistant is
   *not* an `api_tokens` writer (its call site passes a literal `None`), so the
   counters close on coder **and playground** — which is what made playground a
   step of its own (4½), where it was deleted rather than ported;
   `todo_items` still has a Python writer here, which the step-3 note denied;
   and coder holds the first in-process state shared between two HTTP requests,
   which fixes its granularity at five-routes-or-nothing.

   **Done:** the tokenizer decision (below), `POST /api/v1/chat` (`chat.rs`), the
   shared helpers `chat_usage.rs` + `chat_thread_title.rs` that both halves
   and playground need, and — as of 2026-08-07 — **the whole assistant half**:
   the reads, the profile route (closing the `assistant_domain_profiles`
   two-writer), threads + the LLM turns, apply/reviews (closing `todo_items`),
   `chat/retry` + `chat/submit-form`, and `/assistant/reset`. **Held:**
   `POST /chat/threads`, proxied by choice — the three dead Flow-UI GETs are
   deleted (see step 1's note).

   **Done, 2026-08-07: coder too, so this step is closed.** All ten routes,
   the agent loop, both executors and the delegated tool park — details in the
   coder scope note below.
4½. ~~**playground**~~ — **deleted, 2026-08-07**, not ported. 699 LOC and six
   routes with no caller anywhere: not in `desktop/crates/`, not in the deleted
   `web/`, nowhere but its own test file. Porting it would have written a second
   Rust copy of `coder/service.py`'s shape to serve a UI that does not exist.
   Gone: `app/playground/` and `app/tests/test_playground_api.py`, the
   `main.py` mount, the `conftest.py` table registration, and the mention in the
   `chat:write` scope description. `test_chat_thread_title.py` had borrowed
   `PlaygroundChatThread` as a generic row and now borrows `CoderChatThread` —
   that module is table-agnostic.

   **The table stays.** `playground_chat_threads` and its migration
   (`x3y4z5a6b7c8`) are left alone: dropping it would delete existing rows on
   upgrade, and an unused table costs nothing. Rewriting the historical
   migration would break every database that has already run it.

   **This closes the `api_tokens` two-writer hazard step 3 opened**, and it is
   worth being explicit about why, because the reason is not "playground was the
   last caller". Python still *imports* `record_api_token_usage` in four places,
   and every one is now unreachable or inert: `coder/routes.py` (Rust owns all
   ten paths), the three DAG services (Rust owns every process route and the
   executor), and `assistant/routes.py:185`, which passes a literal `None` and
   short-circuits. Playground was the last **reachable** writer.
5. ~~**`system_routes` last.**~~ — **scoped 2026-08-07 and deliberately not
   ported.** The sequencing claim was right (it could only move after the
   others) but it was never a proof that it *should*. Scoping it found three
   things:

   - **Every field it returns is already correct through the proxy**, and one of
     them is correct *by design*: `listening_on` reads
     `AGENT_PLATFORM_PUBLIC_HOST`/`_PORT`, which
     [`upstream.rs:122`](desktop/crates/server/src/upstream.rs) passes to the
     child **precisely so this endpoint reports the public pair rather than the
     child's ephemeral one**. Porting rips out an indirection that exists for
     this route and gains nothing. `uptime_seconds`, the readiness DB ping, the
     `process` counts and the `llm_proxy` provider check all read the same
     database and the same config files either side.
   - **Two fields cannot be produced in Rust at all.** `python` is
     `sys.version.split()[0]` and `platform` is `platform.platform()` — a
     Python-formatted host string (`Windows-11-10.0.26200-SP0`). Both are
     **required** fields on `client/src/types.rs::SystemStatus` and both are
     rendered on the Status screen (`screen.rs:726-727`), so they can be
     neither dropped nor faked. Answering them would mean asking the child for
     them, which is a proxy with extra steps.
   - **Porting `/system/logs` would lose information.** It serves Python's
     `RingBufferHandler`, and the only request log in the system is Python's
     structlog. Rust has no ring and does no request logging, so a Rust
     `/system/logs` returns *different lines* — a redesign, not a port. The
     desktop already has the strictly better source: the daemon's stdout, which
     the child inherits (`upstream.rs:179-181`), captured by the shell, and
     which covers startup and crashes that this endpoint cannot.

   Neither route is a hazard: `/system/status` only reads `process`, and
   `/system/logs` touches no table. Nothing is blocked behind them. **They stay
   proxied**, and the integration test's proxy-passthrough probe can keep using
   `/api/v1/system/logs?limit=5` (see step 3's note) — that choice is now
   permanent rather than temporary.

   Revisit only if Python is actually being retired, at which point `python` and
   `platform` stop having a referent and the endpoint is redefined rather than
   ported.
6. ~~**`api_tokens`**~~ — **done 2026-08-07**,
   [`api_tokens.rs`](desktop/crates/server/src/api_tokens.rs), all eight routes,
   **and it closed the oldest split in the migration**: `auth.rs` has read
   `api_tokens` on every authenticated request since day one while Python owned
   every write.

   **`last_used_at` now advances again, and it was a real defect.** Rust never
   wrote the column and Python only writes it for requests that reach Python —
   so from the first migrated domain onward, a token whose traffic Rust answers
   stopped advancing it *at all*. A coder-only or processes-only token read as
   never used in `GET /api-tokens`, which is precisely the signal an operator
   revokes on. The `ponytail:` note in `auth.rs` had asked whichever domain got
   here first to add the 60s-throttled update; [`auth::touch_last_used`] is it,
   and a failure there is logged and swallowed rather than failing a valid
   caller's request over bookkeeping.

   **The cross-render found five differences, all in validation, all fixed.**
   The domain's happy paths were right first time; its 4xx shapes were not:

   - **A bare `/api-tokens` is a `307`, not an answer.** The router's own path
     is `/`, and FastAPI redirects the unslashed form onto it. Registering both
     spellings in axum answered `200` where Python answers a redirect — so the
     bare form is now *left to fall through to the proxy*, which returns
     Python's redirect verbatim. Fourth occurrence of the path-shape trap, and
     the first where the fix was to register **less**.
   - **`Option<Json<T>>` is not "an optional body".** axum rejects an empty body
     that carries `Content-Type: application/json` — which is what an
     argument-less `POST` from most clients looks like — with a **plain-text
     400**, where Python answers the 422 envelope. The handlers take raw
     `Bytes` and parse them, which also gets `json_invalid` (with
     `JSONDecodeError.pos` as a byte offset) and `model_attributes_type` right.
     **The same latent bug was in every other domain that takes
     `Option<Json<…>>`** — swept 2026-08-07: `chat.rs`, `teams.rs`,
     `projects.rs`, `workflows.rs`, `processes.rs`, `assistant.rs`, `todos.rs`
     and `coder.rs` (30 handlers) all now take `Bytes` and go through two new
     shared helpers in `wire.rs` — `parse_body_typed` (required body) and
     `parse_body_or_default` (an empty body means the same as no
     `Content-Type` at all, i.e. defaults). `coder.rs`'s `require_body` moved
     onto the same two rather than keeping its own copy. 213 `cargo test`
     cases plus the 5 integration tests still pass; no route's required-vs-
     defaulted semantics changed, only the empty-body-with-json-header crash.
   - **Pydantic reports one failure per field**: a non-string `name` is
     `string_type` alone, not `string_type` *and* `string_too_short`.
   - **A `loc` index is an integer**, not a string — `["body", "scopes", 1]`.
     `ApiError::field_error_at` only builds string segments, so that entry is
     assembled directly.
   - **`expires_at` accepts a unix timestamp.** A JSON number, *and any numeric
     string shorter than ten characters*, is seconds since the epoch — so
     `"12345"` stores `1970-01-01 03:25:45` rather than failing. And speedate's
     failure messages have a boundary that is not where it looks: **anything
     under ten characters is "input is too short"** whatever it contains,
     because it was already tried as a timestamp. Both read off a `python -c`
     table against the real validator, not reasoned about.

   **Run:** failure sets **17 of 26 on both, identical, none unique to either
   side** across `test_api_tokens_api.py` + `test_workspace_tenancy.py` (this
   suite hardcodes `test-master-key`, the same trap coder's `test-key` set — so
   both servers must be started with that literal). Cross-render: **45
   comparisons, all matching** — the full lifecycle, every 4xx above, a
   workspace token being refused all three management routes, and `last_used_at`
   going from null to set after one authenticated request.

7. ~~**`llm_proxy` admin (`/api/v1/llm-proxy/*`)**~~ — **done 2026-08-07**,
   [`llm_admin.rs`](desktop/crates/server/src/llm_admin.rs), **fourteen of its
   fifteen routes**, and it closes the live coupling this plan flagged: Python
   owned every write to the `.env` and `config.yaml` that `llm.rs` reads on
   every request.

   The catalog half was almost free — `provider_catalog.rs` already held the
   discovery, so the admin body is a second assembly over it
   (`provider_catalog::build_admin`) rather than a second set of fetchers.

   **`POST /config-yaml` stays with Python, by choice.** Its 400 body is
   `jsonschema`'s own `ValidationError.message` from a Draft 2020-12 validator,
   against a schema file found by a three-candidate path search. A Rust
   validator answers a *different sentence* for the same bad config, which is a
   redesign rather than a port — so the POST is handed to `proxy::forward` on
   the same path the GET is served from, the shape `POST /chat/threads` already
   uses. Registering only the GET would have answered 405, the trap this
   migration has now hit four times.

   **The self-calls keep Python's literal default.** `ORCHESTRATOR_INTERNAL_URL`
   or `http://127.0.0.1:18410` — *not* the port this process is bound to. That
   is what makes the two comparable when the harness runs them on a spare port,
   and it is the same place Python probes.

   **The cross-render found four differences, all fixed:**

   - **`Path.read_text` translates newlines and `fs::read_to_string` does not.**
     `GET /config-yaml` returned CRLF where Python returned LF — on a file
     Python itself had written, since `write_text` translates on the way out
     too. Both directions are handled now (`read_text_universal`, and the
     `.env` writer uses `os.linesep`).
   - **pydantic's lax mode coerces booleans.** `{"thinking": "yes"}` is a
     **200** there and was a 422 here. `"on"`, `"1"`, and `0`/`1` as int or
     float are all booleans; `"maybe"` and `2` are `bool_parsing`; `2.5` and
     `null` are `bool_type`. Read off a `python -c` table against the real
     validator, not reasoned about — the same method the `expires_at` boundary
     needed. `wire::lax_bool`/`lax_int` are shared, since this is not the last
     domain that will take a bool.
   - **A union reports one failure per member.** A bad `tool_choice`
     (`str | dict`) is *two* entries, at `["body","tool_choice","str"]` and
     `["body","tool_choice","dict[str,any]"]`.
   - **A non-`Optional` field with a default still rejects an explicit `null`.**
     `{"message": null}` is a `string_type` failure, not the default.

   **Run:** 22 of 33 comparisons matched exactly; every one of the eleven
   remaining is the known `input`/`ctx` omission in `ApiError::validation`
   (documented in `error.rs`), and nothing else.

8. ~~**`action_orchestrator` routes**~~ — **done 2026-08-07**, all eleven
   (`/action-sets` × 7, `/sessions` × 3, `/sessions/{id}/…` and `/decide`),
   appended to
   [`action_orchestrator.rs`](desktop/crates/server/src/action_orchestrator.rs)
   next to the engine step 3 had already lifted. `decide_actions` grew the
   `history` argument it always had in Python — the "Previous actions and
   results" block — which only the session routes pass.

   **It found a live 500 in Python, and the fix is in this commit.**
   `def get_session(...)` (the `GET /sessions/{id}` handler) **shadowed the
   `database.get_session` dependency imported at the top of the module**, so
   every route declared below it — `/steps`, `/results`, `/complete`,
   `/history` — was handed a *session response dict* where it expected a DB
   session and 500'd on the first attribute access, and `/decide` demanded a
   `session_id` **query** parameter that is not part of its contract. Renamed
   to `get_session_detail`, with a comment saying why. This is the second defect
   a cross-render has surfaced rather than a test (after `last_used_at`), and
   the first that was a hard 500 on four public routes.

   Ported to the *intended* behaviour, not the shadowed one — the same call the
   `last_used_at` fix made. Two smaller shapes worth keeping in mind: the
   namespace is `ws:{workspace_id}` for a workspace token and the
   `X-Agent-Platform-Client` header for the master key, and an **unowned** set
   is public to every caller rather than hidden from all of them.

   **Run:** 43 of 56 comparisons matched, including the full session lifecycle
   through a real model turn; all thirteen remaining are the `input`/`ctx`
   omission.

9. **`model_ops` — thirteen of seventeen routes done 2026-08-07**,
   [`model_ops.rs`](desktop/crates/server/src/model_ops.rs): the Ollama surface
   (list, show, pull, copy, create, delete, and the async job enqueue), the
   project scaffold with both multipart uploads — the first in this crate — and
   the registry.

   **The four pipeline routes stay with Python, and that is the line, not a
   deferral.** `POST /jobs`, `POST /operations/build`, `GET /jobs/{id}`,
   `/jobs/{id}/stream` and `/jobs/{id}/cancel` belong to `runner.py`, for two
   reasons that do not go away:

   - it runs the stages **inside the Python process** (`prepare` and `eval`
     import `model_ops.pipeline.*`, i.e. torch), or as a `python -c` child when
     `MODEL_OPS_GPU_SUBPROCESS` says so — and `eval` returns its dict through a
     *function return*, not through stdout, so there is nothing for another
     process to read;
   - `cancel_job` reads a module-level `_running` dict of live subprocesses, so
     **only the process that started a job can stop one**. A Rust `/cancel`
     would mark the row `cancelled` and leave the training child running.

   The reads next to them stay for the same reason: a job answered here and
   cancelled there is two servers disagreeing about one row. The jobs the
   *Ollama* routes enqueue are a different thing — no subprocess, no `_running`
   entry — and they run here, which the cross-render checks by rendering a
   Rust-written job row through Python's `GET /jobs/{id}`.

   **It found two more live Python defects, both fixed here.**
   `model_ops/ollama_client.py` called `post_with_retry(..., json=…)`, but that
   helper is `UpstreamClient.post`, whose keyword is `json_body` — a `TypeError`,
   i.e. **a 500 on every `GET /ollama/models/{name}` and every
   `DELETE /ollama/models/{name}`**. Third and fourth defects this method has
   surfaced, and again not caught by a test, because nothing tests these routes.

   Three shapes worth keeping:

   - **`{name:path}` is declared before `pull`/`copy`/`create`**, so a *GET* of
     `/ollama/models/pull` is "show the model called pull" in FastAPI. One axum
     wildcard route carries all four methods rather than four routes, because
     registering the static paths separately answers 405 there.
   - **Starlette's own 405 skips the app's exception handler**, so it is a bare
     `{"detail": "Method Not Allowed"}` and not the error envelope.
   - **pydantic stops at the first failed string constraint**: `""` for a
     project name is `string_too_short` alone, never also the pattern mismatch.

   **Run:** 29 of 38 comparisons matched, including both uploads, the resulting
   data-directory trees compared file by file, and the async job rows; the nine
   others are the `input`/`ctx` omission.

   What is left of this router, for whoever takes it:

   - the four pipeline routes above, which only move if the runner moves;
   - `create_build_job` → `link_process_to_job`, which writes
     `process.model_build_job_id` into a table Rust owns. That coupling is
     **still open**, and it stays open for as long as `POST /jobs` is Python's.

   Revisit if the training pipeline is ever run as a service of its own rather
   than as imports inside the API process — that, not the routes, is the
   blocker.

10. ~~**The workspace/document stack**~~ — **done 2026-08-07**, and it ended
    split exactly where the scoping said it would.
    [`workspaces.rs`](desktop/crates/server/src/workspaces.rs) takes all six
    tenant routes; [`workspace_files.rs`](desktop/crates/server/src/workspace_files.rs)
    takes seven of the eight file routes, on **both** prefixes (`/workspace/*`
    and `/files/*` are the same handlers mounted twice in Python, and the same
    handlers registered twice here).

    **Two request shapes stay with Python, both PyMuPDF.**
    `document_service.py` extracts PDF text through `fitz`, which has no Rust
    equivalent:

    - `POST /upload` is not registered at all. It is multipart, and whether it
      needs extraction depends on the filename *inside* the body — which cannot
      be read without consuming a body that would then have to be replayed to
      the proxy.
    - `GET /file` for a `.pdf` path is handed to `proxy::forward` whole,
      **including** the case where the derived markdown already exists: a miss
      there re-ingests, and re-ingesting is extraction.

    That is the same line `todos agent/step` already draws, and the traversal
    guard that note said should not be written twice is now written once, here.

    **The archive cascade came with it**, which is the part that mattered:
    `DELETE /workspaces/{id}` revokes every non-revoked token in the workspace,
    deletes its team templates (detaching any process that pointed at one) and
    stamps the row. `auth.rs` reads `api_tokens` on every request, so Python
    owning that write was the last live token-table split.

    **A third Python defect, fixed here.** `workspace_info` called
    `normalize_relative_path` *outside* its `try`, so
    `GET /workspace/info?path=../escape` raised `WorkspaceError` unhandled — a
    **500** where every other route in that file answers 400.

    Two shapes worth keeping:

    - **`Path.resolve()` is lexical here, not `canonicalize`.** On Windows
      canonicalising returns a `\\?\`-prefixed path, and `absolute_path` is a
      response body field a user pastes into Explorer. Every segment has already
      been through the `..` guard, so the two agree.
    - **`str(OSError)` reaches the client verbatim** (`directory_not_empty`
      carries it), and Python renders the filename through `repr()` — so a
      Windows path arrives with its backslashes doubled. Rust's own rendering is
      a different sentence; `os_error_text` rebuilds Python's.

    **Run:** 46 of 59 comparisons matched, including the archive cascade, the
    `.pdf` handoff on both prefixes, and the two sandbox trees compared file by
    file; the thirteen others are the `input`/`ctx` omission.

#### Closing the todos/workflows split — scope (step 2)

**The whole-row-flush hazard is narrower than "the routes left with Python", and
it includes something that is not a route at all.** Of the five todo routes still
proxied, `agent/step` and `agent/chat` only *append* to `todo_item_events`;
an append cannot clobber a concurrent Rust write. Only `agent/apply`,
`planning-form/submit` and `spawn-process` mutate `todo_items`.

On the workflows side the routes are not the problem either. `{id}/run` inserts
and updates its own `workflow_runs` row, which Rust never touches.

The doc's "SQLAlchemy flushes whole rows" is true of the CRUD-style handlers —
they assign every field from the request payload, and SQLAlchemy marks an
attribute dirty on assignment without comparing, so the UPDATE covers columns
that did not actually change. It is *not* true of
`workflows/engine.py::_run_due_workflows`, which assigns only `next_run_at` and
therefore emits a single-column UPDATE. The scheduler still has to move with
`{id}/run` — two servers must not both be firing the same workflow — but it is
not silently reverting edits today, and the narrower failure it can cause is a
tick computed from an `interval_seconds` that Rust changed after the read.

| Left with Python | Writes | Blocked by |
|---|---|---|
| `todos items/{id}/agent/step` | events only | `action_orchestrator` (`decide_actions`, `list_actions`) |
| ~~`agent/apply`~~ (with `merge_profile`), ~~`planning-form/submit`~~, ~~`{id}/run`~~, ~~the scheduler~~, ~~`agent/chat`~~, ~~`workflows {id}/assist`~~, ~~`spawn-process`~~ | shipped | — |

Ordered by what closes the hazard soonest per line of work:

1. ~~**`{id}/run` with the scheduler on top.**~~ — shipped:
   [`workflow_engine.rs`](desktop/crates/server/src/workflow_engine.rs). Rust owns
   `workflow_runs` now, so the workflows side of the split is closed except for
   `assist`. The cutover is in `upstream.rs`, which starts the Python child with
   `AGENT_PLATFORM_WORKFLOW_SCHEDULER=0`; the Rust loop also declines to start
   when the daemon is *attached* to an upstream it did not spawn
   (`AGENT_PLATFORM_UPSTREAM`), because that server's scheduler is already
   running and cannot be switched off from here.

   Three deliberate divergences, all found by cross-rendering the same workflow
   through both engines:

   - **A non-string header value works here and 500s in Python.** A header whose
     value is exactly one template resolves to the referenced *type* — that is
     the documented rule — and httpx then rejects an `int` header, so
     `{"X-Prev": "{{steps.ping.output.status}}"}` crashes the run. Rust
     stringifies at send time, which is what the author meant. Not comparable, so
     the shared test uses an embedded template instead.
   - **A missing `url` or a non-numeric `timeout_seconds` fails the step** rather
     than raising out of the engine as an unhandled 500. The run then records
     which step and why, which the crash does not.
   - **Skipped steps are listed in declaration order.** Python builds them from a
     set difference and so has no order at all.

   Matched on purpose, because it is user-visible: a failed `http` step renders
   the response body with `json.dumps`'s separators (`", "`, `": "`), not
   serde's compact ones. That needed a 20-line `serde_json::ser::Formatter`, and
   the cross-render is what caught it.
2. **`agent/apply` + `planning-form/submit`** — closes the `todo_items` hazard.
   **`planning-form/submit` is shipped** (`todos.rs`): it edits one metadata key
   and appends one event, so it keeps the same one-column discipline the rest of
   Rust's todo writes follow. Cross-rendered against Python on the happy path, an
   out-of-range index, a missing `form_index`, a negative one and a non-object
   `answers`; the only differences are the `input`/`ctx` fields of the validation
   envelope, which is the known gap listed below.

   **`agent/apply` is shipped too**, so `todo_items` now has one writer for
   everything except `spawn-process`. `merge_profile` came with it — one action
   of the seventeen, `store_user_profile`, writes `assistant_domain_profiles`,
   which is the assistant's table; leaving it proxied was not an option once the
   route itself moved. Cross-rendered over 24 cases, one per action plus every
   skip reason.

   Two things it does differently from the CRUD around it:

   - Only the columns an action actually touched are written, assembled into one
     `UPDATE`. Python assigns fields on the ORM object and flushes whatever it
     marked dirty — which is what made this route the last real hazard.
   - **An offset on a datetime is dropped, not applied.** Writing an aware
     datetime into SQLAlchemy's naive column keeps the wall clock and discards
     the tzinfo, so `09:00Z` lands as `09:00`. Converting to UTC first would move
     every scheduled item by the caller's offset. The cross-render caught this:
     Rust was storing `+00:00` and rendering `…09:00:00Z` where Python renders
     `…09:00:00`. **The item CRUD did have the same bug**, and it is fixed —
     see below.
3. ~~**`workflows {id}/assist`**~~ — shipped, in `workflows.rs` over
   [`llm::complete_internal`](desktop/crates/server/src/llm.rs) (see step 4).
   The `SYSTEM_PROMPT` is byte-identical to Python's, checked by diffing the Rust
   const against the imported Python string rather than by eye. Three knowing
   approximations, all in text a user only sees when something already went
   wrong: a non-string `reply` reads as absent here where Python `str()`s it; the
   discarded-steps parenthetical carries pydantic's *sentence* but not its
   `[type=…, input_value=…]` envelope; and `json.dumps(steps, indent=2)` in the
   prompt escapes non-ASCII and keeps the caller's key order, where serde does
   neither. Prompt text and error text — nothing on the wire.
4. ~~**`agent/chat`**~~ — shipped, with
   [`context_budget.rs`](desktop/crates/server/src/context_budget.rs): the
   char-based path only, as `usage.rs` already does, because the tiktoken branch
   exists to truncate to an exact token count and nothing asserted those numbers.
   **That reasoning expired in step 4 and the tokenizer is now real** — see the
   note below.

   The load-bearing detail is not the budget, it is the prompt:
   `agent_bridge.agent_chat` interpolates the context **dict** into an f-string,
   so what the model reads is Python's `str(dict)` — single quotes, `None`,
   `True`, insertion order. Rendering JSON there would have been a different
   prompt on every turn, so `py_repr`/`py_dict`/`py_str` reproduce the repr,
   the way `workflow_engine.rs` reproduces `json.dumps`'s separators. Nested
   values out of a JSON column still sort alphabetically (serde's map is a
   `BTreeMap`) where Python keeps write order, and that is noted at the function.

   Both of these call
   [`llm::complete_internal`](desktop/crates/server/src/llm.rs) — the
   `/v1/chat/completions` handler's own resolution, coercion, capability guard,
   retry policy and usage normalisation, minus the socket. Python takes a
   loopback hop to reach the same code; there is no reason to open a second one
   from inside the process that serves it. Its errors carry the status the public
   route would have answered with, so each caller can map them the way its Python
   counterpart mapped an HTTP response (`assist` → `502 Assistant unavailable: …`,
   `agent/chat` → `502 LLM proxy returned HTTP {status}`).
5. **`agent/step` and `spawn-process` last** — and step 3's scoping found both of
   those "blocked by" claims to be wrong. ~~`spawn-process`~~ **shipped**: it
   inserts a `pending` process and starts **nothing** (`process_spawn.py` has no
   `BackgroundTasks`, no `create_task`, no `DAGExecutor` — its own response tells
   the caller to `POST /processes/{id}/sync`), so it needed the `Process` insert
   and the team snapshot, not the executor.

   The snapshot was the whole job. `build_process_team_snapshot` is stored as
   `json.dumps(model_dump(), separators=(",",":"))`, so it is not enough to be
   compact — **`ensure_ascii` defaults to true**, which serde does not do, and
   pydantic's key order is field-declaration order where `serde_json::Map`
   sorts. Derived `Serialize` structs give the order; an `EnsureAscii` formatter
   (~15 lines, patterned on `workflow_engine.rs`'s `PythonJson`) gives the
   escapes, emitting `\uXXXX` per UTF-16 unit so an astral char comes out as a
   surrogate pair like Python's. The pinned expectations were generated by
   running `team_schema.py` under `python -c`, and then re-extracted from the
   `.rs` file and re-diffed against a fresh Python run.

   Two things kept verbatim rather than fixed, both flagged in code: a template
   is fetched with a bare `session.get` in Python, so **a workspace token can
   snapshot another workspace's template** — narrowing it would 404 requests
   Python answers, so it is a separate decision; and a corrupt `roster_json`
   500s in Python (pydantic) where Rust's `parse_roster` only deserializes,
   which is pre-existing in the teams read path.

   `agent/step` touches no process table at all and could have shipped here too;
   what it actually needs is `list_actions` plus `decide_actions`, and the one
   thing that does not port is the `document_paths` branch, which reads PDFs
   through PyMuPDF. It is last in step 3's order below.

#### processes / orchestrator — scope (step 3)

**The one-liner above was right about the mechanism and wrong about the
blocking.** `spawn-process` does not need the executor, `agent/step` does not
need the processes domain at all, and the two-writer hazard this step creates is
not on `process` — it is on `api_tokens`.

##### The surface — fourteen routes

Every handler in `process_routes.py` is a **sync `def`** (FastAPI runs them in
the threadpool); every scheduling route takes `BackgroundTasks`. The router is
mounted with `_api_deps` (`main.py:92`), so `require_valid_token` runs first and
each handler then calls `require_scope` itself — the shape `auth::require_token`
+ `Principal::require_scope` already covers. `assert_token_project_access` is
called **without a session** on the per-process routes, so it opens its own
(`api_tokens/auth.py:178-208`); `projects::assert_access` (`projects.rs:105`) is
the equivalent, and `project_id is None` + a workspace token is a 404.

| # | Method / path | Scope | Writes | Schedules |
|---|---|---|---|---|
| 1 | `GET /processes` (`:78`) | `process:read` | — | — |
| 2 | `POST /processes` (`:117`) | `process:write` | **`process`** | `plan` |
| 3 | `GET /processes/{id}` (`:195`) | `process:read` | — | — |
| 4 | `GET /processes/{id}/events` (`:209`) | `process:read` | — | — |
| 5 | `POST /processes/{id}/approve` (`:235`) | `process:write` | **`tasknode`**, **`process`** | `execute_dag` |
| 6 | `POST /processes/{id}/tasks/{tid}/review` (`:276`) | `process:write` | **`tasknode`**, **`process`**, **`eventlog`** | `execute_dag` / `expand_after_review_approval_and_continue` |
| 7 | `POST /processes/{id}/cancel` (`:345`) | `process:write` | **`process.status`** | — |
| 8 | `POST /processes/{id}/sync` (`:373`) | `process:write` | **`process`**, **`tasknode`**, **`eventlog`** | `plan` / `execute_dag` |
| 9 | `POST /processes/{id}/retry` (`:524`) | `process:write` | same | `plan` / `execute_dag` |
| 10 | `POST /processes/{id}/tasks/{tid}/retry` (`:596`) | `process:write` | same | `execute_dag` |
| 11 | `GET /processes/{id}/stream` (`:649`) | `process:read` | — | — |
| 12 | `GET /projects/{id}/processes` (`projects_routes.py:245`) | project access only, **no `process:read`** | — | — |
| 13 | `POST /todos/items/{id}/spawn-process` (`todos/routes.py:311`) | `todos:write` | **`process`**, **`todo_items.linked_process_id`**, **`todo_item_events`** | **nothing** |
| 14 | `POST /todos/items/{id}/agent/step` (`todos/routes.py:225`) | `todos:write` | **`todo_item_events`** (append) | — |

Two out-of-domain readers stay Python: `system_routes.py:92` counts processes by
status (that is step 5's fan-in), and `model_ops/routes.py:398` takes a
`process_id` on a build job.

Route rules a port must carry: #1 **400s unless one of
`project_id`/`client_id`/`unassigned_only` is given** (`:100`), and a workspace
token must pass `project_id` and may never use `unassigned_only` (`:88-97`); #2
adds a template visibility check (`:151-156`) and the
`X-Agent-Platform-Client`/`client_id` merge that
`AGENT_PLATFORM_REQUIRE_CLIENT_ID` makes mandatory; every per-process route runs
`assert_process_client_access`, which **404s a mismatched client header** rather
than 403ing (`client_scope.py:26-33`). #12 is the odd one out — project access,
no `process:read`.

**The desktop calls ten of the eleven process routes plus the stream**
(`client.rs:132-218`) and **neither #13 nor #14** — so the two blocked todo
routes have no UI to check them against and are cross-render-only.

##### The tables — the hazard is not where it was assumed

This domain writes `process`, `tasknode`, `eventlog`, and — through
`record_api_token_usage` — **`api_tokens` and `api_token_usage_daily`**
(`services/{planner_runtime,task_result,subdag}_service.py`).

`process` already has three writers and none of them is new here:
`projects.rs:332` (`project_id = NULL` on project delete), `teams.rs:643`
(`team_template_id = NULL` on template delete) and `model_ops/service.py:219`
(`model_build_job_id`, Python, outside this domain, staying). Each assigns one
attribute, so the UPDATEs cover one column and cannot revert each other.

**The two-writer this step creates is on the token counters.**
`record_api_token_usage` is a read-modify-write (`usage_tracking.py:33-45`:
`row.request_count += 1`). Today only Python does it — from the three DAG
services *and* from `assistant/routes.py:185`, `coder/routes.py:55,152`,
`playground/routes.py:48,133`, which stay Python until step 4. Once the executor
is Rust both processes increment the same row, and Rust's atomic
`SET x = x + 1` does not save it from Python's read-then-write. **Affected rows:
the `api_tokens` row and today's `api_token_usage_daily` row of a project-scoped
token whose process runs while the same token drives coder, assistant or
playground.** Master-key callers are exempt (`token_id is None` short-circuits,
`usage_tracking.py:20`), and `spawn_process_for_item` never sets `token_id`
(`process_spawn.py:61-67`), so todo-spawned processes are always in the safe case.

`tasknode` and `eventlog` get their first Rust writer here and Python keeps
none — no split. Porting `spawn-process` removes the last writer of `todo_items`
*in this domain*.

**Correction, from the step-4 scoping:** it does **not** remove the last Python
writer of `todo_items` outright, as this note first claimed.
`assistant/services/board_action_apply.py:14` imports `create_item` and
`update_item` from `todos.services.board_service` and calls them from three
assistant routes, and `/assistant/reset` deletes rows outright. Those closed in
step 4's sub-step 7 and sub-step 9 respectively — the apply routes first, then
reset — so `todo_items` had a Python writer until 2026-08-07.

##### The concurrency

Six background entry points, all `asyncio`, none surviving the process:

| Started by | Coroutine | On death |
|---|---|---|
| routes 2, 8, 9 | `DAGExecutor.plan` (`orchestrator.py:278`) — one planner call under an optional `AGENT_PLATFORM_PLAN_TIMEOUT_SECONDS`, writes `dag_json` + `approval_required`, and falls straight into `execute_dag` when auto-approving | row stranded `planning` |
| routes 5, 6, 8, 9, 10 | `DAGExecutor.execute_dag` (`:543`) — the wave loop | row stranded `running` |
| route 6 | `expand_after_review_approval_and_continue` (`:636`) | as above |
| inside `execute_dag` | `asyncio.gather` over `execute_task` (`:634`) | task stranded `running` |
| inside `execute_task` | `_maybe_expand_subdag_after_success` (`:447`), merging under an `asyncio.Lock` (`:167`) | expansion lost, task already `completed` |
| lifespan (`main.py:56-58`) | `startup_recovery` | — |

`execute_dag` is **the one that needs a different shape in tokio**: per wave it
re-reads the process, runs `sync_review_assignments`, checks
cancelled/failed/`AGENT_PLATFORM_RUN_MAX_SECONDS`, computes ready ids (FIFO by
`TaskNode.id`, capped by `AGENT_PLATFORM_DAG_MAX_CONCURRENT_TASKS`) and gathers
the wave — a `JoinSet` per wave with the same ordering rule. Cancellation and
pause are **DB-mediated**, not in-process: `cancel` and `sync` write a status and
the loop notices at the top of the next wave. That is what makes a partial
migration survivable.

`startup_recovery` (154 LOC) is route 8's decision table applied once at boot to
`pending`/`planning`/`approved`/`running`, leaving human gates and terminal rows
alone. **Its off-switch already exists** — `AGENT_PLATFORM_RESUME_ON_STARTUP`
(`startup_recovery.py:37-39`) — so the cutover is the workflow-scheduler shape
verbatim: set it to `0` in `upstream.rs` next to
`AGENT_PLATFORM_WORKFLOW_SCHEDULER=0`, and have the Rust side decline when
*attached* rather than parent (`workflow_engine.rs:393-405` is the template). No
Python change needed. Two servers both recovering means every interrupted process
gets planned twice.

The tool-calling path (`_invoke_task_llm`, `:248`) is **dead by default**:
`AGENT_PLATFORM_TOOLS_ENABLED` is unset and `is_allowed` returns false on an
empty allowlist (`tools_policy.py:31,56-61`). Porting `load_policy` (three env
reads) and refusing to start when tools are enabled keeps `tool_handlers.py`'s
782 LOC out of the closure entirely.

##### The SSE stream

`GET /processes/{id}/stream` (`:649`) authorises **before** returning the
response — own session, 404 on a missing process, client-header and project
checks (`:658-663`). Then every **0.8s** (`:700`) it selects
`EventLog WHERE process_id = ? AND id > last ORDER BY id ASC` (no limit) and
emits one data-only frame per row: `{task_id, type, content, timestamp}`.
`created_at` is naive, so **no `Z`** — the trap the todos port already hit;
`wire::iso_from_sql` renders it right. `sse_starlette` adds `: ping` comments
every 15s.

Terminal rules, in order: process gone → `{"type":"error"}`, break; terminal
status → break, **emitting the `terminal` sentinel only if this pass drained no
rows** (`:691-694`); a human gate → always emit the sentinel, break. That middle
asymmetry is the caveat already documented at
[`sse.rs:355-358`](desktop/crates/client/src/sse.rs) — an already-terminal
process with a backlog replays it and closes with **no** sentinel, so the client
reconnects forever and consumers gate on polled status. **Port the bug, not the
intent**; the desktop is written around it.

What `sse.rs` depends on: data-only frames (`event:` always empty); `:` lines
dropped; blank-line frame terminator, CRLF tolerated; **a sentinel is told from a
log row purely by `task_id` *and* `timestamp` both being absent** (`:33-35`) — so
a log row whose type is `"error"` must keep both or the client ends the stream
early; `content` is a `Value`; any frame resets the reconnect counter; backoff
`min(30s, 500ms · 2^min(n-1,6))`.

##### The closure — ~2.9k LOC, not ~6.3k

Ported: `process_routes.py` 702, `orchestrator.py` 639, the twelve process
`services/` 640, `startup_recovery.py` 154, `dag_schema.py` 190, the planner and
sub-DAG halves of `llm_client.py` ~300, three process-facing functions of
`team_schema.py` ~75 (the rest — `stable_palette_color` and friends — **is
already in `teams.rs:199-260`**), the four `context_budget.py` functions
[`context_budget.rs`](desktop/crates/server/src/context_budget.rs) left out
precisely because orchestration was still Python ~56, `context_summarize.py` 72,
`process_approval.py` 32, and ~110 LOC of usage/cost/client-scope helpers.

Already in Rust and reusable: `sanitize_llm_model_alias` (`todos.rs`),
`normalize_usage` (`usage.rs`), `iso_from_sql` (`wire.rs`),
`projects::assert_access`, `Principal::require_scope`, `estimate_tokens`.

**`client/src/dag.rs` already validates a planner DAG in Rust — do not reuse it
as-is.** It was ported from the deleted web app and carries that UI's error
strings, while Python's land verbatim in the 400 body of `/approve` and `/retry`
(`Duplicate client_uuid: 'x'`, `DAG contains a cycle (cyclic dependencies)`, …).
The server crate also does not depend on the client crate today, so reuse costs a
new crate dependency on top of a message rewrite.

Stays Python and needs no duplicate: `tool_handlers.py` + `tools_policy.py` (off
by default), `llm_client.py`'s remaining ~370 LOC (**a Rust executor calls
`llm::complete_internal` directly** — it takes a `Map<String, Value>`, so
`tools`, `response_format` and `temperature` pass straight through, and only the
JSON-repair retry loop has to be re-implemented on top), `process_table_sqlite.py`
(Alembic DDL), and the workspace/document/PDF stack.

**Nothing in-process-imports the orchestrator from outside the domain.**
`DAGExecutor` has exactly two importers — `process_routes.py:24` and
`startup_recovery.py:50` — and both move with this step. That is the opposite of
`llm_proxy/`, where eight external modules forced Python to keep its copy: here
`orchestrator.py`, the twelve services, `process_approval.py`,
`startup_recovery.py` and `process_routes.py` can all be **deleted** from Python
once this lands. `dag_schema.py`, `team_schema.py`, `context_budget.py`,
`llm_client.py` and `chat_usage.py` have other callers and stay.

##### The order

Sub-steps 1–4 all land **before a single tokio task exists**, and all four are
**shipped**: [`processes.rs`](desktop/crates/server/src/processes.rs) plus
`spawn-process` in `todos.rs`.

1. ~~**The three reads + `projects/{id}/processes`**~~ (1, 3, 4, 12) — no writes,
   no tasks, no executor. Unblocked the thing this domain was named for.
2. ~~**The SSE stream**~~ (11) — the 0.8s tail, the naive timestamp, the 15s
   keep-alive and the sentinel asymmetry, ported as the bug it is.
3. ~~**`cancel`**~~ (7) — one column, `SET status = 'cancelled'`, and it does not
   touch `updated_at` on either side.
4. ~~**`spawn-process`**~~ (13) — see step 2's note above for the snapshot.

   What those four cost outside their own files: **`POST /api/v1/processes` had
   to be declared explicitly at `proxy::forward`**, because it shares its path
   with the GET Rust now owns and would otherwise answer 405 instead of falling
   through — ADR 0007's `workflows/assist` consequence, second occurrence. Every
   other Python-owned path in the domain sits under a segment Rust never
   registers, so the fallback still catches it. And the integration test's
   proxy-passthrough probe had to move off `/api/v1/processes?limit=5`, which it
   had been using precisely *because* it was proxied; it now reads
   `/api/v1/system/logs?limit=5`, which is genuinely still Python's.

   Three Python oddities matched on purpose and commented so nobody "fixes"
   them: `GET /projects/{id}/processes` checks project access but **not**
   `process:read`; a mismatched `X-Agent-Platform-Client` is a **404**, not a
   403; and a whitespace-only `client_id` satisfies the "must specify one of"
   guard and then filters on nothing, because Python's guard tests truthiness
   while its WHERE clause tests the stripped value.
5. ~~**The executor + `startup_recovery` + the env gate**~~ — shipped:
   [`executor.rs`](desktop/crates/server/src/executor.rs) and
   [`dag_schema.rs`](desktop/crates/server/src/dag_schema.rs).

   Four fire-and-forget entry points — `spawn_plan`, `spawn_execute_dag`,
   `spawn_expand_after_review`, `spawn_startup_recovery` — each wrapped in
   `catch_unwind`, so a bug **fails one process with a named reason** instead of
   stranding it at `planning` forever. The wave loop is a `JoinSet` per wave over
   ready ids in FIFO id order, and the decision is factored out as a pure
   `plan_wave(&DagSnapshot, cap) -> Wave` so the readiness, cap, deadlock and
   review-gate branches are testable without a database. **The review gate is
   checked before the deadlock branch**, which is what stops "B blocked behind an
   `awaiting_review` A" being reported as a deadlock.

   Cancellation and pause stay **DB-mediated** — no channel, no token, no shared
   flag. `/cancel` and `/sync` write a status and the loop reads it at the top of
   the next wave. Said in the module docs so nobody "improves" it.

   Three notes worth carrying: the merge lock is `futures::lock::Mutex`, because
   it is held across the expansion's LLM call and a `std` guard would make the
   future `!Send`; `dag_json` is dumped **three different ways** in Python
   (`ensure_ascii` true in two places, false in a third) and `dependencies_json`
   keeps `json.dumps`'s space, so one `PyJson { ensure_ascii }` formatter
   replaced the two half-formatters that had grown up in `workflow_engine.rs` and
   `todos.rs`; and the tools branch is refused loudly rather than silently
   ignored if `AGENT_PLATFORM_TOOLS_ENABLED` is ever set, which keeps
   `tool_handlers.py` out of the port.
6. ~~**The eight scheduling routes**~~ (2, 5, 6, 8, 9, 10) — shipped in
   `processes.rs`. Every one commits its own writes *before* scheduling, which is
   what `BackgroundTasks` does.

   **`sync` has nine outcomes, not the seven this note first claimed** — the
   count missed the `400` (approved with no DAG) and the unknown-status
   fallback. Its branch selection is a pure function over
   `{status, awaiting_review, has_dag}`, and two details only a cross-render
   would have caught: on the `running`-with-reviews branch the body reports the
   **post-write** status, because Python reads `proc.status` after mutating it;
   and the `running` branch recomputes `task_counts` *after* resetting stuck
   tasks, so the counts describe the state the caller is left in.
7. ~~**`agent/step`**~~ (14) — shipped, over
   [`action_orchestrator.rs`](desktop/crates/server/src/action_orchestrator.rs):
   `list_actions`, the tool-definition build, the prompts, and **both text
   fallbacks** for a model that answers in prose instead of calling the tool —
   which is most of why the port matters, since this screen mostly runs local
   models. The rest of that router (685 LOC of `/action-sets`, `/sessions`,
   `/decide`) stays proxied.

   Two things worth knowing. `decide_actions` **never fails**: Python's blanket
   `except` makes an unreachable proxy a `200` whose `thought` reads
   `Error during decision: …`, and that shape is preserved rather than turned
   into a 502. And the document branch is **handed to `proxy::forward` whole** —
   not stubbed. `merge_workspace_documents` is really the *workspace* domain
   (path normalisation, the traversal guard, `WorkspaceError` codes) with PyMuPDF
   underneath, so porting the text half would have put a second path-traversal
   guard in the tree for a domain that migrates in step 4 anyway, and still
   answered a PDF request worse than Python does. A request naming a document
   gets Python's exact answer; every other request gets Rust's. The body is
   parsed and validated before the handover, so a malformed one still gets this
   server's 422 without a round trip.

   The hint in this note was wrong on one point, and the port checked rather than
   believed it: `decide_actions` does `json.dumps(ctx, indent=2)`, so that path
   is genuinely JSON — `py_repr` is the right renderer for `agent/chat` and the
   wrong one here.

**What the usual method proves here, and what it does not.** 24 tests in this
domain run against a server — and **ten of them assert on the mocked
`DAGExecutor`** the HTTP fixture hands out (`conftest.py:110-116`). Those ten
fail identically against a live Python server and against Rust, so the failure
sets still match, but **a matching failure set proves nothing about whether any
work was scheduled**. ADR 0007 rule 4's warning is exactly this case.

What proves scheduling instead: after each of routes 2/5/6/8/9/10, poll
`GET /processes/{id}/events` on both servers and assert the same
`status_change` rows in the same order, and that the status leaves its pre-call
value within a bounded wait. The event log is the executor's own trace, so it is
the only observable that separates "scheduled" from "returned 200 and did
nothing".

**Run, 2026-08-06.** Failure sets: **18 of 27 fail, identically on both, none
unique to either side.** Cross-render of a 33-call sequence (reads, every 4xx,
then the state machine — sync, retry, cancel, re-cancel, terminal sync — from
identical database bytes): **24 identical, 8 differing only in pydantic's
`input`/`ctx`, 1 real.** The executor was then driven live: `/retry` answers
`approved`, the status is `running` within 0.5s, and the daemon log shows it
resolving a model and calling the proxy — so the scheduling half is confirmed by
observation, not by a mock.

The one real difference was a **task-id reuse bug in Rust**, now fixed.
`apply_validated_planner_to_process` reads as delete-then-insert, but
SQLAlchemy's unit of work flushes INSERTs *before* DELETEs — so Python numbers
the replacement rows while the old ones still exist, and Rust, deleting first,
emptied the table and let SQLite restart `rowid` at 1. Same twelve rows, ids
1-12 against 55-66, and a client still holding `/tasks/1/retry` would have
addressed a *different task* instead of 404ing. Rust now inserts first and
deletes `id <= old_max`. No test asserts a row id; only the cross-render saw it.

Two of the three "divergences" that run turned up were the **harness**, and both
are now written into the `prove-domain` skill so the next domain does not pay for
them again: each run needs its own fresh copy of the database (these suites
mutate, so the second run answers from what the first left — that alone
manufactured a convincing one-test difference), and the port must be checked free
before starting, because a leftover daemon keeps it, the new one exits after
spawning its child, and the suite then talks to a stale server on a different
database.

A further **51 tests across eleven files cannot run against a server at all** —
`test_dag_executor.py`, `test_dag_schema.py`, `test_startup_recovery.py`, the
seven per-service files, `test_dag_merge.py`, `test_subdag_service.py` — they
import Python objects directly. **The Rust executor inherits no test suite**;
those 51 were the specification to re-express as `cargo test`, and the
wave/deadlock/budget/review-gate/sub-DAG-cap branches have no other coverage.

**What that re-expression covers, and what it does not.** The port added 71
tests (55 → 126 across the crate). Covered, as pure functions over a snapshot:
wave readiness with insertion order ≠ id order, caps of 1/2/99, all four wave
outcomes including deadlock-by-cycle and deadlock-by-orphan, the review gate
beating both Complete and Deadlock, the run budget's env parsing (blank, junk,
zero, negative, and `inf` not panicking `Duration::from_secs_f64`), reviewer
picking with its 100/80/40 role-overlap scoring and lowest-uuid tie-break, every
sub-DAG refusal and both caps at their boundary, the whole startup-recovery
decision table, planner fallback-on-last-attempt-only, and all four
`dag_schema` contract messages character-exact.

**Still uncovered, and these are the ones to be careful around:** `execute_task`
end to end (the `running` flip, dependency-output gathering, the parent-subtask
preamble, both `apply_task_success` branches, `record_task_failure`) — it needs a
pool plus a fake LLM, and `complete_internal` wants a live upstream; the nine
per-service files' single `UPDATE`/`INSERT` statements, which need a DB fixture
this crate does not have; `recover_interrupted_processes`'s SQL side, as opposed
to its decision table; and the retry loops' token/cost accumulation across
discarded attempts. A DB fixture is the one thing that would close most of that
list at once.

Cross-render targets (ids and timestamps compared by type): a full
`GET /processes/{id}` including `team_snapshot_json` — `json.dumps` with
`(",",":")` separators over a pydantic dump, so key order is field-declaration
order (`team_schema.py:291-304`) — the canonical `dag_json` from
`apply_validated_planner_to_process`, the 400 bodies of `/approve` and `/retry`
for a malformed DAG, all nine `sync` outcomes' `{action, detail, task_counts}`,
and one full event-log replay through both streams. Four suites outside this
domain also drive `/api/v1/processes`, so they re-run as part of this step's
evidence.

#### `llm_proxy/` — scope (step 1)

**The surface is nine routes**, and they are mounted *without* `_api_deps`
(`main.py:80`): each one authenticates itself with `Depends(require_valid_token)`
and the two health routes take no auth at all. So these cannot ride the existing
`auth::require_token` layer, which guards exactly `/api/v1/*` — the Rust handlers
call `auth::resolve` themselves, and `/v1/health*` stays open.

| Route | Auth | Needs |
|-------|------|-------|
| `GET /v1/health` | open | provider config + the background catalog cache |
| `GET /v1/health/readiness` | open | `first_configured_provider` only |
| `GET /v1/models` | token | YAML aliases + live model fetches per provider |
| `GET /v1/catalog` | token | the above plus per-model capability probes |
| `GET /v1/capabilities` | token | modality matrix + BYOK discovery |
| `POST /v1/chat/completions` | token + `chat:write` | alias resolve, local-model coercion, capability guard, usage normalize, SSE passthrough |
| `POST /v1/embeddings` | token + `chat:write` | alias resolve, capability guard |
| `POST /v1/images/generations` | token + `chat:write` | image backend registry |
| `POST /v1/audio/speech` | token + `chat:write` | speech backend registry — the desktop's E.V. voice is the one client that calls a `/v1` route directly |

**It owns no tables and writes no usage rows.** There is not a single
`Session`/`models` import in the package; `record_api_token_usage` is called from
the coder, assistant and playground routes and the DAG services, never from a
`/v1` handler. Its entire state is four files under `CONFIG_DIR`, each read
through an mtime+size cache: `.env`, `config.yaml`, `orchestrator_ui.yaml` and
`model_capabilities.json`. Two processes reading those is fine, which is why the
admin writer (`/api/v1/llm-proxy/*`, `admin_routes.py`, 672 LOC) can stay Python —
it writes, Rust re-reads on the next mtime change. This domain has no
todos/workflows-style two-writer split.

**The closure is ~3.8k LOC, not 5.3k**: `admin_routes.py` and
`build_provider_catalog` (a second, admin-only catalog shape) are not on the
`/v1` path.

| Ported | LOC | |
|--------|-----|--|
| `routes/llm.py` | 1124 | the nine handlers |
| `services/provider_catalog.py` | ~350 of 543 | `build_v1_provider_catalog` + `get_resolved_defaults`; the rest is admin |
| `services/model_capabilities.py` | 382 | Ollama `/api/show` probe + sticky disk cache |
| `services/upstream_http.py` | 358 | retries, rate-limit backoff, error classification → `reqwest` |
| `services/local_backends.py` | 273 | only `coerce_local_model_if_needed` is on the request path |
| `core/byok.py` | 246 | header parse + host allowlist |
| `core/{capabilities,config_cache}.py` | 256 | modality matrix, mtime-cached file reads |
| `services/{model_catalog_cache,speech_backends,image_backends}.py` | 256 | |
| `core/provider_config.py` | 159 | env-over-dotenv precedence |
| `usage_normalize.py` | 76 | |

**Python keeps its copy regardless.** Eight modules outside the package import
`llm_proxy.core` / `llm_proxy.services` in-process — `llm_ui_catalog`,
`chat_routes`, `coder/service`, `health_checks`, `startup_validation`,
`admin_routes`, and `model_ops/{ollama_client,config_bridge,pipeline/eval}`.
Moving the routes *duplicates* the ~900 LOC of provider config and catalog; it
does not delete it. That ends with steps 3 and 4, not here.

**The cutover is one line.** `upstream.rs:151` points the Python child's
`LLM_ORCHESTRATOR_BASE_URL` at its *own* port, deliberately, so today every
internal agent call skips the daemon. Leave Python's `/v1` router mounted while
porting — that is what lets the two be cross-rendered and diffed, the same way
the four migrated domains were proven — then flip that env to the public origin
and chat, agents, coder and assistant all route through Rust with no Python
change. One extra loopback hop is the price.

**No tokenizer crate needed.** `synthesize_usage` fires only when an upstream
omits `usage`, and nothing asserts its numbers (`test_chat_usage.py` checks
`total == prompt + completion` and `estimated is True`). Python itself falls back
to `(len + 3) // 4` whenever tiktoken raises; port that and skip `tiktoken-rs`.

Order within the domain, cheapest and most-blocking first:

1. ~~`core/` — provider config, config-file cache, capability matrix.~~ —
   shipped: [`llm_config.rs`](desktop/crates/server/src/llm_config.rs). The five
   chat providers, the image backend and the speech backend are **one table**
   with a `Registry` tag rather than Python's three modules; every caller either
   filters by registry or asks something the whole table answers. Both configured
   checks are kept, because they disagree on purpose — `provider_configured` is
   the chat registry's (empty, `"other"` and unregistered names all answer true,
   so a `config.yaml` provider we have not implemented keeps showing up), and
   `is_configured` is the capability router's, which lets the image and speech
   backends answer for themselves. Files are read through one mtime+size cache
   that also fingerprints the path, so moving `CONFIG_DIR` mid-process cannot
   serve the old parse. Dropped as dead weight: the runtime discovery tier in
   `ollama_api_base`, since `discover_local_llm_bases` only ever sets the
   loopback constant the function already falls back to. `serde_yaml` is the new
   dependency, deserializing into `serde_json::Value` so the config tree walks
   like everything else here.
2. ~~`/v1/health/readiness`, `/v1/capabilities` — pure reads over 1.~~ —
   shipped: [`llm.rs`](desktop/crates/server/src/llm.rs), with all of
   `core/byok.py` in [`byok.rs`](desktop/crates/server/src/byok.rs) (the
   discovery document is part of the capabilities body, and splitting a file
   whose other half lands in step 4 means reading it twice). Auth is the
   structural piece: `require_token` guards exactly `/api/v1/*`, so these routes
   resolve the caller themselves through `auth::ProxyPrincipal` — and readiness
   resolves nobody, because the desktop probes it before it has a key. Both
   directions are covered in `tests/auth_and_proxy.rs`, since a route that stops
   checking serves the config to anyone and one that starts checking breaks that
   probe. `/v1/health` moved to 3: it pings each provider, so it needs
   `upstream_http` and the catalog cache and was never a pure read.
3. ~~`upstream_http` + `/v1/models` + `/v1/health` — the first live fetches.~~ —
   shipped: [`upstream_http.rs`](desktop/crates/server/src/upstream_http.rs),
   [`model_catalog.rs`](desktop/crates/server/src/model_catalog.rs), and the
   alias/defaults half of `llm.rs`. One `send_with_retry` taking a closure,
   because a retry needs the request rebuilt rather than replayed, and it reads
   the body before returning since deciding whether a 4xx *is* a rate limit means
   reading it. The catalog cache sleeps before its first pass exactly as Python's
   does, so a fresh server reports `model_present: null` rather than a wrong
   answer, and `/v1/health` never blocks on a backend. Two knowing divergences
   from httpx, both noted in the module: reqwest cannot tell a write or pool
   timeout apart from any other, so `write_timeout`/`pool_timeout` never appear
   (nothing in this repo branches on those codes — `llm_client.py` catches its own
   transport errors instead); and reqwest pools per host with no global ceiling,
   so httpx's 100-connection cap has no analogue to port. `_effective_defaults`
   came with it, including the double `is_supported_provider` test that collapses
   to one branch because the first blanks what the second reads.
4. ~~`/v1/chat/completions` + `/v1/embeddings` — local-model coercion, the
   capability guard, and SSE passthrough.~~ — shipped:
   [`model_capabilities.rs`](desktop/crates/server/src/model_capabilities.rs),
   [`usage.rs`](desktop/crates/server/src/usage.rs), the coercion half of
   `model_catalog.rs`, and the two handlers in `llm.rs`. A stream that fails once
   it is open cannot change its status, so the error goes out as one more `data:`
   frame — what an SSE client is already reading — while a failure *before* the
   stream opens answers with the upstream's own body. Capability results stay
   sticky: a probe that comes back without `tools` never un-confirms a model that
   was seen with them, since capabilities do not change at runtime but probes do
   fail. Two notes: the name heuristics are matched as literals with the same
   delimiter rule rather than a regex crate (`gemma3.*vision` is dropped, the bare
   `vision` alternative already covers it), and `model_capabilities.json` now has
   two writers — each write is a temp-file rename, so the worst interleave costs
   one freshly probed entry that the next probe rewrites.
5. ~~`/v1/catalog` — the `/api/show` probe and its sticky disk cache.~~ —
   shipped: [`provider_catalog.rs`](desktop/crates/server/src/provider_catalog.rs).
   Model discovery degrades in a fixed order (live list → `config.yaml` aliases →
   the UI's `fallback_models` → the provider's built-in default) and each row says
   which rung it came from, so the desktop's provider screen always renders and
   always says why. `reachable` stays `null` until something has actually tried,
   which is not the same as `false`. The admin surface's *other* catalog shape
   (`build_provider_catalog`, behind `/api/v1/llm-proxy/ui/*`) stays Python — a
   different body for a different screen. Threading the per-caller timeouts
   through the shared fetchers came with it: Python gives the same fetch 8s from
   the background cache, 12s from the coercion path, 15s for a cloud catalog and
   20s for Gemini, and a single constant would have made a cloud provider that
   answers in 10s "unreachable" here and fine there.
6. ~~`/v1/images/generations`, `/v1/audio/speech` — two small registries.~~ —
   shipped, in `llm.rs` over the registries `llm_config.rs` already carried.
   **Still unverified: a successful speech synthesis.** Nothing was listening on
   `SPEECH_API_BASE` while this was written, so both servers answered the same
   `502 connect_failed` — which does exercise the routing, the loopback-refusal
   short-circuit and the error classification, but not a real audio body. Run one
   through a live Piper before trusting the desktop's voice path.

The pytest files that actually exercise `/v1` are `test_v1_catalog.py`,
`test_capabilities.py`, `test_byok.py`, `test_chat_stream.py` and
`test_standalone_api.py`; every other `/v1/` grep hit is `/api/v1/`.

Cross-rendered with all nine routes migrated: forty-four cases — happy paths,
every `400`, the `503`s for an unconfigured provider, the `501`s for a capability
a provider or a BYOK key cannot serve, the `403` for a base URL off the
allowlist, a `502` for an upstream that is not listening, and a live
`providers=all` catalog with capability probes against a running Ollama — parse
identically through both servers, once `elapsed_ms`, `model_list_age_sec`,
`probed_at` and `request_id` are compared by type rather than value (separate
processes, separate caches, separate requests). Every POST in that set is
rejected before it reaches a vendor, because a real completion body is not
comparable between two calls; the completions themselves are covered separately
by driving one through each server, buffered and streamed, and asserting the
contract instead — a `usage` block that adds up, frames arriving ~1s before the
stream ends, a terminating `[DONE]`. **There is no
difference left anywhere on the migrated surface** — the last one, the missing
`request_id`, closed with
[`request_id.rs`](desktop/crates/server/src/request_id.rs). It is the outermost
layer, so an auth rejection is stamped too, and it writes the id onto the
*request* as well: Python's own middleware prefers an incoming `X-Request-ID`, so
one call now reads the same in both halves' logs and in whichever half built the
error envelope. `ApiError` and `AuthError` read it from a task-local rather than
an extractor threaded through every handler signature — they build their bodies
inside the handler's task, so it is in scope where it is needed and nowhere else.

#### The tokenizer is real now — decided before step 4, not during it

`context_budget.rs` carried only Python's char fallback, recorded as safe because
"nothing asserts exact token counts". **Step 4 expired that.** `context_usage` is
a *response body field* on the coder and assistant chat routes, `tiktoken>=0.7.0`
is a hard requirement so Python never takes its own fallback, and — the part that
is not cosmetic — the same estimator drives `fit_chat_messages_for_request`, so
near the budget the two servers would have sent the model **different messages**.

`tiktoken-rs` is now a dependency and `usage::estimate_tokens` is real BPE, with
the char heuristic kept for an encoder that will not load and
`AGENT_PLATFORM_TOKEN_ENCODING` mapped by name (an unknown name falls back to the
heuristic rather than guessing a different vocabulary). Verified against Python on
the same inputs: `"hello world"` → 2, the pangram → 9, and truncating 1000 `x`s to
100 tokens gives **774 characters and exactly 100 tokens on both sides**.

Two things learned doing it, both now in the module docs. A **long run of one
repeated character is the BPE regex's worst case** — ~300ms per 8k here, ~145ms in
Python, superlinear beyond, against ~6ms for prose of the same length. That is
parity, not a port defect, but it means a pathological *tool result* costs real
time in `shrink_messages_to_budget`, which re-estimates every message each round —
worth knowing before coder starts feeding it whatever a command printed. And it
turned one test into a minute of CPU: an assertion written when tokenising was
free passed `"x".repeat(400_000)`. The suite went 0.03s → 324s → 2.08s once that
input became prose.

#### JSON key order is a number in a response body

Found porting `chat_usage`. `context_usage.categories` counts the *tokens of the
rendered JSON* — `estimate_tokens(json.dumps(tools, ensure_ascii=False))` — and a
Python dict renders in insertion order where `serde_json::Map` is a `BTreeMap` and
renders sorted. Measured on the real `coder/executor.py::TOOL_SPECS`: **518 tokens
Python's way, 510 sorted.** Eight tokens of drift in a body field, on every chat
response, from nothing but key order.

`serde_json` now has `preserve_order` on, crate-wide. It cannot make anything
worse — Python is insertion-ordered everywhere, so every rendered object in this
crate moves toward what Python emits, and the 144 existing tests pass unchanged.
It also retires the "nested keys sort alphabetically where Python keeps write
order" caveat that `todos.rs::py_repr` and `provider_catalog.rs` both carry.

It is one line in `Cargo.toml` whose loss would be silent everywhere except a
cross-render, so `chat_usage.rs` asserts it directly
(`json_objects_keep_insertion_order`).

#### coder — scope (step 4)

> **Shipped whole, 2026-08-07.** All ten routes are Rust:
> [`coder.rs`](desktop/crates/server/src/coder.rs) (routes, persistence, SSE
> framing), [`coder_loop.rs`](desktop/crates/server/src/coder_loop.rs)
> (`run_agent_turn`, the LLM step, leaked-call recovery) and
> [`coder_tools.rs`](desktop/crates/server/src/coder_tools.rs) (`LocalExecutor`
> and the delegated park). The CRUD/loop split this note used to warn about
> lasted a few hours and is gone. **What the run found is at the end of this
> section.**

**"Largest and highest-churn" is right about the size and wrong about the risk.**
`app/coder/` writes one table nobody else touches, so there is no two-writer
here — the hazard is that this domain holds the **first in-process state shared
between two HTTP requests** the migration has met, and unlike `/processes` cancel
it is not DB-mediated. That fixes the granularity: five of the ten routes move in
one commit or none of them do.

**Ten routes, seven called.** All sync `def` except `/chat/send`. Each calls
`require_scope("chat:write")` itself. **There is no project scoping anywhere in
this domain** — `coder_chat_threads` has no `project_id` and no handler calls
`assert_token_project_access`, so a workspace token with `chat:write` sees every
coder thread on the box. Port it verbatim; narrowing it is a separate decision.

**Two GETs write.** `_resolve_thread(session, None)` falls through to
`_create_thread_row`, which commits — so `GET /coder/chat/thread` and
`GET /coder/chat/context-usage` INSERT a `"New session"` row on an empty database
and return it. A port that answers those as pure reads diverges on the first call
against a fresh DB, and no test covers it.

Three routes have no caller (`send`, `retry`, `context-usage`); `send` is the
non-streaming twin of `stream` over the same loop, so it is free once the loop
exists. Timestamps are naive and rendered with `.isoformat()` — **no `Z`**, the
same trap todos and processes hit.

##### The crux: the parked tool-call future

`desktop_executor.py:19` is a module-level
`dict[tuple[int, str], asyncio.Future[str]]`. `execute` creates a future on the
running loop, refuses a duplicate key by *returning a string as the tool result*,
and `await asyncio.wait_for(fut, timeout=300.0)`.
`resolve_desktop_tool_result` sets the result; a missing or already-done key is a
`KeyError` → **404**. On timeout the turn does not fail: it appends
`"Error: timed out waiting for desktop to execute tool"` as a `tool` message and
calls the model again. At most one future is live per turn.

Two details a port must not lose. `/chat/tool-result` is a **sync `def`**, so it
calls `set_result` on an asyncio future from a threadpool thread with no
`call_soon_threadsafe` — it works, but the loop is not woken, so the resume waits
for the next poll. `oneshot::Sender::send` is correct by construction, so Rust is
strictly better here and the difference is unobservable. And
`_allow_commands` is assigned and never read; the desktop owns that decision.

Rust replaces it with an `AppState` field:
`Arc<Mutex<HashMap<(i64, String), oneshot::Sender<String>>>>`, park is
`tokio::time::timeout(300s, rx)` with a drop guard that clears the key on every
path, unpark is `tx.send`, and "already sent" becomes "key absent" → the same 404.

**It does not survive a two-process split.** The desktop sets
`delegate_tools: true` and sends `X-Agent-Platform-Client: portal-desktop` on
every coder stream, so the delegated executor is chosen unconditionally. The park
lives in whichever process served `/chat/stream`; the unpark must land in the
same one. Split them and `/chat/tool-result` 404s while the turn hangs the full
300s and then feeds the model "timed out" — a silent wrong answer, not a failure.
**Routes 6, 7, 8, 9 and 10 are one commit.**

Worth stating in the module docs: this domain has *two* pause mechanisms and only
one is portable. The **approval** pause is `pending_call_json` on the row — DB
state, and it survives a split exactly like `/processes` cancel. The
**delegation** pause is process memory and does not.

##### Tables, closure, streaming

One table, `coder_chat_threads`, written by nothing else in either language — no
two-writer. One caveat if the CRUD lands before the loop: Python's `_persist`
writes the whole row back, so a Rust `DELETE` mid-stream leaves SQLAlchemy
updating zero rows — a `StaleDataError` 500 rather than a silent resurrection.
An argument for keeping that split short-lived.

~2.5k LOC ported, ~2.0k of it new, because two large pieces already exist:
`context_budget.rs` has **all four** functions coder needs (step 3 landed three of
them), and **`desktop/crates/app/src/coder_tools.rs` is already `LocalExecutor` in
Rust** — path jail, `read_file`, `write_file`, `list_dir`, `search`, `repo_map`,
with tests. Lifting it into the server crate is a move, not a port. **One constant
must change on the way**: `COMMAND_TIMEOUT` is 180s there against Python's 60s,
and the model reads that timeout in the error string.

**The coder loop needs no streaming internal caller, and that is not a
compromise.** Every LLM call in the loop is buffered — the SSE the routes emit is
the server's own framing of whole steps, which `sse.rs` states outright. So
`complete_internal` is a drop-in and the cost of the missing streaming caller is
zero. The one thing resembling streaming is the 8s `heartbeat` frame while a step
is in flight: `tokio::select!` over the future and an interval.

##### What proves it — much less than step 3

28 tests in `test_coder_api.py`. Ten never touch HTTP; **fifteen of the eighteen
that do monkeypatch `coder.service.httpx.AsyncClient`**, which is in-process only,
so against a live server the patch does nothing and the failure sets match while
proving nothing. That leaves **three** genuine contract tests, against step 3's
eighteen. Also: every client test hardcodes `Bearer test-key`, overriding the
harness key — **both parity servers must be started with
`AGENT_PLATFORM_MASTER_KEY=test-key` literally**, or all eighteen 401 on both
sides and the matching failure set is meaningless.

What to do instead: a **scripted upstream** on `OLLAMA_API_BASE` replaying a
canned completion sequence (a `tool_calls` message, a `run_command` call, then
prose), so the same script drives both servers — `_fake_llm_sequence` moved out of
the process. Then diff the **SSE transcript** (event names in order, then
payloads; `heartbeat` by presence, it is wall-clock) and the **row**
(`messages_json`, `pending_call_json` — the blobs `coder::rebuild_turns` depends
on). For delegation itself, `cargo test -p agent-platform-desktop -- --ignored
delegation` already drives a real server and is the only thing that reaches the
park/unpark; add unit cases for unknown-key and already-resolved 404s. Make the
300s timeout an argument so its branch is assertable in milliseconds — it returns
a tool *result*, and a port that turns it into a 502 breaks a turn Python
recovers.

##### Two defects found while scoping, neither a blocker

- ~~**`tool_call_parse.KNOWN_TOOLS` is stale**~~ — fixed in Python first, as
  this note asked, so the port copied one behaviour instead of choosing between
  two. `search` and `repo_map` are in the set on both sides now.
- **`last_used_at` stops advancing for coder traffic** once this lands — Rust's
  auth does not write it and Python's only does for requests that reach Python.
  Every step has this consequence; coder is the first where one screen is most of
  a token's traffic, so a coder-only token will look unused in `GET /api-tokens`.

##### The CRUD half — shipped 2026-08-07

[`coder.rs`](desktop/crates/server/src/coder.rs): `GET`/`POST /chat/threads`,
`GET /chat/context-usage`, `GET /chat/thread`, `DELETE /chat/thread/{id}`.
Both oddities this note called out are ported as-is and commented as
deliberate — **no project scoping anywhere in the domain**, and **the two
GETs that write** (`_resolve_thread(None)` inserts a `"New session"` row and
returns it, so answering them as pure reads would diverge on the first call
against a fresh database). The insert-on-read was driven on an empty DB to
confirm it.

`TOOL_SPECS` and `CODER_SYSTEM_PROMPT` are embedded byte-exact rather than
rebuilt from Rust structures, because both are *tokenized into every
`context_usage` body* — a drifted character is a changed number in a
response, not a cosmetic difference. Same reasoning as `/profile/forms`.

**The cross-render earned its keep: it found a real bug, and not in the new
code.** `usage.rs::estimate_messages_tokens` counted a message's `tool_calls`
as JSON, where Python counts `str(tc)` — a Python repr. Those are different
strings, so the `conversation` figure came out 68 against Python's 74 (~8%
low on a transcript with tool calls). **Nothing caught it before because no
earlier domain's messages carry `tool_calls`**: the assistant's are
`{role, content, usage, proposed_actions}`. Fixed to use `py_repr`, with a
test pinning the difference. This is the third time this class — Python's
`str()`/`json.dumps` shape reproduced or not — has changed a number or a
prompt; `py_repr`, `PythonJson` and `EnsureAscii` all exist for it.

After that fix, all three read endpoints render **byte-identical** to Python
off the same seeded row (tool calls, a persisted `usage` blob, unicode and an
emoji). Create/list/delete round-trip and both 404s match too. The only
differences left are the documented `input`/`ctx` pydantic envelope fields on
three validation errors — type, `loc`, `msg` and status all match. And the
five still-proxied routes were checked to still *fall through* rather than
405, which is the trap this migration has now hit three times.

##### The loop half — shipped 2026-08-07, and what proving it took

[`coder_loop.rs`](desktop/crates/server/src/coder_loop.rs) is `run_agent_turn`
plus its one LLM step; [`coder_tools.rs`](desktop/crates/server/src/coder_tools.rs)
is both executors. Three shapes are worth knowing before touching either:

- **Events are pushed through an `Emitter`, not yielded.** `POST /chat/send`
  runs the same turn with `Emitter::Discard`. An emit that fails *is* the client
  disconnect — Python's `GeneratorExit` at the next `yield` — so the turn stops
  there and the caller persists what the agent finished, which is what the
  `finally` does.
- **`merge_title_sse_events` did not need porting after all.** Its queue and two
  workers exist to interleave one late frame into a stream; an
  `mpsc::UnboundedSender` cloned to the title task *is* that merge, and "keep
  waiting for the title after the source closes" falls out of the channel
  closing when the last sender drops. The note in `chat_thread_title.rs` said
  this function belonged in the coder commit; it belonged in the bin.
- **The park is `oneshot::Sender` in `AppState::coder_pending`**, cleared by a
  drop guard on every path. Python's `/chat/tool-result` is a sync `def` calling
  `set_result` from a threadpool thread with no `call_soon_threadsafe`, so its
  loop is not woken and the resume waits for the next poll; `send` wakes the
  parked task by construction. Strictly better and unobservable.

Two things the port had to get *wrong* on purpose. A turn that pauses for
approval reports **zero usage** in its `done` payload, because Python only
extends `usage_steps_out` on the two final-assistant paths. And
`LocalExecutor`'s command timeout is **60s here against the desktop
executor's 180s** — the number reaches the model in the timeout string, and
parity is measured against Python.

**What proved it.** The pytest file is nearly worthless against a live server,
exactly as scoped: **16 of 28 fail, identically on both, none unique to either
side** — fifteen monkeypatch `coder.service.httpx.AsyncClient`, which is
in-process only, and one patches the environment. So the real instrument was the
scripted upstream the scope note asked for: a fake Ollama answering
`/api/tags`, `/api/show` and `/v1/chat/completions` from a queue, with a
`/__calls` endpoint that records **what each server sent to the model**. Six
scenarios, each run through Rust and through the Python child and diffed three
ways (SSE transcript, persisted row, outgoing request bodies): tool-call-then-
prose, the approval pause + 409-while-paused + resume, reject / call_id
mismatch / no-pending-call, the PLAN step with a leaked `<function=search>`,
retry + the 4xx shapes, and non-streaming `send`. Then delegation on its own,
with a reader thread answering the `tool_call` frame the way the desktop does,
including the replayed-answer 404. **Everything matched** but the two entries
below.

- **A real bug, and again in the class that keeps producing them.**
  `/chat/tool-result`'s 404 detail is `str(e)` on a `KeyError`, and `str` of a
  one-argument exception is `repr` of that argument — so Python's message
  arrives **wrapped in its own quotes**, double ones here because it contains an
  apostrophe. Rust was sending the bare sentence. `py_repr` again; that is the
  fourth time Python's `str()`/`repr()`/`json.dumps` shape has moved a byte on
  the wire.
- **One deliberate divergence: a missing thread on `/chat/retry` (and
  `/chat/approve`).** `stream_retry` raises its `HTTPException(404)` from
  *inside* the async generator, after `StreamingResponse` has already sent 200
  headers — so Python answers with a dead connection (`peer closed connection
  without sending complete message body`). Rust checks before it opens the
  stream and answers a clean `404`. Not ported as-is: no client is written
  around a truncated chunked read the way `sse.rs` is written around the
  terminal-sentinel asymmetry, and reproducing it would mean half-writing a
  response on purpose. The desktop reads it as `Failed("HTTP 404: Coder thread
  not found")` instead of `Failed(<transport error>)`.

One measurement that is not a divergence and cost a detour: the `plan`-step
scenario's `context_usage.conversation` came out 83 against 82. A recovered
leaked call carries `leaked_<uuid4 hex>`, and a random hex string tokenizes to a
different count every run. Reading **each server's row through both servers**
settled it — the counts agree per row and differ per id — so that scenario
compares its row with the token block dropped. Cross-reading is the cheap test
for "stored differently" versus "counted differently" and should be the first
move next time a number wobbles.

#### assistant + chat — scope (step 4)

**Smaller than the step list implies, and one headline claim was dead code.** The
assistant has no in-process state, no background lifetime outliving a request, and
**no streaming route at all** — its routes can move one at a time.

**Twenty-four routes** (twenty assistant + four chat). **No assistant route calls
`require_scope`** — access is a `project_id` query param plus
`assert_token_project_access`. `POST /api/v1/chat` is the one route here that
checks a scope (`chat:write`).

**Thirteen have a desktop caller, and `/assistant/chat/*` is fully wired** —
the backlog's "except the planning chat" is stale; `agenda_chat.rs` calls threads,
thread, send, retry, submit-form and apply, and renders `present_planning_form`
and the pending-action approval. **Ten routes have no caller at all**, including
all three `chat_routes` GETs, which their own docstrings call Flow UI routes — and
the Flow UI is deleted (ADR 0005). Rust already answers those facts on
`/v1/health/readiness`, `/v1/models` and `/v1/catalog`. **They are deletion
candidates, not port candidates**; decide before writing 141 lines of
`llm_ui_catalog.py` in Rust for nothing.

**State between requests: none.** One module-level `asyncio.Semaphore(8)` in
`chat_routes.py`, and a smart-title `create_task` that is awaited before the
response returns. `assistant_chat.py` is 1054 lines with no mutable module state.
Nothing resembling coder's parked future. The only ordering constraint is
`_resolve_thread`'s insert-when-empty: routes 6, 7 and 8 all create the same row
and both servers must agree on "most recently updated thread", so those three move
together.

**Tables.** `assistant_domain_profiles` is a **live two-writer today** (see the
gaps list). `todo_items` is written from three assistant routes via
`board_action_apply.py`, which is what corrects the step-3 note. Nothing here
writes `api_tokens`.

**Closure ~3.7k LOC, about a third already written**: fifteen of
`board_action_apply.py`'s seventeen action arms already exist in `todos.rs`, and
`merge_profile` *is* `todos.rs::merge_domain_profile`. Needed and not yet Rust:
`chat_usage.py` (157) and the non-SSE half of `chat_thread_title.py` (199) — both
**shared with coder and playground, so write them once, first**.

**One route streams**, `POST /api/v1/chat`, and it is already a byte-for-byte
pass-through of `/v1/chat/completions` — in Rust it is `chat_completions` with a
fixed `chat:write` check and the master-key 503, roughly 60 lines and no new
machinery. It also collapses a real hop: `llm_proxy_base_url_v1()` defaults to
**the Rust daemon's own port**, so today an assistant turn goes desktop → Rust
proxy fallback → Python → HTTP back into Rust `/v1` → upstream.

**Order:** (1) ~~`POST /api/v1/chat` alone~~ — shipped,
[`chat.rs`](desktop/crates/server/src/chat.rs). The handler is ~50 lines because
it calls `llm::chat_completions` *itself* rather than re-porting its parts, with
an unrestricted principal and an empty `HeaderMap` — which is exactly what
Python's loopback produced: `chat_routes.py:233` sends `Bearer {master_key}` and
only two headers, so that call was always master-authenticated regardless of the
caller and BYOK never applied. The caller's own `chat:write` is checked first, on
the real principal. Streaming, buffered, the pre-stream 4xx passthrough and the
error frame all come free.

Two things worth knowing. **tokio's `sync` feature is not enabled in this crate**,
so the 8-slot concurrency cap is a permit pool over `futures::channel::mpsc` with
a `Drop` guard, and the permit is moved into the SSE body stream so a slow reader
still holds its slot — Python holds the semaphore across the generator, not the
request. And the mid-stream error frame now carries `classify_with_context`'s
sharper code (`read_timeout`, `connect_failed`) where Python said
`upstream_error: Upstream request failed` — the frame shape is identical, and
Python's wording described *the loopback hop* failing, which no longer exists.
(2) ~~the three dead GETs stay proxied, pending a decision that is not the
migration's to take~~ — **decided 2026-08-07: deleted, not ported.** `/llm/ready`,
`/chat/resolved-defaults` and `/llm/ui-catalog` had no caller in this repo and
their docstrings named the deleted Flow UI. Gone from Python: the three handlers
in `chat_routes.py`, `llm_ui_catalog.py` whole, `get_resolved_proxy_defaults` (its
only caller), and the four tests that named them in `test_standalone_api.py` (two
were folded into `/api/v1/llm-proxy/ui/providers` instead of dropped outright,
since they covered a real behaviour — a provider switch applying without a
restart — through the dead route rather than because of it).
(3) ~~`chat_usage` + `chat_thread_title` in Rust~~ — **shipped**,
[`chat_usage.rs`](desktop/crates/server/src/chat_usage.rs) and
[`chat_thread_title.rs`](desktop/crates/server/src/chat_thread_title.rs), first
because both halves and playground need them and they are the only part of step 4
that touches no SQL; (4) **the reads — part-shipped**, in
[`assistant.rs`](desktop/crates/server/src/assistant.rs) (new module, 2026-08-06):
`GET /chat/threads`, `GET /profile`, `GET /profile/forms`, `GET /profile/{domain}`
and `GET /reviews/pending`. `dashboard` and `goals` are **not** in this slice —
both call `ensure_assistant_board`, which finds-or-creates the Personal
Assistant's `TodoBoard` and needs the item-listing/filtering logic
`assistant_service.py::get_dashboard` shares with the todos domain; porting them
alone would duplicate that rather than reuse it, so they wait for whichever of
this step or a `todos.rs` extraction lands first. Two paths share a route with a
still-proxied method (`POST /chat/threads`, `PATCH /profile/{domain}`) and had to
declare that method to `proxy::forward` explicitly, same as `processes.rs`'s
`POST /processes` note — leaving it to the fallback would 405 instead of falling
through. `/profile/forms` is `_DOMAIN_FORMS` re-keyed in
`list_domain_form_specs`'s UI order (general, fitness, nutrition, travel,
finance, professional — **not** the source file's definition order, which has
travel before nutrition) as one embedded JSON constant, parsed once per request;
`serde_json`'s crate-wide `preserve_order` (see below) is what keeps its field
order byte-identical to Python's. Cross-rendered against a live Python child on a
scratch DB: `profile` empty and populated, `profile/forms` (byte-identical),
`chat/threads` list after a Python-created row, `reviews/pending` empty, and the
422/404 shapes for a missing/zero/unknown `project_id` — all matched but the
documented `input`/`ctx` gap. Not yet re-driven through a populated
`reviews/pending` row (a live one needs `reviews/run`'s LLM call, not shipped
here); the rendering path is the same `json_object`/`json_array`/`iso_from_sql`
already proven in `todos.rs` and covered by `wire.rs`'s own tests, so this is a
lower-confidence gap, not an unknown.

`dashboard` and `goals` **did land in the same slice, not deferred as first
planned** — `ensure_assistant_board` turned out cheap once `todos.rs`'s
`ItemRow`/`ItemOut`/`CategoryRow`/`CategoryOut`/`ITEM_COLUMNS`/
`CATEGORY_COLUMNS`/`apply_board_template`/`default_board_model` were made
`pub(crate)` and reused rather than re-ported. `ensure_assistant_board` itself
is new: `project.assistant_board_id` (via `state.any`, `projects.rs`'s
Postgres-aware convention) is the fast path, a same-named board or a freshly
templated one are the fallbacks, each writing the pointer back. Horizon math
(`_horizon_range`, `_item_in_horizon`) is `chrono` over `wire::parse_naive`
rather than string comparison, so it costs a parse per `due_at`/`scheduled_at`
but reads the same as everywhere else timestamps cross this boundary.
Cross-rendered on a scratch board seeded through the (already-Rust) todos item
routes: the empty board on both the creating call and a second, fast-path call
from the other server; a goal, a not-done habit, an overdue item, a top-level
item with a subtask, a week-horizon item, and two exclusion cases (a `done`
habit, a `done` week item that the `time_horizon in (None, day, week)` day
fallback would otherwise have surfaced) — `day`/`week`/`month` dashboards and
`goals` all byte-identical. (5) ~~`assistant_domain_profiles`~~ —
**shipped**, `PATCH /profile/{domain}` in `assistant.rs`, closing that
two-writer by calling `todos.rs::merge_domain_profile` (now `pub(crate)`,
returning the merged profile instead of `()`) rather than re-porting
`user_profile_service.merge_profile` — the two were already the same function
in every way but signature. Cross-rendered: insert, merge-onto-existing with a
null and an empty string both dropped (key position preserved, matching
Python's in-place dict update), a new key appended after existing ones, and the
missing-body 422. (6) ~~threads 5/6/7 together, then the LLM turns~~ —
**shipped** (2026-08-07): `GET /chat/context-usage`, `GET /chat/thread`,
`POST /chat/send`, and everything under them — this was the largest single
piece of the migration so far. New modules: [`assistant_turn.rs`](desktop/crates/server/src/assistant_turn.rs)
(the pure reply-text/action-normalization functions from `assistant_chat.py`,
operating on [`action_orchestrator::PlannedAction`](desktop/crates/server/src/action_orchestrator.rs)
directly rather than a re-declared type, since `assistant.schemas.PlannedActionOut`
*is* `todos.schemas.PlannedActionOut` in Python) and
[`clarifying_form.rs`](desktop/crates/server/src/clarifying_form.rs) (the
regex-driven form builder — `regex` is now a direct dependency, added rather
than hand-rolling seven patterns for the first genuinely regex-shaped Python
module this migration hit). `action_orchestrator::decide_actions` gained a
third return value, `Vec<LlmStepUsageOut>` — its one existing caller
(`todos.rs::agent_step`) discards it same as Python's `agent_bridge.agent_step`
does, but `assistant_chat._generate_assistant_turn` does not, and the module's
own docs had called usage accounting dropped "by design" before this made that
narrower than it looked. `todos.rs::merge_domain_profile`,
`build_action_tools` and `python_str` all gained `pub(crate)` for the same
reuse-not-copy reason as step (5).

Verified two ways. Unit tests lift fixtures straight from Python: `route_profile_slug`
against `test_assistant_router.py` verbatim, `clarifying_form` against ad hoc
`python -c` runs including a key-order regression (`coerce_llm_field`'s
`kind` landed last instead of third on first pass — see the comment at its
call site). Then cross-rendered live, both servers against the same scripted
`OLLAMA_API_BASE` upstream (a ~70-line `http.server` script, gone with the
session per the established method) so `decide_actions`/`chat_only` see
identical model output: a cold board-creating `GET /chat/thread` from each
server against the other's project, `GET /chat/context-usage` fast-path, a
`POST /chat/send` that proposes `create_item` (byte-identical including the
`tools` token count and the `decide_actions` usage step), the same with
`propose_actions:false` (the plain `chat_only` path, full prompt/completion
split this time), and one deliberately tangled case — a message that routes to
`travel-planner`, whose profile gaps trigger `maybe_inject_domain_form`
*alongside* a model-proposed `ask_clarifying_questions` — where
`extract_pending_form`'s action-list order (`present_planning_form` scanned
before `ask_clarifying_questions`) decides which form and which reply text win,
and both servers picked the same one. Smart-title generation was verified
too: the fake upstream's plain-text reply became the same cleaned title on
both threads, and `ChatSendResponse` has no `title` field — pydantic drops it
silently on the way out, confirmed with `ChatSendResponse(**data)` in `python
-c` before relying on it, so the route strips it from the Rust response too
even though `send_chat_message` computes and persists it.

Not covered by any of the above, and worth knowing before trusting this
blind: non-English or emoji field labels through `clarifying_form`'s regexes;
and a model that answers the tool-call prompt in prose instead of calling a
tool (`decide_actions`'s own two fallbacks are unit-tested in
`action_orchestrator.rs`, but not re-exercised end-to-end through this new
call site). (7) ~~apply and reviews~~ — **shipped** (2026-08-07):
`POST /chat/apply`, `POST /reviews/run`, `GET /reviews/pending` (already
part-shipped in sub-step 4), `POST /reviews/{id}/apply`,
`POST /reviews/{id}/dismiss`, `POST /items/{id}/complete`. Closes
`todo_items`' last Python writer. (8) `/assistant/reset` last — its ordered
cascade delete is the only place a wrong FK order corrupts, and it has no
caller.

**Scope, before writing it.** `board_action_apply.py`'s `apply_board_actions`
is not `todos.rs`'s per-item `agent_apply` re-targeted at a board — the two
independently implement the seventeen action ids and diverge on two of them,
both matched rather than "fixed": `_apply_item_action` (the board-scoped
inner dispatcher `chat/apply` and `reviews/apply` both go through) has no
`export_markdown_checklist`/`export_ics_event` arms, so those two ids always
land as `"{aid}: no change"` at board scope even though the per-item route
supports them; and its `break_down_task` has no `grocery_groups` branch, only
the generic `steps` one. New file-scoped code:
[`apply_board_actions`/`apply_item_action`](desktop/crates/server/src/assistant.rs)
(the eight real `_apply_item_action` arms, plus `create_item`/`create_habit`/
`create_subtask_item` as their own board-scoped inserts — `create_subtask_item`
here requires an explicit `parent_item_id` and checks it against `board_id`,
where the per-item route's version defaults the parent to "whatever item this
request is about"), `reviews_run`/`reviews_apply`/`reviews_dismiss` over
`review_service.py` (stats computed as a `Map` in the Python dict's field
order, since this crate's `Map` preserves insertion order; `REVIEWER_PROMPT`
copied byte-identical from `todos.seeds.py`), and `complete_item`. Reused
rather than re-ported: `assert_item_access`, `load_item`, `append_item_event`,
`trigger_webhook`, `ItemPatch`, `TODO_STATUSES`, `now_isoformat` (all made
`pub(crate)` in `todos.rs` for this), `python_str`, `merge_domain_profile`
(already `pub(crate)`), and `resolve_thread`/`persist_thread`/
`send_chat_message`/`resolve_pending_proposal_in_messages` from this file's
own sub-step 6.

Two Python quirks worth knowing rather than "fixed": `_apply_item_action`'s
`break_down_task` step fallback is Python's `str(s)` on the *whole dict* when
a list entry has no `"step"` key — matched here with `python_str`, which is
scalar-shaped, not dict-repr-shaped, so a stepless dict entry renders
differently between the two servers (narrower than every other gap in this
file, and the same kind of thing `propose_review`'s `focus_areas` stringify
has). `adjust_plan`'s `status` field is written **unvalidated** against
`TODO_STATUSES` — matched from `_apply_item_action`, unlike `move_item_status`
in the same file, which does validate; that asymmetry is Python's, not a typo
here.

Verified live rather than cross-rendered against Python (the Postgres port
occupying `db.rs`/`AppState` made a byte-for-byte harness more setup than the
step warranted): built and ran against a scratch SQLite DB with a spawned
Python child, `create_item` via `chat/apply`, `reviews/run` end-to-end
including a real LLM call and `reviews/pending`/`apply`/`dismiss` (including
the already-applied 400 and the re-dismiss idempotency), `items/{id}/complete`,
and `chat/apply` exercising `move_item_status` + `add_subtask` together with a
missing-`item_id` skip, an item-not-on-this-board skip, and
`present_planning_form` silently filtered rather than applied or skipped —
all correct. Not yet cross-rendered against a live Python server the way
steps 1-3 were; `cargo test`'s 161 pre-existing cases still pass unchanged.

**(8) `chat/retry` + `chat/submit-form` — shipped** (2026-08-07), which left
`/assistant/reset` as the last one (sub-step 9, below). Both are thin over
machinery sub-step 6 already built (`generate_assistant_turn`,
`send_chat_message`, `resolve_thread`, `persist_thread`,
`extract_pending_form`, `actions_without_forms`,
`resolve_pending_proposal_in_messages`), so the new code is three pure
functions and two handlers.

`submit-form` is **two routes wearing one request shape**, and the branch is
picked by what is already pending on the thread rather than by the caller: a
pending clarifying form takes the Q&A path (drop the
`ask_clarifying_questions` action, append a synthetic user turn, optionally
continue), and everything else takes the profile path (`merge_domain_profile`,
drop form actions, append or continue). Python's own split, kept.

The three new pure functions were **verified against Python before being
trusted, not after**: `resolve_form_submit_domain` over all six precedence
cases and `format_answers_message` /
`clarifying_form::format_clarifying_answers_message` byte-for-byte, each by
running the Python original under `python -c` and diffing the `repr`. Both
are in `cargo test` now (163 total). That mattered — those two summaries are
not display copy, they are the **synthetic user turn the model then answers**,
so a wording drift is a prompt change.

One deliberate divergence, flagged in code: Python declares
`message_index: int = Field(ge=0)`, so a negative index is pydantic's **422**
where this answers a plain **400 `message_index out of range`**. Same known
`input`/`ctx` envelope gap as the rest of this crate, one status code wider.

Driven live on a fresh scratch DB against a real model: the profile branch
with `auto_continue` both ways (the profile write survives the continuation,
and the continuation returns a full `ChatSendResponse`), the clarifying
branch end-to-end — a real `ask_clarifying_questions` elicited from the model,
then submitted, exercising every rendering rule in one call (field-id label
lookup, the underscore fallback for an unmatched id, `True` → `Yes`, `[]` →
`(none)`) — and `retry`'s three refusals (out-of-range, negative, unknown
thread) plus a successful regenerate-from-index-0.

**(9) `POST /assistant/reset` — shipped** (2026-08-07). **The assistant
domain is whole**; only `POST /chat/threads` still proxies, by choice.

This is the route the list kept putting last because its ordered cascade is
the only place a wrong FK order corrupts, so it is worth writing down what
that order actually is: null the project's pointers first (Python flushes
before deleting for exactly this reason — the FK from
`project.assistant_board_id` would otherwise block the board delete), then
chat threads, then reviews, then the board purge — item **events** first
(they are *not* cascaded on item delete), then items, then categories, then
the board row.

Two things the port does differently in shape but not in effect. The
row-by-row cascade is five `DELETE`s, with items split into two passes —
`parent_item_id IS NOT NULL` then the rest — which is Python's
`sorted(..., key=lambda i: (i.parent_item_id is None, i.id or 0))` as a
`WHERE`. That split is on *has a parent* and nothing finer, so a grandchild
shares a pass with its parent; matching Python beat topologically sorting,
and it is latent there identically. And the deletes run in one
`state.pool` transaction (the `executor.rs` precedent) while the project
`UPDATE` sits outside it, because `project` is the one table already on the
Postgres-aware `state.any` pool and a transaction cannot span both while
that migration is mid-flight — noted at the function rather than papered
over.

**Cross-rendered properly, which this route earns.** Both servers reset the
same seeded shape (assistant board, a parent item, a subtask, an item event,
a review, a chat thread, a domain profile, plus an *unrelated* second board)
on the same database: identical response bodies
(`{"project_id":1,"board_id":2,"thread_id":1}` — including SQLite reusing
the rowids, which both sides do), a byte-identical `confirm=false` 400, and
identical resulting table state row for row. The invariants that matter all
held on both: **domain profiles survive** (the documented one), the
unrelated board and its categories are untouched, and the new board's
`created_at` proves it was genuinely purged and rebuilt rather than left in
place. `last_todo_board_id` was then driven through both of its branches —
cleared when it pointed at the assistant board, kept when it pointed at
another.

**What proves it: less than half.** 14 of `test_assistant_api.py`'s 28 patch
`decide_actions` or `_chat_only` in process — that is the entire LLM-touching
surface, i.e. sub-steps 6 and 7. `test_chat_stream.py` monkeypatches httpx
internals and cannot run cross-server at all, so sub-step 1 has **zero** portable
tests. Use the same scripted-upstream harness as coder, and **diff the persisted
`assistant_chat_threads` row**, not just the body — `messages_json` is
`json.dumps(..., ensure_ascii=False)`, which is *not* the `EnsureAscii` shape step
3 built for the team snapshot. Check that first; it is the same class of bug as
the timestamp one.

#### The daemon reads Python's env files now

Found by that cross-render, which disagreed on two providers. `app/database.py`
calls `load_dotenv(<root>/.env)` at import and then
`apply_platform_yaml_defaults()`, so every Python read of `os.environ` sees the
union of the shell, the repo `.env`, and the `env:` block of
`config/agent_platform.yaml`. The daemon inherited only the shell, and every key
it missed was one where the two halves silently disagreed:

- `AGENT_PLATFORM_MASTER_KEY` lives in `.env`, so **Python required a bearer
  token while the daemon in front of it, seeing no key, left auth fully open** —
  the dev convenience firing in a deployment that had configured a key.
- `DATABASE_URL` lives in `.env`, so Python ran on Postgres while Rust read the
  default SQLite file. `Config::from_env` refuses to start in exactly that
  situation and could not, because the variable it checks was never in its
  environment. Projects, teams, todos and workflows were being served from an
  empty database next to a populated one.
- Provider keys (`AIMLAPI_API_KEY`, `SPEECH_API_BASE`) are why
  `/v1/capabilities` differed.

[`dotenv.rs`](desktop/crates/server/src/dotenv.rs) applies both files in the same
precedence Python uses — shell, then `.env`, then YAML, each filling only absent
keys, with the same three secrets never taken from YAML — and `main` calls it
before the tokio runtime exists, because `set_var` is only sound while nothing
else can read the environment. It logs what it applied, which is what makes the
next disagreement of this kind visible in one line.

**Consequence: a checkout whose `.env` sets `DATABASE_URL` can no longer run the
daemon at all** — the guard finally sees it. That is the intended behaviour and
strictly better than the split above, but it is a working setup turning into a
startup refusal, so it needs `DATABASE_URL` unset (or empty, which shadows the
file for both halves) until Postgres support lands.

Known gaps, each cheap on its own:

- **Postgres is unsupported** — the daemon refuses to start with `DATABASE_URL`
  set rather than reading a different database than its child. Needed before the
  cloud deploy, and now also before this repo's own `.env` will start it (see
  above). **Sized 2026-08-06, and it is not a small one:** 166 query sites over
  nine files are typed against `SqlitePool` (74 `query`, 57 `query_as`, 35
  `query_scalar`), so it is `sqlx::Any` or a second implementation — and with
  `Any` come `$1` placeholders instead of `?` at every one of them. The sharper
  problem is `wire.rs`: timestamps are deliberately read and written as *text*
  because one SQLite column holds both naive and `+00:00` values, and Postgres
  has a real timestamp type, so that whole compatibility layer has to be
  rethought rather than ported. Worth scheduling between migration steps rather
  than during one — it touches every domain file at once.

  **Started 2026-08-06 — in progress, resumable. Read this whole block before
  touching it.** [`db.rs`](desktop/crates/server/src/db.rs) is the choke point:
  `Backend`, a `?`→`$n` rewriter that skips placeholders inside string literals,
  and `connect_lazy`. `sqlx` gained `postgres` + `any`. One domain
  (`projects.rs`) is converted and proven; eight are not.

  *The environment it was measured on, so none of it has to be rediscovered:* a
  local **Postgres 18** on 5432 already holds this schema — 29 tables, migrated
  by Python's own Alembic — and the DSN is the `DATABASE_URL` line in the repo
  `.env` (`postgresql://agent_platform:devpass@localhost/agent_platform`). Three
  findings, all measured against it rather than read:

  - **`Any` will not decode a timestamp on *either* backend** — `Any driver does
    not support the SQLite type SqliteTypeInfo(Datetime)` and `… the Postgres
    type PgTypeInfo(Timestamp)`. Nor a Postgres `integer` as `i64`. The fix is
    portable SQL, not Rust: `CAST(id AS BIGINT)` and `CAST(created_at AS TEXT)`
    both decode on both, and on SQLite the cast is a no-op over text it already
    stores. **So this is not "add Postgres" — it is rewriting all 166 select
    lists, including the SQLite path that is currently proven.**
  - **`foreign_keys=false` cannot be set under `Any`.** The URL parameter is
    rejected outright (`unknown query parameter 'foreign_keys'`) and the default
    is ON, which turns "delete a board that still has items" from Python's 204
    into a 500. A per-connection `after_connect` PRAGMA restores it — measured
    `foreign_keys = 0`, orphan insert allowed — and that is why `connect_lazy`
    exists rather than a bare `AnyPoolOptions`.
  - **That hook must be backend-conditional, and getting it wrong hangs rather
    than errors.** Running the PRAGMA against Postgres makes `after_connect`
    return `Err`, every connection is discarded as it is created, and the pool
    reports `pool timed out while waiting for an open connection` with nothing
    naming the hook.

  **The conversion is incremental, not a big bang.** `AppState` carries *both*
  pools — `pool: SqlitePool` for domains not yet moved, `any: AnyPool` for the
  ones that have — so the tree compiles after every file instead of only after
  all nine, which matters while another migration is running through the same
  files. `Config::from_env` keeps refusing `DATABASE_URL` until the last domain
  moves, so there is never a half-ported server reading two databases. When it
  does, `pool` and the refusal are deleted together.

  **`projects.rs` is done and is the template.** The recipe, in order:
  1. `&state.pool` → `&state.any` (and `state.any.begin()` for transactions);
  2. cast in the shared column constant, not per query — `PROJECT_COLUMNS`
     covers most SELECTs in the file at one line, ids `CAST(x AS BIGINT)` and
     timestamps `CAST(x AS TEXT)`;
  3. any `FromRow` field or scalar typed `NaiveDateTime` becomes `String` with a
     matching cast (here `workspace_archived`, only ever tested for `is_some`);
  4. wrap each query's SQL in `db::sql(…, state.backend)` with
     [`scripts/pg_wrap_sql.py`](scripts/pg_wrap_sql.py) — paren-matching, not a
     regex, because the arguments are multi-line literals with `\` continuations
     and a mis-matched closing paren still compiles while sending different SQL.
     One pass per file; it is **not idempotent**, and it leaves a dangling comma
     on multi-line calls, which is a compile error rather than a silent one (the
     `perl` one-liner in its docstring fixes them).
  5. `cargo check -p agent-platform-server`, then prove the file before moving on.

  **How each converted domain is proven** — the casts rewrite the SQLite path
  too, so "it still compiles" is not evidence. Run that domain's suite against a
  throwaway daemon and compare to the baseline below:

  ```powershell
  $env:AGENT_PLATFORM_PORT="18456"; $env:AGENT_PLATFORM_DB_PATH="<scratch>\p.db"
  $env:AGENT_PLATFORM_MASTER_KEY="prove-key"; $env:DATABASE_URL=""
  desktop\target\debug\agent-platformd.exe        # fresh DB per run, see the traps above
  $env:AGENT_PLATFORM_TEST_KEY="prove-key"; $env:AGENT_PLATFORM_TEST_BASE_URL="http://127.0.0.1:18456"
  python -m pytest app/tests/test_<domain>_api.py -q --tb=no -rf -p no:cacheprovider
  ```

  Pre-conversion baselines, from the 2026-08-06 clean run (these fail on *both*
  servers — they mock in-process or read the test engine directly — so the count
  must not move): **projects 3/10** (`delete_project_nullifies_process_fk`,
  `project_workspace_roundtrip`, `project_workspace_state_roundtrip`), **teams
  4**, **todos 4**, **workflows 9**.

  **`projects.rs` is proven**: `3 failed, 7 passed`, the same three. The casts
  change no value on SQLite.

  **Left, and what to expect from each:**

  | File | Note |
  |---|---|
  | `teams.rs`, `workflows.rs` | same shape as projects; shared column constants |
  | `todos.rs` (3.1k), `executor.rs` (3.1k) | the big two |
  | `processes.rs` | landed with migration step 3 — convert *after* that settles |
  | `auth.rs` | the other `Option<NaiveDateTime>` pair (`expires_at`, `archived_at`) |
  | `workflow_engine.rs` | holds the one boolean-as-integer, `enabled = 1`, which Postgres rejects — it has a real `boolean` type |
  | `action_orchestrator.rs` | landed with step 3 |

  Then the finish: delete `AppState.pool`, drop the `DATABASE_URL` refusal in
  `Config::from_env`, and **only then is Postgres provable end to end** — today
  only the query layer has been measured against it, because the daemon still
  refuses to start on it. The last step is the real one: point the daemon at
  Postgres, run all four suites, and diff against the SQLite failure sets.
- **Validation detail is approximate**: `extra.errors` entries carry
  `{type, loc, msg}`, not pydantic's `input`/`ctx`. Status, code and message match.
- **The rate limiter counts in both processes** (each sees every request, so the
  effective limit is unchanged, but both reset independently).
- **`api_tokens` / `api_token_usage_daily` have two writers**, and **the step-4
  scoping corrected which routes**. `record_api_token_usage` is a
  read-modify-write (`usage_tracking.py:33-45`); after step 3 the DAG executor
  increments from Rust while Python still does. Rust uses `SET x = x + 1`, atomic
  on its side; Python's read-then-write is the lossy half. Only a project-scoped
  token is exposed — master-key callers short-circuit on `token_id is None`, and
  `spawn_process_for_item` never sets one.

  The Python writers are **six coder and playground routes**, not the three this
  list first named: `/coder/chat/{send,stream,retry,approve}` and
  `/playground/chat/{send,stream}`. **The assistant is not one of them** —
  `assistant/routes.py:185` passes a literal `None` as `token_id`, so that call
  returns at `usage_tracking.py:20` and writes nothing. It never has.

  **Consequence for the order below: this does not close at the end of step 4 as
  written.** Playground is named nowhere in the migration ordering, and
  `/playground/chat/{send,stream}` keep incrementing from Python after coder
  lands. Playground has to join step 4 or become step 4½ — and it is close to
  free: the desktop calls no playground route at all, so there is no UI parity
  surface to defend, and `playground/service.py` is a strict subset of
  `coder/service.py`'s shape.

  **Closed 2026-08-07.** Coder moved (step 4) and playground was deleted
  outright (step 4½), so no reachable Python path increments these counters any
  more. The four remaining `record_api_token_usage` importers are all inert:
  `coder/routes.py` and the three DAG services sit behind routes Rust owns, and
  the assistant's passes `None`.

  One trap when porting the assistant's call site: port the **no-op**. Making it
  pass a real token id is a behaviour fix, not a migration, and it would have Rust
  counting requests Python never counted — which a cross-render would correctly
  flag as a diff.
- **`assistant_domain_profiles` is a live two-writer today**, and has been since
  `agent/apply` shipped: `todos.rs::merge_domain_profile` on the Rust side and
  `user_profile_service.merge_profile` on the Python side, reached from four
  assistant routes. Both are read-modify-write, so Rust being one statement does
  not save it. Exposure is narrow — the same project's todo agent and assistant
  chat running in the same tick — and it closes in step 4's sub-step 5.
- ~~**`datetime_to_sql` may keep an offset the Python side drops.**~~ — settled,
  and it was real. The old helper swapped `T` for a space and trimmed a trailing
  `Z`, so a **numeric** offset survived into the column: `09:00+02:00` stored as
  `2026-08-06 09:00:00+02:00` where SQLAlchemy stores `2026-08-06 09:00:00.000000`
  — dropping the offset, never applying it. Three consequences, all silent: the
  row rendered back as `…+02:00` on one server and `…09:00:00` on the other,
  Python re-read it as an *aware* datetime, and `due_at`/`scheduled_at` are
  indexed TEXT, so every offset-carrying row sorted after every plain one.
  The evidence is SQLAlchemy's own bind processor run over five inputs, checked
  against the text already on disk in `%APPDATA%`'s SQLite
  (`2026-08-03 21:42:09.754976` — space separator, always six fractional digits).
  Fixed in [`wire.rs`](desktop/crates/server/src/wire.rs): `parse_naive` +
  `sql_string`, `naive_local()` not `naive_utc()`, with the item CRUD and
  `agent/apply`'s `as_datetime` both on it — which also closed a second, narrower
  gap where `as_datetime` rejected the space-separated form pydantic accepts and
  silently skipped the field. **Workflow CRUD was never exposed**: it takes no
  datetime from a caller at all, `next_run_at` is computed server-side on both
  sides.
- ~~**The installer has not been run end to end**~~ — done 2026-08-06, and it
  found a defect. Build is 53.3 MiB (was 48.6 in the one-binary era); silent
  per-user install, launch, and silent uninstall all exit 0. What the launch
  proves that a build does not: the installed app spawns `agent-platformd` from
  its own directory, and *that* spawns the **bundled** runtime
  (`server\runtime\python.exe`, not the machine's Python), and `/health`
  answers. Stopping the app and the daemon leaves no orphan — the child dies on
  `--exit-with-parent`.
  - **The uninstaller exited 0 and left the whole `server\` payload behind**
    (~50 MB of `runtime\` and `app\`). Inno removes only what it installed, and
    the bundled Python compiles the server on first run — 143 `__pycache__`
    directories that were never in the manifest, and one un-removable directory
    strands the tree above it. Fixed with an `[UninstallDelete]` entry for
    `{app}\server`. **A round-trip that does not launch the app in between will
    not reproduce this**, which is presumably how it survived the last check.
- ~~**The pytest suites have not been re-run**~~ — done 2026-08-06 for the four
  CRUD domains: **20 of 50 fail, identically on both**, method and its two traps
  written up above. **Processes too, in the same pass: 18 of 27 fail, identically
  on both, zero unique to either side.** Still open for the domains between them,
  which were proven by cross-render only and have no live-server suite to point
  at. In-process, all 452 pass.
- **`assist` and `agent/chat` cannot be cross-rendered.** Every other migrated
  route was diffed body-for-body against Python's; these two cannot be, because
  the body is a model completion and two calls do not agree with themselves. So
  the *inputs* were pinned instead: `assist`'s system prompt is diffed
  byte-for-byte against the imported Python string, and `agent/chat`'s context
  repr is asserted against output pasted from `str(dict)` run in this repo's
  Python (`todos.rs`, `item_context_renders_as_pythons_dict_repr`). What is still
  unproven is the *assembly* — that a real item row produces the same dict on
  both sides before either renders it. One request each way with the payload
  logged settles that.

Two behaviours that are matched deliberately and will look like bugs later:
foreign keys are **off** on the Rust pool (`AppState::new`) because the schema
declares FKs the data does not honour, and timestamps are read and written as
text through `wire.rs` because the same column holds both naive and `+00:00`
values. Both are documented in the ADR's consequences.

</details>

### The planning chat — the assistant roadmap's last unported surface

Shipped 2026-08-06: [`agenda_chat.rs`](desktop/crates/app/src/agenda_chat.rs)
(state) and [`agenda_chat_view.rs`](desktop/crates/app/src/agenda_chat_view.rs)
(rendering), over six new `Client` methods and the contracts in `types.rs`. The
server side needed no change — `/assistant/chat/*` had been complete since the
Phase 7 roadmap and had never had a client.

**It is a pane on Agenda, not a screen.** Everything it produces lands on the
board two inches to the left, so approving a proposal and watching the rows
appear is one glance rather than a navigation. `agenda.rs` owns the child state
and forwards to it; the one message it intercepts is `Applied(Ok(_))`, which
refetches the board alongside the chat's own reload. When the pane is closed
`agenda_view` renders exactly what it rendered before — `ui::page`; only the open
case switches to `ui::page_fixed` with a row, because a board that scrolls beside
a chat that pins its composer must not also sit inside a page-level scrollable.

- **The thread is the server's, and one type deserializes five routes.**
  `GET /chat/thread`, `send`, `retry` and `submit-form` all answer with the same
  thread in different states of completeness, so `AssistantChatThread` defaults
  every field that is not universal and `absorb` replaces the whole conversation
  rather than patching it. Nothing is streamed: a turn is one blocking call that
  routes to a domain profile, plans actions and answers, so the user's own
  message is shown optimistically and replaced by what comes back.
- **`PlannedAction.parameters` is carried opaquely.** Approving means handing the
  server back the object it sent — seventeen action ids with seventeen parameter
  shapes, none of which this client has a reason to parse.
- **Dismissing is applying an empty action list.** There is no dismiss route; the
  same handler resolves the thread's pending snapshot either way. A dismissal
  that left it pending would re-offer the same actions on reopen.
- **The apply banner only fires when the continuation turn is missing.** Apply
  returns what changed *and* the assistant's turn about it; when that turn came
  back it already carries the summary into the transcript, so the banner is the
  fallback for when the auto-continue LLM call fails — which it is allowed to do,
  the board write having already committed.
- **The form and the proposal live at the end of the scroll region, not in a card
  above the composer.** That was the first arrangement and driving it killed it:
  the fitness intake form is taller than the pane, so a fixed card that size
  pushed the composer *and its own submit button* off the bottom of the window.
- **`ui::chips` is new**, and is `segmented` that wraps (`Row::wrap()`, which
  iced 0.14 has). A multi-select whose five options are one clipped line leaves
  the clipped option unpickable.
- **Submit is disabled until every required field is answered** — the alternative
  is spending a minute-long turn on a form the assistant has to ask for again.

Driven end to end, not only unit-tested: send → reply → approve → the habit
appears on the board in the same frame; reopen → the thread rebuilds with its
decision still marked approved; a fitness ask → the intake form with all five
field kinds → submit → the profile saves and the conversation continues; and
dismiss. Two of the six defects that pass found were the layout ones above.

### Coder screen (hearth migration) — what is built, and what is not

Where it stands (2026-08-06). **Screen::Coder** is a working coding agent over
an open folder: [`coder.rs`](desktop/crates/app/src/coder.rs) (state),
[`coder_view.rs`](desktop/crates/app/src/coder_view.rs) (rendering),
[`coder_tools.rs`](desktop/crates/app/src/coder_tools.rs) (the executor), plus
[`coder_notes.rs`](desktop/crates/app/src/coder_notes.rs) — `.agent/notes.md` in
the workspace itself, carried into every turn's system prompt through
`mode_instruction` so a new session does not re-derive the layout of a folder
the agent has already read — and
[`coder_git.rs`](desktop/crates/app/src/coder_git.rs), the checkpoint repo at
`.agent/git`. Both of the agent's own stores live in the user's project, where
they can be read, edited, committed or ignored without the app's help.

Six tools: `read_file`, `write_file`, `list_dir`, `search`, `repo_map`,
`run_command` — the last behind the approval gate, the middle two added by step
2 below.

**Almost no server code was written for it.** `app/coder/` already had the agent
loop *and* a delegation protocol nobody had ever built a client for: with
`delegate_tools` (or the `portal-desktop` client id) the server emits a
`tool_call` frame and parks the turn on a future keyed `(thread_id, call_id)`,
then waits up to 300s for `POST /coder/chat/tool-result`. The desktop is now
that client — so the model is wherever the proxy points and the files are this
machine's. `coder_stream` in the client crate reads those named SSE frames.

Consequences worth knowing before touching it:

- **Every `tool_call` must be answered.** A dropped frame does not error; it
  stalls the turn silently until the server's timeout.
- **The thread is created before the first turn streams**, because the tool
  result is addressed by `thread_id`.
- **`auto_approve_commands` is always false.** Commands are gated on a card
  showing the command itself, and there is no checkpoint to undo one with yet.
  `allow_commands` also resets to off at launch, on purpose.
- **The model picker is not a nicety.** The server's resolved default is
  `llama3`, which cannot hold a tool loop — it reads the file and then ends the
  turn silently. Provider/model live in the header and persist
  (`coder_provider` / `coder_model` in `settings.json`, with `coder_workspace`).
- **A decision is not final until the server acts on it.** `pending` survives
  the approve request and is only cleared by a frame off the resumed stream; a
  failed send puts the card back. Clearing it optimistically left the server
  holding the call and the UI with nothing to answer from, and every later send
  came back *"thread has a command awaiting approval"* — unrecoverable without
  a new session.
- **History is the server's, not a local store.** Coder threads are persisted
  server-side, so the sidebar is `GET /coder/chat/threads` and reopening one is
  `GET /coder/chat/thread`, rebuilt by `coder::rebuild_turns`. That rebuild and
  the live stream must produce identical rows — a reopened session that renders
  differently is one you cannot trust. The workspace root travels with the
  thread. (Contrast `history.rs`, which is a local file because the *assistant's*
  chat endpoint is stateless.)
- **The status line names what is being waited on**, most specific first: the
  user, then the tool in flight, then the model — with a seconds counter that
  keeps running while parked on the approval gate, which is the longest wait
  available here.

Verified live end-to-end, not just unit-tested: read → write → the edit runs;
approve → command runs → real output reported; and a reopened session renders
identically to the one it replaced and continues into the same thread. The live
check is `cargo test -p agent-platform-desktop -- --ignored delegation` (needs
the platform up and a loadable model alias — see the caveat below).

Four bugs came out of driving it rather than out of the tests, and all four are
the same shape — *a state the UI rendered as nothing*: an empty final assistant
message (read as a hang), an interrupted tool row showing a green tick, a
salvaged `run_command` offering a live Run button over an empty command, and the
desync above. Anything added here should be checked by running it, not only by
`cargo test`.

**What does not port at all.** Hearth's UI is a dead end here: Monaco, xterm.js
and the preview iframe all need a webview, and iced has none. What *does* port
is hearth's Rust half (`src-tauri/src/lib.rs`) and its ten import-free
`src/lib/*.ts` logic modules, which were kept import-free for exactly this — so
every step below is "port the logic, rebuild the surface", never a UI port.

Ordered by what the agent gets out of it, not by how hard it looks. 1–3 make it
a better *agent* and are **done**; 4–6 make it an *IDE* and are the ones worth
questioning before starting — 4 was, and came back half yes:

1. ~~**PLAN step.**~~ — shipped. One tool-free call at the top of
   `run_agent_turn`, behind `plan` on the send/retry bodies and the *Plan first*
   switch in the header (`coder_plan` in `settings.json`, **on** by default —
   this screen mostly runs local models, which is where hearth measures the
   gain). Tool-free is enforced by leaving `tools` out of the payload rather
   than by asking the model not to use them: a model handed tools uses them.
   Three things worth knowing:
   - **It never re-runs on a resume.** `resume_calls is None` is the guard —
     re-planning after a tool result plans around work already done.
   - **The plan persists as a plain assistant message**, without the prompt that
     asked for it. The desktop rebuilds a reopened session from that log, so the
     scaffolding would otherwise show up as a user turn nobody typed. The live
     `plan` SSE frame exists only so the stream can tell it from an answer —
     both render as the same row, which is what keeps rebuild == live.
   - **A plan is not an answer.** `answered` stays false, so a model that plans
     and then dies silently still trips the "cannot hold a tool loop" banner.
2. ~~**Search + repo map.**~~ — shipped, in `TOOL_SPECS` plus both executors
   (`coder/executor.py` and `coder_tools.rs`, constant for constant — the model
   must not be able to tell which side ran them). `search` is a literal
   case-insensitive tree walk, not ripgrep and not a regex: an offline app
   cannot assume the binary, and a model cannot tell a pattern that matched
   nothing from one that never compiled. `repo_map` is a **token walk, not
   hearth's regex** — hearth's is TypeScript-only and this repo is mostly
   Python, so a regex-per-language would have been two dialects to keep in sync
   across two languages. Column 0 only, in all three (Python, Rust, JS/TS):
   methods and `impl` bodies answer "what is in this file", where the map
   answers "where does this name live".
3. ~~**Checkpoints + diff review.**~~ — shipped:
   [`coder_git.rs`](desktop/crates/app/src/coder_git.rs) plus a timeline under
   the sessions sidebar and a review card above the composer. hearth's trick,
   with this repo's directory: `git --git-dir=<root>/.agent/git
   --work-tree=<root>`, so a turn's history never touches the user's own `.git`
   — and a workspace that is not a git repo at all gets checkpoints anyway.
   The baseline is taken at **Send**, chained *before* the turn streams: a
   baseline committed after the first tool wrote would contain that turn's own
   changes and show it as having changed nothing. The turn's commit is taken at
   `Done`, and **not** when `Done` arrives behind an approval pause — that turn's
   command has not run yet. A turn that changed nothing gets no row.
   - **A checkpoint failure never fails a turn.** git may not be installed; that
     reads as "Not checkpointing: …" under the timeline, never in the banner
     that means the turn itself went wrong.
   - **`\\?\` had to be stripped.** `canonicalize` returns verbatim Windows
     paths and git answers `fatal: not a git repository` for a directory that is
     right there. Found by the round-trip test, not by reading.
   - **Restore asks twice.** It is `git reset --hard`: the files come back and
     everything since goes — later checkpoints leave the timeline (reflog only),
     and the user's own unsaved edits go with them.
   - **Not built: per-hunk reject.** hearth reverts one hunk by feeding it back
     through `applyEdits`, the same whitespace-tolerant matcher its `edit_file`
     tool uses. There is no such matcher here — this agent rewrites whole files
     with `write_file` — so a hunk revert would be a second patch-application
     path with nothing to reuse. Whole-checkpoint restore covers the case that
     matters; add hunks if rejecting *part* of a turn turns out to be the common
     ask.
   - **`auto_approve_commands` is now unblockable, and still off.** A checkpoint
     undoes what a command wrote *inside the work tree*; it does not undo a
     `pip install`, a `curl`, or a delete outside the root. Flipping it is a
     decision about that gap, not about undo, and it has not been taken.
4. **File tree + viewer — shipped; the editor was refused.**
   [`coder_files.rs`](desktop/crates/app/src/coder_files.rs) plus a *Files* pane
   on the right of the Coder screen (sessions left, work centre, the folder as
   it is now on the right) and a viewer card above the composer.
   The decision this item asked for, taken: **the tree is worth it, the editor
   is not.** What is needed mid-turn is to *see* what the agent just wrote
   without alt-tabbing; a plain-text editor here would be a worse version of
   the real one already open on the same files, plus a save path and a
   conflict story. `iced::text_editor` is one widget away if that turns out to
   be wrong, and nothing in the module has to change for it.
   - **The tree is re-walked, never cached.** [`flatten`] reads the root plus
     the directories the user opened, and re-reads them on every change: a turn
     writes files, and a stale tree is worse than a slow one — this one is
     neither, because the walk is bounded by what is on screen. It re-walks when
     a turn ends, and by hand for the *other* writer (the user's editor, their
     git).
   - It skips exactly what `search` skips (`coder_tools::SKIP_DIRS` plus
     dot-directories), so the pane and the agent see the same workspace.
   - Reads are synchronous and bounded — one `fs::read` off a click, capped at
     512 KB, with a NUL in the first block sniffing binaries so a PNG says so
     rather than rendering as mojibake.
5. ~~**Terminal.**~~ — shipped, and as a *real* one:
   [`coder_term.rs`](desktop/crates/app/src/coder_term.rs) plus a drawer under
   the transcript. A short-lived run bar was tried first and deleted when this
   landed — two ways to run a command is one too many.
   **The emulator is `iced_term` over `alacritty_terminal`, not a port of
   hearth's terminal.** That is the whole finding: hearth had xterm.js doing the
   emulation and only needed the PTY half in Rust, so "port hearth's terminal"
   silently meant "write an ANSI emulator". `iced_term 0.8` targets **iced 0.14
   exactly** and brings alacritty's state machine — colour, cursor addressing,
   the alternate screen, scrollback, selection, mouse reporting, hyperlinks —
   over ConPTY on Windows. One dependency instead of the largest item on this
   list, and a better terminal than the hand-rolled one would ever have been.
   - **PowerShell, not the crate's `wsl.exe` default**, matching
     `assistant::run_command`: a `cd` that works for the agent and not for the
     user is a difference nobody debugs twice.
   - **Closing the drawer ends the shell** (`Session`'s drop shuts the PTY
     down). A hidden shell holding a dev server is a process the user cannot
     reason about.
   - **Its subscription is not gated on the screen or the window.** A
     `cargo build` keeps printing while the user reads the transcript or walks
     to another screen — it is a live process they started, not a view.
   - **Ids are never reused**, because the widget keys its event subscription on
     the id: a reopened drawer that reused one would be a new PTY wired to the
     old subscription.
   - **`Run in terminal` on every `run_command` row.** The agent ran something
     and it failed; one click puts the same command in the user's own shell, in
     the same folder, where it can be edited, re-run, and answer a prompt. This
     is the write-into-the-PTY half — `\r`, not `\n`, or the shell takes the
     line and never runs it.
   - Open, and the interesting one: **running the *agent's* commands in this
     terminal** instead of headless, so the user watches them live and can
     answer prompts. What stops it is knowing when a command finished — a PTY
     has no exit signal, so it needs shell integration (OSC 133) or a sentinel
     echo (`cmd; echo <mark>$?`) scraped from the grid. Worth it if headless
     `run_command` starts feeling blind; not before.
6. **Runner, and the preview that cannot exist — not building it.** Decided
   2026-08-06, after reading `run_dev` (`src-tauri/src/lib.rs:436`) rather than
   the one-line summary this item used to be. Every part of it exists to serve
   the preview iframe, and the iframe cannot exist here — iced has no webview:
   - the **OS-allocated port** and `--strictPort` exist so the iframe knows which
     URL to load; the **`Local:` readiness parser** (`src/lib/runlog.ts`) exists
     so it knows *when* to load it; and `--config` injects a generated vite
     config wrapping the project's own purely to bridge into that frame. With no
     frame, the user reads the URL off the terminal.
   - **`kill_tree` has an owner already.** Closing the terminal drawer drops the
     PTY, which takes the ConPTY job with it.
   - What is left is package-manager detection and an `install` when
     `node_modules` is missing — i.e. a button that types `npm run dev` into a
     terminal that is already open, in a repo that is mostly Python. That is the
     same shape as the Problems/LSP deferral below: a Node-only convenience for
     the language the agent here writes least, and step 5 already deleted a run
     bar on the principle that two ways to run a command is one too many.

   So the hearth migration is **closed**: the agent half (1–3), the tree and
   viewer (4) and a real terminal (5) shipped; the preview has no native path,
   the runner has no remaining purpose, and Problems/LSP stays deferred. Revisit
   the runner only if iced grows a webview, which would bring the preview back
   and with it every part listed above.

### Coder → daily driver (2026-08-19) — all five phases landed and driven

The hearth migration made this a working agent. Making it the screen coding
actually happens in is a second body of work, planned in
[`docs/coder-daily-driver-plan.md`](docs/coder-daily-driver-plan.md) against
what Cursor 2/3, Claude Code, Junie, Zed and Windsurf ship today. The field has
converged on one pipeline — **editable plan gate → streaming loop with
queue/steer → aggregated diff review → checkpoint rewind → verify** — and the
plan is five phases along it. All five have landed; the one thing outstanding on
this screen is Phase 4's third item, blocked on the terminal crate rather than
deferred, and it says so below.

**Phase 1 — trust the loop — is done.** Four items, all client-side; the wire
protocol was not touched:

1. **Stop.** The composer's Stop, and Esc on this screen (above the assistant's
   abort in `main::EscapePressed` — on Coder the agent turn is the work being
   watched, and the one that writes files). The ordering inside it is the whole
   feature: **answer the parked call, then drop the stream.** The server blocks
   on a future keyed `(thread_id, call_id)`, so a stream dropped while it holds
   one does not end the turn — it stalls for the full 300s delegation timeout and
   the thread refuses the next send until then. `State::outstanding` is the
   call_id owed; Stop posts `"Error: stopped by the user."` for it, and the
   loop's next emit then fails against a client that is gone, which is how a
   turn ends server-side.
   - **A stopped turn is checkpointed** exactly as a finished one is. A stop
     with no undo behind it is the worst of both.
   - `State::stopped` drops the frames already in the runtime's queue when the
     abort lands. Without it a late `Done` raised *"the model ended the turn
     without replying"* for a turn the user themselves ended — the same shape as
     the four bugs above: a state the UI rendered as a failure it wasn't.
   - `close_open_tools` takes its reason now: "stopped by you", not "the turn
     ended before this call was answered".
2. **Per-file revert.** `coder_git::{changes, revert_file}` plus a file list
   above the patch in the Diff dock tab. No patch matcher needed and none
   written: a checkpoint holds whole file contents, which is also why the unit
   is a file and not a hunk. `--no-renames` on purpose — with detection on, a
   rename is one row naming two paths and reverting "the file" would have to put
   the old name back too; off, it is a `D` and an `A` that each revert correctly.
   - **The baseline is not revertable** (`Changes::revertable`, off when the
     commit has no parent) — "revert to before the baseline" would delete files
     the user had before the agent ever ran. The button is not drawn, and
     `revert_file` refuses it as well.
   - **A checkpoint is taken before the revert**, so one click is enough: the
     file as it is *now* may hold edits made since, and the way back exists
     before the thing that needs one.
   - Also a **"changed files" bar above the composer** after a turn commits, so
     a turn says it touched files where the user already is rather than only in
     a sidebar pane they may not have open.
3. **Queue + steer.** Enter during a turn queues instead of erroring; chips
   above the composer, and pressing one takes it back into the box — the queue's
   only exit, which is why it needs no separate remove. **Stop & send** puts the
   correction at the front of the queue.
   - **The queue advances off the checkpoint, not off `Done`.** Same ordering
     rule as the baseline, from the other end: a next turn that starts writing
     before the last turn's commit is taken puts its changes in that commit.
   - **A stop does not drain the queue.** The user ended the work; what is
     behind it waits as chips.
4. **Follow mode.** Header toggle (`coder_follow` in `settings.json`, off by
   default), and a `write_file` that succeeded opens that file in the File dock
   tab. After the result, never on the call — opening on the call shows the
   version being replaced. Writes only; following every `read_file` moves the
   dock under the user for each file the agent skims.

Checks: `stopping_closes_the_open_row_and_clears_what_the_turn_was_holding`,
`stopping_does_nothing_while_a_decision_is_outstanding`,
`follow_ups_typed_during_a_turn_run_in_order_after_it`,
`a_stop_leaves_the_queue_alone_and_a_chip_goes_back_to_the_composer`,
`stop_and_send_puts_the_correction_at_the_front`,
`follow_mode_opens_the_file_the_turn_just_wrote`, and
`one_file_reverts_out_of_a_turn_and_the_rest_of_it_stays` (real git). **Not yet
driven live** — the four bugs above all came out of running it, so it needs a
session with a tool-capable model before this is claimed working.

**Phase 2 — the plan gate and the live checklist — is done.** Both items are
client-only again; `SendRequest.tools` and `plan` already carried them, so the
wire contract was not touched and `portal_desktop` is unaffected.

1. **Plan as an editable, gated artifact.** The header's plan checkbox is a
   three-state control now — `PlanMode::{Off, Inline, Gate}`. `Inline` is what
   the checkbox was (the server's own PLAN step, plan and execute in one turn);
   `Gate` sends a **tool-free** first turn (`tools: []`, which the server reads
   as "no tools" rather than "use the defaults"), and its answer lands in an
   editable `text_editor` card with **Run** and **Discard**.
   - **The ask rides in `mode_instruction`, not on the message.** Appending
     "write the plan and nothing else" to what the user typed would persist it,
     and the row a reopened session rebuilds is the message that was *sent* —
     the transcript would stop showing what the user actually wrote. That field
     is `max_length=4096` and a longer one is a 422 (no turn at all), so
     `coder_notes`' own cap came down to 3800 to leave the ask room, and
     `mode_instruction()` caps the sum as belt and braces.
   - **Run sends the edited plan as the instruction** (`Carry out this plan: …`)
     rather than nudging the model back at the plan already in its history: the
     edit is the entire reason the gate costs a round trip, and an unedited plan
     re-sent verbatim is the same turn for a few hundred tokens.
   - **The plan turn ends on the card, not on a checkpoint.** Nothing ran, so
     there is nothing to commit — and the queue does *not* drain behind it, or a
     follow-up would start while the plan it belongs to is still being read.
   - It also writes `.agent/plan.md` (Windsurf's trick — a file the user can
     open in their own editor), best effort: a read-only workspace must not cost
     the turn.
   - The card is **live-only state**. The plan itself is an ordinary assistant
     message, so a reopened session renders it as a row; what it does not render
     is a card asking a question that was answered hours ago.
2. **Live todo list.** A client-supplied `update_todos` spec goes out in
   `SendRequest.tools`, and `coder::update` answers that call itself — the
   checklist is screen state, so the arguments *are* the result and it never
   reaches the executor. Rendered as a pinned strip of badges above the
   transcript, ticking as the turn goes.
   - **The client now ships the whole spec list.** `tools` *replaces* the
     server's set, so the six have to travel with the seventh:
     `coder_tools::TOOL_SPECS_JSON` mirrors `server/src/coder.rs`'s constant
     verbatim, and both copies change together — a spec added server-side and
     not there is one this screen's turns never see. That is the one real cost
     of Phase 2, and it is what Phase 3's `edit_file` has to remember.
   - **The nudge lives in the tool's own description**, not in
     `mode_instruction` — the field with the 4096 cap the gate is already using,
     and the description is where a model actually reads it.
   - Rebuild: `rebuild_todos` reads the last `update_todos` arguments out of the
     persisted log, so a reopened session shows the list the turn ended on. A
     mangled call leaves the last good list up rather than blanking the panel,
     live and on rebuild alike.

**The server's iteration cap went 15 → 40** (`CODER_MAX_ITERATIONS`, still an
env override). One iteration is one LLM call plus the tools it asked for;
Python's 15 was sized for a local model that loses the thread after a handful of
rounds, and a frontier model spends that many just reading. Hitting the cap
mid-edit leaves the workspace half-changed, which is the worst way for a turn to
end — 40 is high enough that stopping there means the model is looping rather
than working.

Checks: `the_gate_plans_tool_free_then_runs_the_edited_plan` (real temp
workspace, including `.agent/plan.md`),
`the_notes_and_the_plan_ask_together_still_fit_the_field`,
`the_checklist_is_answered_here_and_rebuilds_from_the_log`, and
`the_advertised_tools_are_the_ones_something_can_run`. **Not yet driven live**,
for Phase 1's reason.

**Phase 3 — precise edits and context — is done.** One item touches the server,
and it does so without changing the wire contract.

1. **`edit_file`.** Exact-match replace (`path`, `old_text`, `new_text`) in
   **both** executors, byte-identical wording — `server/src/coder_tools.rs` and
   `app/src/coder_tools.rs`. The match must be unique; an `old_text` that appears
   twice is refused with the count, and nothing is written. One fallback and only
   one: **trailing** whitespace per line is ignored, because a model re-typing a
   block loses it constantly. Leading whitespace is not — a helpful re-indent is
   a silent corruption in Python and YAML.
   - **The splice works on byte ranges, not on rejoined lines.** `lines()` +
     `join("
")` rewrites every CRLF in the file, and a one-line edit that
     reports as a whole-file diff is worse than no edit tool: the checkpoint
     stops being readable, which is the feature this was for.
   - **Advertised from the desktop only.** It is in the app's `TOOL_SPECS_JSON`,
     not in the server's default `tool_specs()` — portal_desktop delegates and
     has no `edit_file` yet, so the default list would hand their models a tool
     their machine cannot run. Both executors already answer it, so promoting it
     later is one constant. Recorded in `docs/coder-delegation-protocol.md`,
     which now has a section on the caller-supplied tool list.
   - Follow mode and the transcript row treat it exactly as `write_file`.
2. **@-mentions.** `@path` in a message is expanded before it is sent —
   resolved through the same `resolve_in_root` the tools use, read with the
   viewer's cap, deduplicated, 32 KB per message.
   - **Expansion happens on the message, not beside it.** The transcript row and
     the persisted message are the same string, or a reopened session would show
     something other than what the model was asked. The view folds the tail away
     behind `MENTION_MARKER`, live and on rebuild alike, so the row still reads
     as a sentence.
   - **An `@` that is not a file is prose.** `ask @tanveer` inlines nothing and
     sends fine. There is no picker yet: typing the path works, and the picker is
     a convenience over a thing that already answers "read this file for me".
3. **AGENTS.md.** Loaded into `mode_instruction` after the agent's own notes and
   before the turn's ask — last thing written is what a model weights hardest,
   and the turn's instruction has to be last. A header chip says the workspace
   has one, because rules you cannot see steering a turn you did not expect is
   the whole complaint about agent memory.
   - That field is `max_length=4096` and now carries three things, so the notes
     came down to 1600 chars (block cap 2400) and `AGENTS.md` gets 1200. Each
     piece says out loud when it truncates.
   - Read, never written. The agent's own memory is `.agent/notes.md`; a tool
     that edits the file the humans instruct it with can instruct itself.

Checks: `an_edit_replaces_one_block_and_refuses_an_ambiguous_one` (in **both**
crates, CRLF preservation included),
`a_mentioned_file_rides_in_the_message_it_was_mentioned_in`,
`agents_md_is_carried_when_the_project_has_one`, and the spec-list test that
names every advertised tool. **Not yet driven live**, for Phase 1's reason.

**Phase 4 — graduated autonomy and a second reader — is done**, its third item
last and only after the dependency it was blocked on was forked. The first two
are client-only; the wire protocol still has not moved since the hearth
migration.

1. **Autonomy tiers and the command allowlist.** The header's Commands checkbox
   is a four-state control now — `Autonomy::{Off, Ask, Allowlist, Auto}`. `Off`
   is what the checkbox's off was (the model is offered no `run_command` at
   all), `Ask` is its on, `Auto` sets the `auto_approve_commands` the server has
   always had and this screen has always pinned false, and `Allowlist` is the
   one worth building: a command matching a saved rule is answered here, without
   a card, and the turn never stops.
   - **The rules are the desktop's, not the server's.** `auto_approve_commands`
     is `Auto` and nothing else — below that tier the server keeps pausing on
     every command and the allowlist answers the pause on this side. A server
     that pre-approved them would be trusting a list it cannot see, and the list
     lives in `settings.json` next to the workspace it belongs to.
   - **Per workspace, keyed by root path.** `cargo test` in a repo you own is
     not the same permission as `cargo test` in one you cloned this morning. The
     whole map is in `State`, so opening another folder picks up that folder's
     rules with no reload path of its own.
   - **A rule allows a program, not a line that starts with one.** `cargo test`
     matches `cargo test --lib` and not `cargo testbed` (a word boundary, not a
     prefix), and **never** a command carrying a shell operator — `;`, `&`, `|`,
     a backtick, `$(`, a redirect, a newline. `cargo test; rm -rf /` starts with
     `cargo test` and is not the command the rule was written for. That is the
     whole security value of the tier, and the one check in this phase that had
     to exist.
   - **"Always allow" writes the verb, not the line.** `cargo test --lib` saves
     `cargo test`; `ls -la` saves `ls`, because a flag is not a subcommand and
     nor is a path. The whole line would only ever match the command it came
     from; the program alone would let `cargo` come to mean `cargo publish`. The
     button spells the rule out — "Always allow cargo test" — rather than saving
     an invisible one, and it moves the tier to `Allowlist` with it, since a
     rule nothing consults is a button that does nothing.
   - **A row nobody read says so.** The resumed turn emits the real `tool_call`
     for whatever a rule answered, and that row is the only place the user ever
     sees the command — unmarked, an approval gate nobody saw looks exactly like
     one somebody read. It reads `$ cargo test --lib — allowed by rule`.
   - The tier switch carries the warning this file wrote when checkpoints
     landed: undo does not cover `pip install`, or a write outside the root.
2. **Review pass.** *Ask the model* on the "changed files" bar sends that
   checkpoint's diff back as a fresh tool-free turn — Amp's Oracle in its
   minimum viable form, and the header's model picker is already the "review it
   with something stronger" half. No new protocol, no second thread.
   - **A third turn kind, not a second bool.** `planning: bool` became
     `TurnKind::{Work, Plan, Review}`: two of the three run no tools and they
     are not the same thing — the gate's plan ends on a card asking to be run, a
     review ends like any other answer — and "tool-free *and* a plan" is a state
     two bools could not rule out.
   - **The review's `@`s are left alone.** It is the one turn whose prompt this
     screen builds rather than the user typing it, and `@@ -1,7 +1,7 @@` is not
     somebody pointing at a file. The patch is capped at 48k chars and says out
     loud when it was cut.
   - It waits for the turn it is about: a review asked for mid-turn does
     nothing, because the turn still running is still changing the files the
     diff describes.
3. **Agent commands in the visible terminal — landed 2026-08-19**, once the
   dependency it was blocked on was forked. An approved `run_command` is typed
   into the drawer the user can see, the dock switches to it, and the row says
   `— in the terminal`. Headless stays the fallback and is what runs when there
   is no folder or the drawer will not open.
   - **The blocker was real and the note above named the wrong method.**
     `Backend::selectable_content()` returns the *current selection*, not the
     screen — scraping with it would have meant hijacking the user's own
     selection. The grid is `Backend::renderable_content().grid`, and
     `Terminal::backend` is `pub(crate)` with no accessor beside it. So the fork
     carries **one method**, `Terminal::text()` — the buffer as `Vec<String>`,
     scrollback first, trailing blanks trimmed, `Backend` still private —
     [`tanvoid0/iced_term@feat/terminal-text`](https://github.com/tanvoid0/iced_term/tree/feat/terminal-text),
     wired in by the `[patch.crates-io]` block in `desktop/Cargo.toml`. Upstream
     is alive (20 commits past the 0.8.0 release, community PRs merging), so
     that block is meant to come out rather than live there.
   - **Every alternative that avoids the fork breaks the thing the PTY was for.**
     Teeing the command to a file we read ourselves needs no crate change and is
     the obvious dodge — but a pipe takes the child off the tty, and a program
     with no tty does not prompt, does not colour and does not paginate. The
     interactivity *is* the feature; a tee buys the watchable half by throwing
     away the promptable one.
   - **Two markers, and the second one carries a number.** `wrap` brackets the
     command with `@@AGPRUN-BEGIN:<id>` / `@@AGPRUN-END:<id> <status> <code>`,
     and `scrape` reads the output back out from between them. The shell echoes
     the line that was typed, so that echo is on screen carrying both markers
     *before* the command runs — which is why a marker only counts at the start
     of a row (the echo sits behind a prompt) and the closing one only counts
     when a **number** follows it (the echo carries the unexpanded
     `$(if($?){0}else{1})` or `%s`). Without that second test a command long
     enough to wrap onto the next row ends itself before it begins.
   - **The two shells disagree about what an exit code is**, so both are sent:
     `sh` has `$?` and it covers everything, while PowerShell has `$?` for "did
     that work" and `$LASTEXITCODE` for "what number did the last native exe
     return" — a cmdlet sets only the first. The reader prefers the number when
     there is one.
   - **The poll rides the clock that was already ticking.** No timer of its own:
     `Message::Tick` fires once a second while a turn is in flight, and that is
     what reads the grid. Same 180s cap as the headless executor — where a
     command runs must not change how long it is allowed to take.
   - **A stop stops watching, it does not kill the command.** It is running in
     the user's own shell, where they can watch it finish or Ctrl-C it; killing
     a shell to end a turn is a bigger thing than that button says. Same for the
     timeout, which tells the model where the command went rather than that it
     vanished. Closing the drawer under a running command *does* end it, and the
     call is answered saying so — the server is blocked on it either way.
   - **Known: the markers are visible in the user's terminal.** That is what
     sentinel echo costs, and the alternative is not reading the screen at all.

Checks: `a_rule_does_not_stretch_past_the_command_it_names` (word boundaries and
every shell operator), `the_saved_rule_is_the_program_and_its_verb`,
`an_allowed_command_runs_without_a_card_and_the_row_says_which_rule`,
`always_allow_saves_the_rule_for_this_folder_and_turns_the_tier_on`,
`the_review_pass_hands_the_diff_back_tool_free`,
`the_review_pass_waits_for_the_turn_it_is_about`, and for 4.3
`the_shells_echo_of_the_command_is_not_mistaken_for_its_output`,
`a_marker_with_no_exit_code_after_it_is_not_the_end_of_anything`,
`a_failure_keeps_the_number_the_process_actually_returned`.

4.3 was **driven live** the day it landed, `llama3.1:8b` on the sandboxed
daemon: an approved `python main.py` ran in the drawer with the row reading
`$ python main.py — in the terminal` and the model answering off its output;
then `pause` stopped on *Press Enter to continue…* with the row still
`running…` and the composer counting, and pressing Enter **in the terminal**
finished the turn — the model's next line was "The program is paused as
requested." That is the accept criterion whole: it streams where the user can
see it, its output reaches the model, and the user can answer it.

**Phase 5.3 — fork / handoff — landed ahead of the rest of the phase**, which it
does not depend on. *Hand off to a new one*, beside *New session*, spends one
tool-free turn asking the model to write instructions for whoever picks the work
up, then opens a fresh thread in the same folder with that text **in the
composer**. Not sent: a handoff nobody read is the restart tax with extra steps,
and this is the one place the user gets to correct what the model thinks it was
doing. The summary also stays in the old thread as its last row, which is where
anyone would look for it. `TurnKind` gained a fourth member; an empty summary
leaves the session standing and says why, because throwing a session away on a
failed call is the only failure here that loses work.

**5.2 — worktree isolation — landed first, because the order in the plan is
wrong.** An *Isolate* toggle in the header, drawn only for a real git
repository, runs the session in `git worktree add --detach .agent/worktrees/<n>`
and points `root` at it. That one swap *is* the feature: the tools already
resolve against `root`, the tree already walks it, the checkpoints already live
in it, so nothing downstream learns a new concept. What must not follow `root`
is `settings.json` — a saved workspace pointing at a worktree reopens the app in
a scratch folder that may not exist — so `project_root()` is what gets written.

- **`.agent/` goes in `.git/info/exclude`, not `.gitignore`.** The ignore file
  is the project's and belongs to whoever owns the project; `info/exclude` is
  this clone's alone. Without it the worktree is untracked junk in the user's
  own `git status`.
- **Merge back stages first.** `git add -A` then `git diff --cached`, because a
  file the agent *created* is untracked and a plain `git diff` would not mention
  it — which is most of what an agent does. Applied with `git apply --3way`, so
  it lands whole or refuses whole.
- **Turning it off does not delete the checkout.** Work that has not been merged
  is not the toggle's to throw away.
- A header badge says *isolated checkout* whenever `main_root` is set: working
  somewhere other than the folder the user opened is the one state this screen
  must not be quiet about.
- Known: `git apply` runs the repo's own `core.autocrlf`, so a merge back on
  Windows can rewrite line endings. That is git doing what it does to every
  commit in that repo, and the test asserts on trimmed content for it.

**5.1 — the sessions board — landed 2026-08-19, and Phase 5 is closed with it.**
N sessions run at once, each its own thread, transcript, stream, queue, tier and
checkpoints; the Sessions pane is the board, one row per live session with
`● running / ⏸ waiting / ✓ idle` beside it, and the server's past threads under a
divider below. [`coder_board.rs`](desktop/crates/app/src/coder_board.rs) is the
whole of it — 200 lines beside a 4000-line screen, and the shape is why:

- **`coder::State` did not move.** It was already one session's worth of state;
  what it lacked was a *second* one. `Board` holds `Vec<Slot>` and derefs to the
  active session, so `state.sending`, `state.pending`, `state.root` still read
  the session on screen from `main.rs`, from `coder_view` and from the 4000-line
  `update` — none of which learned there is more than one. The plan called this
  an XL refactor of `State` into `Session` + screen state; a `Deref` made it an
  S. What that costs is a struct that is not a smart pointer pretending to be
  one, and the payment is that the field names are unambiguous — an inherent
  field on `Board` wins over the session's, and there are none that collide.
- **Every task a session starts is tagged with that session's id**
  (`Message::For(u64, Box<Message>)`), and routed back to it. Without it a
  background stream's frames land in whichever tab is in front — one transcript
  written into another, which is the failure mode a board has and a single
  session cannot. An **untagged** message goes to the active session, which is
  exactly right for the ones `main` starts on entering the screen.
- **A frame for a session that has been closed is dropped**, not applied to the
  active one. Ids are handed out and never reused, so a late frame cannot land in
  the session that took its place in the list. Closing answers the parked call
  first (`Message::Stop`, then remove) — a stream dropped while the server is
  blocked on it stalls that turn for the full 300s delegation timeout.
- **One turn per checkout, refused rather than queued.** The shadow-git repo is
  one per folder, so two turns writing it would interleave `commit_all` and each
  checkpoint would hold the other session's work. `State::busy_roots` is
  refreshed by the board before every message it hands down, and `start_turn`
  refuses with the way out named: *"Another session is running a turn in this
  folder. Wait for it, or run this one in its own checkout with Isolate."* That
  is 5.2 doing the job 5.1's accept criterion assumed it would.
- **The clock and the spinner are the board's, not the tab's.** `Tick` and
  `AnimTick` are broadcast to every session, and `main`'s subscription gates on
  `Board::any_busy` — otherwise switching back to a background turn shows one
  that has apparently been running for no time at all.
- **The completion toast is per session and names it.** `main` diffs
  `Board::running()` across the update and posts for whichever session left it,
  because with a board the turn that ends is usually *not* the one on screen.
- **An allowlist rule is copied to every session on save.** The rules are the
  folder's, and `main` persists the tab in front — a rule left in one tab is one
  the next save drops.
- **New session opens a tab beside this one**; the handoff still replaces in
  place, because it is the same work carried over and two tabs for it would
  leave a dead one behind. Closing the last session starts a fresh one rather
  than leaving the screen with nothing behind it.

Checks: `two_sessions_on_two_folders_each_run_their_own_turn`,
`a_second_session_will_not_run_a_turn_in_a_folder_already_working`,
`a_session_waiting_on_an_approval_still_holds_its_checkout`,
`a_frame_for_a_closed_session_lands_nowhere`,
`closing_the_last_session_starts_a_fresh_one_instead`,
`an_always_allow_rule_reaches_every_session`,
`selecting_a_session_puts_it_on_screen`.

**Known, and left:** the terminal drawer subscribes only to the session on
screen, so a background session's PTY takes no input while it is behind. One
shell at a time is what the drawer already was; a second one is a dock change,
not a board change.

##### Driven live, 2026-08-19 — `llama3.1:8b` over Ollama, sandboxed daemon

`AGENT_PLATFORM_APP_DIR` again, port 18499, two scratch workspaces. What the
screen actually did:

- **Two sessions, two checkouts, at the same time.** Session one parked on the
  approval card for `python main.py` in the project; session two, *Isolate* on,
  streaming in `.agent/worktrees/<n>` with its own tool rows. The board read
  `⏸ waiting` and `● running` on the two rows, and the header showed a
  different root for each. That is 5.1's accept criterion on screen.
- **Approving in one did not touch the other.** *Run* resumed session one's turn
  into session one's transcript; session two kept its own, and its thread is a
  separate row in Past sessions.
- **The refusal fires on the real screen.** With session one parked, a turn in a
  second session on the *same* folder was refused and nothing ran — no row, no
  thread. A parked turn is what found the bug in it: `sending` is false there,
  so the first cut of `busy_roots` let a second turn start beside a turn whose
  checkpoint had not been taken. It counts `pending` too now.
- **A background session's clock keeps counting** — the parked session read
  *waiting for you… 156s* on the way back to it, having been off screen for most
  of that.
- **The completion toast names the session**: *Coder — read main.py and summarise
  it in one line — The main.py file…*, with the badge on the Coder nav item, for
  a turn that finished while the user was on Home.

Two things worth knowing for the next person driving this app, on top of the
ALT-tap and the `CopyFromScreen` traps already recorded: **`cargo test` does not
rebuild the `.exe`** — twenty minutes went into a guard that "did not fire"
against a binary built before the guard existed; and **a clicker that clicks
twice to beat the eaten-first-click will hit whatever the first click reflowed
into that position** — that is how *New session* became *Hand off to a new one*
and ran a handoff turn nobody asked for. Single click, screenshot, then click.

#### Driven live, 2026-08-19 — `qwen3-coder:30b` over Ollama

The debt every phase above carried ("not yet driven live") is **partly paid**.
An isolated daemon (`AGENT_PLATFORM_PORT=18499`, its own SQLite file, so none of
this touched the real install) and a scratch workspace, driven through the real
`/coder/chat/{stream,approve,tool-result}` with a Python stand-in for the desktop
executor — the protocol and the model are real, the iced app is not.

What that proved, none of which the unit tests can:

- **`edit_file` is used by a real model and lands surgically.** Asked for one
  change in `main.py`, the model went `read_file → edit_file` and the file
  differs by one line. Phase 3.1's whole case.
- **The client's 8-spec list is accepted and `tools` really does replace the
  server's set** — `edit_file` and `update_todos` are only callable because the
  desktop sent them.
- **`tools: []` is genuinely tool-free.** The same ask that produced a
  `list_dir` with the list attached produced no calls at all with `[]`. The plan
  gate (2.1), the review pass (4.2) and the handoff (5.3) are all built on that
  one behaviour, and none of them had ever been watched doing it.
- **`mode_instruction` reaches the model.** A turn carrying "reply with exactly
  one word: BANANA" replied BANANA. The agent's notes, `AGENTS.md` and the
  plan-gate ask all ride in that field, so this was the single load-bearing
  assumption in Phases 2 and 3.
- **The approval pause and its resume behave as the code assumes.**
  `approval_required` arrives carrying `{"command": "python main.py"}` — what
  the card shows — and approving **re-emits the call as an ordinary
  `tool_call`**, which is the only place a row for it ever comes from and
  therefore where 4.1 hangs its "allowed by rule" label.
- **`auto_approve_commands: true` never pauses**, and the command runs.
  `Autonomy::Auto` is real rather than a field nobody had set.

**One real defect, and it is the shape this screen keeps producing.**
`qwen3-coder:30b` emitted `run_command {}` — the call with no command in it —
then corrected itself on the next step. Two things were wrong with that:

- The desktop executor **spawned a shell to run nothing**, where the server's
  executor has always answered `Error: run_command requires a non-empty
  command`. Two executors that are supposed to be identical, constant for
  constant, differed on the one input a model actually produces. The guard went
  into `assistant::run_command` — the function *both* the Coder screen and E.V.
  route through — with the server's wording verbatim, rather than into
  `coder_tools::execute` where only one caller would have been fixed.
- The transcript row for it read **`$ `**. Under `Ask` the card catches an
  unreadable call and offers no Run button, but under `Autonomy::Auto` there is
  no card and that row is the only thing the user sees. It says
  `run_command (unreadable)` now, the same words the refusal already used.

Checks: `a_command_with_nothing_in_it_is_refused_rather_than_spawned`,
`a_command_with_nothing_in_it_still_names_itself`.

**The checklist, and the one invariant it could have broken.** A three-step task
(`add subtract`, `add multiply`, `docstring both`) went
`read_file → edit_file × 3 → read_file → update_todos` — three consecutive
surgical edits, and the first time a real model has called `update_todos` at
all. On one- and two-step tasks it does not, which is what its description asks
for and why this needed a task big enough to earn a list.

Reading that thread back is where the interesting part was: the live `tool_call`
frame carries `arguments` as an **object**, and the very same call read back out
of `GET /coder/chat/thread` carries it as a **JSON string**. `call_args` already
unwraps both, so the panel does agree with itself — but that was design, not
evidence, and "rebuild == live" is the constraint the whole screen is built on.
`the_checklist_rebuilds_from_the_shape_the_server_actually_stores` now pins it
against the bytes the server actually wrote, captured from that turn.

Also seen, not a bug and not new: the model leaking `</tool_call>` into its
prose — the thing the unreadable-command card was built for, still live on a
30B coder model.

**Then the app itself, which needed a way to run it that is not the user's own
data.** `AGENT_PLATFORM_APP_DIR` now moves the whole data root — database,
`settings.json`, `master.key`, chats, memories. `AGENT_PLATFORM_PORT` moves only
the port, and setting `%APPDATA%` does nothing, because `dirs::config_dir` asks
Win32 for the known folder rather than reading the variable. That gap is why
every previous "drive it live" meant driving over live data, and the run that
closed it was a run that could not start at all: **the installed database was
corrupt** — the main file passes `integrity_check`, the 383 KB `-wal` replayed
does not (duplicate page references, three indexes with wrong entry counts), so
`GET /system/status` 500s and every gated screen stays padlocked. Left alone,
not repaired.

On a clean data root, with `qwen3-coder:30b` behind it, the screen renders and
behaves:

- the four-way tier control sits beside the three-way plan control without
  breaking the header row, and a settings file carrying the **legacy
  `coder_plan: true` with no `coder_plan_mode`** comes back as *Plan first* —
  the migration path, on a real file, for the first time
- the approval card draws **No / Always allow `<rule>` / Run**, and pressing the
  middle one flips the tier to *Allowlist* and puts
  `$ python main.py — allowed by rule` in the transcript. 4.1 end to end,
  on screen
- the composer becomes *Queue a follow-up* with `waiting for you… 25s` beside it
  while the card is up — 1.3's queue mode and `activity()`, both live
- the header's `AGENTS.md` chip is not decoration: the reply ended **BANANA**,
  which is 3.3's accept criterion ("a rule in AGENTS.md observably steers a
  turn") and had never been watched happening
- *Hand off to a new one* appears beside *New session* once a thread exists (5.3)

**Fixed from what the screen showed:** the card offered *Always allow python
main.py*. `rule_for` rejected a path with a separator in it but not a bare
filename, so the saved rule only ever matched that one script. A dot is the
test — `cargo test` and `npm run` have none, `main.py` and `app.js` do.

Still not driven: Stop mid-command, the todo strip, the changed-files bar and
follow mode moving the dock.

Deliberately deferred rather than listed above: **Problems / LSP.** Hearth talks
to `tsserver` directly and its "not checked ≠ no errors" rule is load-bearing —
but it is TypeScript-only and this repo is mostly Python, so the port would buy
diagnostics for the language the agent here writes least.

**The `provider` field: the code does route on it.** This was listed as "appears
to be ignored by the proxy", which the handler contradicts —
`chat_completions` (`llm_proxy/routes/llm.py`, the `raw_provider_hint` branch)
pins the hint, **400**s an unsupported id and **503**s an unconfigured one, and
only falls through to resolving the provider *from the model alias* when no hint
is sent. The two bases are distinct here too (`OLLAMA_API_BASE` 11434,
`LM_STUDIO_API_BASE` 1234, both in `config/agent_platform.yaml`). So the likely
reading of what was seen: the header's provider box was empty, the request
carried no `provider` at all, and the alias picked the backend. Read from the
code, **not re-driven live** — the remaining way to be sure is one turn with the
box set and the daemon's log next to it.

What is *not* in doubt: an Ollama-style alias (`gemma4:latest`) sent at an LM
Studio backend crashed `llama-server`
(`GGML_ASSERT(n_inputs < GGML_SCHED_MAX_SPLIT_INPUTS)`) rather than 404ing,
taking the platform server down with it. Aliases that load are the ones
`http://127.0.0.1:1234/v1/models` lists, and the header drops a model the newly
picked provider does not offer for exactly this reason.

### In-process inference — next steps

Where it stands: [`local_llm.rs`](desktop/crates/app/src/local_llm.rs) is the
engine (one owned thread, model resident, KV prefix reused across turns),
[`inference.rs`](desktop/crates/app/src/inference.rs) is the single dispatch
point, and everything is behind the `local-llm` feature, off by default.

Point it at a GGUF in **Settings → Status → Local model** (file picker, context
box, and a badge saying which engine answered the last turn); both keys are read
once at the first local turn, so a change wants the app restarted — the card has
the button. The same two keys by hand:

```jsonc
// %APPDATA%\com.tanvoid0.agentplatform\settings.json
"local_model_path": "E:\\...\\blobs\\sha256-<the gguf>",
"local_n_ctx": 8192
```

```bash
cd desktop && cargo run -p agent-platform-desktop --features cuda
```

Windows CUDA build needs `CUDA_PATH_V13_3` set (MSBuild reads the *versioned*
variable) and `%CUDA_PATH%\bin\x64` on `PATH` at runtime, or the exe dies with a
bare `0xC0000135`. The model-backed test is opt-in:

```bash
AGENT_PLATFORM_TEST_GGUF=<path.gguf> cargo test -p agent-platform-desktop --features cuda -- --ignored --nocapture
```

Ordered by what unblocks what:

1. ~~**Settings UI**~~ — shipped: the *Local model* card on Settings → Status
   (`screen.rs::local_llm_card`, `#[cfg(feature = "local-llm")]`), picker +
   context box + last-turn engine badge from `inference::last_turn_was_local`.
   It sits on Status rather than Model ops because that is the one model surface
   that still works with the server down.
2. ~~**Unload / VRAM policy.**~~ — shipped: the engine thread now owns the
   weights instead of a `OnceLock`, so it can drop them. They unload after five
   idle minutes (`IDLE_UNLOAD`, Ollama's `keep_alive` default) and reload on the
   next turn; `local_llm::unload()` frees them early, wired to the *Free VRAM*
   button on the Settings card and fired automatically when a model-ops build
   job starts. Not covered: a partial offload, and a `keep_alive` setting —
   the timeout is a constant.
3. ~~**Installer ships the DLLs.**~~ — shipped: the `.iss` takes
   `target\release\*.dll` (`skipifsourcedoesntexist`, so a default build packages
   none), and `build_installer.py` passes `AGENT_PLATFORM_FEATURES` through to
   cargo and refuses to package a `local-llm` build whose DLLs are missing:

   ```powershell
   $env:AGENT_PLATFORM_FEATURES = "cuda"; python scripts/build_installer.py
   ```

   Still open: a CUDA build does **not** carry the CUDA runtime (cuBLAS alone is
   hundreds of MB), so it only installs onto machines that already have the
   toolkit — the script warns. A CPU-only `local-llm` build has no such
   dependency, and 11 tok/s against 123 is the price.
4. ~~**Tool calls.**~~ — shipped, but by *recognition*, not GBNF. The
   definitions go in as an extra system turn (this binding's
   `apply_chat_template` takes no `tools`), the model is asked to answer with
   one JSON object, and a reply opening `{"name"` is held back instead of
   streamed, then handed over as the same `ChatChunk::ToolCall` the server's
   relay emits — anything else streams as prose from its first character.
   Verified against Qwen3-Coder-30B: a valid call, nothing leaked into the
   transcript.

   The GBNF part was tried and removed. In llama-cpp-2 0.1.154 a lazy grammar
   either builds and never fires (`grammar_lazy`, and `grammar_lazy_patterns`
   with an anchored pattern) or is rejected outright as `NullGrammar`; code that
   never engages is worse than none, and a call that will not parse already
   falls back to text. Revisit when the binding's trigger works — the
   `ponytail:` note in `local_llm.rs` marks the spot. Also not covered:
   per-tool argument schemas, and parallel calls (one call per turn).
5. ~~**Point the Python side here.**~~ — shipped, and cheaper than the shape
   this list assumed: the desktop serves an OpenAI-compatible
   `/v1/chat/completions` + `/v1/models`
   ([`local_server.rs`](desktop/crates/app/src/local_server.rs), hand-rolled
   HTTP on loopback, one thread per connection), so the proxy's existing
   OpenAI-compatible provider reaches it with **no Python change at all**:

   ```jsonc
   "local_server_port": 18411   // settings.json, or the Settings card; 0 = off
   ```
   ```bash
   LM_STUDIO_API_BASE=http://127.0.0.1:18411
   ```

   Off by default, and the coupling is still real: with the endpoint configured,
   server-run agents on that provider fail when the app is closed. Loopback and
   unauthenticated, the same boundary Ollama and LM Studio draw. `tools` go in
   and `tool_calls` come back out (`finish_reason: tool_calls`), so a server-run
   agent turn works, not just prose. Not covered: `/v1/embeddings`, and a
   caller's `model` is ignored — whatever GGUF is configured answers.

6. ~~**Getting a GGUF without Ollama.**~~ — shipped: a *Download* row on the
   same Settings card takes `owner/repo/file.gguf` or any Hugging Face link,
   streams the file into `<data dir>/models/`, and sets `local_model_path` to
   it. [`model_download.rs`](desktop/crates/app/src/model_download.rs) — a
   redirecting `GET` and a `.part` rename, no registry client: HF's own model
   page is a better catalog than anything this could draw, and what it gives
   you is a link to paste. A `/blob/` URL (what the address bar holds) is
   rewritten to `/resolve/`, since otherwise you silently download HTML.
   Cancel is `Task::abortable` — dropping the stream *is* the cancel, and the
   handler sweeps the `.part` the drop leaves mid-write. Not covered: repo
   search, resume, hash verification, and gated repos — nothing sends a token,
   so those come back as the 401 they are. Until this,
   the only pull path was Ollama's, which is what pointed every install at a
   `blobs/sha256-…` file.

   **Driven, not only unit-tested**: two opt-in tests pull llama.cpp's own
   19 MB tinyllamas GGUF off HF. The first takes it whole — redirect followed,
   four progress ticks, the `.part` gone and `GGUF` at byte zero, which is the
   assertion a redirect page served as the file would fail. The second drops
   the stream one tick in and checks the half-file is sitting at exactly the
   path the cancel handler sweeps, since two spellings of that name is the one
   way cancel can leak gigabytes.

   ```bash
   cargo test -p agent-platform-desktop model_download -- --ignored --nocapture
   ```

   **The engine is a pick in the chat header now.** It was reachable only by
   leaving *both* header boxes empty — which is what `inference::chat_stream`
   routes on, and which nothing on screen said. E.V.'s provider box lists
   `local` first when the engine can answer (feature in, GGUF configured, file
   present — the same condition the router uses), picking it clears both fields
   rather than storing an id the proxy would 400 on, and the model box goes
   empty because the local engine ignores `model` and setting one would send
   the turn to the server. The Coder screen's picker is untouched: its agent
   loop runs on the server, so in-process is not a choice there.

   **The card renders in a default build too**, as "Not built into this copy"
   plus the cargo line — it used to be `#[cfg]`'d away entirely, and a card
   that is simply absent reads as "this app cannot do that" when the truth is
   one feature flag. Nothing else in the UI mentions in-process inference
   exists: the Providers screen lists the *server's* catalog (Ollama, LM
   Studio, AIMLAPI…), and the in-process engine is deliberately not in it —
   it answers the desktop's own chat through `inference.rs`, and only reaches
   the server at all through `local_server_port`.

*The pre-desktop refactor checklist (`docs/refactor-handoff-followup.md`) is
complete and the file is deleted: services extracted (`app/services/`),
`datetime.utcnow()` gone, route layers thinned; its frontend items died with
`web/`.*

---

*Refreshed: 2026-08-04 — rewritten for the native-desktop / headless-API
reality; stale references to `web/`, `/ui`, `/app`, the Jinja shell and the
pixel office removed (pixel office permanently deferred with the web app).
Second pass the same day: Plans screen and the Piper speech service marked
shipped, ADR 0006 added to the backlog. 2026-08-05: ADR 0006 spike closed and
its first two slices shipped; "In-process inference — next steps" added.
2026-08-06: ADR 0007 — `agent-platformd` fronts the Python server, auth +
`/health` + projects + teams + todo CRUD + workflow CRUD migrated; "Rust server
migration — next steps" added and the runbook rewritten for two binaries. Second
pass the same day: `llm_proxy/` scoped route by route — it owns no tables and
writes no usage rows, which the first draft of step 1 had wrong. Third pass:
the Coder screen landed — the hearth migration's agent half — with session
history, and its section records what was left unbuilt as six ordered steps,
including the one feature (preview) that has no native path at all. Fourth pass:
hearth steps 1 and 2 shipped — the PLAN step and the `search` / `repo_map`
tools — and the "`provider` is ignored" note corrected against the handler,
which does route on it. Fifth pass: step 3 shipped — checkpoints over `.agent/git`
with a timeline, a diff review and a two-press restore — which leaves the three
agent-quality items done and only the IDE surfaces (4–6) open. Sixth pass: step
4's tree and viewer shipped and its editor refused; 5 shipped as a *real*
terminal — `iced_term` over `alacritty_terminal`, which is why it stopped being
the largest item on the list — so what is left of hearth is the runner and the
preview that cannot exist. Seventh pass: the whole embedded
LLM proxy moved to Rust — all nine `/v1` routes across six steps — and with it
`request_id.rs` and `dotenv.rs`, the second of which fixed a real split where the
daemon could not see the master key or `DATABASE_URL` its Python child was using.
Then migration step 2: the workflow engine and its scheduler, `agent/apply` (with
`merge_profile`) and `planning-form/submit`, which leaves `todo_items`,
`workflows` and `workflow_runs` with a single writer each. Its scope note was
rewritten twice against the code — once to move the hazard off the routes it was
attributed to, once to withdraw a scheduler clobber that does not exist. Eighth
pass: the `/v1` cutover thrown — the Python child now reaches the proxy through
Rust and stops mounting its own copy — plus the last two LLM-backed routes,
`workflows/assist` and `todos agent/chat`, over a new `llm::complete_internal`
and a `context_budget.rs` port. Workflows is whole; todos is one route short —
`agent/step`, which waits on the action orchestrator. The `datetime_to_sql` gap this doc had listed as
unsettled turned out to be a real bug — a numeric offset survived into an indexed
TEXT column — and is fixed in `wire.rs` along with a second, narrower one it was
hiding. Step 3 was then scoped against the code the way steps 1 and 2 were, and
it moved three things: the closure is half what the one-liner implied, four of
its seven sub-steps need no tokio task at all, and its two-writer hazard is on
the API-token counters rather than on `process`; its first four sub-steps then
shipped in the same pass — `processes.rs` (the three reads,
`projects/{id}/processes`, `cancel` and the SSE stream) plus `spawn-process`,
which brings `todo_items` down to a single writer; and then the other three —
the DAG executor with `dag_schema` and startup recovery, the eight scheduling
routes, and `agent/step` over `action_orchestrator.rs` — so **step 3 is done
whole** and the migration is at step 4 (assistant, chat, coder) with
`system_routes` behind it. The executor inherited no test suite, which is why
that pass added 71 `cargo test` cases; what those still do not cover is written
down above rather than left implied. Ninth pass, on the desktop side
rather than the server: the assistant roadmap's last unported UI landed — the
planning chat, as a pane inside Agenda rather than a screen of its own — which
closes that backlog item and leaves the UI at parity with what the server has
served since Phase 7. Its section is above. That pass also closed three
long-standing gaps by doing rather than reading: the UX sweep (Coder's header
was dropping its own controls off the right edge, Plans' columns were cut off by
iced's floating scrollbar, and raw model JSON was reaching users as prose from
one line in `action_orchestrator/engine.py`); the prove-domain figure, re-measured
at **20 of 50, identical on both**; and the installer, which built and installed
fine and whose **uninstaller was silently stranding ~50 MB** of bundled Python —
found only because the round-trip launched the app in between. Tenth pass: the
Postgres port started — `db.rs`, the dual-pool `AppState`, and `projects.rs`
converted and proven — with the three `sqlx::Any` findings that shaped it
written down where the next thread will hit them. **It is mid-flight: eight
domain files to go, and the section above is the resume point.**

Eleventh pass, alongside that one and deliberately kept clear of it: step 4
opened. It was scoped first, in two notes, and the scoping was worth more than
the code — it found that the assistant never wrote `api_tokens` (a literal
`None`), that playground is therefore load-bearing and appears in no step, that
`todo_items` still has a Python writer the step-3 note had declared gone, and
that coder's parked tool-call future fixes its granularity at
five-routes-or-nothing. Then the one blocking decision, taken before any code:
the **tokenizer is real** now, because `context_usage` is a body field and the
same estimator shrinks the outgoing prompt. Shipped after it: `chat_usage.rs`,
`chat_thread_title.rs` and `POST /api/v1/chat`, chosen in that order because they
are the only parts of step 4 that touch no SQL and so could land beside the
Postgres work. Two bugs surfaced on the way — **JSON key order was changing a
number in a response body** (518 tokens against 510; `preserve_order` is on
crate-wide now, with a test guarding it) and `KNOWN_TOOLS` had drifted behind
`TOOL_SPECS`, so leaked `search` and `repo_map` calls were being dropped while
their markup was stripped from the answer.

Twelfth pass closed step 4 and 4½ in one sitting, 2026-08-07. **Coder's agent
loop shipped whole** — `send`, `stream`, `retry`, `approve`, `tool-result` in
one commit, as the parked tool-call future required — over `coder_loop.rs` and
`coder_tools.rs`. Two things the scope note expected turned out cheaper than
written: `merge_title_sse_events` needed no port at all (an `mpsc` sender cloned
to the title task *is* the merge), and the pytest file proved nothing (16 of 28
fail identically, fifteen of them monkeypatching in-process httpx), so a
scripted fake Ollama plus a six-scenario cross-render did the proving instead —
transcript, row, **and what each server sent to the model**. It found one real
bug (`str(KeyError(x))` is `repr(x)`, so Python's 404 detail carries its own
quotes) and one divergence kept on purpose (Python 404s from *inside* the
generator on `/chat/retry`, answering a truncated stream; Rust answers a clean
404). **Playground was deleted, not ported** — 699 LOC with no caller anywhere —
which closed the `api_tokens` two-writer hazard step 3 opened.

Then step 5 was scoped and **closed as a decision, not a port**: `/system/status`
is already correct through the proxy (its `listening_on` indirection exists for
exactly that), two of its fields have no Rust referent, and a Rust
`/system/logs` would return different lines than Python's ring. Both stay
proxied; the reasoning is written into the step so it is not re-opened.

Then `api_tokens` landed as step 6 — the smallest thing left and the one that
closed the migration's oldest split, since `auth.rs` had been reading that table
since day one while Python owned every write. **`last_used_at` advances again**,
which had been silently broken for every token whose traffic Rust answers. Five
validation differences came out of the cross-render, one of which
(`Option<Json<T>>` rejecting an empty body as a plain-text 400) **was latent in
every other domain that takes an optional JSON body — swept 2026-08-07**, see
step 6's note above.

Then two more routers moved the same day. **The LLM-proxy admin surface**
(step 7, `llm_admin.rs`) took fourteen of fifteen routes and closed the
config-file coupling; only `POST /config-yaml` stays, for its `jsonschema` error
text. **The `action_orchestrator` routes** (step 8) took all eleven, and that
cross-render found a shadowed import which had been 500ing four public routes in
Python — fixed in the same commit.

Then **model-ops** (step 9) took thirteen of its seventeen routes; the four
pipeline ones stay with Python because `runner.py` runs torch in-process and
cancels through a live-subprocess dict. That cross-render found two more Python
defects — `ollama_client` passing `json=` to a helper whose keyword is
`json_body`, which was a 500 on every show and every delete.

Then the **workspace/document stack** (step 10) closed the list: all six tenant
routes and seven of the eight file routes, on both prefixes, with the archive
cascade — the last live writer of `api_tokens` outside Rust. `POST /upload` and
`GET /file` for a `.pdf` stay with Python for PyMuPDF, which is where the
scoping said the split would land. A third Python defect fell out of it: a
`normalize_relative_path` call outside its `try`, 500ing on `..`.

**Resume at: deciding what Python is still for.** No router is fully Python any
more. What is left proxied is six deliberate shapes, each with its reason
written into the step above it: the four model-ops pipeline routes (the runner),
`POST /upload` + `GET /file` on a PDF (PyMuPDF), `POST /chat/threads` and
`POST /llm-proxy/config-yaml` (both by choice), and `system_routes` (two fields
with no Rust referent). The next real question is not "which domain" — it is
whether the *pipeline* and *PDF extraction* become services of their own, since
those two are the only technical blockers left.*
