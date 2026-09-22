//! F9 → kayıt → overlay akışı.
//!
//! Tauri global-shortcut işleyicisi yalnızca şu iki çağrıyı yapar:
//! `hotkey_down()` (kısayol basıldı) ve `hotkey_up()` (kısayol bırakıldı);
//! aradaki her saniyelik PCM dilimi `push_second()` ile beslenir. Kısayol
//! tuşu ayarlardan değişir (varsayılan F9); akış tuştan bağımsızdır.
//! Gerçek mikrofon yoksa `MockEncoder` yolu kullanılır (client core'daki
//! mock kayıt yolu korunur; donanım geldiğinde `Encoder` arayüzüne bağlanır).
//!
//! Kilitli kurallar:
//! - Giriş kaydı ENGELLEMEZ: `hotkey_down` her zaman kayıt açar; giriş
//!   yoksa gönderim yapılmaz, `giris-gerekli` toast'ı düşer (ücret YOK).
//! - Sessizlik (konuşma öncesi): gönderme YOK, ücret YOK, ücretsiz toast.
//! - 180sn tavan: istemcide kesilir (`Capped180`), gövde ~2MB altında.
//! - Taban-altı engel: yeni kayıt AÇILMAZ; sürmekte olan iş bitirilir,
//!   sonra `UpdateBlocked` overlay'i gösterilir (dictation kesilmez).
//! - Zorunlu güncelleme: bayrak kurulur, overlay'e YALNIZCA boşta
//!   (`Hidden`/`Done`) geçilir; kayıt/sending/queued sırasında toast'a
//!   bile dokunulmaz (overlay çalınmaz).

use client::audio_enc::MockEncoder;
use client::balance::BalanceView;
use client::hotkey::{HotkeyEvent, HotkeySource};
use client::record::{RecordError, Session, StopReason};
use client_ui::{Overlay, UiEvent, UiState};

use crate::version::{check_at_startup, BootGate, VersionError};

/// Açılış kapısının kabuğa uygulanma özeti (telemetri/log için).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootApply {
    Allowed,
    PendingRestart { latest: String },
    BlockedBelowFloor { latest: String },
    /// Bildiri okunamadı: fail-open (kayıt engellenmez) + uyarı toast'ı.
    VersionUnknown,
}

/// F9 bas-konuş kabuğu: tek EXE'nin durum orkestratörü.
pub struct Shell {
    ui: UiState,
    session: Option<Session<MockEncoder>>,
    pushes: u32,
    in_flight: bool,
    pending_update: bool,
    blocked_update: bool,
    /// Giriş kaydı engellemez; yalnızca gönderim kapısıdır.
    /// Varsayılan `true` (eski akış + testsiz kurulum kayıtsız çalışır);
    /// Tauri tarafı oturuma göre [`Shell::set_logged_in`] ile besler.
    logged_in: bool,
}

impl Shell {
    /// `reduced_motion`: OS `prefers-reduced-motion` → Tauri'den gelir.
    pub fn new(reduced_motion: bool) -> Self {
        Self {
            ui: UiState::new(reduced_motion),
            session: None,
            pushes: 0,
            in_flight: false,
            pending_update: false,
            blocked_update: false,
            logged_in: true,
        }
    }

    /// Oturum kapısı (Tauri komutları giriş/çıkışta çağırır).
    pub fn set_logged_in(&mut self, v: bool) {
        self.logged_in = v;
    }

    pub fn logged_in(&self) -> bool {
        self.logged_in
    }

    pub fn ui(&self) -> &UiState {
        &self.ui
    }

    /// Kayıt sürüyor mu (F9 basılı)?
    pub fn recording(&self) -> bool {
        self.session.is_some()
    }

    /// Sonuç/upload hattında iş var mı?
    pub fn in_flight(&self) -> bool {
        self.in_flight
    }

    /// Yeniden başlatmada kurulacak zorunlu güncelleme bekliyor mu?
    pub fn update_due_at_restart(&self) -> bool {
        self.pending_update
    }

    // ---- açılış ----

    /// Açılışta broker `/v1/version` JSON'u + gömülü sürüm → kapı.
    /// Taban altıysa overlay `UpdateBlocked` olur; zorunlu yenilikse
    /// `UpdatePending` (boşta açılışta göstermek güvenlidir).
    pub fn boot(
        &mut self,
        current_version: &str,
        manifest_json: &str,
    ) -> Result<BootApply, VersionError> {
        match check_at_startup(current_version, manifest_json)? {
            BootGate::Allow => Ok(BootApply::Allowed),
            BootGate::PendingRestart { latest } => {
                self.pending_update = true;
                self.ui.on(UiEvent::UpdatePending);
                Ok(BootApply::PendingRestart { latest })
            }
            BootGate::BlockedBelowFloor { latest } => {
                self.blocked_update = true;
                self.ui.on(UiEvent::UpdateBlocked);
                Ok(BootApply::BlockedBelowFloor { latest })
            }
        }
    }

    /// Bildiri okunamaz/ulaşılamazsa: fail-open + uyarı toast'ı.
    pub fn boot_unknown_version(&mut self) -> BootApply {
        self.ui.on(UiEvent::Toast {
            text: "Surum denetlenemedi, cevrimici olunca tekrar denenecek.".into(),
            kind: client_ui::ToastKind::Warn,
        });
        BootApply::VersionUnknown
    }

    // ---- kısayol (Tauri global-shortcut burayı çağırır) ----

    /// Kısayol basıldı. Taban altıysa yeni kayıt AÇILMAZ (`false`).
    /// Giriş kaydı ASLA engellemez (kayıt açılır; kapı gönderimdedir).
    pub fn hotkey_down(&mut self) -> bool {
        if self.blocked_update {
            // Girişte engel: kayıt açma, güncelleme toast'ı overlay'i çalmaz.
            self.ui.on(UiEvent::Toast {
                text: "Devam etmek icin guncelleme gerekli.".into(),
                kind: client_ui::ToastKind::Warn,
            });
            return false;
        }
        if self.session.is_some() {
            return true; // Zaten basılı (tekrar basımı yoksay).
        }
        self.session = Some(Session::new(MockEncoder::default()));
        self.pushes = 0;
        self.ui.on(UiEvent::HotkeyDown);
        true
    }

    /// Kısayol bırakıldı: oturumu kapat, gönderim kararı ver.
    /// Giriş yoksa gönderim YOK: kayıt çöpe atılır, `giris-gerekli`
    /// toast'ı düşer (ücret YOK, hat meşgul EDİLMEZ).
    pub fn hotkey_up(&mut self) {
        let Some(s) = self.session.take() else { return };
        if self.pushes == 0 {
            // Hiç ses beslenmeden bırakıldı: sessiz say, gönderme.
            self.ui.on(UiEvent::RecordStopped {
                secs: 0,
                reason: StopReason::Silent,
            });
            self.in_flight = false;
            return;
        }
        if !self.logged_in {
            let _ = s.finish(true);
            self.ui.on(UiEvent::RecordStopped {
                secs: 0,
                reason: StopReason::Silent,
            });
            self.ui.on(UiEvent::Toast {
                text: "giris-gerekli: once giris yap, kaydin gonderilmedi.".into(),
                kind: client_ui::ToastKind::Warn,
            });
            self.in_flight = false;
            return;
        }
        let (_enc, secs, reason) = s.finish(true);
        self.ui.on(UiEvent::RecordStopped { secs, reason });
        self.in_flight = true;
    }

    /// 1 saniyelik 16kHz mono PCM dilimi besle (mikrofon ya da test tonu).
    pub fn push_second(&mut self, pcm_16k: &[i16]) {
        let Some(s) = self.session.as_mut() else { return };
        match s.push_second(pcm_16k) {
            Ok(continues) => {
                self.pushes += 1;
                self.ui.on(UiEvent::RecordTick {
                    secs: s.secs(),
                    low_mic: s.low_warned(),
                });
                if !continues {
                    // 180sn tavanında istemcide kesildi: otomatik kapat.
                    let s = self.session.take().expect("oturum az once vardi");
                    let (_enc, secs, reason) = s.finish(true);
                    debug_assert_eq!(reason, StopReason::Capped180);
                    self.ui.on(UiEvent::RecordStopped { secs, reason });
                    self.in_flight = true;
                }
            }
            Err(RecordError::Silent) => {
                // Konuşma öncesi sessizlik: gönderme, ücret yok.
                self.session = None;
                self.ui.on(UiEvent::RecordStopped {
                    secs: 0,
                    reason: StopReason::Silent,
                });
                self.in_flight = false;
            }
        }
    }

    /// `client::hotkey` kaynağını kabuğa bağlar (test/Mock ya da
    /// Tauri global-shortcut köprüsü aynı arayüzü konuşur).
    pub fn pump(&mut self, src: &mut impl HotkeySource) {
        while let Some(ev) = src.next_event() {
            match ev {
                HotkeyEvent::Pressed => {
                    self.hotkey_down();
                }
                HotkeyEvent::Released => self.hotkey_up(),
            }
        }
    }

    // ---- broker olayları ----

    /// Kuyruk konumu bildirimi (sıra görünümü + vazgeçme client core'dadır).
    pub fn queue_update(&mut self, position: usize, pending: usize) {
        self.ui.on(UiEvent::Queued { position, pending });
    }

    /// Sonuç geldi: overlay `Done`; bekleyen engel/zorunlu güncelleme
    /// YALNIZCA iş bittikten sonra gösterilir.
    pub fn result_arrived(&mut self) {
        self.in_flight = false;
        self.ui.on(UiEvent::ResultArrived);
        if self.blocked_update {
            self.ui.on(UiEvent::UpdateBlocked);
        } else if self.pending_update {
            self.ui.on(UiEvent::UpdatePending);
        }
    }

    /// Düşük bakiye rozeti (o anki hat tarifesiyle; ~10dk altı).
    pub fn balance(&mut self, view: BalanceView) {
        self.ui.on(UiEvent::Balance(view));
    }

    /// Toast süresi doldu / kullanıcı kapattı.
    pub fn dismiss(&mut self) {
        self.ui.on(UiEvent::Dismiss);
    }

    /// Kısayol başka programda (kapanmadan devam: pencere çalışır,
    /// global tuş yok). Metin ayarlanan tuşa göre dinamiktir.
    pub fn hotkey_unavailable(&mut self, key: &str) {
        let key = crate::hotkey::canonical(key).unwrap_or_else(|| crate::hotkey::DEFAULT_HOTKEY.to_string());
        self.ui.on(UiEvent::Toast {
            text: format!("{key} baska programda; once onu kapat."),
            kind: client_ui::ToastKind::Warn,
        });
    }

    /// Yeniden başlatmada zorunlu kurulum uygulandı (bayrak temizlenir).
    pub fn restart_applied(&mut self) {
        self.pending_update = false;
        if matches!(self.ui.overlay, Overlay::UpdatePending) {
            self.ui.on(UiEvent::Dismiss);
        }
    }

    /// Gonderim hatti yanit vermezse (yukleme iscisi yoksa): hat mesgul
    /// birakilmaz, overlay gizlenir + hata toast'i. Gecici toast 5sn'de
    /// soner (Tauri `emit_overlay` sonumu).
    pub fn send_timeout(&mut self) {
        if !self.in_flight {
            return;
        }
        self.in_flight = false;
        self.ui.overlay = Overlay::Hidden;
        self.ui.on(UiEvent::Toast {
            text: "Hata: gonderilemedi (hat-yok), kaydın çöpe atıldı.".into(),
            kind: client_ui::ToastKind::Error,
        });
    }

    /// Açılış besleme denetimi yenilik buldu: bayrağı kur, overlay'i bilgilendir.
    /// Engel (`UpdateBlocked`) üstüne yazılmaz; kayıt ortasında overlay çalınmaz
    /// (client-ui kuralı: o sırada bilgi toast'ı düşer, iş bitince bayrak görünür).
    pub fn note_update_available(&mut self) {
        self.pending_update = true;
        if !self.blocked_update {
            self.ui.on(UiEvent::UpdatePending);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use client::balance::{Line, Tariffs};
    use std::f32::consts::PI;

    fn tone_1s() -> Vec<i16> {
        (0..16_000)
            .map(|i| ((i as f32 * 440.0 * 2.0 * PI / 16000.0).sin() * 0.5 * 32767.0) as i16)
            .collect()
    }

    fn allow_manifest() -> String {
        serde_json::to_string(&client::updater::Manifest {
            version: "1.0.0".into(),
            min_supported: "1.0.0".into(),
            url: String::new(),
            sha256_hex: String::new(),
            signature_hex: String::new(),
            mandatory: false,
            notes: String::new(),
        })
        .unwrap()
    }

    fn tariffs() -> Tariffs {
        Tariffs {
            home_per_min: 1.0,
            fallback_per_min: 5.0,
        }
    }

    #[test]
    fn overlay_press_to_result_flow() {
        // Gerekli akış: press → kayıt → bırakma → sending → queued → done.
        let mut sh = Shell::new(false);
        assert_eq!(
            sh.boot("1.0.0", &allow_manifest()),
            Ok(BootApply::Allowed)
        );
        assert!(sh.hotkey_down());
        assert!(matches!(sh.ui.overlay, Overlay::Recording { .. }));
        let t = tone_1s();
        sh.push_second(&t);
        sh.push_second(&t);
        sh.push_second(&t);
        assert!(matches!(
            sh.ui.overlay,
            Overlay::Recording { secs: 3, .. }
        ));
        sh.hotkey_up();
        assert_eq!(sh.ui.overlay, Overlay::Sending);
        assert!(sh.in_flight());
        sh.queue_update(2, 2);
        assert_eq!(sh.ui.overlay, Overlay::Queued {
            position: 2,
            pending: 2
        });
        sh.result_arrived();
        assert_eq!(sh.ui.overlay, Overlay::Done);
        assert!(!sh.in_flight());
        sh.dismiss();
        assert_eq!(sh.ui.overlay, Overlay::Hidden);
        // Bakiye rozeti o anki hat tarifesiyle akar.
        sh.balance(client::balance::evaluate(2.0, Line::Fallback, tariffs()));
        assert!(sh.ui.low_balance.unwrap().low);
    }

    #[test]
    fn silence_is_never_sent_and_free_toast_shows() {
        // Sessizlikte göndermeme: overlay gizlenir, ücretsiz toast çıkar.
        let mut sh = Shell::new(false);
        sh.hotkey_down();
        sh.push_second(&vec![0i16; 16_000]);
        assert!(!sh.recording());
        assert!(!sh.in_flight());
        assert_eq!(sh.ui.overlay, Overlay::Hidden);
        let t = sh.ui.toast.clone().expect("sessizlik toast'i");
        assert_eq!(t.kind, client_ui::ToastKind::Warn);
        assert!(t.text.contains("ucret yok"));
    }

    #[test]
    fn below_floor_blocks_new_press_but_finishes_job() {
        // Taban-altı-bloke: yeni kayıt açılmaz; sürmekte olan iş bitirilir.
        let mut sh = Shell::new(false);
        sh.boot("1.0.0", &allow_manifest()).unwrap();
        assert!(sh.hotkey_down());
        let t = tone_1s();
        sh.push_second(&t);
        // Açılışta değil, kayıt ortasında taban bildirimi gelse bile
        // overlay çalınmaz (client-ui kuralı); kabuk bayrağı kurar.
        sh.blocked_update = true;
        sh.ui.on(UiEvent::UpdateBlocked);
        assert!(matches!(sh.ui.overlay, Overlay::Recording { .. }));
        // Kayıt bitip sonuç gelince engel overlay'i gösterilir.
        sh.hotkey_up();
        assert_eq!(sh.ui.overlay, Overlay::Sending);
        sh.result_arrived();
        assert_eq!(sh.ui.overlay, Overlay::UpdateBlocked);
        // Engel varken yeni F9 açılmaz.
        assert!(!sh.hotkey_down());
        assert_eq!(sh.ui.overlay, Overlay::UpdateBlocked);
    }

    #[test]
    fn boot_below_floor_blocks_at_entry() {
        let mut sh = Shell::new(false);
        let manifest = serde_json::to_string(&client::updater::Manifest {
            version: "1.2.0".into(),
            min_supported: "1.1.0".into(),
            url: String::new(),
            sha256_hex: String::new(),
            signature_hex: String::new(),
            mandatory: true,
            notes: "kritik".into(),
        })
        .unwrap();
        assert_eq!(
            sh.boot("1.0.5", &manifest),
            Ok(BootApply::BlockedBelowFloor {
                latest: "1.2.0".into()
            })
        );
        assert_eq!(sh.ui.overlay, Overlay::UpdateBlocked);
        assert!(!sh.hotkey_down());
    }

    #[test]
    fn mandatory_update_never_steals_recording() {
        // Zorunlu güncelleme dictation ortasında uygulanmaz.
        let mut sh = Shell::new(false);
        let manifest = serde_json::to_string(&client::updater::Manifest {
            version: "1.2.0".into(),
            min_supported: "1.0.0".into(),
            url: String::new(),
            sha256_hex: String::new(),
            signature_hex: String::new(),
            mandatory: true,
            notes: "kritik".into(),
        })
        .unwrap();
        assert_eq!(
            sh.boot("1.0.0", &manifest),
            Ok(BootApply::PendingRestart {
                latest: "1.2.0".into()
            })
        );
        assert!(sh.update_due_at_restart());
        assert!(sh.hotkey_down());
        // Kayıt sırasında overlay kayıt odağını korur.
        let t = tone_1s();
        sh.push_second(&t);
        assert!(matches!(sh.ui.overlay, Overlay::Recording { .. }));
        sh.hotkey_up();
        sh.result_arrived();
        // İş bitince bekleyen güncelleme görünür; yeniden başlatmada kurulur.
        assert_eq!(sh.ui.overlay, Overlay::UpdatePending);
        sh.restart_applied();
        assert!(!sh.update_due_at_restart());
    }

    #[test]
    fn boot_note_marks_pending_without_stealing() {
        // Açılış besleme denetimi yenilik buldu: bayrak kurulur, boşta
        // overlay bilgi verir; engel üstüne yazılmaz.
        let mut sh = Shell::new(false);
        sh.note_update_available();
        assert!(sh.update_due_at_restart());
        assert_eq!(sh.ui.overlay, Overlay::UpdatePending);
        // Engel varken bayrak kurulur ama engel overlay'i korunur.
        let mut blocked = Shell::new(false);
        let manifest = serde_json::to_string(&client::updater::Manifest {
            version: "1.2.0".into(),
            min_supported: "1.1.0".into(),
            url: String::new(),
            sha256_hex: String::new(),
            signature_hex: String::new(),
            mandatory: true,
            notes: "kritik".into(),
        })
        .unwrap();
        blocked.boot("1.0.5", &manifest).unwrap();
        blocked.note_update_available();
        assert!(blocked.update_due_at_restart());
        assert_eq!(blocked.ui.overlay, Overlay::UpdateBlocked);
    }

    #[test]
    fn caps_at_180s_and_stays_under_body_limit() {
        let mut sh = Shell::new(false);
        sh.hotkey_down();
        let t = tone_1s();
        for _ in 0..200 {
            if !sh.recording() {
                break;
            }
            sh.push_second(&t);
        }
        // 180sn tavanında kesildi, gönderiliyor.
        assert!(!sh.recording());
        assert_eq!(sh.ui.overlay, Overlay::Sending);
        assert!(sh.in_flight());
        let toast = sh.ui.toast.clone().expect("tavan toast'i");
        assert!(toast.text.contains("180"));
        // Gövde tavanı (~2MB) client core'da sabit; tam kayıt altında kalır.
        assert!(client::audio_enc::MAX_BODY_BYTES >= 2 * 1024 * 1024 - 1);
        assert_eq!(client::audio_enc::MAX_SECS, 180);
    }

    #[test]
    fn send_timeout_clears_stuck_sending() {
        // Yukleme iscisi bagli degilse hat asili kalmaz: hata toast'i + gizli.
        let mut sh = Shell::new(false);
        sh.set_logged_in(true);
        assert!(sh.hotkey_down());
        sh.push_second(&tone_1s());
        sh.hotkey_up();
        assert!(sh.in_flight());
        sh.send_timeout();
        assert!(!sh.in_flight());
        assert_eq!(sh.ui.overlay, Overlay::Hidden);
        let toast = sh.ui.toast.clone().expect("hata toast'i");
        assert!(toast.text.contains("gonderilemedi"));
        // Bosta cagri etkisizdir.
        sh.send_timeout();
    }

    #[test]
    fn hotkey_records_without_login_but_send_needs_login() {
        // Arkadaş makinesi: giriş yokken kısayol kayda izinlidir;
        // bırakınca gönderim YOK, `giris-gerekli` toast'ı düşer.
        let mut sh = Shell::new(false);
        sh.set_logged_in(false);
        assert!(!sh.logged_in());
        assert!(sh.hotkey_down());
        assert!(matches!(sh.ui.overlay, Overlay::Recording { .. }));
        let t = tone_1s();
        sh.push_second(&t);
        sh.hotkey_up();
        assert!(!sh.recording());
        assert!(!sh.in_flight());
        assert_eq!(sh.ui.overlay, Overlay::Hidden);
        let toast = sh.ui.toast.clone().expect("giris toast'i");
        assert!(toast.text.contains("giris-gerekli"));
        // Giriş gelince aynı akış gönderir.
        sh.set_logged_in(true);
        assert!(sh.hotkey_down());
        sh.push_second(&tone_1s());
        sh.hotkey_up();
        assert_eq!(sh.ui.overlay, Overlay::Sending);
        assert!(sh.in_flight());
    }

    #[test]
    fn blocked_update_explains_itself() {
        let mut sh = Shell::new(false);
        sh.blocked_update = true;
        assert!(!sh.hotkey_down());
        let toast = sh.ui.toast.clone().expect("engel toast'i");
        assert!(toast.text.contains("guncelleme"));
    }

    #[test]
    fn hotkey_unavailable_names_configured_key() {
        let mut sh = Shell::new(false);
        sh.hotkey_unavailable("F11");
        let toast = sh.ui.toast.clone().expect("kisayol toast'i");
        assert!(toast.text.contains("F11"));
        assert!(!toast.text.contains("F9"));
    }
}
