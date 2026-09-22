//! F9 kısayolu soyutlaması. Tauri derlemesinde global-shortcut
//! ile bağlanır; testlerde ve donanımsız ortamda `MockHotkey` kullanılır.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Pressed,
    Released,
}

pub trait HotkeySource {
    fn next_event(&mut self) -> Option<HotkeyEvent>;
}

/// Önceden yazılmış olay dizisini oynatan sahte kaynak.
#[derive(Debug, Default)]
pub struct MockHotkey {
    events: Vec<HotkeyEvent>,
    pos: usize,
}

impl MockHotkey {
    pub fn new(events: Vec<HotkeyEvent>) -> Self {
        Self { events, pos: 0 }
    }

    /// Bas-konuş: basıldı, `hold_secs` saniye tutuldu, bırakıldı.
    pub fn press_hold(hold_secs: u32) -> Self {
        let mut events = vec![HotkeyEvent::Pressed];
        for _ in 0..hold_secs {
            // Tutma arası olay yok; kayıt döngüsü saniye başına beslenir.
        }
        events.push(HotkeyEvent::Released);
        Self::new(events)
    }
}

impl HotkeySource for MockHotkey {
    fn next_event(&mut self) -> Option<HotkeyEvent> {
        let e = self.events.get(self.pos).copied();
        if e.is_some() {
            self.pos += 1;
        }
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_replays_press_release() {
        let mut h = MockHotkey::press_hold(3);
        assert_eq!(h.next_event(), Some(HotkeyEvent::Pressed));
        assert_eq!(h.next_event(), Some(HotkeyEvent::Released));
        assert_eq!(h.next_event(), None);
    }
}
