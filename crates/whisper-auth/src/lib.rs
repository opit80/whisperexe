//! F1a kimlik doğrulama: PLAN.md §3 kilitli kararlar.
//!
//! - Davet kodu: tek kullanımlık + 7 gün süreli. İlk girişte şifre +
//!   HWID 1. slota bağlanır. Varsayılan 2 slot.
//! - Access token 15dk: imzalı (HMAC-SHA256), HWID talepli.
//! - Refresh 30 gün: HWID'ye bağlı, her kullanımda döner;
//!   TEKRAR KULLANIM tüm oturumları düşürür.
//! - 5 başarısız girişte 5dk kilit + admin uyarı kaydı.
//! - Token kabul anında geçerliyse isteğin sonuna kadar yaşar
//!   (broker kabulde bir kez doğrular, ortada öldürmez).
//! - Revoke (durdurma, HWID sıfırlama) SONRAKİ kabulde + refresh
//!   dönüşünde işler; devam eden iş bitirilir.
//!
//! NOT: HWID beyanı istemciden gelir, sunucu doğrulayamaz; amaç
//! fırsatçı paylaşımı zorlaştırmaktır. Şifre KDF'si format etiketlidir
//! (`s256$...`); üretimde Argon2 + OS CSPRNG'ye taşınabilir.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Sabitler
// ---------------------------------------------------------------------------

/// Davet kodu ömrü: 7 gün (saniye).
pub const INVITE_TTL_SECS: u64 = 7 * 24 * 3600;
/// Access token ömrü: 15 dakika.
pub const ACCESS_TTL_SECS: u64 = 15 * 60;
/// Refresh token ömrü: 30 gün.
pub const REFRESH_TTL_SECS: u64 = 30 * 24 * 3600;
/// Kaba-kuvvet eşiği: 5 başarısız giriş...
pub const LOCKOUT_THRESHOLD: u32 = 5;
/// ...sonrası 5 dakika kilit.
pub const LOCKOUT_SECS: u64 = 5 * 60;
/// Varsayılan cihaz slotu.
pub const DEFAULT_SLOTS: usize = 2;
/// Şifre KDF tur sayısı (SHA-256 zinciri).
pub const PW_ROUNDS: u32 = 10_000;

// ---------------------------------------------------------------------------
// Hatalar
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    InviteNotFound,
    InviteUsed,
    InviteExpired,
    AccountExists,
    AccountNotFound,
    AccountLocked { retry_after_secs: u64 },
    AccountSuspended,
    BadPassword,
    BadHwid,
    NoFreeSlot,
    BadToken,
    TokenExpired,
    TokenRevoked,
    RefreshReuse,
    WeakPassword,
}

impl AuthError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InviteNotFound => "invite_not_found",
            Self::InviteUsed => "invite_used",
            Self::InviteExpired => "invite_expired",
            Self::AccountExists => "account_exists",
            Self::AccountNotFound => "account_not_found",
            Self::AccountLocked { .. } => "account_locked",
            Self::AccountSuspended => "account_suspended",
            Self::BadPassword => "bad_password",
            Self::BadHwid => "bad_hwid",
            Self::NoFreeSlot => "no_free_slot",
            Self::BadToken => "bad_token",
            Self::TokenExpired => "token_expired",
            Self::TokenRevoked => "token_revoked",
            Self::RefreshReuse => "refresh_reuse",
            Self::WeakPassword => "weak_password",
        }
    }
}

// ---------------------------------------------------------------------------
// SHA-256 (std-only) + HMAC-SHA256 + base64url
// ---------------------------------------------------------------------------

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256(msg: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (msg.len() as u64).wrapping_mul(8);
    let mut padded = msg.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, b) in chunk.chunks_exact(4).enumerate().take(16) {
            w[i] = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut kb = [0u8; 64];
    if key.len() > 64 {
        let h = sha256(key);
        kb[..32].copy_from_slice(&h);
    } else {
        kb[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= kb[i];
        opad[i] ^= kb[i];
    }
    let mut inner = ipad.to_vec();
    inner.extend_from_slice(msg);
    let ih = sha256(&inner);
    let mut outer = opad.to_vec();
    outer.extend_from_slice(&ih);
    sha256(&outer)
}

const B64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn b64url_encode(data: &[u8]) -> String {
    let mut s = String::with_capacity((data.len() + 2) / 3 * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (if c.len() > 1 { c[1] as u32 } else { 0 }) << 8
            | (if c.len() > 2 { c[2] as u32 } else { 0 });
        s.push(B64URL[((n >> 18) & 63) as usize] as char);
        s.push(B64URL[((n >> 12) & 63) as usize] as char);
        if c.len() > 1 {
            s.push(B64URL[((n >> 6) & 63) as usize] as char);
        }
        if c.len() > 2 {
            s.push(B64URL[(n & 63) as usize] as char);
        }
    }
    s
}

fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut vals: Vec<u8> = Vec::with_capacity(s.len());
    for b in s.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        vals.push(v);
    }
    if vals.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(vals.len() / 4 * 3);
    for c in vals.chunks(4) {
        let n = (c[0] as u32) << 18
            | (c[1] as u32) << 12
            | (if c.len() > 2 { c[2] as u32 } else { 0 }) << 6
            | (if c.len() > 3 { c[3] as u32 } else { 0 });
        out.push((n >> 16) as u8);
        if c.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if c.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

fn hex(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 15) as usize] as char);
    }
    s
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let b = s.as_bytes();
    let hv = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    for p in b.chunks_exact(2) {
        out.push(hv(p[0])? << 4 | hv(p[1])?);
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Şifre KDF (format etiketli; üretimde Argon2'ye taşınabilir)
// ---------------------------------------------------------------------------

static SALT_CTR: AtomicU64 = AtomicU64::new(0);

fn make_salt() -> [u8; 16] {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let c = SALT_CTR.fetch_add(1, Ordering::Relaxed);
    let mut salt = [0u8; 16];
    salt[..8].copy_from_slice(&t.wrapping_mul(0x9E3779B97F4A7C15).to_le_bytes());
    salt[8..].copy_from_slice(&c.wrapping_mul(0xBF58476D1CE4E5B9).to_le_bytes());
    salt
}

fn kdf(password: &[u8], salt: &[u8], rounds: u32) -> [u8; 32] {
    let mut buf = Vec::with_capacity(password.len() + salt.len() + 4);
    buf.extend_from_slice(password);
    buf.extend_from_slice(salt);
    let mut h = sha256(&buf);
    for _ in 1..rounds {
        let mut b = Vec::with_capacity(32 + salt.len());
        b.extend_from_slice(&h);
        b.extend_from_slice(salt);
        h = sha256(&b);
    }
    h
}

fn hash_password(password: &str) -> String {
    let salt = make_salt();
    let h = kdf(password.as_bytes(), &salt, PW_ROUNDS);
    format!("s256${}${}${}", PW_ROUNDS, hex(&salt), hex(&h))
}

fn verify_password(password: &str, stored: &str) -> bool {
    let parts: Vec<&str> = stored.split('$').collect();
    if parts.len() != 4 || parts[0] != "s256" {
        return false;
    }
    let rounds: u32 = match parts[1].parse() {
        Ok(r) => r,
        Err(_) => return false,
    };
    let salt = match unhex(parts[2]) {
        Some(s) => s,
        None => return false,
    };
    let expect = match unhex(parts[3]) {
        Some(h) => h,
        None => return false,
    };
    if expect.len() != 32 || !(1..=1_000_000).contains(&rounds) {
        return false;
    }
    let got = kdf(password.as_bytes(), &salt, rounds);
    // Sabit-süreli karşılaştırma.
    let mut diff = 0u8;
    for (a, b) in got.iter().zip(expect.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

fn valid_name(s: &str) -> bool {
    // Token gövdesinde '|' ayraçtır; isimlerde yasak.
    !s.is_empty() && s.len() <= 64 && !s.contains('|') && !s.contains(' ')
}

fn valid_hwid(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.contains('|') && !s.contains(' ')
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invite {
    pub code: String,
    pub created_at_secs: u64,
    pub used: bool,
    pub used_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RefreshRecord {
    hwid: String,
    expires_at: u64,
    /// Döndürülmüş (kullanılmış) ama henüz süresi dolmamış jeton.
    /// Bu kimlikle TEKRAR gelinirse → çalıntı şüphesi → tüm oturumlar düşer.
    rotated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    password_hash: String,
    pub hwid_slots: Vec<String>,
    pub max_slots: usize,
    pub suspended: bool,
    failed_attempts: u32,
    locked_until_secs: u64,
    /// Revoke nesli: artınca eski access token'lar sonraki kabulde ölür.
    generation: u64,
    refresh: HashMap<String, RefreshRecord>,
    refresh_ctr: u64,
}

#[derive(Debug, Clone)]
pub struct AdminAlert {
    pub at_secs: u64,
    pub kind: &'static str,
    pub detail: String,
}

/// Bellek-içi auth deposu. Üretimde broker bunu kalıcı depoya bağlar;
/// tüm kurallar burada tek-elden uygulanır (saat parametreyle girer).
///
/// Kalıcılık: `secret` (ortamdan gelir) ve `alerts` (geçici uyarı kuyruğu)
/// anlık görüntüye YAZILMAZ; yüklemede sır yeniden verilir
/// ([`AuthStore::set_secret`]).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AuthStore {
    #[serde(skip)]
    secret: Vec<u8>,
    invites: HashMap<String, Invite>,
    accounts: HashMap<String, Account>,
    #[serde(skip)]
    pub alerts: Vec<AdminAlert>,
    token_ctr: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenPair {
    pub access: String,
    pub refresh: String,
    pub access_expires_at: u64,
    pub refresh_expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessClaims {
    pub account: String,
    pub hwid: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub generation: u64,
}

impl AuthStore {
    pub fn new(secret: &[u8]) -> Self {
        Self {
            secret: secret.to_vec(),
            ..Default::default()
        }
    }

    /// Anlık görüntüden yüklemede sırrı yeniden verir (sır dosyaya yazılmaz).
    pub fn set_secret(&mut self, secret: &[u8]) {
        self.secret = secret.to_vec();
    }

    // -- davet --

    /// Admin davet açar: kullanıcı + açılış bakiyesi panelde işlenir,
    /// burada yalnızca tek-kullanımlık kod üretilir/kaydedilir.
    pub fn create_invite(&mut self, code: &str, now_secs: u64) -> Result<(), AuthError> {
        if code.is_empty() || code.len() > 128 || code.contains(' ') {
            return Err(AuthError::InviteNotFound);
        }
        if self.invites.contains_key(code) {
            return Err(AuthError::AccountExists);
        }
        self.invites.insert(
            code.to_string(),
            Invite {
                code: code.to_string(),
                created_at_secs: now_secs,
                used: false,
                used_by: None,
            },
        );
        Ok(())
    }

    /// Davetle ilk giriş: şifre belirlenir + HWID 1. slota bağlanır.
    /// Kod TEK kullanımlıktır; 7 gün sonra ölür.
    pub fn redeem_invite(
        &mut self,
        code: &str,
        account_id: &str,
        password: &str,
        hwid: &str,
        now_secs: u64,
    ) -> Result<TokenPair, AuthError> {
        let inv = self.invites.get(code).ok_or(AuthError::InviteNotFound)?;
        if inv.used {
            return Err(AuthError::InviteUsed);
        }
        if now_secs.saturating_sub(inv.created_at_secs) > INVITE_TTL_SECS {
            return Err(AuthError::InviteExpired);
        }
        if !valid_name(account_id) || self.accounts.contains_key(account_id) {
            return Err(AuthError::AccountExists);
        }
        if password.len() < 8 {
            return Err(AuthError::WeakPassword);
        }
        if !valid_hwid(hwid) {
            return Err(AuthError::BadHwid);
        }
        let acc = Account {
            id: account_id.to_string(),
            password_hash: hash_password(password),
            hwid_slots: vec![hwid.to_string()],
            max_slots: DEFAULT_SLOTS,
            suspended: false,
            failed_attempts: 0,
            locked_until_secs: 0,
            generation: 0,
            refresh: HashMap::new(),
            refresh_ctr: 0,
        };
        self.accounts.insert(account_id.to_string(), acc);
        let inv = self.invites.get_mut(code).expect("checked");
        inv.used = true;
        inv.used_by = Some(account_id.to_string());
        Ok(self.issue_pair(account_id, hwid, now_secs))
    }

    // -- giriş --

    /// Kullanıcı adı + şifreyle giriş. Boş slot varsa yeni cihaz bağlanır,
    /// slot yoksa admin sıfırlar. 5 başarısızda 5dk kilit + admin uyarısı.
    pub fn login(
        &mut self,
        account_id: &str,
        password: &str,
        hwid: &str,
        now_secs: u64,
    ) -> Result<TokenPair, AuthError> {
        let acc = self.accounts.get(account_id).ok_or(AuthError::AccountNotFound)?;
        if acc.suspended {
            return Err(AuthError::AccountSuspended);
        }
        if now_secs < acc.locked_until_secs {
            return Err(AuthError::AccountLocked {
                retry_after_secs: acc.locked_until_secs - now_secs,
            });
        }
        if !verify_password(password, &acc.password_hash) {
            let acc = self.accounts.get_mut(account_id).expect("checked");
            acc.failed_attempts += 1;
            if acc.failed_attempts >= LOCKOUT_THRESHOLD {
                acc.failed_attempts = 0;
                acc.locked_until_secs = now_secs + LOCKOUT_SECS;
                self.alerts.push(AdminAlert {
                    at_secs: now_secs,
                    kind: "lockout",
                    detail: format!("hesap {account_id} 5 basarisiz giris: 5dk kilit"),
                });
            }
            return Err(AuthError::BadPassword);
        }
        let acc = self.accounts.get_mut(account_id).expect("checked");
        acc.failed_attempts = 0;
        if !acc.hwid_slots.contains(&hwid.to_string()) {
            if acc.hwid_slots.len() >= acc.max_slots {
                return Err(AuthError::NoFreeSlot);
            }
            if !valid_hwid(hwid) {
                return Err(AuthError::BadHwid);
            }
            acc.hwid_slots.push(hwid.to_string());
        }
        Ok(self.issue_pair(account_id, hwid, now_secs))
    }

    // -- token --

    fn sign(&self, payload_b64: &str) -> String {
        b64url_encode(&hmac_sha256(&self.secret, payload_b64.as_bytes()))
    }

    fn issue_pair(&mut self, account_id: &str, hwid: &str, now_secs: u64) -> TokenPair {
        let generation = self.accounts.get(account_id).expect("account exists").generation;
        let iat = now_secs;
        let exp = now_secs + ACCESS_TTL_SECS;
        let payload = format!("{}|{}|{}|{}|{}", account_id, hwid, iat, exp, generation);
        let pb = b64url_encode(payload.as_bytes());
        let sig = self.sign(&pb);
        let access = format!("v1.{}.{}", pb, sig);
        self.token_ctr += 1;
        let token_ctr = self.token_ctr;
        let acc = self.accounts.get_mut(account_id).expect("account exists");
        acc.refresh_ctr += 1;
        let rid = format!("r{}-{}", acc.refresh_ctr, &hex(&sha256(format!("{account_id}{hwid}{now_secs}{token_ctr}").as_bytes()))[..16]);
        acc.refresh.insert(
            rid.clone(),
            RefreshRecord {
                hwid: hwid.to_string(),
                expires_at: now_secs + REFRESH_TTL_SECS,
                rotated: false,
            },
        );
        TokenPair {
            access,
            refresh: rid,
            access_expires_at: exp,
            refresh_expires_at: now_secs + REFRESH_TTL_SECS,
        }
    }

    /// Kabul kapısı: imza + süre + HWID + nesil (revoke) kontrolü.
    /// Kabul anında geçerliyse OK döner; broker isteği SONUNA kadar
    /// işletir (ortada yeniden sormaz). Revoke sonraki kabulde işler.
    pub fn verify_access(
        &self,
        token: &str,
        expect_hwid: &str,
        now_secs: u64,
    ) -> Result<AccessClaims, AuthError> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 || parts[0] != "v1" {
            return Err(AuthError::BadToken);
        }
        let mut sig_ok = false;
        let expect = self.sign(parts[1]);
        if expect.len() == parts[2].len() {
            let mut diff = 0u8;
            for (a, b) in expect.bytes().zip(parts[2].bytes()) {
                diff |= a ^ b;
            }
            sig_ok = diff == 0;
        }
        if !sig_ok {
            return Err(AuthError::BadToken);
        }
        let raw = b64url_decode(parts[1]).ok_or(AuthError::BadToken)?;
        let payload = String::from_utf8(raw).map_err(|_| AuthError::BadToken)?;
        let f: Vec<&str> = payload.split('|').collect();
        if f.len() != 5 {
            return Err(AuthError::BadToken);
        }
        let claims = AccessClaims {
            account: f[0].to_string(),
            hwid: f[1].to_string(),
            issued_at: f[2].parse().map_err(|_| AuthError::BadToken)?,
            expires_at: f[3].parse().map_err(|_| AuthError::BadToken)?,
            generation: f[4].parse().map_err(|_| AuthError::BadToken)?,
        };
        if now_secs >= claims.expires_at {
            return Err(AuthError::TokenExpired);
        }
        if claims.hwid != expect_hwid {
            return Err(AuthError::BadHwid);
        }
        let acc = self.accounts.get(&claims.account).ok_or(AuthError::BadToken)?;
        if acc.suspended || claims.generation != acc.generation {
            return Err(AuthError::TokenRevoked);
        }
        Ok(claims)
    }

    /// Refresh dönüşü: eski jeton tek kullanımlıktır, yenisi verilir.
    /// Eski/döndürülmüş/bilinmeyen jetonun TEKRARI → çalıntı şüphesi:
    /// hesabın TÜM oturumları düşer (nesil artar + refresh'ler silinir).
    pub fn refresh(
        &mut self,
        refresh_token: &str,
        hwid: &str,
        now_secs: u64,
    ) -> Result<TokenPair, AuthError> {
        // Önce salt-okunur inceleme (borrow çakışmasız).
        let (account_id, reuse) = {
            let mut owner: Option<(String, bool)> = None;
            for (id, acc) in &self.accounts {
                if let Some(rec) = acc.refresh.get(refresh_token) {
                    let bad = rec.rotated || rec.hwid != hwid || now_secs >= rec.expires_at;
                    // Süresi dolmuş meşru jeton: reuse DEĞİL, normal ret.
                    let reuse = rec.rotated || rec.hwid != hwid;
                    owner = Some((id.clone(), if bad { reuse } else { false }));
                    if !bad {
                        break;
                    }
                }
            }
            match owner {
                Some(o) => o,
                None => return Err(AuthError::BadToken),
            }
        };
        // Süresi dolmuş ama döndürülmemiş jeton: sessiz ret (saldırı sayılmaz).
        if !reuse {
            let expired = self
                .accounts
                .get(&account_id)
                .and_then(|a| a.refresh.get(refresh_token))
                .map(|r| now_secs >= r.expires_at)
                .unwrap_or(true);
            if expired {
                return Err(AuthError::TokenExpired);
            }
        }
        if reuse {
            self.drop_all_sessions(&account_id, now_secs, "refresh_reuse");
            return Err(AuthError::RefreshReuse);
        }
        // Meşru dönüş: eskiyi döndürülmüş işaretle, yeni çift ver.
        let acc = self.accounts.get_mut(&account_id).expect("checked");
        if let Some(rec) = acc.refresh.get_mut(refresh_token) {
            rec.rotated = true;
        }
        Ok(self.issue_pair(&account_id, hwid, now_secs))
    }

    fn drop_all_sessions(&mut self, account_id: &str, now_secs: u64, kind: &'static str) {
        if let Some(acc) = self.accounts.get_mut(account_id) {
            acc.refresh.clear();
            acc.generation += 1;
        }
        self.alerts.push(AdminAlert {
            at_secs: now_secs,
            kind,
            detail: format!("hesap {account_id}: tum oturumlar dusuruldu ({kind})"),
        });
    }

    // -- admin --

    /// Hesabı durdur: sonraki kabul + refresh dönüşünde işler.
    pub fn suspend(&mut self, account_id: &str, now_secs: u64) -> Result<(), AuthError> {
        let acc = self.accounts.get_mut(account_id).ok_or(AuthError::AccountNotFound)?;
        acc.suspended = true;
        acc.generation += 1;
        acc.refresh.clear();
        self.alerts.push(AdminAlert {
            at_secs: now_secs,
            kind: "suspend",
            detail: format!("hesap {account_id} durduruldu"),
        });
        Ok(())
    }

    /// Hesabı yeniden aç (`suspend` aynası): `suspended=false` + audit log.
    /// Suspend sırasında nesil artıp refresh'ler temizlendiği için
    /// suspend-ÖNCESİ access/refresh jetonlar ÖLÜ kalır (nesil geri
    /// alınmaz); hesap yeni login ile çalışır. Kabul-anında-geçerli
    /// token kuralı korunur (PLAN.md §3).
    pub fn unsuspend(&mut self, account_id: &str, now_secs: u64) -> Result<(), AuthError> {
        let acc = self.accounts.get_mut(account_id).ok_or(AuthError::AccountNotFound)?;
        acc.suspended = false;
        self.alerts.push(AdminAlert {
            at_secs: now_secs,
            kind: "unsuspend",
            detail: format!("hesap {account_id} yeniden acildi"),
        });
        Ok(())
    }

    /// HWID sıfırlama (kayıp/spoof): slotlar boşalır, nesil artar,
    /// sonraki girişte cihaz yeniden bağlanır. Loglanır.
    pub fn reset_hwid(&mut self, account_id: &str, now_secs: u64) -> Result<(), AuthError> {
        let acc = self.accounts.get_mut(account_id).ok_or(AuthError::AccountNotFound)?;
        acc.hwid_slots.clear();
        acc.generation += 1;
        acc.refresh.clear();
        self.alerts.push(AdminAlert {
            at_secs: now_secs,
            kind: "hwid_reset",
            detail: format!("hesap {account_id} HWID slotlari sifirlandi"),
        });
        Ok(())
    }

    pub fn set_max_slots(
        &mut self,
        account_id: &str,
        max: usize,
        now_secs: u64,
    ) -> Result<(), AuthError> {
        if max == 0 || max > 16 {
            return Err(AuthError::BadToken);
        }
        let acc = self.accounts.get_mut(account_id).ok_or(AuthError::AccountNotFound)?;
        acc.max_slots = max;
        self.alerts.push(AdminAlert {
            at_secs: now_secs,
            kind: "slots",
            detail: format!("hesap {account_id} slot sayisi {max} oldu"),
        });
        Ok(())
    }

    pub fn account(&self, id: &str) -> Option<&Account> {
        self.accounts.get(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> AuthStore {
        AuthStore::new(b"test-secret-123")
    }

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn invite_single_use() {
        let mut s = store();
        s.create_invite("DAVET-1", 1000).unwrap();
        let p = s.redeem_invite("DAVET-1", "ali", "gizli-sifre-1", "hwid-A", 2000).unwrap();
        assert!(!p.access.is_empty());
        // İkinci kullanım: aynı kod ÖLÜDÜR.
        assert_eq!(
            s.redeem_invite("DAVET-1", "veli", "gizli-sifre-2", "hwid-B", 2000),
            Err(AuthError::InviteUsed)
        );
        // İlk girişte HWID 1. slota bağlandı.
        assert_eq!(s.account("ali").unwrap().hwid_slots, vec!["hwid-A"]);
    }

    #[test]
    fn invite_expires_after_7_days() {
        let mut s = store();
        s.create_invite("DAVET-2", 1000).unwrap();
        assert_eq!(
            s.redeem_invite("DAVET-2", "ali", "gizli-sifre-1", "hwid-A", 1000 + INVITE_TTL_SECS + 1),
            Err(AuthError::InviteExpired)
        );
    }

    #[test]
    fn second_device_binds_free_slot_third_rejected() {
        let mut s = store();
        s.create_invite("D", 0).unwrap();
        s.redeem_invite("D", "ali", "gizli-sifre-1", "hwid-A", 10).unwrap();
        // 2. cihaz: boş slot → bağlanır.
        s.login("ali", "gizli-sifre-1", "hwid-B", 20).unwrap();
        assert_eq!(s.account("ali").unwrap().hwid_slots.len(), 2);
        // 3. cihaz: slot yok → admin sıfırlamadan giremez.
        assert_eq!(
            s.login("ali", "gizli-sifre-1", "hwid-C", 30),
            Err(AuthError::NoFreeSlot)
        );
    }

    #[test]
    fn access_15min_and_hwid_bound() {
        let mut s = store();
        s.create_invite("D", 0).unwrap();
        let p = s.redeem_invite("D", "ali", "gizli-sifre-1", "hwid-A", 1000).unwrap();
        assert!(s.verify_access(&p.access, "hwid-A", 1000 + ACCESS_TTL_SECS - 1).is_ok());
        // 15dk doldu → ret.
        assert_eq!(
            s.verify_access(&p.access, "hwid-A", 1000 + ACCESS_TTL_SECS),
            Err(AuthError::TokenExpired)
        );
        // Yanlış HWID → ret.
        assert_eq!(
            s.verify_access(&p.access, "hwid-BASKASI", 1001),
            Err(AuthError::BadHwid)
        );
    }

    #[test]
    fn refresh_rotates_and_reuse_drops_all_sessions() {
        let mut s = store();
        s.create_invite("D", 0).unwrap();
        let p1 = s.redeem_invite("D", "ali", "gizli-sifre-1", "hwid-A", 1000).unwrap();
        // Meşru dönüş: yeni çift.
        let p2 = s.refresh(&p1.refresh, "hwid-A", 1100).unwrap();
        assert!(s.verify_access(&p2.access, "hwid-A", 1100).is_ok());
        // Eski refresh TEKRAR kullanılırsa → tüm oturumlar düşer.
        assert_eq!(
            s.refresh(&p1.refresh, "hwid-A", 1200),
            Err(AuthError::RefreshReuse)
        );
        // Yeni access bile sonraki kabulde ÖLÜDÜR (nesil arttı)...
        assert_eq!(
            s.verify_access(&p2.access, "hwid-A", 1200),
            Err(AuthError::TokenRevoked)
        );
        // ...ve yeni refresh de geçersizdir.
        assert_eq!(
            s.refresh(&p2.refresh, "hwid-A", 1200),
            Err(AuthError::BadToken)
        );
        // Admin uyarı kaydında iz var.
        assert!(s.alerts.iter().any(|a| a.kind == "refresh_reuse"));
    }

    #[test]
    fn five_failures_lock_5min_and_alert() {
        let mut s = store();
        s.create_invite("D", 0).unwrap();
        s.redeem_invite("D", "ali", "gizli-sifre-1", "hwid-A", 0).unwrap();
        for _ in 0..4 {
            assert_eq!(
                s.login("ali", "yanlis", "hwid-A", 100),
                Err(AuthError::BadPassword)
            );
        }
        // 5. yanlış → yine BadPassword döner ama kilit kurulur.
        assert_eq!(
            s.login("ali", "yanlis", "hwid-A", 100),
            Err(AuthError::BadPassword)
        );
        // Doğru şifre bile kilitteyken geçmez.
        match s.login("ali", "gizli-sifre-1", "hwid-A", 101) {
            Err(AuthError::AccountLocked { .. }) => {}
            other => panic!("kilit bekleniyordu: {other:?}"),
        }
        assert!(s.alerts.iter().any(|a| a.kind == "lockout"));
        // 5dk sonra kilit açılır.
        s.login("ali", "gizli-sifre-1", "hwid-A", 100 + LOCKOUT_SECS + 1).unwrap();
    }

    #[test]
    fn revoke_applies_at_next_accept_ongoing_finishes() {
        let mut s = store();
        s.create_invite("D", 0).unwrap();
        let p = s.redeem_invite("D", "ali", "gizli-sifre-1", "hwid-A", 1000).unwrap();
        // Kabul anında geçerli → broker işi sonuna kadar işletir
        // (bu davranış broker'ındır; burada kanıt: verify OK).
        assert!(s.verify_access(&p.access, "hwid-A", 1001).is_ok());
        // Admin durdurur → SONRAKİ kabulde ret.
        s.suspend("ali", 1002).unwrap();
        assert_eq!(
            s.verify_access(&p.access, "hwid-A", 1003),
            Err(AuthError::TokenRevoked)
        );
    }

    #[test]
    fn suspend_blocks_login() {
        let mut s = store();
        s.create_invite("D", 0).unwrap();
        s.redeem_invite("D", "ali", "gizli-sifre-1", "hwid-A", 1000).unwrap();
        s.suspend("ali", 1001).unwrap();
        assert_eq!(
            s.login("ali", "gizli-sifre-1", "hwid-A", 1002),
            Err(AuthError::AccountSuspended)
        );
        assert!(s.account("ali").unwrap().suspended);
    }

    #[test]
    fn suspend_unsuspend_login_works_but_old_tokens_stay_dead() {
        let mut s = store();
        s.create_invite("D", 0).unwrap();
        let p1 = s.redeem_invite("D", "ali", "gizli-sifre-1", "hwid-A", 1000).unwrap();
        s.suspend("ali", 1001).unwrap();
        // Durdurulmuşken login 403 (AccountSuspended).
        assert_eq!(
            s.login("ali", "gizli-sifre-1", "hwid-A", 1002),
            Err(AuthError::AccountSuspended)
        );
        // Yeniden aç.
        s.unsuspend("ali", 1003).unwrap();
        assert!(!s.account("ali").unwrap().suspended);
        // Yeni login çalışır.
        let p2 = s.login("ali", "gizli-sifre-1", "hwid-A", 1004).unwrap();
        assert!(s.verify_access(&p2.access, "hwid-A", 1005).is_ok());
        // Suspend-ÖNCESİ access/refresh ÖLÜ kalır (nesil geri alınmaz).
        assert_eq!(
            s.verify_access(&p1.access, "hwid-A", 1005),
            Err(AuthError::TokenRevoked)
        );
        assert_eq!(
            s.refresh(&p1.refresh, "hwid-A", 1005),
            Err(AuthError::BadToken)
        );
        assert!(s.alerts.iter().any(|a| a.kind == "unsuspend"));
    }

    #[test]
    fn unsuspend_unknown_account_not_found() {
        let mut s = store();
        assert_eq!(
            s.unsuspend("yok", 10),
            Err(AuthError::AccountNotFound)
        );
    }

    #[test]
    fn hwid_reset_clears_slots_and_logs() {
        let mut s = store();
        s.create_invite("D", 0).unwrap();
        s.redeem_invite("D", "ali", "gizli-sifre-1", "hwid-A", 0).unwrap();
        s.reset_hwid("ali", 50).unwrap();
        assert!(s.account("ali").unwrap().hwid_slots.is_empty());
        // Yeni cihaz yeniden bağlanabilir.
        s.login("ali", "gizli-sifre-1", "hwid-YENI", 60).unwrap();
        assert!(s.alerts.iter().any(|a| a.kind == "hwid_reset"));
    }
}
