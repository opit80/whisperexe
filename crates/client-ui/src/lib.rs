//! Durum overlay'i durum makinesi.
//!
//! Intent (interface-design): dikte eden insan F9'a basılı tutarken gözü
//! klavyededir; overlay çevresel görüşe oynar: tek odak (durum noktası),
//! tek vurgu rengi (#7aa2ff), sayı tabular-nums, hareket 200ms altı.
//! `prefers-reduced-motion` saygısı: nabız animasyonu yerine statik rozet.
//!
//! Tauri bağlaması bu makineyi sürer; WebView2 gereksinimi README'dedir.

use client::balance::BalanceView;
use client::record::StopReason;

/// Overlay'in gösterebileceği tek odak durum.
#[derive(Debug, Clone, PartialEq)]
pub enum Overlay {
    Hidden,
    /// F9 basılı: kayıt sürüyor (sn + düşük-seviye uyarısı).
    Recording { secs: u32, low_mic: bool },
    /// Gönderiliyor.
    Sending,
    /// Kuyrukta: konum / bekleyen.
    Queued { position: usize, pending: usize },
    /// Sonuç geldi (kısa süre gösterilir).
    Done,
    /// Zorunlu güncelleme: yeniden başlatmada kurulur (dictation kesilmez).
    UpdatePending,
    /// Taban altı: girişte güncelleme ister.
    UpdateBlocked,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Error,
    Warn,
    Info,
}

#[derive(Debug, Clone)]
pub enum UiEvent {
    HotkeyDown,
    RecordTick { secs: u32, low_mic: bool },
    RecordStopped { secs: u32, reason: StopReason },
    SendStarted,
    Queued { position: usize, pending: usize },
    ResultArrived,
    Balance(BalanceView),
    Toast { text: String, kind: ToastKind },
    ToastExpired,
    UpdatePending,
    UpdateBlocked,
    Dismiss,
}

#[derive(Debug, Clone)]
pub struct UiState {
    pub overlay: Overlay,
    pub toast: Option<Toast>,
    /// Düşük bakiye overlay rozeti (o anki hat tarifesiyle).
    pub low_balance: Option<BalanceView>,
    /// Hareket azaltma isteği (OS `prefers-reduced-motion` → Tauri'den gelir).
    pub reduced_motion: bool,
}

impl UiState {
    pub fn new(reduced_motion: bool) -> Self {
        Self {
            overlay: Overlay::Hidden,
            toast: None,
            low_balance: None,
            reduced_motion,
        }
    }

    /// Kayıt göstergesi nabız mı (animasyonlu) yoksa statik rozet mi?
    pub fn recording_pulse(&self) -> bool {
        matches!(self.overlay, Overlay::Recording { .. }) && !self.reduced_motion
    }

    pub fn on(&mut self, ev: UiEvent) {
        match ev {
            UiEvent::HotkeyDown => {
                self.overlay = Overlay::Recording {
                    secs: 0,
                    low_mic: false,
                };
                self.toast = None;
            }
            UiEvent::RecordTick { secs, low_mic } => {
                if matches!(self.overlay, Overlay::Recording { .. }) {
                    self.overlay = Overlay::Recording { secs, low_mic };
                }
            }
            UiEvent::RecordStopped { reason, .. } => {
                match reason {
                    StopReason::Silent => {
                        self.overlay = Overlay::Hidden;
                        self.toast = Some(Toast {
                            text: "Ses duyulmadi, gonderilmedi (ucret yok).".into(),
                            kind: ToastKind::Warn,
                        });
                    }
                    StopReason::Capped180 => {
                        self.overlay = Overlay::Sending;
                        self.toast = Some(Toast {
                            text: "180 sn tavaninda kesildi, gonderiliyor.".into(),
                            kind: ToastKind::Info,
                        });
                    }
                    StopReason::Released => self.overlay = Overlay::Sending,
                }
            }
            UiEvent::SendStarted => self.overlay = Overlay::Sending,
            UiEvent::Queued { position, pending } => {
                self.overlay = Overlay::Queued { position, pending };
            }
            UiEvent::ResultArrived => {
                self.overlay = Overlay::Done;
                self.toast = None;
            }
            UiEvent::Balance(v) => {
                self.low_balance = v.low.then_some(v);
            }
            UiEvent::Toast { text, kind } => {
                self.toast = Some(Toast { text, kind });
            }
            UiEvent::ToastExpired | UiEvent::Dismiss => {
                if matches!(self.overlay, Overlay::Done) {
                    self.overlay = Overlay::Hidden;
                }
                self.toast = None;
            }
            UiEvent::UpdatePending => {
                // Dictation ortasında overlay çalınmaz: kayıt bitince gösterilir.
                if matches!(self.overlay, Overlay::Recording { .. }) {
                    self.toast = Some(Toast {
                        text: "Yeniden baslattiginda guncellenecek.".into(),
                        kind: ToastKind::Info,
                    });
                } else {
                    self.overlay = Overlay::UpdatePending;
                }
            }
            UiEvent::UpdateBlocked => {
                if matches!(self.overlay, Overlay::Recording { .. }) {
                    self.toast = Some(Toast {
                        text: "Kayit bitince guncelleme gerekli.".into(),
                        kind: ToastKind::Warn,
                    });
                } else {
                    self.overlay = Overlay::UpdateBlocked;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use client::balance::{Line, Tariffs};

    fn low_view() -> BalanceView {
        client::balance::evaluate(
            2.0,
            Line::Fallback,
            Tariffs {
                home_per_min: 1.0,
                fallback_per_min: 5.0,
            },
        )
    }

    #[test]
    fn press_to_result_flow() {
        let mut s = UiState::new(false);
        s.on(UiEvent::HotkeyDown);
        assert!(matches!(s.overlay, Overlay::Recording { .. }));
        assert!(s.recording_pulse());
        s.on(UiEvent::RecordTick {
            secs: 3,
            low_mic: false,
        });
        s.on(UiEvent::RecordStopped {
            secs: 3,
            reason: StopReason::Released,
        });
        assert_eq!(s.overlay, Overlay::Sending);
        s.on(UiEvent::Queued {
            position: 2,
            pending: 2,
        });
        assert_eq!(
            s.overlay,
            Overlay::Queued {
                position: 2,
                pending: 2
            }
        );
        s.on(UiEvent::ResultArrived);
        assert_eq!(s.overlay, Overlay::Done);
        s.on(UiEvent::Dismiss);
        assert_eq!(s.overlay, Overlay::Hidden);
    }

    #[test]
    fn silent_record_shows_no_charge_toast() {
        let mut s = UiState::new(false);
        s.on(UiEvent::HotkeyDown);
        s.on(UiEvent::RecordStopped {
            secs: 0,
            reason: StopReason::Silent,
        });
        assert_eq!(s.overlay, Overlay::Hidden);
        let t = s.toast.unwrap();
        assert_eq!(t.kind, ToastKind::Warn);
        assert!(t.text.contains("ucret yok"));
    }

    #[test]
    fn reduced_motion_disables_pulse_but_keeps_state() {
        let mut s = UiState::new(true);
        s.on(UiEvent::HotkeyDown);
        assert!(!s.recording_pulse());
        assert!(matches!(s.overlay, Overlay::Recording { .. }));
    }

    #[test]
    fn low_balance_badge_uses_current_line() {
        let mut s = UiState::new(false);
        assert!(s.low_balance.is_none());
        s.on(UiEvent::Balance(low_view()));
        assert!(s.low_balance.unwrap().low);
    }

    #[test]
    fn mandatory_update_never_interrupts_recording_state() {
        // Zorunlu güncelleme kayıt ortasında overlay'i çalmaz: kayıt sürer,
        // bilgi toast'a düşer; kayıt bitince overlay gösterilir.
        let mut s = UiState::new(false);
        s.on(UiEvent::HotkeyDown);
        s.on(UiEvent::UpdateBlocked);
        assert!(matches!(s.overlay, Overlay::Recording { .. }));
        assert!(s.toast.is_some());
        s.on(UiEvent::RecordStopped {
            secs: 2,
            reason: StopReason::Released,
        });
        s.on(UiEvent::UpdateBlocked);
        assert_eq!(s.overlay, Overlay::UpdateBlocked);
    }
}
