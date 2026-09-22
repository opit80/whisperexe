//! Panel API uçları: broker'ın HTTP katmanına bağlayacağı rota tablosu.
//!
//! Bu crate saf mantıktır (HTTP sunucusu yok); broker her rotayı aşağıdaki
//! `ROUTES` tablosundaki işleve bağlar. Tüm admin işlemleri denetim
//! günlüğüne (`AuditLog`) yazılır.

use serde::{Deserialize, Serialize};

/// (yöntem, yol, açıklama) — panel API özeti.
pub const ROUTES: &[(&str, &str, &str)] = &[
    ("POST", "/v1/admin/setup", "kurulum: yerel admin sifresi (bir kez, argon2)"),
    ("POST", "/v1/admin/login", "admin girisi (kaba-kuvvet kilitli)"),
    ("POST", "/v1/admin/password", "admin sifre degisimi"),
    ("POST", "/v1/invites", "hesap acma: kullanici + acilis bakiyesi -> tek kullanimlik davet"),
    ("POST", "/v1/redeem", "davetle ilk giris: sifre belirleme + HWID 1. slot"),
    ("GET", "/v1/users", "kullanici tablosu (cevrimici, kullanim ev/fallback, son aktiflik, surum, bakiye)"),
    ("GET", "/v1/users/{u}", "kullanici satiri detayi"),
    ("POST", "/v1/users/{u}/topup", "bakiye ekle (pilot: havale sonrasi elle)"),
    ("PUT", "/v1/users/{u}/limits", "limit koy: gunluk/aylik, hat bazinda"),
    ("PUT", "/v1/users/{u}/fallback-cap", "fallback gunluk tavani"),
    ("PUT", "/v1/users/{u}/home-only", "sadece-ev modu ac/kapat"),
    ("POST", "/v1/users/{u}/hwid-reset", "HWID sifirla (loglu + sik reset uyarisi)"),
    ("POST", "/v1/users/{u}/suspend", "hesabi durdur / ac"),
    ("POST", "/v1/users/{u}/force-update", "guncellemeye zorla (hesap tabani)"),
    ("GET", "/v1/tariffs", "guncel tarife (ev TL/dk + fallback)"),
    ("PUT", "/v1/tariffs", "tarife degisimi (sonraki isteklere uygulanir)"),
    ("GET", "/v1/fallback/switch", "acil salter durumu"),
    ("POST", "/v1/fallback/switch", "acil salter: fallback harcamasini durdur/baslat"),
    ("GET", "/v1/fallback/vendor", "aktif fallback hatti + hat fiyatlari + anahtar VAR/YOK"),
    ("POST", "/v1/fallback/vendor", "hat secimi: groq | openai | local (sonraki isteklere uygulanir)"),
    ("PUT", "/v1/fallback/tariffs", "hat fiyati: vendor + carpan/sabit (hat secilince uygulanir)"),
    ("PUT", "/v1/fallback/upstream", "hat tabani: vendor + saniye (en az 3sn)"),
    ("POST", "/v1/fallback/keys", "saglayici anahtari gir (bellek-ici, loga yazilmaz)"),
    ("DELETE", "/v1/fallback/keys", "saglayici anahtari sil"),
    ("POST", "/v1/releases", "imzali .msi yayinlama (surum + not + zorunlu + taban)"),
    ("GET", "/v1/feed", "Tauri updater beslemesi (broker'dan)"),
    ("GET", "/v1/version-check", "istemci surum kontrolu (acilista)"),
    ("GET", "/v1/disks", "disk durumu (isci/broker/arsiv, %80 uyarisi)"),
    ("PUT", "/v1/disks", "disk doluluk bildirimi"),
    ("GET", "/v1/users/{u}/retention", "saklama durumu (varsayilan ACIK)"),
    ("PUT", "/v1/users/{u}/retention", "hesap bazinda saklama kapatma"),
    ("DELETE", "/v1/users/{u}/data", "KVKK hard-delete (ses + metin + onbellek, loglu)"),
    ("GET", "/v1/audit", "admin islem gunlugu"),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub at: u64,
    pub admin: String,
    pub action: String,
    pub detail: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AuditLog {
    pub entries: Vec<AuditEntry>,
}

impl AuditLog {
    pub fn record(&mut self, at: u64, admin: &str, action: &str, detail: &str) {
        self.entries.push(AuditEntry {
            at,
            admin: admin.to_string(),
            action: action.to_string(),
            detail: detail.to_string(),
        });
    }
}
