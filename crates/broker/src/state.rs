//! Broker iş mantığı (G1): durum + tüm uçların çekirdeği.
//!
//! Kapsam (LAN aşaması, TLS YOK, ev-only, mock upstream):
//! - Auth: `POST /v1/redeem`, `/v1/login`, `/v1/refresh` (whisper-auth).
//! - Transcribe: idempotency → doğrulama → bakiye/bloke → kuyruk sırası
//!   (PLAN.md §3). Kuyruktan çıkışta bakiye yeniden-kontrol + o an bloke.
//!   Başarısız/boş sonuçta ücret YOK (bloke aynen iade).
//! - Panel: `panel::api::ROUTES` alt kümesi → `PanelState`.
//! - Durum bellekte; ledger satırları JSONL dosyaya append edilir.
//!   Crash'te tasfiyesiz bloke `expire_stale_blocks` ile çözülür
//!   (her transcribe girişinde + yeniden başlatmada zaman aşımı).
//!
//! Bilinen LAN-fazı basitleştirmeleri:
//! - Kuyruk senkron drene edilir (gerçek bekleme yok); derinlik/hesap
//!   kotası + çıkışta yeniden-kontrol guard'ları aktiftir.
//! - Hat her zaman evdir; rota fallback seçerse ücretsiz bakım reti dönülür.
//! - `suspend(stop=false)` panel + whisper-auth iki tarafını da açar
//!   (`unsuspend` köprülüdür); suspend-öncesi jetonlar ölü kalır
//!   (nesil artışı geri alınmaz).

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use broker_core::{
    Lane as CoreLane, PopOutcome, Queue, QueueConfig, QueuedReq, RouteConfig, RouteTable,
};
use ev_worker::{
    InferenceBackend, RECONCILE_TOLERANCE_SECS, measure_secs, reconcile,
};
use ledger::{Ledger, LedgerError, Line as LedgerLine, Tariff as LedgerTariff};
use panel::{
    PanelState,
    tariffs::{FallbackPrice, FallbackVendor, Line as PanelLine, Tariff as PanelTariff},
};
use provider::{DecodeError, GateConfig, GateReject, OpusDecoder, PCM_RATE_HZ, PcmAudio, gate};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use whisper_auth::{AuthError, AuthStore};
use whisper_protocol::{
    AccountId, AudioHash, IdempotencyKey, RequestId, ResultCache, accept_body,
};

// ---------------------------------------------------------------------------
// Sabitler
// ---------------------------------------------------------------------------

/// Ev hattı varsayılan tarifesi: 120 kuruş/dk (1.20 TL/dk).
pub const DEFAULT_HOME_KRS_PER_MIN: i64 = 120;
/// Fallback fiyatı bu fazda kullanılmaz; 5x çarpan varsayılanı.
pub const DEFAULT_FALLBACK_BP: u64 = 50000;
/// Stale-bloke zaman aşımı: 1 saat hareketsiz bloke iade edilir.
pub const STALE_BLOCK_TIMEOUT_SECS: u64 = 3600;
/// Admin oturum ömrü: 24 saat.
pub const ADMIN_TOKEN_TTL_SECS: u64 = 24 * 3600;
/// Anlık görüntü biçimi sürümü (uyumsuzsa temiz başlanır).
const SNAPSHOT_VERSION: u32 = 1;
/// Anlık görüntü dosyası adı (defterle aynı dizinde).
const SNAPSHOT_FILE: &str = "panel.json";

// ---------------------------------------------------------------------------
// Hata
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub code: String,
    pub message: String,
}

impl ApiError {
    pub fn new(status: u16, code: &str, message: impl Into<String>) -> Self {
        Self { status, code: code.to_string(), message: message.into() }
    }

    pub fn body(&self) -> String {
        format!(
            "{{\"ok\":false,\"error\":{{\"code\":\"{}\",\"message\":\"{}\"}}}}",
            esc(&self.code),
            esc(&self.message)
        )
    }
}

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

fn auth_err(e: AuthError) -> ApiError {
    let msg = format!("{:?}", e);
    match e {
        AuthError::InviteNotFound => ApiError::new(404, "invite_not_found", msg),
        AuthError::InviteUsed => ApiError::new(409, "invite_used", msg),
        AuthError::InviteExpired => ApiError::new(410, "invite_expired", msg),
        AuthError::AccountExists => ApiError::new(409, "account_exists", msg),
        AuthError::AccountNotFound => ApiError::new(404, "account_not_found", msg),
        AuthError::AccountLocked { .. } => ApiError::new(423, "account_locked", msg),
        AuthError::AccountSuspended => ApiError::new(403, "account_suspended", msg),
        AuthError::BadPassword => ApiError::new(401, "bad_password", msg),
        AuthError::BadHwid => ApiError::new(403, "forbidden_hwid", msg),
        AuthError::NoFreeSlot => ApiError::new(403, "no_free_slot", msg),
        AuthError::BadToken => ApiError::new(401, "unauthorized", msg),
        AuthError::TokenExpired => ApiError::new(401, "token_expired", msg),
        AuthError::TokenRevoked => ApiError::new(401, "token_revoked", msg),
        AuthError::RefreshReuse => ApiError::new(401, "refresh_reuse", msg),
        AuthError::WeakPassword => ApiError::new(400, "weak_password", msg),
    }
}

fn ledger_err(e: LedgerError) -> ApiError {
    let code = e.code().to_string();
    let msg = format!("{:?}", e);
    match e {
        LedgerError::InsufficientBalance { .. } => ApiError::new(402, &code, msg),
        LedgerError::DoubleBlock { .. } => ApiError::new(409, "id_conflict", msg),
        LedgerError::InvalidAmount => ApiError::new(400, &code, msg),
        LedgerError::OverSettle { .. } | LedgerError::OverRelease { .. } => {
            ApiError::new(500, "internal", msg)
        }
    }
}

fn gate_err(e: GateReject) -> ApiError {
    match e {
        GateReject::Empty => ApiError::new(400, "empty_body", "bos govde"),
        GateReject::TooBig => ApiError::new(413, "payload_too_large", "govde tavani asti (2MB)"),
        GateReject::Decode => ApiError::new(400, "decode_failed", "decode basarisiz"),
        GateReject::TooLong => ApiError::new(400, "duration_too_long", "sure tavani asti (180sn)"),
        GateReject::Silence => ApiError::new(400, "silent_audio", "sessizlik: ucretsiz ret"),
    }
}

fn panel_account_err(e: panel::accounts::AccountError) -> ApiError {
    use panel::accounts::AccountError as E;
    let msg = format!("{:?}", e);
    match e {
        E::AccountNotFound | E::InviteNotFound => ApiError::new(404, "account_not_found", msg),
        E::InviteExists | E::InviteRedeemed | E::AccountExists => {
            ApiError::new(409, "invite_used", msg)
        }
        E::InviteExpired => ApiError::new(410, "invite_expired", msg),
        E::Suspended => ApiError::new(403, "account_suspended", msg),
        E::BadPassword => ApiError::new(401, "bad_password", msg),
        E::NoFreeSlot | E::HwidMismatch => ApiError::new(403, "forbidden_hwid", msg),
        E::WeakPassword | E::BadOpeningBalance => ApiError::new(400, "weak_password", msg),
        E::InsufficientBalance => ApiError::new(402, "insufficient_balance", msg),
    }
}

fn route_err(e: panel::routing::RouteError) -> ApiError {
    match e {
        panel::routing::RouteError::Maintenance(m) => {
            ApiError::new(503, "maintenance", m.to_string())
        }
        panel::routing::RouteError::Capped(m) => ApiError::new(429, "capped", m.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Yardımcılar
// ---------------------------------------------------------------------------

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
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

pub fn sha256_hex(data: &[u8]) -> String {
    hex_of(&Sha256::digest(data))
}

fn ledger_tariff(pt: &PanelTariff) -> LedgerTariff {
    LedgerTariff {
        version: pt.version,
        home_kurus_per_min: pt.home_krs_per_min.max(0) as u64,
        fallback_kurus_per_min: pt.fallback_krs_per_min().max(0) as u64,
        upstream_min_secs: pt.upstream_min_secs.min(u32::MAX as u64) as u32,
    }
}

/// LAN-fazı mock çözücü: ham baytları 16kHz mono PCM (i16 LE) sayar.
/// Süre = örnek/16000; enerji gerçek RMS'tir. Sihir kuralları FakeDecoder
/// ile aynıdır: boş → Empty, `FF FF FF FF` öneki → Corrupt, tamamı
/// sıfır → sessizlik (kapıda elenir). Gerçek Opus decode'u üretim fazındadır.
struct BrokerDecoder;

impl OpusDecoder for BrokerDecoder {
    fn decode(&self, opus: &[u8]) -> Result<PcmAudio, DecodeError> {
        if opus.is_empty() {
            return Err(DecodeError::Empty);
        }
        if opus.len() >= 4 && opus[0..4] == [0xFF, 0xFF, 0xFF, 0xFF] {
            return Err(DecodeError::Corrupt);
        }
        let n = opus.len() / 2;
        let mut samples = Vec::with_capacity(n);
        for i in 0..n {
            samples.push(i16::from_le_bytes([opus[2 * i], opus[2 * i + 1]]));
        }
        Ok(PcmAudio { samples, rate_hz: PCM_RATE_HZ })
    }
}

// ---------------------------------------------------------------------------
// Durum
// ---------------------------------------------------------------------------

pub struct BrokerState {
    auth: AuthStore,
    ledger: Ledger,
    cache: ResultCache,
    queue: Queue,
    routes: RouteTable,
    panel: PanelState,
    admin_tokens: HashMap<String, u64>,
    in_flight: HashSet<String>,
    req_seq: u64,
    token_ctr: u64,
    ledger_persisted: usize,
    ledger_path: String,
    stale_timeout: u64,
}

fn snapshot_path_for(ledger_path: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(ledger_path);
    match p.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.join(SNAPSHOT_FILE),
        _ => std::path::PathBuf::from(SNAPSHOT_FILE),
    }
}

/// Restart-dayanıklı durum zarfı (sırlar hariç).
#[derive(Serialize, Deserialize)]
struct Snapshot {
    v: u32,
    auth: AuthStore,
    panel: PanelState,
    token_ctr: u64,
    req_seq: u64,
}

impl BrokerState {
    pub fn new(secret: &[u8], ledger_path: String) -> Self {
        let mut s = Self {
            auth: AuthStore::new(secret),
            ledger: Ledger::new(),
            cache: ResultCache::new(),
            queue: Queue::new(QueueConfig::default()),
            routes: RouteTable::new(RouteConfig::default()),
            panel: PanelState::new(DEFAULT_HOME_KRS_PER_MIN, FallbackPrice::MultiplierBp(DEFAULT_FALLBACK_BP)),
            admin_tokens: HashMap::new(),
            in_flight: HashSet::new(),
            req_seq: 0,
            token_ctr: 0,
            ledger_persisted: 0,
            ledger_path,
            stale_timeout: STALE_BLOCK_TIMEOUT_SECS,
        };
        s.load_snapshot(secret);
        s
    }

    // -- anlık görüntü (restart-dayanıklılık) --
    //
    // Kapsam: auth (hesap/davet/refresh) + panel (admin hash, hesap/davet,
    // tarife, şalter, besleme, denetim) + sayaçlar. YAZILMAYANLAR: sır
    // (ortamdan gelir), geçici admin jetonları, sağlayıcı anahtarları
    // (bellek-içi), kuyruk/rota/önbellek (geçici), defter (zaten JSONL).

    fn snapshot_path(&self) -> std::path::PathBuf {
        snapshot_path_for(&self.ledger_path)
    }

    /// Değişen her istekten sonra çağrılır (GET atlanır; kilit çağıranda).
    pub fn save_snapshot(&self) {
        let snap = Snapshot {
            v: SNAPSHOT_VERSION,
            auth: self.auth.clone(),
            panel: self.panel.clone(),
            token_ctr: self.token_ctr,
            req_seq: self.req_seq,
        };
        let text = match serde_json::to_string(&snap) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("broker: anlik goruntu serilestirilemedi: {e}");
                return;
            }
        };
        let path = self.snapshot_path();
        let tmp = path.with_extension("json.tmp");
        if let Err(e) = std::fs::write(&tmp, text) {
            eprintln!("broker: anlik goruntu yazilamadi: {e}");
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            eprintln!("broker: anlik goruntu tasinamadi: {e}");
        }
    }

    fn load_snapshot(&mut self, secret: &[u8]) {
        let path = self.snapshot_path();
        let raw = match std::fs::read_to_string(&path) {
            Ok(r) => r,
            Err(_) => return, // ilk kurulum: dosya yok, temiz başlanır.
        };
        let snap: Snapshot = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("broker: anlik goruntu okunamadi ({e}), temiz baslatiliyor");
                return;
            }
        };
        if snap.v != SNAPSHOT_VERSION {
            eprintln!("broker: anlik goruntu surumu uyumsuz, temiz baslatiliyor");
            return;
        }
        self.auth = snap.auth;
        self.auth.set_secret(secret);
        self.panel = snap.panel;
        self.token_ctr = snap.token_ctr;
        self.req_seq = snap.req_seq;
        eprintln!("broker: anlik goruntu yuklendi ({})", path.display());
    }

    // -- defter kalıcılığı (append-only JSONL; salt audit izi) --

    fn persist_ledger(&mut self) {
        use std::fs::OpenOptions;
        use std::io::Write;
        if self.ledger.entries().len() <= self.ledger_persisted {
            return;
        }
        let mut buf = String::new();
        for e in &self.ledger.entries()[self.ledger_persisted..] {
            let line = match e.line {
                LedgerLine::Home => "home",
                LedgerLine::Fallback => "fallback",
            };
            let kind = match e.kind {
                ledger::Kind::Topup => "topup",
                ledger::Kind::Deduct => "deduct",
                ledger::Kind::Block => "block",
                ledger::Kind::Settle => "settle",
                ledger::Kind::Release => "release",
            };
            buf.push_str(&format!(
                "{{\"seq\":{},\"at\":{},\"account\":\"{}\",\"request_id\":\"{}\",\"line\":\"{}\",\"tariff_version\":{},\"measured_secs\":{},\"kind\":\"{}\",\"amount_kurus\":{},\"note\":\"{}\"}}\n",
                e.seq, e.at_secs, esc(&e.account), esc(&e.request_id), line,
                e.tariff_version, e.measured_secs, kind, e.amount_kurus, esc(&e.note)
            ));
        }
        match OpenOptions::new().create(true).append(true).open(&self.ledger_path) {
            Ok(mut f) => {
                if let Err(err) = f.write_all(buf.as_bytes()) {
                    eprintln!("broker: ledger append hatasi: {err}");
                    return;
                }
                self.ledger_persisted = self.ledger.entries().len();
            }
            Err(err) => eprintln!("broker: ledger dosyasi acilamadi: {err}"),
        }
    }

    // -- auth uçları --

    /// Davetle ilk giriş: panel + auth davetleri birlikte tüketilir,
    /// açılış bakiyesi deftere işlenir.
    pub fn redeem(&mut self, code: &str, password: &str, hwid: &str, now: u64) -> Result<Value, ApiError> {
        let inv = self.panel.accounts.invites.get(code).cloned().ok_or_else(|| {
            ApiError::new(404, "invite_not_found", "davet bulunamadi")
        })?;
        if inv.redeemed {
            return Err(ApiError::new(409, "invite_used", "davet kullanilmis"));
        }
        if now > inv.expires_at {
            return Err(ApiError::new(410, "invite_expired", "davet suresi dolmus"));
        }
        let username = inv.username.clone();
        match self.auth.create_invite(code, now) {
            Ok(()) | Err(AuthError::AccountExists) => {}
            Err(e) => return Err(auth_err(e)),
        }
        let pair = self.auth.redeem_invite(code, &username, password, hwid, now).map_err(auth_err)?;
        self.panel.accounts.redeem_invite(code, password, hwid, now).map_err(panel_account_err)?;
        if inv.opening_balance_krs > 0 {
            self.ledger.topup(now, &username, inv.opening_balance_krs as u64, "acilis davet").map_err(ledger_err)?;
        }
        self.panel.audit.record(now, "broker", "redeem", &username);
        self.persist_ledger();
        Ok(json!({
            "ok": true, "account": username,
            "access": pair.access, "refresh": pair.refresh,
            "access_expires_at": pair.access_expires_at,
            "refresh_expires_at": pair.refresh_expires_at,
            "balance_kurus": self.ledger.balance(&username),
        }))
    }

    pub fn login(
        &mut self, account: &str, password: &str, hwid: &str,
        client_version: Option<&str>, now: u64,
    ) -> Result<Value, ApiError> {
        let pair = self.auth.login(account, password, hwid, now).map_err(auth_err)?;
        // Panel görünümü en-iyi-gayretle eşitlenir (auth kapı için yetkilidir).
        let mut panel_device = "no-panel-account";
        if self.panel.accounts.get(account).is_some() {
            panel_device = match self.panel.accounts.bind_device(account, password, hwid, now) {
                Ok(()) => {
                    if self.panel.accounts.get(account).map(|a| a.devices.iter().any(|d| d.hwid == hwid)).unwrap_or(false) {
                        "bound"
                    } else {
                        "bound"
                    }
                }
                Err(panel::accounts::AccountError::NoFreeSlot) => "full",
                Err(_) => "error",
            };
            if let Some(a) = self.panel.accounts.get_mut(account) {
                a.online = true;
                a.last_active = now;
                if let Some(v) = client_version {
                    a.client_version = v.to_string();
                }
            }
        }
        Ok(json!({
            "ok": true, "account": account,
            "access": pair.access, "refresh": pair.refresh,
            "access_expires_at": pair.access_expires_at,
            "refresh_expires_at": pair.refresh_expires_at,
            "panel_device": panel_device,
        }))
    }

    pub fn refresh(&mut self, refresh: &str, hwid: &str, now: u64) -> Result<Value, ApiError> {
        let pair = self.auth.refresh(refresh, hwid, now).map_err(auth_err)?;
        Ok(json!({
            "ok": true,
            "access": pair.access, "refresh": pair.refresh,
            "access_expires_at": pair.access_expires_at,
            "refresh_expires_at": pair.refresh_expires_at,
        }))
    }

    /// Girisli hesap ozeti: bakiye + o anki hat tarifeleri (istemci penceresi).
    /// Yalnizca okur; deftere/audit'e yazmaz.
    pub fn me(&self, token: &str, hwid: &str, now: u64) -> Result<Value, ApiError> {
        let claims = self.auth.verify_access(token, hwid, now).map_err(auth_err)?;
        let account = claims.account.clone();
        let pt = self.panel.tariffs.as_ref().map(|t| t.current());
        let (home, fb) = match pt {
            Some(t) => (t.home_krs_per_min, t.fallback_krs_per_min()),
            None => (0, 0),
        };
        Ok(json!({
            "ok": true, "account": account,
            "balance_kurus": self.ledger.balance(&account),
            "home_krs_per_min": home, "fallback_krs_per_min": fb,
        }))
    }

    // -- transcribe --

    /// Tam hat: idempotency → doğrulama → bakiye/bloke → kuyruk → kapı →
    /// ev-worker mock backend → kesinleştirme/iade.
    pub fn transcribe(
        &mut self, token: &str, hwid: &str, req_id: &str,
        audio_hash_claimed: &str, body: &[u8], now: u64,
    ) -> Result<Value, ApiError> {
        // Crash artığı bloke varsa zaman aşımıyla çözülür.
        let _ = self.ledger.expire_stale_blocks(now, self.stale_timeout);
        let claims = self.auth.verify_access(token, hwid, now).map_err(auth_err)?;
        let account = claims.account.clone();

        let acc_id = AccountId::new(&account)
            .map_err(|e| ApiError::new(400, e.code(), "gecersiz hesap"))?;
        let req = RequestId::new(req_id)
            .map_err(|e| ApiError::new(400, e.code(), "gecersiz istek id"))?;
        let claimed = AudioHash::new(audio_hash_claimed)
            .map_err(|e| ApiError::new(400, e.code(), "gecersiz ses hash"))?;
        let actual_hex = sha256_hex(body);
        if claimed.as_str().to_lowercase() != actual_hex {
            return Err(ApiError::new(400, "invalid_audio_hash", "ses hash govdeyle uyusmuyor"));
        }
        let ahash = AudioHash::new(&actual_hex).map_err(|_| {
            ApiError::new(500, "internal", "ic hash uretilemedi")
        })?;
        let key = IdempotencyKey::new(acc_id, req, ahash);

        // 1) Idempotency: başarı 24sa önbellekte → ücretsiz dönüş.
        if let Some(text) = self.cache.lookup(&key, now) {
            return Ok(json!({
                "ok": true, "cached": true, "text": text,
                "cost_kurus": 0, "billed_secs": 0,
            }));
        }

        // 2) Doğrulama: gövde tavanı.
        accept_body(body.len(), 0.0).map_err(|e| match e {
            whisper_protocol::ErrorCode::PayloadTooLarge => {
                ApiError::new(413, "payload_too_large", "govde tavani asti (2MB)")
            }
            _ => ApiError::new(400, e.code(), "govde kabul disi"),
        })?;

        let pt = self.panel.tariffs.as_ref().map(|t| t.current()).ok_or_else(|| {
            ApiError::new(503, "maintenance", "tarife yok")
        })?;
        let lt = ledger_tariff(&pt);

        // 3) Minimum ön-kontrol (bakiye ≥ 1 quantum), ücretsiz ret.
        self.ledger.precheck(&account, LedgerLine::Home, &lt).map_err(ledger_err)?;

        // 4) Sağlayıcı giriş kapısı: decode + süre + sessizlik (ücretsiz ret).
        let gate_cfg = GateConfig::default();
        let measured = gate(body, &BrokerDecoder, &gate_cfg).map_err(gate_err)?;
        let ceil_secs = measured.secs.ceil().max(1.0) as u64;

        // Panel kabul kapısı (şalter/limit; hesap yoksa auth-only devam).
        if self.panel.accounts.get(&account).is_some() {
            self.panel.admit(&account, PanelLine::Home, ceil_secs, now).map_err(route_err)?;
        }

        if !self.in_flight.insert(account.clone()) {
            return Err(ApiError::new(429, "account_busy", "hesabin isleyen istegi var"));
        }
        let out = self.transcribe_inner(&account, req_id, key, body, measured.secs, &lt, &pt, ceil_secs, now);
        self.in_flight.remove(&account);
        self.persist_ledger();
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn transcribe_inner(
        &mut self, account: &str, req_id: &str, key: IdempotencyKey,
        body: &[u8], measured_secs: f64, lt: &LedgerTariff, pt: &PanelTariff,
        ceil_secs: u64, now: u64,
    ) -> Result<Value, ApiError> {
        // 5) Kuyruk (girişte ön-kontrol geçti).
        self.req_seq += 1;
        let seq = self.req_seq;
        let qreq = QueuedReq {
            req_id: seq,
            account: account.to_string(),
            enqueued_at: now,
            tariff_version: lt.version as u32,
            lane: CoreLane::Home,
        };
        self.queue.push(qreq, true, now).map_err(|e| match e {
            broker_core::EnqueueReject::BalanceShort => {
                ApiError::new(402, "insufficient_balance", "bakiye yetersiz")
            }
            broker_core::EnqueueReject::AccountFull | broker_core::EnqueueReject::GlobalFull => {
                ApiError::new(429, "queue_full", "kuyruk dolu: ucretsiz ret")
            }
        })?;
        match self.queue.pop_next(now, &|_| false) {
            Some(PopOutcome::Ready(_)) => {}
            Some(PopOutcome::ExpiredFree(_)) => {
                return Err(ApiError::new(429, "queue_wait_timeout", "maksimum bekleme asildi: ucretsiz"));
            }
            None => return Err(ApiError::new(500, "internal", "kuyruk bos")),
        }

        // 6) Kuyruk ÇIKIŞINDA bakiye yeniden-kontrol + o an bloke.
        //    (Bloke bakiyeden düşmüş görünür; sonraki ön-kontrolü delmez.)
        if let Err(e) = self.ledger.precheck(account, LedgerLine::Home, lt) {
            return Err(ledger_err(e));
        }
        let block_amt = self
            .ledger
            .block(now, account, req_id, measured_secs, LedgerLine::Home, lt)
            .map_err(ledger_err)?;

        // 7) Rota: ev-only. Fallback seçilirse bloke çözülür, ücretsiz bakım reti.
        let mut assign = self.routes.assign(seq, now);
        if assign.lane != CoreLane::Home {
            let _ = self.ledger.release(now, account, req_id, block_amt, "fallback kapali (ev-only)");
            return Err(ApiError::new(503, "maintenance", "fallback kapali (ev-only)"));
        }
        RouteTable::begin_inference(&mut assign);

        // 8) Ev-worker arka-ucu (`EV_BACKEND=wl` → gerçek `wl --serve`,
        //    yoksa sahte; ölçümde işçi yetkilidir).
        let pcm = BrokerDecoder.decode(body).map_err(|_| gate_err(GateReject::Decode))?;
        let worker_secs = measure_secs(pcm.samples.len(), pcm.rate_hz);
        let rec = reconcile(worker_secs, measured_secs, RECONCILE_TOLERANCE_SECS);
        let text = ev_worker::backend().run(&pcm.samples, pcm.rate_hz);

        // 9) Boş transkript = başarısız = ÜCRETSİZ (bloke aynen iade, önbellek yok).
        if text.trim().is_empty() {
            let _ = self.ledger.fail_request(now, account, req_id, "bos transkript");
            return Err(ApiError::new(400, "empty_transcript", "metin cikmadi: ucretsiz"));
        }

        // 10) Kesinleştirme: yalnızca bloke içinden düşer.
        self.ledger.settle(now, account, req_id, block_amt).map_err(ledger_err)?;

        // Panel görünümü eşitlenir (bakiye/kullanım/harcama; ücret iki kez yazılmaz:
        // panel finalize'ı aynı dondurulmuş tekliften düşer).
        let mut low = false;
        if self.panel.accounts.get(account).is_some() {
            let q = pt.quote(PanelLine::Home, ceil_secs);
            debug_assert_eq!(q.cost_krs.max(0) as u64, block_amt);
            let _ = self.panel.finalize(account, &q, now);
            if let (Some(a), Some(t)) = (self.panel.accounts.get(account), self.panel.tariffs.as_ref()) {
                low = panel::users::is_low_balance(a, &t.current(), PanelLine::Home);
            }
        }
        if let Some(a) = self.panel.accounts.get_mut(account) {
            a.last_active = now;
            a.online = true;
        }
        self.cache.store(key, &text, now);

        let billed = ledger::billable_seconds(measured_secs, LedgerLine::Home, lt);
        Ok(json!({
            "ok": true, "cached": false, "text": text,
            "billed_secs": billed, "cost_kurus": block_amt,
            "tariff_version": lt.version,
            "worker_wins_warning": rec.worker_wins_warning,
            "low_balance": low,
        }))
    }

    // -- sürüm beslemesi --

    pub fn version(&self) -> Value {
        let floor = self.panel.feed.as_ref().map(|f| f.floor_version.clone()).unwrap_or_default();
        match self.panel.feed.as_ref().and_then(|f| f.tauri_feed()) {
            Some(feed) => json!({"ok": true, "feed": feed, "floor_version": floor}),
            None => json!({
                "ok": true, "version": "0.0.0-dev",
                "notes": "besleme henuz yayinlanmadi",
                "floor_version": floor, "platforms": {},
            }),
        }
    }

    pub fn feed(&self) -> Value {
        match self.panel.feed.as_ref().and_then(|f| f.tauri_feed()) {
            Some(feed) => json!({"ok": true, "feed": feed}),
            None => json!({"ok": true, "feed": null}),
        }
    }

    pub fn version_check(&self, client_version: &str) -> Value {
        use panel::feed::UpdateCheck;
        match self.panel.check_client(client_version) {
            UpdateCheck::Ok => json!({"ok": true, "status": "ok"}),
            UpdateCheck::Available { latest, forced, notes } => {
                json!({"ok": true, "status": "available", "latest": latest, "forced": forced, "notes": notes})
            }
            UpdateCheck::Blocked { latest, reason } => {
                json!({"ok": true, "status": "blocked", "latest": latest, "reason": reason})
            }
        }
    }

    // -- admin oturumu --

    fn issue_admin_token(&mut self, now: u64) -> (String, u64) {
        self.token_ctr += 1;
        let tok = sha256_hex(format!("admin:{}:{}:whisperexe", now, self.token_ctr).as_bytes());
        let exp = now + ADMIN_TOKEN_TTL_SECS;
        self.admin_tokens.insert(tok.clone(), exp);
        (tok, exp)
    }

    pub fn admin_require(&self, token: &str, now: u64) -> Result<(), ApiError> {
        match self.admin_tokens.get(token) {
            Some(exp) if now < *exp => Ok(()),
            Some(_) => Err(ApiError::new(401, "admin_token_expired", "admin oturumu suresi dolmus")),
            None => Err(ApiError::new(401, "admin_unauthorized", "admin girisi gerekli")),
        }
    }

    pub fn admin_setup(&mut self, password: &str) -> Result<Value, ApiError> {
        self.panel.admin.setup(password).map_err(|e| match e {
            panel::AdminError::AlreadySetup => ApiError::new(409, "admin_already_setup", "admin kurulu"),
            _ => ApiError::new(400, "weak_password", "zayif sifre (en az 12 karakter)"),
        })?;
        Ok(json!({"ok": true}))
    }

    pub fn admin_login(&mut self, password: &str, now: u64) -> Result<Value, ApiError> {
        self.panel.admin.verify(password, now).map_err(|e| match e {
            panel::AdminError::Locked { .. } => ApiError::new(423, "account_locked", "5 basarisiz: 5dk kilit"),
            panel::AdminError::NotSetup => ApiError::new(409, "admin_not_setup", "once kurulum yapilmali"),
            _ => ApiError::new(401, "bad_password", "hatali sifre"),
        })?;
        let (tok, exp) = self.issue_admin_token(now);
        Ok(json!({"ok": true, "admin_token": tok, "expires_at": exp}))
    }

    // -- panel uçları --

    pub fn create_invite(
        &mut self, username: &str, opening_krs: i64, code_opt: Option<&str>, now: u64,
    ) -> Result<Value, ApiError> {
        let mut code = code_opt.unwrap_or("").to_string();
        if code.is_empty() {
            self.token_ctr += 1;
            code = sha256_hex(format!("davet:{}:{}:{}", now, self.token_ctr, username).as_bytes())[..20].to_string();
        }
        let inv = self.panel.open_account("admin", &code, username, opening_krs, now).map_err(panel_account_err)?;
        if let Err(e) = self.auth.create_invite(&code, now) {
            self.panel.accounts.invites.remove(&code);
            return Err(auth_err(e));
        }
        Ok(json!({
            "ok": true, "code": inv.code, "username": inv.username,
            "opening_balance_krs": inv.opening_balance_krs, "expires_at": inv.expires_at,
        }))
    }

    pub fn users(&self, now: u64) -> Value {
        let rows = self.panel.user_table(PanelLine::Home, now);
        let arr: Vec<Value> = rows
            .iter()
            .map(|r| {
                json!({
                    "username": r.username, "online": r.online,
                    "usage_home_secs": r.usage_home_secs, "usage_fb_secs": r.usage_fb_secs,
                    "last_active": r.last_active, "client_version": r.client_version,
                    "balance_krs": r.balance_krs, "suspended": r.suspended,
                    "low_balance": r.low_balance, "hwid_reset_warning": r.hwid_reset_warning,
                })
            })
            .collect();
        json!({"ok": true, "users": arr})
    }

    pub fn user_detail(&self, username: &str, now: u64) -> Result<Value, ApiError> {
        let a = self.panel.accounts.get(username).ok_or_else(|| {
            ApiError::new(404, "account_not_found", "hesap bulunamadi")
        })?;
        let t = self.panel.tariffs.as_ref().map(|x| x.current());
        let row = match t {
            Some(ref tt) => panel::users::user_row(a, tt, PanelLine::Home, now),
            None => panel::users::user_row(
                a,
                &panel::tariffs::TariffTable::new(0, FallbackPrice::FixedKrsPerMin(0)).current(),
                PanelLine::Home,
                now,
            ),
        };
        Ok(json!({
            "ok": true, "username": row.username, "online": row.online,
            "usage_home_secs": row.usage_home_secs, "usage_fb_secs": row.usage_fb_secs,
            "last_active": row.last_active, "client_version": row.client_version,
            "balance_krs": row.balance_krs, "suspended": row.suspended,
            "low_balance": row.low_balance, "hwid_reset_warning": row.hwid_reset_warning,
            "ledger_balance_krs": self.ledger.balance(username),
            "devices": a.devices.len(), "home_only": a.home_only,
        }))
    }

    pub fn topup(&mut self, username: &str, amount_krs: i64, now: u64) -> Result<Value, ApiError> {
        if amount_krs <= 0 {
            return Err(ApiError::new(400, "invalid_amount", "tutar pozitif olmali"));
        }
        self.panel.top_up("admin", username, amount_krs, now).map_err(panel_account_err)?;
        self.ledger.topup(now, username, amount_krs as u64, "admin yukleme").map_err(ledger_err)?;
        self.persist_ledger();
        Ok(json!({
            "ok": true, "username": username,
            "balance_krs": self.ledger.balance(username),
        }))
    }

    pub fn deduct(&mut self, username: &str, amount_krs: i64, now: u64) -> Result<Value, ApiError> {
        if amount_krs <= 0 {
            return Err(ApiError::new(400, "invalid_amount", "tutar pozitif olmali"));
        }
        self.panel.deduct("admin", username, amount_krs, now).map_err(panel_account_err)?;
        self.ledger.deduct(now, username, amount_krs as u64, "admin dusurme").map_err(ledger_err)?;
        self.persist_ledger();
        Ok(json!({
            "ok": true, "username": username,
            "balance_krs": self.ledger.balance(username),
        }))
    }

    pub fn set_limits(&mut self, username: &str, v: &Value, now: u64) -> Result<Value, ApiError> {
        let cur = self.panel.accounts.get(username).cloned().ok_or_else(|| {
            ApiError::new(404, "account_not_found", "hesap bulunamadi")
        })?;
        let opt = |k: &str| v.get(k).and_then(|x| x.as_i64());
        let has = |k: &str| v.get(k).is_some();
        self.panel
            .set_limits(
                "admin", username,
                if has("daily_home") { opt("daily_home") } else { cur.daily_cap_home_krs },
                if has("daily_fb") { opt("daily_fb") } else { cur.daily_cap_fb_krs },
                if has("monthly_home") { opt("monthly_home") } else { cur.monthly_cap_home_krs },
                if has("monthly_fb") { opt("monthly_fb") } else { cur.monthly_cap_fb_krs },
                if has("fb_cap") { opt("fb_cap") } else { cur.fallback_daily_cap_krs },
                v.get("home_only").and_then(|x| x.as_bool()),
                now,
            )
            .map_err(panel_account_err)?;
        // home_only bayrağı set_limits içinde işlenmezse ayrıca işlenir
        // (mevcut PanelStateset_limits bunu kapsar; burada ek işlem yok).
        Ok(json!({"ok": true}))
    }

    pub fn set_fallback_cap(&mut self, username: &str, cap: Option<i64>, now: u64) -> Result<Value, ApiError> {
        let cur = self.panel.accounts.get(username).cloned().ok_or_else(|| {
            ApiError::new(404, "account_not_found", "hesap bulunamadi")
        })?;
        self.panel
            .set_limits(
                "admin", username,
                cur.daily_cap_home_krs, cur.daily_cap_fb_krs,
                cur.monthly_cap_home_krs, cur.monthly_cap_fb_krs,
                cap, None, now,
            )
            .map_err(panel_account_err)?;
        Ok(json!({"ok": true}))
    }

    pub fn set_home_only(&mut self, username: &str, home_only: bool, now: u64) -> Result<Value, ApiError> {
        let cur = self.panel.accounts.get(username).cloned().ok_or_else(|| {
            ApiError::new(404, "account_not_found", "hesap bulunamadi")
        })?;
        self.panel
            .set_limits(
                "admin", username,
                cur.daily_cap_home_krs, cur.daily_cap_fb_krs,
                cur.monthly_cap_home_krs, cur.monthly_cap_fb_krs,
                cur.fallback_daily_cap_krs, Some(home_only), now,
            )
            .map_err(panel_account_err)?;
        Ok(json!({"ok": true, "home_only": home_only}))
    }

    pub fn hwid_reset(&mut self, username: &str, now: u64) -> Result<Value, ApiError> {
        let e = self.panel.reset_hwid("admin", username, now).map_err(panel_account_err)?;
        let auth_bridged = self.auth.reset_hwid(username, now).is_ok();
        Ok(json!({
            "ok": true, "username": e.username, "at": e.at,
            "slots_cleared": e.slots_cleared, "auth_bridged": auth_bridged,
        }))
    }

    pub fn suspend(&mut self, username: &str, stop: bool, now: u64) -> Result<Value, ApiError> {
        self.panel.suspend("admin", username, stop, now).map_err(panel_account_err)?;
        if stop {
            let _ = self.auth.suspend(username, now);
        } else {
            let _ = self.auth.unsuspend(username, now);
        }
        Ok(json!({"ok": true, "username": username, "suspended": stop}))
    }

    pub fn tariffs_get(&self) -> Value {
        match self.panel.tariffs.as_ref().map(|t| t.current()) {
            Some(t) => {
                let (fb_type, fb_value) = match t.fallback {
                    FallbackPrice::MultiplierBp(bp) => ("multiplier_bp", bp as i64),
                    FallbackPrice::FixedKrsPerMin(v) => ("fixed_krs_per_min", v),
                };
                json!({
                    "ok": true, "version": t.version,
                    "home_krs_per_min": t.home_krs_per_min,
                    "fallback_type": fb_type, "fallback_value": fb_value,
                    "upstream_min_secs": t.upstream_min_secs,
                })
            }
            None => json!({"ok": true, "version": 0}),
        }
    }

    pub fn tariffs_put(&mut self, v: &Value, now: u64) -> Result<Value, ApiError> {
        let home = v.get("home_krs_per_min").and_then(|x| x.as_i64()).ok_or_else(|| {
            ApiError::new(400, "invalid_amount", "home_krs_per_min gerekli")
        })?;
        if home < 0 {
            return Err(ApiError::new(400, "invalid_amount", "tarife negatif olamaz"));
        }
        let cur_fb = self.panel.tariffs.as_ref().map(|t| t.current().fallback);
        let fb = if let Some(f) = v.get("fallback_fixed_krs_per_min").and_then(|x| x.as_i64()) {
            if f < 0 {
                return Err(ApiError::new(400, "invalid_amount", "tarife negatif olamaz"));
            }
            FallbackPrice::FixedKrsPerMin(f)
        } else if let Some(bp) = v.get("fallback_multiplier_bp").and_then(|x| x.as_u64()) {
            FallbackPrice::MultiplierBp(bp)
        } else {
            cur_fb.unwrap_or(FallbackPrice::MultiplierBp(DEFAULT_FALLBACK_BP))
        };
        let t = self.panel.set_tariff("admin", home, fb, now);
        Ok(json!({"ok": true, "version": t.version, "home_krs_per_min": t.home_krs_per_min}))
    }

    pub fn switch_get(&self) -> Value {
        json!({"ok": true, "open": self.panel.switch.open})
    }

    pub fn switch_set(&mut self, open: bool, now: u64) -> Value {
        self.panel.set_switch("admin", open, now);
        json!({"ok": true, "open": open})
    }

    // -- fallback hat seçimi + hat tarifeleri + anahtar kasası --

    fn fb_price_json(p: &FallbackPrice) -> Value {
        match *p {
            FallbackPrice::MultiplierBp(bp) => json!({"type": "multiplier_bp", "value": bp}),
            FallbackPrice::FixedKrsPerMin(v) => json!({"type": "fixed_krs_per_min", "value": v}),
        }
    }

    /// Aktif hat + üç hattın fiyat/tabanı + anahtar VAR/YOK (değer asla).
    /// Local hat secretsizdir: key_set her zaman true, fiyat sabit 0.
    pub fn vendor_get(&self) -> Value {
        let tt = self.panel.tariffs.as_ref().map(|t| t.current());
        let (vendor, g, o, l) = match self.panel.tariffs.as_ref() {
            Some(t) => (
                t.vendor().name(),
                t.vendor_cfg(FallbackVendor::Groq),
                t.vendor_cfg(FallbackVendor::OpenAi),
                t.vendor_cfg(FallbackVendor::Local),
            ),
            None => ("groq", panel::tariffs::VendorCfg {
                price: FallbackPrice::MultiplierBp(DEFAULT_FALLBACK_BP),
                upstream_min_secs: panel::tariffs::DEFAULT_UPSTREAM_MIN_SECS,
            }, panel::tariffs::VendorCfg {
                price: FallbackPrice::MultiplierBp(DEFAULT_FALLBACK_BP),
                upstream_min_secs: panel::tariffs::OPENAI_UPSTREAM_MIN_SECS,
            }, panel::tariffs::VendorCfg {
                price: FallbackPrice::FixedKrsPerMin(0),
                upstream_min_secs: panel::tariffs::LOCAL_UPSTREAM_MIN_SECS,
            }),
        };
        let (groq_key, openai_key) = self.panel.keys_set();
        json!({
            "ok": true, "vendor": vendor,
            "version": tt.map(|t| t.version).unwrap_or(0),
            "groq": {"price": Self::fb_price_json(&g.price), "upstream_min_secs": g.upstream_min_secs, "key_set": groq_key},
            "openai": {"price": Self::fb_price_json(&o.price), "upstream_min_secs": o.upstream_min_secs, "key_set": openai_key},
            "local": {"price": Self::fb_price_json(&l.price), "upstream_min_secs": l.upstream_min_secs, "key_set": true},
        })
    }

    fn parse_vendor(s: &str) -> Result<FallbackVendor, ApiError> {
        FallbackVendor::parse(s).ok_or_else(|| {
            ApiError::new(400, "bad_vendor", "hat groq, openai ya da local olmali")
        })
    }

    fn parse_price(v: &Value) -> Result<FallbackPrice, ApiError> {
        if let Some(f) = v.get("fixed_krs_per_min").and_then(|x| x.as_i64()) {
            if f < 0 {
                return Err(ApiError::new(400, "invalid_amount", "tarife negatif olamaz"));
            }
            return Ok(FallbackPrice::FixedKrsPerMin(f));
        }
        if let Some(bp) = v.get("multiplier_bp").and_then(|x| x.as_u64()) {
            if bp == 0 || bp > 1_000_000 {
                return Err(ApiError::new(400, "invalid_amount", "carpan 1..1000000 bp olmali"));
            }
            return Ok(FallbackPrice::MultiplierBp(bp));
        }
        Err(ApiError::new(400, "invalid_amount", "fixed_krs_per_min ya da multiplier_bp gerekli"))
    }

    pub fn vendor_set(&mut self, vendor: &str, now: u64) -> Result<Value, ApiError> {
        let v = Self::parse_vendor(vendor)?;
        let t = self.panel.set_vendor("admin", v, now);
        Ok(json!({"ok": true, "vendor": v.name(), "version": t.version}))
    }

    pub fn vendor_price(&mut self, v: &Value, now: u64) -> Result<Value, ApiError> {
        let vendor = v.get("vendor").and_then(|x| x.as_str()).unwrap_or("");
        let vv = Self::parse_vendor(vendor)?;
        // Local hat ücretsizdir (sabit 0); ücretli fiyat yazılamaz.
        if vv == FallbackVendor::Local {
            let fixed = v.get("fixed_krs_per_min").and_then(|x| x.as_i64());
            let bp = v.get("multiplier_bp").and_then(|x| x.as_u64());
            let ok_zero = fixed == Some(0) && bp.is_none()
                || fixed.is_none() && bp.is_none();
            if !ok_zero {
                return Err(ApiError::new(400, "invalid_amount", "local hat ucretsizdir (sabit 0)"));
            }
            let t = self.panel.set_vendor_price("admin", vv, FallbackPrice::FixedKrsPerMin(0), now);
            return Ok(json!({"ok": true, "vendor": vv.name(), "version": t.version}));
        }
        let price = Self::parse_price(v)?;
        let t = self.panel.set_vendor_price("admin", vv, price, now);
        Ok(json!({"ok": true, "vendor": vv.name(), "version": t.version}))
    }

    pub fn vendor_upstream(&mut self, v: &Value, now: u64) -> Result<Value, ApiError> {
        let vendor = v.get("vendor").and_then(|x| x.as_str()).unwrap_or("");
        let vv = Self::parse_vendor(vendor)?;
        let secs = v.get("secs").and_then(|x| x.as_u64()).ok_or_else(|| {
            ApiError::new(400, "bad_request", "'secs' gerekli")
        })?;
        if secs > 3600 {
            return Err(ApiError::new(400, "bad_request", "taban en fazla 3600sn"));
        }
        let t = self.panel.set_vendor_upstream_min("admin", vv, secs, now);
        Ok(json!({"ok": true, "vendor": vv.name(), "upstream_min_secs": t.upstream_min_secs, "version": t.version}))
    }

    /// Sağlayıcı anahtarı gir (bellek-içi; yanıt/audit/loga değer YAZILMAZ).
    /// Local secretsizdir: anahtar gerekmez, boş anahtar local'i etkilemez.
    pub fn key_set(&mut self, v: &Value, now: u64) -> Result<Value, ApiError> {
        let vendor = v.get("vendor").and_then(|x| x.as_str()).unwrap_or("");
        let vv = Self::parse_vendor(vendor)?;
        if vv == FallbackVendor::Local {
            self.panel.set_provider_key("admin", vv, "", now);
            return Ok(json!({"ok": true, "vendor": vv.name(), "stored": true}));
        }
        let key = v.get("key").and_then(|x| x.as_str()).unwrap_or("");
        if key.len() > 2048 {
            return Err(ApiError::new(400, "bad_request", "anahtar en fazla 2048 karakter"));
        }
        let stored = self.panel.set_provider_key("admin", vv, key, now);
        Ok(json!({"ok": true, "vendor": vv.name(), "stored": stored}))
    }

    pub fn key_clear(&mut self, vendor: &str, now: u64) -> Result<Value, ApiError> {
        let vv = Self::parse_vendor(vendor)?;
        self.panel.set_provider_key("admin", vv, "", now);
        // Local secretsiz hazır kalır; temizleme etkilemez.
        let stored = vv == FallbackVendor::Local;
        Ok(json!({"ok": true, "vendor": vv.name(), "stored": stored}))
    }

    pub fn audit(&self, limit: usize) -> Value {
        let n = self.panel.audit.entries.len();
        let from = n.saturating_sub(limit);
        let arr: Vec<Value> = self.panel.audit.entries[from..]
            .iter()
            .map(|e| json!({"at": e.at, "admin": e.admin, "action": e.action, "detail": e.detail}))
            .collect();
        json!({"ok": true, "entries": arr})
    }

    pub fn disks_get(&self) -> Value {
        let arr: Vec<Value> = self.panel.disks.disks
            .iter()
            .map(|d| json!({"disk": d.kind.name(), "used_pct": d.used_pct, "warn": d.warn()}))
            .collect();
        json!({"ok": true, "disks": arr, "warnings": self.panel.disks.warnings()})
    }

    pub fn disks_put(&mut self, worker: u8, broker: u8, archive: u8) -> Value {
        self.panel.report_disks(worker, broker, archive);
        self.disks_get()
    }
}

// ---------------------------------------------------------------------------
// Testler
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn state(path: &str) -> BrokerState {
        let _ = std::fs::remove_file(path);
        // Anlık görüntü aynı dizinde durur; bayat dosya testi kirletmesin.
        let snap = std::path::Path::new(path)
            .parent()
            .map(|d| d.join("panel.json"))
            .unwrap_or_else(|| std::path::PathBuf::from("panel.json"));
        let _ = std::fs::remove_file(&snap);
        BrokerState::new(b"test-secret-xyz", path.to_string())
    }

    fn redeem_login(st: &mut BrokerState, now: u64) -> (String, String) {
        st.create_invite("ali", 10_000, Some("DAVET-1"), now).unwrap();
        let r = st.redeem("DAVET-1", "gizli-sifre-1", "hwid-A", now + 1).unwrap();
        assert_eq!(r["account"], json!("ali"));
        let access = r["access"].as_str().unwrap().to_string();
        (access, "ali".to_string())
    }

    /// Konuşma enerjili sahte gövde: 0x40 dolgusu (RMS ~0.5, sessizlik değil).
    fn speech_body(nbytes: usize) -> Vec<u8> {
        vec![0x40u8; nbytes]
    }

    #[test]
    fn redeem_login_transcribe_happy_path() {
        let mut st = state("test-ledger-happy.jsonl");
        let (access, _) = redeem_login(&mut st, 1000);
        assert_eq!(st.ledger.balance("ali"), 10_000);

        let body = speech_body(3200); // 1600 örnek / 16kHz = 0.1sn → 3sn quantum
        let h = sha256_hex(&body);
        let before = st.ledger.balance("ali");
        let r = st.transcribe(&access, "hwid-A", "req-1", &h, &body, 1100).unwrap();
        assert_eq!(r["ok"], json!(true));
        assert_eq!(r["cached"], json!(false));
        assert!(!r["text"].as_str().unwrap().is_empty());
        assert_eq!(r["billed_secs"], json!(3));
        // Ev 120kr/dk: 3sn → ceil(120*3/60) = 6 kuruş.
        assert_eq!(r["cost_kurus"], json!(6));
        assert_eq!(st.ledger.balance("ali"), before - 6);
        // Panel bakiyesi defterle aynı izde.
        assert_eq!(st.panel.accounts.get("ali").unwrap().balance_krs, before - 6);
        let _ = std::fs::remove_file("test-ledger-happy.jsonl");
    }

    #[test]
    fn bakiye_yetersiz_ret_ucretsiz() {
        let mut st = state("test-ledger-poor.jsonl");
        st.create_invite("yoksul", 0, Some("DAVET-0"), 1000).unwrap();
        st.redeem("DAVET-0", "gizli-sifre-1", "hwid-A", 1001).unwrap();
        let l = st.redeem("DAVET-0", "x", "h", 1002);
        assert!(l.is_err()); // davet tükendi (kurulum doğrulaması)
        // yoksul hesabıyla giriş yapıp transcribe dene.
        let p = st.login("yoksul", "gizli-sifre-1", "hwid-A", None, 1003).unwrap();
        let access = p["access"].as_str().unwrap().to_string();
        let body = speech_body(3200);
        let h = sha256_hex(&body);
        let n_before = st.ledger.entries().len();
        let err = st.transcribe(&access, "hwid-A", "req-1", &h, &body, 1100).unwrap_err();
        assert_eq!(err.status, 402);
        assert_eq!(err.code, "insufficient_balance");
        // Ücret satırı YAZILMADI, bakiye aynı.
        assert_eq!(st.ledger.entries().len(), n_before);
        assert_eq!(st.ledger.balance("yoksul"), 0);
        let _ = std::fs::remove_file("test-ledger-poor.jsonl");
    }

    #[test]
    fn ayni_id_tekrar_ucretsiz() {
        let mut st = state("test-ledger-replay.jsonl");
        let (access, _) = redeem_login(&mut st, 1000);
        let body = speech_body(3200);
        let h = sha256_hex(&body);
        let r1 = st.transcribe(&access, "hwid-A", "req-7", &h, &body, 1100).unwrap();
        let after_first = st.ledger.balance("ali");
        let text1 = r1["text"].as_str().unwrap().to_string();
        // Aynı ID + aynı ses → önbellekten ücretsiz.
        let r2 = st.transcribe(&access, "hwid-A", "req-7", &h, &body, 1101).unwrap();
        assert_eq!(r2["cached"], json!(true));
        assert_eq!(r2["text"], json!(text1));
        assert_eq!(r2["cost_kurus"], json!(0));
        assert_eq!(st.ledger.balance("ali"), after_first);
        // Aynı ID farklı ses → YENİ istek sayılır, normal ücretlenir.
        let body2 = speech_body(6400);
        let h2 = sha256_hex(&body2);
        let r3 = st.transcribe(&access, "hwid-A", "req-7", &h2, &body2, 1102).unwrap();
        assert_eq!(r3["cached"], json!(false));
        assert!(st.ledger.balance("ali") < after_first);
        let _ = std::fs::remove_file("test-ledger-replay.jsonl");
    }

    #[test]
    fn sessizlik_ve_hash_uyusmazligi_ucretsiz_ret() {
        let mut st = state("test-ledger-gate.jsonl");
        let (access, _) = redeem_login(&mut st, 1000);
        let before = st.ledger.balance("ali");
        // Sessizlik (tamamı sıfır) → ücretsiz ret.
        let silent = vec![0u8; 3200];
        let hs = sha256_hex(&silent);
        let err = st.transcribe(&access, "hwid-A", "req-s", &hs, &silent, 1100).unwrap_err();
        assert_eq!(err.code, "silent_audio");
        assert_eq!(st.ledger.balance("ali"), before);
        // Hash uyuşmazlığı → ret.
        let body = speech_body(3200);
        let err = st.transcribe(&access, "hwid-A", "req-x", "00ab", &body, 1100).unwrap_err();
        assert_eq!(err.code, "invalid_audio_hash");
        assert_eq!(st.ledger.balance("ali"), before);
        let _ = std::fs::remove_file("test-ledger-gate.jsonl");
    }

    #[test]
    fn suspend_false_reopens_auth_and_panel_consistent() {
        let mut st = state("test-ledger-suspend.jsonl");
        let (redeem_access, _) = redeem_login(&mut st, 1000);
        // Suspend-öncesi refresh'i de yakala (ölü-kalma kanıtı için).
        let p = st.login("ali", "gizli-sifre-1", "hwid-A", None, 1002).unwrap();
        let pre_access = p["access"].as_str().unwrap().to_string();
        let pre_refresh = p["refresh"].as_str().unwrap().to_string();

        // Durdur: panel + auth kapanır.
        st.suspend("ali", true, 1010).unwrap();
        assert!(st.auth.account("ali").unwrap().suspended);
        // Panel `users` görünümü kapalı gösterir.
        let users = st.users(1011);
        let row = users["users"].as_array().unwrap().iter()
            .find(|r| r["username"] == json!("ali")).expect("ali satiri");
        assert_eq!(row["suspended"], json!(true));
        // Auth kapalı: login 403.
        let err = st.login("ali", "gizli-sifre-1", "hwid-A", None, 1012).unwrap_err();
        assert_eq!(err.status, 403);
        assert_eq!(err.code, "account_suspended");
        // Suspend-öncesi token sonraki kabulde ölü (revoke).
        let body = speech_body(3200);
        let h = sha256_hex(&body);
        let err = st.transcribe(&pre_access, "hwid-A", "req-pre", &h, &body, 1013).unwrap_err();
        assert_eq!(err.code, "token_revoked");
        let err = st.transcribe(&redeem_access, "hwid-A", "req-pre2", &h, &body, 1013).unwrap_err();
        assert_eq!(err.code, "token_revoked");

        // Yeniden aç: panel + auth açılır.
        st.suspend("ali", false, 1020).unwrap();
        assert!(!st.auth.account("ali").unwrap().suspended);
        let users = st.users(1021);
        let row = users["users"].as_array().unwrap().iter()
            .find(|r| r["username"] == json!("ali")).expect("ali satiri");
        assert_eq!(row["suspended"], json!(false));
        // Yeni login çalışır (200 eşdeğeri Ok).
        let p2 = st.login("ali", "gizli-sifre-1", "hwid-A", None, 1022).unwrap();
        assert_eq!(p2["ok"], json!(true));
        let new_access = p2["access"].as_str().unwrap().to_string();
        // Suspend-öncesi access/refresh hâlâ geçersiz.
        let err = st.transcribe(&pre_access, "hwid-A", "req-old", &h, &body, 1023).unwrap_err();
        assert_eq!(err.code, "token_revoked");
        let err = st.refresh(&pre_refresh, "hwid-A", 1023).unwrap_err();
        assert_eq!(err.code, "unauthorized"); // refresh temizlendi → BadToken
        // Yeni token ile transcribe çalışır.
        let r = st.transcribe(&new_access, "hwid-A", "req-new", &h, &body, 1024).unwrap();
        assert_eq!(r["ok"], json!(true));
        let _ = std::fs::remove_file("test-ledger-suspend.jsonl");
    }

    #[test]
    fn me_returns_balance_and_tariff_without_writes() {
        let mut st = state("test-ledger-me.jsonl");
        let (access, _) = redeem_login(&mut st, 1000);
        let n_before = st.ledger.entries().len();
        let v = st.me(&access, "hwid-A", 1100).unwrap();
        assert_eq!(v["account"], json!("ali"));
        assert_eq!(v["balance_kurus"], json!(10_000));
        assert_eq!(v["home_krs_per_min"], json!(120));
        assert!(v["fallback_krs_per_min"].as_i64().unwrap() > 0);
        // Okuma deftere yazmaz.
        assert_eq!(st.ledger.entries().len(), n_before);
        // Bozuk jeton reddedilir.
        assert!(st.me("bozuk", "hwid-A", 1100).is_err());
        let _ = std::fs::remove_file("test-ledger-me.jsonl");
    }

    #[test]
    fn vendor_switch_price_and_key_vault() {
        let mut st = state("test-ledger-vendor.jsonl");
        redeem_login(&mut st, 1000);
        // Varsayılan hat Groq, anahtarlar boş.
        let v = st.vendor_get();
        assert_eq!(v["vendor"], json!("groq"));
        assert_eq!(v["groq"]["key_set"], json!(false));
        assert_eq!(v["openai"]["key_set"], json!(false));
        // Hat değişimi + hat fiyatı + taban (sürümler artar).
        let v0 = v["version"].as_u64().unwrap();
        let r = st.vendor_set("openai", 1010).unwrap();
        assert_eq!(r["vendor"], json!("openai"));
        assert!(r["version"].as_u64().unwrap() > v0);
        let r = st.vendor_price(&json!({"vendor": "openai", "fixed_krs_per_min": 300}), 1011).unwrap();
        assert!(r["version"].as_u64().unwrap() > v0);
        let v = st.vendor_get();
        assert_eq!(v["openai"]["price"], json!({"type": "fixed_krs_per_min", "value": 300}));
        assert_eq!(v["openai"]["upstream_min_secs"], json!(3));
        // Bozuk hat adı reddedilir.
        assert!(st.vendor_set("derin-ses", 1012).is_err());
        // Anahtar kasası: değer yanıta/audit'e sızmaz.
        let r = st.key_set(&json!({"vendor": "groq", "key": "gizli-anahtar"}), 1013).unwrap();
        assert_eq!(r, json!({"ok": true, "vendor": "groq", "stored": true}));
        assert!(!st.vendor_get().to_string().contains("gizli"));
        let audit = st.audit(50).to_string();
        assert!(!audit.contains("gizli"));
        let r = st.key_clear("groq", 1014).unwrap();
        assert_eq!(r["stored"], json!(false));
        assert_eq!(st.vendor_get()["groq"]["key_set"], json!(false));
        let _ = std::fs::remove_file("test-ledger-vendor.jsonl");
    }

    #[test]
    fn local_vendor_free_secretsiz_auditli() {
        let mut st = state("test-ledger-local.jsonl");
        redeem_login(&mut st, 1000);
        // Yönetimden local hatta geç: anahtar gerekmez.
        let r = st.vendor_set("local", 1010).unwrap();
        assert_eq!(r["vendor"], json!("local"));
        let v = st.vendor_get();
        assert_eq!(v["vendor"], json!("local"));
        assert_eq!(v["local"]["key_set"], json!(true));
        assert_eq!(v["local"]["price"], json!({"type": "fixed_krs_per_min", "value": 0}));
        // Local fiyat ücretli yapılamaz; boş/0 kabul, diğerleri ret.
        assert!(st.vendor_price(&json!({"vendor": "local", "fixed_krs_per_min": 900}), 1011).is_err());
        assert!(st.vendor_price(&json!({"vendor": "local", "fixed_krs_per_min": 0}), 1012).is_ok());
        // Boş anahtar local'i etkilemez (secretsiz hazır kalır).
        let r = st.key_set(&json!({"vendor": "local"}), 1013).unwrap();
        assert_eq!(r["stored"], json!(true));
        let r = st.key_clear("local", 1014).unwrap();
        assert_eq!(r["stored"], json!(true));
        assert_eq!(st.vendor_get()["local"]["key_set"], json!(true));
        // Groq/OpenAI akışı bozulmadı.
        assert!(st.vendor_set("groq", 1015).is_ok());
        assert_eq!(st.vendor_get()["vendor"], json!("groq"));
        // Audit izi: hat-secimi local.
        let audit = st.audit(50).to_string();
        assert!(audit.contains("hat-secimi") && audit.contains("local"));
        let _ = std::fs::remove_file("test-ledger-local.jsonl");
    }

    #[test]
    fn snapshot_survives_restart() {
        // Yeniden başlatma: admin kurulumu + davet + hesap kaybolmaz.
        let dir = std::env::temp_dir().join("whisperexe-broker-snap");
        let _ = std::fs::create_dir_all(&dir);
        let ledger = dir.join("ledger.jsonl").to_string_lossy().to_string();
        let snap = dir.join("panel.json");
        let _ = std::fs::remove_file(&ledger);
        let _ = std::fs::remove_file(&snap);
        let secret = b"snapshot-test-secret";
        let mut st = BrokerState::new(secret, ledger.clone());
        st.admin_setup("yonetici-sifresi-123").unwrap();
        st.create_invite("ali", 5000, Some("DAVET-SNAP"), 1000).unwrap();
        st.redeem("DAVET-SNAP", "gizli-sifre-1", "hwid-A", 1001).unwrap();
        st.save_snapshot();
        assert!(snap.exists(), "anlik goruntu yazilmali");
        // Ayni sırla yeniden başlat: her şey yerinde.
        let mut st2 = BrokerState::new(secret, ledger.clone());
        assert!(st2.admin_login("yonetici-sifresi-123", 1002).is_ok());
        assert!(st2.panel.accounts.invites.contains_key("DAVET-SNAP"));
        assert!(st2.panel.accounts.get("ali").is_some());
        let p = st2.login("ali", "gizli-sifre-1", "hwid-A", None, 1003).unwrap();
        assert_eq!(p["account"], json!("ali"));
        // Sırlar dosyaya SIZMAZ (hash'ler argon2, anahtar yok).
        let raw = std::fs::read_to_string(&snap).unwrap();
        assert!(!raw.contains("gizli-sifre-1"));
        let _ = std::fs::remove_file(&ledger);
        let _ = std::fs::remove_file(&snap);
    }

    #[test]
    fn topup_deduct_guards() {
        let mut st = state("test-ledger-topup-deduct.jsonl");
        redeem_login(&mut st, 1000);
        assert_eq!(st.ledger.balance("ali"), 10_000);
        // Pozitif yükleme.
        let v = st.topup("ali", 500, 1001).unwrap();
        assert_eq!(v["balance_krs"], json!(10_500));
        // Sıfır/negatif ret (yükleme ve düşürme).
        assert_eq!(st.topup("ali", 0, 1002).unwrap_err().code, "invalid_amount");
        assert_eq!(st.topup("ali", -5, 1002).unwrap_err().code, "invalid_amount");
        assert_eq!(st.deduct("ali", 0, 1002).unwrap_err().code, "invalid_amount");
        assert_eq!(st.deduct("ali", -5, 1002).unwrap_err().code, "invalid_amount");
        // Pozitif düşürme + ayrı Deduct izi + admin audit izi.
        let n_before = st.ledger.entries().len();
        let v = st.deduct("ali", 1500, 1003).unwrap();
        assert_eq!(v["balance_krs"], json!(9_000));
        assert_eq!(st.ledger.entries().len(), n_before + 1);
        assert!(matches!(
            st.ledger.entries().last().map(|e| e.kind),
            Some(ledger::Kind::Deduct)
        ));
        assert!(st.audit(50).to_string().contains("bakiye-dusur"));
        // Yetersiz bakiye: 402 insufficient_balance, bakiye ve satır sayısı aynı.
        let n_before = st.ledger.entries().len();
        let err = st.deduct("ali", 9_001, 1004).unwrap_err();
        assert_eq!(err.status, 402);
        assert_eq!(err.code, "insufficient_balance");
        assert_eq!(st.ledger.balance("ali"), 9_000);
        assert_eq!(st.ledger.entries().len(), n_before);
        // Tamamını düşürmek serbest, eksiye düşürmek yasak.
        st.deduct("ali", 9_000, 1005).unwrap();
        assert_eq!(st.ledger.balance("ali"), 0);
        assert_eq!(st.deduct("ali", 1, 1006).unwrap_err().code, "insufficient_balance");
        // Sır sızmaz: hata gövdesinde jeton/şifre yok.
        let body = st.deduct("ali", 1, 1007).unwrap_err().body();
        assert!(!body.contains("gizli-sifre-1"));
        let _ = std::fs::remove_file("test-ledger-topup-deduct.jsonl");
    }
}
