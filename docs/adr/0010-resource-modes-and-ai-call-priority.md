# 10. Resource modes, and a priority gate in front of every AI call

Date: 2026-08-19

## Status

Accepted.

## Context

The ask was "review the app's performance, fix the pitfalls", with three named
requirements: **use only what is needed while running in the background**,
**balance AI calls by queue or priority**, and **give the user a low/high/auto
knob in Settings**.

The review found the app's *foreground* cost already well managed and its
*background* cost not managed at all. Both halves are recorded here, because the
first half is why this ADR does not touch the subscription layer.

### What is already right, and stays that way

`desktop/crates/app/src/main.rs`'s `subscription` is careful work and the
review confirms it. Every poll is gated on `live = app.window.is_some() &&
app.view_available()`; the tray listener parks a blocking thread instead of
polling a crossbeam channel at 6.7 Hz; the HUD rides `window::frames()` rather
than a 16 ms timer that Windows would quantise to its 15.6 ms tick; the health
poll's interval is its own subscription identity, so it backs off from 750 ms to
5 s the moment the server answers. A run the user walked away from deliberately
keeps its poll, because that poll is what fires the completion toast.

None of that is changed. The one gap is that `StatusTick` keeps running at 5 s
with the window closed, when the app is a server host and nobody is reading a
status page.

### What is wrong: nothing bounds the fan-out

`executor.rs` plans a wave of ready DAG nodes and spawns one task per node into a
`JoinSet`:

```rust
Wave::Run(task_ids) => {
    let mut wave: JoinSet<()> = JoinSet::new();
    for task_id in task_ids { wave.spawn(...execute_task(task_id)); }
```

The wave width comes from `plan_wave(&snapshot, max_concurrent_tasks())`, and
`max_concurrent_tasks()` is `env_positive_i64("AGENT_PLATFORM_DAG_MAX_CONCURRENT_TASKS")`
— `None` unless an operator set it. **The shipped default is unbounded.** The
planner prompt asks for exactly the shape that makes this hurt:

> Prefer **many small parallel subagents** over one large step

So a 40-node ready wave is 40 concurrent LLM calls, and two concurrent processes
are 80. Nothing anywhere caps it. Three separate failures follow from that one
fact:

1. **The machine is not the user's any more.** 40 in-flight requests, 40 rows of
   context assembly, 40 result writes — while the user is trying to compile
   something in another window. This is precisely the "don't make me struggle
   with the other things I'm running" complaint.
2. **The interactive call queues behind the batch.** A chat turn, a Coder tool
   loop and a background DAG node are the same kind of request to
   `upstream_http::send_with_retry`, which is the single funnel every vendor call
   goes through. There is no lane, no priority, no ordering — the user's cursor
   blinks while a batch job that nobody is watching holds the connections.
3. **Rate limits amplify instead of shedding.** `send_with_retry` retries a 429
   six times with backoff. Forty callers each retrying six times is 240 requests
   to a vendor that just said stop. Bounded concurrency is the fix for this;
   more retry cleverness is not.

The same unboundedness is why the SQLite pool is a latent problem rather than a
present one. `AnyPoolOptions::new()` takes sqlx's default 10 connections and 30 s
acquire timeout; a 40-wide wave is 40 tasks contending for 10 connections against
a database with one writer. Bounding the wave removes the pressure, so the pool
is left alone — see [Consequences](#consequences).

### Three smaller findings

- **Blocking filesystem walks on the async runtime.** `coder_tools::search` and
  `coder_tools::repo_map` (in both `crates/server` and `crates/app`) call
  `walk_files`, which is a synchronous `read_dir` descent, and then
  `std::fs::read_to_string` every file it found. On a real workspace that is
  seconds of a tokio worker thread parked in the kernel, and it is called from an
  `async fn` with no `spawn_blocking` around it. With N worker threads, N
  concurrent searches stall the whole runtime — including the health route.
- **The model catalog probes forever at a fixed rate.** `model_catalog`'s refresh
  loop wakes every 30 s for the life of the process and issues two HTTP requests
  with 8 s timeouts, to Ollama and LM Studio, whether or not either is installed.
  On the overwhelming majority of installs both fail, forever, at a fixed cost.
- **`StatusTick` with no window**, as above.

### Why a knob, and not just better defaults

A single tuned default cannot be right. "Use most performance when they need it"
and "don't fight the other things I'm running" are the same user thirty minutes
apart. The knob has to exist, and the third mode — auto — has to be the default,
because a knob most users never touch is a knob that only helps the users who
already knew they had a problem.

## Decision

### 1. One mode, resolved to two lanes, enforced by one gate

A new `crates/server/src/resources.rs` owns a process-wide `Limits`, held on
`AppState`. It has two parts a caller can see:

```rust
pub enum Mode { Eco, Balanced, Turbo, Auto }
pub enum Priority { Interactive, Background }

limits.acquire(Priority::Background).await  // -> permit, held for the call
```

**Two lanes, not a priority queue.** `tokio::sync::Semaphore` is FIFO-fair with
no priority support, and a real priority queue in front of it would be a
scheduler we would then own. The observation that makes it unnecessary: the
problem is *background work stampeding*, and interactive work is one call at a
time per human. So the interactive lane is capped generously (a bound against
pathology, not a throttle) and the background lane is capped tightly. Interactive
calls essentially never wait; background calls do, which is the point.

**The mode sets the background lane's width**, from
`std::thread::available_parallelism()` (stdlib — no `num_cpus` dependency):

| Mode | Background permits | Interactive permits |
|---|---|---|
| `Eco` | 1 | 4 |
| `Balanced` | `cpus / 2`, clamped 2..=8 | 8 |
| `Turbo` | `cpus`, clamped 4..=16 | 16 |

`Auto` is not a fourth width. It resolves, on each acquire, to one of the three:

- **`Turbo`** while the user is at the window — the desktop app pushes
  focus/visibility, and a user watching a run wants it to finish.
- **`Balanced`** for 60 s after the last interactive acquire — they just walked
  away from a chat and may come back; the run should not have collapsed to
  single-file in the meantime.
- **`Eco`** otherwise — nobody is watching, so the app gets out of the way of
  whatever else is on the machine. This is the "when to sleep" half of the ask.

Resizing is `Semaphore::add_permits` / `forget_permits` against a recorded grant
count, reconciled lazily on acquire. Permits already handed out are not revoked;
a mode change takes effect as work drains, which is the correct behaviour for a
setting toggled mid-run.

### 2. Gate at `complete_internal`, not at the transport

`upstream_http::send_with_retry` is tempting as the single choke point, but it
also carries model pulls, provider catalogue listings and capability probes.
Those are not inference and must not share a lane sized for it.

The right boundary turned out to be one function lower: **`llm::complete_internal`
is the only way anything in this process calls a model on the server's own
behalf** — eleven callers, from `coder_loop` and `assistant` to `executor`,
`media`, `search`, `todos` and `chat_thread_title`. It took a **required**
`priority` parameter rather than an optional one, so a twelfth caller cannot
silently inherit whichever lane happened to be the default; the compiler asks.
The public `/v1/chat/completions` route is gated separately as `Interactive`,
since it is the desktop chat and any external client.

| Caller | Lane | Why |
|---|---|---|
| `executor::call_llm` (DAG nodes, planning, summarisation) | `Background` | the stampede this ADR exists for |
| `chat_thread_title` | `Background` | auto-titling; nobody is waiting |
| `media` prompt suggest/enhance | `Background` | feeds a job that already runs detached |
| `coder_loop`, `assistant`, `action_orchestrator` | `Interactive` | a turn a human is sitting through |
| `search` (dork translation), `todos`, `workflows::assist` | `Interactive` | route-driven, user typed something |
| `llm::chat_completions` (the public route) | `Interactive` | the desktop chat and any external client |

No nesting is possible — `complete_internal` never calls itself and no
interactive handler awaits a background one — so a lane of 1 in Eco cannot
deadlock against itself.

The permit is held for the whole buffered call. On the streaming branch of the
public route it covers opening the stream and not the body that follows, because
the permit borrows from `&Limits` and cannot outlive the handler. That still
bounds concurrent initiations, which is where a pile-up forms, and the
interactive lane is a ceiling rather than a throttle anyway.

### 3. The DAG wave gets a real default

`max_concurrent_tasks()` stops returning `None`. It falls back to the resolved
background permit count, so the wave is bounded even before the semaphore sees
it — the executor stops *creating* 40 tasks rather than creating them and having
39 block. `AGENT_PLATFORM_DAG_MAX_CONCURRENT_TASKS` still wins when set, because
an operator who pinned that number meant it.

### 4. The knob in Settings

`Settings` in `shell.rs` gains `resource_mode: ResourceMode`, defaulting to
`Auto`, persisted in `settings.json` like everything else. A new
**Settings → Performance** tab holds it: three-plus-auto segmented control, a
line of prose per mode, and a live readout of what `Auto` currently resolves to
so the mode is legible rather than magic.

The desktop pushes mode and presence to `PUT /api/v1/system/resources` **on
change only** — mode toggled, window focused, unfocused, closed, and once when
the server first reports ready (so a restarted daemon relearns it). Not on a
timer: an event that fires a few times an hour does not need a poll, and adding
one would contradict the ADR it lives in.

The mode is deliberately *not* an env var read at spawn. A setting that needs an
app restart to take effect is a setting users toggle once and never trust again.

### 5. A sidebar monitor that cannot become the problem it reports

Between the nav list and the utility strip: a line of text (calls in flight, and
the resolved tier) over a segmented meter. It exists so the user can see the app
running hot without opening a settings page.

The constraint that shaped it is that **a monitor which polls is a monitor that
contradicts this ADR.** So it has no timer and no sampler of its own:

- Every number it draws is already an atomic in the server — semaphore permits
  granted minus available. `GET /system/resources` is two atomic loads and a
  mutex, not a measurement.
- It rides `StatusTick`, the health poll the app was running anyway, and skips
  ticks according to the tier it is reporting on: every 4th in Eco (20 s), every
  2nd in Balanced (10 s), every tick in Turbo (5 s). The user who asked the app
  to stay out of the way did not mean "except for the widget that says so".
- It draws nothing at all when the server is not answering, or when no window is
  open. An empty gauge reads as "idle", which is a different claim from "no data".

**Host CPU and memory are deliberately not shown.** Reading them needs a
per-platform dependency and a thread that wakes to sample, which is exactly the
cost the widget is supposed to avoid — and the number it would produce is one
the user cannot act on from a sidebar. What they *can* act on is the app's own
model-call load against the limit their setting chose, which is what is drawn.
ponytail: revisit if "is it me or is it the machine" turns out to be a question
people actually ask here.

### 6. The three smaller fixes

- `search` and `repo_map` move to `tokio::task::spawn_blocking` in **both**
  crates. Not rayon: a data-parallel walk is a new dependency to solve a problem
  that "stop blocking the async runtime" already solves, and the walk is
  I/O-bound. The app-side copy matters for a second reason — that runtime also
  draws the UI, so a workspace-wide grep was stalling frames, not just requests.
- The catalog refresh interval becomes adaptive — 30 s while a backend is
  answering, 5 minutes once both have come back empty. An install with no local
  backend pays 1/10th of today's cost; an install with one is unchanged.
- `StatusTick` backs off to 30 s when `app.window.is_none()`, matching the
  existing pattern where the interval is the subscription's identity.

## Consequences

**A background run is slower, on purpose, and only when nobody is looking.** In
`Auto` with the window open it is `Turbo` — wider than most machines were
usefully running before, because 40-way fan-out was mostly self-contention. The
slow case is `Eco`, which is the case where the user asked for it.

**Vendor rate limits become rarer for the right reason.** Six-deep retry storms
need forty simultaneous callers to happen; with a background lane of 1–8 they
mostly cannot form.

**The SQLite pool is left at its defaults.** 10 connections and a 30 s acquire
timeout were never the bug — the 40-wide wave contending for them was. Revisit if
a pool timeout is ever actually observed, and not before.

**The tokio runtime still starts a worker per core in every mode.** A runtime
cannot be resized after `Runtime::new()`, and idle workers park rather than spin,
so the cost of not shrinking it is thread stacks and not CPU. The semaphore is
what bounds the work; the thread count is not worth a restart to change.

**The monitor lags by up to 20 seconds in Eco.** A burst of background calls
that starts and finishes inside one refresh window is never drawn. That is the
correct trade: the alternative is a widget that wakes the app four times as often
in the mode whose whole purpose is not waking the app.

**`Auto` can be wrong.** It reads window presence and recency, which are proxies
for "the user cares right now", not measurements of it. Someone who starts a long
run and immediately alt-tabs to something unrelated gets `Eco` — arguably right.
Someone who alt-tabs to *watch a log* gets `Eco` too — arguably wrong. That is
what the explicit modes are for, and why the resolved tier is shown in the UI
rather than hidden.

**Host load is not an input.** `Auto` does not sample CPU or memory pressure;
that needs a per-platform dependency and a hysteresis policy to avoid
oscillating, for a signal that window presence already approximates. ponytail:
worth adding the first time someone reports `Turbo` fighting a compile they
started *after* focusing the app.
