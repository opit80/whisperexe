//! Tarifeler (PLAN §3 fiyat + §4: ev TL/dk, fallback çarpan/sabit panelden
//! dinamik; yeni fiyat sonraki isteklere uygulanır).
//!
//! Para birimi: kuruş (i64). Ölçü: evde minimum quantum 3sn; fallback hattında
//! `max(3sn, upstream minimumu)` (Groq'ta 10sn). Ücret = tavana yuvarlanmış
//! `tarife * faturalı_sn / 60`. Tarife kabul anında dondurulur: bloke edilen
//! sürümle kesinleşir (sürüm numarası teklifte taşınır).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Line {
    Home,
    Fallback,
}

pub const HOME_QUANTUM_SECS: u64 = 3;
pub const FALLBACK_QUANTUM_FLOOR_SECS: u64 = 3;
/// Varsayılan upstream minimumu (Groq 10sn).
pub const DEFAULT_UPSTREAM_MIN_SECS: u64 = 10;
/// OpenAI hattı tabanı (minimumsuz -> ev tabanı 3sn işler).
pub const OPENAI_UPSTREAM_MIN_SECS: u64 = 3;
/// Local hat tabanı (ücretsiz ev modeli; 3sn quantum).
pub const LOCAL_UPSTREAM_MIN_SECS: u64 = 3;
/// Local hat fiyatı: sabit 0 kuruş/dk (ücretsiz, anahtarsız).
pub const LOCAL_FIXED_KRS_PER_MIN: i64 = 0;

/// Fallback sağlayıcı seçimi (panelden dinamik; varsayılan Groq).
/// Karar: `docs/FALLBACK.md` §1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FallbackVendor {
    Groq,
    OpenAi,
    /// Ücretsiz ev/local model (anahtar gerekmez, sabit 0 kuruş/dk).
    Local,
}

impl FallbackVendor {
    pub fn name(&self) -> &'static str {
        match self {
            FallbackVendor::Groq => "groq",
            FallbackVendor::OpenAi => "openai",
            FallbackVendor::Local => "local",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "groq" | "groq-turbo" | "whisper-large-v3-turbo" => Some(FallbackVendor::Groq),
            "openai" | "whisper-1" | "gpt-4o-mini-transcribe" => Some(FallbackVendor::OpenAi),
            "local" | "ev-local" | "ev" => Some(FallbackVendor::Local),
            _ => None,
        }
    }

    /// Hattın upstream tabanı (Groq 10sn, OpenAI/Local 3sn).
    pub fn default_upstream_min(&self) -> u64 {
        match self {
            FallbackVendor::Groq => DEFAULT_UPSTREAM_MIN_SECS,
            FallbackVendor::OpenAi => OPENAI_UPSTREAM_MIN_SECS,
            FallbackVendor::Local => LOCAL_UPSTREAM_MIN_SECS,
        }
    }

    /// Local hat sır gerektirmez (ücretsiz ev modeli).
    pub fn needs_key(&self) -> bool {
        !matches!(self, FallbackVendor::Local)
    }
}

/// Hat-bazlı saklanan fallback yapılandırması (aktif + pasif hat).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorCfg {
    pub price: FallbackPrice,
    pub upstream_min_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FallbackPrice {
    /// Ev tarifesinin baz puan cinsinden çarpanı (örn. 50000 = 5x).
    MultiplierBp(u64),
    /// Sabit fallback tarifesi (kuruş/dk).
    FixedKrsPerMin(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tariff {
    pub version: u64,
    pub home_krs_per_min: i64,
    pub fallback: FallbackPrice,
    pub upstream_min_secs: u64,
}

impl Tariff {
    pub fn fallback_krs_per_min(&self) -> i64 {
        match self.fallback {
            FallbackPrice::FixedKrsPerMin(v) => v,
            FallbackPrice::MultiplierBp(bp) => {
                ((self.home_krs_per_min as i128 * bp as i128 + 9999) / 10000) as i64
            }
        }
    }

    pub fn rate_krs_per_min(&self, line: Line) -> i64 {
        match line {
            Line::Home => self.home_krs_per_min,
            Line::Fallback => self.fallback_krs_per_min(),
        }
    }

    pub fn quantum_secs(&self, line: Line) -> u64 {
        match line {
            Line::Home => HOME_QUANTUM_SECS,
            Line::Fallback => FALLBACK_QUANTUM_FLOOR_SECS.max(self.upstream_min_secs),
        }
    }

    /// Kabul-anı teklifi: faturalı süre + tutar + dondurulan tarife sürümü.
    pub fn quote(&self, line: Line, measured_secs: u64) -> Quote {
        let billable = measured_secs.max(self.quantum_secs(line));
        let rate = self.rate_krs_per_min(line);
        // Tavana yuvarla: ceil(rate * billable / 60).
        let cost = ((rate as i128 * billable as i128 + 59) / 60) as i64;
        Quote { line, measured_secs, billable_secs: billable, cost_krs: cost.max(0), tariff_version: self.version }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quote {
    pub line: Line,
    pub measured_secs: u64,
    pub billable_secs: u64,
    pub cost_krs: i64,
    pub tariff_version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TariffTable {
    current: Tariff,
    vendor: FallbackVendor,
    groq: VendorCfg,
    openai: VendorCfg,
    #[serde(default = "default_local_cfg")]
    local: VendorCfg,
}

fn default_local_cfg() -> VendorCfg {
    VendorCfg {
        price: FallbackPrice::FixedKrsPerMin(LOCAL_FIXED_KRS_PER_MIN),
        upstream_min_secs: LOCAL_UPSTREAM_MIN_SECS,
    }
}

impl TariffTable {
    pub fn new(home_krs_per_min: i64, fallback: FallbackPrice) -> Self {
        let groq = VendorCfg { price: fallback, upstream_min_secs: DEFAULT_UPSTREAM_MIN_SECS };
        let openai = VendorCfg { price: fallback, upstream_min_secs: OPENAI_UPSTREAM_MIN_SECS };
        Self {
            current: Tariff {
                version: 1,
                home_krs_per_min,
                fallback: groq.price,
                upstream_min_secs: groq.upstream_min_secs,
            },
            vendor: FallbackVendor::Groq,
            groq,
            openai,
            local: default_local_cfg(),
        }
    }

    pub fn current(&self) -> Tariff {
        self.current
    }

    pub fn vendor(&self) -> FallbackVendor {
        self.vendor
    }

    pub fn vendor_cfg(&self, vendor: FallbackVendor) -> VendorCfg {
        match vendor {
            FallbackVendor::Groq => self.groq,
            FallbackVendor::OpenAi => self.openai,
            FallbackVendor::Local => self.local,
        }
    }

    fn apply(&mut self) -> Tariff {
        let cfg = self.vendor_cfg(self.vendor);
        self.current = Tariff {
            version: self.current.version + 1,
            home_krs_per_min: self.current.home_krs_per_min,
            fallback: cfg.price,
            upstream_min_secs: cfg.upstream_min_secs,
        };
        self.current
    }

    /// Panelden dinamik değişim: sürüm artar, yeni fiyat yalnızca
    /// sonraki isteklere uygulanır (devam edenler eski sürümle kesinleşir).
    pub fn set_tariff(&mut self, home_krs_per_min: i64, fallback: FallbackPrice) -> Tariff {
        // Eski davranış korunur: aktif hattın fiyatı değişir (tek sürüm artışı).
        // Local hat ücretsizdir; aktifken fiyat yazımı Fixed(0) ile sınırlanır.
        let fb = match self.vendor {
            FallbackVendor::Local => FallbackPrice::FixedKrsPerMin(LOCAL_FIXED_KRS_PER_MIN),
            _ => fallback,
        };
        match self.vendor {
            FallbackVendor::Groq => self.groq.price = fb,
            FallbackVendor::OpenAi => self.openai.price = fb,
            FallbackVendor::Local => {}
        }
        self.current.home_krs_per_min = home_krs_per_min;
        self.apply()
    }

    pub fn set_upstream_min(&mut self, secs: u64) -> Tariff {
        self.set_vendor_upstream_min(self.vendor, secs)
    }

    /// Aktif fallback hattını değiştirir (Groq varsayılan). Sürüm artar;
    /// iki hattın fiyat/taban ayarları ayrı saklanır, kaybolmaz.
    pub fn set_vendor(&mut self, vendor: FallbackVendor) -> Tariff {
        self.vendor = vendor;
        self.apply()
    }

    /// Belirli hattın fiyatını ayarlar (aktif hat ise hemen, değilse
    /// hat seçilince uygulanır). Her değişim sürümü artırır.
    /// Local hat ücretsizdir: fiyatı her zaman Fixed(0) kalır.
    pub fn set_vendor_price(&mut self, vendor: FallbackVendor, price: FallbackPrice) -> Tariff {
        match vendor {
            FallbackVendor::Groq => self.groq.price = price,
            FallbackVendor::OpenAi => self.openai.price = price,
            FallbackVendor::Local => {
                self.local.price = FallbackPrice::FixedKrsPerMin(LOCAL_FIXED_KRS_PER_MIN);
            }
        }
        self.apply()
    }

    /// Belirli hattın upstream tabanını ayarlar (3sn altına kelepçelenir).
    pub fn set_vendor_upstream_min(&mut self, vendor: FallbackVendor, secs: u64) -> Tariff {
        let v = secs.max(FALLBACK_QUANTUM_FLOOR_SECS);
        match vendor {
            FallbackVendor::Groq => self.groq.upstream_min_secs = v,
            FallbackVendor::OpenAi => self.openai.upstream_min_secs = v,
            FallbackVendor::Local => self.local.upstream_min_secs = v,
        }
        self.apply()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_quantum_and_ceil() {
        let t = TariffTable::new(120, FallbackPrice::MultiplierBp(50000)); // ev 1.20TL/dk, fb 5x
        assert_eq!(t.current().fallback_krs_per_min(), 600);
        // 1 sn evde → 3sn quantum: ceil(120*3/60)=6 kuruş.
        let q = t.current().quote(Line::Home, 1);
        assert_eq!((q.billable_secs, q.cost_krs), (3, 6));
        // 90sn evde: 120*90/60=180.
        let q = t.current().quote(Line::Home, 90);
        assert_eq!(q.cost_krs, 180);
        // Fallback 5sn → upstream min 10sn: ceil(600*10/60)=100.
        let q = t.current().quote(Line::Fallback, 5);
        assert_eq!((q.billable_secs, q.cost_krs), (10, 100));
    }

    #[test]
    fn tariff_change_applies_to_next_requests_only() {
        let mut t = TariffTable::new(120, FallbackPrice::MultiplierBp(50000));
        let before = t.current().quote(Line::Home, 60);
        assert_eq!(before.tariff_version, 1);
        assert_eq!(before.cost_krs, 120);
        // Panelden fiyat değişir.
        let nt = t.set_tariff(240, FallbackPrice::FixedKrsPerMin(900));
        assert_eq!(nt.version, 2);
        // Sonraki istek yeni fiyattan…
        let after = t.current().quote(Line::Home, 60);
        assert_eq!((after.tariff_version, after.cost_krs), (2, 240));
        let fb = t.current().quote(Line::Fallback, 60);
        assert_eq!(fb.cost_krs, 900);
        // …ama eski teklif eski sürümü taşır (bloke o sürümle kesinleşir).
        assert_eq!(before.tariff_version, 1);
        assert_eq!(before.cost_krs, 120);
    }

    #[test]
    fn vendor_default_groq_and_switch_keeps_per_vendor_cfg() {
        let mut t = TariffTable::new(120, FallbackPrice::MultiplierBp(50000));
        assert_eq!(t.vendor(), FallbackVendor::Groq);
        // Groq: 5sn -> 10sn taban, 100 kuruş.
        let q = t.current().quote(Line::Fallback, 5);
        assert_eq!((q.billable_secs, q.cost_krs), (10, 100));
        // OpenAI hattına geç: aynı fiyat ama 3sn taban (5sn -> 5sn, 50 kuruş).
        let nt = t.set_vendor(FallbackVendor::OpenAi);
        assert_eq!(nt.version, 2);
        assert_eq!(t.vendor(), FallbackVendor::OpenAi);
        let q = t.current().quote(Line::Fallback, 5);
        assert_eq!((q.billable_secs, q.cost_krs), (5, 50));
        // Groq'a dön: ayarları kaybolmadı.
        t.set_vendor(FallbackVendor::Groq);
        let q = t.current().quote(Line::Fallback, 5);
        assert_eq!((q.billable_secs, q.cost_krs), (10, 100));
    }

    #[test]
    fn vendor_price_and_upstream_set_independently() {
        let mut t = TariffTable::new(120, FallbackPrice::MultiplierBp(50000));
        // Pasif hattın fiyatı önden ayarlanabilir.
        t.set_vendor_price(FallbackVendor::OpenAi, FallbackPrice::FixedKrsPerMin(300));
        t.set_vendor(FallbackVendor::OpenAi);
        let q = t.current().quote(Line::Fallback, 60);
        assert_eq!(q.cost_krs, 300);
        // Taban 3sn altına kelepçelenir.
        t.set_vendor_upstream_min(FallbackVendor::OpenAi, 1);
        assert_eq!(t.current().upstream_min_secs, 3);
        // Aktif hattın eski set_tariff davranışı korunur (tek sürüm artışı).
        let v0 = t.current().version;
        let nt = t.set_tariff(120, FallbackPrice::FixedKrsPerMin(400));
        assert_eq!(nt.version, v0 + 1);
        assert_eq!(t.current().quote(Line::Fallback, 60).cost_krs, 400);
    }

    #[test]
    fn vendor_parse_names() {
        assert_eq!(FallbackVendor::parse("groq"), Some(FallbackVendor::Groq));
        assert_eq!(FallbackVendor::parse("whisper-1"), Some(FallbackVendor::OpenAi));
        assert_eq!(FallbackVendor::parse("gpt-4o-mini-transcribe"), Some(FallbackVendor::OpenAi));
        assert_eq!(FallbackVendor::parse("local"), Some(FallbackVendor::Local));
        assert_eq!(FallbackVendor::parse("bilinmez"), None);
    }

    #[test]
    fn local_vendor_free_no_key_no_upstream_floor() {
        // Local hat: sabit 0 kuruş/dk (ücretsiz), anahtar gerekmez, taban 3sn.
        assert!(!FallbackVendor::Local.needs_key());
        assert!(FallbackVendor::Groq.needs_key());
        let mut t = TariffTable::new(120, FallbackPrice::MultiplierBp(50000));
        t.set_vendor(FallbackVendor::Local);
        assert_eq!(t.vendor(), FallbackVendor::Local);
        // 60sn bile 0 kuruş; taban 3sn.
        let q = t.current().quote(Line::Fallback, 60);
        assert_eq!((q.billable_secs, q.cost_krs), (60, 0));
        let q = t.current().quote(Line::Fallback, 1);
        assert_eq!((q.billable_secs, q.cost_krs), (3, 0));
        // Fiyat yazımı local'i 0'da tutar (ücretli yapılamaz).
        t.set_vendor_price(FallbackVendor::Local, FallbackPrice::FixedKrsPerMin(900));
        let q = t.current().quote(Line::Fallback, 60);
        assert_eq!(q.cost_krs, 0);
        // Groq/OpenAI akışı bozulmadı: geri dönünce eski fiyat durur.
        t.set_vendor(FallbackVendor::Groq);
        let q = t.current().quote(Line::Fallback, 60);
        assert_eq!(q.cost_krs, 600);
    }
}
