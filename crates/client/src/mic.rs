//! Mikrofon seviye eşiği + sessizlik ön-kontrolü.
//! PLAN §3: göndermeden önce seviye eşiğin altındaysa uyarılır,
//! GÖNDERİLMEZ ve ücret yazılmaz. Sessiz kayıt ücretli hatta gitmez.

/// Sessizlik sayılan RMS tavanı (16-bit normalize, 0..1).
/// Altındaki kayıt gönderilmez.
pub const SILENCE_RMS: f32 = 0.015;
/// Kullanıcıya "mikrofon kapalı/sessiz" uyarısı eşiği (bir tık üstte).
pub const WARN_RMS: f32 = 0.03;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicGate {
    /// Gönderilebilir.
    Open,
    /// Düşük ama gönderilebilir sınırda: uyar, yine de gönder.
    LowWarn,
    /// Sessiz: gönderme, ücret yok.
    Silent,
}

/// i16 PCM diliminin normalize RMS'i.
pub fn rms_i16(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples
        .iter()
        .map(|&s| {
            let v = s as f64 / 32768.0;
            v * v
        })
        .sum();
    (sum / samples.len() as f64).sqrt() as f32
}

pub fn gate(samples: &[i16]) -> MicGate {
    let r = rms_i16(samples);
    if r < SILENCE_RMS {
        MicGate::Silent
    } else if r < WARN_RMS {
        MicGate::LowWarn
    } else {
        MicGate::Open
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn tone(amplitude: f32, n: usize) -> Vec<i16> {
        (0..n)
            .map(|i| ((i as f32 * 440.0 * 2.0 * PI / 16000.0).sin() * amplitude * 32767.0) as i16)
            .collect()
    }

    #[test]
    fn silence_is_blocked() {
        assert_eq!(gate(&vec![0i16; 1600]), MicGate::Silent);
        assert_eq!(gate(&[]), MicGate::Silent);
    }

    #[test]
    fn loud_tone_passes() {
        assert_eq!(gate(&tone(0.5, 1600)), MicGate::Open);
    }

    #[test]
    fn faint_tone_warns_but_passes() {
        // RMS ~0.02: sessiz değil, uyarı bandında.
        assert_eq!(gate(&tone(0.03, 1600)), MicGate::LowWarn);
    }
}
