//! Tauri komutlari — YALNIZCA `tauri` ozelligiyle derlenir.
//!
//! Kural: sirlar (jeton, parola, anahtar) Rust tarafinda kalir; JS'e deger
//! ASLA donmez, hata metinlerine ASLA girmez. Admin jetonu `AdminSession`da,
//! kullanici cifti `UserSession`da tutulur (islem bellegi).

use std::sync::Mutex;

use serde_json::{json, Value};

use crate::session::{now_unix, AdminSession, UserSession};
use crate::{net, tauri_app};

fn base() -> &'static str {
    tauri_app::BROKER_BASE
}

/// Kalici oturum dosyasi (app-data; yoksa/okunamazsa fail-open).
pub(crate) fn session_path(app: &tauri::AppHandle) -> std::path::PathBuf {
    use tauri::Manager;
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("whisperexe"))
        .join(crate::session::SESSION_FILE_NAME)
}

/// Giris/davet/yenileme sonrasi cift diske yazilir (hata sessiz: giris
/// yine de gecerli, sadece hatirlanmaz).
fn persist(app: &tauri::AppHandle, s: &UserSession) {
    if let Err(e) = s.save_to_file(&session_path(app)) {
        eprintln!("oturum kaydi yazilamadi: {e}");
    }
}

fn auth_headers(sess: &UserSession) -> Result<[(&'static str, String); 2], String> {
    let t = sess.access().ok_or_else(|| "giris-gerekli".to_string())?;
    Ok([
        ("Authorization", format!("Bearer {t}")),
        ("X-Hwid", sess.hwid().to_string()),
    ])
}

fn admin_headers(sess: &AdminSession) -> Result<[(&'static str, String); 1], String> {
    let t = sess.token().ok_or_else(|| "admin-giris-gerekli".to_string())?;
    Ok([("X-Admin-Token", t.to_string())])
}

// ---- kullanici ----

/// Durum satiri: giris, hesap, bakiye (son bilinen), broker.
#[tauri::command]
pub fn user_status(u: tauri::State<Mutex<UserSession>>) -> Value {
    let s = u.lock().expect("oturum kilidi");
    let now = now_unix();
    json!({
        "logged_in": s.logged_in(),
        "account": s.account(),
        "hwid": s.hwid(),
        "broker": base(),
        "balance_kurus": s.balance_kurus(),
        "access_valid": s.access_valid(now),
    })
}

/// Davetle ilk giris: kod + kullanici + sifre (en az 12 karakter).
#[tauri::command]
pub fn user_redeem(app: tauri::AppHandle, u: tauri::State<Mutex<UserSession>>, code: String, username: String, password: String) -> Result<Value, String> {
    if password.len() < client::auth::MIN_PASSWORD_LEN {
        return Err("zayif-sifre-12".to_string());
    }
    let hwid = {
        let s = u.lock().expect("oturum kilidi");
        s.hwid().to_string()
    };
    let v = net::post(
        base(),
        "/v1/redeem",
        &[],
        &json!({"code": code, "password": password, "hwid": hwid}),
    )?;
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let num = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    {
        let mut s = u.lock().expect("oturum kilidi");
        s.set_pair(&username, get("access"), get("refresh"), num("access_expires_at"), num("refresh_expires_at"));
        if let Some(b) = v.get("balance_kurus").and_then(|x| x.as_i64()) {
            s.set_balance(b);
        }
        persist(&app, &s);
    }
    Ok(json!({"ok": true, "account": username}))
}

/// Hesap girisi: hesap + sifre (HWID otomatik).
#[tauri::command]
pub fn user_login(app: tauri::AppHandle, u: tauri::State<Mutex<UserSession>>, account: String, password: String) -> Result<Value, String> {
    let hwid = {
        let s = u.lock().expect("oturum kilidi");
        s.hwid().to_string()
    };
    let v = net::post(
        base(),
        "/v1/login",
        &[],
        &json!({"account": account, "password": password, "hwid": hwid}),
    )?;
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let num = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    {
        let mut s = u.lock().expect("oturum kilidi");
        s.set_pair(&account, get("access"), get("refresh"), num("access_expires_at"), num("refresh_expires_at"));
        persist(&app, &s);
    }
    Ok(json!({"ok": true, "account": account}))
}

/// Kaydedilmis refresh ile sessiz yenileme (acilis + access bitimi).
/// Basariliysa cift doner ve diske yazilir; refresh de bitmisse hata
/// doner (on-yuz giris formuna duser, donma YOK).
#[tauri::command]
pub fn user_refresh(app: tauri::AppHandle, u: tauri::State<Mutex<UserSession>>) -> Result<Value, String> {
    let (account, refresh, hwid) = {
        let s = u.lock().expect("oturum kilidi");
        (
            s.account().unwrap_or("").to_string(),
            s.refresh_token().ok_or_else(|| "giris-gerekli".to_string())?.to_string(),
            s.hwid().to_string(),
        )
    };
    if account.is_empty() {
        return Err("giris-gerekli".to_string());
    }
    let v = net::post(
        base(),
        "/v1/refresh",
        &[],
        &json!({"refresh": refresh, "hwid": hwid}),
    )?;
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let num = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    {
        let mut s = u.lock().expect("oturum kilidi");
        s.set_pair(&account, get("access"), get("refresh"), num("access_expires_at"), num("refresh_expires_at"));
        persist(&app, &s);
    }
    Ok(json!({"ok": true, "account": account}))
}

/// Bakiye + tarife (girisli hesap).
#[tauri::command]
pub fn user_me(u: tauri::State<Mutex<UserSession>>) -> Result<Value, String> {
    let h = {
        let s = u.lock().expect("oturum kilidi");
        auth_headers(&s)?
    };
    let v = net::get_auth(base(), "/v1/me", &h)?;
    if let Some(b) = v.get("balance_kurus").and_then(|x| x.as_i64()) {
        u.lock().expect("oturum kilidi").set_balance(b);
    }
    Ok(v)
}

/// Cikis: cift bellekten + diskten silinir (hatirla temizlenir).
#[tauri::command]
pub fn user_logout(app: tauri::AppHandle, u: tauri::State<Mutex<UserSession>>) -> Value {
    u.lock().expect("oturum kilidi").clear();
    let _ = std::fs::remove_file(session_path(&app));
    json!({"ok": true})
}

/// Broker bildirimi (surum + taban; girissiz).
#[tauri::command]
pub fn broker_info() -> Result<Value, String> {
    net::get(base(), "/v1/version")
}

/// Bas-güncelle: GitHub son kurulumu indirip çalıştırır, uygulamayı kapatır.
/// İmza doğrulaması YOK (sahip kararı); kaynak `update::RELEASES_LATEST` sabiti.
#[tauri::command]
pub fn fetch_update(app: tauri::AppHandle) -> Result<Value, String> {
    let (tag, path) = crate::update::fetch_and_stage()?;
    std::process::Command::new(&path)
        .spawn()
        .map_err(|e| format!("baslatma-hatasi:{e}"))?;
    std::thread::sleep(std::time::Duration::from_millis(1500));
    app.exit(0);
    Ok(json!({"ok": true, "tag": tag}))
}

/// Yonetim artik ayri pencere degil, ana pencerede "Yonetim" sekmesidir.
/// Bu komut pencere ACMAZ; on-yuz sekmeye gecer. Geriye uyumluluk icin
/// `{"ok": true}` doner (eski cagiranlar bozulmaz).
#[tauri::command]
pub fn open_admin() -> Result<Value, String> {
    Ok(json!({"ok": true}))
}

// ---- yonetim ----

#[tauri::command]
pub fn admin_status(a: tauri::State<Mutex<AdminSession>>) -> Value {
    let s = a.lock().expect("admin kilidi");
    json!({"active": s.active(now_unix())})
}

#[tauri::command]
pub fn admin_login(a: tauri::State<Mutex<AdminSession>>, password: String) -> Result<Value, String> {
    let v = net::post(base(), "/v1/admin/login", &[], &json!({"password": password}))?;
    let tok = v.get("admin_token").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let exp = v.get("expires_at").and_then(|x| x.as_u64()).unwrap_or(0);
    if tok.is_empty() {
        return Err("bozuk-yanit".to_string());
    }
    a.lock().expect("admin kilidi").set(tok, exp);
    Ok(json!({"ok": true, "expires_at": exp}))
}

#[tauri::command]
pub fn admin_logout(a: tauri::State<Mutex<AdminSession>>) -> Value {
    a.lock().expect("admin kilidi").clear();
    json!({"ok": true})
}

fn aget(a: &tauri::State<Mutex<AdminSession>>, path: &str) -> Result<Value, String> {
    let h = {
        let s = a.lock().expect("admin kilidi");
        admin_headers(&s)?
    };
    net::get_auth(base(), path, &h)
}

#[tauri::command]
pub fn admin_users(a: tauri::State<Mutex<AdminSession>>) -> Result<Value, String> {
    aget(&a, "/v1/users")
}

#[tauri::command]
pub fn admin_user(a: tauri::State<Mutex<AdminSession>>, username: String) -> Result<Value, String> {
    aget(&a, &format!("/v1/users/{username}"))
}

#[tauri::command]
pub fn admin_audit(a: tauri::State<Mutex<AdminSession>>) -> Result<Value, String> {
    aget(&a, "/v1/audit")
}

#[tauri::command]
pub fn admin_disks(a: tauri::State<Mutex<AdminSession>>) -> Result<Value, String> {
    aget(&a, "/v1/disks")
}

#[tauri::command]
pub fn admin_tariffs(a: tauri::State<Mutex<AdminSession>>) -> Result<Value, String> {
    aget(&a, "/v1/tariffs")
}

#[tauri::command]
pub fn admin_switch(a: tauri::State<Mutex<AdminSession>>) -> Result<Value, String> {
    aget(&a, "/v1/fallback/switch")
}

#[tauri::command]
pub fn admin_vendor(a: tauri::State<Mutex<AdminSession>>) -> Result<Value, String> {
    aget(&a, "/v1/fallback/vendor")
}

fn apost(a: &tauri::State<Mutex<AdminSession>>, path: &str, body: &Value) -> Result<Value, String> {
    let h = {
        let s = a.lock().expect("admin kilidi");
        admin_headers(&s)?
    };
    net::post(base(), path, &h, body)
}

fn aput(a: &tauri::State<Mutex<AdminSession>>, path: &str, body: &Value) -> Result<Value, String> {
    let h = {
        let s = a.lock().expect("admin kilidi");
        admin_headers(&s)?
    };
    net::put(base(), path, &h, body)
}

#[tauri::command]
pub fn admin_invite(
    a: tauri::State<Mutex<AdminSession>>,
    username: String,
    opening_krs: i64,
    code: Option<String>,
) -> Result<Value, String> {
    let mut b = json!({"username": username, "opening_krs": opening_krs});
    if let Some(c) = code {
        if !c.trim().is_empty() {
            b["code"] = json!(c);
        }
    }
    apost(&a, "/v1/invites", &b)
}

#[tauri::command]
pub fn admin_topup(a: tauri::State<Mutex<AdminSession>>, username: String, amount_krs: i64) -> Result<Value, String> {
    apost(&a, &format!("/v1/users/{username}/topup"), &json!({"amount_krs": amount_krs}))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn admin_limits(
    a: tauri::State<Mutex<AdminSession>>,
    username: String,
    daily_home: Option<i64>,
    daily_fb: Option<i64>,
    monthly_home: Option<i64>,
    monthly_fb: Option<i64>,
    fb_cap: Option<i64>,
    home_only: Option<bool>,
) -> Result<Value, String> {
    let mut b = json!({});
    if let Some(v) = daily_home {
        b["daily_home"] = json!(v);
    }
    if let Some(v) = daily_fb {
        b["daily_fb"] = json!(v);
    }
    if let Some(v) = monthly_home {
        b["monthly_home"] = json!(v);
    }
    if let Some(v) = monthly_fb {
        b["monthly_fb"] = json!(v);
    }
    if let Some(v) = fb_cap {
        b["fb_cap"] = json!(v);
    }
    if let Some(v) = home_only {
        b["home_only"] = json!(v);
    }
    aput(&a, &format!("/v1/users/{username}/limits"), &b)
}

#[tauri::command]
pub fn admin_home_only(a: tauri::State<Mutex<AdminSession>>, username: String, home_only: bool) -> Result<Value, String> {
    apost(&a, &format!("/v1/users/{username}/home-only"), &json!({"home_only": home_only}))
}

#[tauri::command]
pub fn admin_hwid_reset(a: tauri::State<Mutex<AdminSession>>, username: String) -> Result<Value, String> {
    apost(&a, &format!("/v1/users/{username}/hwid-reset"), &json!({}))
}

#[tauri::command]
pub fn admin_suspend(a: tauri::State<Mutex<AdminSession>>, username: String, stop: bool) -> Result<Value, String> {
    apost(&a, &format!("/v1/users/{username}/suspend"), &json!({"stop": stop}))
}

#[tauri::command]
pub fn admin_set_tariff(
    a: tauri::State<Mutex<AdminSession>>,
    home_krs_per_min: i64,
    fallback_fixed: Option<i64>,
    fallback_bp: Option<u64>,
) -> Result<Value, String> {
    let mut b = json!({"home_krs_per_min": home_krs_per_min});
    if let Some(v) = fallback_fixed {
        b["fallback_fixed_krs_per_min"] = json!(v);
    }
    if let Some(v) = fallback_bp {
        b["fallback_multiplier_bp"] = json!(v);
    }
    aput(&a, "/v1/tariffs", &b)
}

#[tauri::command]
pub fn admin_set_switch(a: tauri::State<Mutex<AdminSession>>, open: bool) -> Result<Value, String> {
    apost(&a, "/v1/fallback/switch", &json!({"open": open}))
}

#[tauri::command]
pub fn admin_set_vendor(a: tauri::State<Mutex<AdminSession>>, vendor: String) -> Result<Value, String> {
    apost(&a, "/v1/fallback/vendor", &json!({"vendor": vendor}))
}

#[tauri::command]
pub fn admin_vendor_price(
    a: tauri::State<Mutex<AdminSession>>,
    vendor: String,
    fixed: Option<i64>,
    bp: Option<u64>,
) -> Result<Value, String> {
    let mut b = json!({"vendor": vendor});
    if let Some(v) = fixed {
        b["fixed_krs_per_min"] = json!(v);
    }
    if let Some(v) = bp {
        b["multiplier_bp"] = json!(v);
    }
    aput(&a, "/v1/fallback/tariffs", &b)
}

#[tauri::command]
pub fn admin_vendor_upstream(a: tauri::State<Mutex<AdminSession>>, vendor: String, secs: u64) -> Result<Value, String> {
    aput(&a, "/v1/fallback/upstream", &json!({"vendor": vendor, "secs": secs}))
}

/// Saglayici anahtari gir (deger Rust'tan cikar, yanitta VAR/YOK doner).
#[tauri::command]
pub fn admin_key_set(a: tauri::State<Mutex<AdminSession>>, vendor: String, key: String) -> Result<Value, String> {
    apost(&a, "/v1/fallback/keys", &json!({"vendor": vendor, "key": key}))
}

#[tauri::command]
pub fn admin_key_clear(a: tauri::State<Mutex<AdminSession>>, vendor: String) -> Result<Value, String> {
    let h = {
        let s = a.lock().expect("admin kilidi");
        admin_headers(&s)?
    };
    net::delete(base(), "/v1/fallback/keys", &h, &format!("vendor={vendor}"))
}
