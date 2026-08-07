//! E.V. — Extra-Vehicular Assistant, the onboard suit AI and the app's one
//! conversation surface. Stateless chat endpoint, a persona system prompt, the
//! long-term memory in `memory.rs`, and — behind the voice-mode toggle — an
//! animated HUD, a live mic and spoken replies. Voice mode off is plain chat:
//! same thread, same tools, no HUD and no audio.
//!
//! Voice: Microsoft Edge neural TTS (AriaNeural over the free websocket
//! endpoint — no key, needs internet) played through rodio; falls back to the
//! platform's native engine (SAPI/WinRT, AVSpeech, speech-dispatcher) offline.

use agent_platform_client::sse::ChatChunk;
use agent_platform_client::types::{ChatCompletionBody, ChatMessage, ProviderEntry, ToolCall};
use agent_platform_client::Client;
use iced::Task;
use std::collections::VecDeque;
use std::io::Cursor;
use tts::Tts;

/// What the assistant is called, everywhere the user can see it — window
/// labels, the HUD readout, and the byline on a memory it contributed.
pub const NAME: &str = "E.V.";

const PERSONA: &str = "You are E.V. (Extra-Vehicular Assistant), the onboard \
suit AI of this Agent Platform. \
Style: a superhero suit's heads-up-display assistant — calm, quick-witted, \
protective, with a dry quip now and then. You monitor systems, analyze the \
situation, and give tactical, actionable answers. Answer directly first; skip \
filler. Your replies render as markdown in the HUD and may also be read aloud \
(markdown is stripped for speech) — use lists, bold and code fences whenever \
they make the answer clearer, and keep the prose short and conversational. \
You have a real terminal on the user's machine via the run_command tool \
(PowerShell on Windows). When a question concerns the local machine — files, \
git, processes, system state — run a command and answer from its output \
instead of guessing. Never run destructive commands unless explicitly asked. \
You also keep a long-term memory of the user, listed above. When the user asks \
you to remember, correct or forget something about them, use the memory tools \
rather than only saying you will — list_memories first when you need an id. A \
fact that replaces one you already remember is an update_memory call on that \
fact's id, not a second memory. Never claim to have remembered, changed or \
forgotten anything you did not actually do with a tool.";

/// Rounds of tool calls allowed per user turn before the model is forced to
/// answer in text (tools withheld from the request past the cap).
const MAX_TOOL_ROUNDS: u8 = 5;
/// Hard deadline on one command; past it the process is killed.
const TOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// Output kept per command — both for the model and the transcript.
const MAX_TOOL_OUTPUT: usize = 8_000;

/// What E.V. carries: a terminal, and the keys to its own long-term memory.
fn tools_spec() -> serde_json::Value {
    let mut tools = vec![serde_json::json!({
        "type": "function",
        "function": {
            "name": "run_command",
            "description": "Run one shell command on the user's machine (PowerShell on \
                            Windows, sh elsewhere) and return its combined stdout/stderr.",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The command line to execute." }
                },
                "required": ["command"]
            }
        }
    })];
    tools.extend(crate::memory::tools_spec());
    serde_json::Value::Array(tools)
}

/// The `command` a call asked to run, or its raw arguments while they are
/// still streaming/malformed — shown in the transcript either way.
fn command_of(call: &ToolCall) -> String {
    if call.function.name != "run_command" {
        return format!("{}({})", call.function.name, call.function.arguments.trim());
    }
    serde_json::from_str::<serde_json::Value>(&call.function.arguments)
        .ok()
        .and_then(|v| v.get("command").and_then(|c| c.as_str()).map(str::to_string))
        .unwrap_or_else(|| call.function.arguments.clone())
}

/// A reply that *is* a tool call written out as text, rather than a reply that
/// happens to mention one. Strict on purpose: the whole message must be the
/// call (or a list of them), and every name must be a tool that exists — a
/// model explaining `remember` in a code fence is prose, and running it would
/// be worse than the bug this works around.
fn salvage_calls(content: &str) -> Option<Vec<ToolCall>> {
    let text = content.trim().trim_start_matches("```json").trim_matches('`').trim();
    let parsed: serde_json::Value = serde_json::from_str(text).ok()?;
    let items = match parsed {
        serde_json::Value::Array(items) => items,
        object @ serde_json::Value::Object(_) => vec![object],
        _ => return None,
    };
    let mut calls = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let name = item.get("name")?.as_str()?;
        if name != "run_command" && !crate::memory::TOOLS.contains(&name) {
            return None;
        }
        let arguments = match item.get("arguments") {
            // Written either way, depending on the model's mood.
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
            None => "{}".to_string(),
        };
        calls.push(ToolCall {
            id: format!("salvaged_{i}"),
            function: agent_platform_client::types::ToolFunction {
                name: name.to_string(),
                arguments,
            },
            ..ToolCall::default()
        });
    }
    (!calls.is_empty()).then_some(calls)
}

#[derive(Debug, Clone)]
pub struct ToolOutcome {
    /// `tool_call_id` the result answers.
    pub id: String,
    pub output: String,
}

/// Execute one shell command, capped by `timeout` and `MAX_TOOL_OUTPUT`.
/// Errors come back as text — the model reads them and corrects itself.
///
/// `cwd` is `None` for E.V., which runs wherever the app was launched, and the
/// workspace root for the Coder screen ([`crate::coder_tools`]) — the only
/// difference between the two, which is why they share this rather than each
/// keeping a copy of the timeout, the truncation and the no-console-flash flag.
pub(crate) async fn run_command(
    command: String,
    cwd: Option<std::path::PathBuf>,
    timeout: std::time::Duration,
) -> String {
    let mut cmd = if cfg!(windows) {
        let mut c = tokio::process::Command::new("powershell");
        c.args(["-NoProfile", "-NonInteractive", "-Command", &command]);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-lc", &command]);
        c
    };
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW — no console flash
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.stdin(std::process::Stdio::null()).kill_on_drop(true);
    let out = match tokio::time::timeout(timeout, cmd.output()).await {
        Err(_) => return format!("(timed out after {}s)", timeout.as_secs()),
        Ok(Err(e)) => return format!("(failed to start: {e})"),
        Ok(Ok(out)) => out,
    };
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&err);
    }
    if !out.status.success() {
        text.push_str(&format!("\n(exit code {})", out.status.code().unwrap_or(-1)));
    }
    if text.trim().is_empty() {
        text = "(no output)".into();
    }
    if text.len() > MAX_TOOL_OUTPUT {
        let mut cut = MAX_TOOL_OUTPUT;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n… (truncated)");
    }
    text
}

/// Run every terminal call of one round, in order. Sequential on purpose: the
/// calls in a round often depend on the same working state (cd, files just
/// written). `results` carries the memory calls of the same round, already
/// answered against the live store before this task was spawned.
async fn run_tools(calls: Vec<ToolCall>, mut results: Vec<ToolOutcome>) -> Vec<ToolOutcome> {
    results.reserve(calls.len());
    for call in calls {
        let output = if call.function.name == "run_command" {
            match serde_json::from_str::<serde_json::Value>(&call.function.arguments)
                .ok()
                .and_then(|v| v.get("command").and_then(|c| c.as_str()).map(str::to_string))
            {
                Some(cmd) => run_command(cmd, None, TOOL_TIMEOUT).await,
                None => format!(
                    "error: run_command needs {{\"command\": \"…\"}}, got: {}",
                    call.function.arguments
                ),
            }
        } else {
            format!("error: unknown tool {:?}", call.function.name)
        };
        results.push(ToolOutcome { id: call.id, output });
    }
    results
}

const EDGE_VOICE: &str = "Microsoft Server Speech Text to Speech Voice (en-US, AriaNeural)";

/// Speech rates the settings screen offers, as Edge's percent-of-normal.
pub const VOICE_RATES: [(&str, i32); 5] =
    [("Calm", -10), ("Normal", 0), ("Brisk", 15), ("Fast", 30), ("Rapid", 45)];

/// Above normal on purpose: E.V. answers in short bursts, and a narration pace
/// makes a two-line answer feel like a wait.
pub const DEFAULT_VOICE_RATE: i32 = 15;

/// How far the adaptive nudge below may push the rate either side of the
/// setting — past this the voice stops sounding like the one that was picked.
const RATE_SPAN: i32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Idle,
    /// Hands-free and the mic is live, but nobody is talking to E.V. yet.
    Armed,
    /// The gate opened: this is being captured as an utterance.
    Listening,
    Thinking,
    Speaking,
}

/// The analyzer's step. Drawing runs at the display's rate; this does not — the
/// window length, the attack/release constants and every frame counter in the
/// mic gate below are written against 60 Hz, and a variable rate would retune
/// all of them at once.
const DT: f32 = 1.0 / 60.0;
/// Spectrum bins the HUD draws. Also the number of web spokes it lights.
pub const BANDS: usize = 24;
/// Samples fed to the analyzer each frame (~43 ms at 48 kHz — long enough for
/// the bass bins to see a couple of cycles).
const WINDOW: usize = 2048;
/// Level history behind the waveform ribbon, newest last (~2 s at 60 fps).
pub const WAVE: usize = 120;

// --- The gate ---------------------------------------------------------------
// Hands-free means the mic hears everything: the fan, the keyboard, the person
// on the phone next door, and E.V.'s own replies coming back out of the
// speakers. Every constant below exists to throw one of those away.

/// Speech must clear the room's own noise by this much (linear, ≈11 dB). A
/// person talking to their machine sits well above it; a television two rooms
/// away does not.
const OPEN_SNR: f32 = 3.5;
/// Absolute floor, for a silent room where the adaptive floor is near zero and
/// any hiss would otherwise clear the SNR test.
const ABS_FLOOR: f32 = 0.006;
/// Frames of speech-shaped audio before the gate opens (~130 ms). A door, a
/// key press and a mouse click are all shorter than this.
const ONSET_FRAMES: u32 = 8;
/// Silence that ends an utterance. Long enough to think mid-sentence.
const HANG: f32 = 0.75;
/// Shortest thing that counts as an instruction, in seconds of actual speech.
const MIN_VOICED: f32 = 0.25;
/// Hard cap on one utterance.
const MAX_UTTERANCE: f32 = 30.0;
/// Pre-roll kept ahead of the gate opening, so the first consonant survives.
const PREROLL: f32 = 0.4;
/// The mic hears the speakers: stay shut while E.V. talks, plus this tail for
/// the room's reverb.
const ECHO_TAIL: f32 = 0.35;
/// How far above the speakers' own bleed a voice has to sit to read as you
/// talking over E.V. rather than as E.V. hearing itself.
///
/// ponytail: still half-duplex — this detects the interruption and stops the
/// reply, it does not transcribe through playback. That needs acoustic echo
/// cancellation. On speakers loud enough to clip the mic preamp, the bleed
/// stops tracking and barge-in goes deaf; headphones or lower volume fix it.
const BARGE_SNR: f32 = 3.0;
/// Frames of that before E.V. stops talking (~200 ms) — deliberately longer
/// than `ONSET_FRAMES`, because cutting a reply off by mistake is worse than
/// stopping a beat late.
const BARGE_FRAMES: u32 = 12;
/// After E.V. replies you can just talk. Long enough to read the reply and
/// think before answering it. Outside this window an utterance has to name
/// E.V. to be sent on its own — and while the mic is armed there is no
/// window at all, `armed` keeps it open.
const FOLLOW_UP: f32 = 45.0;
/// How much louder than the floor an utterance's peak must be to read as
/// close-talk rather than someone else's conversation across the room.
const CLOSE_TALK_SNR: f32 = 5.0;
/// Cosine similarity to the enrolled voice below which an utterance is treated
/// as somebody else. Deliberately forgiving: a stranger reaching the model is
/// recoverable, but E.V. ignoring the person it belongs to is not. Watch the
/// HUD's `VID` readout against your own voice and tighten it if strangers get
/// through.
pub const VOICE_MATCH: f32 = 0.82;
/// Utterances averaged into the enrolled print before it stops being provisional.
const ENROLL_UTTERANCES: u32 = 4;
/// Similarity to the *provisional* print required to be averaged into it.
/// Looser than `VOICE_MATCH` — that print is one utterance old and still
/// moving, so this bar only has to reject a different person, not a different
/// sentence from the same one.
const ENROLL_MATCH: f32 = 0.70;

/// Is this frame shaped like a voice? Fans and traffic sit under 200 Hz, hiss
/// and keyboard clatter spread flat across everything; speech puts most of its
/// energy in the middle. The bands are already computed for the HUD, so this
/// costs a couple of dozen adds.
fn voice_like(bands: &[f32; BANDS]) -> bool {
    let total: f32 = bands.iter().sum();
    if total < 0.05 {
        return false;
    }
    let (mut speech, mut bins) = (0.0, 0);
    for (i, v) in bands.iter().enumerate() {
        if (150.0..4000.0).contains(&crate::stt::band_freq(i, BANDS)) {
            speech += v;
            bins += 1;
        }
    }
    // Compare against what a *flat* spectrum would score, not a fixed number:
    // the bins are log-spaced, so most of them already fall inside the speech
    // window and white noise clears any fixed threshold. Speech has to be more
    // concentrated than noise, which is the only claim actually being made.
    //
    // The margin is thin on purpose. These are display bands — sqrt-compressed,
    // with the HUD's high-frequency tilt — so real speech concentrates only a
    // few points above flat while noise sits at or below it; a 1.1 margin put
    // the bar above actual speech and made hands-free deaf.
    speech / total > (bins as f32 / bands.len() as f32) * 1.03
}

/// Whisper narrates silence. Fed a stretch of room tone, a cough or a door it
/// does not emit nothing — it emits one of a small set of stock captions burned
/// in from its training data. That was survivable while an unaddressed line
/// only parked in the composer; now that an armed mic sends everything, a cough
/// opens a turn that says "you".
///
/// ponytail: fixed English list, whole-utterance match only. Extend it as new
/// ghosts turn up. The real fix is whisper's own no-speech probability, which
/// the binding does not expose.
const GHOSTS: [&str; 8] = [
    "you",
    "thank you",
    "thanks",
    "thank you very much",
    "thanks for watching",
    "thank you for watching",
    "bye",
    "silence",
];

/// Is the whole transcript one of whisper's stock captions for "nothing was
/// said"? Costs a real "thank you" aimed at E.V., which is a reply worth
/// nothing anyway.
fn is_ghost(text: &str) -> bool {
    let plain: String = text
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    GHOSTS.contains(&plain.split_whitespace().collect::<Vec<_>>().join(" ").as_str())
}

/// Whitelist of how whisper spells "E.V." when someone says it out loud. The
/// three-letter spellings stay: people who used to say "E.V.A." still do, and
/// a name the assistant no longer answers to is a bug report.
const NAMES: [&str; 12] = [
    "eva", "ava", "evah", "ev", "eev", "eve", "evie", "evee", "eevee", "heavy", "evy", "ivy",
];
/// Words allowed to precede the name.
const OPENERS: [&str; 6] = ["hey", "ok", "okay", "yo", "hi", "hello"];

/// Does this transcript open by addressing E.V.? Returns what was said *after*
/// the name. Matching is on sounds rather than spelling — whisper writes the
/// same two letters a dozen different ways.
fn addressed(text: &str) -> Option<&str> {
    let mut rest = text.trim_start();
    for step in 0..2 {
        let (word, tail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let plain: String =
            word.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
        if NAMES.contains(&plain.as_str()) {
            return Some(tail.trim_start_matches([',', '.', '!', '?', ' ']));
        }
        // One opener ("hey", "ok") may come first; anything else means the
        // sentence is not addressed to E.V.
        if step > 0 || !OPENERS.contains(&plain.as_str()) || tail.is_empty() {
            return None;
        }
        rest = tail;
    }
    None
}

/// Identity of the transcript scrollable, so replies can snap it to the end
/// without anchoring it there permanently (which fights the user's scrolling).
pub fn transcript_id() -> iced::widget::Id {
    iced::widget::Id::new("ev-transcript")
}

#[derive(Default)]
pub struct State {
    pub messages: Vec<ChatMessage>,
    /// Parsed markdown per message, same indices as `messages` — parsed once
    /// at push, not per frame (the HUD redraws at 60 fps).
    pub md: Vec<Vec<iced::widget::markdown::Item>>,
    /// Chain-of-thought per message, same indices — empty except for assistant
    /// turns from a reasoning model. Display-only: never on the wire, and
    /// never spoken (the voice reads the reply, not the deliberation).
    pub reasoning: Vec<String>,
    /// Messages whose thinking section the user has expanded.
    pub reasoning_open: std::collections::HashSet<usize>,
    pub draft: String,
    pub sending: bool,
    pub error: Option<String>,
    /// Voice mode: the HUD is on screen, the mic can be armed and replies are
    /// spoken. Off is the same thread as plain text.
    pub voice: bool,
    /// Provider/model override for this thread; empty = the server's default.
    /// Persisted in `shell::Settings`, like every other screen preference.
    pub provider: String,
    pub model: String,
    /// How fast E.V. reads, as Edge's percent-of-normal. The user's setting;
    /// `speech_rate` is what a given sentence actually gets.
    pub voice_rate: i32,
    /// Providers the proxy knows, for the header dropdowns. Loaded on screen
    /// entry; empty until then (the dropdowns just have nothing to offer).
    pub catalog: Vec<ProviderEntry>,
    /// Long-term recall, refreshed by the app before every message. `None` when
    /// memory is off or empty — see `memory::Store::system_block`.
    pub memory: Option<String>,
    /// Seconds of animation time, monotonic while the screen is open.
    pub phase: f32,
    /// Timestamp of the last drawn frame, for measuring real elapsed time.
    /// `None` until the first one lands.
    pub last_frame: Option<std::time::Instant>,
    /// Frame time banked toward the next 60 Hz analyzer step.
    pub audio_acc: f32,
    /// Smoothed mic input level (0..1) while recording — drives the HUD meter.
    pub mic_level: f32,
    /// Per-band energy of whoever is talking (mic while listening, E.V.'s own
    /// voice while speaking), smoothed with fast attack / slow release.
    pub bands: [f32; BANDS],
    /// Broadband energy, same smoothing — the "how loud overall" number.
    pub energy: f32,
    /// Transient flash 0..1: spikes on an attack, decays over ~0.3 s.
    pub beat: f32,
    /// Rolling `energy` history, oldest first, for the waveform ribbon.
    pub wave: Vec<f32>,
    /// Mode crossfade: 0 at the switch, eased to 1 over ~0.3 s.
    pub mode_t: f32,
    /// Mode the crossfade is coming *from*, and the one it settled on.
    pub mode_prev: Mode,
    mode_now: Mode,
    /// Seconds in the current mode — the HUD's `T+` readout.
    pub elapsed: f32,
    /// Power-on sweep 0..1, played once when the screen first ticks.
    pub boot: f32,
    /// The room's noise floor (raw RMS), tracked continuously while armed —
    /// the gate is relative to this, so a loud room raises the bar instead of
    /// firing constantly.
    pub floor: f32,
    /// What E.V.'s own voice measures at the mic, learned while it plays. The
    /// barge-in test is relative to this, so headphones (near zero) and open
    /// speakers (loud) both work without a volume setting.
    pub bleed: f32,
    /// Consecutive frames of a voice over the top of E.V.'s own.
    barge: u32,
    /// Kill switch for the turn in flight. `None` when nothing is streaming.
    abort: Option<iced::task::Handle>,
    /// Sample index where the current utterance began. `Some` = capturing.
    capture: Option<u64>,
    /// Consecutive frames that looked like speech, while the gate is shut.
    onset: u32,
    /// Seconds since the last speech-shaped frame, while capturing.
    hang: f32,
    /// Seconds of actual speech in the current utterance — a cough is short.
    voiced: f32,
    /// Loudest frame of the current utterance, for the close-talk check.
    peak: f32,
    /// `phase` when E.V. last finished a reply, or when you armed it: inside the
    /// follow-up window you don't have to say its name.
    last_reply: f32,
    /// The mic button is on. Pressing it is intent enough on its own, so every
    /// utterance that clears the gate while it is on gets sent — no wake word,
    /// no clock. Cleared wherever the recorder is dropped.
    armed: bool,
    /// `phase` while E.V.'s own voice is playing — the mic hears the speakers,
    /// so capture stays shut until a beat after it stops.
    spoke_at: f32,
    /// The enrolled speaker, learned from the utterances E.V. accepted —
    /// there is no enrollment wizard, the first few things you say are it.
    voice_print: Option<crate::stt::VoicePrint>,
    /// How many utterances have been averaged into that print.
    enrolled: u32,
    /// Similarity of the last utterance to the enrolled voice, for the HUD.
    /// `None` before enrollment, or when the audio was too short to print.
    pub voice_sim: Option<f32>,
    /// The mic, open the whole time hands-free listening is armed.
    recorder: Option<crate::stt::Recorder>,
    transcribing: bool,
    synthesizing: bool,
    audio: Option<(rodio::OutputStream, rodio::OutputStreamHandle)>,
    sink: Option<rodio::Sink>,
    /// Mono copy of what the sink is playing, for the output level meter.
    playback: Option<(Vec<f32>, u32)>,
    tts: Option<Tts>,
    tts_failed: bool,
    /// An assistant turn is open and collecting deltas. Public so the view can
    /// keep the live turn's thinking section open while it streams.
    pub streaming: bool,
    /// Tool calls of the round in flight, assembled from streamed fragments.
    tool_buf: Vec<ToolCall>,
    /// Tool rounds taken since the user last spoke; capped by MAX_TOOL_ROUNDS.
    tool_rounds: u8,
    /// Streamed speech text past the last sentence boundary — not enough to say
    /// yet, held until the sentence closes or the stream ends.
    speech_buf: String,
    /// Closed sentences waiting their turn at the synthesizer.
    speech_queue: VecDeque<String>,
    /// Synthesized clips waiting for the sink to free up. One sentence is
    /// synthesized while the previous one plays, so the voice runs continuously.
    audio_queue: VecDeque<Vec<u8>>,
    /// Text currently at the synthesizer, kept so the native engine can speak it
    /// if the network voice fails.
    speaking: Option<String>,
    /// Nothing of this reply has reached the synthesizer yet — the one chunk
    /// the user is actually waiting on, cut short on purpose.
    first_chunk: bool,
}

impl State {
    pub fn new() -> Self {
        Self { voice: true, voice_rate: DEFAULT_VOICE_RATE, ..Self::default() }
    }

    /// The screen's thread, opened on the persisted provider/model pair.
    pub fn with_defaults(provider: String, model: String, voice_rate: i32) -> Self {
        Self { provider, model, voice_rate, ..Self::new() }
    }

    pub fn provider_ids(&self) -> Vec<String> {
        self.catalog.iter().map(|p| p.id.clone()).collect()
    }

    /// Models the chosen provider offers; every provider's models when no
    /// provider is picked (the proxy resolves an alias to its provider).
    pub fn model_options(&self) -> Vec<String> {
        self.catalog
            .iter()
            .filter(|p| self.provider.is_empty() || p.id == self.provider)
            .flat_map(|p| p.models.options.iter().cloned())
            .collect()
    }

    /// Hands-free listening is on: the mic is open and E.V. decides when you
    /// are talking to it.
    pub fn armed(&self) -> bool {
        self.recorder.is_some()
    }

    /// Whatever was just said counts as aimed at E.V. even though nobody named
    /// it: either the mic button is on, or E.V. only just stopped talking and
    /// this is the other half of the exchange.
    ///
    /// Measured from `spoke_at` — the last frame E.V.'s voice was playing — and
    /// not from when its text finished. The gate is held shut while E.V. speaks,
    /// so a window timed from the end of generation is already partly spent by
    /// the time you are physically able to answer, and a long spoken reply eats
    /// all of it.
    fn follow_up_open(&self) -> bool {
        self.armed || since(self.phase, self.last_reply.max(self.spoke_at)) < FOLLOW_UP
    }

    /// Fold an accepted utterance into the enrolled voice. The first few carry
    /// full weight (that is the enrollment), then it tracks slowly — a voice
    /// changes with a cold, a headset or the time of day.
    fn learn_voice(&mut self, print: Option<crate::stt::VoicePrint>) {
        let Some(heard) = print else { return };
        match &mut self.voice_print {
            None => self.voice_print = Some(heard),
            Some(known) => {
                let w = if self.enrolled < ENROLL_UTTERANCES {
                    1.0 / (self.enrolled + 1) as f32
                } else {
                    0.1
                };
                for (k, h) in known.iter_mut().zip(heard) {
                    *k += (h - *k) * w;
                }
                // Averaging two unit vectors does not give a unit vector, and
                // the similarity test assumes one.
                let norm = known.iter().map(|v| v * v).sum::<f32>().sqrt();
                if norm > 1e-6 {
                    for v in known.iter_mut() {
                        *v /= norm;
                    }
                }
            }
        }
        self.enrolled += 1;
    }

    /// Forget the enrolled voice — wrong person enrolled, or a new mic.
    pub fn forget_voice(&mut self) {
        self.voice_print = None;
        self.enrolled = 0;
        self.voice_sim = None;
    }

    /// Whether a voice has been learned well enough to reject strangers.
    pub fn voice_enrolled(&self) -> bool {
        self.enrolled >= ENROLL_UTTERANCES
    }

    /// Replace the thread with a saved conversation (empty = a fresh one).
    /// Callers guard against an in-flight send; the draft and the mic are left
    /// alone. Rebuilds the transcript the way the live stream renders it: tool
    /// results fenced, tool calls shown as the command they ran.
    pub fn load_thread(&mut self, messages: Vec<ChatMessage>, reasoning: Vec<String>) {
        self.hush();
        self.md = messages
            .iter()
            .map(|m| {
                let mut shown = if m.role == "tool" {
                    format!("````text\n{}\n````", m.content)
                } else {
                    m.content.clone()
                };
                for c in m.tool_calls.iter().flatten() {
                    shown.push_str(&format!("\n\n```\n$ {}\n```", command_of(c)));
                }
                iced::widget::markdown::parse(&shown).collect()
            })
            .collect();
        self.reasoning = if reasoning.len() == messages.len() {
            reasoning
        } else {
            vec![String::new(); messages.len()]
        };
        self.messages = messages;
        self.reasoning_open.clear();
        self.streaming = false;
        self.tool_buf.clear();
        self.tool_rounds = 0;
        self.error = None;
    }

    fn push_turn(&mut self, role: &str, content: String) {
        self.md.push(iced::widget::markdown::parse(&content).collect());
        self.reasoning.push(String::new());
        self.messages.push(ChatMessage::text(role, content));
    }

    /// Append a reasoning delta — opens the turn like `push_delta`, since a
    /// thinking model reasons before its first reply token. Never enqueued for
    /// speech.
    fn push_reasoning(&mut self, text: &str) {
        if !self.streaming {
            self.push_turn("assistant", String::new());
            self.streaming = true;
        }
        let last = self.messages.len() - 1;
        self.reasoning[last].push_str(text);
    }

    /// Is this the streaming turn whose answer hasn't started yet? The view
    /// keeps that one's thinking section open without a click.
    pub fn reasoning_live(&self, idx: usize) -> bool {
        self.streaming && idx + 1 == self.messages.len() && self.messages[idx].content.is_empty()
    }

    /// Fire the chat request for the current history: persona, recall, then
    /// every visible turn (including tool calls and their results).
    fn request(&mut self, client: &Client) -> Task<Message> {
        let mut messages = vec![ChatMessage::text("system", PERSONA)];
        // Recall after the persona: who E.V. is comes first, then who it
        // is talking to.
        if let Some(recall) = &self.memory {
            messages.push(ChatMessage::text("system", recall.clone()));
        }
        messages.extend(self.messages.iter().cloned());
        let body = ChatCompletionBody {
            messages,
            model: non_empty(&self.model),
            provider: non_empty(&self.provider),
            temperature: None,
            max_tokens: None,
            // Past the round cap the tools disappear from the request, which
            // forces a text answer instead of an endless loop.
            tools: (self.tool_rounds < MAX_TOOL_ROUNDS).then(tools_spec),
            stream: Some(true),
        };
        // Abortable so the turn can actually be stopped — hushing the voice
        // while the tokens keep arriving is not stopping, it is muting.
        let (stream, handle) =
            Task::run(crate::inference::chat_stream(client.clone(), body), Message::Chunk)
                .abortable();
        self.abort = Some(handle);
        // The socket opens while the model is still thinking, so the first
        // sentence goes straight to synthesis instead of waiting behind a
        // handshake — the model's own latency pays for it.
        let warm = if self.voice { warm_voice() } else { Task::none() };
        Task::batch([iced::widget::operation::snap_to_end(transcript_id()), warm, stream])
    }

    /// Stop the turn in flight: the voice, the queued sentences and the stream
    /// itself. Whatever text already arrived stays in the transcript — it was
    /// said, and deleting it would lose the half of the answer you wanted.
    fn abort_turn(&mut self) {
        self.hush();
        if let Some(handle) = self.abort.take() {
            handle.abort();
        }
        self.sending = false;
        self.streaming = false;
        self.tool_buf.clear();
    }

    /// Append a streamed delta to the assistant turn in flight, opening one if
    /// this is the first token, and hand any newly closed sentence to the voice.
    fn push_delta(&mut self, text: &str) {
        if !self.streaming {
            self.push_turn("assistant", String::new());
            self.streaming = true;
            self.first_chunk = true;
        }
        let last = self.messages.len() - 1;
        self.messages[last].content.push_str(text);
        self.md[last] = iced::widget::markdown::parse(&self.messages[last].content).collect();

        if self.voice {
            self.speech_buf.push_str(text);
            while let Some(sentence) = take_sentence(&mut self.speech_buf, self.first_chunk) {
                self.enqueue_speech(&sentence);
            }
        }
    }

    /// Queue one chunk of reply text for the voice, markdown stripped.
    fn enqueue_speech(&mut self, raw: &str) {
        let spoken = speech_text(raw);
        if !spoken.trim().is_empty() {
            self.speech_queue.push_back(spoken);
            self.first_chunk = false;
        }
    }

    /// The rate this sentence gets: the setting, nudged by how much speech is
    /// already waiting behind it.
    ///
    /// An empty queue while the model is still writing means the voice is about
    /// to run out of words — read slower and let the stream catch up, which is
    /// a smaller seam than stopping mid-answer. A backlog means the voice is
    /// behind the text, so read faster and close the gap.
    fn speech_rate(&self) -> i32 {
        let backlog = self.speech_queue.len() + self.audio_queue.len();
        let nudge = match backlog {
            0 if self.streaming => -RATE_SPAN / 2,
            0 | 1 => 0,
            2 => RATE_SPAN / 2,
            _ => RATE_SPAN,
        };
        (self.voice_rate + nudge).clamp(-50, 90)
    }

    /// Start synthesizing the next queued sentence, if the synthesizer is free.
    /// Runs a sentence ahead of playback, which is the whole point: the clip for
    /// sentence N+1 is being made while N is still coming out of the speakers.
    fn next_synthesis(&mut self, client: &Client) -> Task<Message> {
        if self.synthesizing {
            return Task::none();
        }
        let Some(text) = self.speech_queue.pop_front() else {
            return Task::none();
        };
        self.synthesizing = true;
        self.speaking = Some(text.clone());
        let rate = self.speech_rate();
        let client = client.clone();
        Task::perform(
            async move {
                // The server's own voice first (`SPEECH_API_BASE`: a hosted
                // provider, or a local Piper/Kokoro), until it says it has no
                // backend — after that asking again is a round-trip in front of
                // every sentence, for an answer that will not change until the
                // app is restarted.
                if SERVER_SPEECH.load(std::sync::atomic::Ordering::Relaxed) {
                    match client.speech(&text).await {
                        Ok(bytes) => return Ok(bytes),
                        // The server answered and refused; only that is settled.
                        // A transport error is the server being down, and it
                        // may well be up by the next sentence.
                        Err(agent_platform_client::Error::Api { .. }) => {
                            SERVER_SPEECH.store(false, std::sync::atomic::Ordering::Relaxed);
                        }
                        Err(_) => {}
                    }
                }
                tokio::task::spawn_blocking(move || synthesize(&text, rate))
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()))
            },
            Message::Synthesized,
        )
    }

    /// Play the next ready clip once the sink has drained. Called from the tick,
    /// so gaps between sentences are at most one frame.
    fn drain_audio(&mut self) {
        if self.sink.as_ref().is_some_and(|s| !s.empty()) {
            return;
        }
        if let Some(bytes) = self.audio_queue.pop_front() {
            if let Err(e) = self.play(bytes) {
                self.error = Some(e);
            }
        }
    }

    /// Is E.V.'s own voice coming out of the speakers right now?
    ///
    /// Deliberately not `mode() == Speaking`: while a reply streams, the next
    /// sentence is synthesized *over* the one still playing, and `mode` reports
    /// that as Thinking. The gate reading that as "E.V. is quiet" is how a reply
    /// ends up captured and transcribed as if you had said it.
    pub fn speaking(&self) -> bool {
        self.sink.as_ref().is_some_and(|s| !s.empty())
            || self.tts.as_ref().is_some_and(|t| t.is_speaking().unwrap_or(false))
    }

    /// Is there a turn still in flight, or a voice still finishing it? Either
    /// way the tick has work to do — which is why it keeps beating after the
    /// user leaves this screen (see `main::subscription`).
    pub fn busy(&self) -> bool {
        self.sending
            || self.synthesizing
            || self.speaking()
            || !self.speech_queue.is_empty()
            || !self.audio_queue.is_empty()
    }

    pub fn mode(&self) -> Mode {
        if self.capture.is_some() {
            Mode::Listening
        } else if self.sending || self.synthesizing || self.transcribing {
            Mode::Thinking
        } else if self.speaking() {
            Mode::Speaking
        } else if self.armed() {
            Mode::Armed
        } else {
            Mode::Idle
        }
    }

    fn play(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        use rodio::Source;
        if self.audio.is_none() {
            self.audio = Some(rodio::OutputStream::try_default().map_err(|e| e.to_string())?);
        }
        let (_, handle) = self.audio.as_ref().unwrap();
        let sink = rodio::Sink::try_new(handle).map_err(|e| e.to_string())?;
        // Decode up front and keep a mono copy: the HUD reads amplitude at the
        // sink's play position, so the web moves with E.V.'s actual voice.
        let decoder = rodio::Decoder::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
        let (channels, rate) = (decoder.channels(), decoder.sample_rate());
        let raw: Vec<f32> = decoder.convert_samples().collect();
        let ch = channels.max(1) as usize;
        let mono: Vec<f32> =
            raw.chunks_exact(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
        sink.append(rodio::buffer::SamplesBuffer::new(1, rate, mono.clone()));
        self.playback = Some((mono, rate));
        // A fresh sink per utterance sidesteps rodio's stopped-sink semantics.
        if let Some(old) = self.sink.replace(sink) {
            old.stop();
        }
        Ok(())
    }

    /// The audio the HUD should be visualizing right now: E.V.'s own voice at
    /// the sink's play position while it is speaking (so the HUD still moves
    /// on headphones, where the mic hears nothing), otherwise the live mic.
    fn source_tail(&self) -> Option<(Vec<f32>, u32)> {
        if let (Some(sink), Some((mono, rate))) = (&self.sink, &self.playback) {
            if !sink.empty() {
                let idx = (sink.get_pos().as_secs_f32() * *rate as f32) as usize;
                let end = (idx + WINDOW).min(mono.len());
                return Some((mono[idx.min(end)..end].to_vec(), *rate));
            }
        }
        Some(self.recorder.as_ref()?.tail_mono(WINDOW))
    }

    fn speak_native(&mut self, text: &str) {
        if self.tts_failed {
            return;
        }
        if self.tts.is_none() {
            match Tts::default() {
                Ok(t) => self.tts = Some(t),
                Err(e) => {
                    self.tts_failed = true;
                    self.error = Some(format!("Voice unavailable: {e}"));
                    return;
                }
            }
        }
        let rate = self.speech_rate();
        if let Some(t) = self.tts.as_mut() {
            // Same percent, different scale: the native engines report their own
            // range, and normal→max is what a positive percent spends.
            let (normal, min, max) = (t.normal_rate(), t.min_rate(), t.max_rate());
            let span = if rate >= 0 { max - normal } else { normal - min };
            let _ = t.set_rate(normal + span * rate as f32 / 100.0);
            // Queued, not interrupting: a streamed reply falls back one sentence
            // at a time and each must wait its turn.
            let _ = t.speak(text, false);
        }
    }

    fn hush(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        if let Some(t) = self.tts.as_mut() {
            let _ = t.stop();
        }
        // Stopping the sink is not enough while a reply is streaming — whatever
        // is queued behind it would start playing on the next tick.
        self.speech_buf.clear();
        self.speech_queue.clear();
        self.audio_queue.clear();
    }
}

/// Post-capture triage: does a closed utterance go to the transcriber, or was
/// it too brief to be an instruction / too far from the mic to have been aimed
/// at one? Someone else's conversation carries, but it carries quietly.
///
/// The close-talk test is *relative* to the learned room floor; the divisor
/// clamp only guards digital silence. Clamping it at `ABS_FLOOR` — as this
/// once did — quietly turned the test into "peak above 0.03 absolute", which a
/// low-gain mic never reaches: the gate would open (that bar is `ABS_FLOOR`),
/// show Listening, then drop every utterance right here.
/// Is this frame somebody talking over E.V., rather than E.V.'s own voice
/// arriving back through the mic? Both tests matter: the level rules out the
/// speakers, the shape rules out a door slamming during a reply.
fn over_playback(mic: f32, bleed: f32, gate: f32, bands: &[f32; BANDS]) -> bool {
    mic > (bleed * BARGE_SNR).max(gate) && voice_like(bands)
}

fn keep_utterance(voiced: f32, samples: usize, peak: f32, floor: f32) -> bool {
    voiced >= MIN_VOICED
        && samples >= crate::stt::MIN_SAMPLES
        && peak / floor.max(1e-4) >= CLOSE_TALK_SNR
}

/// Seconds between two `phase` readings. `phase` wraps every hour, so a plain
/// subtraction goes hugely negative across the wrap — which would read as "E.V.
/// just spoke" forever and leave both timers stuck in their open state.
fn since(now: f32, then: f32) -> f32 {
    (now - then).rem_euclid(3600.0)
}

/// Shortest chunk worth sending to the synthesizer. Below this the per-clip
/// overhead costs more than the sentence saves, and "Hm." on its own is a
/// worse listen than waiting for the clause it belongs to.
const MIN_SPEECH_CHUNK: usize = 24;

/// Except for the chunk that opens a reply, which also breaks on a comma and
/// at half the length. That one is the only chunk anybody waits on: until it
/// is synthesized there is silence, so a clause spoken now beats the whole
/// sentence spoken a second later. Everything after it is being made while the
/// previous one plays, where a longer chunk reads better and costs nothing.
const MIN_FIRST_CHUNK: usize = 12;

/// Split off the first complete sentence, leaving the remainder in `buf`.
///
/// Returns `None` while the buffer holds no closed sentence of usable length —
/// the caller keeps accumulating deltas and flushes the tail at end of stream.
///
/// ponytail: naive terminator scan. Splits "3.5" and "Dr. Chen" mid-sentence,
/// which costs a small pause in the wrong place, not a wrong word. Swap in a
/// real segmenter if the voice starts sounding choppy on numeric answers.
fn take_sentence(buf: &mut String, first: bool) -> Option<String> {
    let min = if first { MIN_FIRST_CHUNK } else { MIN_SPEECH_CHUNK };
    let bytes = buf.as_bytes();
    for (i, c) in buf.char_indices() {
        if i + 1 < min {
            continue;
        }
        let terminator = matches!(c, '.' | '!' | '?' | '\n' | ';' | ':') || (first && c == ',');
        // A terminator only closes a sentence when whitespace follows it, so a
        // decimal point or "e.g." inside a word does not split the clause.
        let closes = terminator
            && bytes
                .get(i + c.len_utf8())
                .is_none_or(|b| b.is_ascii_whitespace());
        if closes {
            let cut = i + c.len_utf8();
            let sentence = buf[..cut].to_string();
            buf.drain(..cut);
            // Leading space of the next sentence belongs to nobody.
            while buf.starts_with(char::is_whitespace) {
                buf.remove(0);
            }
            return Some(sentence);
        }
    }
    None
}

/// What the voice actually says: markdown stripped, so E.V. never reads
/// "asterisk asterisk" or a wall of code aloud.
fn speech_text(md: &str) -> String {
    let mut out = String::new();
    let mut in_fence = false;
    for line in md.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if !in_fence {
                out.push_str("Code omitted. ");
            }
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        // Heading/list/quote markers carry no speech.
        let body = trimmed.trim_start_matches(['#', '>', '-', '+', ' ']);
        let mut chars = body.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '*' | '_' | '`' | '~' => {}
                '!' if chars.peek() == Some(&'[') => {}
                '[' => {}
                ']' => {
                    // Keep a link's text, drop its (url).
                    if chars.peek() == Some(&'(') {
                        for c in chars.by_ref() {
                            if c == ')' {
                                break;
                            }
                        }
                    }
                }
                _ => out.push(c),
            }
        }
        out.push(' ');
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether the server's own speech endpoint is worth asking — see
/// `next_synthesis`. Process-wide because the answer is about the server, not
/// about one thread.
static SERVER_SPEECH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

type EdgeClient = msedge_tts::tts::client::MSEdgeTTSClient<std::net::TcpStream>;

/// The Edge websocket, kept open across sentences.
///
/// Opening one costs a DNS lookup, a TCP connect and a TLS + websocket
/// handshake — a second or two on a cold start, and it used to sit in front of
/// every single sentence. That handshake was most of the gap between the first
/// token appearing and the first word being spoken.
static EDGE: std::sync::Mutex<Option<EdgeClient>> = std::sync::Mutex::new(None);

fn edge_lock() -> std::sync::MutexGuard<'static, Option<EdgeClient>> {
    // A panic inside a synthesis leaves no state worth protecting: the socket
    // is dropped either way, and refusing to ever speak again is worse.
    EDGE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Open the socket now, so the first sentence of the reply does not pay for the
/// handshake. A no-op when one is already up.
fn warm_voice() -> Task<Message> {
    Task::future(async {
        let _ = tokio::task::spawn_blocking(|| {
            let mut edge = edge_lock();
            if edge.is_none() {
                *edge = msedge_tts::tts::client::connect().ok();
            }
        })
        .await;
    })
    .discard()
}

/// Neural synthesis over Edge's websocket. Blocking, so callers wrap it in
/// `spawn_blocking`; MP3 bytes come back for rodio to decode.
fn synthesize(text: &str, rate: i32) -> Result<Vec<u8>, String> {
    use msedge_tts::tts::SpeechConfig;
    let config = SpeechConfig {
        voice_name: EDGE_VOICE.into(),
        audio_format: "audio-24khz-96kbitrate-mono-mp3".into(),
        pitch: 0,
        rate,
        volume: 0,
    };
    let mut edge = edge_lock();
    let mut failure = String::new();
    // Twice: a kept socket goes away on its own — idle timeout, sleep, a
    // dropped network — and the far end's half of that only shows up here, on
    // the next send. The retry is a fresh connection, which is what the caller
    // would have got anyway.
    for _ in 0..2 {
        if edge.is_none() {
            *edge = Some(msedge_tts::tts::client::connect().map_err(|e| e.to_string())?);
        }
        match edge.as_mut().unwrap().synthesize(text, &config) {
            Ok(audio) if !audio.audio_bytes.is_empty() => return Ok(audio.audio_bytes),
            Ok(_) => failure = "empty audio".into(),
            Err(e) => failure = e.to_string(),
        }
        // Whatever went wrong, this socket is not trusted for the retry.
        *edge = None;
    }
    Err(failure)
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Fetch the provider catalog for the header dropdowns.
pub fn load_catalog(client: &Client) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move { client.llm_providers().await.map(|c| c.providers).map_err(|e| e.to_string()) },
        Message::CatalogLoaded,
    )
}

#[derive(Debug, Clone)]
pub enum Message {
    /// "View logs" on a traced error banner — intercepted in `main::update`
    /// before it reaches here, so this arm exists only to satisfy exhaustiveness.
    TraceLogs(String),
    DraftChanged(String),
    ProviderChanged(String),
    ModelChanged(String),
    /// Back to the server's default provider and model.
    UseDefaults,
    CatalogLoaded(Result<Vec<ProviderEntry>, String>),
    Send,
    /// Stop the turn in flight — Esc, or the mic button while it is talking.
    Abort,
    /// Toggle: first press starts the mic, second press stops and transcribes.
    Listen,
    /// A finished utterance: what was said, and the voice that said it.
    Heard(Result<(String, Option<crate::stt::VoicePrint>), String>),
    OpenMicSettings,
    LinkClicked(String),
    /// One chunk of the streamed reply.
    Chunk(ChatChunk),
    /// Show/hide the thinking section of message `idx`.
    ToggleReasoning(usize),
    /// The terminal finished a round of tool calls; results go back to the model.
    ToolResults(Vec<ToolOutcome>),
    Synthesized(Result<Vec<u8>, String>),
    ToggleVoice,
    Clear,
    /// Drop the enrolled voice and learn the next speaker from scratch.
    ForgetVoice,
    DismissError,
    /// Animation heartbeat, one per drawn frame, carrying the frame's own
    /// timestamp. Only runs while a screen showing the canvas is visible.
    Tick(std::time::Instant),
}

pub fn update(
    state: &mut State,
    client: &Client,
    memory: &mut crate::memory::Store,
    message: Message,
) -> Task<Message> {
    match message {
        Message::TraceLogs(_) => Task::none(),
        Message::DraftChanged(v) => {
            state.draft = v;
            Task::none()
        }
        Message::ProviderChanged(v) => {
            // The picked model belongs to the old provider; keep it only if
            // the new one also offers it.
            state.provider = v;
            if !state.model_options().iter().any(|m| m == &state.model) {
                state.model.clear();
            }
            Task::none()
        }
        Message::ModelChanged(v) => {
            state.model = v;
            Task::none()
        }
        Message::UseDefaults => {
            state.provider.clear();
            state.model.clear();
            Task::none()
        }
        Message::CatalogLoaded(Ok(providers)) => {
            // The catalog lists every provider the proxy knows, configured or
            // not. Only the configured ones can answer, so only those are
            // offered here — the rest stay in Settings → Providers until they
            // have a key or an endpoint, and appear on the next load.
            state.catalog = providers.into_iter().filter(|p| p.configured).collect();
            Task::none()
        }
        // The dropdowns just stay empty; chat itself still works on defaults.
        Message::CatalogLoaded(Err(_)) => Task::none(),
        Message::Send => {
            let prompt = state.draft.trim().to_string();
            if prompt.is_empty() {
                return Task::none();
            }
            // Interrupting replaces the turn in flight rather than being
            // dropped on the floor: you talked over it, you meant it. The old
            // answer keeps whatever it managed to say.
            state.abort_turn();
            state.push_turn("user", prompt);
            state.draft.clear();
            state.sending = true;
            state.tool_rounds = 0;
            state.tool_buf.clear();
            state.request(client)
        }
        Message::Chunk(ChatChunk::ToolCall(d)) => {
            // Fragments assemble by index: id and name arrive first, the
            // arguments JSON drips in across many chunks.
            while state.tool_buf.len() <= d.index {
                state.tool_buf.push(ToolCall::default());
            }
            let tc = &mut state.tool_buf[d.index];
            if let Some(id) = d.id {
                tc.id = id;
            }
            if let Some(name) = d.name {
                tc.function.name = name;
            }
            tc.function.arguments.push_str(&d.arguments);
            Task::none()
        }
        Message::ToolResults(results) => {
            for r in results {
                // Wire content is the raw output; the transcript shows it fenced.
                state
                    .md
                    .push(iced::widget::markdown::parse(&format!("````text\n{}\n````", r.output)).collect());
                state.reasoning.push(String::new());
                state.messages.push(ChatMessage {
                    role: "tool".into(),
                    content: r.output,
                    tool_calls: None,
                    tool_call_id: Some(r.id),
                });
            }
            state.tool_rounds += 1;
            // Still `sending`: the reply continues with the results in hand.
            state.request(client)
        }
        Message::Chunk(ChatChunk::Reasoning(text)) => {
            // Shown in the transcript, never spoken: E.V. reading its own
            // chain-of-thought aloud would bury the answer.
            state.push_reasoning(&text);
            iced::widget::operation::snap_to_end(transcript_id())
        }
        Message::ToggleReasoning(idx) => {
            if !state.reasoning_open.remove(&idx) {
                state.reasoning_open.insert(idx);
            }
            Task::none()
        }
        Message::Chunk(ChatChunk::Delta(text)) => {
            state.push_delta(&text);
            // The first sentence goes to the synthesizer while the rest of the
            // reply is still being generated — that is the whole latency win.
            Task::batch([
                iced::widget::operation::snap_to_end(transcript_id()),
                state.next_synthesis(client),
            ])
        }
        Message::Chunk(ChatChunk::Done) => {
            let streamed = std::mem::take(&mut state.streaming);
            // The stream ended on its own; there is nothing left to abort.
            state.abort = None;
            // Whatever was said this round gets spoken either way; the tail
            // after the last terminator is a sentence too.
            if state.voice && !state.speech_buf.trim().is_empty() {
                let tail = std::mem::take(&mut state.speech_buf);
                state.enqueue_speech(&tail);
            }
            state.speech_buf.clear();
            // Some local models write the call they meant to make as prose
            // instead of emitting it — seen live, and the cost is a silent
            // no-op: the user is told their memory changed and it did not.
            if state.tool_buf.is_empty() && streamed {
                let last = state.messages.len() - 1;
                if let Some(calls) = salvage_calls(&state.messages[last].content) {
                    state.messages[last].content.clear();
                    state.md[last] = Vec::new();
                    state.speech_queue.clear();
                    state.tool_buf = calls;
                }
            }
            if !state.tool_buf.is_empty() {
                // Tool round: attach the calls to the assistant turn (opening
                // one if the model sent no text), show what runs, and keep
                // `sending` — the HUD stays in Thinking while the terminal works.
                let calls = std::mem::take(&mut state.tool_buf);
                if !streamed {
                    state.push_turn("assistant", String::new());
                }
                let last = state.messages.len() - 1;
                state.messages[last].tool_calls = Some(calls.clone());
                let mut shown = state.messages[last].content.clone();
                for c in &calls {
                    shown.push_str(&format!("\n\n```\n$ {}\n```", command_of(c)));
                }
                state.md[last] = iced::widget::markdown::parse(&shown).collect();
                // Memory calls are answered here and now, against the live store
                // the dashboard shows; only the terminal ones need a task.
                let (mem_calls, shell_calls): (Vec<_>, Vec<_>) = calls
                    .into_iter()
                    .partition(|c| crate::memory::TOOLS.contains(&c.function.name.as_str()));
                let done: Vec<ToolOutcome> = mem_calls
                    .into_iter()
                    .filter_map(|c| {
                        crate::memory::run_tool(memory, &c.function.name, &c.function.arguments)
                            .map(|output| ToolOutcome { id: c.id, output })
                    })
                    .collect();
                if !done.is_empty() {
                    // The next request in this same turn must recall what was
                    // just written, not what was remembered before it.
                    state.memory = memory.system_block();
                }
                return Task::batch([
                    iced::widget::operation::snap_to_end(transcript_id()),
                    state.next_synthesis(client),
                    Task::perform(run_tools(shell_calls, done), Message::ToolResults),
                ]);
            }
            state.sending = false;
            // A turn that produced neither a word nor a tool call: some models
            // do this, and without a word here the user is left staring at
            // their own message wondering whether anything happened.
            if !streamed {
                state.error = Some("The model returned an empty reply. Ask again.".into());
            }
            // Opens the follow-up window: the answer invites a follow-up, and
            // having to say "E.V." again mid-conversation is not conversation.
            state.last_reply = state.phase;
            Task::batch([
                iced::widget::operation::snap_to_end(transcript_id()),
                state.next_synthesis(client),
            ])
        }
        Message::Chunk(ChatChunk::Failed(e)) => {
            state.sending = false;
            state.streaming = false;
            state.abort = None;
            state.tool_buf.clear();
            state.error = Some(e);
            // Speak what did arrive rather than cutting off mid-word.
            if state.voice && !state.speech_buf.trim().is_empty() {
                let tail = std::mem::take(&mut state.speech_buf);
                state.enqueue_speech(&tail);
            }
            state.speech_buf.clear();
            state.next_synthesis(client)
        }
        Message::Abort => {
            state.abort_turn();
            Task::none()
        }
        Message::Listen => {
            // Toggle hands-free listening. Off drops the stream, which is the
            // only honest way to say "the mic is not on".
            if state.recorder.take().is_some() {
                state.capture = None;
                state.onset = 0;
                state.armed = false;
                return Task::none();
            }
            state.hush();
            match crate::stt::Recorder::start() {
                Ok(rec) => {
                    state.recorder = Some(rec);
                    state.capture = None;
                    state.onset = 0;
                    state.floor = ABS_FLOOR;
                    // You pressed the button, so anything you say while it is on
                    // is obviously for E.V. — no need to name it, and no
                    // deadline to say it by.
                    state.last_reply = state.phase;
                    state.armed = true;
                }
                Err(e) => state.error = Some(e),
            }
            Task::none()
        }
        Message::Heard(result) => {
            state.transcribing = false;
            match result {
                Ok((text, print)) if !text.trim().is_empty() && !is_ghost(&text) => {
                    // Whose voice was that? Until enrolled, everything that got
                    // through the gate counts as yours; after that, a voice this
                    // far from the enrolled one is somebody else in the room.
                    state.voice_sim = match (state.voice_print, print) {
                        (Some(known), Some(heard)) => {
                            Some(crate::stt::print_similarity(&known, &heard))
                        }
                        _ => None,
                    };
                    let mine = match state.voice_sim {
                        // Nothing to compare against: the very first utterance,
                        // or audio too short to print. Yours by default.
                        None => true,
                        Some(sim) if state.voice_enrolled() => sim >= VOICE_MATCH,
                        // Still enrolling. The first utterance seeded the print;
                        // everything averaged in after it still has to be the
                        // same person, or somebody who talks during your first
                        // four sentences becomes half of "you" permanently.
                        Some(sim) => sim >= ENROLL_MATCH,
                    };
                    if mine {
                        state.learn_voice(print);
                    }
                    // Addressed by name, or spoken inside the follow-up window
                    // after E.V.'s last reply → it was meant for E.V.
                    let follow_up = state.follow_up_open() && mine;
                    // Another voice never auto-sends, however it phrased itself:
                    // it lands in the composer for you to send or discard.
                    let addressed = if mine { addressed(&text) } else { None };
                    match (addressed, follow_up) {
                        (Some(body), _) if !body.trim().is_empty() => {
                            state.draft = body.to_string();
                            update(state, client, memory, Message::Send)
                        }
                        // "E.V.?" on its own is still someone calling it.
                        (Some(_), _) => {
                            state.draft = "?".to_string();
                            update(state, client, memory, Message::Send)
                        }
                        (None, true) => {
                            state.draft = text;
                            update(state, client, memory, Message::Send)
                        }
                        // Heard, but nothing said it was for E.V. — park it in
                        // the composer rather than answering the room. Appended,
                        // not assigned: overwriting would eat what you typed, and
                        // eat the first half of a sentence the gate split in two.
                        (None, false) => {
                            if !state.draft.trim().is_empty() {
                                state.draft.push(' ');
                            }
                            state.draft.push_str(&text);
                            Task::none()
                        }
                    }
                }
                // Whisper found no words in it: it was noise after all. Silent
                // by design — hands-free must not nag about every passing sound.
                Ok(_) => Task::none(),
                Err(e) => {
                    state.error = Some(e);
                    Task::none()
                }
            }
        }
        Message::Synthesized(result) => {
            state.synthesizing = false;
            match result {
                // Queued rather than played: the previous sentence may still be
                // in the sink. `drain_audio` starts it the moment that ends.
                Ok(bytes) => state.audio_queue.push_back(bytes),
                Err(_) => {
                    // Offline or no audio device for rodio — native engine's turn
                    // for the sentence that failed. Queued, not interrupting, so
                    // the earlier sentences of this reply still finish.
                    if let Some(text) = state.speaking.take() {
                        state.speak_native(&text);
                    }
                }
            }
            state.drain_audio();
            state.next_synthesis(client)
        }
        Message::OpenMicSettings => {
            // ms-settings: deep link straight to Privacy → Microphone.
            crate::shell::reveal_path("ms-settings:privacy-microphone");
            Task::none()
        }
        Message::ToggleVoice => {
            state.voice = !state.voice;
            if !state.voice {
                // Leaving voice mode closes the mic too — the HUD it was
                // reported on is gone, and a live mic nobody can see is the one
                // thing this screen must never do.
                state.hush();
                state.recorder = None;
                state.capture = None;
                state.onset = 0;
                state.armed = false;
            }
            Task::none()
        }
        Message::Clear => {
            state.hush();
            state.messages.clear();
            state.md.clear();
            state.reasoning.clear();
            state.reasoning_open.clear();
            state.streaming = false;
            state.tool_buf.clear();
            state.tool_rounds = 0;
            state.error = None;
            Task::none()
        }
        Message::ForgetVoice => {
            state.forget_voice();
            Task::none()
        }
        Message::LinkClicked(url) => {
            // Only open real web links — a hallucinated file path via explorer
            // would be a surprise.
            if url.starts_with("http://") || url.starts_with("https://") {
                crate::shell::reveal_path(&url);
            }
            Task::none()
        }
        Message::DismissError => {
            state.error = None;
            Task::none()
        }
        Message::Tick(now) => {
            // Visual time is measured, never assumed: this fires once per drawn
            // frame, and that rate is the display's, not ours — 60, 120 or a
            // stalled 12 during a GC pause. Advancing a constant per frame made
            // the animation speed lurch with the frame time, which is what a
            // wobbling blob reads as "not smooth". Clamped so a long stall
            // resumes instead of teleporting.
            let dt = state
                .last_frame
                .map_or(DT, |prev| now.saturating_duration_since(prev).as_secs_f32())
                .clamp(0.0, 0.1);
            state.last_frame = Some(now);
            state.phase = (state.phase + dt) % 3600.0;
            state.elapsed += dt;
            state.boot = (state.boot + dt * 0.9).min(1.0);
            // Mode changes crossfade rather than cut: colour, radii and labels
            // all ease across `mode_t`.
            state.mode_t = (state.mode_t + dt * 3.5).min(1.0);

            // Everything below is the analyzer and the mic gate, whose windows,
            // frame counters and time constants are all written against 60 Hz.
            // Drawing runs at whatever the display does; this does not.
            state.audio_acc += dt;
            if state.audio_acc < DT {
                return Task::none();
            }
            // Never bank more than one frame of debt: after a stall the gate
            // catches up by continuing, not by replaying the backlog.
            state.audio_acc = (state.audio_acc - DT).min(DT);

            // Start the next sentence the frame after the current one ends, so
            // the gap between them is inaudible.
            state.drain_audio();

            // One analysis pass drives everything visual: bands, overall energy,
            // the transient flash and the scrolling waveform.
            let mut fresh = [0.0_f32; BANDS];
            let mut rms = 0.0;
            if let Some((mono, rate)) = state.source_tail() {
                crate::stt::bands(&mono, rate, &mut fresh);
                if !mono.is_empty() {
                    rms = (mono.iter().map(|s| s * s).sum::<f32>() / mono.len() as f32).sqrt();
                }
            }
            // Fast attack, slow release, per band — bars snap up on a consonant
            // and sag through the vowel instead of chattering.
            for (b, f) in state.bands.iter_mut().zip(fresh) {
                *b = if f > *b { *b + (f - *b) * 0.6 } else { *b * 0.90 };
            }
            let loud = (rms * 6.0).clamp(0.0, 1.0);
            let jump = (loud - state.energy).max(0.0);
            // Eased on the way up as well as down. Snapping straight to `loud`
            // stepped the orb's radius by a whole consonant in one frame, which
            // is a visible pop; `beat` below is the channel that still wants the
            // transient, so nothing is lost by smoothing this one.
            state.energy += (loud - state.energy) * if loud > state.energy { 0.35 } else { 0.08 };
            state.beat = (state.beat * 0.88).max((jump * 5.0).min(1.0));
            state.wave.push(state.energy);
            if state.wave.len() > WAVE {
                state.wave.drain(..state.wave.len() - WAVE);
            }

            let mode = state.mode();
            if mode != state.mode_now {
                state.mode_prev = state.mode_now;
                state.mode_now = mode;
                state.mode_t = 0.0;
                state.elapsed = 0.0;
            }

            let Some(rec) = &state.recorder else {
                if state.speaking() {
                    state.spoke_at = state.phase;
                }
                // Not armed: the meter tracks E.V.'s own voice instead, so the
                // web reacts to whoever is talking.
                state.mic_level =
                    if loud > state.mic_level { loud } else { state.mic_level * 0.95 };
                return Task::none();
            };

            // --- The gate ---------------------------------------------------
            // Everything below runs on the mic's own RMS, never on the tail the
            // HUD happens to be drawing: while E.V. speaks that tail is E.V.
            let mic = rec.rms();
            let meter = rec.level();
            state.mic_level =
                if meter > state.mic_level { meter } else { state.mic_level * 0.95 };

            let gate = (state.floor * OPEN_SNR).max(ABS_FLOOR);

            // --- Barge-in ----------------------------------------------------
            // While E.V.'s voice is playing the gate stays shut — the mic hears
            // the speakers — but the mic is still *read*, because talking over
            // the top of a reply is how you stop it.
            if state.speaking() {
                // `state.bands` is E.V.'s own playback while it speaks (the HUD
                // has to move on headphones too), so the shape test needs its
                // own pass over the mic.
                let (tail, rate) = rec.tail_mono(WINDOW);
                let mut heard = [0.0_f32; BANDS];
                crate::stt::bands(&tail, rate, &mut heard);
                // Rises slowly and falls fast — the opposite of the room floor.
                // A bleed that chased the mic upward would absorb the person
                // interrupting and never fire.
                let adapt = if mic > state.bleed { 0.02 } else { 0.2 };
                state.bleed += (mic - state.bleed) * adapt;
                state.barge = if over_playback(mic, state.bleed, gate, &heard) {
                    state.barge + 1
                } else {
                    0
                };
                if state.barge >= BARGE_FRAMES {
                    state.barge = 0;
                    // Drop the rest of the reply, spoken and queued, and let the
                    // gate open on whatever is being said over it.
                    state.hush();
                }
                state.spoke_at = state.phase;
                state.onset = 0;
                return Task::none();
            }
            state.barge = 0;

            // The floor learns the room whenever nothing is being captured:
            // fast down to a quiet moment, slow up so a sentence never becomes
            // "normal" (rise constant ~30 s, fall ~0.3 s).
            if state.capture.is_none() {
                let rate = if mic < state.floor { 0.05 } else { 0.0006 };
                state.floor += (mic - state.floor) * rate;
            }
            let speechy = mic > gate && voice_like(&state.bands);

            // Only the reverb tail of E.V.'s last word blocks the gate now.
            // Thinking and synthesizing deliberately do not: they are silent,
            // and a mic that goes deaf between your sentences drops the half of
            // the thought you said while it was catching up.
            let blocked = since(state.phase, state.spoke_at) < ECHO_TAIL;

            match state.capture {
                None => {
                    state.onset = if speechy && !blocked { state.onset + 1 } else { 0 };
                    if state.onset >= ONSET_FRAMES {
                        let preroll = (PREROLL * rec.rate() as f32) as u64;
                        state.capture = Some(rec.now().saturating_sub(preroll));
                        state.onset = 0;
                        state.hang = 0.0;
                        state.voiced = 0.0;
                        state.peak = mic;
                    }
                }
                Some(start) => {
                    if speechy {
                        state.hang = 0.0;
                        state.voiced += DT;
                    } else {
                        state.hang += DT;
                    }
                    state.peak = state.peak.max(mic);
                    let held = (rec.now().saturating_sub(start)) as f32 / rec.rate() as f32;
                    if state.hang > HANG || held > MAX_UTTERANCE {
                        let samples = rec.since(start);
                        state.capture = None;
                        state.hang = 0.0;
                        let voiced = std::mem::take(&mut state.voiced);
                        let peak = std::mem::take(&mut state.peak);
                        if !keep_utterance(voiced, samples.len(), peak, state.floor) {
                            return Task::none();
                        }
                        state.transcribing = true;
                        return Task::perform(
                            async move {
                                // Watchdog: first run downloads the model
                                // (~60 MB), so the deadline is generous — but
                                // the UI must never wedge in "analyzing".
                                match tokio::time::timeout(
                                    std::time::Duration::from_secs(180),
                                    tokio::task::spawn_blocking(move || {
                                        // The print comes from the same audio,
                                        // on the same thread: whose voice it
                                        // was is part of what was heard.
                                        let print = crate::stt::voice_print(
                                            &samples,
                                            crate::stt::WHISPER_RATE,
                                        );
                                        crate::stt::transcribe(&samples)
                                            .map(|text| (text, print))
                                    }),
                                )
                                .await
                                {
                                    Ok(joined) => joined.unwrap_or_else(|e| Err(e.to_string())),
                                    Err(_) => Err("Transcription timed out — if this was \
                                                   the first use, the model download may \
                                                   be slow; try again."
                                        .to_string()),
                                }
                            },
                            Message::Heard,
                        );
                    }
                }
            }
            Task::none()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Client {
        Client::new("http://127.0.0.1:1", "k")
    }

    /// A throwaway memory store per call: the tests below exercise the chat
    /// loop, not the store, and one that writes nowhere real is enough.
    fn mem() -> crate::memory::Store {
        crate::memory::Store::load(&std::env::temp_dir().join("ev-assistant-test-memory"))
    }

    #[test]
    fn sending_appends_turn_and_thinks() {
        let mut s = State { draft: " status? ".into(), ..State::new() };
        let _ = update(&mut s, &client(), &mut mem(), Message::Send);
        assert_eq!(s.messages.len(), 1);
        assert_eq!(s.messages[0].content, "status?");
        assert!(s.sending);
        assert_eq!(s.mode(), Mode::Thinking);
    }

    #[test]
    fn a_blank_send_is_ignored_but_an_interrupting_one_replaces_the_turn() {
        let mut s = State { draft: "  ".into(), ..State::new() };
        let _ = update(&mut s, &client(), &mut mem(), Message::Send);
        assert!(s.messages.is_empty());

        // Sending over the top of a reply that is still arriving: the new turn
        // goes through and the old stream is cut, rather than the new turn
        // being silently dropped.
        let mut s =
            State { draft: "hi".into(), sending: true, streaming: true, ..State::new() };
        let _ = update(&mut s, &client(), &mut mem(), Message::Send);
        assert_eq!(s.messages.len(), 1);
        assert_eq!(s.messages[0].content, "hi");
        assert!(!s.streaming, "the interrupted stream is closed, not left open");
    }

    #[test]
    fn abort_stops_the_turn_and_keeps_what_was_already_said() {
        let mut s = State { draft: "hi".into(), ..State::new() };
        let _ = update(&mut s, &client(), &mut mem(), Message::Send);
        let _ = update(&mut s, &client(), &mut mem(), Message::Chunk(ChatChunk::Delta("Half".into())));
        let _ = update(&mut s, &client(), &mut mem(), Message::Abort);
        assert!(!s.sending);
        assert!(!s.streaming);
        assert_eq!(s.messages.last().unwrap().content, "Half", "the words it got out stay");
    }

    /// A whole reply arriving as one delta plus the end-of-stream marker.
    fn reply(s: &mut State, text: &str) {
        let _ = update(s, &client(), &mut mem(), Message::Chunk(ChatChunk::Delta(text.into())));
        let _ = update(s, &client(), &mut mem(), Message::Chunk(ChatChunk::Done));
    }

    #[test]
    fn failed_turn_stays_with_error() {
        let mut s = State { draft: "hi".into(), ..State::new() };
        let _ = update(&mut s, &client(), &mut mem(), Message::Send);
        let _ = update(&mut s, &client(), &mut mem(), Message::Chunk(ChatChunk::Failed("boom".into())));
        assert_eq!(s.messages.len(), 1);
        assert!(!s.sending);
        assert_eq!(s.error.as_deref(), Some("boom"));
    }

    #[test]
    fn muted_reply_skips_synthesis() {
        let mut s = State { voice: false, ..State::new() };
        reply(&mut s, "hello");
        assert_eq!(s.messages.len(), 1);
        assert!(!s.synthesizing);
        assert_eq!(s.mode(), Mode::Idle);

        let _ = update(&mut s, &client(), &mut mem(), Message::ToggleVoice);
        assert!(s.voice);
    }

    #[test]
    fn reasoning_is_shown_but_never_spoken() {
        let mut s = State::new(); // voice on
        let _ = update(
            &mut s,
            &client(),
            &mut mem(),
            Message::Chunk(ChatChunk::Reasoning("User asks for status; check the basics.".into())),
        );
        // Reasoning alone opened the turn and nothing went to the synthesizer.
        assert_eq!(s.messages.len(), 1);
        assert!(s.reasoning_live(0));
        assert!(!s.synthesizing);
        let _ = update(&mut s, &client(), &mut mem(), Message::Chunk(ChatChunk::Delta("Systems nominal.".into())));
        let _ = update(&mut s, &client(), &mut mem(), Message::Chunk(ChatChunk::Done));
        assert_eq!(s.reasoning[0], "User asks for status; check the basics.");
        assert_eq!(s.messages[0].content, "Systems nominal.");
        // What reached the voice is the reply, not the deliberation.
        assert_eq!(s.speaking.as_deref(), Some("Systems nominal."));
    }

    /// A catalog row as the proxy sends it: every known provider, whether or not
    /// it has credentials.
    fn entry(id: &str, model: &str, configured: bool) -> ProviderEntry {
        use agent_platform_client::types::ProviderModels;
        ProviderEntry {
            id: id.into(),
            label: id.into(),
            configured,
            local: false,
            models: ProviderModels {
                options: vec![model.into()],
                selected_model: model.into(),
                source: "discovery".into(),
                warning: None,
                fallback_note: None,
            },
        }
    }

    #[test]
    fn unconfigured_providers_are_not_offered() {
        let mut s = State::new();
        let _ = update(
            &mut s,
            &client(),
            &mut mem(),
            Message::CatalogLoaded(Ok(vec![
                entry("ollama", "qwen2.5:7b", true),
                entry("gemini", "gemini-2.0-flash", false),
            ])),
        );
        assert_eq!(s.provider_ids(), ["ollama"]);
        // No provider picked means "every model on offer" — still only the
        // configured provider's.
        assert_eq!(s.model_options(), ["qwen2.5:7b"]);
    }

    #[test]
    fn switching_provider_drops_a_model_it_does_not_offer() {
        let mut s = State {
            catalog: vec![entry("a", "a-model", true), entry("b", "b-model", true)],
            provider: "a".into(),
            model: "a-model".into(),
            ..State::new()
        };
        let _ = update(&mut s, &client(), &mut mem(), Message::ProviderChanged("b".into()));
        assert!(s.model.is_empty(), "a-model does not exist on provider b");

        let _ = update(&mut s, &client(), &mut mem(), Message::ModelChanged("b-model".into()));
        let _ = update(&mut s, &client(), &mut mem(), Message::UseDefaults);
        assert!(s.provider.is_empty() && s.model.is_empty());
    }

    #[test]
    fn speech_text_strips_markdown() {
        assert_eq!(speech_text("**Systems** are `nominal`."), "Systems are nominal.");
        assert_eq!(speech_text("# Status\n- web: *ok*\n- net: ok"), "Status web: ok net: ok");
        assert_eq!(
            speech_text("See [the docs](https://example.com) now."),
            "See the docs now."
        );
        assert_eq!(
            speech_text("Run this:\n```rust\nfn main() {}\n```\nDone."),
            "Run this: Code omitted. Done."
        );
    }

    #[test]
    fn heard_text_autosends_and_silence_does_not() {
        let mut s = State { transcribing: true, ..State::new() };
        let _ = update(&mut s, &client(), &mut mem(), Message::Heard(Ok(("run diagnostics".into(), None))));
        assert!(!s.transcribing);
        assert_eq!(s.messages.len(), 1);
        assert_eq!(s.messages[0].content, "run diagnostics");
        assert!(s.sending);

        let mut s = State { transcribing: true, ..State::new() };
        let _ = update(&mut s, &client(), &mut mem(), Message::Heard(Ok(("  ".into(), None))));
        assert!(s.messages.is_empty());
        assert!(!s.sending);

        let mut s = State { transcribing: true, ..State::new() };
        let _ = update(&mut s, &client(), &mut mem(), Message::Heard(Err("mic missing".into())));
        assert_eq!(s.error.as_deref(), Some("mic missing"));
    }

    #[test]
    fn only_speech_aimed_at_ev_is_sent_on_its_own() {
        // Outside the follow-up window, unaddressed speech is parked in the
        // composer — heard, but not answered.
        let mut s = State { transcribing: true, phase: 60.0, ..State::new() };
        let _ = update(&mut s, &client(), &mut mem(), Message::Heard(Ok(("so anyway I told him no".into(), None))));
        assert!(s.messages.is_empty(), "room chatter must not reach the model");
        assert_eq!(s.draft, "so anyway I told him no");
        assert!(!s.sending);

        // Naming E.V. sends it, and the name itself is not part of the question.
        let mut s = State { transcribing: true, phase: 60.0, ..State::new() };
        let _ = update(&mut s, &client(), &mut mem(), Message::Heard(Ok(("Hey Eve, run diagnostics".into(), None))));
        assert_eq!(s.messages.len(), 1);
        assert_eq!(s.messages[0].content, "run diagnostics");

        // A reply opens the window: the next thing said needs no name.
        let mut s = State { phase: 60.0, voice: false, ..State::new() };
        reply(&mut s, "Done.");
        s.transcribing = true;
        let _ = update(&mut s, &client(), &mut mem(), Message::Heard(Ok(("and the second one?".into(), None))));
        assert_eq!(s.messages.last().unwrap().content, "and the second one?");
    }

    #[test]
    fn a_stranger_never_auto_sends_however_they_phrase_it() {
        let mine = [0.5_f32; crate::stt::PRINT_DIM];
        let mut theirs = mine;
        // Flip half the coefficients: an unmistakably different voice.
        for v in theirs.iter_mut().take(crate::stt::PRINT_DIM / 2) {
            *v = -*v;
        }

        let enrolled = |print| State {
            voice_print: Some(print),
            enrolled: ENROLL_UTTERANCES,
            transcribing: true,
            ..State::new()
        };

        // Names E.V., is inside the follow-up window, still parked.
        let mut s = enrolled(mine);
        let _ = update(
            &mut s,
            &client(),
            &mut mem(),
            Message::Heard(Ok(("E.V., wipe the database".into(), Some(theirs)))),
        );
        assert!(s.messages.is_empty(), "a stranger must not reach the model");
        assert_eq!(s.draft, "E.V., wipe the database");

        // The enrolled voice, same words, goes straight out.
        let mut s = enrolled(mine);
        let _ = update(
            &mut s,
            &client(),
            &mut mem(),
            Message::Heard(Ok(("E.V., run diagnostics".into(), Some(mine)))),
        );
        assert_eq!(s.messages.len(), 1);
        assert_eq!(s.messages[0].content, "run diagnostics");

        // Before enrollment finishes, nothing is rejected — that is what the
        // first few utterances are for.
        let mut s = State { transcribing: true, ..State::new() };
        let _ = update(&mut s, &client(), &mut mem(), Message::Heard(Ok(("hello there".into(), Some(theirs)))));
        assert_eq!(s.messages.len(), 1);
        assert!(s.voice_print.is_some());
    }

    #[test]
    fn address_matching_survives_how_whisper_spells_it() {
        assert_eq!(addressed("E.V., status?"), Some("status?"));
        assert_eq!(addressed("hey EV run the build"), Some("run the build"));
        assert_eq!(addressed("Evie: what broke"), Some("what broke"));
        assert_eq!(addressed("EV"), Some(""));
        // Not addressed: the name has to come first, not turn up mid-sentence.
        assert_eq!(addressed("what's the weather"), None);
        assert_eq!(addressed("tell Eve I said hi"), None);
        assert_eq!(addressed(""), None);
    }

    #[test]
    fn the_gate_ignores_noise_that_is_not_voice_shaped() {
        // Flat broadband hiss and low rumble both fail the speech-band test;
        // a voice-shaped frame passes it.
        let mut flat = [0.5_f32; BANDS];
        assert!(!voice_like(&flat));
        flat[..4].fill(1.0);
        flat[4..].fill(0.02);
        assert!(!voice_like(&flat), "bass rumble is not speech");

        let mut voice = [0.02_f32; BANDS];
        for (i, b) in voice.iter_mut().enumerate() {
            let hz = crate::stt::band_freq(i, BANDS);
            if (150.0..4000.0).contains(&hz) {
                *b = 0.8;
            }
        }
        assert!(voice_like(&voice));
        assert!(!voice_like(&[0.0; BANDS]), "silence is not speech");

        // A realistic voice frame is not clean: the fundamental of a deep
        // voice sits below 150 Hz, and the HUD's tilt inflates the bins above
        // 4 kHz. Concentration lands only a few points above flat — this is
        // the frame the old 1.1 margin rejected, which made hands-free deaf.
        let mut real = [0.0_f32; BANDS];
        for (i, b) in real.iter_mut().enumerate() {
            let hz = crate::stt::band_freq(i, BANDS);
            *b = if (150.0..4000.0).contains(&hz) {
                0.65 // formants
            } else if hz < 150.0 {
                0.6 // fundamental
            } else {
                0.35 // tilt-boosted sibilant leakage
            };
        }
        assert!(voice_like(&real), "a bass-heavy real voice must open the gate");
    }

    #[test]
    fn a_quiet_mic_keeps_its_utterance_and_noise_still_drops() {
        let second = crate::stt::WHISPER_RATE as usize;
        // Low-gain mic in a quiet room: peak 0.012 is only twice ABS_FLOOR but
        // 6x the actual learned floor. Clamping the divisor at ABS_FLOOR made
        // this read as SNR 2 and silently dropped every utterance.
        assert!(keep_utterance(1.0, 2 * second, 0.012, 0.002));
        // Same room, someone talking across it: not enough over the floor.
        assert!(!keep_utterance(1.0, 2 * second, 0.008, 0.002));
        // Loud room, distant conversation: relative test still rejects.
        assert!(!keep_utterance(1.0, 2 * second, 0.03, 0.01));
        // A cough is loud but too brief; a stray click is too few samples.
        assert!(!keep_utterance(0.1, 2 * second, 0.1, 0.002));
        assert!(!keep_utterance(1.0, second / 4, 0.1, 0.002));
    }

    #[test]
    #[ignore = "network: hits Edge's TTS endpoint"]
    fn live_edge_synthesis() {
        let bytes = synthesize("Systems online. Good to see you.", DEFAULT_VOICE_RATE).unwrap();
        assert!(bytes.len() > 1000, "suspiciously small audio: {} bytes", bytes.len());
        // The whole point of keeping the socket: sentence two must not need a
        // second handshake. If Edge closed the turn, the retry inside
        // `synthesize` still answers — but silently, and the latency win is
        // gone, so assert the connection itself survived.
        let again = synthesize("Second sentence, same socket.", DEFAULT_VOICE_RATE).unwrap();
        assert!(again.len() > 1000);
        assert!(edge_lock().is_some(), "the socket was thrown away between sentences");
    }

    #[test]
    #[ignore = "network + model download: full TTS→STT round trip"]
    fn live_voice_round_trip() {
        use rodio::Source;
        let bytes = synthesize("Hello Peter, systems are online.", DEFAULT_VOICE_RATE).unwrap();
        let decoder = rodio::Decoder::new(std::io::Cursor::new(bytes)).unwrap();
        let (channels, rate) = (decoder.channels(), decoder.sample_rate());
        let raw: Vec<f32> = decoder.convert_samples().collect();
        let samples = crate::stt::mono_16k(&raw, channels, rate);
        let text = crate::stt::transcribe(&samples).unwrap().to_lowercase();
        assert!(text.contains("systems"), "whisper heard: {text:?}");
    }

    #[test]
    fn voiced_reply_enters_synthesis() {
        let mut s = State::new();
        reply(&mut s, "hi");
        assert!(s.synthesizing);
        assert_eq!(s.mode(), Mode::Thinking);
    }

    #[test]
    fn the_first_sentence_is_synthesized_before_the_reply_ends() {
        let mut s = State { draft: "status".into(), ..State::new() };
        let _ = update(&mut s, &client(), &mut mem(), Message::Send);
        // Mid-stream: one closed sentence, one still arriving.
        let _ = update(
            &mut s,
            &client(),
            &mut mem(),
            Message::Chunk(ChatChunk::Delta("Systems are nominal, boss. And the".into())),
        );
        assert!(s.synthesizing, "sentence one goes out while the reply is still streaming");
        // The opening chunk leaves at the comma — nothing is playing yet, so
        // the shortest usable clause wins.
        assert_eq!(s.speaking.as_deref(), Some("Systems are nominal,"));
        // Back to whole sentences behind it, so what is left is under the
        // ordinary minimum and waits.
        assert_eq!(s.speech_buf, "boss. And the", "the open clause waits for its terminator");
        assert!(s.sending, "the turn is not over yet");

        // The tail flushes at end of stream even without a terminator.
        let _ = update(&mut s, &client(), &mut mem(), Message::Chunk(ChatChunk::Delta(" rest".into())));
        let _ = update(&mut s, &client(), &mut mem(), Message::Chunk(ChatChunk::Done));
        assert_eq!(s.speech_queue.front().map(String::as_str), Some("boss. And the rest"));
        assert_eq!(s.messages.last().unwrap().content, "Systems are nominal, boss. And the rest");
        assert!(!s.sending);
    }

    #[test]
    fn a_tool_round_runs_the_terminal_and_keeps_the_turn_open() {
        use agent_platform_client::sse::ToolCallDelta;
        let mut s = State { draft: "how much disk is free?".into(), voice: false, ..State::new() };
        let _ = update(&mut s, &client(), &mut mem(), Message::Send);

        // The model narrates, then asks for the terminal in streamed fragments.
        let _ = update(&mut s, &client(), &mut mem(), Message::Chunk(ChatChunk::Delta("Checking.".into())));
        for d in [
            ToolCallDelta {
                index: 0,
                id: Some("call_1".into()),
                name: Some("run_command".into()),
                arguments: "{\"command\": \"Get-".into(),
            },
            ToolCallDelta { index: 0, arguments: "PSDrive C\"}".into(), ..Default::default() },
        ] {
            let _ = update(&mut s, &client(), &mut mem(), Message::Chunk(ChatChunk::ToolCall(d)));
        }
        let _ = update(&mut s, &client(), &mut mem(), Message::Chunk(ChatChunk::Done));

        let turn = s.messages.last().unwrap();
        let calls = turn.tool_calls.as_ref().expect("calls attached to the assistant turn");
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].function.arguments, "{\"command\": \"Get-PSDrive C\"}");
        assert_eq!(command_of(&calls[0]), "Get-PSDrive C");
        assert!(s.sending, "the turn stays open while the terminal works");
        assert!(s.tool_buf.is_empty());

        // Results come back: a tool message joins the wire history and the
        // request goes out again with the round counted.
        let _ = update(
            &mut s,
            &client(),
            &mut mem(),
            Message::ToolResults(vec![ToolOutcome { id: "call_1".into(), output: "Free: 120G".into() }]),
        );
        let tool = s.messages.last().unwrap();
        assert_eq!(tool.role, "tool");
        assert_eq!(tool.tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(tool.content, "Free: 120G");
        assert_eq!(s.tool_rounds, 1);
        assert!(s.sending);
        assert_eq!(s.messages.len(), s.md.len(), "transcript and markdown stay aligned");
    }

    /// A memory call is answered against the store the dashboard shows, and the
    /// fact it wrote is in the recall of the very next request of that turn.
    #[test]
    fn a_memory_call_writes_the_store_and_refreshes_recall() {
        use agent_platform_client::sse::ToolCallDelta;
        let mut store = mem();
        let _ = crate::memory::update(&mut store, crate::memory::Message::ForgetAll);
        let _ = crate::memory::update(&mut store, crate::memory::Message::ForgetAll);

        let mut s = State { draft: "remember I live in London".into(), ..State::new() };
        let _ = update(&mut s, &client(), &mut store, Message::Send);
        let d = ToolCallDelta {
            index: 0,
            id: Some("c1".into()),
            name: Some("remember".into()),
            arguments: r#"{"text": "Lives in London."}"#.into(),
        };
        let _ = update(&mut s, &client(), &mut store, Message::Chunk(ChatChunk::ToolCall(d)));
        let _ = update(&mut s, &client(), &mut store, Message::Chunk(ChatChunk::Done));

        assert_eq!(store.items.len(), 1, "the call reached the live store");
        assert_eq!(store.items[0].text, "Lives in London.");
        assert!(
            s.memory.as_deref().is_some_and(|r| r.contains("Lives in London.")),
            "the fact is recalled on the follow-up request, not the next restart"
        );
        // The transcript shows the call, and it is not rendered as a command.
        let calls = s.messages.last().unwrap().tool_calls.as_ref().expect("calls attached");
        assert!(command_of(&calls[0]).starts_with("remember("));
        assert!(s.sending, "the turn stays open until the result comes back");
    }

    /// Local models sometimes type the call instead of making it. Rescuing that
    /// turn is the difference between a memory written and a memory promised.
    #[test]
    fn a_call_written_out_as_prose_is_still_run() {
        let mut store = mem();
        let _ = crate::memory::update(&mut store, crate::memory::Message::ForgetAll);
        let _ = crate::memory::update(&mut store, crate::memory::Message::ForgetAll);

        let mut s = State { draft: "remember I fly a Quinjet".into(), ..State::new() };
        let _ = update(&mut s, &client(), &mut store, Message::Send);
        let typed = r#"[{"name": "remember", "arguments": {"text": "Flies a Quinjet."}}]"#;
        let _ = update(&mut s, &client(), &mut store, Message::Chunk(ChatChunk::Delta(typed.into())));
        let _ = update(&mut s, &client(), &mut store, Message::Chunk(ChatChunk::Done));
        assert_eq!(store.items.len(), 1, "the typed-out call ran");
        assert_eq!(store.items[0].text, "Flies a Quinjet.");
        assert!(
            s.messages.last().unwrap().content.is_empty(),
            "the raw JSON is replaced by the call, not left in the transcript"
        );

        // Anything that is not exactly a call to a tool that exists stays prose.
        assert!(salvage_calls("Sure, I'll remember that.").is_none());
        assert!(salvage_calls(r#"{"name": "rm_rf", "arguments": {}}"#).is_none());
        assert!(salvage_calls(r#"Use {"name": "forget", "arguments": {"id": 1}} to delete."#)
            .is_none());
        // Fenced, and with arguments as a string, are both still calls.
        let fenced = salvage_calls("```json\n{\"name\": \"forget\", \"arguments\": \"{\\\"id\\\": 2}\"}\n```")
            .expect("a fenced call");
        assert_eq!(fenced[0].function.name, "forget");
        assert_eq!(fenced[0].function.arguments, r#"{"id": 2}"#);
    }

    #[test]
    fn a_tool_only_reply_still_gets_an_assistant_turn() {
        use agent_platform_client::sse::ToolCallDelta;
        let mut s = State { draft: "list files".into(), voice: false, ..State::new() };
        let _ = update(&mut s, &client(), &mut mem(), Message::Send);
        // No text at all — straight to the tool.
        let d = ToolCallDelta {
            index: 0,
            id: Some("c1".into()),
            name: Some("run_command".into()),
            arguments: "{\"command\": \"ls\"}".into(),
        };
        let _ = update(&mut s, &client(), &mut mem(), Message::Chunk(ChatChunk::ToolCall(d)));
        let _ = update(&mut s, &client(), &mut mem(), Message::Chunk(ChatChunk::Done));
        let turn = s.messages.last().unwrap();
        assert_eq!(turn.role, "assistant");
        assert_eq!(turn.content, "");
        assert!(turn.tool_calls.is_some());
    }

    #[tokio::test]
    async fn run_command_captures_output_and_reports_failure() {
        let out = run_command("echo hello".into(), None, TOOL_TIMEOUT).await;
        assert!(out.contains("hello"), "got: {out:?}");
        let out = run_command("exit 3".into(), None, TOOL_TIMEOUT).await;
        assert!(out.contains("exit code 3"), "got: {out:?}");
    }

    /// The Coder screen's half of the shared runner: a command has to land in
    /// the workspace, not wherever the app happened to be launched from.
    #[tokio::test]
    async fn run_command_runs_where_it_was_told_to() {
        let dir = std::env::temp_dir().join("coder-cwd-check");
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        let pwd = if cfg!(windows) { "$PWD.Path" } else { "pwd" };
        let out = run_command(pwd.into(), Some(dir.clone()), TOOL_TIMEOUT).await;
        let leaf = dir.file_name().unwrap().to_string_lossy().to_lowercase();
        assert!(out.to_lowercase().contains(&leaf), "got: {out:?}");
    }

    #[test]
    fn hush_drops_queued_speech() {
        let mut s = State::new();
        reply(&mut s, "One thing and another thing. Then a second sentence here.");
        s.audio_queue.push_back(vec![0u8; 4]);
        s.hush();
        assert!(s.speech_queue.is_empty());
        assert!(s.audio_queue.is_empty());
        assert!(s.speech_buf.is_empty());
    }

    /// Feed a transcript in as if the mic had just closed on it.
    fn heard(s: &mut State, text: &str) {
        s.transcribing = true;
        let _ = update(s, &client(), &mut mem(), Message::Heard(Ok((text.into(), None))));
    }

    #[test]
    fn a_long_spoken_reply_does_not_eat_the_follow_up_window() {
        // Generation finished 30 s ago — but E.V. was reading the answer out
        // loud for most of it and only just stopped, and the gate was shut the
        // whole time. The reply to it is still a reply.
        let mut s = State {
            phase: 100.0,
            last_reply: 70.0,
            spoke_at: 99.5,
            voice: false,
            ..State::new()
        };
        assert!(s.follow_up_open());
        heard(&mut s, "and the second one?");
        assert_eq!(s.messages.last().unwrap().content, "and the second one?");
        assert!(s.sending, "it should have sent itself, not parked in the composer");
    }

    #[test]
    fn everything_said_while_armed_sends_however_long_you_took() {
        // Pressed the mic button, then thought about it for a minute.
        let mut s = State { phase: 300.0, last_reply: 0.0, armed: true, ..State::new() };
        assert!(s.follow_up_open());
        heard(&mut s, "run diagnostics");
        assert_eq!(s.messages.last().unwrap().content, "run diagnostics");
        assert!(s.sending);

        // ...and so does the second one, long after and unaddressed. The button
        // is still on, so it is still being talked to.
        s.sending = false;
        s.phase = 400.0;
        heard(&mut s, "and the second one?");
        assert_eq!(s.messages.last().unwrap().content, "and the second one?");
        assert!(s.sending, "armed means armed until you press it again");

        // Pressing it again ends that: unaddressed speech parks in the composer.
        s.armed = false;
        s.sending = false;
        s.phase = 500.0;
        heard(&mut s, "so anyway I told him no");
        assert_eq!(s.messages.len(), 2, "no new turn");
        assert_eq!(s.draft, "so anyway I told him no");
    }

    #[test]
    fn whispers_stock_captions_for_silence_never_reach_the_model() {
        // A cough with the mic armed: whisper captions it "Thank you." and the
        // armed path would send that as a turn.
        let mut s = State { armed: true, phase: 300.0, voice: false, ..State::new() };
        heard(&mut s, "Thank you.");
        heard(&mut s, " you ");
        heard(&mut s, "Thanks for watching!");
        assert!(s.messages.is_empty(), "ghosts must not open a turn");
        assert!(s.draft.is_empty(), "and must not land in the composer either");

        // Real speech containing the same words is not a ghost — only the whole
        // utterance matching one is.
        heard(&mut s, "thank you for the summary, now do the other one");
        assert_eq!(s.messages.len(), 1);
    }

    #[test]
    fn parked_speech_appends_rather_than_eating_the_composer() {
        // Two unaddressed utterances outside the window, and something already
        // typed. Assignment would leave only the last one.
        let mut s = State { phase: 300.0, draft: "check".into(), ..State::new() };
        heard(&mut s, "the build logs");
        heard(&mut s, "from last night");
        assert!(s.messages.is_empty());
        assert_eq!(s.draft, "check the build logs from last night");
    }

    /// Bands shaped like a voice: energy in the speech window, nothing outside.
    fn speech_bands() -> [f32; BANDS] {
        let mut b = [0.02_f32; BANDS];
        for (i, v) in b.iter_mut().enumerate() {
            if (150.0..4000.0).contains(&crate::stt::band_freq(i, BANDS)) {
                *v = 0.5;
            }
        }
        b
    }

    #[test]
    fn barge_in_fires_on_a_voice_over_the_top_but_not_on_the_speakers() {
        let (bleed, gate) = (0.05, 0.02);
        // E.V.'s own voice at the mic, at exactly the level the bleed learned.
        assert!(!over_playback(bleed, bleed, gate, &speech_bands()), "that is E.V. itself");
        // The room's own noise while it talks, well under the bleed.
        assert!(!over_playback(0.01, bleed, gate, &speech_bands()));
        // Someone talking over it, comfortably above.
        assert!(over_playback(bleed * BARGE_SNR * 1.5, bleed, gate, &speech_bands()));
        // Just as loud, but shaped like a door rather than a voice.
        assert!(!over_playback(bleed * BARGE_SNR * 1.5, bleed, gate, &[0.5; BANDS]));
        // Headphones: bleed near zero, so the room floor is what has to be
        // cleared instead — otherwise any hiss would stop every reply.
        assert!(!over_playback(0.01, 0.0, gate, &speech_bands()), "hiss must not interrupt");
        assert!(over_playback(0.3, 0.0, gate, &speech_bands()));
    }

    #[test]
    fn the_hourly_phase_wrap_does_not_jam_either_timer() {
        // `phase` wrapped 0.5 s ago; the events it is compared against are just
        // before the wrap. Plain subtraction would read as -3599 and leave the
        // follow-up window stuck open and the mic gate stuck shut.
        assert_eq!(since(0.5, 3599.0), 1.5);
        assert!(since(0.5, 3599.0) < FOLLOW_UP, "still inside the window, correctly");

        let s = State { phase: 100.0, last_reply: 3590.0, spoke_at: 3590.0, ..State::new() };
        assert!(!s.follow_up_open(), "110 s after E.V. spoke is not a follow-up");
    }

    #[test]
    fn sentences_split_on_terminators_but_not_inside_words() {
        let mut buf = "Shields are at 82.5 percent, boss. Next up.".to_string();
        // The decimal point does not close a sentence — no whitespace follows it.
        assert_eq!(
            take_sentence(&mut buf, false).as_deref(),
            Some("Shields are at 82.5 percent, boss.")
        );
        assert_eq!(buf, "Next up.");
        // Too short to be worth a clip on its own; it waits for more text.
        assert_eq!(take_sentence(&mut buf, false), None);
        assert_eq!(buf, "Next up.");
    }

    #[test]
    fn first_chunk_breaks_early_so_the_voice_starts_sooner() {
        // Same text, and the opening chunk leaves at the comma rather than
        // waiting out the rest of the sentence.
        let mut buf = "Shields are at 82.5 percent, boss. Next up.".to_string();
        assert_eq!(
            take_sentence(&mut buf, true).as_deref(),
            Some("Shields are at 82.5 percent,")
        );
        // Only the opening chunk is cut short; behind it the ordinary minimum
        // applies again, and this remainder is under it.
        assert_eq!(take_sentence(&mut buf, false), None);
        assert_eq!(buf, "boss. Next up.");
    }

    #[test]
    fn rate_tracks_the_stream_around_the_setting() {
        let mut s = State { voice_rate: 15, streaming: true, ..State::new() };
        // Nothing queued while the model is still writing: read slower rather
        // than run out of words mid-answer.
        assert_eq!(s.speech_rate(), 15 - RATE_SPAN / 2);
        // The stream ended, so an empty queue is just the last sentence.
        s.streaming = false;
        assert_eq!(s.speech_rate(), 15);
        // Text piling up behind the voice: close the gap.
        s.speech_queue.extend(["a".to_string(), "b".to_string(), "c".to_string()]);
        assert_eq!(s.speech_rate(), 15 + RATE_SPAN);
    }
}

