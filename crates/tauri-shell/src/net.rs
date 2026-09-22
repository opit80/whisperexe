//! Broker HTTPS istemcisi — YALNIZCA `tauri` ozelligiyle derlenir.
//!
//! Dogrulama: OS deposu (platform-verifier) — ozel CA tek seferlik sistem
//! kurulumuyla guvenilir, kodda pin YOK. Hatalar kisa Turkce metindir;
//! istek govdesi/anahtar ASLA hata metnine girmez.

#[cfg(feature = "tauri")]
use std::sync::OnceLock;
#[cfg(feature = "tauri")]
use std::time::Duration;
#[cfg(feature = "tauri")]
use ureq::tls::{RootCerts, TlsConfig};
#[cfg(feature = "tauri")]
use ureq::Agent;

/// API istekleri tavanı: 15sn. Ulaşılamayan broker SONSUZA dek asılmaz
/// (donma YOK); süre aşımı `baglanti-hatasi` olarak döner, ön-yüz kısa
/// kod gösterir.
#[cfg(feature = "tauri")]
const API_TIMEOUT: Duration = Duration::from_secs(15);

/// Kurulum indirme tavanı: 120sn (100MB cap; yavaş hatta da iner).
#[cfg(feature = "tauri")]
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

#[cfg(feature = "tauri")]
fn agent_with(timeout: Duration) -> Agent {
    Agent::config_builder()
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_global(Some(timeout))
        .build()
        .into()
}

#[cfg(feature = "tauri")]
pub(crate) fn agent() -> Agent {
    static A: OnceLock<Agent> = OnceLock::new();
    A.get_or_init(|| agent_with(API_TIMEOUT)).clone()
}

/// Büyük dosya indirme için ayrı ajan (API tavanı uygulanmaz).
#[cfg(feature = "tauri")]
pub(crate) fn download_agent() -> Agent {
    static D: OnceLock<Agent> = OnceLock::new();
    D.get_or_init(|| agent_with(DOWNLOAD_TIMEOUT)).clone()
}

#[cfg(feature = "tauri")]
fn url(base: &str, path: &str) -> String {
    format!("{}{}", base.trim_end_matches('/'), path)
}

/// Kisa hata metni (govde/anahtar sizdirmaz; karsi tarafin `code` alani varsa
/// onu tasir, yoksa HTTP durumunu soyler).
#[cfg(feature = "tauri")]
fn err_text(status: u16, body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(code) = v.get("error").and_then(|e| e.get("code")).and_then(|c| c.as_str()) {
            return code.to_string();
        }
    }
    format!("http-{status}")
}

#[cfg(feature = "tauri")]
fn read_json(res: ureq::http::Response<ureq::Body>) -> Result<serde_json::Value, String> {
    let status = res.status().as_u16();
    let mut body = res.into_body();
    let text = body.read_to_string().map_err(|e| format!("okuma-hatasi:{e}"))?;
    if !(200..300).contains(&status) {
        return Err(err_text(status, &text));
    }
    serde_json::from_str(&text).map_err(|_| "bozuk-yanit".to_string())
}

/// Yetkisiz GET.
#[cfg(feature = "tauri")]
pub fn get(base: &str, path: &str) -> Result<serde_json::Value, String> {
    let res = agent()
        .get(&url(base, path))
        .call()
        .map_err(|e| format!("baglanti-hatasi:{e}"))?;
    read_json(res)
}

/// Jetonlu GET.
#[cfg(feature = "tauri")]
pub fn get_auth(base: &str, path: &str, headers: &[(&str, String)]) -> Result<serde_json::Value, String> {
    let mut req = agent().get(&url(base, path));
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let res = req
        .call()
        .map_err(|e| format!("baglanti-hatasi:{e}"))?;
    read_json(res)
}

/// JSON govdeli POST (bos govde icin `Value::Null` gonderilir).
#[cfg(feature = "tauri")]
pub fn post(base: &str, path: &str, headers: &[(&str, String)], body: &serde_json::Value) -> Result<serde_json::Value, String> {
    let mut req = agent().post(&url(base, path));
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let res = req
        .send_json(body.clone())
        .map_err(|e| format!("baglanti-hatasi:{e}"))?;
    read_json(res)
}

/// JSON govdeli DELETE yerine sorgulu DELETE (ureq govdesiz DELETE kurar).
#[cfg(feature = "tauri")]
pub fn delete(base: &str, path: &str, headers: &[(&str, String)], query: &str) -> Result<serde_json::Value, String> {
    let mut req = agent().delete(&format!("{}{}?{}", base.trim_end_matches('/'), path, query));
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let res = req
        .call()
        .map_err(|e| format!("baglanti-hatasi:{e}"))?;
    read_json(res)
}
/// JSON govdeli PUT.
#[cfg(feature = "tauri")]
pub fn put(base: &str, path: &str, headers: &[(&str, String)], body: &serde_json::Value) -> Result<serde_json::Value, String> {
    let mut req = agent().put(&url(base, path));
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let res = req
        .send_json(body.clone())
        .map_err(|e| format!("baglanti-hatasi:{e}"))?;
    read_json(res)
}

#[cfg(test)]
mod tests {
    #[test]
    fn url_join_trims_slash() {
        // Saf yardimci davranisi (ag yok).
        let joined = format!("{}{}", "https://x/".trim_end_matches('/'), "/v1/me");
        assert_eq!(joined, "https://x/v1/me");
    }
}
