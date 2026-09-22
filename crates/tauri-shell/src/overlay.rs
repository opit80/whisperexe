//! Overlay görünüm modeli: `UiState` → WebView2'ye taşınabilir yük.
//!
//! Tauri tarafı `render(&UiState)` çıktısını `overlay` penceresine
//! `emit("overlay", view)` ile iter; `ui/overlay.html` bunu çizer.
//! Tek odak kuralı burada korunur: görünümde tek `state`, tek `secs`
//! sayacı (tabular-nums), isteğe bağlı tek rozet ve tek toast vardır.

/// WebView2'ye gönderilen seri overlay yükü.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct OverlayView {
    /// `hidden | recording | sending | queued | done |
    /// update-pending | update-blocked`
    pub state: &'static str,
    pub secs: u32,
    pub low_mic: bool,
    pub queue: Option<QueueView>,
    pub toast: Option<ToastView>,
    /// Düşük bakiye rozeti (o anki hat; ~10dk altı).
    pub low_balance: Option<BalanceBadge>,
    /// Nabız animasyonu mu, statik rozet mi (reduced-motion saygısı).
    pub pulse: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct QueueView {
    pub position: usize,
    pub pending: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ToastView {
    pub text: String,
    pub kind: &'static str,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct BalanceBadge {
    pub minutes_left: f64,
}

/// Pencere görünürlük kuralı (client-ui semantiği): rozet/toast `Hidden`
/// üstünde de taşınır; pencere YALNIZCA durum `hidden` VE toast VE rozet
/// yokken gizlenir. `show` SADECE bu kural `true` iken çağrılır, `hide`
/// SADECE `false` iken — arada hayalet pencere kalmaz.
pub fn is_visible(view: &OverlayView) -> bool {
    view.state != "hidden" || view.toast.is_some() || view.low_balance.is_some()
}

pub fn render(ui: &client_ui::UiState) -> OverlayView {
    use client_ui::Overlay;
    let (state, secs, low_mic, queue) = match &ui.overlay {
        Overlay::Hidden => ("hidden", 0, false, None),
        Overlay::Recording { secs, low_mic } => ("recording", *secs, *low_mic, None),
        Overlay::Sending => ("sending", 0, false, None),
        Overlay::Queued { position, pending } => (
            "queued",
            0,
            false,
            Some(QueueView {
                position: *position,
                pending: *pending,
            }),
        ),
        Overlay::Done => ("done", 0, false, None),
        Overlay::UpdatePending => ("update-pending", 0, false, None),
        Overlay::UpdateBlocked => ("update-blocked", 0, false, None),
    };
    OverlayView {
        state,
        secs,
        low_mic,
        queue,
        toast: ui.toast.as_ref().map(|t| ToastView {
            text: t.text.clone(),
            kind: match t.kind {
                client_ui::ToastKind::Error => "error",
                client_ui::ToastKind::Warn => "warn",
                client_ui::ToastKind::Info => "info",
            },
        }),
        low_balance: ui.low_balance.map(|b| BalanceBadge {
            minutes_left: b.minutes_left,
        }),
        pulse: ui.recording_pulse(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use client_ui::{Overlay, UiEvent, UiState};

    #[test]
    fn view_keeps_single_focus() {
        let mut s = UiState::new(false);
        assert_eq!(render(&s).state, "hidden");
        s.on(UiEvent::HotkeyDown);
        let v = render(&s);
        assert_eq!((v.state, v.pulse), ("recording", true));
        s.on(UiEvent::RecordStopped {
            secs: 3,
            reason: client::record::StopReason::Released,
        });
        assert_eq!(render(&s).state, "sending");
        s.on(UiEvent::Queued {
            position: 1,
            pending: 1,
        });
        let v = render(&s);
        assert_eq!(v.state, "queued");
        assert_eq!(v.queue.unwrap().position, 1);
    }

    #[test]
    fn reduced_motion_yields_static_badge() {
        let mut s = UiState::new(true);
        s.on(UiEvent::HotkeyDown);
        let v = render(&s);
        assert_eq!(v.state, "recording");
        assert!(!v.pulse);
    }

    #[test]
    fn blocked_and_pending_states_pass_through() {
        let mut s = UiState::new(false);
        s.on(UiEvent::UpdateBlocked);
        assert_eq!(render(&s).state, "update-blocked");
        let mut s = UiState::new(false);
        s.on(UiEvent::UpdatePending);
        assert_eq!(render(&s).state, "update-pending");
        let _ = Overlay::Hidden;
    }

    #[test]
    fn visibility_rule_leaves_no_ghost() {
        // hidden + toast yok + rozet yok = gizle (hayalet YOK).
        let mut s = UiState::new(false);
        let v = render(&s);
        assert_eq!(v.state, "hidden");
        assert!(!is_visible(&v));
        // Toast taşınırken pencere görünür (boş hap değil, bilgi var).
        s.on(UiEvent::Toast {
            text: "x".into(),
            kind: client_ui::ToastKind::Warn,
        });
        assert!(is_visible(&render(&s)));
        // Sessizlik: overlay hidden ama toast var -> görünür.
        let mut s = UiState::new(false);
        s.on(UiEvent::HotkeyDown);
        s.on(UiEvent::RecordStopped {
            secs: 0,
            reason: client::record::StopReason::Silent,
        });
        let v = render(&s);
        assert_eq!(v.state, "hidden");
        assert!(v.toast.is_some());
        assert!(is_visible(&v));
        // Dismiss sonrası her şey temiz -> gizle.
        s.on(UiEvent::Dismiss);
        let v = render(&s);
        assert!(!is_visible(&v));
        // Done görünür; Done->Hidden akışı gizler.
        let mut s = UiState::new(false);
        s.on(UiEvent::HotkeyDown);
        s.on(UiEvent::RecordStopped {
            secs: 2,
            reason: client::record::StopReason::Released,
        });
        s.on(UiEvent::ResultArrived);
        assert!(is_visible(&render(&s)));
        s.on(UiEvent::Dismiss);
        assert!(!is_visible(&render(&s)));
    }
}
