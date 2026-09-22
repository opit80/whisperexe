//! Kullanıcı tablosu satırları + HWID sıfırlama + düşük bakiye (PLAN §4).
//!
//! Satır: çevrimiçi durumu, toplam kullanım (ev/fallback ayrı), son aktiflik,
//! istemci sürümü, bakiye. İşlemler: bakiye ekle, limitler, fallback tavanı,
//! sadece-ev, HWID sıfırla (loglu + sık reset uyarısı), durdur, zorla-güncelle.

use crate::accounts::{Account, AccountError};
use crate::tariffs::{Line, Tariff};

/// Sık reset eşiği: 30 günde bu kadar reset uyarı üretir.
pub const RESET_WARN_COUNT: usize = 3;
pub const RESET_WINDOW_SECS: u64 = 30 * 86400;
/// Düşük bakiye eşiği: ~10 dakikalık hat karşılığı.
pub const LOW_BALANCE_MINUTES: i64 = 10;

#[derive(Debug, Clone)]
pub struct UserRow {
    pub username: String,
    pub online: bool,
    pub usage_home_secs: u64,
    pub usage_fb_secs: u64,
    pub last_active: u64,
    pub client_version: String,
    pub balance_krs: i64,
    pub suspended: bool,
    pub low_balance: bool,
    pub hwid_reset_warning: bool,
}

#[derive(Debug, Clone)]
pub struct HwidResetLog {
    pub entries: Vec<HwidResetEntry>,
}

#[derive(Debug, Clone)]
pub struct HwidResetEntry {
    pub username: String,
    pub at: u64,
    pub by_admin: String,
    pub slots_cleared: usize,
}

impl Default for HwidResetLog {
    fn default() -> Self {
        Self { entries: Vec::new() }
    }
}

/// HWID sıfırla: tüm cihaz slotları boşaltılır (kayıp/spoof/yenileme aynı buton).
/// İşlem loglanır; 30 günde 3+ reset uyarı bayrağı üretir.
pub fn reset_hwid(
    acc: &mut Account,
    admin: &str,
    log: &mut HwidResetLog,
    now: u64,
) -> HwidResetEntry {
    let cleared = acc.devices.len();
    acc.devices.clear();
    acc.hwid_resets.push(now);
    let e = HwidResetEntry {
        username: acc.username.clone(),
        at: now,
        by_admin: admin.to_string(),
        slots_cleared: cleared,
    };
    log.entries.push(e.clone());
    e
}

/// Sık reset uyarısı: pencerede RESET_WARN_COUNT ve üstü.
pub fn hwid_reset_warning(acc: &Account, now: u64) -> bool {
    acc.hwid_resets
        .iter()
        .filter(|t| now.saturating_sub(**t) < RESET_WINDOW_SECS)
        .count()
        >= RESET_WARN_COUNT
}

/// Düşük bakiye eşiği: o anki hattın tarifesiyle ~10 dakika.
pub fn low_balance_threshold_krs(tariff: &Tariff, line: Line) -> i64 {
    // ceil(rate * 10) kuruş (10 dakikalık karşılık).
    tariff.rate_krs_per_min(line) * LOW_BALANCE_MINUTES
}

pub fn is_low_balance(acc: &Account, tariff: &Tariff, line: Line) -> bool {
    acc.balance_krs < low_balance_threshold_krs(tariff, line)
}

pub fn user_row(acc: &Account, tariff: &Tariff, line_for_warning: Line, now: u64) -> UserRow {
    UserRow {
        username: acc.username.clone(),
        online: acc.online,
        usage_home_secs: acc.usage_home_secs,
        usage_fb_secs: acc.usage_fb_secs,
        last_active: acc.last_active,
        client_version: acc.client_version.clone(),
        balance_krs: acc.balance_krs,
        suspended: acc.suspended,
        low_balance: is_low_balance(acc, tariff, line_for_warning),
        hwid_reset_warning: hwid_reset_warning(acc, now),
    }
}

/// Bakiye ekle (pilot: havale sonrası admin elle yazar).
pub fn top_up(acc: &mut Account, amount_krs: i64) -> Result<i64, AccountError> {
    if amount_krs <= 0 {
        return Err(AccountError::BadOpeningBalance);
    }
    acc.balance_krs += amount_krs;
    Ok(acc.balance_krs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::AccountStore;
    use crate::tariffs::{FallbackPrice, TariffTable};

    fn mk() -> (AccountStore, TariffTable) {
        let mut s = AccountStore::default();
        s.create_invite("K", "ali", 500, 0).unwrap();
        s.redeem_invite("K", "guclu-sifre-1", "H1", 1).unwrap();
        (s, TariffTable::new(120, FallbackPrice::MultiplierBp(50000)))
    }

    #[test]
    fn hwid_reset_logged_and_warns_on_frequent() {
        let (mut s, t) = mk();
        let mut log = HwidResetLog::default();
        // Sık reset uyarısı: 30 gün içinde 3 reset.
        for now in [100, 200, 300] {
            let acc = s.get_mut("ali").unwrap();
            reset_hwid(acc, "admin", &mut log, now);
        }
        assert_eq!(log.entries.len(), 3);
        assert_eq!(log.entries[0].by_admin, "admin");
        assert_eq!(log.entries[0].slots_cleared, 1);
        let acc = s.get("ali").unwrap();
        assert!(acc.devices.is_empty());
        assert!(hwid_reset_warning(acc, 400));
        assert!(!hwid_reset_warning(acc, 300 + RESET_WINDOW_SECS + 1));
        let row = user_row(acc, &t.current(), Line::Home, 400);
        assert!(row.hwid_reset_warning);
    }

    #[test]
    fn low_balance_uses_current_line_tariff() {
        let (mut s, t) = mk();
        let cur = t.current();
        // Ev 120kr/dk → eşik 1200kr; bakiye 500 → düşük.
        let acc = s.get("ali").unwrap();
        assert!(is_low_balance(acc, &cur, Line::Home));
        // Fallback'teyken ev dakikasıyla yanıltılmaz: fb 600kr/dk → eşik 6000.
        assert!(is_low_balance(acc, &cur, Line::Fallback));
        s.get_mut("ali").unwrap().balance_krs = 50_000;
        let acc = s.get("ali").unwrap();
        assert!(!is_low_balance(acc, &cur, Line::Home));
        // Bakiye yükleme.
        let b = top_up(s.get_mut("ali").unwrap(), 1000).unwrap();
        assert_eq!(b, 51_000);
        assert!(top_up(s.get_mut("ali").unwrap(), 0).is_err());
    }
}
