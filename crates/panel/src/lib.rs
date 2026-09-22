//! Panel çekirdeği (F1c): hesap/davet, tarife, limit/şalter, kullanıcı
//! tablosu, HWID, saklama/disk, admin auth, sürüm beslemesi.
//!
//! Broker bu `PanelState`'i kilit altında tutar; HTTP uçları (`api::ROUTES`)
//! bu yöntemlere bağlanır. Davranış kilitleri PLAN §3–§4'ten gelir:
//! davet tek kullanımlık + 7 gün; tarife değişimi sonraki isteklere; şalter
//! kapalıyken yeniler ücretsiz bakım reti, devam edenler bitirilip ücretlenir.

pub mod accounts;
pub mod admin;
pub mod api;
pub mod feed;
pub mod retention;
pub mod routing;
pub mod tariffs;
pub mod users;

use accounts::{AccountError, AccountStore};
use admin::AdminAuth;
use api::{AuditLog, ROUTES};
use feed::{FeedState, Release, UpdateCheck};
use retention::{DiskKind, DiskMonitor, DiskStatus, HardDeleteReport};
use routing::{admit, FallbackSwitch, RouteError, SpendEntry};
use serde::{Deserialize, Serialize};
use tariffs::{FallbackPrice, FallbackVendor, Line, Quote, Tariff, TariffTable};
use users::{HwidResetEntry, HwidResetLog, UserRow};

/// Tarife tablosu hiç kurulmamışken kullanılan yedek varsayılanlar
/// (broker kurulumuyla aynı: ev 120 kuruş/dk, fallback 5x).
const DEFAULT_HOME_KRS: i64 = 120;
const DEFAULT_FB_BP: u64 = 50000;

fn default_table() -> TariffTable {
    TariffTable::new(DEFAULT_HOME_KRS, FallbackPrice::MultiplierBp(DEFAULT_FB_BP))
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PanelState {
    pub accounts: AccountStore,
    pub admin: AdminAuth,
    pub tariffs: Option<TariffTable>,
    pub switch: FallbackSwitch,
    pub spend: Vec<SpendEntry>,
    pub hwid_log: HwidResetLog,
    pub audit: AuditLog,
    pub feed: Option<FeedState>,
    pub disks: DiskMonitor,
    /// Sağlayıcı API anahtarları: YALNIZCA bellek-içi. Hiçbir yanıta,
    /// denetim detayına, deftere ya da dosyaya yazılmaz. Yeniden başlatmada
    /// silinir (panelden yeniden girilir); boşsa ortam değişkenine düşülür.
    /// Anlık görüntüye de yazılmaz ([`serde(skip)`]).
    #[serde(skip)]
    pub secrets: ProviderSecrets,
}

/// Bellek-içi sağlayıcı anahtar kasası (değerler dışarı sızmaz).
#[derive(Debug, Default, Clone)]
pub struct ProviderSecrets {
    groq_key: Option<String>,
    openai_key: Option<String>,
}

impl ProviderSecrets {
    /// Yalnızca VAR/YOK bilgisi dışarı verilir, değer asla.
    pub fn has(&self, vendor: FallbackVendor) -> bool {
        match vendor {
            FallbackVendor::Groq => self.groq_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false),
            FallbackVendor::OpenAi => self.openai_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false),
        }
    }

    fn set(&mut self, vendor: FallbackVendor, key: &str) {
        let v = if key.is_empty() { None } else { Some(key.to_string()) };
        match vendor {
            FallbackVendor::Groq => self.groq_key = v,
            FallbackVendor::OpenAi => self.openai_key = v,
        }
    }
}

impl PanelState {
    pub fn new(home_krs_per_min: i64, fallback: FallbackPrice) -> Self {
        Self {
            tariffs: Some(TariffTable::new(home_krs_per_min, fallback)),
            ..Default::default()
        }
    }

    fn tariff(&self) -> Tariff {
        self.tariffs.as_ref().map(|t| t.current()).unwrap_or(Tariff {
            version: 0,
            home_krs_per_min: 0,
            fallback: FallbackPrice::FixedKrsPerMin(0),
            upstream_min_secs: tariffs::DEFAULT_UPSTREAM_MIN_SECS,
        })
    }

    // ---- hesap/davet ----
    pub fn open_account(
        &mut self,
        admin: &str,
        code: &str,
        username: &str,
        opening_krs: i64,
        now: u64,
    ) -> Result<accounts::Invite, AccountError> {
        let inv = self.accounts.create_invite(code, username, opening_krs, now)?;
        self.audit.record(now, admin, "hesap-acma", &format!("{} bakiye={}", username, opening_krs));
        Ok(inv)
    }

    // ---- kullanıcı satır işlemleri ----
    pub fn top_up(&mut self, admin: &str, username: &str, amount_krs: i64, now: u64) -> Result<i64, AccountError> {
        let acc = self.accounts.get_mut(username).ok_or(AccountError::AccountNotFound)?;
        let b = users::top_up(acc, amount_krs)?;
        self.audit.record(now, admin, "bakiye-ekle", &format!("{} +{} -> {}", username, amount_krs, b));
        Ok(b)
    }

    pub fn set_limits(
        &mut self,
        admin: &str,
        username: &str,
        daily_home: Option<i64>,
        daily_fb: Option<i64>,
        monthly_home: Option<i64>,
        monthly_fb: Option<i64>,
        fb_cap: Option<i64>,
        home_only: Option<bool>,
        now: u64,
    ) -> Result<(), AccountError> {
        let acc = self.accounts.get_mut(username).ok_or(AccountError::AccountNotFound)?;
        acc.daily_cap_home_krs = daily_home;
        acc.daily_cap_fb_krs = daily_fb;
        acc.monthly_cap_home_krs = monthly_home;
        acc.monthly_cap_fb_krs = monthly_fb;
        acc.fallback_daily_cap_krs = fb_cap;
        if let Some(h) = home_only {
            acc.home_only = h;
        }
        self.audit.record(now, admin, "limit-koy", username);
        Ok(())
    }

    pub fn reset_hwid(&mut self, admin: &str, username: &str, now: u64) -> Result<HwidResetEntry, AccountError> {
        let acc = self.accounts.get_mut(username).ok_or(AccountError::AccountNotFound)?;
        let e = users::reset_hwid(acc, admin, &mut self.hwid_log, now);
        let warn = users::hwid_reset_warning(acc, now);
        self.audit.record(
            now,
            admin,
            "hwid-reset",
            &format!("{} slot={} uyari={}", username, e.slots_cleared, warn),
        );
        Ok(e)
    }

    pub fn suspend(&mut self, admin: &str, username: &str, stop: bool, now: u64) -> Result<(), AccountError> {
        let acc = self.accounts.get_mut(username).ok_or(AccountError::AccountNotFound)?;
        acc.suspended = stop;
        self.audit.record(now, admin, if stop { "hesap-durdur" } else { "hesap-ac" }, username);
        Ok(())
    }

    pub fn force_update(
        &mut self,
        admin: &str,
        username: &str,
        min_version: Option<&str>,
        now: u64,
    ) -> Result<(), AccountError> {
        let acc = self.accounts.get_mut(username).ok_or(AccountError::AccountNotFound)?;
        acc.forced_min_version = min_version.map(|s| s.to_string());
        self.audit.record(now, admin, "guncellemeye-zorla", &format!("{} taban={:?}", username, min_version));
        Ok(())
    }

    pub fn user_table(&self, line_for_warning: Line, now: u64) -> Vec<UserRow> {
        let t = self.tariff();
        let mut rows: Vec<UserRow> = self
            .accounts
            .accounts
            .values()
            .map(|a| users::user_row(a, &t, line_for_warning, now))
            .collect();
        rows.sort_by(|a, b| a.username.cmp(&b.username));
        rows
    }

    // ---- tarife + kabul kapısı ----
    pub fn set_tariff(&mut self, admin: &str, home: i64, fb: FallbackPrice, now: u64) -> Tariff {
        let t = self.tariffs.get_or_insert_with(|| TariffTable::new(home, fb)).set_tariff(home, fb);
        self.audit.record(now, admin, "tarife-degisimi", &format!("v{} ev={}", t.version, home));
        t
    }

    /// Fallback hat seçimi (Groq varsayılan; karar: docs/FALLBACK.md §1).
    /// Sürüm artar, sonraki isteklere uygulanır.
    pub fn set_vendor(&mut self, admin: &str, vendor: FallbackVendor, now: u64) -> Tariff {
        let t = self
            .tariffs
            .get_or_insert_with(default_table)
            .set_vendor(vendor);
        self.audit.record(now, admin, "hat-secimi", vendor.name());
        t
    }

    /// Belirli hattın fiyatını ayarlar (aktif/pasif fark etmez, kaybolmaz).
    pub fn set_vendor_price(&mut self, admin: &str, vendor: FallbackVendor, price: FallbackPrice, now: u64) -> Tariff {
        let t = self
            .tariffs
            .get_or_insert_with(default_table)
            .set_vendor_price(vendor, price);
        self.audit.record(now, admin, "hat-fiyat", &format!("{} v{}", vendor.name(), t.version));
        t
    }

    /// Belirli hattın upstream tabanını ayarlar (3sn altına kelepçelenir).
    pub fn set_vendor_upstream_min(&mut self, admin: &str, vendor: FallbackVendor, secs: u64, now: u64) -> Tariff {
        let t = self
            .tariffs
            .get_or_insert_with(default_table)
            .set_vendor_upstream_min(vendor, secs);
        self.audit.record(now, admin, "hat-tabani", &format!("{} {}sn", vendor.name(), t.upstream_min_secs));
        t
    }

    /// Sağlayıcı anahtarını panele girer (bellek-içi; değer loga YAZILMAZ).
    /// Boş anahtar = temizleme ile aynıdır.
    pub fn set_provider_key(&mut self, admin: &str, vendor: FallbackVendor, key: &str, now: u64) -> bool {
        let stored = !key.is_empty();
        self.secrets.set(vendor, key);
        self.audit.record(now, admin, if stored { "anahtar-giris" } else { "anahtar-sil" }, vendor.name());
        stored
    }

    /// Anahtar VAR/YOK durumu (değerler asla dışarı verilmez).
    pub fn keys_set(&self) -> (bool, bool) {
        (self.secrets.has(FallbackVendor::Groq), self.secrets.has(FallbackVendor::OpenAi))
    }

    /// Yeni istek kabul kapısı (ücretsiz retler burada; ücret yazılmaz).
    pub fn admit(&self, username: &str, line: Line, measured_secs: u64, now: u64) -> Result<Quote, RouteError> {
        let acc = self.accounts.get(username).ok_or(RouteError::Maintenance("hesap"))?;
        let q = self.tariff().quote(line, measured_secs);
        let user_spend: Vec<SpendEntry> = Vec::new(); // hesap-bazlı süzme broker defterinden beslenir
        let _ = &user_spend;
        // Not: spend defteri broker'da tutulur; burada hesap-içi günlük takibi
        // için `self.spend` süzülür (aşağıda). İmza basitliği için doğrudan:
        admit(acc, line, q.cost_krs, &self.switch, &self.spend_filtered(username, now), now)?;
        Ok(q)
    }

    fn spend_filtered(&self, _username: &str, _now: u64) -> Vec<SpendEntry> {
        // Basitlik: spend kayıtları hesap etiketi taşımaz; broker entegrasyonunda
        // hesap-bazlı süzülecek (F1a defteri bağlanınca). Şimdilik global pencere.
        self.spend.clone()
    }

    /// Başarılı işin kesinleşmesi: HER ZAMAN izinli (şalter devam edeni
    /// durdurmaz; upstream harcaması geri alınamaz).
    pub fn finalize(&mut self, username: &str, q: &Quote, now: u64) -> Result<i64, AccountError> {
        let acc = self.accounts.get_mut(username).ok_or(AccountError::AccountNotFound)?;
        acc.balance_krs -= q.cost_krs;
        match q.line {
            Line::Home => acc.usage_home_secs += q.billable_secs,
            Line::Fallback => acc.usage_fb_secs += q.billable_secs,
        }
        acc.last_active = now;
        self.spend.push(SpendEntry { at: now, line: q.line, amount_krs: q.cost_krs });
        Ok(acc.balance_krs)
    }

    // ---- şalter ----
    pub fn set_switch(&mut self, admin: &str, open: bool, now: u64) {
        self.switch = FallbackSwitch { open };
        self.audit.record(now, admin, "acil-salter", if open { "acik" } else { "kapali" });
    }

    // ---- besleme ----
    pub fn init_feed(&mut self, public_key: [u8; 32], base_url: &str) {
        self.feed = Some(FeedState::new(public_key, base_url));
    }

    #[allow(clippy::too_many_arguments)]
    pub fn publish_release(
        &mut self,
        admin: &str,
        version: &str,
        notes: &str,
        forced: bool,
        floor: &str,
        msi: &[u8],
        sig_hex: &str,
        now: u64,
    ) -> Result<Release, FeedError> {
        let f = self.feed.as_mut().ok_or(FeedError::BadPublicKey)?;
        let r = f.publish(version, notes, forced, floor, msi, sig_hex, now)?;
        self.audit.record(now, admin, "surum-yayinlama", &format!("{} zorunlu={} taban={}", version, forced, floor));
        Ok(r)
    }

    pub fn check_client(&self, client_version: &str) -> UpdateCheck {
        match &self.feed {
            Some(f) => f.check_client(client_version),
            None => UpdateCheck::Ok,
        }
    }

    // ---- saklama / KVKK / disk ----
    pub fn set_retention(&mut self, admin: &str, username: &str, opt_out: bool, now: u64) -> Result<(), AccountError> {
        let acc = self.accounts.get_mut(username).ok_or(AccountError::AccountNotFound)?;
        acc.retention_opt_out = opt_out;
        self.audit.record(now, admin, "saklama", &format!("{} opt_out={}", username, opt_out));
        Ok(())
    }

    pub fn kvkk_hard_delete(&mut self, admin: &str, username: &str, now: u64) -> Result<HardDeleteReport, AccountError> {
        if !self.accounts.accounts.contains_key(username) {
            return Err(AccountError::AccountNotFound);
        }
        self.accounts.accounts.remove(username);
        self.invites_cleanup(username);
        let rep = retention::hard_delete_plan(username, admin, now);
        self.audit.record(now, admin, "kvkk-hard-delete", &format!("{} kalem={}", username, rep.removed.len()));
        Ok(rep)
    }

    fn invites_cleanup(&mut self, username: &str) {
        let dead: Vec<String> = self
            .accounts
            .invites
            .iter()
            .filter(|(_, inv)| inv.username == username)
            .map(|(c, _)| c.clone())
            .collect();
        for c in dead {
            self.accounts.invites.remove(&c);
        }
    }

    pub fn report_disks(&mut self, worker: u8, broker: u8, archive: u8) {
        self.disks = DiskMonitor {
            disks: vec![
                DiskStatus { kind: DiskKind::Worker, used_pct: worker.min(100) },
                DiskStatus { kind: DiskKind::Broker, used_pct: broker.min(100) },
                DiskStatus { kind: DiskKind::Archive, used_pct: archive.min(100) },
            ],
        };
    }

    pub fn routes() -> &'static [(&'static str, &'static str, &'static str)] {
        ROUTES
    }
}

pub use admin::AdminError;
pub use feed::FeedError;

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> PanelState {
        PanelState::new(120, FallbackPrice::MultiplierBp(50000))
    }

    fn ali(s: &mut PanelState) {
        s.open_account("admin", "KOD1", "ali", 10_000, 0).unwrap();
        s.accounts.redeem_invite("KOD1", "guclu-sifre-1", "H1", 1).unwrap();
    }

    #[test]
    fn invite_flow_through_state_with_audit() {
        let mut s = state();
        ali(&mut s);
        assert!(s.audit.entries.iter().any(|e| e.action == "hesap-acma"));
        // Davet yeniden kullanılamaz.
        assert!(s.accounts.redeem_invite("KOD1", "baska-sifre-2", "H2", 2).is_err());
    }

    #[test]
    fn tariff_change_hits_next_request() {
        let mut s = state();
        ali(&mut s);
        let q1 = s.admit("ali", Line::Home, 60, 10).unwrap();
        assert_eq!((q1.tariff_version, q1.cost_krs), (1, 120));
        s.set_tariff("admin", 240, FallbackPrice::FixedKrsPerMin(900), 11);
        let q2 = s.admit("ali", Line::Home, 60, 12).unwrap();
        assert_eq!((q2.tariff_version, q2.cost_krs), (2, 240));
        // Eski teklif eski sürümle kesinleşir.
        s.finalize("ali", &q1, 13).unwrap();
        assert_eq!(s.accounts.get("ali").unwrap().balance_krs, 10_000 - 120);
    }

    #[test]
    fn switch_closed_rejects_new_but_finalize_still_charges() {
        let mut s = state();
        ali(&mut s);
        // Devam eden işin teklifi şalter açıkken alınır.
        let inflight = s.admit("ali", Line::Fallback, 60, 10).unwrap();
        s.set_switch("admin", false, 11);
        // Yeni fallback isteği bakım hatasıyla ücretsiz reddedilir.
        assert_eq!(
            s.admit("ali", Line::Fallback, 60, 12),
            Err(RouteError::Maintenance("fallback bakimda"))
        );
        // Ev hattı çalışmaya devam eder.
        assert!(s.admit("ali", Line::Home, 60, 12).is_ok());
        // Devam eden iş bitirilip normal ücretlenir.
        let cost = inflight.cost_krs;
        s.finalize("ali", &inflight, 13).unwrap();
        assert_eq!(s.accounts.get("ali").unwrap().balance_krs, 10_000 - cost);
        assert!(s.audit.entries.iter().any(|e| e.action == "acil-salter"));
    }

    #[test]
    fn hwid_reset_logged_and_admin_ops_audited() {
        let mut s = state();
        ali(&mut s);
        s.reset_hwid("admin", "ali", 50).unwrap();
        assert_eq!(s.hwid_log.entries.len(), 1);
        assert!(s.audit.entries.iter().any(|e| e.action == "hwid-reset"));
        s.suspend("admin", "ali", true, 51).unwrap();
        assert!(s.admit("ali", Line::Home, 10, 52).is_err());
        s.suspend("admin", "ali", false, 53).unwrap();
        assert!(s.admit("ali", Line::Home, 10, 54).is_ok());
    }

    #[test]
    fn retention_defaults_on_and_hard_delete() {
        let mut s = state();
        ali(&mut s);
        assert!(!s.accounts.get("ali").unwrap().retention_opt_out);
        s.set_retention("admin", "ali", true, 60).unwrap();
        assert!(s.accounts.get("ali").unwrap().retention_opt_out);
        let rep = s.kvkk_hard_delete("admin", "ali", 61).unwrap();
        assert!(rep.removed.contains(&"idempotency-onbellek"));
        assert!(s.accounts.get("ali").is_none());
        assert!(s.audit.entries.iter().any(|e| e.action == "kvkk-hard-delete"));
    }

    #[test]
    fn vendor_switch_and_key_vault() {
        let mut s = state();
        ali(&mut s);
        assert_eq!(s.tariffs.as_ref().unwrap().vendor(), FallbackVendor::Groq);
        // OpenAI hattına geç: 5sn fallback 5sn/50 kuruş (3sn taban).
        s.set_vendor("admin", FallbackVendor::OpenAi, 10);
        let q = s.admit("ali", Line::Fallback, 5, 11).unwrap();
        assert_eq!((q.billable_secs, q.cost_krs), (5, 50));
        // Anahtar kasası: değer dışarı sızmaz, audit detayda değer yok.
        assert_eq!(s.keys_set(), (false, false));
        assert!(s.set_provider_key("admin", FallbackVendor::Groq, "gizli-anahtar", 12));
        assert_eq!(s.keys_set(), (true, false));
        assert!(s.audit.entries.iter().any(|e| e.action == "anahtar-giris"
            && e.detail == "groq" && !e.detail.contains("gizli")));
        // Boş anahtar = temizleme.
        assert!(!s.set_provider_key("admin", FallbackVendor::Groq, "", 13));
        assert_eq!(s.keys_set(), (false, false));
        assert!(s.audit.entries.iter().any(|e| e.action == "hat-secimi"));
    }

    #[test]
    fn disks_and_routes_present() {
        let mut s = state();
        s.report_disks(90, 10, 80);
        let w = s.disks.warnings();
        assert_eq!(w.len(), 2);
        assert!(PanelState::routes().iter().any(|(_, p, _)| *p == "/v1/feed"));
        assert!(PanelState::routes().iter().any(|(_, p, _)| *p == "/v1/users/{u}/data"));
    }

    #[test]
    fn admin_setup_lock_and_update_check_without_feed() {
        let mut s = state();
        assert_eq!(s.check_client("0.1.0"), UpdateCheck::Ok);
        assert!(s.admin.verify("x", 0).is_err());
        s.admin.setup("yonetici-sifresi-1").unwrap();
        assert!(s.admin.verify("yonetici-sifresi-1", 1).is_ok());
        let e: Result<(), AdminError> = s.admin.verify("y", 2).map(|_| ());
        assert!(e.is_err());
    }
}
