//! F1a protokol crate'i: PLAN.md §3 kilitli kararlar.
//!
//! Kapsar:
//! - İstek ID / hesap ID / ses_hash doğrulama
//! - Opus kabul kuralı (tavan 180sn ≈ 2MB)
//! - Hata kodları (makine-okunur, sabit string)
//! - Idempotency anahtarı: ID + ses_hash, hesap başına kapsam
//! - Başarı 24 saat önbellek (yalnızca metin, ses asla)
//! - Boru-hattı sırası: idempotency → doğrulama → bakiye/bloke → kuyruk
//! - Quantum: ev 3sn, fallback max(3sn, upstream minimumu)
//! - Tarife kabul anında dondurulur (bloke edilen sürümle kesinleşir)
//!
//! Ölçü kuralı: fatura SUNUCUNUN ölçtüğü gerçek saniyeye göredir;
//! istemcinin bildirdiği süre yok sayılır (log dışında).

use std::collections::HashMap;

/// Tek istek tavanı: 180 saniye Opus ≈ 2MB.
pub const MAX_OPUS_BYTES: usize = 2 * 1024 * 1024;
/// Tek istek en fazla 180sn.
pub const MAX_SECONDS: u32 = 180;
/// Ev hattı minimum quantum: 3sn.
pub const QUANTUM_HOME_SECS: u32 = 3;
/// Başarılı sonuç önbelleği TTL: 24 saat (saniye).
pub const RESULT_CACHE_TTL_SECS: u64 = 24 * 3600;

/// Hat (tarife) seçimi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    Home,
    Fallback,
}

/// Tarife anlık görüntüsü. Kabul anında dondurulur; bloke edilen
/// sürümle kesinleşir (sonradan değişen fiyat işlemez).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tariff {
    /// Tarife sürümü (deftere yazılır).
    pub version: u64,
    /// Ev hattı ücreti, kuruş/dk.
    pub home_kurus_per_min: u64,
    /// Fallback hattı ücreti, kuruş/dk (upstream maliyeti + marj).
    pub fallback_kurus_per_min: u64,
    /// Upstream sağlayıcının minimum faturalama süresi (sn). Örn. Groq: 10.
    pub upstream_min_secs: u32,
}

/// Ölçülen (sunucu-tarafı, decode sonrası gerçek) süreye uygulanacak
/// faturalı saniye: yukarı yuvarlanır, quantumun altına düşmez.
///
/// * ev: `max(ölçülen↑, 3sn)`
/// * fallback: `max(ölçülen↑, 3sn, upstream minimumu)`
pub fn billable_seconds(measured_secs: f64, line: Line, tariff: &Tariff) -> u32 {
    let measured = if measured_secs.is_finite() && measured_secs > 0.0 {
        measured_secs.ceil() as u32
    } else {
        0
    };
    let quantum = match line {
        Line::Home => QUANTUM_HOME_SECS,
        Line::Fallback => QUANTUM_HOME_SECS.max(tariff.upstream_min_secs),
    };
    measured.max(quantum).min(MAX_SECONDS)
}

/// Faturalı saniyeden kuruş tutar: dakikaya yukarı yuvarlanır.
/// Doyumlu aritmetik (taşmada paniğe/sarmaya karşı u64::MAX'a doyar).
pub fn quote_kurus(billable_secs: u32, line: Line, tariff: &Tariff) -> u64 {
    let per_min = match line {
        Line::Home => tariff.home_kurus_per_min,
        Line::Fallback => tariff.fallback_kurus_per_min,
    };
    (billable_secs as u64)
        .saturating_mul(per_min)
        .saturating_add(59)
        / 60
}

/// Bir quantumluk (minimum) ön-kontrol tutarı: bakiye < bu ise
/// inference başlamadan reddedilir, ücret yazılmaz.
pub fn quantum_cost_kurus(line: Line, tariff: &Tariff) -> u64 {
    let q = match line {
        Line::Home => QUANTUM_HOME_SECS,
        Line::Fallback => QUANTUM_HOME_SECS.max(tariff.upstream_min_secs),
    };
    quote_kurus(q, line, tariff)
}

// ---------------------------------------------------------------------------
// Kimlikler
// ---------------------------------------------------------------------------

/// Hesap kimliği: boş olamaz, ≤64, `[A-Za-z0-9._-]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AccountId(String);

/// Tekil istek kimliği: boş olamaz, ≤128, `[A-Za-z0-9._:-]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(String);

/// Ses hash'i (sunucu-tarafı baytların özeti, örn. hex SHA-256):
/// boş olamaz, ≤256, ASCII grafik (boşluk/kontrol yok).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AudioHash(String);

fn check_token(s: &str, max_len: usize, extra: &[u8]) -> bool {
    if s.is_empty() || s.len() > max_len {
        return false;
    }
    s.bytes().all(|b| {
        b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-' || extra.contains(&b)
    })
}

impl AccountId {
    pub fn new(s: &str) -> Result<Self, ErrorCode> {
        if check_token(s, 64, &[]) {
            Ok(Self(s.to_string()))
        } else {
            Err(ErrorCode::InvalidAccount)
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl RequestId {
    pub fn new(s: &str) -> Result<Self, ErrorCode> {
        if check_token(s, 128, &[b':']) {
            Ok(Self(s.to_string()))
        } else {
            Err(ErrorCode::InvalidRequestId)
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AudioHash {
    pub fn new(s: &str) -> Result<Self, ErrorCode> {
        if s.is_empty() || s.len() > 256 {
            return Err(ErrorCode::InvalidAudioHash);
        }
        if s.bytes().all(|b| b.is_ascii_graphic()) {
            Ok(Self(s.to_string()))
        } else {
            Err(ErrorCode::InvalidAudioHash)
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ---------------------------------------------------------------------------
// Hata kodları
// ---------------------------------------------------------------------------

/// Makine-okunur hata kodları. Kabloda `code` string'i taşınır.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    InvalidRequestId,
    InvalidAudioHash,
    InvalidAccount,
    PayloadTooLarge,
    DurationTooLong,
    Unauthorized,
    ForbiddenHwid,
    UpdateRequired,
    InsufficientBalance,
    QueueFull,
    QueueWaitTimeout,
    AccountCooldown,
    SilentAudio,
    DecodeFailed,
    Maintenance,
    IdConflict,
    Timeout,
    Internal,
}

impl ErrorCode {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequestId => "invalid_request_id",
            Self::InvalidAudioHash => "invalid_audio_hash",
            Self::InvalidAccount => "invalid_account",
            Self::PayloadTooLarge => "payload_too_large",
            Self::DurationTooLong => "duration_too_long",
            Self::Unauthorized => "unauthorized",
            Self::ForbiddenHwid => "forbidden_hwid",
            Self::UpdateRequired => "update_required",
            Self::InsufficientBalance => "insufficient_balance",
            Self::QueueFull => "queue_full",
            Self::QueueWaitTimeout => "queue_wait_timeout",
            Self::AccountCooldown => "account_cooldown",
            Self::SilentAudio => "silent_audio",
            Self::DecodeFailed => "decode_failed",
            Self::Maintenance => "maintenance",
            Self::IdConflict => "id_conflict",
            Self::Timeout => "timeout",
            Self::Internal => "internal",
        }
    }
}

// ---------------------------------------------------------------------------
// Gövde kabul
// ---------------------------------------------------------------------------

/// Hat üstü gövde kabulü (boyut tavanı). Süre beyanı FİYATA girmez;
/// sunucu decode sonrası kendi ölçer. `measured_secs` yalnızca
/// üst-sınır kontrolü içindir (tavan 180sn).
pub fn accept_body(body_bytes: usize, measured_secs: f64) -> Result<(), ErrorCode> {
    if body_bytes > MAX_OPUS_BYTES {
        return Err(ErrorCode::PayloadTooLarge);
    }
    if measured_secs.is_finite() && measured_secs > MAX_SECONDS as f64 {
        return Err(ErrorCode::DurationTooLong);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Idempotency
// ---------------------------------------------------------------------------

/// Idempotency anahtarı: ID + ses_hash, hesap başına kapsam.
/// Aynı ID farklı sesle gelirse YENİ istek sayılır (önbellek dönülmez).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey {
    pub account: AccountId,
    pub request_id: RequestId,
    pub audio_hash: AudioHash,
}

impl IdempotencyKey {
    pub fn new(account: AccountId, request_id: RequestId, audio_hash: AudioHash) -> Self {
        Self {
            account,
            request_id,
            audio_hash,
        }
    }
}

#[derive(Debug, Clone)]
struct CachedEntry {
    text: String,
    stored_at_secs: u64,
}

/// Başarı sonuç önbelleği: 24 saat, yalnızca metin (ses asla),
/// hesap başına kapsam. Başarısızlık ÖNBELLEĞE YAZILMAZ — aynı ID
/// ile yeniden denemeye izin verilir, deneme başarılı olursa
/// ücretlenir ve o zaman önbelleğe yazılır.
///
/// Saat (`now_secs`) dışarıdan verilir: test edilebilirlik + sunucu
/// saatine tek-elden bağlılık için.
#[derive(Debug, Default)]
pub struct ResultCache {
    map: HashMap<IdempotencyKey, CachedEntry>,
}

impl ResultCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Vuruşta metni döndürür (ücretsiz dönüş). Süre dolmuşsa düşürür.
    pub fn lookup(&mut self, key: &IdempotencyKey, now_secs: u64) -> Option<String> {
        match self.map.get(key) {
            Some(e) if now_secs.saturating_sub(e.stored_at_secs) < RESULT_CACHE_TTL_SECS => {
                Some(e.text.clone())
            }
            Some(_) => {
                self.map.remove(key);
                None
            }
            None => None,
        }
    }

    /// Yalnızca BAŞARILI + BOŞ OLMAYAN transkript önbelleğe yazılır.
    /// Boş transkript başarısız sayılır, yazılmaz.
    pub fn store(&mut self, key: IdempotencyKey, text: &str, now_secs: u64) {
        if text.is_empty() {
            return;
        }
        self.map.insert(
            key,
            CachedEntry {
                text: text.to_string(),
                stored_at_secs: now_secs,
            },
        );
    }

    /// KVKK hard-delete: hesabın tüm önbellek girdileri silinir.
    pub fn purge_account(&mut self, account: &AccountId) {
        self.map.retain(|k, _| &k.account != account);
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }
}

// ---------------------------------------------------------------------------
// Boru-hattı sırası
// ---------------------------------------------------------------------------

/// Kontrol sırası (kilitli): idempotency → doğrulama → bakiye/bloke → kuyruk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineStage {
    Idempotency,
    Verify,
    BalanceBlock,
    Queue,
}

/// Geçerli sıra sabiti; broker bu sırada çalışır.
pub const PIPELINE_ORDER: [PipelineStage; 4] = [
    PipelineStage::Idempotency,
    PipelineStage::Verify,
    PipelineStage::BalanceBlock,
    PipelineStage::Queue,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn tariff() -> Tariff {
        Tariff {
            version: 7,
            home_kurus_per_min: 60,
            fallback_kurus_per_min: 600,
            upstream_min_secs: 10,
        }
    }

    #[test]
    fn quantum_ev_3sn_fallback_max_3_upstream() {
        let t = tariff();
        assert_eq!(billable_seconds(1.2, Line::Home, &t), 3);
        assert_eq!(billable_seconds(5.0, Line::Home, &t), 5);
        // Groq 10sn minimumu: 4sn ses 10sn faturalandırılır.
        assert_eq!(billable_seconds(4.0, Line::Fallback, &t), 10);
        assert_eq!(billable_seconds(12.3, Line::Fallback, &t), 13);
    }

    #[test]
    fn client_declared_duration_ignored_for_price_shape() {
        // Fiyat yalnızca ölçülen süre + quantumdan türer; beyanın
        // kabul fonksiyonunda fiyat etkisi yoktur (derleyici-kanıtı:
        // accept_body fiyat döndürmez, quote yalnızca measured alır).
        let t = tariff();
        assert_eq!(quote_kurus(3, Line::Home, &t), 3); // 3sn * 60kr/dk / 60
        assert_eq!(quote_kurus(10, Line::Fallback, &t), 100);
    }

    #[test]
    fn body_limits() {
        assert!(accept_body(512 * 1024, 120.0).is_ok());
        assert_eq!(
            accept_body(MAX_OPUS_BYTES + 1, 10.0),
            Err(ErrorCode::PayloadTooLarge)
        );
        assert_eq!(
            accept_body(100, 181.0),
            Err(ErrorCode::DurationTooLong)
        );
    }

    #[test]
    fn ids_validated() {
        assert!(RequestId::new("").is_err());
        assert!(RequestId::new("req-1:abc_xyz.2").is_ok());
        assert!(RequestId::new("kötü id").is_err());
        assert!(AudioHash::new("").is_err());
        assert!(AudioHash::new("ab12cd ef").is_err());
        assert!(AudioHash::new("ab12cdef0123").is_ok());
        assert!(AccountId::new("").is_err());
        assert!(AccountId::new("ali").is_ok());
    }

    #[test]
    fn idempotent_replay_returns_cached_text_free() {
        let mut c = ResultCache::new();
        let k = IdempotencyKey::new(
            AccountId::new("ali").unwrap(),
            RequestId::new("req-1").unwrap(),
            AudioHash::new("hash-aaa").unwrap(),
        );
        assert_eq!(c.lookup(&k, 1000), None);
        c.store(k.clone(), "merhaba dünya", 1000);
        // Aynı ID + aynı ses → 24sa içinde ücretsiz vuruş.
        assert_eq!(c.lookup(&k, 2000), Some("merhaba dünya".into()));
    }

    #[test]
    fn same_id_different_audio_is_new_request() {
        let mut c = ResultCache::new();
        let k1 = IdempotencyKey::new(
            AccountId::new("ali").unwrap(),
            RequestId::new("req-1").unwrap(),
            AudioHash::new("hash-aaa").unwrap(),
        );
        c.store(k1, "ilk metin", 1000);
        let k2 = IdempotencyKey::new(
            AccountId::new("ali").unwrap(),
            RequestId::new("req-1").unwrap(),
            AudioHash::new("hash-BBB-farkli-ses").unwrap(),
        );
        // Farklı ses → önbellek dönülmez, normal ücretlenir.
        assert_eq!(c.lookup(&k2, 2000), None);
    }

    #[test]
    fn cache_scoped_per_account() {
        let mut c = ResultCache::new();
        let mk = |acc: &str| {
            IdempotencyKey::new(
                AccountId::new(acc).unwrap(),
                RequestId::new("req-9").unwrap(),
                AudioHash::new("h").unwrap(),
            )
        };
        c.store(mk("ali"), "ali'nin metni", 1000);
        assert_eq!(c.lookup(&mk("veli"), 2000), None);
    }

    #[test]
    fn cache_expires_after_24h_and_empty_never_cached() {
        let mut c = ResultCache::new();
        let k = IdempotencyKey::new(
            AccountId::new("ali").unwrap(),
            RequestId::new("req-2").unwrap(),
            AudioHash::new("h").unwrap(),
        );
        c.store(k.clone(), "", 1000); // boş transkript → yazılmaz
        assert_eq!(c.lookup(&k, 2000), None);
        c.store(k.clone(), "dolu", 1000);
        assert_eq!(c.lookup(&k, 1000 + RESULT_CACHE_TTL_SECS - 1), Some("dolu".into()));
        assert_eq!(c.lookup(&k, 1000 + RESULT_CACHE_TTL_SECS), None);
    }

    #[test]
    fn pipeline_order_is_locked() {
        assert_eq!(
            PIPELINE_ORDER,
            [
                PipelineStage::Idempotency,
                PipelineStage::Verify,
                PipelineStage::BalanceBlock,
                PipelineStage::Queue,
            ]
        );
    }

    #[test]
    fn tavan_180sn_ustu_faturalanmaz() {
        let t = tariff();
        // Kilitli tavan: 180sn üstü ölçü gelse bile faturalı süre 180'de kalır.
        assert_eq!(billable_seconds(200.0, Line::Home, &t), 180);
        assert_eq!(billable_seconds(200.0, Line::Fallback, &t), 180);
        assert_eq!(billable_seconds(180.0, Line::Home, &t), 180);
    }

    #[test]
    fn devasa_tarife_tasmaz_doyar() {
        let huge = Tariff {
            version: 7,
            home_kurus_per_min: u64::MAX,
            fallback_kurus_per_min: u64::MAX,
            upstream_min_secs: 10,
        };
        // Taşma paniği/sarması yerine doyum.
        assert_eq!(quote_kurus(180, Line::Home, &huge), u64::MAX / 60);
        assert_eq!(quote_kurus(3, Line::Home, &huge), u64::MAX / 60);
    }
}
