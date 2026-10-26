//! A soft two-note chime, synthesised once into an in-memory WAV.

use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::Media::Audio::*;

static CHIME: OnceLock<Vec<u8>> = OnceLock::new();

fn build() -> Vec<u8> {
    let rate = 44100u32;
    let dur = 1.1f32;
    let n = (rate as f32 * dur) as usize;
    // Two bell-like notes (E6 then A5) with a little harmonic shimmer.
    let notes = [(1318.5f32, 0.0f32), (880.0, 0.16)];
    let mut samples = vec![0f32; n];
    for (freq, start) in notes {
        let s0 = (start * rate as f32) as usize;
        for i in s0..n {
            let t = (i - s0) as f32 / rate as f32;
            let env = (-t * 4.2).exp() * (1.0 - (-t * 400.0).exp());
            let v = (std::f32::consts::TAU * freq * t).sin() * 0.6
                + (std::f32::consts::TAU * freq * 2.01 * t).sin() * 0.18 * (-t * 7.0).exp()
                + (std::f32::consts::TAU * freq * 3.0 * t).sin() * 0.06 * (-t * 10.0).exp();
            samples[i] += v * env * 0.32;
        }
    }
    let data_len = (n * 2) as u32;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 2).to_le_bytes());
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

pub fn chime() {
    let wav = CHIME.get_or_init(build);
    unsafe {
        let _ = PlaySoundW(PCWSTR(wav.as_ptr() as *const u16), None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}
