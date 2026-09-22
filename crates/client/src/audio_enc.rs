//! Ses kodlayıcı soyutlaması. Hat üstü format Opus'tur
//! (PLAN §3: 180sn ≈ 0.5MB, hızlı upload; tavan gövde ~2MB).
//!
//! Gerçek Opus bağlaması Tauri derlemesinde `audiopus` ile verilir;
//! çekirdek burada çerçeveleme/tavan matematiğini ve `Encoder`
//! arayüzünü sabitler. Gerçek mikrofon yoksa `MockSource` kullanılır.

/// Opus hat parametreleri.
pub const OPUS_SAMPLE_RATE: u32 = 48_000;
pub const OPUS_CHANNELS: u32 = 1;
pub const OPUS_FRAME_MS: u32 = 20;
/// Tek istek tavanı (sn). Üstü istemcide kesilir.
pub const MAX_SECS: u32 = 180;
/// 20ms @48kHz mono = 960 örnek.
pub const FRAME_SAMPLES: usize = (OPUS_SAMPLE_RATE / 1000 * OPUS_FRAME_MS) as usize;
/// 180sn'deki çerçeve sayısı.
pub const MAX_FRAMES: u32 = MAX_SECS * 1000 / OPUS_FRAME_MS;
/// Gövde tavanı (~2MB, PLAN §3).
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Kodlayıcı arayüzü: 16kHz/mikrofon PCM girer, Opus paket çıkar.
pub trait Encoder {
    fn push_pcm16(&mut self, samples: &[i16]) -> Result<(), EncodeError>;
    fn finish(self) -> Result<Vec<u8>, EncodeError>;
    fn encoded_bytes(&self) -> usize;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeError {
    TooLong,
    Backend(String),
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncodeError::TooLong => write!(f, "Kayit 180 saniyeyi asti, kesildi."),
            EncodeError::Backend(e) => write!(f, "Kodlayici hatasi: {e}"),
        }
    }
}

/// Test/mock kodlayıcı: tavan mantığını birebir uygular,
/// bayt yerine çerçeve sayacı yazar (boyut tahmini için).
#[derive(Debug, Default)]
pub struct MockEncoder {
    frames: u32,
    bytes: usize,
}

impl MockEncoder {
    /// Tipik Opus ~2.8KB/sn varsayımıyla tahmini gövde boyutu.
    pub const BYTES_PER_SEC: usize = 2_800;
}

impl Encoder for MockEncoder {
    fn push_pcm16(&mut self, samples: &[i16]) -> Result<(), EncodeError> {
        // 16kHz giriş varsayımı: 320 örnek = 20ms.
        let frames = samples.len().div_ceil(320) as u32;
        if self.frames + frames > MAX_FRAMES {
            return Err(EncodeError::TooLong);
        }
        self.frames += frames;
        self.bytes = self.frames as usize * Self::BYTES_PER_SEC / 50;
        if self.bytes > MAX_BODY_BYTES {
            return Err(EncodeError::TooLong);
        }
        Ok(())
    }

    fn finish(self) -> Result<Vec<u8>, EncodeError> {
        Ok(vec![0u8; self.bytes])
    }

    fn encoded_bytes(&self) -> usize {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_math_matches_180s_cap() {
        assert_eq!(FRAME_SAMPLES, 960);
        assert_eq!(MAX_FRAMES, 9_000);
    }

    #[test]
    fn mock_encoder_caps_at_180s() {
        let mut e = MockEncoder::default();
        // 179sn geçer.
        assert!(e.push_pcm16(&vec![0i16; 179 * 16_000]).is_ok());
        // +2sn taşar → kes.
        assert_eq!(
            e.push_pcm16(&vec![0i16; 2 * 16_000]),
            Err(EncodeError::TooLong)
        );
    }

    #[test]
    fn full_180s_stays_under_2mb() {
        let mut e = MockEncoder::default();
        e.push_pcm16(&vec![0i16; 180 * 16_000]).unwrap();
        assert!(e.encoded_bytes() < MAX_BODY_BYTES);
    }
}
