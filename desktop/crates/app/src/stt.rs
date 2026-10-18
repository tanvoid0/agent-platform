//! Local speech-to-text: cpal mic capture → whisper.cpp (`whisper-rs`).
//!
//! Fully offline after first run. The quantized `base.en` model (~60 MB) is
//! downloaded once into the app data dir. Replaced the WinRT recognizer,
//! whose online backend no longer transcribes on current Windows 11 builds.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en-q5_1.bin";
const MODEL_FILE: &str = "ggml-base.en-q5_1.bin";
pub const WHISPER_RATE: u32 = 16_000;

/// Under half a second of audio is a stray click, not an utterance.
pub const MIN_SAMPLES: usize = (WHISPER_RATE / 2) as usize;

/// How much audio the ring keeps. The gate caps an utterance well below this;
/// the slack covers a slow transcribe handing back late.
const RING_SECS: usize = 45;

/// Above this, whisper itself thinks the segment was not speech. Tune down to
/// be stricter about background noise, up if real speech gets dropped.
const NO_SPEECH_MAX: f32 = 0.6;

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

/// Mono capture ring. The window slides, but `base` keeps counting, so an
/// index handed out earlier still means the same instant in time after the
/// audio behind it has aged out.
#[derive(Default)]
struct Ring {
    data: VecDeque<f32>,
    base: u64,
    cap: usize,
}

impl Ring {
    fn push(&mut self, sample: f32) {
        self.data.push_back(sample);
        while self.data.len() > self.cap {
            self.data.pop_front();
            self.base += 1;
        }
    }

    fn end(&self) -> u64 {
        self.base + self.data.len() as u64
    }
}

/// An always-open mic stream feeding a rolling window. `cpal::Stream` is
/// `!Send`, so the recorder lives in UI state, never in a task.
///
/// Hands-free listening needs the mic running before it knows an utterance has
/// started — the pre-roll that saves the first consonant is audio captured
/// before anything decided to capture it.
pub struct Recorder {
    _stream: cpal::Stream,
    ring: Arc<Mutex<Ring>>,
    rate: u32,
}

impl Recorder {
    pub fn start() -> Result<Self, String> {
        let device = cpal::default_host().default_input_device().ok_or(
            "No microphone found — check Windows Settings → Privacy → Microphone \
             and that a mic is plugged in.",
        )?;
        let config = device.default_input_config().map_err(|e| e.to_string())?;
        let rate = config.sample_rate().0;
        let ch = config.channels().max(1) as usize;
        let ring = Arc::new(Mutex::new(Ring {
            cap: rate as usize * RING_SECS,
            ..Ring::default()
        }));
        // Downmix in the callback: every reader downstream wants mono, and the
        // ring is cheaper to reason about with one sample per frame.
        let sink = ring.clone();
        let sink_i16 = ring.clone();
        let err_fn = |e| eprintln!("[stt] stream error: {e}");
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &_| {
                    let mut ring = sink.lock().unwrap();
                    for frame in data.chunks_exact(ch) {
                        ring.push(frame.iter().sum::<f32>() / ch as f32);
                    }
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                &config.into(),
                move |data: &[i16], _: &_| {
                    let mut ring = sink_i16.lock().unwrap();
                    for frame in data.chunks_exact(ch) {
                        let sum: f32 = frame.iter().map(|s| *s as f32 / 32768.0).sum();
                        ring.push(sum / ch as f32);
                    }
                },
                err_fn,
                None,
            ),
            other => return Err(format!("Unsupported mic sample format: {other:?}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Self { _stream: stream, ring, rate })
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Index one past the newest sample — the marker a capture starts from.
    pub fn now(&self) -> u64 {
        self.ring.lock().unwrap().end()
    }

    /// Raw RMS of the last ~100 ms. The gate works in this scale: it is the
    /// room's actual amplitude, not a display value.
    pub fn rms(&self) -> f32 {
        let ring = self.ring.lock().unwrap();
        let window = ring.data.len().saturating_sub(self.rate as usize / 10);
        let tail = ring.data.iter().skip(window);
        let n = ring.data.len() - window;
        if n == 0 {
            return 0.0;
        }
        (tail.map(|s| s * s).sum::<f32>() / n as f32).sqrt()
    }

    /// The same level scaled for the "is the mic hearing me" meter.
    pub fn level(&self) -> f32 {
        (self.rms() * 8.0).clamp(0.0, 1.0) // speech RMS ~0.01–0.3 → usable 0..1
    }

    /// The newest `n` mono samples, plus the capture rate — the HUD's window
    /// onto what the mic is hearing right now.
    pub fn tail_mono(&self, n: usize) -> (Vec<f32>, u32) {
        let ring = self.ring.lock().unwrap();
        let skip = ring.data.len().saturating_sub(n);
        (ring.data.iter().skip(skip).copied().collect(), self.rate)
    }

    /// Everything captured since `from` (clamped to whatever is still in the
    /// ring), resampled to 16 kHz for whisper.
    pub fn since(&self, from: u64) -> Vec<f32> {
        let ring = self.ring.lock().unwrap();
        let skip = from.saturating_sub(ring.base) as usize;
        let raw: Vec<f32> = ring.data.iter().skip(skip).copied().collect();
        drop(ring);
        mono_16k(&raw, 1, self.rate)
    }
}

/// Interleaved multi-channel at any rate → mono 16 kHz (linear resample —
/// plenty for speech).
pub(crate) fn mono_16k(raw: &[f32], channels: u16, rate: u32) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    let mono: Vec<f32> =
        raw.chunks_exact(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
    if rate == WHISPER_RATE || mono.is_empty() {
        return mono;
    }
    let ratio = rate as f32 / WHISPER_RATE as f32;
    let out_len = (mono.len() as f32 / ratio) as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f32 * ratio;
            let idx = pos as usize;
            let frac = pos - idx as f32;
            let a = mono[idx];
            let b = *mono.get(idx + 1).unwrap_or(&a);
            a + (b - a) * frac
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Spectrum
// ---------------------------------------------------------------------------

/// Lowest/highest band centre — speech lives in here, and the HUD only ever
/// shows a couple of dozen bins, so nothing above 6 kHz is worth the cycles.
const BAND_LO: f32 = 90.0;
const BAND_HI: f32 = 6_000.0;
/// Empirical: speech band magnitudes land around 0.002–0.08. Tune here if the
/// HUD reads flat on a quiet mic or pins on a hot one.
const BAND_GAIN: f32 = 16.0;

/// Centre frequency of band `i` of `count` — what the HUD's peak readout
/// reports, kept next to the spacing it has to match.
pub fn band_freq(i: usize, count: usize) -> f32 {
    BAND_LO * (BAND_HI / BAND_LO).powf(i as f32 / (count.max(2) - 1) as f32)
}

/// Hann window applied once — every band that follows sees the same frame.
fn hann(mono: &[f32]) -> Vec<f32> {
    let n = mono.len() as f32;
    mono.iter()
        .enumerate()
        .map(|(j, x)| x * (0.5 - 0.5 * (std::f32::consts::TAU * j as f32 / n).cos()))
        .collect()
}

/// Magnitude at one frequency over an already-windowed frame. Goertzel: a full
/// FFT for a couple of dozen bins would be more code and more work.
fn goertzel(windowed: &[f32], rate: u32, freq: f32) -> f32 {
    let w = std::f32::consts::TAU * freq / rate as f32;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for x in windowed {
        let s = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    let power = (s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0);
    power.sqrt() / (windowed.len() as f32 * 0.25)
}

/// Log-spaced band magnitudes of `mono`, normalized to 0..1, written into
/// `out`. This runs every frame, for the HUD.
pub fn bands(mono: &[f32], rate: u32, out: &mut [f32]) {
    let n = mono.len();
    if n < 256 || rate == 0 || out.len() < 2 {
        out.fill(0.0);
        return;
    }
    let windowed = hann(mono);
    let count = out.len();
    let last = (count - 1) as f32;
    for (i, slot) in out.iter_mut().enumerate() {
        let mag = goertzel(&windowed, rate, band_freq(i, count));
        // Voice is bass-heavy; tilt the gain up with frequency or the top half
        // of the spectrum never moves.
        let tilt = 1.0 + 2.5 * (i as f32 / last);
        *slot = (mag * BAND_GAIN * tilt).sqrt().clamp(0.0, 1.0);
    }
}

// ---------------------------------------------------------------------------
// Voice print
// ---------------------------------------------------------------------------

/// Mel filters behind the cepstrum, and how many coefficients are kept. 13 is
/// the standard speech count; the first (overall loudness) is dropped, because
/// how loud you were is not who you are.
const MFCC_BANDS: usize = 26;
const MFCC_KEEP: usize = 13;
/// A print is the per-coefficient mean and spread over an utterance: the mean
/// carries the timbre of the voice, the spread carries how it moves.
pub const PRINT_DIM: usize = MFCC_KEEP * 2;
pub type VoicePrint = [f32; PRINT_DIM];

/// Mel-spaced filter centres, 80 Hz–6 kHz. Mel rather than the HUD's plain log
/// spacing: it is the scale these features were designed on.
fn mel_center(i: usize) -> f32 {
    let mel = |f: f32| 2595.0 * (1.0 + f / 700.0).log10();
    let (lo, hi) = (mel(80.0), mel(6_000.0));
    let m = lo + (hi - lo) * (i as f32 / (MFCC_BANDS - 1) as f32);
    700.0 * (10f32.powf(m / 2595.0) - 1.0)
}

/// A speaker print for one utterance, or `None` if there was not enough voiced
/// audio to characterize.
///
/// This is cepstral statistics, not a neural embedding: it separates voices
/// that differ in pitch and timbre reliably, and similar voices poorly. It
/// costs no dependency, no model download and no enrollment wizard, which is
/// the trade being made.
/// ponytail: upgrade path is an ECAPA/wespeaker ONNX embedding via `ort` —
/// worth it only if this misfires in practice.
pub fn voice_print(mono: &[f32], rate: u32) -> Option<VoicePrint> {
    let frame = (rate as usize * 25) / 1000;
    let hop = (rate as usize * 10) / 1000;
    if rate == 0 || frame == 0 || mono.len() < frame * 8 {
        return None;
    }
    // Frames far below the utterance's own peak are pauses between words;
    // averaging them in would print the room, not the speaker.
    let peak = mono.iter().fold(0.0_f32, |a, b| a.max(b.abs()));
    if peak < 1e-3 {
        return None; // silence has no timbre to print
    }
    let quiet = peak * 0.08;

    let (mut sums, mut squares) = ([0.0_f32; MFCC_KEEP], [0.0_f32; MFCC_KEEP]);
    let mut frames = 0_usize;
    let mut start = 0;
    while start + frame <= mono.len() {
        let slice = &mono[start..start + frame];
        start += hop;
        let rms = (slice.iter().map(|s| s * s).sum::<f32>() / frame as f32).sqrt();
        if rms < quiet {
            continue;
        }
        let windowed = hann(slice);
        let logs: Vec<f32> = (0..MFCC_BANDS)
            .map(|i| (goertzel(&windowed, rate, mel_center(i)) + 1e-6).ln())
            .collect();
        // DCT-II, dropping coefficient 0 with the loudness it carries.
        for (k, (sum, sq)) in sums.iter_mut().zip(squares.iter_mut()).enumerate() {
            let kk = (k + 1) as f32;
            let c: f32 = logs
                .iter()
                .enumerate()
                .map(|(n, e)| {
                    e * (std::f32::consts::PI * kk * (n as f32 + 0.5) / MFCC_BANDS as f32)
                        .cos()
                })
                .sum();
            *sum += c;
            *sq += c * c;
        }
        frames += 1;
    }
    if frames < 10 {
        return None;
    }

    let n = frames as f32;
    let mut print = [0.0_f32; PRINT_DIM];
    for k in 0..MFCC_KEEP {
        let mean = sums[k] / n;
        print[k] = mean;
        print[MFCC_KEEP + k] = ((squares[k] / n) - mean * mean).max(0.0).sqrt();
    }
    // L2-normalize so comparison is a plain dot product and loudness, mic gain
    // and utterance length drop out of it.
    let norm = print.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm < 1e-6 {
        return None;
    }
    for v in print.iter_mut() {
        *v /= norm;
    }
    Some(print)
}

/// Cosine similarity of two prints, in -1..1. Same speaker on the same mic
/// lands near 1; a different voice drops away from it.
pub fn print_similarity(a: &VoicePrint, b: &VoicePrint) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

// ---------------------------------------------------------------------------
// Transcription
// ---------------------------------------------------------------------------

fn model_path() -> PathBuf {
    crate::shell::app_dir().join("models").join(MODEL_FILE)
}

/// First run downloads the model (~60 MB); after that it's a file read.
///
/// ponytail: deliberately not on the [`crate::downloads`] queue, unlike every
/// other model file. This runs on the whisper worker thread, underneath a
/// `spawn_blocking` in the middle of a dictation — routing it through the UI
/// queue would mean the first press of the mic fails and asks the user to go
/// watch a bar somewhere else. 60 MB once is short enough to just wait for.
/// Move it if the model gets big enough that the wait needs a progress bar.
fn ensure_model() -> Result<PathBuf, String> {
    let path = model_path();
    if path.is_file() {
        return Ok(path);
    }
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let client = reqwest::blocking::Client::builder()
        .timeout(None) // whole-file download; the caller owns the deadline
        .build()
        .map_err(|e| e.to_string())?;
    let bytes = client
        .get(MODEL_URL)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .map_err(|e| format!("Could not download the speech model: {e}"))?;
    // Write via temp file so a cancelled download never looks like a model.
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Blocking (model load + inference) — callers wrap it in `spawn_blocking`.
// ponytail: context is rebuilt per call (~1s); cache it in a thread-local if
// dictation becomes frequent enough to care.
pub fn transcribe(samples: &[f32]) -> Result<String, String> {
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    let model = ensure_model()?;
    let ctx = WhisperContext::new_with_params(
        model.to_str().ok_or("model path is not UTF-8")?,
        WhisperContextParameters::default(),
    )
    .map_err(|e| e.to_string())?;
    let mut state = ctx.create_state().map_err(|e| e.to_string())?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some("en"));
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_no_context(true);

    state.full(params, samples).map_err(|e| e.to_string())?;

    let mut text = String::new();
    for segment in state.as_iter() {
        // Whisper narrates noise it cannot resolve — "(wind blowing)",
        // "[BLANK_AUDIO]", or a confident-looking "Thank you." over a fan —
        // and flags those segments as probably-not-speech. Dropping them here
        // is the cheapest filter available and it needs no second model.
        if segment.no_speech_probability() > NO_SPEECH_MAX {
            continue;
        }
        text.push_str(&segment.to_str_lossy().map_err(|e| e.to_string())?);
    }
    // Whatever survived that is still a transcript of a *sound*: a bracketed
    // or parenthesized whole is a stage direction, not an instruction.
    let trimmed = text.trim();
    let bracketed = (trimmed.starts_with('[') && trimmed.ends_with(']'))
        || (trimmed.starts_with('(') && trimmed.ends_with(')'));
    if bracketed || !trimmed.chars().any(|c| c.is_alphanumeric()) {
        return Ok(String::new());
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_16k_downmixes_and_resamples() {
        // Stereo 32 kHz: L=1.0, R=0.0 → mono 0.5, half the frames.
        let raw: Vec<f32> = (0..64).flat_map(|_| [1.0, 0.0]).collect();
        let out = mono_16k(&raw, 2, 32_000);
        assert_eq!(out.len(), 32);
        assert!(out.iter().all(|s| (s - 0.5).abs() < 1e-6));

        // Already mono 16 kHz: untouched.
        let raw = vec![0.25_f32; 100];
        assert_eq!(mono_16k(&raw, 1, WHISPER_RATE), raw);
    }

    #[test]
    fn bands_peak_at_the_tone_that_is_playing() {
        let rate = 48_000;
        let tone = |hz: f32| -> Vec<f32> {
            (0..2048)
                .map(|i| (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin() * 0.2)
                .collect()
        };
        let mut out = [0.0_f32; 24];
        let peak = |out: &[f32; 24]| {
            out.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0
        };

        bands(&tone(1_000.0), rate, &mut out);
        let hi = peak(&out);
        bands(&tone(200.0), rate, &mut out);
        let lo = peak(&out);
        assert!(lo < hi, "200 Hz should peak below 1 kHz, got {lo} vs {hi}");

        // Silence reads as silence; too short a window reads as nothing at all.
        bands(&vec![0.0; 2048], rate, &mut out);
        assert!(out.iter().all(|b| *b < 0.01), "silence lit up: {out:?}");
        out.fill(1.0);
        bands(&tone(1_000.0)[..64], rate, &mut out);
        assert!(out.iter().all(|b| *b == 0.0));
    }

    /// A crude voice: harmonics of `f0` shaped by two formants. Different
    /// pitch and different formants is what "a different person" means here.
    fn voiced(f0: f32, formants: [f32; 2], rate: u32, secs: f32, phase: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / rate as f32 + phase;
                let mut s = 0.0;
                for h in 1..40 {
                    let f = f0 * h as f32;
                    if f > 6_000.0 {
                        break;
                    }
                    let gain: f32 =
                        formants.iter().map(|c| 1.0 / (1.0 + ((f - c) / 140.0).powi(2))).sum();
                    s += gain * (std::f32::consts::TAU * f * t).sin();
                }
                // Syllable-rate envelope, so the spread half of the print is
                // not measuring a perfectly steady tone.
                s * 0.1 * (0.6 + 0.4 * (std::f32::consts::TAU * 4.0 * t).sin())
            })
            .collect()
    }

    #[test]
    fn voice_prints_separate_two_speakers() {
        let rate = 16_000;
        let me = voice_print(&voiced(120.0, [700.0, 1_200.0], rate, 2.0, 0.0), rate).unwrap();
        let me_again =
            voice_print(&voiced(120.0, [700.0, 1_200.0], rate, 1.4, 0.37), rate).unwrap();
        let someone_else =
            voice_print(&voiced(210.0, [450.0, 2_500.0], rate, 2.0, 0.0), rate).unwrap();

        let same = print_similarity(&me, &me_again);
        let other = print_similarity(&me, &someone_else);
        // Measured on these two: same 1.00, different 0.79 — which is where
        // `assistant::VOICE_MATCH` (0.82) was set. Real voices are noisier than
        // synthetic ones, so that threshold leans towards letting you through.
        assert!(same > 0.9, "same voice should match itself closely, got {same}");
        assert!(
            same - other > 0.1,
            "speakers must separate: same {same}, other {other}"
        );

        // Too little audio to characterize is `None`, never a bogus print.
        assert!(voice_print(&voiced(120.0, [700.0, 1_200.0], rate, 0.05, 0.0), rate).is_none());
        assert!(voice_print(&vec![0.0; rate as usize], rate).is_none());
    }
}
