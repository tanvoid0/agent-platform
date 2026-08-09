//! Text-to-speech: what gets spoken, how it is cut into sentences, and the Edge
//! websocket that speaks it.
//!
//! Split out of `assistant.rs` for the same reason as [`crate::assistant_gate`]
//! — nothing here reads `State`. `assistant::warm_voice` stays behind because it
//! returns a `Task<Message>`; it drives [`edge_lock`] from there.

/// The voice Edge speaks in until the user picks another, as a short id.
pub const DEFAULT_VOICE: &str = "en-US-AriaNeural";

/// Edge wants the long form of a voice id. Everything else — the speech backend
/// behind `SPEECH_API_BASE`, including a trained Piper model — wants the short
/// one, so the short one is what is stored and this expands it here.
///
/// Anything that is not `<locale>-<Voice>` falls back to the default: a typo
/// reaching Edge comes back as a socket error in front of every sentence.
pub fn edge_voice(short: &str) -> String {
    let short = if short.is_empty() { DEFAULT_VOICE } else { short };
    let Some((locale, voice)) = short.rsplit_once('-') else {
        return edge_voice(DEFAULT_VOICE);
    };
    format!("Microsoft Server Speech Text to Speech Voice ({locale}, {voice})")
}

/// Speech rates the settings screen offers, as Edge's percent-of-normal.
pub const VOICE_RATES: [(&str, i32); 5] =
    [("Calm", -10), ("Normal", 0), ("Brisk", 15), ("Fast", 30), ("Rapid", 45)];

/// Above normal on purpose: E.V. answers in short bursts, and a narration pace
/// makes a two-line answer feel like a wait.
pub const DEFAULT_VOICE_RATE: i32 = 15;

/// How far the adaptive nudge below may push the rate either side of the
/// setting — past this the voice stops sounding like the one that was picked.
pub const RATE_SPAN: i32 = 20;

/// Shortest chunk worth sending to the synthesizer. Below this the per-clip
/// overhead costs more than the sentence saves, and "Hm." on its own is a
/// worse listen than waiting for the clause it belongs to.
pub const MIN_SPEECH_CHUNK: usize = 24;

/// Except for the chunk that opens a reply, which also breaks on a comma and
/// at half the length. That one is the only chunk anybody waits on: until it
/// is synthesized there is silence, so a clause spoken now beats the whole
/// sentence spoken a second later. Everything after it is being made while the
/// previous one plays, where a longer chunk reads better and costs nothing.
pub const MIN_FIRST_CHUNK: usize = 12;

/// Split off the first complete sentence, leaving the remainder in `buf`.
///
/// Returns `None` while the buffer holds no closed sentence of usable length —
/// the caller keeps accumulating deltas and flushes the tail at end of stream.
///
/// ponytail: naive terminator scan. Splits "3.5" and "Dr. Chen" mid-sentence,
/// which costs a small pause in the wrong place, not a wrong word. Swap in a
/// real segmenter if the voice starts sounding choppy on numeric answers.
pub fn take_sentence(buf: &mut String, first: bool) -> Option<String> {
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
pub fn speech_text(md: &str) -> String {
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
pub static SERVER_SPEECH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

type EdgeClient = msedge_tts::tts::client::MSEdgeTTSClient<std::net::TcpStream>;

/// The Edge websocket, kept open across sentences.
///
/// Opening one costs a DNS lookup, a TCP connect and a TLS + websocket
/// handshake — a second or two on a cold start, and it used to sit in front of
/// every single sentence. That handshake was most of the gap between the first
/// token appearing and the first word being spoken.
pub static EDGE: std::sync::Mutex<Option<EdgeClient>> = std::sync::Mutex::new(None);

pub fn edge_lock() -> std::sync::MutexGuard<'static, Option<EdgeClient>> {
    // A panic inside a synthesis leaves no state worth protecting: the socket
    // is dropped either way, and refusing to ever speak again is worse.
    EDGE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Neural synthesis over Edge's websocket. Blocking, so callers wrap it in
/// `spawn_blocking`; MP3 bytes come back for rodio to decode.
pub fn synthesize(text: &str, rate: i32, voice: &str) -> Result<Vec<u8>, String> {
    use msedge_tts::tts::SpeechConfig;
    let config = SpeechConfig {
        voice_name: edge_voice(voice),
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
