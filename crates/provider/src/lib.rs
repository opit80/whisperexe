//! provider: sağlayıcı arayüzü + giriş kapısı + sahte gövdeler (F1b).
//!
//! PLAN.md §3 "Kuyruk" + "Ölçü":
//! - Her iki hatta da işlem öncesi doğrulama kapısı: decode + süre +
//!   sessizlik (enerji) kontrolü; geçemeyen kuyruğa/upstream'e girmeden
//!   ÜCRETSİZ reddedilir (sessizlik ücretli API'ye gönderilmez).
//! - Fatura sunucunun ölçtüğü gerçek ses saniyesine göredir; istemcinin
//!   bildirdiği süre yok sayılır. Minimum quantum evde 3sn; fallback
//!   hattında `max(3sn, upstream minimumu)`.
//!
//! Bu faz EV-ONLY'dir: Groq/OpenAI gerçek seçimi YOKTUR; upstream
//! adaptörleri arayüz + sahte (mock) gövdeyle gelir. Gerçek seçim
//! fresh-context test/tartışmasıyla sonra belirlenecek.
//! NOT: Groq'ta istek başına minimum 10sn faturalandırılır (PLAN.md §7).

use std::fmt;

/// Hat üstü tavan: 180sn Opus ≈ 2MB (PLAN.md §3 "Tahsilat").
pub const MAX_OPUS_BYTES: usize = 2 * 1024 * 1024;
/// Tek istek en fazla 180sn (PLAN.md §3 "Limitler").
pub const MAX_SECS: f64 = 180.0;
/// Ev minimum quantumu: 3sn.
pub const HOME_MIN_SECS: f64 = 3.0;
/// Groq upstream minimumu: istek başına 10sn faturalandırılır.
/// Kısa dikte cümlelerinde bile 10sn işler (kuruş mertebesi, §7).
pub const GROQ_MIN_SECS: f64 = 10.0;
/// OpenAI whisper-1: saniyeye yuvarlanır, minimum yok → ev tabanı 3sn işler.
pub const OPENAI_MIN_SECS: f64 = 0.0;

/// Hat üstü format Opus'tur; sunucu inference için 16kHz mono PCM'e çözer.
pub const PCM_RATE_HZ: u32 = 16_000;

/// Çözülmüş ses: 16kHz mono PCM. Süre buradan ölçülür (gerçek-süre).
#[derive(Clone, Debug)]
pub struct PcmAudio {
    pub samples: Vec<i16>,
    pub rate_hz: u32,
}

impl PcmAudio {
    /// Gerçek ses saniyesi = örnek / oran. Fatura buna göre kesilir.
    pub fn duration_secs(&self) -> f64 {
        if self.rate_hz == 0 {
            return 0.0;
        }
        self.samples.len() as f64 / self.rate_hz as f64
    }

    /// Enerji kapısı için RMS (0..1 normalize). Sessizlik bu değerle elenir.
    pub fn rms(&self) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let sum: f64 = self
            .samples
            .iter()
            .map(|&s| {
                let v = s as f64 / 32768.0;
                v * v
            })
            .sum();
        (sum / self.samples.len() as f64).sqrt() as f32
    }
}

/// Opus→PCM çözücü soyutlaması. Gerçek decode ev-işçisindedir
/// (ev-worker crate'i); burada yalnızca kapı mantığı + sahte çözücü var.
pub trait OpusDecoder {
    fn decode(&self, opus: &[u8]) -> Result<PcmAudio, DecodeError>;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DecodeError {
    Empty,
    Corrupt,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Empty => write!(f, "bos opus govdesi"),
            DecodeError::Corrupt => write!(f, "bozuk opus govdesi"),
        }
    }
}

/// Test/sahte çözücü: sentetik PCM üretir (gerçek Opus çözmez).
#[derive(Clone, Copy, Debug)]
pub struct FakeDecoder {
    pub rate_hz: u32,
    /// Üretilecek sesin süresi (sn).
    pub secs: f64,
    /// true ise sessizlik (sıfır örnek) üretir.
    pub silence: bool,
}

impl OpusDecoder for FakeDecoder {
    fn decode(&self, opus: &[u8]) -> Result<PcmAudio, DecodeError> {
        if opus.is_empty() {
            return Err(DecodeError::Empty);
        }
        if opus.len() >= 4 && opus[0..4] == [0xFF, 0xFF, 0xFF, 0xFF] {
            return Err(DecodeError::Corrupt);
        }
        let n = (self.secs * self.rate_hz as f64).round() as usize;
        let mut samples = Vec::with_capacity(n);
        for i in 0..n {
            let v = if self.silence {
                0
            } else {
                // 440Hz sinüs, konuşma seviyesinde genlik.
                let t = i as f64 / self.rate_hz as f64;
                ((2.0 * std::f64::consts::PI * 440.0 * t).sin() * 9000.0) as i16
            };
            samples.push(v);
        }
        Ok(PcmAudio {
            samples,
            rate_hz: self.rate_hz,
        })
    }
}

/// Kapı reddi = ÜCRETSİZ ret (kuyruğa/upstream'e girmeden).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GateReject {
    /// Boş gövde.
    Empty,
    /// Tavan üstü gövde (>2MB).
    TooBig,
    /// Decode başarısız.
    Decode,
    /// Süre aşımı (>180sn).
    TooLong,
    /// Sessizlik/enerji eşiği altı (ücretli API'ye gönderilmez).
    Silence,
}

/// Kapıdan geçmiş ölçüm: faturalandırılabilir gerçek-süre + enerji.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measured {
    pub secs: f64,
    pub rms: f32,
}

/// Doğrulama kapısı yapılandırması.
#[derive(Clone, Copy, Debug)]
pub struct GateConfig {
    pub max_secs: f64,
    pub max_bytes: usize,
    /// Altındaki RMS sessizlik sayılır.
    pub silence_rms: f32,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            max_secs: MAX_SECS,
            max_bytes: MAX_OPUS_BYTES,
            silence_rms: 0.01,
        }
    }
}

/// Doğrulama kapısı: decode + süre + sessizlik. Sıra: boyut → decode →
/// süre → enerji. Geçemeyen ücretsiz reddedilir.
pub fn gate(
    opus: &[u8],
    dec: &dyn OpusDecoder,
    cfg: &GateConfig,
) -> Result<Measured, GateReject> {
    if opus.is_empty() {
        return Err(GateReject::Empty);
    }
    if opus.len() > cfg.max_bytes {
        return Err(GateReject::TooBig);
    }
    let pcm = dec.decode(opus).map_err(|_| GateReject::Decode)?;
    let secs = pcm.duration_secs();
    if secs <= 0.0 || secs > cfg.max_secs {
        return Err(GateReject::TooLong);
    }
    let rms = pcm.rms();
    if rms < cfg.silence_rms {
        return Err(GateReject::Silence);
    }
    Ok(Measured { secs, rms })
}

/// Sağlayıcı hattı (faturalandırma kimliği).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LaneKind {
    Home,
    FallbackGroq,
    FallbackOpenAi,
}

/// Ücretlendirilecek saniye: önce tam-saniyeye yukarı yuvarla, sonra hat
/// minimumunu uygula. Ev: 3sn; Groq: max(3sn, 10sn)=10sn; OpenAI: 3sn taban.
pub fn billable_secs(lane: LaneKind, measured_secs: f64) -> f64 {
    let rounded = measured_secs.ceil().max(1.0);
    let floor = match lane {
        LaneKind::Home => HOME_MIN_SECS,
        LaneKind::FallbackGroq => HOME_MIN_SECS.max(GROQ_MIN_SECS),
        LaneKind::FallbackOpenAi => HOME_MIN_SECS.max(OPENAI_MIN_SECS),
    };
    rounded.max(floor)
}

/// Sağlayıcı sonucu. Boş transkript BAŞARISIZ sayılır (ücretsizdir);
/// upstream'in kestiği kuruşlar operasyonel giderdir, yansıtılmaz.
#[derive(Clone, Debug)]
pub struct ProviderResult {
    pub transcript: String,
    pub billed_secs: f64,
}

impl ProviderResult {
    /// Gerçek sesten metin çıktı mı.
    pub fn is_success(&self) -> bool {
        !self.transcript.trim().is_empty()
    }
}

/// Sağlayıcı arayüzü. Gerçek seçim bu fazda YOK; mock gövdelerle gelir.
pub trait Provider {
    fn lane(&self) -> LaneKind;
    fn transcribe(&self, audio: &Measured) -> ProviderResult;
}

/// Sahte ev sağlayıcısı (ev-only fazın varsayılanı).
pub struct MockHome;

impl Provider for MockHome {
    fn lane(&self) -> LaneKind {
        LaneKind::Home
    }
    fn transcribe(&self, audio: &Measured) -> ProviderResult {
        ProviderResult {
            transcript: format!("[ev {:.1}sn] ornek transkript", audio.secs),
            billed_secs: billable_secs(LaneKind::Home, audio.secs),
        }
    }
}

/// Sahte Groq adaptörü (arayüz + sahte gövde; gerçek seçim sonra).
/// Groq notu: istek başına minimum 10sn faturalandırılır.
pub struct MockGroq;

impl Provider for MockGroq {
    fn lane(&self) -> LaneKind {
        LaneKind::FallbackGroq
    }
    fn transcribe(&self, audio: &Measured) -> ProviderResult {
        ProviderResult {
            transcript: format!("[groq {:.1}sn] ornek transkript", audio.secs),
            billed_secs: billable_secs(LaneKind::FallbackGroq, audio.secs),
        }
    }
}

/// Sahte OpenAI adaptörü (arayüz + sahte gövde; gerçek seçim sonra).
pub struct MockOpenAi;

impl Provider for MockOpenAi {
    fn lane(&self) -> LaneKind {
        LaneKind::FallbackOpenAi
    }
    fn transcribe(&self, audio: &Measured) -> ProviderResult {
        ProviderResult {
            transcript: format!("[openai {:.1}sn] ornek transkript", audio.secs),
            billed_secs: billable_secs(LaneKind::FallbackOpenAi, audio.secs),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> GateConfig {
        GateConfig::default()
    }

    fn speech_dec(secs: f64) -> FakeDecoder {
        FakeDecoder {
            rate_hz: PCM_RATE_HZ,
            secs,
            silence: false,
        }
    }

    #[test]
    fn kapi_bos_govdeyi_ucretsiz_reder() {
        assert_eq!(gate(&[], &speech_dec(5.0), &cfg()), Err(GateReject::Empty));
    }

    #[test]
    fn kapi_tavan_ustu_govdeyi_reder() {
        let big = vec![1u8; MAX_OPUS_BYTES + 1];
        assert_eq!(
            gate(&big, &speech_dec(5.0), &cfg()),
            Err(GateReject::TooBig)
        );
    }

    #[test]
    fn kapi_bozuk_decode_ucretsiz_ret() {
        let bad = [0xFF, 0xFF, 0xFF, 0xFF, 0x01];
        assert_eq!(
            gate(&bad, &speech_dec(5.0), &cfg()),
            Err(GateReject::Decode)
        );
    }

    #[test]
    fn sure_kapisi_180sn_ustunu_reder() {
        let opus = [0x01u8; 8];
        // 200sn > 180sn tavan.
        assert_eq!(
            gate(&opus, &speech_dec(200.0), &cfg()),
            Err(GateReject::TooLong)
        );
        // 180sn sınırda geçer.
        assert!(gate(&opus, &speech_dec(180.0), &cfg()).is_ok());
    }

    #[test]
    fn sessizlik_upstream_oncesi_ucretsiz_elenir() {
        let opus = [0x01u8; 8];
        let silent = FakeDecoder {
            rate_hz: PCM_RATE_HZ,
            secs: 5.0,
            silence: true,
        };
        assert_eq!(gate(&opus, &silent, &cfg()), Err(GateReject::Silence));
    }

    #[test]
    fn gecerli_ses_kapidan_gecer_sure_olculur() {
        let opus = [0x01u8; 8];
        let m = gate(&opus, &speech_dec(7.5), &cfg()).expect("gecmeli");
        assert!((m.secs - 7.5).abs() < 1e-6);
        assert!(m.rms >= cfg().silence_rms);
    }

    #[test]
    fn groq_10sn_minimumu_kisa_seste_isler() {
        // 2.3sn dikte → yukarı yuvarlama 3sn, Groq tabanı 10sn.
        assert_eq!(billable_secs(LaneKind::FallbackGroq, 2.3), 10.0);
        assert_eq!(billable_secs(LaneKind::Home, 2.3), 3.0);
        assert_eq!(billable_secs(LaneKind::FallbackGroq, 12.1), 13.0);
        assert_eq!(billable_secs(LaneKind::Home, 12.1), 13.0);
    }

    #[test]
    fn bos_transkript_basarisiz_ucretsiz() {
        let r = ProviderResult {
            transcript: "   ".to_string(),
            billed_secs: 10.0,
        };
        assert!(!r.is_success());
        let ok = MockHome.transcribe(&Measured { secs: 5.0, rms: 0.2 });
        assert!(ok.is_success());
        assert_eq!(ok.billed_secs, 5.0);
    }
}
