//! Hesap + davet kodu (PLAN §3: hesap, davet, cihaz kilidi).
//!
//! Akış: admin `create_invite(kullanıcı + açılış bakiyesi)` → tek kullanımlık
//! davet kodu (7 gün süreli). Davetle ilk girişte şifre belirlenir + HWID
//! 1. slota bağlanır. Boş slot varsa kullanıcı adı + şifreyle yeni cihaz
//! bağlanır; slot yoksa admin panelden boşaltır (HWID reset).

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const INVITE_TTL_SECS: u64 = 7 * 24 * 3600;
pub const DEFAULT_DEVICE_SLOTS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    pub code: String,
    pub username: String,
    pub opening_balance_krs: i64,
    pub created_at: u64,
    pub expires_at: u64,
    pub redeemed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceBinding {
    pub hwid: String,
    pub bound_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub username: String,
    pub balance_krs: i64,
    /// Argon2 PHC dizesi; davet redeem'inde belirlenir.
    pub password_hash: String,
    pub devices: Vec<DeviceBinding>,
    pub max_devices: usize,
    pub suspended: bool,
    /// Sadece-ev modu: fallback hattı bu hesaba kapalı.
    pub home_only: bool,
    /// Saklama kapatma bayrağı (varsayılan AÇIK = false; kodda hazır).
    pub retention_opt_out: bool,
    pub fallback_daily_cap_krs: Option<i64>,
    pub daily_cap_home_krs: Option<i64>,
    pub daily_cap_fb_krs: Option<i64>,
    pub monthly_cap_home_krs: Option<i64>,
    pub monthly_cap_fb_krs: Option<i64>,
    pub usage_home_secs: u64,
    pub usage_fb_secs: u64,
    pub last_active: u64,
    pub online: bool,
    pub client_version: String,
    /// HWID reset zamanları (epoch sn); sık reset uyarısı için.
    pub hwid_resets: Vec<u64>,
    /// Hesabı güncellemeye zorla: bu sürümün altı girişte reddedilir.
    pub forced_min_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountError {
    InviteExists,
    InviteNotFound,
    InviteRedeemed,
    InviteExpired,
    AccountExists,
    AccountNotFound,
    Suspended,
    BadPassword,
    NoFreeSlot,
    HwidMismatch,
    WeakPassword,
    BadOpeningBalance,
    InsufficientBalance,
}

impl std::fmt::Display for AccountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for AccountError {}

/// Tek kullanımlık davet kodu üretir (sözlük-dışı, 20 alfanümerik).
pub fn generate_invite_code() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(20)
        .map(char::from)
        .collect()
}

fn hash_password(password: &str) -> Result<String, AccountError> {
    if password.len() < 8 {
        return Err(AccountError::WeakPassword);
    }
    let salt = SaltString::generate(&mut rand::thread_rng());
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|_| AccountError::WeakPassword)
}

fn verify_password(hash: &str, password: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AccountStore {
    pub invites: HashMap<String, Invite>,
    pub accounts: HashMap<String, Account>,
}

impl AccountStore {
    /// Hesap açma: kullanıcı + açılış bakiyesi → tek kullanımlık davet.
    pub fn create_invite(
        &mut self,
        code: &str,
        username: &str,
        opening_balance_krs: i64,
        now: u64,
    ) -> Result<Invite, AccountError> {
        if code.is_empty() || self.invites.contains_key(code) {
            return Err(AccountError::InviteExists);
        }
        if self.accounts.contains_key(username) {
            return Err(AccountError::AccountExists);
        }
        if opening_balance_krs < 0 {
            return Err(AccountError::BadOpeningBalance);
        }
        let inv = Invite {
            code: code.to_string(),
            username: username.to_string(),
            opening_balance_krs,
            created_at: now,
            expires_at: now + INVITE_TTL_SECS,
            redeemed: false,
        };
        self.invites.insert(code.to_string(), inv.clone());
        Ok(inv)
    }

    /// Davetle ilk giriş: şifre belirlenir + HWID 1. slota bağlanır.
    pub fn redeem_invite(
        &mut self,
        code: &str,
        password: &str,
        hwid: &str,
        now: u64,
    ) -> Result<Account, AccountError> {
        let inv = self.invites.get_mut(code).ok_or(AccountError::InviteNotFound)?;
        if inv.redeemed {
            return Err(AccountError::InviteRedeemed);
        }
        if now > inv.expires_at {
            return Err(AccountError::InviteExpired);
        }
        if self.accounts.contains_key(&inv.username) {
            return Err(AccountError::AccountExists);
        }
        let password_hash = hash_password(password)?;
        let acc = Account {
            username: inv.username.clone(),
            balance_krs: inv.opening_balance_krs,
            password_hash,
            devices: vec![DeviceBinding { hwid: hwid.to_string(), bound_at: now }],
            max_devices: DEFAULT_DEVICE_SLOTS,
            suspended: false,
            home_only: false,
            retention_opt_out: false,
            fallback_daily_cap_krs: None,
            daily_cap_home_krs: None,
            daily_cap_fb_krs: None,
            monthly_cap_home_krs: None,
            monthly_cap_fb_krs: None,
            usage_home_secs: 0,
            usage_fb_secs: 0,
            last_active: now,
            online: false,
            client_version: String::new(),
            hwid_resets: Vec::new(),
            forced_min_version: None,
        };
        inv.redeemed = true;
        self.accounts.insert(acc.username.clone(), acc.clone());
        Ok(acc)
    }

    /// Boş slot varsa kullanıcı adı + şifreyle yeni cihaz bağlanır.
    pub fn bind_device(
        &mut self,
        username: &str,
        password: &str,
        hwid: &str,
        now: u64,
    ) -> Result<(), AccountError> {
        let acc = self.accounts.get_mut(username).ok_or(AccountError::AccountNotFound)?;
        if acc.suspended {
            return Err(AccountError::Suspended);
        }
        if !verify_password(&acc.password_hash, password) {
            return Err(AccountError::BadPassword);
        }
        if acc.devices.iter().any(|d| d.hwid == hwid) {
            return Ok(());
        }
        if acc.devices.len() >= acc.max_devices {
            return Err(AccountError::NoFreeSlot);
        }
        acc.devices.push(DeviceBinding { hwid: hwid.to_string(), bound_at: now });
        Ok(())
    }

    /// Cihaz girişi: HWID kayıtlı slotlardan biriyle eşleşmeli.
    pub fn check_device(&self, username: &str, hwid: &str) -> Result<(), AccountError> {
        let acc = self.accounts.get(username).ok_or(AccountError::AccountNotFound)?;
        if acc.suspended {
            return Err(AccountError::Suspended);
        }
        if acc.devices.iter().any(|d| d.hwid == hwid) {
            Ok(())
        } else {
            Err(AccountError::HwidMismatch)
        }
    }

    pub fn verify_user_password(&self, username: &str, password: &str) -> Result<(), AccountError> {
        let acc = self.accounts.get(username).ok_or(AccountError::AccountNotFound)?;
        if acc.suspended {
            return Err(AccountError::Suspended);
        }
        if verify_password(&acc.password_hash, password) {
            Ok(())
        } else {
            Err(AccountError::BadPassword)
        }
    }

    pub fn get(&self, username: &str) -> Option<&Account> {
        self.accounts.get(username)
    }

    pub fn get_mut(&mut self, username: &str) -> Option<&mut Account> {
        self.accounts.get_mut(username)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> AccountStore {
        AccountStore::default()
    }

    #[test]
    fn invite_create_and_redeem() {
        let mut s = store();
        let inv = s.create_invite("KOD123", "ali", 5000, 1000).unwrap();
        assert_eq!(inv.expires_at, 1000 + INVITE_TTL_SECS);
        assert!(!inv.redeemed);
        let acc = s.redeem_invite("KOD123", "guclu-sifre-1", "HWID-1", 1001).unwrap();
        assert_eq!(acc.balance_krs, 5000);
        assert_eq!(acc.devices.len(), 1);
        assert_eq!(acc.max_devices, DEFAULT_DEVICE_SLOTS);
        // Tek kullanımlık: ikinci redeem reddedilir.
        assert_eq!(
            s.redeem_invite("KOD123", "baska-sifre-2", "HWID-2", 1002),
            Err(AccountError::InviteRedeemed)
        );
    }

    #[test]
    fn invite_expiry_and_unknown() {
        let mut s = store();
        s.create_invite("KOD9", "veli", 0, 0).unwrap();
        assert_eq!(
            s.redeem_invite("KOD9", "guclu-sifre-1", "H", INVITE_TTL_SECS + 1),
            Err(AccountError::InviteExpired)
        );
        assert_eq!(
            s.redeem_invite("YOK", "guclu-sifre-1", "H", 1),
            Err(AccountError::InviteNotFound)
        );
    }

    #[test]
    fn device_slots_and_admin_free() {
        let mut s = store();
        s.create_invite("K", "ali", 100, 0).unwrap();
        s.redeem_invite("K", "guclu-sifre-1", "H1", 1).unwrap();
        s.bind_device("ali", "guclu-sifre-1", "H2", 2).unwrap();
        // Slot dolu → 3. cihaz reddedilir.
        assert_eq!(
            s.bind_device("ali", "guclu-sifre-1", "H3", 3),
            Err(AccountError::NoFreeSlot)
        );
        assert!(s.check_device("ali", "H1").is_ok());
        assert_eq!(s.check_device("ali", "HX"), Err(AccountError::HwidMismatch));
        assert_eq!(
            s.bind_device("ali", "yanlis-sifre", "H3", 3),
            Err(AccountError::BadPassword)
        );
    }

    #[test]
    fn weak_password_and_negative_balance_rejected() {
        let mut s = store();
        assert_eq!(
            s.create_invite("K1", "a", -5, 0),
            Err(AccountError::BadOpeningBalance)
        );
        s.create_invite("K2", "b", 0, 0).unwrap();
        assert_eq!(
            s.redeem_invite("K2", "kisa", "H", 1),
            Err(AccountError::WeakPassword)
        );
    }
}
