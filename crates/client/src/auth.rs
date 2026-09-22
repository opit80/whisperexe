//! Davet kodu ile ilk giriş, şifre belirleme, HWID bağlama,
//! access (15dk) + refresh (30 gün, dönerli) token saklama.

use crate::hwid::Hwid;

pub const ACCESS_TTL_SECS: u64 = 15 * 60;
pub const REFRESH_TTL_SECS: u64 = 30 * 24 * 3600;
/// Hesap başına varsayılan cihaz slotu (PLAN §3).
pub const DEFAULT_DEVICE_SLOTS: u32 = 2;
/// Şifre en az uzunluğu.
pub const MIN_PASSWORD_LEN: usize = 12;
/// 5 başarısız girişte 5dk kilit.
pub const MAX_FAILED_ATTEMPTS: u32 = 5;
pub const LOCKOUT_SECS: u64 = 5 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteRedeem {
    pub invite_code: String,
    pub username: String,
    pub password: String,
    pub hwid: Hwid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    EmptyInvite,
    EmptyUsername,
    WeakPassword,
    EmptyHwid,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            AuthError::EmptyInvite => "Davet kodu bos olamaz.",
            AuthError::EmptyUsername => "Kullanici adi bos olamaz.",
            AuthError::WeakPassword => "Sifre en az 12 karakter olmali.",
            AuthError::EmptyHwid => "Cihaz kimligi okunamadi.",
        };
        f.write_str(msg)
    }
}

impl InviteRedeem {
    /// Davetle ilk giriş: şifre belirleme + HWID 1. slota bağlama.
    pub fn new(
        invite_code: &str,
        username: &str,
        password: &str,
        hwid: Hwid,
    ) -> Result<Self, AuthError> {
        if invite_code.trim().is_empty() {
            return Err(AuthError::EmptyInvite);
        }
        if username.trim().is_empty() {
            return Err(AuthError::EmptyUsername);
        }
        if password.len() < MIN_PASSWORD_LEN {
            return Err(AuthError::WeakPassword);
        }
        if hwid.0.trim().is_empty() {
            return Err(AuthError::EmptyHwid);
        }
        Ok(Self {
            invite_code: invite_code.trim().to_string(),
            username: username.trim().to_string(),
            password: password.to_string(),
            hwid,
        })
    }
}

/// Bellekte tutulan token çifti. Refresh her kullanımda döner;
/// yenisi gelince eskisi anında unutulur (tekrar kullanım sunucuda
/// tüm oturumları düşürür, o yüzden eskisini asla saklamayız).
#[derive(Debug, Clone, Default)]
pub struct TokenStore {
    access: Option<String>,
    access_expires_at_unix: u64,
    refresh: Option<String>,
}

impl TokenStore {
    /// `now_unix` anında geçerli access var mı? Kabul anında geçerli
    /// token isteğin sonuna kadar yaşar, o yüzden sınırda tolerans yok.
    pub fn access_valid(&self, now_unix: u64) -> bool {
        self.access.is_some() && now_unix < self.access_expires_at_unix
    }

    pub fn set_pair(&mut self, access: String, now_unix: u64, refresh: String) {
        self.access = Some(access);
        self.access_expires_at_unix = now_unix + ACCESS_TTL_SECS;
        self.refresh = Some(refresh);
    }

    /// Refresh döndürme: eski çifti yenisiyle değiştirir.
    pub fn rotate(&mut self, access: String, now_unix: u64, refresh: String) {
        self.set_pair(access, now_unix, refresh);
    }

    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh.as_deref()
    }

    /// Çıkış / revoke: her şeyi unut.
    pub fn clear(&mut self) {
        self.access = None;
        self.refresh = None;
        self.access_expires_at_unix = 0;
    }
}

/// Başarısız giriş sayacı: 5 hata → 5dk kilit.
#[derive(Debug, Clone, Default)]
pub struct LoginAttempts {
    pub failed: u32,
    pub locked_until_unix: u64,
}

impl LoginAttempts {
    pub fn locked(&self, now_unix: u64) -> bool {
        now_unix < self.locked_until_unix
    }

    pub fn record_failure(&mut self, now_unix: u64) {
        self.failed += 1;
        if self.failed >= MAX_FAILED_ATTEMPTS {
            self.locked_until_unix = now_unix + LOCKOUT_SECS;
            self.failed = 0;
        }
    }

    pub fn record_success(&mut self) {
        self.failed = 0;
        self.locked_until_unix = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hwid() -> Hwid {
        Hwid("test-hwid-1".into())
    }

    #[test]
    fn redeem_validates_inputs() {
        assert!(InviteRedeem::new("", "ali", "uzun-sifre-123", hwid()).is_err());
        assert!(InviteRedeem::new("DAVET-1", "", "uzun-sifre-123", hwid()).is_err());
        assert!(InviteRedeem::new("DAVET-1", "ali", "kisa", hwid()).is_err());
        assert!(InviteRedeem::new("DAVET-1", "ali", "uzun-sifre-123", Hwid(String::new())).is_err());
        assert!(InviteRedeem::new("DAVET-1", "ali", "uzun-sifre-123", hwid()).is_ok());
    }

    #[test]
    fn access_lives_15_minutes() {
        let mut s = TokenStore::default();
        s.set_pair("a".into(), 1_000, "r".into());
        assert!(s.access_valid(1_000 + ACCESS_TTL_SECS - 1));
        assert!(!s.access_valid(1_000 + ACCESS_TTL_SECS));
    }

    #[test]
    fn rotate_forgets_old_refresh() {
        let mut s = TokenStore::default();
        s.set_pair("a1".into(), 1_000, "r1".into());
        s.rotate("a2".into(), 2_000, "r2".into());
        assert_eq!(s.refresh_token(), Some("r2"));
        assert!(s.access_valid(2_000));
    }

    #[test]
    fn five_failures_lock_for_5_minutes() {
        let mut a = LoginAttempts::default();
        for _ in 0..4 {
            a.record_failure(1_000);
            assert!(!a.locked(1_000));
        }
        a.record_failure(1_000);
        assert!(a.locked(1_000));
        assert!(!a.locked(1_000 + LOCKOUT_SECS));
    }
}
