//! Saklama + KVKK hard-delete + disk izleme (PLAN §3).
//!
//! - Saklama varsayılan AÇIK (hesap `retention_opt_out=false`); kapatma
//!   bayrağı hesap bazında kodda hazır.
//! - KVKK silme talebi panelden hard-delete ile yerine getirilir
//!   (ses + metin + önbellek; işlem loglanır).
//! - Disk %80 uyarısı işçi / broker / arşiv disklerinde ayrı izlenir.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskKind {
    Worker,
    Broker,
    Archive,
}

impl DiskKind {
    pub fn name(&self) -> &'static str {
        match self {
            DiskKind::Worker => "isçi",
            DiskKind::Broker => "broker",
            DiskKind::Archive => "arsiv",
        }
    }
}

/// %80 doluluk uyarı eşiği.
pub const DISK_WARN_PCT: u8 = 80;

#[derive(Debug, Clone)]
pub struct DiskStatus {
    pub kind: DiskKind,
    /// Doluluk yüzdesi 0..=100.
    pub used_pct: u8,
}

impl DiskStatus {
    pub fn warn(&self) -> bool {
        self.used_pct >= DISK_WARN_PCT
    }
}

#[derive(Debug, Default, Clone)]
pub struct DiskMonitor {
    pub disks: Vec<DiskStatus>,
}

impl DiskMonitor {
    /// Üç diskin her birinde ayrı uyarı üretir.
    pub fn warnings(&self) -> Vec<String> {
        self.disks
            .iter()
            .filter(|d| d.warn())
            .map(|d| format!("disk uyarisi: {} %{} dolu (esik %{})", d.kind.name(), d.used_pct, DISK_WARN_PCT))
            .collect()
    }
}

/// KVKK hard-delete raporu: silinen veri sınıfları + log kaydı.
/// (Gerçek dosya/önbellek silmeyi broker yapar; panel rapor + denetim izi üretir.)
#[derive(Debug, Clone)]
pub struct HardDeleteReport {
    pub username: String,
    pub at: u64,
    pub by_admin: String,
    pub removed: Vec<&'static str>,
}

pub fn hard_delete_plan(username: &str, admin: &str, now: u64) -> HardDeleteReport {
    HardDeleteReport {
        username: username.to_string(),
        at: now,
        by_admin: admin.to_string(),
        removed: vec![
            "cozulmus-wav",
            "opus-kopya",
            "transkript-metin",
            "idempotency-onbellek",
            "hesap-kaydi",
            "davet-kaydi",
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_warnings_per_disk() {
        let m = DiskMonitor {
            disks: vec![
                DiskStatus { kind: DiskKind::Worker, used_pct: 81 },
                DiskStatus { kind: DiskKind::Broker, used_pct: 50 },
                DiskStatus { kind: DiskKind::Archive, used_pct: 95 },
            ],
        };
        let w = m.warnings();
        assert_eq!(w.len(), 2);
        assert!(w[0].contains("isçi"));
        assert!(w[1].contains("arsiv"));
        // Sınır: %80 uyarır, %79 uyarmaz.
        assert!(DiskStatus { kind: DiskKind::Broker, used_pct: 80 }.warn());
        assert!(!DiskStatus { kind: DiskKind::Broker, used_pct: 79 }.warn());
    }

    #[test]
    fn hard_delete_covers_voice_text_cache() {
        let r = hard_delete_plan("ali", "admin", 999);
        assert!(r.removed.contains(&"cozulmus-wav"));
        assert!(r.removed.contains(&"transkript-metin"));
        assert!(r.removed.contains(&"idempotency-onbellek"));
    }
}
