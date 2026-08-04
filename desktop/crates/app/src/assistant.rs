//! E.V. — Extra-Vehicular Assistant, the onboard suit AI. Same stateless chat
//! endpoint as `chat.rs`, plus a persona system prompt, an animated HUD, spoken
//! replies, and the long-term memory in `memory.rs`.
//!
//! Voice: Microsoft Edge neural TTS (AriaNeural over the free websocket
//! endpoint — no key, needs internet) played through rodio; falls back to the
//! platform's native engine (SAPI/WinRT, AVSpeech, speech-dispatcher) offline.

use agent_platform_client::sse::{self, ChatChunk};
use agent_platform_client::types::{ChatCompletionBody, ChatMessage, ToolCall};
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
instead of guessing. Never run destructive commands unless explicitly asked.";

/// Rounds of tool calls allowed per user turn before the model is forced to
/// answer in text (tools withheld from the request past the cap).
const MAX_TOOL_ROUNDS: u8 = 5;
/// Hard deadline on one command; past it the process is killed.
const TOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// Output kept per command — both for the model and the transcript.
const MAX_TOOL_OUTPUT: usize = 8_000;

/// The one tool E.V. carries: a terminal.
fn tools_spec() -> serde_json::Value {
    serde_json::json!([{
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
    }])
}

/// The `command` a call asked to run, or its raw arguments while they are
/// still streaming/malformed — shown in the transcript either way.
fn command_of(call: &ToolCall) -> String {
    serde_json::from_str::<serde_json::Value>(&call.function.arguments)
        .ok()
        .and_then(|v| v.get("command").and_then(|c| c.as_str()).map(str::to_string))
        .unwrap_or_else(|| call.function.arguments.clone())
}

#[derive(Debug, Clone)]
pub struct ToolOutcome {
    /// `tool_call_id` the result answers.
    pub id: String,
    pub output: String,
}

/// Execute one shell command, capped by `TOOL_TIMEOUT` and `MAX_TOOL_OUTPUT`.
/// Errors come back as text — the model reads them and corrects itself.
async fn run_command(command: String) -> String {
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
    cmd.stdin(std::process::Stdio::null()).kill_on_drop(true);
    let out = match tokio::time::timeout(TOOL_TIMEOUT, cmd.output()).await {
        Err(_) => return format!("(timed out after {}s)", TOOL_TIMEOUT.as_secs()),
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

/// Run every call of one round, in order. Sequential on purpose: the calls in
/// a round often depend on the same working state (cd, files just written).
async fn run_tools(calls: Vec<ToolCall>) -> Vec<ToolOutcome> {
    let mut results = Vec::with_capacity(calls.len());
    for call in calls {
        let output = if call.function.name == "run_command" {
            match serde_json::from_str::<serde_json::Value>(&call.function.arguments)
                .ok()
                .and_then(|v| v.get("command").and_then(|c| c.as_str()).map(str::to_string))
            {
                Some(cmd) => run_command(cmd).await,
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

/// HUD animation heartbeat. 60 fps: the spectrum ring and the core respond to
/// speech transients, and at 20 fps that reads as stepping, not breathing.
pub const TICK: std::time::Duration = std::time::Duration::from_millis(16);
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
/// ponytail: half-duplex. True barge-in needs acoustic echo cancellation.
const ECHO_TAIL: f32 = 0.35;
/// After E.V. replies (or right after you arm it), you can just talk. Outside
/// this window an utterance has to name E.V. to be sent on its own.
const FOLLOW_UP: f32 = 12.0;
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
    pub voice: bool,
    /// Long-term recall, refreshed by the app before every message. `None` when
    /// memory is off or empty — see `memory::Store::system_block`.
    pub memory: Option<String>,
    /// Seconds of animation time, monotonic while the screen is open.
    pub phase: f32,
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
    /// Armed, and nothing has been said yet. Pressing the mic button is intent
    /// enough on its own — the first utterance after it goes through however
    /// long you took to start talking.
    awaiting_first: bool,
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
}

impl State {
    pub fn new() -> Self {
        Self { voice: true, ..Self::default() }
    }

    /// Hands-free listening is on: the mic is open and E.V. decides when you
    /// are talking to it.
    pub fn armed(&self) -> bool {
        self.recorder.is_some()
    }

    /// Whatever was just said counts as aimed at E.V. even though nobody named
    /// it: either it is the first thing since you armed the mic, or E.V. only
    /// just stopped talking and this is the other half of the exchange.
    ///
    /// Measured from `spoke_at` — the last frame E.V.'s voice was playing — and
    /// not from when its text finished. The gate is held shut while E.V. speaks,
    /// so a window timed from the end of generation is already partly spent by
    /// the time you are physically able to answer, and a long spoken reply eats
    /// all of it.
    fn follow_up_open(&self) -> bool {
        self.awaiting_first
            || since(self.phase, self.last_reply.max(self.spoke_at)) < FOLLOW_UP
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
    fn request(&self, client: &Client) -> Task<Message> {
        let mut messages = vec![ChatMessage::text("system", PERSONA)];
        // Recall after the persona: who E.V. is comes first, then who it
        // is talking to.
        if let Some(recall) = &self.memory {
            messages.push(ChatMessage::text("system", recall.clone()));
        }
        messages.extend(self.messages.iter().cloned());
        let body = ChatCompletionBody {
            messages,
            model: None,
            provider: None,
            temperature: None,
            max_tokens: None,
            // Past the round cap the tools disappear from the request, which
            // forces a text answer instead of an endless loop.
            tools: (self.tool_rounds < MAX_TOOL_ROUNDS).then(tools_spec),
            stream: Some(true),
        };
        Task::batch([
            iced::widget::operation::snap_to_end(transcript_id()),
            Task::run(sse::chat_stream(client.clone(), body), Message::Chunk),
        ])
    }

    /// Append a streamed delta to the assistant turn in flight, opening one if
    /// this is the first token, and hand any newly closed sentence to the voice.
    fn push_delta(&mut self, text: &str) {
        if !self.streaming {
            self.push_turn("assistant", String::new());
            self.streaming = true;
        }
        let last = self.messages.len() - 1;
        self.messages[last].content.push_str(text);
        self.md[last] = iced::widget::markdown::parse(&self.messages[last].content).collect();

        if self.voice {
            self.speech_buf.push_str(text);
            while let Some(sentence) = take_sentence(&mut self.speech_buf) {
                self.enqueue_speech(&sentence);
            }
        }
    }

    /// Queue one chunk of reply text for the voice, markdown stripped.
    fn enqueue_speech(&mut self, raw: &str) {
        let spoken = speech_text(raw);
        if !spoken.trim().is_empty() {
            self.speech_queue.push_back(spoken);
        }
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
        let client = client.clone();
        Task::perform(
            async move {
                // The server's own voice first (`SPEECH_API_BASE`: a hosted
                // provider, or a local Piper/Kokoro). It answers 501 when no
                // backend is configured, which is the common case — so this is
                // a loopback round-trip, not a real cost, and turning the
                // backend on takes effect without restarting the app.
                if let Ok(bytes) = client.speech(&text).await {
                    return Ok(bytes);
                }
                tokio::task::spawn_blocking(move || synthesize(&text))
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

    pub fn mode(&self) -> Mode {
        let speaking = self.sink.as_ref().is_some_and(|s| !s.empty())
            || self.tts.as_ref().is_some_and(|t| t.is_speaking().unwrap_or(false));
        if self.capture.is_some() {
            Mode::Listening
        } else if self.sending || self.synthesizing || self.transcribing {
            Mode::Thinking
        } else if speaking {
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
        if let Some(t) = self.tts.as_mut() {
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

/// Split off the first complete sentence, leaving the remainder in `buf`.
///
/// Returns `None` while the buffer holds no closed sentence of usable length —
/// the caller keeps accumulating deltas and flushes the tail at end of stream.
///
/// ponytail: naive terminator scan. Splits "3.5" and "Dr. Chen" mid-sentence,
/// which costs a small pause in the wrong place, not a wrong word. Swap in a
/// real segmenter if the voice starts sounding choppy on numeric answers.
fn take_sentence(buf: &mut String) -> Option<String> {
    let bytes = buf.as_bytes();
    for (i, c) in buf.char_indices() {
        if i + 1 < MIN_SPEECH_CHUNK {
            continue;
        }
        let terminator = matches!(c, '.' | '!' | '?' | '\n' | ';' | ':');
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

/// Neural synthesis over Edge's websocket. Blocking, so callers wrap it in
/// `spawn_blocking`; MP3 bytes come back for rodio to decode.
fn synthesize(text: &str) -> Result<Vec<u8>, String> {
    use msedge_tts::tts::{client::connect, SpeechConfig};
    let mut client = connect().map_err(|e| e.to_string())?;
    let config = SpeechConfig {
        voice_name: EDGE_VOICE.into(),
        audio_format: "audio-24khz-96kbitrate-mono-mp3".into(),
        pitch: 0,
        rate: 6, // slightly brisk reads as conversational, not narration
        volume: 0,
    };
    let audio = client.synthesize(text, &config).map_err(|e| e.to_string())?;
    if audio.audio_bytes.is_empty() {
        return Err("empty audio".into());
    }
    Ok(audio.audio_bytes)
}

#[derive(Debug, Clone)]
pub enum Message {
    DraftChanged(String),
    Send,
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
    /// Animation heartbeat; only runs while the Assistant screen is visible.
    Tick,
}

pub fn update(state: &mut State, client: &Client, message: Message) -> Task<Message> {
    match message {
        Message::DraftChanged(v) => {
            state.draft = v;
            Task::none()
        }
        Message::Send => {
            let prompt = state.draft.trim().to_string();
            if prompt.is_empty() || state.sending {
                return Task::none();
            }
            state.hush();
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
            // Whatever was said this round gets spoken either way; the tail
            // after the last terminator is a sentence too.
            if state.voice && !state.speech_buf.trim().is_empty() {
                let tail = std::mem::take(&mut state.speech_buf);
                state.enqueue_speech(&tail);
            }
            state.speech_buf.clear();
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
                return Task::batch([
                    iced::widget::operation::snap_to_end(transcript_id()),
                    state.next_synthesis(client),
                    Task::perform(run_tools(calls), Message::ToolResults),
                ]);
            }
            state.sending = false;
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
        Message::Listen => {
            // Toggle hands-free listening. Off drops the stream, which is the
            // only honest way to say "the mic is not on".
            if state.recorder.take().is_some() {
                state.capture = None;
                state.onset = 0;
                return Task::none();
            }
            state.hush();
            match crate::stt::Recorder::start() {
                Ok(rec) => {
                    state.recorder = Some(rec);
                    state.capture = None;
                    state.onset = 0;
                    state.floor = ABS_FLOOR;
                    // You just pressed the button, so the next thing you say is
                    // obviously for E.V. — no need to name it, and no deadline
                    // to say it by.
                    state.last_reply = state.phase;
                    state.awaiting_first = true;
                }
                Err(e) => state.error = Some(e),
            }
            Task::none()
        }
        Message::Heard(result) => {
            state.transcribing = false;
            match result {
                Ok((text, print)) if !text.trim().is_empty() => {
                    // Whose voice was that? Until enrolled, everything that got
                    // through the gate counts as yours; after that, a voice this
                    // far from the enrolled one is somebody else in the room.
                    state.voice_sim = match (state.voice_print, print) {
                        (Some(known), Some(heard)) => {
                            Some(crate::stt::print_similarity(&known, &heard))
                        }
                        _ => None,
                    };
                    // Nothing is rejected while still enrolling: the print those
                    // first utterances build is what the test compares against.
                    let mine = !state.voice_enrolled()
                        || state.voice_sim.is_none_or(|sim| sim >= VOICE_MATCH);
                    if mine {
                        state.learn_voice(print);
                    }
                    // Addressed by name, or spoken inside the follow-up window
                    // after E.V.'s last reply → it was meant for E.V.
                    let follow_up = state.follow_up_open() && mine;
                    state.awaiting_first = false;
                    // Another voice never auto-sends, however it phrased itself:
                    // it lands in the composer for you to send or discard.
                    let addressed = if mine { addressed(&text) } else { None };
                    match (addressed, follow_up) {
                        (Some(body), _) if !body.trim().is_empty() => {
                            state.draft = body.to_string();
                            update(state, client, Message::Send)
                        }
                        // "E.V.?" on its own is still someone calling it.
                        (Some(_), _) => {
                            state.draft = "?".to_string();
                            update(state, client, Message::Send)
                        }
                        (None, true) => {
                            state.draft = text;
                            update(state, client, Message::Send)
                        }
                        // Heard, but nothing said it was for E.V. — park it in
                        // the composer rather than answering the room.
                        (None, false) => {
                            state.draft = text;
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
                state.hush();
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
        Message::Tick => {
            state.phase = (state.phase + DT) % 3600.0;
            state.elapsed += DT;
            state.boot = (state.boot + DT * 0.9).min(1.0);
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
            state.energy = if loud > state.energy { loud } else { state.energy * 0.92 };
            state.beat = (state.beat * 0.88).max((jump * 5.0).min(1.0));
            state.wave.push(state.energy);
            if state.wave.len() > WAVE {
                state.wave.drain(..state.wave.len() - WAVE);
            }

            // Mode changes crossfade rather than cut: colour, radii and labels
            // all ease across `mode_t`.
            let mode = state.mode();
            if mode != state.mode_now {
                state.mode_prev = state.mode_now;
                state.mode_now = mode;
                state.mode_t = 0.0;
                state.elapsed = 0.0;
            }
            state.mode_t = (state.mode_t + DT * 3.5).min(1.0);
            if mode == Mode::Speaking {
                state.spoke_at = state.phase;
            }

            let Some(rec) = &state.recorder else {
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

            // The floor learns the room whenever nothing is being captured:
            // fast down to a quiet moment, slow up so a sentence never becomes
            // "normal" (rise constant ~30 s, fall ~0.3 s).
            if state.capture.is_none() {
                let rate = if mic < state.floor { 0.05 } else { 0.0006 };
                state.floor += (mic - state.floor) * rate;
            }
            let gate = (state.floor * OPEN_SNR).max(ABS_FLOOR);
            let speechy = mic > gate && voice_like(&state.bands);

            // E.V.'s own voice comes back through the mic, so the gate stays
            // shut while it talks and for a beat after. Same for the seconds
            // when a reply is already on its way.
            let blocked = state.sending
                || state.transcribing
                || state.synthesizing
                || mode == Mode::Speaking
                || since(state.phase, state.spoke_at) < ECHO_TAIL;

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

    #[test]
    fn sending_appends_turn_and_thinks() {
        let mut s = State { draft: " status? ".into(), ..State::new() };
        let _ = update(&mut s, &client(), Message::Send);
        assert_eq!(s.messages.len(), 1);
        assert_eq!(s.messages[0].content, "status?");
        assert!(s.sending);
        assert_eq!(s.mode(), Mode::Thinking);
    }

    #[test]
    fn blank_and_in_flight_sends_ignored() {
        let mut s = State { draft: "  ".into(), ..State::new() };
        let _ = update(&mut s, &client(), Message::Send);
        assert!(s.messages.is_empty());

        let mut s = State { draft: "hi".into(), sending: true, ..State::new() };
        let _ = update(&mut s, &client(), Message::Send);
        assert!(s.messages.is_empty());
    }

    /// A whole reply arriving as one delta plus the end-of-stream marker.
    fn reply(s: &mut State, text: &str) {
        let _ = update(s, &client(), Message::Chunk(ChatChunk::Delta(text.into())));
        let _ = update(s, &client(), Message::Chunk(ChatChunk::Done));
    }

    #[test]
    fn failed_turn_stays_with_error() {
        let mut s = State { draft: "hi".into(), ..State::new() };
        let _ = update(&mut s, &client(), Message::Send);
        let _ = update(&mut s, &client(), Message::Chunk(ChatChunk::Failed("boom".into())));
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

        let _ = update(&mut s, &client(), Message::ToggleVoice);
        assert!(s.voice);
    }

    #[test]
    fn reasoning_is_shown_but_never_spoken() {
        let mut s = State::new(); // voice on
        let _ = update(
            &mut s,
            &client(),
            Message::Chunk(ChatChunk::Reasoning("User asks for status; check the basics.".into())),
        );
        // Reasoning alone opened the turn and nothing went to the synthesizer.
        assert_eq!(s.messages.len(), 1);
        assert!(s.reasoning_live(0));
        assert!(!s.synthesizing);
        let _ = update(&mut s, &client(), Message::Chunk(ChatChunk::Delta("Systems nominal.".into())));
        let _ = update(&mut s, &client(), Message::Chunk(ChatChunk::Done));
        assert_eq!(s.reasoning[0], "User asks for status; check the basics.");
        assert_eq!(s.messages[0].content, "Systems nominal.");
        // What reached the voice is the reply, not the deliberation.
        assert_eq!(s.speaking.as_deref(), Some("Systems nominal."));
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
        let _ = update(&mut s, &client(), Message::Heard(Ok(("run diagnostics".into(), None))));
        assert!(!s.transcribing);
        assert_eq!(s.messages.len(), 1);
        assert_eq!(s.messages[0].content, "run diagnostics");
        assert!(s.sending);

        let mut s = State { transcribing: true, ..State::new() };
        let _ = update(&mut s, &client(), Message::Heard(Ok(("  ".into(), None))));
        assert!(s.messages.is_empty());
        assert!(!s.sending);

        let mut s = State { transcribing: true, ..State::new() };
        let _ = update(&mut s, &client(), Message::Heard(Err("mic missing".into())));
        assert_eq!(s.error.as_deref(), Some("mic missing"));
    }

    #[test]
    fn only_speech_aimed_at_ev_is_sent_on_its_own() {
        // Outside the follow-up window, unaddressed speech is parked in the
        // composer — heard, but not answered.
        let mut s = State { transcribing: true, phase: 60.0, ..State::new() };
        let _ = update(&mut s, &client(), Message::Heard(Ok(("so anyway I told him no".into(), None))));
        assert!(s.messages.is_empty(), "room chatter must not reach the model");
        assert_eq!(s.draft, "so anyway I told him no");
        assert!(!s.sending);

        // Naming E.V. sends it, and the name itself is not part of the question.
        let mut s = State { transcribing: true, phase: 60.0, ..State::new() };
        let _ = update(&mut s, &client(), Message::Heard(Ok(("Hey Eve, run diagnostics".into(), None))));
        assert_eq!(s.messages.len(), 1);
        assert_eq!(s.messages[0].content, "run diagnostics");

        // A reply opens the window: the next thing said needs no name.
        let mut s = State { phase: 60.0, voice: false, ..State::new() };
        reply(&mut s, "Done.");
        s.transcribing = true;
        let _ = update(&mut s, &client(), Message::Heard(Ok(("and the second one?".into(), None))));
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
            Message::Heard(Ok(("E.V., wipe the database".into(), Some(theirs)))),
        );
        assert!(s.messages.is_empty(), "a stranger must not reach the model");
        assert_eq!(s.draft, "E.V., wipe the database");

        // The enrolled voice, same words, goes straight out.
        let mut s = enrolled(mine);
        let _ = update(
            &mut s,
            &client(),
            Message::Heard(Ok(("E.V., run diagnostics".into(), Some(mine)))),
        );
        assert_eq!(s.messages.len(), 1);
        assert_eq!(s.messages[0].content, "run diagnostics");

        // Before enrollment finishes, nothing is rejected — that is what the
        // first few utterances are for.
        let mut s = State { transcribing: true, ..State::new() };
        let _ = update(&mut s, &client(), Message::Heard(Ok(("hello there".into(), Some(theirs)))));
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
        let bytes = synthesize("Systems online. Good to see you.").unwrap();
        assert!(bytes.len() > 1000, "suspiciously small audio: {} bytes", bytes.len());
    }

    #[test]
    #[ignore = "network + model download: full TTS→STT round trip"]
    fn live_voice_round_trip() {
        use rodio::Source;
        let bytes = synthesize("Hello Peter, systems are online.").unwrap();
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
        let _ = update(&mut s, &client(), Message::Send);
        // Mid-stream: one closed sentence, one still arriving.
        let _ = update(
            &mut s,
            &client(),
            Message::Chunk(ChatChunk::Delta("Systems are nominal, boss. And the".into())),
        );
        assert!(s.synthesizing, "sentence one goes out while the reply is still streaming");
        assert_eq!(s.speaking.as_deref(), Some("Systems are nominal, boss."));
        assert_eq!(s.speech_buf, "And the", "the open clause waits for its terminator");
        assert!(s.sending, "the turn is not over yet");

        // The tail flushes at end of stream even without a terminator.
        let _ = update(&mut s, &client(), Message::Chunk(ChatChunk::Delta(" rest".into())));
        let _ = update(&mut s, &client(), Message::Chunk(ChatChunk::Done));
        assert_eq!(s.speech_queue.front().map(String::as_str), Some("And the rest"));
        assert_eq!(s.messages.last().unwrap().content, "Systems are nominal, boss. And the rest");
        assert!(!s.sending);
    }

    #[test]
    fn a_tool_round_runs_the_terminal_and_keeps_the_turn_open() {
        use agent_platform_client::sse::ToolCallDelta;
        let mut s = State { draft: "how much disk is free?".into(), voice: false, ..State::new() };
        let _ = update(&mut s, &client(), Message::Send);

        // The model narrates, then asks for the terminal in streamed fragments.
        let _ = update(&mut s, &client(), Message::Chunk(ChatChunk::Delta("Checking.".into())));
        for d in [
            ToolCallDelta {
                index: 0,
                id: Some("call_1".into()),
                name: Some("run_command".into()),
                arguments: "{\"command\": \"Get-".into(),
            },
            ToolCallDelta { index: 0, arguments: "PSDrive C\"}".into(), ..Default::default() },
        ] {
            let _ = update(&mut s, &client(), Message::Chunk(ChatChunk::ToolCall(d)));
        }
        let _ = update(&mut s, &client(), Message::Chunk(ChatChunk::Done));

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

    #[test]
    fn a_tool_only_reply_still_gets_an_assistant_turn() {
        use agent_platform_client::sse::ToolCallDelta;
        let mut s = State { draft: "list files".into(), voice: false, ..State::new() };
        let _ = update(&mut s, &client(), Message::Send);
        // No text at all — straight to the tool.
        let d = ToolCallDelta {
            index: 0,
            id: Some("c1".into()),
            name: Some("run_command".into()),
            arguments: "{\"command\": \"ls\"}".into(),
        };
        let _ = update(&mut s, &client(), Message::Chunk(ChatChunk::ToolCall(d)));
        let _ = update(&mut s, &client(), Message::Chunk(ChatChunk::Done));
        let turn = s.messages.last().unwrap();
        assert_eq!(turn.role, "assistant");
        assert_eq!(turn.content, "");
        assert!(turn.tool_calls.is_some());
    }

    #[tokio::test]
    async fn run_command_captures_output_and_reports_failure() {
        let out = run_command(if cfg!(windows) { "echo hello" } else { "echo hello" }.into()).await;
        assert!(out.contains("hello"), "got: {out:?}");
        let out = run_command("exit 3".into()).await;
        assert!(out.contains("exit code 3"), "got: {out:?}");
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
        let _ = update(s, &client(), Message::Heard(Ok((text.into(), None))));
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
    fn the_first_thing_said_after_arming_sends_however_long_you_took() {
        // Pressed the mic button, then thought about it for a minute.
        let mut s =
            State { phase: 300.0, last_reply: 0.0, awaiting_first: true, ..State::new() };
        assert!(s.follow_up_open());
        heard(&mut s, "run diagnostics");
        assert_eq!(s.messages.last().unwrap().content, "run diagnostics");
        assert!(s.sending);

        // ...and the *second* unaddressed thing, long after, is back to being
        // room noise: it parks rather than answering whoever is talking.
        s.sending = false;
        s.phase = 400.0;
        heard(&mut s, "so anyway I told him no");
        assert_eq!(s.messages.len(), 1, "no new turn");
        assert_eq!(s.draft, "so anyway I told him no");
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
        assert_eq!(take_sentence(&mut buf).as_deref(), Some("Shields are at 82.5 percent, boss."));
        assert_eq!(buf, "Next up.");
        // Too short to be worth a clip on its own; it waits for more text.
        assert_eq!(take_sentence(&mut buf), None);
        assert_eq!(buf, "Next up.");
    }
}
