//! Notification sounds. Each one is synthesised once into an in-memory WAV;
//! "Windows" plays the user's own Windows notification sound, and "Off" is
//! silent. The choice lives in Settings (`store::Sound`).

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::OnceLock;

use windows::core::{w, PCWSTR};
use windows::Win32::Media::Audio::*;

use crate::store::Sound;

const RATE: u32 = 44100;
static CURRENT: AtomicU8 = AtomicU8::new(1);
static MUTED: AtomicBool = AtomicBool::new(false);
static WAVS: [OnceLock<Vec<u8>>; 4] = [OnceLock::new(), OnceLock::new(), OnceLock::new(), OnceLock::new()];

fn index(s: Sound) -> u8 {
    Sound::ALL.iter().position(|x| *x == s).unwrap_or(1) as u8
}

/// Which sound `chime()` plays (set from Settings).
pub fn set(s: Sound) {
    CURRENT.store(index(s), Ordering::Relaxed);
}

/// Silence everything (the self-test runs on the user's PC).
pub fn mute(on: bool) {
    MUTED.store(on, Ordering::Relaxed);
}

/// Play the chosen notification sound. Returns false when it is off.
pub fn chime() -> bool {
    play(Sound::ALL[CURRENT.load(Ordering::Relaxed) as usize % Sound::ALL.len()])
}

/// Play `s` now (Settings previews a sound when you pick it).
pub fn play(s: Sound) -> bool {
    if MUTED.load(Ordering::Relaxed) || s == Sound::Off {
        return false;
    }
    unsafe {
        if s == Sound::Windows {
            let _ = PlaySoundW(w!("Notification.Default"), None, SND_ALIAS | SND_ASYNC | SND_NODEFAULT);
            return true;
        }
        let Some(wav) = wav(s) else { return false };
        let _ = PlaySoundW(PCWSTR(wav.as_ptr() as *const u16), None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
    true
}

/// The synthesised WAV for `s` (None for Off and Windows).
pub fn wav(s: Sound) -> Option<&'static Vec<u8>> {
    let (slot, build): (usize, fn() -> Vec<f32>) = match s {
        Sound::Chime => (0, chime_samples),
        Sound::Bell => (1, bell_samples),
        Sound::Pop => (2, pop_samples),
        Sound::Marimba => (3, marimba_samples),
        Sound::Off | Sound::Windows => return None,
    };
    Some(WAVS[slot].get_or_init(|| to_wav(&build())))
}

fn buf(secs: f32) -> Vec<f32> {
    vec![0f32; (RATE as f32 * secs) as usize]
}

/// Add a decaying tone with a few partials: (ratio, level, extra decay).
fn tone(out: &mut [f32], freq: f32, start: f32, decay: f32, gain: f32, partials: &[(f32, f32, f32)]) {
    let s0 = (start * RATE as f32) as usize;
    for i in s0..out.len() {
        let t = (i - s0) as f32 / RATE as f32;
        let env = (-t * decay).exp() * (1.0 - (-t * 400.0).exp());
        let mut v = 0.0;
        for &(ratio, level, extra) in partials {
            v += (std::f32::consts::TAU * freq * ratio * t).sin() * level * (-t * extra).exp();
        }
        out[i] += v * env * gain;
    }
}

/// The original: a soft two-note chime (E6 then A5).
fn chime_samples() -> Vec<f32> {
    let mut s = buf(1.1);
    let p = [(1.0, 0.6, 0.0), (2.01, 0.18, 7.0), (3.0, 0.06, 10.0)];
    tone(&mut s, 1318.5, 0.0, 4.2, 0.32, &p);
    tone(&mut s, 880.0, 0.16, 4.2, 0.32, &p);
    s
}

/// One bright bell strike with inharmonic overtones and a long ring.
fn bell_samples() -> Vec<f32> {
    let mut s = buf(1.6);
    tone(&mut s, 1046.5, 0.0, 2.6, 0.34, &[(1.0, 0.55, 0.0), (2.76, 0.25, 3.0), (5.4, 0.12, 6.0), (8.93, 0.05, 9.0)]);
    s
}

/// Two quick rising bubbles.
fn pop_samples() -> Vec<f32> {
    let mut s = buf(0.35);
    for (start, f0) in [(0.0f32, 520.0f32), (0.09, 760.0)] {
        let s0 = (start * RATE as f32) as usize;
        let mut phase = 0.0f32;
        for i in s0..s.len() {
            let t = (i - s0) as f32 / RATE as f32;
            let f = f0 * (1.0 + 1.2 * (1.0 - (-t * 40.0).exp()));
            phase += std::f32::consts::TAU * f / RATE as f32;
            let env = (-t * 26.0).exp() * (1.0 - (-t * 900.0).exp());
            s[i] += phase.sin() * env * 0.42;
        }
    }
    s
}

/// A short wooden arpeggio (C6, E6, G6).
fn marimba_samples() -> Vec<f32> {
    let mut s = buf(0.9);
    let p = [(1.0, 0.7, 0.0), (3.93, 0.2, 18.0), (9.2, 0.05, 30.0)];
    for (k, f) in [1046.5f32, 1318.5, 1568.0].into_iter().enumerate() {
        tone(&mut s, f, k as f32 * 0.11, 7.5, 0.3, &p);
    }
    s
}

fn to_wav(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        w.extend_from_slice(&v.to_le_bytes());
    }
    w
}

/// Peak level (0..1) of a WAV built here, for the self-test.
pub fn peak(wav: &[u8]) -> f32 {
    wav[44..].chunks_exact(2).map(|c| (i16::from_le_bytes([c[0], c[1]]) as f32 / 32767.0).abs()).fold(0.0, f32::max)
}
