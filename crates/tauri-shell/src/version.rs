//! Açılış kapısı: broker `/v1/version` bildirimi.
//!
//! Tel kuralı: özel indirici YOKTUR. Broker'ın yayınladığı JSON,
//! `client::updater::Manifest` şeklindedir (`version`, `min_supported`,
//! `url`, `sha256_hex`, `signature_hex`, `mandatory`, `notes`); kabuk
//! yalnızca KARARI verir (indirme/kurulum Tauri updater'ındır).
//!
//! Karar semantiği (PLAN §4/§6):
//! - Taban altı → girişte engelle (devam eden iş bitirilir).
//! - Zorunlu yenilik → yeniden başlatmada kur (dictation kesilmez).
//! - Geri dönüş (taban düşüşü) imzalı manifestle gelir; taban sunucunun
//!   bildirdiğidir, deadlock olmaz.

use client::updater::{decide, Manifest, UpdateDecision};

/// Broker sürüm ucu (göreli yol). Paneldeki `/v1/version-check`
/// ile aynı bildirimi döner; kabuk bu yolu kullanır.
pub const VERSION_ENDPOINT: &str = "/v1/version";

/// Broker taban URL'inden tam sürüm URL'i kurar.
pub fn endpoint_url(broker_base: &str) -> String {
    format!("{}/{}", broker_base.trim_end_matches('/'), "v1/version")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionError {
    BadJson(String),
}

impl std::fmt::Display for VersionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VersionError::BadJson(e) => write!(f, "Surum bildirimi okunamadi: {e}"),
        }
    }
}

impl std::error::Error for VersionError {}

/// Açılış kararı: kabuk ne yapmalı?
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootGate {
    /// Güncel, devam.
    Allow,
    /// Yeni (zorunlu) sürüm var: yeniden başlatmada kur, şimdi kesme.
    PendingRestart { latest: String },
    /// Taban altı: girişte engelle (iş bitince overlay gösterilir).
    BlockedBelowFloor { latest: String },
}

/// Broker'dan gelen ham JSON'u çözümle (önce şema, sonra karar).
pub fn parse_manifest(json: &str) -> Result<Manifest, VersionError> {
    serde_json::from_str(json).map_err(|e| VersionError::BadJson(e.to_string()))
}

/// Açılış kararı (saf: ağ yok, yalnızca karşılaştırma).
pub fn decide_boot(current: &str, manifest: &Manifest) -> BootGate {
    match decide(current, manifest) {
        UpdateDecision::Current => BootGate::Allow,
        UpdateDecision::PendingRestart => BootGate::PendingRestart {
            latest: manifest.version.clone(),
        },
        UpdateDecision::BlockedBelowFloor => BootGate::BlockedBelowFloor {
            latest: manifest.version.clone(),
        },
    }
}

/// Tek adım: JSON + mevcut sürüm → kapı kararı.
pub fn check_at_startup(current: &str, manifest_json: &str) -> Result<BootGate, VersionError> {
    Ok(decide_boot(current, &parse_manifest(manifest_json)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn manifest_json(version: &str, min_supported: &str) -> String {
        serde_json::to_string(&Manifest {
            version: version.into(),
            min_supported: min_supported.into(),
            url: "https://broker/v1/feed/1.msi".into(),
            sha256_hex: String::new(),
            signature_hex: String::new(),
            mandatory: true,
            notes: "n".into(),
        })
        .unwrap()
    }

    #[test]
    fn floor_blocks_old_client_at_entry() {
        // Taban-altı-bloke: girişte güncelleme ister.
        let gate = check_at_startup("1.0.5", &manifest_json("1.2.0", "1.1.0")).unwrap();
        assert_eq!(
            gate,
            BootGate::BlockedBelowFloor {
                latest: "1.2.0".into()
            }
        );
    }

    #[test]
    fn mandatory_update_is_pending_not_blocking() {
        let gate = check_at_startup("1.1.0", &manifest_json("1.2.0", "1.1.0")).unwrap();
        assert_eq!(
            gate,
            BootGate::PendingRestart {
                latest: "1.2.0".into()
            }
        );
        let gate = check_at_startup("1.2.0", &manifest_json("1.2.0", "1.1.0")).unwrap();
        assert_eq!(gate, BootGate::Allow);
    }

    #[test]
    fn bad_json_is_an_error_not_a_gate() {
        assert!(matches!(
            check_at_startup("1.0.0", "{bozuk"),
            Err(VersionError::BadJson(_))
        ));
    }

    #[test]
    fn endpoint_is_broker_version_feed() {
        assert_eq!(VERSION_ENDPOINT, "/v1/version");
        assert_eq!(
            endpoint_url("https://broker.example/"),
            "https://broker.example/v1/version"
        );
    }
}
