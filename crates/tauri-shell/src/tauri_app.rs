//! Tauri çalışma-zamanı kablolaması — YALNIZCA `tauri` özelliğiyle derlenir.
//!
//! docs/TAURI.md §"Tauri kablolama taslağı"nın as-built karşılığı:
//! - F9 (global-shortcut): basıldı → [`Shell::hotkey_down`], bırakıldı →
//!   [`Shell::hotkey_up`]; her değişimde [`overlay::render`] çıktısı `overlay`
//!   penceresine `emit("overlay", view)` edilir.
//! - Açılış: broker `GET /v1/version` → [`Shell::boot`]; taban altı girişte
//!   engeller, zorunlu kurulum YENİDEN BAŞLATMADA uygulanır (dictation
//!   ortasında ASLA). Bildiri okunamazsa fail-open + uyarı toast'ı.
//! - Güncelleme: Tauri updater + broker feed; ÖZEL İNDİRİCİ YOK. Zorunlu
//!   kurulum YALNIZCA boşta (kayıt/in-flight yokken) uygulanır.
//! - Gerçek mikrofon donanımı bu modülde YOKTUR: `push_second` beslemesi
//!   donanım `Encoder` arayüzü geldiğinde bağlanır; o güne dek mock yolu
//!   korunur (kayıt bırakma = sessiz/süre-sıfır yolu).

use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use crate::flow::{BootApply, Shell};
use crate::{overlay, version};

/// Derleme-zamanında doğrulanan broker tabanı (`build.rs`, `TAURI_BROKER_HOST`).
pub(crate) const BROKER_BASE: &str = env!("WHISPER_BROKER_BASE");

/// Açılış sürüm sorusu için üst sınır (fail-open; kilitli akış kesilmez).
const BOOT_TIMEOUT: Duration = Duration::from_secs(5);

/// Boştaki zorunlu-kurulum denetimi aralığı.
const IDLE_POLL: Duration = Duration::from_secs(5);

struct AppState {
    shell: Mutex<Shell>,
}

pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            shell: Mutex::new(Shell::new(false)),
        })
        .manage(Mutex::new(crate::session::UserSession::default()))
        .manage(Mutex::new(crate::session::AdminSession::default()))
        .invoke_handler(tauri::generate_handler![
            crate::commands::user_status,
            crate::commands::user_redeem,
            crate::commands::user_login,
            crate::commands::user_me,
            crate::commands::user_logout,
            crate::commands::broker_info,
            crate::commands::open_admin,
            crate::commands::admin_status,
            crate::commands::admin_login,
            crate::commands::admin_logout,
            crate::commands::admin_users,
            crate::commands::admin_user,
            crate::commands::admin_audit,
            crate::commands::admin_disks,
            crate::commands::admin_tariffs,
            crate::commands::admin_switch,
            crate::commands::admin_vendor,
            crate::commands::admin_invite,
            crate::commands::admin_topup,
            crate::commands::admin_limits,
            crate::commands::admin_home_only,
            crate::commands::admin_hwid_reset,
            crate::commands::admin_suspend,
            crate::commands::admin_set_tariff,
            crate::commands::admin_set_switch,
            crate::commands::admin_set_vendor,
            crate::commands::admin_vendor_price,
            crate::commands::admin_vendor_upstream,
            crate::commands::admin_key_set,
            crate::commands::admin_key_clear,
        ])
        .setup(|app| {
            boot_from_broker(app.handle());
            #[cfg(desktop)]
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())?;
            #[cfg(desktop)]
            {
                use tauri_plugin_global_shortcut::{
                    Code, Modifiers, ShortcutState,
                };
                let plugin = tauri_plugin_global_shortcut::Builder::new()
                    .with_shortcuts(["F9"])
                    .map(|b| {
                        b.with_handler(|app, shortcut, event| {
                            if !shortcut.matches(Modifiers::empty(), Code::F9) {
                                return;
                            }
                            let open = {
                                let state = app.state::<AppState>();
                                let mut shell =
                                    state.shell.lock().expect("shell kilidi");
                                match event.state {
                                    ShortcutState::Pressed => shell.hotkey_down(),
                                    ShortcutState::Released => {
                                        shell.hotkey_up();
                                        true
                                    }
                                }
                            };
                            let _ = open;
                            emit_overlay(app);
                        })
                        .build()
                    });
                match plugin {
                    Ok(p) => {
                        if let Err(e) = app.handle().plugin(p) {
                            eprintln!("tauri: F9 eklentisi kurulamadi: {e}");
                            f9_warn(app.handle());
                        }
                    }
                    _ => {
                        eprintln!("tauri: F9 baska programda, global tus yok");
                        f9_warn(app.handle());
                    }
                }
            }
            spawn_idle_updater(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("tauri kabugu baslatilamadi");
}

/// Kabuk durumunu `overlay` penceresine iter + pencere görünürlüğünü senkronlar.
///
/// Görünürlük kuralı (client-ui semantiği): rozet/toast `Hidden` üstünde de
/// taşınır; bu yüzden pencere YALNIZCA durum `hidden` VE toast VE rozet
/// yokken gizlenir. Açılış `hidden` = normal (pencere `visible: false` gelir).
pub fn emit_overlay(app: &AppHandle) {
    let view = {
        let state = app.state::<AppState>();
        let shell = state.shell.lock().expect("shell kilidi");
        overlay::render(shell.ui())
    };
    let visible =
        view.state != "hidden" || view.toast.is_some() || view.low_balance.is_some();
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.emit("overlay", &view);
        if visible {
            let _ = win.show();
        } else {
            let _ = win.hide();
        }
    } else {
        let _ = app.emit("overlay", &view);
    }
}

/// F9 alınamadı uyarısı (geçici toast; kapanmadan devam edilir).
fn f9_warn(app: &AppHandle) {
    {
        let state = app.state::<AppState>();
        state.shell.lock().expect("shell kilidi").f9_unavailable();
    }
    emit_overlay(app);
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(8));
        let state = handle.state::<AppState>();
        state.shell.lock().expect("shell kilidi").dismiss();
        emit_overlay(&handle);
    });
}

/// Açılış kapısı: broker bildirimi → `Shell::boot`. Ağ işi ayrı iş parçacığında
/// yapılır (kurulum engellenmez); zaman aşımı/bozuk yanıt = fail-open + toast.
fn boot_from_broker(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        let url = version::endpoint_url(BROKER_BASE);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let body = ureq::get(&url)
                .call()
                .ok()
                .and_then(|mut res| res.body_mut().read_to_string().ok());
            let _ = tx.send(body);
        });
        let body = rx.recv_timeout(BOOT_TIMEOUT).ok().flatten();
        let unknown = {
            let state = handle.state::<AppState>();
            let mut shell = state.shell.lock().expect("shell kilidi");
            match body {
                Some(json) => match shell.boot(env!("CARGO_PKG_VERSION"), &json) {
                    Ok(BootApply::VersionUnknown) => {
                        shell.boot_unknown_version();
                        true
                    }
                    Ok(_) => false,
                    Err(_) => {
                        shell.boot_unknown_version();
                        true
                    }
                },
                None => {
                    shell.boot_unknown_version();
                    true
                }
            }
        };
        emit_overlay(&handle);
        if unknown {
            // Açılış uyarısı geçicidir: 8sn sonra söner, boş hayalet kalmaz.
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(8));
                let state = handle.state::<AppState>();
                state.shell.lock().expect("shell kilidi").dismiss();
                emit_overlay(&handle);
            });
        }
    });
}

/// Zorunlu kurulum YALNIZCA boşta uygulanır: kayıt sürmüyor VE sonuç hattı boş
/// VE `update_due_at_restart` kuruluysa updater çalışır (dictation kesilmez).
fn spawn_idle_updater(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(IDLE_POLL);
        let due = {
            let state = app.state::<AppState>();
            let shell = state.shell.lock().expect("shell kilidi");
            shell.update_due_at_restart() && !shell.recording() && !shell.in_flight()
        };
        if !due {
            continue;
        }
        #[cfg(desktop)]
        {
            let applied =
                tauri::async_runtime::block_on(try_install_update(&app));
            if applied {
                let state = app.state::<AppState>();
                state.shell.lock().expect("shell kilidi").restart_applied();
                app.restart();
            }
        }
    });
}

/// Tek deneme: beslemede yenilik varsa indir + kur. Özel indirici YOK.
#[cfg(desktop)]
async fn try_install_update(app: &AppHandle) -> bool {
    use tauri_plugin_updater::UpdaterExt;
    let updater = match app.updater() {
        Ok(u) => u,
        Err(_) => return false,
    };
    let update = match updater.check().await {
        Ok(Some(u)) => u,
        _ => return false,
    };
    update.download_and_install(|_, _| {}, || {}).await.is_ok()
}
