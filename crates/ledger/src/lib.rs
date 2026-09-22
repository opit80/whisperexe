//! F1a TL defter: PLAN.md §3 kilitli kararlar.
//!
//! - Append-only satırlar: istek ID, ölçülen sn, hat, tarife sürümü,
//!   bloke / kesinleştirme (tasfiye) / iade (release), admin yükleme.
//! - Bakiye TÜRETİLMİŞ alandır; okuma-değiştirme-yazma YOKTUR.
//!   Geçmiş satır asla değiştirilmez/silinmez — yalnızca yeni satır eklenir.
//! - Bloke SENKRONDUR (bakiyeden düşmüş görünür). Tasfiye YALNIZCA
//!   bloke içinden düşer, asla üstüne çıkmaz.
//! - Başarısız işte ücret YOKTUR (bloke aynen iade edilir).
//!   Boş transkript başarısız sayılır.
//! - Crash'te tasfiyesiz bloke: yeniden başlatmada zaman aşımıyla çözülür
//!   ([`Ledger::expire_stale_blocks`]).
//! - Ölçü sunucunun gerçek saniyesinedir; quantum ev 3sn,
//!   fallback max(3sn, upstream minimumu).

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Fiyat/ölçü yardımcıları (protocol ile aynı kural; bu crate bağımsızdır)
// ---------------------------------------------------------------------------

/// Ev hattı minimum quantum: 3sn.
pub const QUANTUM_HOME_SECS: u32 = 3;
/// Tek istek tavanı: 180sn.
pub const MAX_SECONDS: u32 = 180;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    Home,
    Fallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tariff {
    pub version: u64,
    pub home_kurus_per_min: u64,
    pub fallback_kurus_per_min: u64,
    pub upstream_min_secs: u32,
}

pub fn billable_seconds(measured_secs: f64, line: Line, tariff: &Tariff) -> u32 {
    let measured = if measured_secs.is_finite() && measured_secs > 0.0 {
        measured_secs.ceil() as u32
    } else {
        0
    };
    let q = match line {
        Line::Home => QUANTUM_HOME_SECS,
        Line::Fallback => QUANTUM_HOME_SECS.max(tariff.upstream_min_secs),
    };
    measured.max(q).min(MAX_SECONDS)
}

pub fn quote_kurus(billable_secs: u32, line: Line, tariff: &Tariff) -> u64 {
    let per_min = match line {
        Line::Home => tariff.home_kurus_per_min,
        Line::Fallback => tariff.fallback_kurus_per_min,
    };
    // Doyumlu aritmetik: devasa tarife girdisinde taşma paniği/sarması
    // yerine u64::MAX'a doyar; bloke o zaman bakiyeye takılır (402, ücretsiz ret).
    (billable_secs as u64)
        .saturating_mul(per_min)
        .saturating_add(59)
        / 60
}

/// Minimum ön-kontrol tutarı (1 quantum): bakiye bunun altındaysa
/// inference başlamadan reddedilir, ücret yazılmaz.
pub fn quantum_cost_kurus(line: Line, tariff: &Tariff) -> u64 {
    let q = match line {
        Line::Home => QUANTUM_HOME_SECS,
        Line::Fallback => QUANTUM_HOME_SECS.max(tariff.upstream_min_secs),
    };
    quote_kurus(q, line, tariff)
}

// ---------------------------------------------------------------------------
// Defter
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Admin yüklemesi (+bakiye).
    Topup,
    /// Admin düşürmesi (−bakiye; eksiye düşürmez).
    Deduct,
    /// Senkron ön-bloke: ölçülen tutar bakiyeden düşmüş görünür (−bakiye).
    Block,
    /// Kesinleştirme (tasfiye): YALNIZCA bloke içinden düşer, delta 0'dır
    /// (para blokede zaten düşmüştü). `amount ≤ bloke − tasfiye − iade`.
    Settle,
    /// İade: bloke içinden bakiyeye dönüş (+bakiye).
    /// `amount ≤ bloke − tasfiye − iade`.
    Release,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub seq: u64,
    pub at_secs: u64,
    pub account: String,
    pub request_id: String,
    pub line: Line,
    pub tariff_version: u64,
    /// Sunucunun ölçtüğü gerçek ses saniyesi (ölçüde yetkili).
    pub measured_secs: f64,
    pub kind: Kind,
    /// Kuruş. Block/Settle/Release/Topup için ≥ 0.
    pub amount_kurus: u64,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerError {
    InsufficientBalance { have_kurus: i64, need_kurus: u64 },
    OverSettle { outstanding_kurus: u64, asked_kurus: u64 },
    OverRelease { outstanding_kurus: u64, asked_kurus: u64 },
    InvalidAmount,
    DoubleBlock { request_id: String },
}

impl LedgerError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InsufficientBalance { .. } => "insufficient_balance",
            Self::OverSettle { .. } => "over_settle",
            Self::OverRelease { .. } => "over_release",
            Self::InvalidAmount => "invalid_amount",
            Self::DoubleBlock { .. } => "double_block",
        }
    }
}

/// Append-only defter. `entries` dışarıdan değiştirilemez;
/// tek mutasyon yolu yeni satır ekleyen yöntemlerdir.
#[derive(Debug, Default)]
pub struct Ledger {
    entries: Vec<Entry>,
    /// İstek başına son hareket saati (stale-bloke zaman aşımı için).
    last_touch: HashMap<(String, String), u64>,
}

impl Ledger {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&mut self, e: Entry) {
        let key = (e.account.clone(), e.request_id.clone());
        self.last_touch.insert(key, e.at_secs);
        self.entries.push(e);
    }

    fn next_seq(&self) -> u64 {
        self.entries.len() as u64
    }

    /// Türetilmiş bakiye (kuruş): Topup − Block + Release toplamı.
    /// Settle delta 0'dır. Okuma-değiştirme-yazma YOKTUR.
    pub fn balance(&self, account: &str) -> i64 {
        self.entries
            .iter()
            .filter(|e| e.account == account)
            .map(|e| match e.kind {
                Kind::Topup => e.amount_kurus as i64,
                Kind::Deduct => -(e.amount_kurus as i64),
                Kind::Block => -(e.amount_kurus as i64),
                Kind::Release => e.amount_kurus as i64,
                Kind::Settle => 0,
            })
            .sum()
    }

    /// İsteğin açık (bloke edilmiş ama tasfiye/iade edilmemiş) tutarı.
    pub fn outstanding(&self, account: &str, request_id: &str) -> u64 {
        let (mut b, mut s, mut r) = (0u64, 0u64, 0u64);
        for e in self.entries.iter().filter(|e| e.account == account && e.request_id == request_id)
        {
            match e.kind {
                Kind::Block => b += e.amount_kurus,
                Kind::Settle => s += e.amount_kurus,
                Kind::Release => r += e.amount_kurus,
                Kind::Topup | Kind::Deduct => {}
            }
        }
        b.saturating_sub(s).saturating_sub(r)
    }

    /// Tasfiyesi yazılmış toplam (çift-tasfiye/idempotency guard'ı için).
    pub fn settled_total(&self, account: &str, request_id: &str) -> u64 {
        self.entries
            .iter()
            .filter(|e| {
                e.account == account && e.request_id == request_id && e.kind == Kind::Settle
            })
            .map(|e| e.amount_kurus)
            .sum()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    // -- yazmalar --

    /// Admin yüklemesi (havale sonrası panelden).
    pub fn topup(
        &mut self,
        at_secs: u64,
        account: &str,
        amount_kurus: u64,
        note: &str,
    ) -> Result<u64, LedgerError> {
        if amount_kurus == 0 {
            return Err(LedgerError::InvalidAmount);
        }
        let seq = self.next_seq();
        self.push(Entry {
            seq,
            at_secs,
            account: account.into(),
            request_id: String::new(),
            line: Line::Home,
            tariff_version: 0,
            measured_secs: 0.0,
            kind: Kind::Topup,
            amount_kurus,
            note: note.into(),
        });
        Ok(seq)
    }

    /// Admin düşürmesi: bakiye eksiye düşmez, yetersizse ret (ücret satırı YOK,
    /// yalnızca başarılı düşürme yeni `Deduct` satırı yazar).
    pub fn deduct(
        &mut self,
        at_secs: u64,
        account: &str,
        amount_kurus: u64,
        note: &str,
    ) -> Result<u64, LedgerError> {
        if amount_kurus == 0 {
            return Err(LedgerError::InvalidAmount);
        }
        let have = self.balance(account);
        if have < amount_kurus as i64 {
            return Err(LedgerError::InsufficientBalance {
                have_kurus: have,
                need_kurus: amount_kurus,
            });
        }
        let seq = self.next_seq();
        self.push(Entry {
            seq,
            at_secs,
            account: account.into(),
            request_id: String::new(),
            line: Line::Home,
            tariff_version: 0,
            measured_secs: 0.0,
            kind: Kind::Deduct,
            amount_kurus,
            note: note.into(),
        });
        Ok(seq)
    }

    /// Minimum ön-kontrol: bakiye ≥ 1 quantum mu? (gövde kabulünden önce)
    pub fn precheck(&self, account: &str, line: Line, tariff: &Tariff) -> Result<(), LedgerError> {
        let need = quantum_cost_kurus(line, tariff);
        let have = self.balance(account);
        if have < need as i64 {
            return Err(LedgerError::InsufficientBalance {
                have_kurus: have,
                need_kurus: need,
            });
        }
        Ok(())
    }

    /// Senkron bloke: decode + süre ölçümünden sonra, işlemden önce.
    /// Bakiye yetmezse inference başlamadan reddedilir, ücret yazılmaz.
    /// Aynı isteğe ikinci bloke YOKTUR (failover yeni satırla taşınır,
    /// tekrar deneme önce iade ister — çift-bloke defteri bozmaz).
    pub fn block(
        &mut self,
        at_secs: u64,
        account: &str,
        request_id: &str,
        measured_secs: f64,
        line: Line,
        tariff: &Tariff,
    ) -> Result<u64, LedgerError> {
        let amount = quote_kurus(billable_seconds(measured_secs, line, tariff), line, tariff);
        if amount == 0 {
            return Err(LedgerError::InvalidAmount);
        }
        if self.outstanding(account, request_id) > 0 {
            return Err(LedgerError::DoubleBlock {
                request_id: request_id.into(),
            });
        }
        let have = self.balance(account);
        if have < amount as i64 {
            return Err(LedgerError::InsufficientBalance {
                have_kurus: have,
                need_kurus: amount,
            });
        }
        let seq = self.next_seq();
        self.push(Entry {
            seq,
            at_secs,
            account: account.into(),
            request_id: request_id.into(),
            line,
            tariff_version: tariff.version,
            measured_secs,
            kind: Kind::Block,
            amount_kurus: amount,
            note: format!("bloke v{}", tariff.version),
        });
        Ok(amount)
    }

    /// Kesinleştirme: yalnızca bloke içinden düşer, üstüne çıkamaz.
    /// Tasfiyesi yazılmamış bitmiş iş bile sonraki ön-kontrolü delmez
    /// (para blokede zaten düşmüştür).
    pub fn settle(
        &mut self,
        at_secs: u64,
        account: &str,
        request_id: &str,
        amount_kurus: u64,
    ) -> Result<u64, LedgerError> {
        let out = self.outstanding(account, request_id);
        if amount_kurus > out {
            return Err(LedgerError::OverSettle {
                outstanding_kurus: out,
                asked_kurus: amount_kurus,
            });
        }
        let prev = self.prev_line(account, request_id);
        let seq = self.next_seq();
        self.push(Entry {
            seq,
            at_secs,
            account: account.into(),
            request_id: request_id.into(),
            line: prev.0,
            tariff_version: prev.1,
            measured_secs: prev.2,
            kind: Kind::Settle,
            amount_kurus,
            note: "tasfiye".into(),
        });
        Ok(seq)
    }

    /// İade: kuyrukta vazgeçme, failover taşıma, BAŞARISIZ iş
    /// (boş transkript dahil) → ücret YOK, bloke aynen döner.
    pub fn release(
        &mut self,
        at_secs: u64,
        account: &str,
        request_id: &str,
        amount_kurus: u64,
        note: &str,
    ) -> Result<u64, LedgerError> {
        let out = self.outstanding(account, request_id);
        if amount_kurus == 0 || amount_kurus > out {
            return Err(LedgerError::OverRelease {
                outstanding_kurus: out,
                asked_kurus: amount_kurus,
            });
        }
        let prev = self.prev_line(account, request_id);
        let seq = self.next_seq();
        self.push(Entry {
            seq,
            at_secs,
            account: account.into(),
            request_id: request_id.into(),
            line: prev.0,
            tariff_version: prev.1,
            measured_secs: prev.2,
            kind: Kind::Release,
            amount_kurus,
            note: note.into(),
        });
        Ok(seq)
    }

    /// Başarısız iş kolaylığı: açık blokenin TAMAMI iade edilir,
    /// ücret yazılmaz. Açık bloke yoksa no-op (Ok(0) satır yazılmaz).
    pub fn fail_request(
        &mut self,
        at_secs: u64,
        account: &str,
        request_id: &str,
        note: &str,
    ) -> Result<u64, LedgerError> {
        let out = self.outstanding(account, request_id);
        if out == 0 {
            return Ok(0);
        }
        self.release(at_secs, account, request_id, out, note)
    }

    /// Crash kurtarma: tasfiyesiz/iadesiz kalmış bloke, `timeout_secs`
    /// hareketsizlikten sonra zaman aşımıyla iade edilir (yeni satır).
    /// Dönen: çözülen istek sayısı.
    pub fn expire_stale_blocks(&mut self, now_secs: u64, timeout_secs: u64) -> usize {
        let stale: Vec<(String, String, u64)> = self
            .last_touch
            .iter()
            .filter(|(_, t)| now_secs.saturating_sub(**t) >= timeout_secs)
            .map(|(k, _)| (k.0.clone(), k.1.clone(), 0u64))
            .collect();
        let mut n = 0;
        for (acc, req, _) in stale {
            if self.outstanding(&acc, &req) > 0 {
                let out = self.outstanding(&acc, &req);
                // release borrow'ları bitirir; hata olamaz (out > 0 taze okundu,
                // tek-iş parçacıklı append sırasında değişmez).
                if self.release(now_secs, &acc, &req, out, "stale-bloke zamanasimi").is_ok() {
                    n += 1;
                }
            }
        }
        n
    }

    fn prev_line(&self, account: &str, request_id: &str) -> (Line, u64, f64) {
        for e in self.entries.iter().rev() {
            if e.account == account && e.request_id == request_id {
                return (e.line, e.tariff_version, e.measured_secs);
            }
        }
        (Line::Home, 0, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tariff() -> Tariff {
        Tariff {
            version: 7,
            home_kurus_per_min: 60,      // 1 kr/sn
            fallback_kurus_per_min: 600, // 10 kr/sn
            upstream_min_secs: 10,       // Groq minimumu
        }
    }

    fn funded() -> Ledger {
        let mut l = Ledger::new();
        l.topup(0, "ali", 10_000, "acilis").unwrap();
        l
    }

    #[test]
    fn balance_is_derived_no_rmw() {
        let mut l = funded();
        assert_eq!(l.balance("ali"), 10_000);
        // 5sn ev sesi: quantum 3 → 5sn → 5 kr bloke.
        let b = l.block(10, "ali", "req-1", 5.0, Line::Home, &tariff()).unwrap();
        assert_eq!(b, 5);
        assert_eq!(l.balance("ali"), 9_995); // bloke düşmüş görünür
        l.settle(20, "ali", "req-1", 5).unwrap();
        assert_eq!(l.balance("ali"), 9_995); // tasfiye delta 0
    }

    #[test]
    fn settle_never_exceeds_block() {
        let mut l = funded();
        l.block(10, "ali", "req-1", 5.0, Line::Home, &tariff()).unwrap(); // 5 kr
        // Bloke 5 iken 6 tasfiye EDİLEMEZ.
        assert_eq!(
            l.settle(20, "ali", "req-1", 6),
            Err(LedgerError::OverSettle {
                outstanding_kurus: 5,
                asked_kurus: 6
            })
        );
        // Kısmi tasfiye + kalan iade geçerlidir.
        l.settle(20, "ali", "req-1", 3).unwrap();
        l.release(21, "ali", "req-1", 2, "duzeltme").unwrap();
        assert_eq!(l.balance("ali"), 9_997);
    }

    #[test]
    fn idempotent_retry_is_free_and_recharges_once() {
        let mut l = funded();
        // Başarılı ilk deneme: bloke 5 + tasfiye 5.
        l.block(10, "ali", "req-1", 5.0, Line::Home, &tariff()).unwrap();
        l.settle(20, "ali", "req-1", 5).unwrap();
        assert_eq!(l.balance("ali"), 9_995);
        // Aynı isteğin tekrar tasfiyesi REDDEDİLİR (çift ücret yok).
        assert_eq!(
            l.settle(30, "ali", "req-1", 5),
            Err(LedgerError::OverSettle {
                outstanding_kurus: 0,
                asked_kurus: 5
            })
        );
        assert_eq!(l.balance("ali"), 9_995);

        // Başarısız deneme: bloke → tam iade → bakiye aynı.
        l.block(40, "ali", "req-2", 8.0, Line::Home, &tariff()).unwrap();
        assert_eq!(l.balance("ali"), 9_987);
        l.fail_request(50, "ali", "req-2", "bos transkript").unwrap();
        assert_eq!(l.balance("ali"), 9_995);
        // Yeniden deneme BAŞARILI olursa o başarı normal ücretlenir.
        l.block(60, "ali", "req-2", 8.0, Line::Home, &tariff()).unwrap();
        l.settle(70, "ali", "req-2", 8).unwrap();
        assert_eq!(l.balance("ali"), 9_987);
    }

    #[test]
    fn insufficient_balance_rejects_before_inference_no_fee() {
        let mut l = Ledger::new();
        l.topup(0, "ali", 2, "kucuk").unwrap();
        // Ön-kontrol: 1 quantum (3sn ev = 3kr) bile yok → ret.
        assert!(l.precheck("ali", Line::Home, &tariff()).is_err());
        let n_before = l.entries().len();
        assert!(l.block(10, "ali", "req-1", 5.0, Line::Home, &tariff()).is_err());
        // Ücret satırı YAZILMADI (yalnızca topup durur).
        assert_eq!(l.entries().len(), n_before);
        assert_eq!(l.balance("ali"), 2);
    }

    #[test]
    fn stale_block_expires_after_crash() {
        let mut l = funded();
        l.block(100, "ali", "req-9", 60.0, Line::Home, &tariff()).unwrap();
        let held = l.balance("ali");
        assert!(held < 10_000);
        // Yeniden başlatma: 1 saat hareketsiz bloke zaman aşımına uğrar.
        assert_eq!(l.expire_stale_blocks(100 + 3600, 3600), 1);
        assert_eq!(l.balance("ali"), 10_000);
        // İkinci çalıştırma aynı bloğu tekrar çözmez.
        assert_eq!(l.expire_stale_blocks(100 + 7200, 3600), 0);
    }

    #[test]
    fn queue_cancel_releases_block_free() {
        let mut l = funded();
        l.block(10, "ali", "req-1", 30.0, Line::Home, &tariff()).unwrap();
        l.release(11, "ali", "req-1", 30, "kuyrukta vazgecti").unwrap();
        assert_eq!(l.balance("ali"), 10_000);
    }

    #[test]
    fn failover_moves_block_without_double_charge() {
        let mut l = funded();
        // Evde bloke edildi, 3sn'de bağlanamadı → bloke çözülür...
        let b1 = l.block(10, "ali", "req-1", 5.0, Line::Home, &tariff()).unwrap();
        l.release(13, "ali", "req-1", b1, "failover: ev baglanamadi").unwrap();
        // ...aynı ID fallback hattında YENİDEN bloke edilir (iki satır).
        let b2 = l.block(14, "ali", "req-1", 5.0, Line::Fallback, &tariff()).unwrap();
        assert_eq!(b2, 100); // fallback 10sn quantum × 10kr/sn
        l.settle(30, "ali", "req-1", b2).unwrap();
        assert_eq!(l.balance("ali"), 10_000 - 100); // tek ücret
    }

    #[test]
    fn devasa_tarife_tasmaz_doyar_bloke_402_doner() {
        // Admin yanlışlıkla devasa tarife girerse (u64::MAX): eski kodda
        // `180 * per_min` u64 taşması debug'da panik, release'de sarmaydı.
        let huge = Tariff {
            version: 7,
            home_kurus_per_min: u64::MAX,
            fallback_kurus_per_min: u64::MAX,
            upstream_min_secs: 10,
        };
        assert_eq!(quote_kurus(180, Line::Home, &huge), u64::MAX / 60);
        let mut l = funded();
        let n_before = l.entries().len();
        // Tutar bakiyeyi aşar: ücretsiz ret, panik yok, satır yazılmaz.
        assert!(matches!(
            l.block(10, "ali", "req-huge", 5.0, Line::Home, &huge),
            Err(LedgerError::InsufficientBalance { .. })
        ));
        assert_eq!(l.entries().len(), n_before);
        assert_eq!(l.balance("ali"), 10_000);
    }
}
