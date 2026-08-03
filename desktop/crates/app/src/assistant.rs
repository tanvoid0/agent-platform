//! E.V. — the onboard suit AI. Same stateless chat endpoint as `chat.rs`,
//! plus a persona system prompt, an animated HUD, and spoken replies.
//!
//! Voice: Microsoft Edge neural TTS (AriaNeural over the free websocket
//! endpoint — no key, needs internet) played through rodio; falls back to the
//! platform's native engine (SAPI/WinRT, AVSpeech, speech-dispatcher) offline.

use agent_platform_client::sse::{self, ChatChunk};
use agent_platform_client::types::{ChatCompletionBody, ChatMessage};
use agent_platform_client::Client;
use iced::Task;
use std::collections::VecDeque;
use std::io::Cursor;
use tts::Tts;

const PERSONA: &str = "You are E.V., the onboard suit AI of this Agent Platform. \
Style: a superhero suit's heads-up-display assistant — calm, quick-witted, \
protective, with a dry quip now and then. You monitor systems, analyze the \
situation, and give tactical, actionable answers. Your replies may be spoken \
aloud, so keep them short, conversational and free of markdown, bullet lists \
or code fences unless the user asks for code. Answer directly first; skip filler.";

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

/// Is this frame shaped like a voice? Fans and traffic sit under 200 Hz, hiss
/// and keyboard clatter spread flat across everything; speech puts most of its
/// energy in the middle. The bands are already computed for the HUD, so this
/// costs a couple of dozen adds.
fn voice_like(bands: &[f32; BANDS]) -> bool {
    let total: f32 = bands.iter().sum();
    if total < 0.05 {
        return false;
    }
    let speech: f32 = bands
        .iter()
        .enumerate()
        .filter(|(i, _)| (200.0..3600.0).contains(&crate::stt::band_freq(*i, BANDS)))
        .map(|(_, v)| *v)
        .sum();
    speech / total > 0.55
}

/// Whitelist of how whisper spells "E.V." when someone says it out loud.
const NAMES: [&str; 9] =
    ["ev", "eev", "eve", "evie", "evee", "eevee", "heavy", "evy", "ivy"];
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
    pub draft: String,
    pub sending: bool,
    pub error: Option<String>,
    pub voice: bool,
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
    /// `phase` when E.V. last spoke to you, or when you armed it: inside the
    /// follow-up window you don't have to say its name.
    last_reply: f32,
    /// `phase` while E.V.'s own voice is playing — the mic hears the speakers,
    /// so capture stays shut until a beat after it stops.
    spoke_at: f32,
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
    /// An assistant turn is open and collecting deltas.
    streaming: bool,
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

    fn push_turn(&mut self, role: &str, content: String) {
        self.md.push(iced::widget::markdown::parse(&content).collect());
        self.messages.push(ChatMessage { role: role.into(), content });
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
    fn next_synthesis(&mut self) -> Task<Message> {
        if self.synthesizing {
            return Task::none();
        }
        let Some(text) = self.speech_queue.pop_front() else {
            return Task::none();
        };
        self.synthesizing = true;
        self.speaking = Some(text.clone());
        Task::perform(
            async move {
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
    Heard(Result<String, String>),
    OpenMicSettings,
    LinkClicked(String),
    /// One chunk of the streamed reply.
    Chunk(ChatChunk),
    Synthesized(Result<Vec<u8>, String>),
    ToggleVoice,
    Clear,
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

            let mut messages =
                vec![ChatMessage { role: "system".into(), content: PERSONA.into() }];
            messages.extend(state.messages.iter().cloned());
            let body = ChatCompletionBody {
                messages,
                model: None,
                temperature: None,
                max_tokens: None,
                stream: Some(true),
            };
            Task::batch([
                iced::widget::operation::snap_to_end(transcript_id()),
                Task::run(sse::chat_stream(client.clone(), body), Message::Chunk),
            ])
        }
        Message::Chunk(ChatChunk::Delta(text)) => {
            state.push_delta(&text);
            // The first sentence goes to the synthesizer while the rest of the
            // reply is still being generated — that is the whole latency win.
            Task::batch([
                iced::widget::operation::snap_to_end(transcript_id()),
                state.next_synthesis(),
            ])
        }
        Message::Chunk(ChatChunk::Done) => {
            state.sending = false;
            state.streaming = false;
            // Opens the follow-up window: the answer invites a follow-up, and
            // having to say "E.V." again mid-conversation is not conversation.
            state.last_reply = state.phase;
            // The tail after the last terminator is a sentence too.
            if state.voice && !state.speech_buf.trim().is_empty() {
                let tail = std::mem::take(&mut state.speech_buf);
                state.enqueue_speech(&tail);
            }
            state.speech_buf.clear();
            Task::batch([
                iced::widget::operation::snap_to_end(transcript_id()),
                state.next_synthesis(),
            ])
        }
        Message::Chunk(ChatChunk::Failed(e)) => {
            state.sending = false;
            state.streaming = false;
            state.error = Some(e);
            // Speak what did arrive rather than cutting off mid-word.
            if state.voice && !state.speech_buf.trim().is_empty() {
                let tail = std::mem::take(&mut state.speech_buf);
                state.enqueue_speech(&tail);
            }
            state.speech_buf.clear();
            state.next_synthesis()
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
                    // obviously for E.V. — no need to name it.
                    state.last_reply = state.phase;
                }
                Err(e) => state.error = Some(e),
            }
            Task::none()
        }
        Message::Heard(result) => {
            state.transcribing = false;
            match result {
                Ok(text) if !text.trim().is_empty() => {
                    // Addressed by name, or spoken inside the follow-up window
                    // after E.V.'s last reply → it was meant for E.V.
                    let follow_up = state.phase - state.last_reply < FOLLOW_UP;
                    match (addressed(&text), follow_up) {
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
            state.next_synthesis()
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
            state.streaming = false;
            state.error = None;
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
                || state.phase - state.spoke_at < ECHO_TAIL;

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
                        let snr = state.peak / state.floor.max(ABS_FLOOR);
                        state.peak = 0.0;
                        // Too brief to be an instruction, or too far from the
                        // mic to have been aimed at one: someone else's
                        // conversation carries, but it carries quietly.
                        if voiced < MIN_VOICED
                            || samples.len() < crate::stt::MIN_SAMPLES
                            || snr < CLOSE_TALK_SNR
                        {
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
                                        crate::stt::transcribe(&samples)
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
        let _ = update(&mut s, &client(), Message::Heard(Ok("run diagnostics".into())));
        assert!(!s.transcribing);
        assert_eq!(s.messages.len(), 1);
        assert_eq!(s.messages[0].content, "run diagnostics");
        assert!(s.sending);

        let mut s = State { transcribing: true, ..State::new() };
        let _ = update(&mut s, &client(), Message::Heard(Ok("  ".into())));
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
        let _ = update(&mut s, &client(), Message::Heard(Ok("so anyway I told him no".into())));
        assert!(s.messages.is_empty(), "room chatter must not reach the model");
        assert_eq!(s.draft, "so anyway I told him no");
        assert!(!s.sending);

        // Naming E.V. sends it, and the name itself is not part of the question.
        let mut s = State { transcribing: true, phase: 60.0, ..State::new() };
        let _ = update(&mut s, &client(), Message::Heard(Ok("Hey Eve, run diagnostics".into())));
        assert_eq!(s.messages.len(), 1);
        assert_eq!(s.messages[0].content, "run diagnostics");

        // A reply opens the window: the next thing said needs no name.
        let mut s = State { phase: 60.0, voice: false, ..State::new() };
        reply(&mut s, "Done.");
        s.transcribing = true;
        let _ = update(&mut s, &client(), Message::Heard(Ok("and the second one?".into())));
        assert_eq!(s.messages.last().unwrap().content, "and the second one?");
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
            if (200.0..3600.0).contains(&hz) {
                *b = 0.8;
            }
        }
        assert!(voice_like(&voice));
        assert!(!voice_like(&[0.0; BANDS]), "silence is not speech");
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
        let _ = update(&mut s, &client(), Message::Replied(Ok("hi".into())));
        assert!(s.synthesizing);
        assert_eq!(s.mode(), Mode::Thinking);
    }
}
