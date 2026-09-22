//! Broker sürüm kontrolü: imzalı paket indir + hash/imza doğrulama.
//! PLAN §4/§6: imza anahtarı offline'dadır, istemcide yalnızca public key
//! durur. Sürüm her açılışta sorulur; taban altındaki istemci girişte
//! "güncelle" yer. Zorunlu güncelleme dictation ortasında DEĞİL,
//! yeniden başlatmada uygulanır.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

/// Broker'ın yayınladığı sürüm bildirimi.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    pub version: String,
    pub min_supported: String,
    pub url: String,
    pub sha256_hex: String,
    /// `sha256_hex` + `version` + `min_supported` üzerine ed25519 imza (hex).
    pub signature_hex: String,
    pub mandatory: bool,
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    BadSignature,
    BadHash,
    BadFormat(String),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::BadSignature => write!(f, "Guncelleme imzasi gecersiz, kurulmadi."),
            UpdateError::BadHash => write!(f, "Indirilen paket beklenen hash ile eslesmedi."),
            UpdateError::BadFormat(e) => write!(f, "Surum bildirimi hatali: {e}"),
        }
    }
}

fn signed_bytes(m: &Manifest) -> Vec<u8> {
    format!("{}|{}|{}", m.sha256_hex, m.version, m.min_supported).into_bytes()
}

fn decode_hex(s: &str) -> Result<Vec<u8>, UpdateError> {
    if !s.len().is_multiple_of(2) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(UpdateError::BadFormat("hex cozumlenemedi".into()));
    }
    Ok((0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap_or(0))
        .collect())
}

/// İmzayı doğrula (önce imza, sonra hash: bozuk imza ağı görmeden elenir).
pub fn verify(manifest: &Manifest, package: &[u8], public_key: &[u8; 32]) -> Result<(), UpdateError> {
    let key = VerifyingKey::from_bytes(public_key)
        .map_err(|e| UpdateError::BadFormat(e.to_string()))?;
    let sig_bytes = decode_hex(&manifest.signature_hex)?;
    let sig_arr: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| UpdateError::BadFormat("imza 64 bayt degil".into()))?;
    key.verify(&signed_bytes(manifest), &Signature::from_bytes(&sig_arr))
        .map_err(|_| UpdateError::BadSignature)?;

    let mut h = Sha256::new();
    h.update(package);
    let digest = hex_of(h.finalize());
    if digest != manifest.sha256_hex.to_lowercase() {
        return Err(UpdateError::BadHash);
    }
    Ok(())
}

fn hex_of(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// Karar: istemci ne yapmalı?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateDecision {
    /// Güncel, devam.
    Current,
    /// Yeni sürüm var ama dictation ortasında değil, yeniden başlatmada kur.
    PendingRestart,
    /// Taban altı: girişte "güncelle" yer, devam eden iş bitirilir.
    BlockedBelowFloor,
}

pub fn decide(current: &str, manifest: &Manifest) -> UpdateDecision {
    if cmp_ver(current, &manifest.min_supported) == std::cmp::Ordering::Less {
        return UpdateDecision::BlockedBelowFloor;
    }
    if cmp_ver(current, &manifest.version) == std::cmp::Ordering::Less {
        return UpdateDecision::PendingRestart;
    }
    UpdateDecision::Current
}

/// Basit semver karşılaştırma (major.minor.patch; eksik = 0).
fn cmp_ver(a: &str, b: &str) -> std::cmp::Ordering {
    let pa = parse_ver(a);
    let pb = parse_ver(b);
    pa.cmp(&pb)
}

fn parse_ver(v: &str) -> [u64; 3] {
    let mut out = [0u64; 3];
    for (i, p) in v.split('.').take(3).enumerate() {
        out[i] = p.parse().unwrap_or(0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn keypair() -> (SigningKey, [u8; 32]) {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let pk = sk.verifying_key().to_bytes();
        (sk, pk)
    }

    fn signed_manifest(sk: &SigningKey, package: &[u8]) -> Manifest {
        let mut h = Sha256::new();
        h.update(package);
        let sha = hex_of(h.finalize());
        let mut m = Manifest {
            version: "1.2.0".into(),
            min_supported: "1.0.0".into(),
            url: "https://broker/update/1.2.0.msi".into(),
            sha256_hex: sha,
            signature_hex: String::new(),
            mandatory: true,
            notes: "kritik".into(),
        };
        let sig = sk.sign(&signed_bytes(&m));
        m.signature_hex = hex_of(sig.to_bytes());
        m
    }

    #[test]
    fn valid_package_verifies() {
        let (sk, pk) = keypair();
        let pkg = b"sahte-msi-icerigi";
        let m = signed_manifest(&sk, pkg);
        assert!(verify(&m, pkg, &pk).is_ok());
    }

    #[test]
    fn tampered_package_rejected() {
        let (sk, pk) = keypair();
        let m = signed_manifest(&sk, b"sahte-msi-icerigi");
        // İmza geçerli ama içerik değişmiş → hash reddi.
        assert_eq!(
            verify(&m, b"degistirilmis-icerik", &pk),
            Err(UpdateError::BadHash)
        );
    }

    #[test]
    fn forged_signature_rejected() {
        let (sk, pk) = keypair();
        let pkg = b"sahte-msi-icerigi";
        let mut m = signed_manifest(&sk, pkg);
        // Saldırgan kendi paketinin hash'ini yazıp imzayı kopyalar → imza reddi.
        let mut h = Sha256::new();
        h.update("kötü-paket".as_bytes());
        m.sha256_hex = hex_of(h.finalize());
        assert_eq!(
            verify(&m, "kötü-paket".as_bytes(), &pk),
            Err(UpdateError::BadSignature)
        );
        // Yanlış anahtarla da reddedilir.
        let (_, other_pk) = (SigningKey::from_bytes(&[9u8; 32]), {
            let s = SigningKey::from_bytes(&[9u8; 32]);
            s.verifying_key().to_bytes()
        });
        let m2 = signed_manifest(&sk, pkg);
        assert_eq!(
            verify(&m2, pkg, &other_pk),
            Err(UpdateError::BadSignature)
        );
    }

    #[test]
    fn floor_blocks_old_client() {
        let m = Manifest {
            version: "1.2.0".into(),
            min_supported: "1.1.0".into(),
            url: String::new(),
            sha256_hex: String::new(),
            signature_hex: String::new(),
            mandatory: true,
            notes: String::new(),
        };
        assert_eq!(decide("1.0.5", &m), UpdateDecision::BlockedBelowFloor);
        assert_eq!(decide("1.1.0", &m), UpdateDecision::PendingRestart);
        assert_eq!(decide("1.2.0", &m), UpdateDecision::Current);
    }
}
