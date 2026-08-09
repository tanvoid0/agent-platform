//! The mic gate and its analyzer: the constants that decide what counts as
//! somebody talking, and the pure predicates over one frame of audio.
//!
//! Split out of `assistant.rs` because none of it touches `State` or `Message`.
//! It is a small DSP unit with its own vocabulary — SNR, onset frames, hang
//! time — and reading the assistant's update loop never requires reading it.
//! [`crate::stt`] owns the analysis that produces the `bands` these read.

/// The analyzer's step. Drawing runs at the display's rate; this does not — the
/// window length, the attack/release constants and every frame counter in the
/// mic gate below are written against 60 Hz, and a variable rate would retune
/// all of them at once.
pub const DT: f32 = 1.0 / 60.0;
/// Spectrum bins the HUD draws. Also the number of web spokes it lights.
pub const BANDS: usize = 24;
/// Samples fed to the analyzer each frame (~43 ms at 48 kHz — long enough for
/// the bass bins to see a couple of cycles).
pub const WINDOW: usize = 2048;
/// Level history behind the waveform ribbon, newest last (~2 s at 60 fps).
pub const WAVE: usize = 120;

// --- The gate ---------------------------------------------------------------
// Hands-free means the mic hears everything: the fan, the keyboard, the person
// on the phone next door, and E.V.'s own replies coming back out of the
// speakers. Every constant below exists to throw one of those away.

/// Speech must clear the room's own noise by this much (linear, ≈11 dB). A
/// person talking to their machine sits well above it; a television two rooms
/// away does not.
pub const OPEN_SNR: f32 = 3.5;
/// Absolute floor, for a silent room where the adaptive floor is near zero and
/// any hiss would otherwise clear the SNR test.
pub const ABS_FLOOR: f32 = 0.006;
/// Frames of speech-shaped audio before the gate opens (~130 ms). A door, a
/// key press and a mouse click are all shorter than this.
pub const ONSET_FRAMES: u32 = 8;
/// Silence that ends an utterance. Long enough to think mid-sentence.
pub const HANG: f32 = 0.75;
/// Shortest thing that counts as an instruction, in seconds of actual speech.
pub const MIN_VOICED: f32 = 0.25;
/// Hard cap on one utterance.
pub const MAX_UTTERANCE: f32 = 30.0;
/// Pre-roll kept ahead of the gate opening, so the first consonant survives.
pub const PREROLL: f32 = 0.4;
/// The mic hears the speakers: stay shut while E.V. talks, plus this tail for
/// the room's reverb.
pub const ECHO_TAIL: f32 = 0.35;
/// How far above the speakers' own bleed a voice has to sit to read as you
/// talking over E.V. rather than as E.V. hearing itself.
///
/// ponytail: still half-duplex — this detects the interruption and stops the
/// reply, it does not transcribe through playback. That needs acoustic echo
/// cancellation. On speakers loud enough to clip the mic preamp, the bleed
/// stops tracking and barge-in goes deaf; headphones or lower volume fix it.
pub const BARGE_SNR: f32 = 3.0;
/// Frames of that before E.V. stops talking (~200 ms) — deliberately longer
/// than `ONSET_FRAMES`, because cutting a reply off by mistake is worse than
/// stopping a beat late.
pub const BARGE_FRAMES: u32 = 12;
/// After E.V. replies — or after you open voice mode — you can just talk. Long
/// enough to read the reply and think before answering it. Outside this window
/// an utterance has to name E.V. to be sent on its own: that is the wake word,
/// and `addressed` is what checks it.
pub const FOLLOW_UP: f32 = 45.0;
/// How much louder than the floor an utterance's peak must be to read as
/// close-talk rather than someone else's conversation across the room.
pub const CLOSE_TALK_SNR: f32 = 5.0;
/// Cosine similarity to the enrolled voice below which an utterance is treated
/// as somebody else. Deliberately forgiving: a stranger reaching the model is
/// recoverable, but E.V. ignoring the person it belongs to is not. Watch the
/// HUD's `VID` readout against your own voice and tighten it if strangers get
/// through.
pub const VOICE_MATCH: f32 = 0.82;
/// Utterances averaged into the enrolled print before it stops being provisional.
pub const ENROLL_UTTERANCES: u32 = 4;
/// Similarity to the *provisional* print required to be averaged into it.
/// Looser than `VOICE_MATCH` — that print is one utterance old and still
/// moving, so this bar only has to reject a different person, not a different
/// sentence from the same one.
pub const ENROLL_MATCH: f32 = 0.70;

/// Is this frame shaped like a voice? Fans and traffic sit under 200 Hz, hiss
/// and keyboard clatter spread flat across everything; speech puts most of its
/// energy in the middle. The bands are already computed for the HUD, so this
/// costs a couple of dozen adds.
pub fn voice_like(bands: &[f32; BANDS]) -> bool {
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
pub const GHOSTS: [&str; 8] = [
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
pub fn is_ghost(text: &str) -> bool {
    let plain: String = text
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    GHOSTS.contains(&plain.split_whitespace().collect::<Vec<_>>().join(" ").as_str())
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
pub fn keep_utterance(voiced: f32, samples: usize, peak: f32, floor: f32) -> bool {
    voiced >= MIN_VOICED
        && samples >= crate::stt::MIN_SAMPLES
        && peak / floor.max(1e-4) >= CLOSE_TALK_SNR
}

/// Seconds between two `phase` readings. `phase` wraps every hour, so a plain
/// subtraction goes hugely negative across the wrap — which would read as "E.V.
/// just spoke" forever and leave both timers stuck in their open state.
pub fn since(now: f32, then: f32) -> f32 {
    (now - then).rem_euclid(3600.0)
}

/// Is this frame somebody talking over E.V., rather than E.V.'s own voice
/// arriving back through the mic? Both tests matter: the level rules out the
/// speakers, the shape rules out a door slamming during a reply.
pub fn over_playback(mic: f32, bleed: f32, gate: f32, bands: &[f32; BANDS]) -> bool {
    mic > (bleed * BARGE_SNR).max(gate) && voice_like(bands)
}
