//! whisperexe Tauri derleme kancası.
//!
//! `tauri` özelliği KAPALIYKEN hiçbir şey yapmaz: `cargo build/test -p tauri-shell`
//! aynen çalışır. `tauri` özelliği AÇIKKEN (`cargo tauri build` dahil):
//!  1. `TAURI_BROKER_HOST` + `TAURI_UPDATER_PUBKEY` ortam değişkenlerini ZORUNLU
//!     kılar (yoksa/geçersizse derleme AÇIK bir hatayla durur).
//!     Host kuralı: `https://` şarttır; YALNIZCA yerel test için
//!     `http://127.0.0.1:...` / `http://localhost:...` ve ev-dışı kurulum
//!     için `http://95.70.173.199:...` kabul edilir,
//!  2. doğrulanmış değerleri `tauri.conf.json` içine yazar
//!     (`__TAURI_BROKER_HOST__` / `__TAURI_UPDATER_PUBKEY__` + sürüm),
//!  3. `tauri_build::build()` çalıştırır (Tauri v2 codegen).
//!
//! Gerçek host/pubkey ASLA hardcode edilmez. ÖZEL imza anahtarı bu dosyaya,
//! ortama ya da repoya GİRMEZ: imzalama, anahtarı elinde tutanın release
//! makinesinde `TAURI_SIGNING_PRIVATE_KEY` ile yapılır (bkz. docs/TAURI.md).

#[cfg(feature = "tauri")]
use std::path::PathBuf;

#[cfg(feature = "tauri")]
fn manifest_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR yok"))
        .join(name)
}

#[cfg(feature = "tauri")]
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string())
}

/// Placeholder kalıntısı mı (işlenmemiş şablon değeri)?
#[cfg(feature = "tauri")]
fn looks_like_placeholder(v: &str) -> bool {
    v.contains("__TAURI_") || v.contains("BROKER") || v.contains("EDIT")
}

#[cfg(feature = "tauri")]
fn read_required_env() -> (String, String) {
    let host = env_var("TAURI_BROKER_HOST").unwrap_or_default();
    if host.is_empty() {
        panic!(
            "tauri-shell: TAURI_BROKER_HOST yok. \
             Ornek: $env:TAURI_BROKER_HOST='https://broker.ornek.test' \
             (broker taban URL'i; feed ucuna /v1/feed eklenir)."
        );
    }
    let https = host.starts_with("https://");
    // Sifresiz yalnizca yerel teste izin ver (uretim derlemesi https kalir).
    // Ev-dis-IP istisnasi: sahip guvenlik sartini kaldirdi; arkadas kurulumu
    // dogrudan bu makineye http ile baglanir (bkz. modem 8899 yonlendirmesi).
    let http_ev = host.starts_with("http://95.70.173.199:");
    let http_loopback = host.starts_with("http://") && {
        let rest = host.trim_start_matches("http://");
        rest == "localhost"
            || rest.starts_with("localhost:")
            || rest.starts_with("localhost/")
            || rest == "127.0.0.1"
            || rest.starts_with("127.0.0.1:")
            || rest.starts_with("127.0.0.1/")
            || rest == "[::1]"
            || rest.starts_with("[::1]:")
            || rest.starts_with("[::1]/")
    };
    if (!https && !http_loopback && !http_ev) || host.contains(char::is_whitespace) {
        panic!(
            "tauri-shell: TAURI_BROKER_HOST gecersiz ({host:?}). \
             'https://' ile baslamali (yerel test 'http://127.0.0.1:...', \
             ev-disi 'http://95.70.173.199:...' olur), bosluk icermemeli."
        );
    }
    if looks_like_placeholder(&host) {
        panic!("tauri-shell: TAURI_BROKER_HOST placeholder iceriyor ({host:?}).");
    }
    let host = host.trim_end_matches('/').to_string();

    // Güncelleme beslemesi: varsayılan broker `/v1/feed`; `TAURI_FEED_URL`
    // verilirse (örn. GitHub latest.json) o kullanılır.

    let pubkey = env_var("TAURI_UPDATER_PUBKEY").unwrap_or_default();
    if pubkey.is_empty() {
        panic!(
            "tauri-shell: TAURI_UPDATER_PUBKEY yok. \
             Offline imza anahtarinin PUBLIC karsiligini verin \
             (ozel anahtar ASLA buraya girmez)."
        );
    }
    if pubkey.len() < 32
        || pubkey.contains(char::is_whitespace)
        || looks_like_placeholder(&pubkey)
    {
        panic!("tauri-shell: TAURI_UPDATER_PUBKEY bicimi gecersiz (bosluk/placeholder olmamali, >=32 karakter).");
    }
    (host, pubkey)
}

/// Doğrulanmış env değerlerini `tauri.conf.json`'a yazar (değişmediyse dokunmaz).
#[cfg(feature = "tauri")]
fn render_tauri_conf(host: &str, pubkey: &str) {
    let feed = match env_var("TAURI_FEED_URL").map(|v| v.trim().to_string()) {
        Some(f) if !f.is_empty() && !looks_like_placeholder(&f) => f,
        _ => format!("{host}/v1/feed"),
    };
    let path = manifest_path("tauri.conf.json");
    let raw = std::fs::read_to_string(&path).expect("tauri.conf.json okunamadi");
    let mut conf: serde_json::Value =
        serde_json::from_str(&raw).expect("tauri.conf.json JSON degil");
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION yok");
    conf["version"] = serde_json::Value::String(version);
    conf["plugins"]["updater"]["endpoints"] = serde_json::json!([feed]);
    conf["plugins"]["updater"]["pubkey"] = serde_json::Value::String(pubkey.to_string());
    let rendered = serde_json::to_string_pretty(&conf).expect("tauri.conf.json serilestirilemedi") + "\n";
    if rendered != raw {
        std::fs::write(&path, rendered).expect("tauri.conf.json yazilamadi");
    }
    println!("cargo:rustc-env=WHISPER_BROKER_BASE={host}");
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-env-changed=TAURI_BROKER_HOST");
    println!("cargo:rerun-if-env-changed=TAURI_UPDATER_PUBKEY");
    println!("cargo:rerun-if-env-changed=TAURI_FEED_URL");

    #[cfg(feature = "tauri")]
    {
        let (host, pubkey) = read_required_env();
        render_tauri_conf(&host, &pubkey);
        tauri_build::build();
    }
}
