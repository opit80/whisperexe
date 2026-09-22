//! Sürüm beslemesi (PLAN §4 güncelleme yayınlama).
//!
//! - Derlenen imzalı .msi panele yüklenir: sürüm + değişiklik notu + zorunlu
//!   bayrağı. İmzalama anahtarı offline'dadır; panelde/broker'da yalnızca
//!   PUBLIC KEY + imza durur (panel ele geçse sahte EXE imzalanamaz).
//! - Yayınlama = ed25519 imza doğrulama (public key ile) + sha256 eşleşmesi.
//! - Tabanın (min desteklenen sürüm) altındaki istemci girişte "güncelle" yer;
//!   devam eden işi bitirilir. Geri dönüş = önceki sürümü yeniden yayınlama;
//!   taban düşüşü de imzalı manifestle olur (taban sunucunun bildirdiğidir,
//!   deadlock olmaz). Tauri updater beslemesi broker'dan verilir; özel
//!   indirici yazılmaz.
//!
//! Gerçek imzalama anahtarı üretilmez/istenmez: testlerde anlık test
//! anahtarı + public-key doğrulama kullanılır.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedError {
    BadSignature,
    BadPublicKey,
    HashMismatch,
    UnknownVersion,
    EmptyNotes,
}

impl std::fmt::Display for FeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for FeedError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub forced: bool,
    pub msi_sha256_hex: String,
    pub msi_len: u64,
    pub signature_hex: String,
    pub published_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedState {
    /// Panelde duran TEK sır: public key (private ASLA burada yok).
    pub public_key: [u8; 32],
    pub releases: Vec<Release>,
    /// Zorunlu-sürüm tabanı (sunucunun bildirdiği).
    pub floor_version: String,
    /// İndirme kökü (broker verir).
    pub download_base_url: String,
}

impl FeedState {
    pub fn new(public_key: [u8; 32], download_base_url: &str) -> Self {
        Self {
            public_key,
            releases: Vec::new(),
            floor_version: String::new(),
            download_base_url: download_base_url.trim_end_matches('/').to_string(),
        }
    }

    pub fn verifying_key(&self) -> Result<VerifyingKey, FeedError> {
        VerifyingKey::from_bytes(&self.public_key).map_err(|_| FeedError::BadPublicKey)
    }

    /// İmzalanan kanonik bildiri baytları (sürüm + not + zorunlu + taban + sha).
    pub fn canonical_manifest(
        version: &str,
        notes: &str,
        forced: bool,
        floor_version: &str,
        msi_sha256_hex: &str,
    ) -> Vec<u8> {
        format!(
            "whisperexe-update-v1\n{}\n{}\n{}\n{}\n{}",
            version, notes, forced as u8, floor_version, msi_sha256_hex
        )
        .into_bytes()
    }

    fn decode_hex(s: &str) -> Result<Vec<u8>, FeedError> {
        if s.len() % 2 != 0 {
            return Err(FeedError::BadSignature);
        }
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| FeedError::BadSignature))
            .collect()
    }

    /// Sürü yayınlama: imza public key ile doğrulanır + sha256 tutarlılığı
    /// kontrol edilir. Taban değişimi aynı imzalı manifestle gelir.
    #[allow(clippy::too_many_arguments)]
    pub fn publish(
        &mut self,
        version: &str,
        notes: &str,
        forced: bool,
        floor_version: &str,
        msi_bytes: &[u8],
        signature_hex: &str,
        now: u64,
    ) -> Result<Release, FeedError> {
        if notes.trim().is_empty() {
            return Err(FeedError::EmptyNotes);
        }
        let digest = Sha256::digest(msi_bytes);
        let sha_hex = hex_of(&digest);
        let msg = Self::canonical_manifest(version, notes, forced, floor_version, &sha_hex);
        let sig_bytes = Self::decode_hex(signature_hex)?;
        if sig_bytes.len() != 64 {
            return Err(FeedError::BadSignature);
        }
        let mut arr = [0u8; 64];
        arr.copy_from_slice(&sig_bytes);
        let sig = Signature::from_bytes(&arr);
        self.verifying_key()?
            .verify(&msg, &sig)
            .map_err(|_| FeedError::BadSignature)?;
        // Aynı sürüm yeniden yayınlanabilir (geri dönüş); kayıt güncellenir.
        self.floor_version = floor_version.to_string();
        if let Some(r) = self.releases.iter_mut().find(|r| r.version == version) {
            *r = Release {
                version: version.to_string(),
                notes: notes.to_string(),
                forced,
                msi_sha256_hex: sha_hex,
                msi_len: msi_bytes.len() as u64,
                signature_hex: signature_hex.to_string(),
                published_at: now,
            };
            return Ok(r.clone());
        }
        let r = Release {
            version: version.to_string(),
            notes: notes.to_string(),
            forced,
            msi_sha256_hex: sha_hex,
            msi_len: msi_bytes.len() as u64,
            signature_hex: signature_hex.to_string(),
            published_at: now,
        };
        self.releases.push(r.clone());
        Ok(r)
    }

    pub fn latest(&self) -> Option<&Release> {
        self.releases.iter().max_by(|a, b| cmp_version(&a.version, &b.version))
    }

    /// İstemci sürüm kontrolü (her açılışta): taban altı → güncelle (devam
    /// eden iş bitirilir); zorunlu bayraklı yenilik → zorunlu güncelleme.
    pub fn check_client(&self, client_version: &str) -> UpdateCheck {
        let Some(latest) = self.latest() else {
            return UpdateCheck::Ok;
        };
        if !self.floor_version.is_empty() && cmp_version(client_version, &self.floor_version).is_lt() {
            return UpdateCheck::Blocked {
                latest: latest.version.clone(),
                reason: "taban surumun altinda: giriste guncelle",
            };
        }
        if cmp_version(client_version, &latest.version).is_lt() {
            return UpdateCheck::Available {
                latest: latest.version.clone(),
                forced: latest.forced,
                notes: latest.notes.clone(),
            };
        }
        UpdateCheck::Ok
    }

    /// Tauri updater beslemesi (broker'dan servis edilir).
    pub fn tauri_feed(&self) -> Option<serde_json::Value> {
        let latest = self.latest()?;
        Some(serde_json::json!({
            "version": latest.version,
            "notes": latest.notes,
            "pub_date": latest.published_at.to_string(),
            "platforms": {
                "windows-x86_64": {
                    "signature": latest.signature_hex,
                    "url": format!("{}/{}.msi", self.download_base_url, latest.version),
                }
            }
        }))
    }
}

fn hex_of(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 15) as usize] as char);
    }
    s
}

/// Basit sayısal sürüm karşılaştırma (1.2.10 > 1.2.9); parse edilemeyen
/// kuyruk sözlüksel karşılaştırılır.
pub fn cmp_version(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let pa: Vec<u64> = a.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    let pb: Vec<u64> = b.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    let n = pa.len().max(pb.len());
    for i in 0..n {
        let x = pa.get(i).copied().unwrap_or(0);
        let y = pb.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            Ordering::Equal => continue,
            o => return o,
        }
    }
    Ordering::Equal
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    Ok,
    Available { latest: String, forced: bool, notes: String },
    Blocked { latest: String, reason: &'static str },
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};

    fn test_key() -> SigningKey {
        // Anlık TEST anahtarı (gerçek imzalama anahtarı değil).
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn sign_manifest(
        key: &SigningKey,
        version: &str,
        notes: &str,
        forced: bool,
        floor: &str,
        msi: &[u8],
    ) -> (String, String) {
        let sha = {
            let d = Sha256::digest(msi);
            super::hex_of(&d)
        };
        let msg = FeedState::canonical_manifest(version, notes, forced, floor, &sha);
        let sig = key.sign(&msg);
        (sha, super::hex_of(&sig.to_bytes()))
    }

    fn feed() -> (FeedState, SigningKey) {
        let key = test_key();
        let vk = key.verifying_key().to_bytes();
        (FeedState::new(vk, "https://ornek.test/indir"), key)
    }

    #[test]
    fn publish_verifies_signature_with_public_key_only() {
        let (mut f, key) = feed();
        let msi = b"sahte-msi-icerigi";
        let (_, sig) = sign_manifest(&key, "1.2.0", "ilk surum", false, "1.0.0", msi);
        let r = f.publish("1.2.0", "ilk surum", false, "1.0.0", msi, &sig, 100).unwrap();
        assert_eq!(r.msi_len, msi.len() as u64);
        assert_eq!(f.floor_version, "1.0.0");
        // Sahte imza (başka anahtar) reddedilir.
        let other = SigningKey::from_bytes(&[9u8; 32]);
        let (_, bad) = sign_manifest(&other, "1.3.0", "sahte", false, "1.0.0", msi);
        assert_eq!(
            f.publish("1.3.0", "sahte", false, "1.0.0", msi, &bad, 101),
            Err(FeedError::BadSignature)
        );
        // Kurcalanmış not imza doğrulamasını bozar.
        assert_eq!(
            f.publish("1.2.0", "degistirilmis not", false, "1.0.0", msi, &sig, 102),
            Err(FeedError::BadSignature)
        );
    }

    #[test]
    fn floor_blocks_old_client_and_downgrade_drops_floor() {
        let (mut f, key) = feed();
        let msi = b"msi";
        let (_, s1) = sign_manifest(&key, "1.1.0", "n1", false, "1.0.0", msi);
        f.publish("1.1.0", "n1", false, "1.0.0", msi, &s1, 10).unwrap();
        let (_, s2) = sign_manifest(&key, "1.2.0", "n2", true, "1.2.0", msi);
        f.publish("1.2.0", "n2", true, "1.2.0", msi, &s2, 20).unwrap();
        // Taban altı istemci girişte günceller.
        assert!(matches!(
            f.check_client("1.0.0"),
            UpdateCheck::Blocked { .. }
        ));
        assert!(matches!(f.check_client("1.2.0"), UpdateCheck::Ok));
        // Geri dönüş: önceki sürüm yeniden yayınlanır, taban birlikte düşer.
        let (_, s0) = sign_manifest(&key, "1.1.0", "geri donus", false, "1.0.0", msi);
        f.publish("1.1.0", "geri donus", false, "1.0.0", msi, &s0, 30).unwrap();
        assert_eq!(f.floor_version, "1.0.0");
        // 1.0.0 artık taban altı değil (Blocked değil), ama en yüksek sürüm
        // 1.2.0 durduğu için güncelleme mevcuttur (Available, Ok değil).
        assert!(matches!(
            f.check_client("1.0.0"),
            UpdateCheck::Available { .. }
        ));
        // Tauri beslemesi en yüksek sürümü verir.
        let feed_json = f.tauri_feed().unwrap();
        assert_eq!(feed_json["version"], "1.2.0");
    }
}
