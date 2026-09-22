//! Tauri çalışma-zamanı kablolaması — YALNIZCA `tauri` özelliğiyle derlenir.
//!
//! docs/TAURI.md §"Tauri kablolama taslağı"nın as-built karşılığı:
//! - Kısayol (global-shortcut, varsayılan F9, ayarlardan değişir):
//!   basıldı → [`Shell::hotkey_down`], bırakıldı → [`Shell::hotkey_up`];
//!   her değişimde [`overlay::render`] çıktısı `overlay` penceresine
//!   `emit("overlay", view)` edilir. Kayıtlı tuş açılışta
//!   `hotkey.json`'dan okunur; değişimde eski kayıt silinip yenisi
//!   kurulur (unregister/register).
//! - Açılış: broker `GET /v1/version` → [`Shell::boot`]; taban altı girişte
//!   engeller, zorunlu kurulum YENİDEN BAŞLATMADA uygulanır (dictation
//!   ortasında ASLA). Bildiri okunamazsa fail-open + uyarı toast'ı.
//! - Güncelleme: Tauri updater + broker feed; ÖZEL İNDİRİCİ YOK. Açılışta bir
//!   kez sessiz besleme denetimi (10sn gecikmeli, 15sn tavanlı, hatada sessiz);
//!   yenilik + kayıtlı işaretten yeniyse `UpdatePending` bayrağı kurulur.
//!   Zorunlu kurulum YALNIZCA boşta (kayıt/in-flight yokken) uygulanır.
//! - Gerçek mikrofon donanımı bu modülde YOKTUR: `push_second` beslemesi
//!   donanım `Encoder` arayüzü geldiğinde bağlanır; o güne dek mock yolu
//!   korunur (kayıt bırakma = sessiz/süre-sıfır yolu).
//! - Görünürlük: pencere YALNIZCA [`overlay::is_visible`] `true` iken
//!   `show`, `false` iken kesin `hide` edilir (hayalet pencere YOK).
//!   `Done` 3sn sonra kendiliğinden `Hidden`'a düşer (Done->Hidden akışı
//!   hayalet bırakmaz); 8sn'lik uyarıların sönmesi `emit_overlay` ile olur.
//! - Giriş kaydı engellemez: oturum kapısı Tauri tarafında
//!   [`set_shell_logged_in`] ile kabuğa beslenir; girişsiz bırakmada
//!   gönderim YOK + `giris-gerekli` toast'ı (akış [`flow::Shell`]'de).

use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};

use crate::flow::{BootApply, Shell};
use crate::{overlay, version};

/// Derleme-zamanında doğrulanan broker tabanı (`build.rs`, `TAURI_BROKER_HOST`).
pub(crate) const BROKER_BASE: &str = env!("WHISPER_BROKER_BASE");

/// Açılış sürüm sorusu için üst sınır (fail-open; kilitli akış kesilmez).
const BOOT_TIMEOUT: Duration = Duration::from_secs(5);

/// Boştaki zorunlu-kurulum denetimi aralığı.
const IDLE_POLL: Duration = Duration::from_secs(5);

/// Açılış besleme denetimi gecikmesi: pencere önce açılır (≤15sn hedefi
/// etkilenmez); denetim ayrı iş parçacığında, sessizce yapılır.
const BOOT_UPDATE_DELAY: Duration = Duration::from_secs(10);

/// Besleme denetimi ağ tavanı: yanıt yoksa sessizce vazgeçilir (donma YOK,
/// sahte çevrimdışı toast'ı YOK).
const UPDATE_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) struct AppState {
    pub(crate) shell: Mutex<Shell>,
    pub(crate) hotkey: Mutex<String>,
    pub(crate) mic: Mutex<Option<String>>,
    /// Son birakmanin 16kHz mono sesi (yerel transkripsiyon girisi).
    pub(crate) last_pcm: Mutex<Vec<i16>>,
}

pub(crate) fn hotkey_path(app: &AppHandle) -> std::path::PathBuf {
    use tauri::Manager;
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("whisperexe"))
        .join(crate::hotkey::HOTKEY_FILE_NAME)
}

/// Secili mikrofon dosyasi (yoksa sistem varsayilani kullanilir).
pub(crate) fn mic_path(app: &AppHandle) -> std::path::PathBuf {
    use tauri::Manager;
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("whisperexe"))
        .join(crate::mic_cap::MIC_FILE_NAME)
}

/// Oturum kapısını kabuğa besler (giriş kaydı engellemez, gönderimi kapatır).
pub(crate) fn set_shell_logged_in(app: &AppHandle, v: bool) {
    if let Some(state) = app.try_state::<AppState>() {
        state.shell.lock().expect("shell kilidi").set_logged_in(v);
    }
}

pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            shell: Mutex::new(Shell::new(false)),
            hotkey: Mutex::new(crate::hotkey::DEFAULT_HOTKEY.to_string()),
            mic: Mutex::new(None),
            last_pcm: Mutex::new(Vec::new()),
        })
        .manage(Mutex::new(crate::session::UserSession::default()))
        .manage(Mutex::new(crate::session::AdminSession::default()))
        .invoke_handler(tauri::generate_handler![
            crate::commands::user_status,
            crate::commands::user_redeem,
            crate::commands::user_login,
            crate::commands::user_refresh,
            crate::commands::user_me,
            crate::commands::user_logout,
            crate::commands::broker_info,
            crate::commands::app_version,
            crate::commands::fetch_update,
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
            crate::commands::admin_deduct,
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
            crate::commands::hotkeyGet,
            crate::commands::hotkeySet,
            crate::commands::mic_list,
            crate::commands::mic_get,
            crate::commands::mic_set,
            crate::commands::server_start,
            crate::commands::server_status,
        ])
        .setup(|app| {
            restore_session(app.handle());
            // Açılışta kayıtlı tuş okunur (yoksa/bozuksa F9; fail-open).
            let saved = crate::hotkey::load_from_file(&hotkey_path(app.handle()));
            let mic = crate::mic_cap::load_from_file(&mic_path(app.handle()));
            if let Some(state) = app.handle().try_state::<AppState>() {
                *state.hotkey.lock().expect("kisayol kilidi") = saved.clone();
                *state.mic.lock().expect("mikrofon kilidi") = mic;
                let logged = app
                    .handle()
                    .try_state::<Mutex<crate::session::UserSession>>()
                    .map(|s| s.lock().expect("oturum kilidi").logged_in())
                    .unwrap_or(false);
                state.shell.lock().expect("shell kilidi").set_logged_in(logged);
            }
            boot_from_broker(app.handle());
            setup_tray(app.handle());
            #[cfg(desktop)]
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())?;
            #[cfg(desktop)]
            {
                use tauri_plugin_global_shortcut::ShortcutState;
                let plugin = tauri_plugin_global_shortcut::Builder::new()
                    .with_shortcuts([saved.as_str()])
                    .map(|b| {
                        b.with_handler(|app, _shortcut, event| {
                            // Tek kısayol kayıtlıdır; tuş-agnostik karşılanır
                            // (ayarlanan tuş neyse o çalışır).
                            let sending = {
                                let state = app.state::<AppState>();
                                let mut shell =
                                    state.shell.lock().expect("shell kilidi");
                                match event.state {
                                    ShortcutState::Pressed => {
                                        shell.hotkey_down();
                                        mic_begin(app);
                                        false
                                    }
                                    ShortcutState::Released => {
                                        mic_feed(app, &mut shell);
                                        shell.hotkey_up();
                                        shell.in_flight()
                                    }
                                }
                            };
                            emit_overlay(app);
                            if sending {
                                spawn_transcribe(app.clone());
                                schedule_send_timeout(app.clone());
                            }
                        })
                        .build()
                    });
                match plugin {
                    Ok(p) => {
                        if let Err(e) = app.handle().plugin(p) {
                            eprintln!("tauri: kisayol eklentisi kurulamadi: {e}");
                            hotkey_warn(app.handle(), &saved);
                        }
                    }
                    _ => {
                        eprintln!("tauri: {saved} baska programda, global tus yok");
                        hotkey_warn(app.handle(), &saved);
                    }
                }
            }
            spawn_idle_updater(app.handle().clone());
            spawn_boot_updater(app.handle().clone());
            Ok(())
        })
        .on_window_event(|win, ev| {
            // Kapatma = tepsiye in (uygulama + F9 çalışmaya devam eder).
            if win.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = ev {
                    api.prevent_close();
                    let _ = win.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("tauri kabugu baslatilamadi");
}

/// Kabuk durumunu `overlay` penceresine iter + pencere görünürlüğünü senkronlar.
///
/// Görünürlük kuralı [`overlay::is_visible`]’dır: pencere YALNIZCA durum
/// `hidden` VE toast VE rozet yokken gizlenir (`hide` kesin). Açılış
/// `hidden` = normal (pencere `visible: false` gelir). `Done` 3sn sonra
/// kendiliğinden sönümlenir (Done->Hidden akışı hayalet bırakmaz).
pub fn emit_overlay(app: &AppHandle) {
    let view = {
        let state = app.state::<AppState>();
        let shell = state.shell.lock().expect("shell kilidi");
        overlay::render(shell.ui())
    };
    let visible = overlay::is_visible(&view);
    if view.state == "done" {
        schedule_done_dismiss(app.clone());
    }
    // Gecici bilgi (hidden + toast): 5sn sonra kendiliginden soner,
    // arayuz acik kalmaz (sessiz/giris toast'i hayaleti duzeltmesi).
    if view.state == "hidden" {
        if let Some(t) = &view.toast {
            schedule_transient_dismiss(app.clone(), t.text.clone());
        }
    }
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

/// Gecici toast sonumu: ayni metin duruyorsa ve kayit yoksa temizlenir.
/// Kosullu: arada yeni kayit basladiysa ya da toast degistiyse dokunulmaz.
fn schedule_transient_dismiss(app: AppHandle, text: String) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(5));
        let same = {
            let state = app.state::<AppState>();
            let shell = state.shell.lock().expect("shell kilidi");
            !shell.recording()
                && !shell.in_flight()
                && shell.ui().toast.as_ref().map(|t| t.text == text).unwrap_or(false)
        };
        if same {
            let state = app.state::<AppState>();
            state
                .shell
                .lock()
                .expect("shell kilidi")
                .dismiss();
            emit_overlay(&app);
        }
    });
}

/// `Done` rozeti kısa süre gösterilir, sonra `Hidden`'a düşer.
/// Koşullu sönüm: arada yeni kayıt başladıysa dokunulmaz.
fn schedule_done_dismiss(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        let still_done = {
            let state = app.state::<AppState>();
            let shell = state.shell.lock().expect("shell kilidi");
            matches!(shell.ui().overlay, client_ui::Overlay::Done)
        };
        if still_done {
            let state = app.state::<AppState>();
            state.shell.lock().expect("shell kilidi").dismiss();
            emit_overlay(&app);
        }
    });
}

/// Kısayol alınamadı uyarısı (ayarlanan tuş adıyla; geçici toast,
/// kapanmadan devam edilir; 8sn sonra `emit_overlay` ile gizlenir).
fn hotkey_warn(app: &AppHandle, key: &str) {
    {
        let state = app.state::<AppState>();
        state
            .shell
            .lock()
            .expect("shell kilidi")
            .hotkey_unavailable(key);
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

/// Kısayol basıldı: secili/varsayılan mikrofondan yakalamayi baslat.
/// Donanim yoksa sessizce gecilir (birakma sessiz-toast yoluna duser).
pub(crate) fn mic_begin(app: &AppHandle) {
    let device = app
        .try_state::<AppState>()
        .and_then(|s| s.mic.lock().expect("mikrofon kilidi").clone());
    if let Err(e) = crate::mic_cap::capture_start(device.as_deref()) {
        eprintln!("mikrofon baslatilamadi ({e}), sessiz yol");
    }
}

/// Kısayol bırakıldı: biriken sesi 1sn'lik 16kHz dilimler halinde kabuga
/// besler (o anki `push_second` akisi; bos ise sessiz sayilir).
pub(crate) fn mic_feed(app: &AppHandle, shell: &mut Shell) {
    let pcm = crate::mic_cap::capture_stop();
    if let Some(state) = app.try_state::<AppState>() {
        *state.last_pcm.lock().expect("ses kilidi") = pcm.clone();
    }
    for chunk in pcm.chunks(16_000) {
        if !shell.recording() {
            break;
        }
        shell.push_second(chunk);
    }
}

/// Birakma sonrasi yerel transkripsiyon: `last_pcm` WAV yapilip
/// `wl --serve`'e gonderilir; metin overlay'e duser, hata `send_timeout`
/// yoluna duser (hat asili kalmaz). Ayri izlekte calisir (kilit tutulmaz).
pub(crate) fn spawn_transcribe(app: AppHandle) {
    std::thread::spawn(move || {
        let pcm = app
            .try_state::<AppState>()
            .map(|s| s.last_pcm.lock().expect("ses kilidi").clone())
            .unwrap_or_default();
        if pcm.is_empty() {
            return; // Sessiz yol zaten toast'i dustu.
        }
        let wav = crate::transcribe::wav_bytes_16k_mono(&pcm);
        let boundary = "whisperexe";
        let body = crate::transcribe::multipart_body(boundary, &wav, crate::transcribe::TRANSCRIBE_MODEL);
        let out = crate::net::download_agent()
            .post(crate::transcribe::LOCAL_TRANSCRIBE_URL)
            .header(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send(&body[..])
            .ok()
            .and_then(|res| res.into_body().read_to_string().ok())
            .and_then(|t| crate::transcribe::parse_text(&t));
        let state = app.state::<AppState>();
        let mut shell = state.shell.lock().expect("shell kilidi");
        match out {
            Some(text) => shell.transcript_arrived(&text),
            None => shell.send_timeout(),
        }
        drop(shell);
        emit_overlay(&app);
    });
}

/// Gonderim gozetimi (yedek): transkripsiyon izlegi her durumda terminal
/// duruma getirir; bu gozetim 150sn'yi asan asili hati temizler.
pub(crate) fn schedule_send_timeout(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(150));
        let due = {
            let state = app.state::<AppState>();
            let shell = state.shell.lock().expect("shell kilidi");
            shell.in_flight()
        };
        if due {
            let state = app.state::<AppState>();
            state.shell.lock().expect("shell kilidi").send_timeout();
            emit_overlay(&app);
        }
    });
}

/// Sistem tepsisi: Göster / Çık. Simge gömülüdür (derleme-dışı dosyaya bakmaz).
fn setup_tray(app: &AppHandle) {
    let icon = match tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png")) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("tauri: tepsi simgesi okunamadi: {e}");
            return;
        }
    };
    let show = match MenuItem::with_id(app, "goster", "Göster", true, None::<&str>) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("tauri: tepsi menusu kurulamadi: {e}");
            return;
        }
    };
    let quit = match MenuItem::with_id(app, "cik", "Çık", true, None::<&str>) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("tauri: tepsi menusu kurulamadi: {e}");
            return;
        }
    };
    let menu = match Menu::with_items(app, &[&show, &quit]) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("tauri: tepsi menusu kurulamadi: {e}");
            return;
        }
    };
    if let Err(e) = TrayIconBuilder::new()
        .icon(icon)
        .menu(&menu)
        .tooltip("whisperexe")
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "goster" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                } else {
                    eprintln!("tauri: 'main' penceresi kayitli degil");
                }
            }
            "cik" => app.exit(0),
            _ => {}
        })
        .build(app)
    {
        eprintln!("tauri: tepsi kurulamadi: {e}");
    }
}

/// Kalici oturumu (varsa) bellege yukler. Dosya yoksa/bozuksa sessizce
/// gecilir (kullanici giris formunu gorur; donma YOK, sir sizmaz).
fn restore_session(app: &AppHandle) {
    let path = crate::commands::session_path(app);
    if let Some(state) = app.try_state::<Mutex<crate::session::UserSession>>() {
        let ok = state
            .lock()
            .map(|mut s| s.load_from_file(&path))
            .unwrap_or(false);
        if !ok {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Açılış kapısı: broker bildirimi → `Shell::boot`. Ağ işi ayrı iş parçacığında
/// yapılır (kurulum engellenmez); zaman aşımı/bozuk yanıt = fail-open + toast.
fn boot_from_broker(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        let url = version::endpoint_url(BROKER_BASE);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            // KRITIK: dogrudan `ureq::get` DEGIL `net::agent` kullanilir.
            // `net::agent` OS sertifika deposuna guvenir (platform-verifier);
            // dogrudan cagri ozel CA / IP sertifikasinda basarisiz olup her
            // acilista "Surum denetlenemedi" toast'i birakir (goruntudeki hata).
            let body = crate::net::agent()
                .get(&url)
                .call()
                .ok()
                .and_then(|res| res.into_body().read_to_string().ok());
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

/// İşaret dizini: oturum dosyasıyla aynı app-data dizini (yoksa temp).
fn applied_dir(app: &AppHandle) -> std::path::PathBuf {
    use tauri::Manager;
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("whisperexe"))
}

/// Her girişte bir kez: besleme sessizce sorulur (Tauri updater `check()`).
/// Ağ işi ayrı iş parçacığındadır (kurulum engellenmez); 15sn tavan aşılırsa
/// ya da yanıt olumsuzsa sessizce vazgeçilir (toast YOK).
/// Yenilik varsa ve kayıtlı işaretten yeniyse bayrak kurulur + overlay
/// bilgilendirilir (`UpdatePending`; kayıt ortasında çalınmaz). İşaret yoksa
/// o anki besleme benimsenir (temiz kurulumda aynı kurulum yeniden kurulmaz).
/// Gerçek indirme+kurulum YALNIZCA boşta olur ([`spawn_idle_updater`]).
fn spawn_boot_updater(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(BOOT_UPDATE_DELAY);
        let (tx, rx) = mpsc::channel();
        let handle = app.clone();
        std::thread::spawn(move || {
            let _ = tx.send(check_update_blocking(&handle));
        });
        let found = rx.recv_timeout(UPDATE_TIMEOUT).ok().flatten();
        let Some(feed_ver) = found else { return };
        let dir = applied_dir(&app);
        match crate::update::read_applied_version(&dir) {
            None => crate::update::write_applied_version(&dir, &feed_ver),
            Some(applied) => {
                if crate::update::feed_is_newer(&feed_ver, &applied) {
                    let state = app.state::<AppState>();
                    state
                        .shell
                        .lock()
                        .expect("shell kilidi")
                        .note_update_available();
                    emit_overlay(&app);
                }
            }
        }
    });
}

/// Engellemesiz köprü: updater `check()` async'tir; sonuç sürüm metnidir.
/// `None` = güncel/ulaşılamaz/bozuk yanıt (hepsi sessiz geçilir).
#[cfg(desktop)]
fn check_update_blocking(app: &AppHandle) -> Option<String> {
    tauri::async_runtime::block_on(check_for_update(app))
}

#[cfg(not(desktop))]
fn check_update_blocking(_app: &AppHandle) -> Option<String> {
    None
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
            if let Some(ver) = applied {
                crate::update::write_applied_version(&applied_dir(&app), &ver);
                let state = app.state::<AppState>();
                state.shell.lock().expect("shell kilidi").restart_applied();
                app.restart();
            }
        }
    });
}

/// Tek deneme: beslemede yenilik varsa sürüm metnini döner, yoksa/bozuksa
/// `None` (sessiz geçilir; özel indirici YOK, imza updater'ındır).
#[cfg(desktop)]
async fn check_for_update(app: &AppHandle) -> Option<String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = match app.updater() {
        Ok(u) => u,
        Err(_) => return None,
    };
    match updater.check().await {
        Ok(Some(u)) => Some(u.version),
        _ => None,
    }
}

/// Tek deneme: beslemede yenilik varsa indir + kur, kurulan sürümü döner.
/// Özel indirici YOK.
#[cfg(desktop)]
async fn try_install_update(app: &AppHandle) -> Option<String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = match app.updater() {
        Ok(u) => u,
        Err(_) => return None,
    };
    let update = match updater.check().await {
        Ok(Some(u)) => u,
        _ => return None,
    };
    let version = update.version.clone();
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .ok()?;
    Some(version)
}
