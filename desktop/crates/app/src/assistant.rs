//! E.V. — the onboard suit AI. Same stateless chat endpoint as `chat.rs`,
//! plus a persona system prompt, an animated HUD, and spoken replies.
//!
//! Voice: Microsoft Edge neural TTS (AriaNeural over the free websocket
//! endpoint — no key, needs internet) played through rodio; falls back to the
//! platform's native engine (SAPI/WinRT, AVSpeech, speech-dispatcher) offline.

use agent_platform_client::types::{ChatCompletionBody, ChatMessage};
use agent_platform_client::Client;
use iced::Task;
use std::io::Cursor;
use tts::Tts;

const PERSONA: &str = "You are E.V., the onboard suit AI of this Agent Platform. \
Style: a superhero suit's heads-up-display assistant — calm, quick-witted, \
protective, with a dry quip now and then. You monitor systems, analyze the \
situation, and give tactical, actionable answers. Your replies may be spoken \
aloud, so keep them short, conversational and free of markdown, bullet lists \
or code fences unless the user asks for code. Answer directly first; skip filler.";

const EDGE_VOICE: &str = "Microsoft Server Speech Text to Speech Voice (en-US, AriaNeural)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Idle,
    Listening,
    Thinking,
    Speaking,
}

#[derive(Default)]
pub struct State {
    pub messages: Vec<ChatMessage>,
    pub draft: String,
    pub sending: bool,
    pub error: Option<String>,
    pub voice: bool,
    pub listening: bool,
    pub phase: f32,
    synthesizing: bool,
    audio: Option<(rodio::OutputStream, rodio::OutputStreamHandle)>,
    sink: Option<rodio::Sink>,
    tts: Option<Tts>,
    tts_failed: bool,
}

impl State {
    pub fn new() -> Self {
        Self { voice: true, ..Self::default() }
    }

    pub fn mode(&self) -> Mode {
        let speaking = self.sink.as_ref().is_some_and(|s| !s.empty())
            || self.tts.as_ref().is_some_and(|t| t.is_speaking().unwrap_or(false));
        if self.listening {
            Mode::Listening
        } else if self.sending || self.synthesizing {
            Mode::Thinking
        } else if speaking {
            Mode::Speaking
        } else {
            Mode::Idle
        }
    }

    fn play(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        if self.audio.is_none() {
            self.audio = Some(rodio::OutputStream::try_default().map_err(|e| e.to_string())?);
        }
        let (_, handle) = self.audio.as_ref().unwrap();
        let sink = rodio::Sink::try_new(handle).map_err(|e| e.to_string())?;
        let source = rodio::Decoder::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
        sink.append(source);
        // A fresh sink per utterance sidesteps rodio's stopped-sink semantics.
        if let Some(old) = self.sink.replace(sink) {
            old.stop();
        }
        Ok(())
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
            let _ = t.speak(text, true);
        }
    }

    fn hush(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        if let Some(t) = self.tts.as_mut() {
            let _ = t.stop();
        }
    }
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

/// One-shot dictation via the OS recognizer: starts the mic, returns after the
/// speaker goes quiet. Blocking — callers wrap it in `spawn_blocking`.
#[cfg(windows)]
fn listen_blocking() -> Result<String, String> {
    use windows::Media::SpeechRecognition::{
        SpeechRecognitionResultStatus, SpeechRecognizer,
    };
    // WinRT on a plain worker thread: without apartment init the recognizer's
    // async ops can silently never complete.
    unsafe {
        let _ = windows::Win32::System::WinRT::RoInitialize(
            windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
        );
    }
    let friendly = |e: windows::core::Error| {
        // 0x80045509: "online speech recognition" is off in Windows privacy
        // settings — the by-far most common failure, worth a real message.
        if e.code().0 as u32 == 0x8004_5509 {
            "Enable it in Windows Settings → Privacy → Speech (online speech \
             recognition), then try again."
                .to_string()
        } else {
            e.to_string()
        }
    };
    let rec = SpeechRecognizer::new().map_err(friendly)?;
    // Explicit timeouts: defaults have been observed to wait forever.
    if let Ok(t) = rec.Timeouts() {
        let ts = |secs: u64| windows::Foundation::TimeSpan {
            Duration: (secs * 10_000_000) as i64,
        };
        let _ = t.SetInitialSilenceTimeout(ts(6));
        let _ = t.SetBabbleTimeout(ts(4));
        let _ = t.SetEndSilenceTimeout(ts(1));
    }
    rec.CompileConstraintsAsync().map_err(friendly)?.get().map_err(friendly)?;
    let result = rec.RecognizeAsync().map_err(friendly)?.get().map_err(friendly)?;
    let status = result.Status().map_err(friendly)?;
    if status != SpeechRecognitionResultStatus::Success {
        return Err(match status {
            SpeechRecognitionResultStatus::MicrophoneUnavailable => {
                "Microphone unavailable — check Windows Settings → Privacy → \
                 Microphone, and that a mic is plugged in."
                    .to_string()
            }
            SpeechRecognitionResultStatus::NetworkFailure => {
                "Speech service unreachable — check your internet connection.".to_string()
            }
            other => format!("Speech recognition failed ({})", other.0),
        });
    }
    Ok(result.Text().map_err(friendly)?.to_string())
}

#[cfg(not(windows))]
fn listen_blocking() -> Result<String, String> {
    // ponytail: whisper-rs + cpal makes this cross-platform — see
    // docs/ev-voice-roadmap.md.
    Err("Voice input is Windows-only for now.".to_string())
}

#[derive(Debug, Clone)]
pub enum Message {
    DraftChanged(String),
    Send,
    Listen,
    Heard(Result<String, String>),
    OpenSpeechSettings,
    Replied(Result<String, String>),
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
            state.messages.push(ChatMessage { role: "user".into(), content: prompt });
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
            };
            let client = client.clone();
            Task::perform(
                async move { client.chat(&body).await.map_err(|e| e.to_string()) },
                Message::Replied,
            )
        }
        Message::Replied(Ok(reply)) => {
            state.sending = false;
            state.messages.push(ChatMessage { role: "assistant".into(), content: reply.clone() });
            if state.voice {
                state.synthesizing = true;
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || synthesize(&reply))
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()))
                    },
                    Message::Synthesized,
                );
            }
            Task::none()
        }
        Message::Listen => {
            if state.listening || state.sending {
                return Task::none();
            }
            state.hush();
            state.listening = true;
            Task::perform(
                async move {
                    // Watchdog: the OS recognizer must never be able to wedge
                    // the UI in "listening" — 20s covers init + a long sentence.
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(20),
                        tokio::task::spawn_blocking(listen_blocking),
                    )
                    .await
                    {
                        Ok(joined) => joined.unwrap_or_else(|e| Err(e.to_string())),
                        Err(_) => Err("Didn't catch anything — the speech service \
                                       isn't responding. Check mic access (Settings \
                                       → Privacy → Microphone) and try again."
                            .to_string()),
                    }
                },
                Message::Heard,
            )
        }
        Message::Heard(result) => {
            state.listening = false;
            match result {
                Ok(text) if !text.trim().is_empty() => {
                    state.draft = text;
                    // Natural conversation: what you said is what you sent.
                    update(state, client, Message::Send)
                }
                Ok(_) => Task::none(), // silence — nothing to send
                Err(e) => {
                    state.error = Some(e);
                    Task::none()
                }
            }
        }
        Message::Replied(Err(e)) => {
            state.sending = false;
            state.error = Some(e);
            Task::none()
        }
        Message::Synthesized(result) => {
            state.synthesizing = false;
            let fallback = match result {
                Ok(bytes) => state.play(bytes).err(),
                Err(e) => Some(e),
            };
            if fallback.is_some() {
                // Offline or no audio device for rodio — native engine's turn.
                if let Some(last) = state.messages.last().filter(|m| m.role == "assistant") {
                    let text = last.content.clone();
                    state.speak_native(&text);
                }
            }
            Task::none()
        }
        Message::OpenSpeechSettings => {
            // ms-settings: deep link straight to Privacy → Speech.
            crate::shell::reveal_path("ms-settings:privacy-speech");
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
            state.error = None;
            Task::none()
        }
        Message::DismissError => {
            state.error = None;
            Task::none()
        }
        Message::Tick => {
            state.phase = (state.phase + 0.05) % (std::f32::consts::TAU * 60.0);
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

    #[test]
    fn failed_turn_stays_with_error() {
        let mut s = State { draft: "hi".into(), ..State::new() };
        let _ = update(&mut s, &client(), Message::Send);
        let _ = update(&mut s, &client(), Message::Replied(Err("boom".into())));
        assert_eq!(s.messages.len(), 1);
        assert!(!s.sending);
        assert_eq!(s.error.as_deref(), Some("boom"));
    }

    #[test]
    fn muted_reply_skips_synthesis() {
        let mut s = State { voice: false, ..State::new() };
        let _ = update(&mut s, &client(), Message::Replied(Ok("hello".into())));
        assert_eq!(s.messages.len(), 1);
        assert!(!s.synthesizing);
        assert_eq!(s.mode(), Mode::Idle);

        let _ = update(&mut s, &client(), Message::ToggleVoice);
        assert!(s.voice);
    }

    #[test]
    fn heard_text_autosends_and_silence_does_not() {
        let mut s = State { listening: true, ..State::new() };
        let _ = update(&mut s, &client(), Message::Heard(Ok("run diagnostics".into())));
        assert!(!s.listening);
        assert_eq!(s.messages.len(), 1);
        assert_eq!(s.messages[0].content, "run diagnostics");
        assert!(s.sending);

        let mut s = State { listening: true, ..State::new() };
        let _ = update(&mut s, &client(), Message::Heard(Ok("  ".into())));
        assert!(s.messages.is_empty());
        assert!(!s.sending);

        let mut s = State { listening: true, ..State::new() };
        let _ = update(&mut s, &client(), Message::Heard(Err("mic missing".into())));
        assert_eq!(s.error.as_deref(), Some("mic missing"));
    }

    #[test]
    #[ignore = "network: hits Edge's TTS endpoint"]
    fn live_edge_synthesis() {
        let bytes = synthesize("Systems online. Good to see you.").unwrap();
        assert!(bytes.len() > 1000, "suspiciously small audio: {} bytes", bytes.len());
    }

    #[test]
    fn voiced_reply_enters_synthesis() {
        let mut s = State::new();
        let _ = update(&mut s, &client(), Message::Replied(Ok("hi".into())));
        assert!(s.synthesizing);
        assert_eq!(s.mode(), Mode::Thinking);
    }
}
