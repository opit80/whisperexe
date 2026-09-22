//! lan-e2e (G4): LAN uçtan uca entegrasyon kanıtı.
//!
//! Üç test, üçü de gerçek TCP turu (ham `TcpStream` + HTTP/1.1, TLS YOK):
//! 1. `version_feed_allows_current` — broker `/v1/version` + shell kararı.
//! 2. `redeem_login_transcribe_replay` — davet→giriş→ücretli iş→ücretsiz replay.
//! 3. `poor_and_silent_keep_balance` — 402 + sessizlik ücretsiz reti, bakiye sabit.
//!
//! Her test kendi broker'ını ephemeral portta başlatır
//! (`127.0.0.1:0`'dan alınan port broker'a argümanla verilir), kendi temp
//! ledger dosyasını kullanır ve sonunda siler (panikte bile: `BrokerProc::drop`).
//!
//! G1 LAN notu (doğrulanan): kuyruk senkron drene edilir (bekleme yok,
//! transcribe tek turda ücretli sonuca ulaşır), hat her zaman evdir
//! (mock işçi metni + ev tarifesi 120 kr/dk üzerinden 3sn quantum → 6 kuruş).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Broker süreci + ham HTTP/1.1 istemcisi
// ---------------------------------------------------------------------------

struct BrokerProc {
    child: Child,
    addr: String,
    ledger: PathBuf,
}

impl Drop for BrokerProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.ledger);
    }
}

fn broker_exe() -> PathBuf {
    if let Ok(p) = std::env::var("LAN_E2E_BROKER_EXE") {
        let pb = PathBuf::from(&p);
        if pb.exists() {
            return pb;
        }
        panic!("LAN_E2E_BROKER_EXE yok: {p}");
    }
    let exe_name = if cfg!(windows) { "broker.exe" } else { "broker" };
    // Entegrasyon testi ikilisi `target/debug/deps/` altındadır; broker
    // ikilisi iki üstte, `target/debug/` içindedir.
    if let Ok(cur) = std::env::current_exe() {
        if let Some(debug) = cur.parent().and_then(|p| p.parent()) {
            let cand = debug.join(exe_name);
            if cand.exists() {
                return cand;
            }
        }
    }
    // Yedek: workspace köküne göre `target/debug/`.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(root) = manifest.parent().and_then(|p| p.parent()) {
        let cand = root.join("target").join("debug").join(exe_name);
        if cand.exists() {
            return cand;
        }
    }
    panic!(
        "broker ikilisi bulunamadi; once `cargo build -p broker` calistirin \
         (veya LAN_E2E_BROKER_EXE ile yol verin)"
    );
}

fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").expect("ephemeral port alinmali");
    let p = l.local_addr().expect("yerel adres").port();
    drop(l);
    p
}

fn spawn_broker(tag: &str) -> BrokerProc {
    let exe = broker_exe();
    let mut last_err = String::from("deneme yok");
    // Paralel başlangıçta port çakışması gibi geçici durumlara karşı
    // farklı ephemeral portlarla birkaç kez dene.
    for attempt in 0..5 {
        let port = free_port();
        let addr = format!("127.0.0.1:{port}");
        let ledger = std::env::temp_dir().join(format!(
            "lan-e2e-{}-{}-{}-{}.jsonl",
            std::process::id(),
            tag,
            port,
            attempt
        ));
        let _ = std::fs::remove_file(&ledger);
        let child = Command::new(&exe)
            .arg(&addr)
            .env("BROKER_SECRET", "lan-e2e-test-secret-0123456789abcdef")
            .env("BROKER_LEDGER_PATH", &ledger)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("broker baslatilamadi ({}): {e}", exe.display()));
        let mut proc = BrokerProc { child, addr, ledger };
        // Hazır olana kadar yokla (sağlık ucu 200 dönmeli).
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut ready = false;
        loop {
            match http_raw(&proc.addr, "GET", "/", &[], &[]) {
                Ok((200, _)) => {
                    ready = true;
                    break;
                }
                _ => {
                    // Broker erken öldüyse boşuna bekleme, yeni portla dene.
                    match proc.child.try_wait() {
                        Ok(Some(st)) => {
                            last_err = format!("erken cikti ({st})");
                            break;
                        }
                        Ok(None) => {}
                        Err(e) => {
                            last_err = format!("saglik kontrolu: {e}");
                            break;
                        }
                    }
                }
            }
            if Instant::now() > deadline {
                last_err = format!("10sn'de hazir olmadi: {}", proc.addr);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if ready {
            return proc;
        }
        let _ = proc.child.kill();
        let _ = proc.child.wait();
        let _ = std::fs::remove_file(&proc.ledger);
        std::thread::sleep(Duration::from_millis(100 * (attempt + 1) as u64));
    }
    panic!("broker baslatilamadi (5 deneme, exe={}): {last_err}", exe.display());
}

/// Ham TCP HTTP/1.1 turu. Dönen: (durum kodu, ham gövde baytları).
fn http_raw(
    addr: &str, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8],
) -> std::io::Result<(u16, Vec<u8>)> {
    let mut s = TcpStream::connect(addr)?;
    s.set_read_timeout(Some(Duration::from_secs(20)))?;
    let mut req = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nContent-Length: {}\r\n",
        method,
        path,
        addr,
        body.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{}: {}\r\n", k, v));
    }
    req.push_str("Connection: close\r\n\r\n");
    s.write_all(req.as_bytes())?;
    s.write_all(body)?;
    let mut out = Vec::new();
    s.read_to_end(&mut out)?;
    let sep = out
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("HTTP yanit ayraci bulunmali");
    let head = String::from_utf8_lossy(&out[..sep]).to_string();
    let status: u16 = head.split_whitespace().nth(1).unwrap_or("0").parse().unwrap_or(0);
    Ok((status, out[sep + 4..].to_vec()))
}

fn jpost(b: &BrokerProc, path: &str, headers: &[(&str, &str)], v: &Value) -> (u16, Value) {
    let body = v.to_string().into_bytes();
    let mut h = vec![("Content-Type", "application/json")];
    h.extend_from_slice(headers);
    let (st, rb) = http_raw(&b.addr, "POST", path, &h, &body).expect("TCP turu");
    let j: Value = serde_json::from_slice(&rb).unwrap_or(Value::Null);
    (st, j)
}

fn jget(b: &BrokerProc, path: &str, headers: &[(&str, &str)]) -> (u16, Value) {
    let (st, rb) = http_raw(&b.addr, "GET", path, headers, &[]).expect("TCP turu");
    let j: Value = serde_json::from_slice(&rb).unwrap_or(Value::Null);
    (st, j)
}

fn sha_hex(data: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let dg = Sha256::digest(data);
    let mut s = String::with_capacity(64);
    for x in dg {
        s.push(H[(x >> 4) as usize] as char);
        s.push(H[(x & 15) as usize] as char);
    }
    s
}

/// Konuşma enerjili sahte gövde: 0x40 dolgusu (RMS ~0.5, sessizlik değil).
/// 3200 bayt = 1600 örnek / 16kHz = 0.1sn → 3sn quantum ile faturalandırılır.
fn speech_body() -> Vec<u8> {
    vec![0x40u8; 3200]
}

fn admin_setup_and_login(b: &BrokerProc) -> String {
    let (st, v) = jpost(b, "/v1/admin/setup", &[], &json!({"password": "yonetici-sifresi-123"}));
    assert_eq!(st, 200, "admin setup: {v}");
    let (st, v) = jpost(b, "/v1/admin/login", &[], &json!({"password": "yonetici-sifresi-123"}));
    assert_eq!(st, 200, "admin login: {v}");
    v["admin_token"].as_str().expect("admin_token").to_string()
}

fn ledger_balance(b: &BrokerProc, admin: &str, user: &str) -> i64 {
    let (st, v) = jget(b, &format!("/v1/users/{user}"), &[("X-Admin-Token", admin)]);
    assert_eq!(st, 200, "kullanici detay: {v}");
    v["ledger_balance_krs"].as_i64().expect("ledger_balance_krs")
}

// ---------------------------------------------------------------------------
// Test 1: version-feed + shell açılış kararı
// ---------------------------------------------------------------------------

#[test]
fn version_feed_allows_current() {
    let b = spawn_broker("version");

    // 1) Gerçek TCP: broker sürüm ucu.
    let (st, raw) = http_raw(&b.addr, "GET", "/v1/version", &[], &[]).expect("TCP turu");
    assert_eq!(st, 200, "GET /v1/version: {}", String::from_utf8_lossy(&raw));
    let v: Value = serde_json::from_slice(&raw).expect("version JSON olmali");
    assert_eq!(v["ok"], json!(true));

    // Broker'ın bildirdiği sürüm + taban (LAN fazında yayın yoksa
    // "0.0.0-dev" + boş taban döner; ikisi de broker'ın kelimesidir).
    let broker_version = v
        .get("feed")
        .and_then(|f| f.get("version"))
        .and_then(|x| x.as_str())
        .or_else(|| v.get("version").and_then(|x| x.as_str()))
        .unwrap_or("0.0.0-dev")
        .to_string();
    let floor = v.get("floor_version").and_then(|x| x.as_str()).unwrap_or("").to_string();

    // LAN-fazı şekil notu: `/v1/version` Tauri besleme şeklindedir
    // (`feed`/`version`/`floor_version`), istemci `Manifest` şekliyle
    // birebir değildir; bu yüzden test broker'ın bildirdiği sürüm+tabanı
    // Manifest'e taşıyıp kabuk kararını doğrular (kelime broker'dan gelir).
    let min_supported = if floor.is_empty() { broker_version.clone() } else { floor.clone() };
    let manifest_json = json!({
        "version": broker_version,
        "min_supported": min_supported,
        "url": "https://broker.local/v1/feed/lan-e2e.msi",
        "sha256_hex": "",
        "signature_hex": "",
        "mandatory": false,
        "notes": format!("lan-e2e: broker {broker_version} taban {min_supported}"),
    })
    .to_string();

    // 2) Shell akışı: parse_manifest + decide → Allow.
    let m = tauri_shell::version::parse_manifest(&manifest_json).expect("manifest cozulmeli");
    let gate = tauri_shell::version::decide_boot(&broker_version, &m);
    assert_eq!(
        gate,
        tauri_shell::version::BootGate::Allow,
        "guncel istemci (broker {broker_version}) engellenmemeli"
    );
}

// ---------------------------------------------------------------------------
// Test 2: redeem → login → transcribe (ücretli) → replay (ücretsiz)
// ---------------------------------------------------------------------------

#[test]
fn redeem_login_transcribe_replay() {
    let b = spawn_broker("happy");
    let admin = admin_setup_and_login(&b);
    let ah = [("X-Admin-Token", admin.as_str())];

    // Davet → redeem → login (üçü de gerçek TCP).
    let (st, v) = jpost(&b, "/v1/invites", &ah, &json!({"username": "ali", "opening_krs": 10000}));
    assert_eq!(st, 200, "davet: {v}");
    let code = v["code"].as_str().expect("code").to_string();

    let (st, v) = jpost(
        &b,
        "/v1/redeem",
        &[],
        &json!({"code": code, "password": "gizli-sifre-1", "hwid": "hwid-A"}),
    );
    assert_eq!(st, 200, "redeem: {v}");
    let access = v["access"].as_str().expect("access").to_string();
    assert_eq!(v["balance_kurus"], json!(10000));

    let (st, v) = jpost(
        &b,
        "/v1/login",
        &[],
        &json!({"account": "ali", "password": "gizli-sifre-1", "hwid": "hwid-A"}),
    );
    assert_eq!(st, 200, "login: {v}");

    // Ücretli transcribe (konuşma enerjili gövde).
    let body = speech_body();
    let h = sha_hex(&body);
    let auth = format!("Bearer {access}");
    let hd = [
        ("Authorization", auth.as_str()),
        ("X-Hwid", "hwid-A"),
        ("X-Request-Id", "e2e-req-1"),
        ("X-Audio-Hash", h.as_str()),
        ("Content-Type", "application/octet-stream"),
    ];
    let bal_before = ledger_balance(&b, &admin, "ali");
    let (st, rb) = http_raw(&b.addr, "POST", "/v1/transcribe", &hd, &body).expect("TCP turu");
    assert_eq!(st, 200, "transcribe: {}", String::from_utf8_lossy(&rb));
    let r1: Value = serde_json::from_slice(&rb).expect("transcribe JSON");
    assert_eq!(r1["ok"], json!(true));
    assert_eq!(r1["cached"], json!(false));
    let text1 = r1["text"].as_str().expect("text").to_string();
    assert!(!text1.trim().is_empty(), "mock isci metin donmeli");
    // PLAN §3: 0.1sn ses → 3sn quantum; ev 120 kr/dk → 6 kuruş.
    // G1 LAN notu: hat ev (ev tarifesi + işçi mock metni).
    assert_eq!(r1["billed_secs"], json!(3), "quantum 3sn: {r1}");
    assert_eq!(r1["cost_kurus"], json!(6), "ev tarifesi: {r1}");
    assert!(r1["cost_kurus"].as_i64().unwrap() > 0, "ucret dolu olmali");
    let bal_after_first = ledger_balance(&b, &admin, "ali");
    assert_eq!(bal_after_first, bal_before - 6, "tek ücret düşmeli");

    // Aynı ID + aynı ses → önbellekten ücretsiz (kuyruk senkron drendiği
    // için ikinci tur da tek turda döner; çift ücret yazılmaz).
    let (st, rb) = http_raw(&b.addr, "POST", "/v1/transcribe", &hd, &body).expect("TCP turu");
    assert_eq!(st, 200, "replay: {}", String::from_utf8_lossy(&rb));
    let r2: Value = serde_json::from_slice(&rb).expect("replay JSON");
    assert_eq!(r2["cached"], json!(true));
    assert_eq!(r2["cost_kurus"], json!(0));
    assert_eq!(r2["billed_secs"], json!(0));
    assert_eq!(r2["text"], json!(text1));
    let bal_after_replay = ledger_balance(&b, &admin, "ali");
    assert_eq!(bal_after_replay, bal_after_first, "replay bakiyeyi kımıldatmamalı");
}

// ---------------------------------------------------------------------------
// Test 3: sıfır bakiye 402 + sessiz gövde ücretsiz ret (bakiyeler sabit)
// ---------------------------------------------------------------------------

#[test]
fn poor_and_silent_keep_balance() {
    let b = spawn_broker("poor");
    let admin = admin_setup_and_login(&b);
    let ah = [("X-Admin-Token", admin.as_str())];

    // Sıfır bakiyeli hesap.
    let (st, v) = jpost(&b, "/v1/invites", &ah, &json!({"username": "yoksul", "opening_krs": 0}));
    assert_eq!(st, 200, "davet: {v}");
    let code0 = v["code"].as_str().expect("code").to_string();
    let (st, v) = jpost(
        &b,
        "/v1/redeem",
        &[],
        &json!({"code": code0, "password": "gizli-sifre-1", "hwid": "hwid-A"}),
    );
    assert_eq!(st, 200, "redeem: {v}");
    let access0 = v["access"].as_str().expect("access").to_string();

    // Sessizlik kapısını ölçmek için bakiyeli ikinci hesap (sıfır bakiyede
    // ön-kontrol kapıdan önce devreye girer; sessizlik yolu bakiye ister).
    let (st, v) = jpost(&b, "/v1/invites", &ah, &json!({"username": "konusur", "opening_krs": 10000}));
    assert_eq!(st, 200, "davet: {v}");
    let code1 = v["code"].as_str().expect("code").to_string();
    let (st, v) = jpost(
        &b,
        "/v1/redeem",
        &[],
        &json!({"code": code1, "password": "gizli-sifre-1", "hwid": "hwid-B"}),
    );
    assert_eq!(st, 200, "redeem: {v}");
    let access1 = v["access"].as_str().expect("access").to_string();

    // 1) Sıfır bakiye + konuşma gövdesi → 402, bakiye 0'da sabit.
    let body = speech_body();
    let h = sha_hex(&body);
    let auth0 = format!("Bearer {access0}");
    let hd0 = [
        ("Authorization", auth0.as_str()),
        ("X-Hwid", "hwid-A"),
        ("X-Request-Id", "e2e-poor-1"),
        ("X-Audio-Hash", h.as_str()),
        ("Content-Type", "application/octet-stream"),
    ];
    let poor_before = ledger_balance(&b, &admin, "yoksul");
    assert_eq!(poor_before, 0);
    let (st, rb) = http_raw(&b.addr, "POST", "/v1/transcribe", &hd0, &body).expect("TCP turu");
    assert_eq!(st, 402, "sifir bakiye reddedilmeli: {}", String::from_utf8_lossy(&rb));
    let e: Value = serde_json::from_slice(&rb).expect("hata JSON");
    assert_eq!(e["error"]["code"], json!("insufficient_balance"));
    assert_eq!(ledger_balance(&b, &admin, "yoksul"), 0, "402 ucretsiz: bakiye sabit");

    // 2) Bakiyeli hesap + sessiz gövde → 400 sessizlik reti, bakiye sabit.
    let silent = vec![0u8; 3200];
    let hs = sha_hex(&silent);
    let auth1 = format!("Bearer {access1}");
    let hd1 = [
        ("Authorization", auth1.as_str()),
        ("X-Hwid", "hwid-B"),
        ("X-Request-Id", "e2e-silent-1"),
        ("X-Audio-Hash", hs.as_str()),
        ("Content-Type", "application/octet-stream"),
    ];
    let rich_before = ledger_balance(&b, &admin, "konusur");
    let (st, rb) = http_raw(&b.addr, "POST", "/v1/transcribe", &hd1, &silent).expect("TCP turu");
    assert_eq!(st, 400, "sessizlik reddedilmeli: {}", String::from_utf8_lossy(&rb));
    let e: Value = serde_json::from_slice(&rb).expect("hata JSON");
    assert_eq!(e["error"]["code"], json!("silent_audio"));
    assert_eq!(
        ledger_balance(&b, &admin, "konusur"),
        rich_before,
        "sessizlik ucretsiz: bakiye sabit"
    );
}
