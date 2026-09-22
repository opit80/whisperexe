//! Admin kimliği (PLAN §4: kurulumda yerelde argon2, kaba-kuvvet kilidi,
//! admin işlemleri loglanır; panele internetten erişim varsayımı).

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use serde::{Deserialize, Serialize};

pub const MAX_FAILED: u32 = 5;
pub const LOCK_SECS: u64 = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminError {
    AlreadySetup,
    NotSetup,
    Locked { until: u64 },
    BadPassword,
    WeakPassword,
}

impl std::fmt::Display for AdminError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for AdminError {}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AdminAuth {
    hash: Option<String>,
    failed: u32,
    pub locked_until: u64,
    /// Kaba-kuvvet kilit uyarıları (adminin göreceği kuyruk).
    pub alerts: Vec<String>,
}

impl AdminAuth {
    pub fn is_setup(&self) -> bool {
        self.hash.is_some()
    }

    /// Kurulumda, yerelde bir kez çağrılır.
    pub fn setup(&mut self, password: &str) -> Result<(), AdminError> {
        if self.hash.is_some() {
            return Err(AdminError::AlreadySetup);
        }
        if password.len() < 12 {
            return Err(AdminError::WeakPassword);
        }
        let salt = SaltString::generate(&mut rand::thread_rng());
        let h = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|_| AdminError::WeakPassword)?;
        self.hash = Some(h.to_string());
        Ok(())
    }

    /// 5 başarısız girişte 5dk kilit + admin uyarısı.
    pub fn verify(&mut self, password: &str, now: u64) -> Result<(), AdminError> {
        let hash = self.hash.as_ref().ok_or(AdminError::NotSetup)?;
        if now < self.locked_until {
            return Err(AdminError::Locked { until: self.locked_until });
        }
        let ok = PasswordHash::new(hash)
            .map(|p| Argon2::default().verify_password(password.as_bytes(), &p).is_ok())
            .unwrap_or(false);
        if ok {
            self.failed = 0;
            Ok(())
        } else {
            self.failed += 1;
            if self.failed >= MAX_FAILED {
                self.locked_until = now + LOCK_SECS;
                self.failed = 0;
                self.alerts.push(format!(
                    "kaba-kuvvet kilidi: {} basarisiz giris, {} sn kilit (t={})",
                    MAX_FAILED, LOCK_SECS, now
                ));
            }
            Err(AdminError::BadPassword)
        }
    }

    /// Şifre değişimi: mevcut şifre doğrulanmak zorunda.
    pub fn change_password(&mut self, current: &str, next: &str, now: u64) -> Result<(), AdminError> {
        self.verify(current, now)?;
        if next.len() < 12 {
            return Err(AdminError::WeakPassword);
        }
        let salt = SaltString::generate(&mut rand::thread_rng());
        let h = Argon2::default()
            .hash_password(next.as_bytes(), &salt)
            .map_err(|_| AdminError::WeakPassword)?;
        self.hash = Some(h.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_once_and_login() {
        let mut a = AdminAuth::default();
        assert!(a.verify("x", 0).is_err());
        a.setup("cok-gizli-sifre-123").unwrap();
        assert_eq!(a.setup("baska-sifre-4567"), Err(AdminError::AlreadySetup));
        assert!(a.verify("cok-gizli-sifre-123", 10).is_ok());
        assert_eq!(a.verify("yanlis", 11), Err(AdminError::BadPassword));
    }

    #[test]
    fn brute_force_lock_and_alert() {
        let mut a = AdminAuth::default();
        a.setup("cok-gizli-sifre-123").unwrap();
        for i in 0..5 {
            assert_eq!(a.verify("yanlis", i), Err(AdminError::BadPassword));
        }
        // 5. hatada kilit devreye girer.
        assert!(matches!(a.verify("cok-gizli-sifre-123", 5), Err(AdminError::Locked { .. })));
        assert_eq!(a.alerts.len(), 1);
        // Kilit süresi geçince doğru şifre yine çalışır.
        assert!(a.verify("cok-gizli-sifre-123", 5 + LOCK_SECS).is_ok());
    }
}
