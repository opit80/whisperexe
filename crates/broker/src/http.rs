//! Senkron minimal HTTP katmanı (std-only; tokio/axum YOK).
//!
//! LAN aşaması için yeterlidir: HTTP/1.1, `Content-Length` gövdeler,
//! her bağlantıda `Connection: close`. TLS YOK (plan dışı).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::state::{ApiError, BrokerState, esc, now_secs};

// ---------------------------------------------------------------------------
// İstek ayrıştırma
// ---------------------------------------------------------------------------

const MAX_HEADER_LINES: usize = 128;
const MAX_HEADER_LINE: usize = 8192;
const MAX_BODY: usize = 3 * 1024 * 1024;

struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &TcpStream) -> Result<Request, ApiError> {
    let mut rd = BufReader::new(stream);
    let mut line = String::new();
    rd.read_line(&mut line)
        .map_err(|_| ApiError::new(400, "bad_request", "istek okunamadi"))?;
    let parts: Vec<&str> = line.trim_end().splitn(3, ' ').collect();
    if parts.len() != 3 {
        return Err(ApiError::new(400, "bad_request", "hatali istek satiri"));
    }
    let method = parts[0].to_string();
    let target = parts[1].to_string();

    let mut headers = HashMap::new();
    for _ in 0..MAX_HEADER_LINES {
        let mut h = String::new();
        rd.read_line(&mut h)
            .map_err(|_| ApiError::new(400, "bad_request", "baslik okunamadi"))?;
        if h.len() > MAX_HEADER_LINE {
            return Err(ApiError::new(431, "headers_too_large", "baslik cok buyuk"));
        }
        let h = h.trim_end().to_string();
        if h.is_empty() {
            break;
        }
        if let Some(i) = h.find(':') {
            headers.insert(h[..i].trim().to_lowercase(), h[i + 1..].trim().to_string());
        }
    }
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if len > MAX_BODY {
        return Err(ApiError::new(413, "payload_too_large", "govde tavani asti"));
    }
    let mut body = vec![0u8; len];
    if len > 0 {
        rd.read_exact(&mut body)
            .map_err(|_| ApiError::new(400, "bad_request", "govde eksik"))?;
    }
    let (path, query) = match target.find('?') {
        Some(i) => (target[..i].to_string(), parse_query(&target[i + 1..])),
        None => (target, HashMap::new()),
    };
    Ok(Request { method, path, query, headers, body })
}

fn parse_query(q: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for kv in q.split('&') {
        if kv.is_empty() {
            continue;
        }
        match kv.find('=') {
            Some(i) => {
                m.insert(kv[..i].to_string(), kv[i + 1..].to_string());
            }
            None => {
                m.insert(kv.to_string(), String::new());
            }
        }
    }
    m
}

// ---------------------------------------------------------------------------
// Yönlendirme
// ---------------------------------------------------------------------------

pub struct Server {
    pub state: Arc<Mutex<BrokerState>>,
}

impl Server {
    pub fn new(state: Arc<Mutex<BrokerState>>) -> Self {
        Self { state }
    }

    pub fn serve_forever(&self, listener: TcpListener) {
        for conn in listener.incoming() {
            match conn {
                Ok(s) => {
                    let st = self.state.clone();
                    std::thread::spawn(move || handle_conn(s, &st));
                }
                Err(e) => eprintln!("broker: kabul hatasi: {e}"),
            }
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        410 => "Gone",
        413 => "Payload Too Large",
        423 => "Locked",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

fn respond(stream: &mut TcpStream, status: u16, body: &str) {
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status,
        reason(status),
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

fn handle_conn(stream: TcpStream, state: &Arc<Mutex<BrokerState>>) {
    let mut s = stream;
    let req = match read_request(&s) {
        Ok(r) => r,
        Err(e) => {
            respond(&mut s, e.status, &e.body());
            return;
        }
    };
    let (status, body) = dispatch(&req, state);
    respond(&mut s, status, &body);
    // Değişen istek sonrası anlık görüntü (GET salt-okunur, atlanır).
    if req.method != "GET" {
        if let Ok(st) = state.lock() {
            st.save_snapshot();
        }
    }
}

// -- gövde yardımcıları --

fn json_body(req: &Request) -> Result<Value, ApiError> {
    serde_json::from_slice(&req.body).map_err(|_| ApiError::new(400, "bad_json", "json govde hatali"))
}

fn req_str(v: &Value, key: &str) -> Result<String, ApiError> {
    v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string()).ok_or_else(|| {
        ApiError::new(400, "bad_request", format!("'{key}' gerekli"))
    })
}

fn opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string())
}

fn bearer(req: &Request) -> Result<String, ApiError> {
    req.headers
        .get("authorization")
        .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::new(401, "unauthorized", "bearer token gerekli"))
}

fn hwid(req: &Request) -> Result<String, ApiError> {
    req.headers.get("x-hwid").cloned().filter(|s| !s.is_empty()).ok_or_else(|| {
        ApiError::new(400, "bad_request", "'X-Hwid' basligi gerekli")
    })
}

fn admin_of(req: &Request, st: &BrokerState, now: u64) -> Result<(), ApiError> {
    let tok = req.headers.get("x-admin-token").cloned().unwrap_or_default();
    st.admin_require(&tok, now)
}

fn segs(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

fn dispatch(req: &Request, state: &Arc<Mutex<BrokerState>>) -> (u16, String) {
    let now = now_secs();
    let r = dispatch_inner(req, state, now);
    match r {
        Ok((status, body)) => (status, body),
        Err(e) => (e.status, e.body()),
    }
}

fn dispatch_inner(
    req: &Request, state: &Arc<Mutex<BrokerState>>, now: u64,
) -> Result<(u16, String), ApiError> {
    let m = req.method.as_str();
    let s = segs(&req.path);

    // Sağlık.
    if m == "GET" && s.is_empty() {
        return Ok((200, "{\"ok\":true,\"service\":\"broker\"}".to_string()));
    }
    // Sürüm beslemesi (panel feed).
    if m == "GET" && s == ["v1", "version"] {
        let st = state.lock().map_err(|_| ApiError::new(500, "internal", "kilit"))?;
        return Ok((200, st.version().to_string()));
    }
    if m == "GET" && s == ["v1", "feed"] {
        let st = state.lock().map_err(|_| ApiError::new(500, "internal", "kilit"))?;
        return Ok((200, st.feed().to_string()));
    }
    if m == "GET" && s == ["v1", "version-check"] {
        let cv = req.query.get("client_version").cloned().unwrap_or_default();
        let st = state.lock().map_err(|_| ApiError::new(500, "internal", "kilit"))?;
        return Ok((200, st.version_check(&cv).to_string()));
    }

    // Auth (kullanıcı).
    if m == "POST" && s == ["v1", "redeem"] {
        let v = json_body(req)?;
        let (code, password, hwid) = (req_str(&v, "code")?, req_str(&v, "password")?, req_str(&v, "hwid")?);
        let mut st = state.lock().map_err(|_| ApiError::new(500, "internal", "kilit"))?;
        return st.redeem(&code, &password, &hwid, now).map(|v| (200, v.to_string()));
    }
    if m == "POST" && s == ["v1", "login"] {
        let v = json_body(req)?;
        let (account, password, hwid) =
            (req_str(&v, "account")?, req_str(&v, "password")?, req_str(&v, "hwid")?);
        let cv = opt_str(&v, "client_version");
        let mut st = state.lock().map_err(|_| ApiError::new(500, "internal", "kilit"))?;
        return st.login(&account, &password, &hwid, cv.as_deref(), now).map(|v| (200, v.to_string()));
    }
    if m == "POST" && s == ["v1", "refresh"] {
        let v = json_body(req)?;
        let (refresh, hwid) = (req_str(&v, "refresh")?, req_str(&v, "hwid")?);
        let mut st = state.lock().map_err(|_| ApiError::new(500, "internal", "kilit"))?;
        return st.refresh(&refresh, &hwid, now).map(|v| (200, v.to_string()));
    }
    if m == "POST" && s == ["v1", "transcribe"] {
        let token = bearer(req)?;
        let hwid = hwid(req)?;
        let req_id = req.headers.get("x-request-id").cloned().filter(|x| !x.is_empty()).ok_or_else(|| {
            ApiError::new(400, "bad_request", "'X-Request-Id' basligi gerekli")
        })?;
        let ahash = req.headers.get("x-audio-hash").cloned().filter(|x| !x.is_empty()).ok_or_else(|| {
            ApiError::new(400, "bad_request", "'X-Audio-Hash' basligi gerekli")
        })?;
        let mut st = state.lock().map_err(|_| ApiError::new(500, "internal", "kilit"))?;
        return st.transcribe(&token, &hwid, &req_id, &ahash, &req.body, now).map(|v| (200, v.to_string()));
    }

    if m == "GET" && s == ["v1", "me"] {
        let token = bearer(req)?;
        let hwid = hwid(req)?;
        let st = state.lock().map_err(|_| ApiError::new(500, "internal", "kilit"))?;
        return st.me(&token, &hwid, now).map(|v| (200, v.to_string()));
    }

    // Admin kurulum/giriş (jetonsuz).
    if m == "POST" && s == ["v1", "admin", "setup"] {
        let v = json_body(req)?;
        let pw = req_str(&v, "password")?;
        let mut st = state.lock().map_err(|_| ApiError::new(500, "internal", "kilit"))?;
        return st.admin_setup(&pw).map(|v| (200, v.to_string()));
    }
    if m == "POST" && s == ["v1", "admin", "login"] {
        let v = json_body(req)?;
        let pw = req_str(&v, "password")?;
        let mut st = state.lock().map_err(|_| ApiError::new(500, "internal", "kilit"))?;
        return st.admin_login(&pw, now).map(|v| (200, v.to_string()));
    }

    // Panel (admin jetonlu).
    if s.first() == Some(&"v1") {
        let mut st = state.lock().map_err(|_| ApiError::new(500, "internal", "kilit"))?;
        admin_of(req, &st, now)?;
        if m == "POST" && s == ["v1", "invites"] {
            let v = json_body(req)?;
            let username = req_str(&v, "username")?;
            let opening = v.get("opening_krs").and_then(|x| x.as_i64()).unwrap_or(0);
            let code = opt_str(&v, "code");
            return st.create_invite(&username, opening, code.as_deref(), now).map(|v| (200, v.to_string()));
        }
        if m == "GET" && s == ["v1", "users"] {
            return Ok((200, st.users(now).to_string()));
        }
        if s.len() == 3 && s[0] == "v1" && s[1] == "users" && m == "GET" {
            return st.user_detail(s[2], now).map(|v| (200, v.to_string()));
        }
        if s.len() == 4 && s[0] == "v1" && s[1] == "users" && m == "POST" && s[3] == "topup" {
            let v = json_body(req)?;
            let amount = v.get("amount_krs").and_then(|x| x.as_i64()).ok_or_else(|| {
                ApiError::new(400, "bad_request", "'amount_krs' gerekli")
            })?;
            return st.topup(s[2], amount, now).map(|v| (200, v.to_string()));
        }
        if s.len() == 4 && s[0] == "v1" && s[1] == "users" && m == "POST" && s[3] == "deduct" {
            let v = json_body(req)?;
            let amount = v.get("amount_krs").and_then(|x| x.as_i64()).ok_or_else(|| {
                ApiError::new(400, "bad_request", "'amount_krs' gerekli")
            })?;
            return st.deduct(s[2], amount, now).map(|v| (200, v.to_string()));
        }
        if s.len() == 4 && s[0] == "v1" && s[1] == "users" && m == "PUT" && s[3] == "limits" {
            let v = json_body(req)?;
            return st.set_limits(s[2], &v, now).map(|v| (200, v.to_string()));
        }
        if s.len() == 4 && s[0] == "v1" && s[1] == "users" && m == "PUT" && s[3] == "fallback-cap" {
            let v = json_body(req)?;
            let cap = v.get("cap_krs").and_then(|x| x.as_i64());
            return st.set_fallback_cap(s[2], cap, now).map(|v| (200, v.to_string()));
        }
        if s.len() == 4 && s[0] == "v1" && s[1] == "users" && m == "PUT" && s[3] == "home-only" {
            let v = json_body(req)?;
            let ho = v.get("home_only").and_then(|x| x.as_bool()).ok_or_else(|| {
                ApiError::new(400, "bad_request", "'home_only' gerekli")
            })?;
            return st.set_home_only(s[2], ho, now).map(|v| (200, v.to_string()));
        }
        if s.len() == 4 && s[0] == "v1" && s[1] == "users" && m == "POST" && s[3] == "hwid-reset" {
            return st.hwid_reset(s[2], now).map(|v| (200, v.to_string()));
        }
        if s.len() == 4 && s[0] == "v1" && s[1] == "users" && m == "POST" && s[3] == "suspend" {
            let v = json_body(req).unwrap_or(Value::Null);
            let stop = v.get("stop").and_then(|x| x.as_bool()).unwrap_or(true);
            return st.suspend(s[2], stop, now).map(|v| (200, v.to_string()));
        }
        if m == "GET" && s == ["v1", "tariffs"] {
            return Ok((200, st.tariffs_get().to_string()));
        }
        if m == "PUT" && s == ["v1", "tariffs"] {
            let v = json_body(req)?;
            return st.tariffs_put(&v, now).map(|v| (200, v.to_string()));
        }
        if m == "GET" && s == ["v1", "fallback", "switch"] {
            return Ok((200, st.switch_get().to_string()));
        }
        if m == "POST" && s == ["v1", "fallback", "switch"] {
            let v = json_body(req)?;
            let open = v.get("open").and_then(|x| x.as_bool()).ok_or_else(|| {
                ApiError::new(400, "bad_request", "'open' gerekli")
            })?;
            return Ok((200, st.switch_set(open, now).to_string()));
        }
        if m == "GET" && s == ["v1", "fallback", "vendor"] {
            return Ok((200, st.vendor_get().to_string()));
        }
        if m == "POST" && s == ["v1", "fallback", "vendor"] {
            let v = json_body(req)?;
            let vendor = v.get("vendor").and_then(|x| x.as_str()).unwrap_or("");
            return st.vendor_set(vendor, now).map(|v| (200, v.to_string()));
        }
        if m == "PUT" && s == ["v1", "fallback", "tariffs"] {
            let v = json_body(req)?;
            return st.vendor_price(&v, now).map(|v| (200, v.to_string()));
        }
        if m == "PUT" && s == ["v1", "fallback", "upstream"] {
            let v = json_body(req)?;
            return st.vendor_upstream(&v, now).map(|v| (200, v.to_string()));
        }
        if m == "POST" && s == ["v1", "fallback", "keys"] {
            let v = json_body(req)?;
            return st.key_set(&v, now).map(|v| (200, v.to_string()));
        }
        if m == "DELETE" && s == ["v1", "fallback", "keys"] {
            let vendor = req.query.get("vendor").cloned().unwrap_or_default();
            let vendor = if vendor.is_empty() {
                json_body(req).unwrap_or(Value::Null).get("vendor").and_then(|x| x.as_str()).unwrap_or("").to_string()
            } else {
                vendor
            };
            return st.key_clear(&vendor, now).map(|v| (200, v.to_string()));
        }
        if m == "GET" && s == ["v1", "audit"] {
            return Ok((200, st.audit(200).to_string()));
        }
        if m == "GET" && s == ["v1", "disks"] {
            return Ok((200, st.disks_get().to_string()));
        }
        if m == "PUT" && s == ["v1", "disks"] {
            let v = json_body(req)?;
            let u = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0).min(100) as u8;
            return Ok((200, st.disks_put(u("worker"), u("broker"), u("archive")).to_string()));
        }
        let _ = esc; // (esc state::body'de kullanılır)
        return Err(ApiError::new(404, "not_found", "rota bulunamadi"));
    }

    Err(ApiError::new(404, "not_found", "rota bulunamadi"))
}

// ---------------------------------------------------------------------------
// Testler (canlı TCP turu)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    fn test_server(ledger: &str) -> (String, Arc<Mutex<BrokerState>>) {
        let _ = std::fs::remove_file(ledger);
        // Anlık görüntü bayatı testi kirletmesin (defterle aynı dizin).
        let snap = std::path::Path::new(ledger)
            .parent()
            .map(|d| d.join("panel.json"))
            .unwrap_or_else(|| std::path::PathBuf::from("panel.json"));
        let _ = std::fs::remove_file(&snap);
        let st = Arc::new(Mutex::new(BrokerState::new(b"http-test-secret", ledger.to_string())));
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", l.local_addr().unwrap());
        let srv = Server::new(st.clone());
        std::thread::spawn(move || srv.serve_forever(l));
        // Hazır olana kadar yokla.
        for _ in 0..50 {
            if TcpStream::connect(addr.trim_start_matches("http://")).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        (addr.trim_start_matches("http://").to_string(), st)
    }

    fn call(addr: &str, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> (u16, Vec<u8>) {
        let mut s = TcpStream::connect(addr).unwrap();
        let mut req = format!("{} {} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n", method, path, body.len());
        for (k, v) in headers {
            req.push_str(&format!("{}: {}\r\n", k, v));
        }
        req.push_str("Connection: close\r\n\r\n");
        s.write_all(req.as_bytes()).unwrap();
        s.write_all(body).unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        let text = String::from_utf8_lossy(&out).to_string();
        let status: u16 = text.split_whitespace().nth(1).unwrap_or("0").parse().unwrap_or(0);
        let body = text.split("\r\n\r\n").nth(1).unwrap_or("").as_bytes().to_vec();
        (status, body)
    }

    fn jpost(addr: &str, path: &str, headers: &[(&str, &str)], v: &Value) -> (u16, Value) {
        let b = v.to_string().into_bytes();
        let mut h = vec![("Content-Type", "application/json")];
        h.extend_from_slice(headers);
        let (st, body) = call(addr, "POST", path, &h, &b);
        (st, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    fn sha_hex(b: &[u8]) -> String {
        crate::state::sha256_hex(b)
    }

    #[test]
    fn http_redeem_login_transcribe_replay_poor() {
        let ledger = format!("{}/broker-http-{}.jsonl", std::env::temp_dir().display(), std::process::id());
        let (addr, _) = test_server(&ledger);

        // Admin kurulum + davet.
        let (st, _) = jpost(&addr, "/v1/admin/setup", &[], &json!({"password": "yonetici-sifresi-123"}));
        assert_eq!(st, 200);
        let (st, v) = jpost(&addr, "/v1/admin/login", &[], &json!({"password": "yonetici-sifresi-123"}));
        assert_eq!(st, 200);
        let atok = v["admin_token"].as_str().unwrap().to_string();
        let ah = [("X-Admin-Token", atok.as_str())];
        let (st, v) = jpost(&addr, "/v1/invites", &ah, &json!({"username": "ali", "opening_krs": 10000}));
        assert_eq!(st, 200);
        let code = v["code"].as_str().unwrap().to_string();

        // redeem → login.
        let (st, v) = jpost(&addr, "/v1/redeem", &[], &json!({"code": code, "password": "gizli-sifre-1", "hwid": "hwid-A"}));
        assert_eq!(st, 200, "{v}");
        let access = v["access"].as_str().unwrap().to_string();
        let (st, _) = jpost(&addr, "/v1/login", &[], &json!({"account": "ali", "password": "gizli-sifre-1", "hwid": "hwid-A"}));
        assert_eq!(st, 200);

        // transcribe happy-path.
        let body = vec![0x40u8; 3200];
        let h = sha_hex(&body);
        let auth = format!("Bearer {}", access);
        let hd = [("Authorization", auth.as_str()), ("X-Hwid", "hwid-A"), ("X-Request-Id", "req-1"), ("X-Audio-Hash", h.as_str()), ("Content-Type", "application/octet-stream")];
        let (st, rb) = call(&addr, "POST", "/v1/transcribe", &hd, &body);
        assert_eq!(st, 200, "{}", String::from_utf8_lossy(&rb));
        let r1: Value = serde_json::from_slice(&rb).unwrap();
        assert_eq!(r1["cached"], json!(false));
        assert!(!r1["text"].as_str().unwrap().is_empty());
        assert_eq!(r1["cost_kurus"], json!(6));

        // Aynı ID tekrar → ücretsiz.
        let (st, rb) = call(&addr, "POST", "/v1/transcribe", &hd, &body);
        assert_eq!(st, 200);
        let r2: Value = serde_json::from_slice(&rb).unwrap();
        assert_eq!(r2["cached"], json!(true));
        assert_eq!(r2["cost_kurus"], json!(0));
        assert_eq!(r2["text"], r1["text"]);

        // Bakiye yetersiz hesap.
        let (st, v) = jpost(&addr, "/v1/invites", &ah, &json!({"username": "yoksul", "opening_krs": 0}));
        assert_eq!(st, 200);
        let code0 = v["code"].as_str().unwrap().to_string();
        let (st, v) = jpost(&addr, "/v1/redeem", &[], &json!({"code": code0, "password": "gizli-sifre-1", "hwid": "hwid-A"}));
        assert_eq!(st, 200);
        let access0 = v["access"].as_str().unwrap().to_string();
        let auth0 = format!("Bearer {}", access0);
        let hd0 = [("Authorization", auth0.as_str()), ("X-Hwid", "hwid-A"), ("X-Request-Id", "req-9"), ("X-Audio-Hash", h.as_str()), ("Content-Type", "application/octet-stream")];
        let (st, rb) = call(&addr, "POST", "/v1/transcribe", &hd0, &body);
        assert_eq!(st, 402, "{}", String::from_utf8_lossy(&rb));

        // Panel uçları: users, topup, tariffs get/put, switch, hwid-reset, suspend, version.
        let (st, _) = call(&addr, "GET", "/v1/users", &ah, &[]);
        assert_eq!(st, 200);
        let (st, _) = jpost(&addr, "/v1/users/ali/topup", &ah, &json!({"amount_krs": 500}));
        assert_eq!(st, 200);
        let (st, _) = call(&addr, "GET", "/v1/tariffs", &ah, &[]);
        assert_eq!(st, 200);
        let (st, _) = call(&addr, "PUT", "/v1/tariffs", &ah, &br#"{"home_krs_per_min":120}"#[..]);
        assert_eq!(st, 200);
        let (st, _) = call(&addr, "GET", "/v1/fallback/switch", &ah, &[]);
        assert_eq!(st, 200);
        let (st, _) = jpost(&addr, "/v1/fallback/switch", &ah, &json!({"open": true}));
        assert_eq!(st, 200);
        let (st, _) = jpost(&addr, "/v1/users/ali/hwid-reset", &ah, &json!({}));
        assert_eq!(st, 200);
        let (st, _) = jpost(&addr, "/v1/users/ali/suspend", &ah, &json!({"stop": true}));
        assert_eq!(st, 200);
        let (st, _) = call(&addr, "GET", "/v1/version", &[], &[]);
        assert_eq!(st, 200);
        let _ = std::fs::remove_file(&ledger);
        let _ = std::fs::remove_file(
            std::path::Path::new(&ledger)
                .parent()
                .map(|d| d.join("panel.json"))
                .unwrap_or_else(|| std::path::PathBuf::from("panel.json")),
        );
    }

    use serde_json::json;
}
