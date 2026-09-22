//! F9 bas-konuş kayıt oturumu: mikrofon eşiği, sessizlik
//! ön-kontrolü, 180sn tavan kesmesi. Her F9 basışı bağımsız istektir
//! (PLAN §3: oturum kavramı yoktur).

use crate::audio_enc::{Encoder, MAX_SECS};
use crate::mic::{self, MicGate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Kullanıcı F9'u bıraktı.
    Released,
    /// 180sn tavanında istemcide kesildi.
    Capped180,
    /// Sessizlik: gönderilmedi, ücret yok.
    Silent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordError {
    Silent,
}

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecordError::Silent => write!(f, "Ses seviyesi cok dusuk, gonderilmedi (ucret yok)."),
        }
    }
}

pub struct Session<E: Encoder> {
    encoder: E,
    secs_recorded: u32,
    heard_audio: bool,
    warned_low: bool,
}

impl<E: Encoder> Session<E> {
    pub fn new(encoder: E) -> Self {
        Self {
            encoder,
            secs_recorded: 0,
            heard_audio: false,
            warned_low: false,
        }
    }

    /// 1 saniyelik 16kHz mono PCM dilimi besle.
    /// `Ok(true)` kayıt sürüyor, `Ok(false)` tavan kesmesi (bitir).
    pub fn push_second(&mut self, pcm_16k: &[i16]) -> Result<bool, RecordError> {
        match mic::gate(pcm_16k) {
            MicGate::Silent => {
                if !self.heard_audio {
                    return Err(RecordError::Silent);
                }
                // Konuşma sonrası kısa sessizlik: kayda devam (bitişi F9 bırakır).
            }
            MicGate::LowWarn => self.warned_low = true,
            MicGate::Open => self.heard_audio = true,
        }
        if self.secs_recorded >= MAX_SECS {
            return Ok(false);
        }
        // Kodlayıcı tavanı da keser (derin savunma).
        if self.encoder.push_pcm16(pcm_16k).is_err() {
            self.secs_recorded = MAX_SECS;
            return Ok(false);
        }
        self.secs_recorded += 1;
        Ok(self.secs_recorded < MAX_SECS)
    }

    pub fn low_warned(&self) -> bool {
        self.warned_low
    }

    pub fn secs(&self) -> u32 {
        self.secs_recorded
    }

    pub fn finish(self, released: bool) -> (E, u32, StopReason) {
        let reason = if self.secs_recorded >= MAX_SECS {
            StopReason::Capped180
        } else if released {
            StopReason::Released
        } else {
            StopReason::Released
        };
        let secs = self.secs_recorded;
        let enc = self.encoder;
        (enc, secs, reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_enc::MockEncoder;
    use std::f32::consts::PI;

    fn tone_1s() -> Vec<i16> {
        (0..16_000)
            .map(|i| ((i as f32 * 440.0 * 2.0 * PI / 16000.0).sin() * 0.5 * 32767.0) as i16)
            .collect()
    }

    #[test]
    fn silence_upfront_is_rejected_without_charge() {
        let mut s = Session::new(MockEncoder::default());
        assert_eq!(
            s.push_second(&vec![0i16; 16_000]),
            Err(RecordError::Silent)
        );
        assert_eq!(s.secs(), 0);
    }

    #[test]
    fn caps_at_180_seconds() {
        let mut s = Session::new(MockEncoder::default());
        let t = tone_1s();
        let mut cont = true;
        for _ in 0..200 {
            cont = s.push_second(&t).unwrap();
            if !cont {
                break;
            }
        }
        assert!(!cont);
        assert_eq!(s.secs(), MAX_SECS);
        let (_, secs, reason) = s.finish(true);
        assert_eq!((secs, reason), (MAX_SECS, StopReason::Capped180));
    }
}
