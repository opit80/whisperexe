//! Acil şalter + hat limitleri (PLAN §3).
//!
//! - Global şalter: panelden tüm fallback harcaması tek tuşla durdurulur.
//!   Şalter kapalıyken YENİ fallback istekleri "bakım" hatasıyla reddedilir
//!   (ücretsiz); DEVAM EDENLER bitirilip normal ücretlenir.
//! - Hesap bazında: fallback günlük tavanı, "sadece-ev" modu, günlük/aylık
//!   harcama tavanları (hat bazında).

use crate::accounts::{Account, AccountError};
use crate::tariffs::Line;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteError {
    /// Şalter kapalı / sadece-ev / tavan: ücretsiz ret, bakım hatası.
    Maintenance(&'static str),
    /// Günlük/aylık harcama tavanı: ücretsiz ret.
    Capped(&'static str),
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouteError::Maintenance(m) => write!(f, "bakim: {}", m),
            RouteError::Capped(m) => write!(f, "tavan: {}", m),
        }
    }
}

impl std::error::Error for RouteError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FallbackSwitch {
    /// true = fallback harcaması açık (normal); false = durduruldu.
    pub open: bool,
}

impl Default for FallbackSwitch {
    fn default() -> Self {
        Self { open: true }
    }
}

/// Harcama kaydı (limit kontrolleri için; panel ölçeğinde tutulur).
#[derive(Debug, Clone, Copy)]
pub struct SpendEntry {
    pub at: u64,
    pub line: Line,
    pub amount_krs: i64,
}

const DAY_SECS: u64 = 86400;
const MONTH_WINDOW_SECS: u64 = 30 * DAY_SECS; // haddelenmiş 30 gün penceresi

/// Yeni istek kabul kapısı: şalter + sadece-ev + tavan kontrolleri.
/// Başarılıysa `Ok(())`; ret her zaman ücretsizdir (ücret yazılmaz).
pub fn admit(
    acc: &Account,
    line: Line,
    cost_krs: i64,
    switch: &FallbackSwitch,
    spend: &[SpendEntry],
    now: u64,
) -> Result<(), RouteError> {
    if acc.suspended {
        return Err(RouteError::Maintenance("hesap durduruldu"));
    }
    if line == Line::Fallback {
        if !switch.open {
            return Err(RouteError::Maintenance("fallback bakimda"));
        }
        if acc.home_only {
            return Err(RouteError::Maintenance("hesap sadece-ev modunda"));
        }
        if let Some(cap) = acc.fallback_daily_cap_krs {
            let used: i64 = spend
                .iter()
                .filter(|e| e.line == Line::Fallback && now.saturating_sub(e.at) < DAY_SECS)
                .map(|e| e.amount_krs)
                .sum();
            if used + cost_krs > cap {
                return Err(RouteError::Capped("fallback gunluk tavan asildi"));
            }
        }
    }
    let (dcap, mcap) = match line {
        Line::Home => (acc.daily_cap_home_krs, acc.monthly_cap_home_krs),
        Line::Fallback => (acc.daily_cap_fb_krs, acc.monthly_cap_fb_krs),
    };
    if let Some(cap) = dcap {
        let used: i64 = spend
            .iter()
            .filter(|e| e.line == line && now.saturating_sub(e.at) < DAY_SECS)
            .map(|e| e.amount_krs)
            .sum();
        if used + cost_krs > cap {
            return Err(RouteError::Capped("gunluk harcama tavani asildi"));
        }
    }
    if let Some(cap) = mcap {
        let used: i64 = spend
            .iter()
            .filter(|e| e.line == line && now.saturating_sub(e.at) < MONTH_WINDOW_SECS)
            .map(|e| e.amount_krs)
            .sum();
        if used + cost_krs > cap {
            return Err(RouteError::Capped("aylik harcama tavani asildi"));
        }
    }
    Ok(())
}

impl From<AccountError> for RouteError {
    fn from(_: AccountError) -> Self {
        RouteError::Maintenance("hesap")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::AccountStore;

    fn acc_with(store: &mut AccountStore) -> Account {
        store.create_invite("K", "ali", 100_000, 0).unwrap();
        store.redeem_invite("K", "guclu-sifre-1", "H1", 1).unwrap();
        store.get("ali").unwrap().clone()
    }

    #[test]
    fn switch_closed_rejects_new_fallback_free_but_home_ok() {
        let mut s = AccountStore::default();
        let acc = acc_with(&mut s);
        let closed = FallbackSwitch { open: false };
        // Yeni fallback isteği bakım hatasıyla reddedilir (ücret yazılmaz).
        assert_eq!(
            admit(&acc, Line::Fallback, 500, &closed, &[], 100),
            Err(RouteError::Maintenance("fallback bakimda"))
        );
        // Ev hattı şalterden etkilenmez.
        assert!(admit(&acc, Line::Home, 500, &closed, &[], 100).is_ok());
        // Şalter açıkken fallback geçer.
        assert!(admit(&acc, Line::Fallback, 500, &FallbackSwitch::default(), &[], 100).is_ok());
    }

    #[test]
    fn caps_and_home_only() {
        let mut s = AccountStore::default();
        let mut acc = acc_with(&mut s);
        acc.home_only = true;
        assert!(admit(&acc, Line::Fallback, 10, &FallbackSwitch::default(), &[], 100).is_err());
        assert!(admit(&acc, Line::Home, 10, &FallbackSwitch::default(), &[], 100).is_ok());
        acc.home_only = false;
        acc.fallback_daily_cap_krs = Some(100);
        let spent = vec![SpendEntry { at: 90, line: Line::Fallback, amount_krs: 80 }];
        assert!(admit(&acc, Line::Fallback, 10, &FallbackSwitch::default(), &spent, 100).is_ok());
        assert_eq!(
            admit(&acc, Line::Fallback, 30, &FallbackSwitch::default(), &spent, 100),
            Err(RouteError::Capped("fallback gunluk tavan asildi"))
        );
        acc.daily_cap_home_krs = Some(50);
        let hspent = vec![SpendEntry { at: 90, line: Line::Home, amount_krs: 50 }];
        assert!(admit(&acc, Line::Home, 1, &FallbackSwitch::default(), &hspent, 100).is_err());
    }
}
