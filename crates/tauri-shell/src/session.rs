//! Bellek-ici oturumlar (kullanici + admin).
//!
//! Sırlar YALNIZCA Rust tarafinda tutulur; JS'e asla gonderilmez, loga
//! yazilmaz. `Debug` bilerek elle yazildi: deger sizdirmaz.

/// Su anki unix-saniye (komutlar `now_secs` ile besler; test enjekte eder).
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Kullanici oturumu: access/refresh cifti + son bilinen bakiye.
pub struct UserSession {
    account: Option<String>,
    access: Option<String>,
    refresh: Option<String>,
    access_expires_at: u64,
    refresh_expires_at: u64,
    balance_kurus: Option<i64>,
    hwid: String,
}

impl std::fmt::Debug for UserSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserSession")
            .field("account", &self.account)
            .field("access_set", &self.access.is_some())
            .field("refresh_set", &self.refresh.is_some())
            .field("balance_kurus", &self.balance_kurus)
            .finish()
    }
}

impl Default for UserSession {
    fn default() -> Self {
        Self {
            account: None,
            access: None,
            refresh: None,
            access_expires_at: 0,
            refresh_expires_at: 0,
            balance_kurus: None,
            hwid: client::hwid::current().0,
        }
    }
}

impl UserSession {
    pub fn account(&self) -> Option<&str> {
        self.account.as_deref()
    }

    pub fn hwid(&self) -> &str {
        &self.hwid
    }

    pub fn access(&self) -> Option<&str> {
        self.access.as_deref()
    }

    pub fn access_valid(&self, now: u64) -> bool {
        self.access.is_some() && now < self.access_expires_at
    }

    pub fn logged_in(&self) -> bool {
        self.account.is_some() && self.access.is_some()
    }

    pub fn balance_kurus(&self) -> Option<i64> {
        self.balance_kurus
    }

    pub fn set_pair(
        &mut self,
        account: &str,
        access: String,
        refresh: String,
        access_expires_at: u64,
        refresh_expires_at: u64,
    ) {
        self.account = Some(account.to_string());
        self.access = Some(access);
        self.refresh = Some(refresh);
        self.access_expires_at = access_expires_at;
        self.refresh_expires_at = refresh_expires_at;
    }

    pub fn set_balance(&mut self, v: i64) {
        self.balance_kurus = Some(v);
    }

    pub fn clear(&mut self) {
        self.account = None;
        self.access = None;
        self.refresh = None;
        self.access_expires_at = 0;
        self.refresh_expires_at = 0;
        self.balance_kurus = None;
    }
}

/// Admin oturumu: jeton + bitis (bellekte; sifre tutulmaz).
pub struct AdminSession {
    token: Option<String>,
    expires_at: u64,
}

impl std::fmt::Debug for AdminSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminSession")
            .field("token_set", &self.token.is_some())
            .finish()
    }
}

impl Default for AdminSession {
    fn default() -> Self {
        Self {
            token: None,
            expires_at: 0,
        }
    }
}

impl AdminSession {
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    pub fn active(&self, now: u64) -> bool {
        self.token.is_some() && now < self.expires_at
    }

    pub fn set(&mut self, token: String, expires_at: u64) {
        self.token = Some(token);
        self.expires_at = expires_at;
    }

    pub fn clear(&mut self) {
        self.token = None;
        self.expires_at = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_pair_lifecycle() {
        let mut s = UserSession::default();
        assert!(!s.logged_in());
        assert!(!s.access_valid(100));
        s.set_pair("ali", "gizli-access-xyz".into(), "gizli-refresh-xyz".into(), 200, 300);
        assert!(s.logged_in());
        assert!(s.access_valid(199));
        assert!(!s.access_valid(200));
        assert_eq!(s.account(), Some("ali"));
        s.clear();
        assert!(!s.logged_in());
        // Debug deger sizdirmaz.
        let mut s2 = UserSession::default();
        s2.set_pair("ali", "gizli-access-xyz".into(), "gizli-refresh-xyz".into(), 200, 300);
        let d = format!("{:?}", s2);
        assert!(!d.contains("gizli-access-xyz"));
        assert!(!d.contains("gizli-refresh-xyz"));
    }

    #[test]
    fn admin_token_lifecycle() {
        let mut s = AdminSession::default();
        assert!(!s.active(10));
        s.set("gizli-tok-xyz".into(), 100);
        assert!(s.active(99));
        assert!(!s.active(100));
        s.clear();
        assert!(!s.active(50));
        let mut s2 = AdminSession::default();
        s2.set("gizli-tok-xyz".into(), 100);
        let d = format!("{:?}", s2);
        assert!(!d.contains("gizli-tok-xyz"));
    }
}
