//! Bas-konus global kisayol ayari (F9 varsayilan, ayarlardan degisir).
//!
//! Bicim eklenti dilidir (`+` ayracli, once degistiriciler, sonda tek ana
//! tus): `F9`, `Q`, `Ctrl+Shift+K`, `Alt+Q`, `Space`. Takma adlar:
//! `Ctrl/Control`, `Alt/Option`, `Shift`, `Super/Win/Cmd/Command`.
//! Gecersizde komut `"kisayol-gecersiz"` doner (on-yuz `Hata: ...` basar).
//! Kalicilik: app-data `hotkey.json` (`{"hotkey":"F9"}`); yoksa/bozuksa
//! sessizce varsayilana dusulur (acilis engellenmez).
//!
//! Not: tek basina degistirici (`Ctrl`) veya bos girdi gecersizdir; kayit
//! carpmasi calisma-aninda eklentiden doner (`kisayol-kayit-hatasi:...`).

/// Kalici kisayol dosyasi adi (app-data dizininde).
pub const HOTKEY_FILE_NAME: &str = "hotkey.json";

/// Kayit yoksa/bozuksa kullanilan tus.
pub const DEFAULT_HOTKEY: &str = "F9";

/// Gorunum sirasi sabit: Ctrl, Alt, Shift, Super.
const MOD_ORDER: &[&str] = &["Ctrl", "Alt", "Shift", "Super"];

/// Jetonu kanonik degistiriciye cevir (degistirici degilse `None`).
fn canon_mod(tok: &str) -> Option<&'static str> {
    match tok {
        "CTRL" | "CONTROL" => Some("Ctrl"),
        "ALT" | "OPTION" => Some("Alt"),
        "SHIFT" => Some("Shift"),
        "SUPER" | "WIN" | "WINDOWS" | "CMD" | "COMMAND" => Some("Super"),
        "COMMANDORCONTROL" | "COMMANDORCTRL" | "CMDORCTRL" | "CMDORCONTROL" => {
            Some("Ctrl")
        }
        _ => None,
    }
}

/// Jetonu kanonik ana tusa cevir (ana tus degilse `None`).
/// Eklentinin cozumledigi kume aynen kabul edilir (harf, rakam, F1-F12,
/// Space/Enter/oklar/semboller; PrintScreen dahil — kayit tutmazsa
/// calisma-aninda hata doner).
fn canon_key(tok: &str) -> Option<String> {
    if tok.is_empty() {
        return None;
    }
    // Tek karakter: harf/rakam/sembol dogrudan tus olur.
    if tok.chars().count() == 1 {
        let c = tok.chars().next().expect("bos degil");
        if c == '+' {
            return None;
        }
        return Some(c.to_ascii_uppercase().to_string());
    }
    // Adli tuslar (eklenti `parse_key` kumesinin buyuk-kucuk duyarsiz hali).
    const NAMED: &[&str] = &[
        "BACKQUOTE", "BACKSLASH", "BRACKETLEFT", "BRACKETRIGHT", "PAUSE", "PAUSEBREAK",
        "COMMA", "EQUAL", "MINUS", "PERIOD", "QUOTE", "SEMICOLON", "SLASH", "BACKSPACE",
        "CAPSLOCK", "ENTER", "SPACE", "TAB", "DELETE", "END", "HOME", "INSERT", "PAGEDOWN",
        "PAGEUP", "PRINTSCREEN", "SCROLLLOCK", "ARROWDOWN", "DOWN", "ARROWLEFT", "LEFT",
        "ARROWRIGHT", "RIGHT", "ARROWUP", "UP", "NUMLOCK", "NUMPAD0", "NUM0", "NUMPAD1",
        "NUM1", "NUMPAD2", "NUM2", "NUMPAD3", "NUM3", "NUMPAD4", "NUM4", "NUMPAD5", "NUM5",
        "NUMPAD6", "NUM6", "NUMPAD7", "NUM7", "NUMPAD8", "NUM8", "NUMPAD9", "NUM9",
        "NUMPADADD", "NUMADD", "NUMPADPLUS", "NUMPLUS", "NUMPADDECIMAL", "NUMDECIMAL",
        "NUMPADDIVIDE", "NUMDIVIDE", "NUMPADENTER", "NUMENTER", "NUMPADEQUAL", "NUMEQUAL",
        "NUMPADMULTIPLY", "NUMMULTIPLY", "NUMPADSUBTRACT", "NUMSUBTRACT", "ESCAPE", "ESC",
        "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13",
        "F14", "F15", "F16", "AUDIOVOLUMEDOWN", "VOLUMEDOWN", "AUDIOVOLUMEUP", "VOLUMEUP",
        "AUDIOVOLUMEMUTE", "VOLUMEMUTE", "MEDIAPLAY", "MEDIAPAUSE", "MEDIAPLAYPAUSE",
        "MEDIASTOP", "MEDIATRACKNEXT", "MEDIATRACKPREV", "MEDIATRACKPREVIOUS",
        "DIGIT0", "DIGIT1", "DIGIT2", "DIGIT3", "DIGIT4", "DIGIT5", "DIGIT6", "DIGIT7",
        "DIGIT8", "DIGIT9",
    ];
    // `KEYA` bicimi de kabul edilir.
    let mut t = tok.to_string();
    if t.len() == 4 && t.starts_with("KEY") {
        t = t[3..].to_string();
    }
    if t.chars().count() == 1 {
        return Some(t);
    }
    if NAMED.contains(&t.as_str()) {
        // Gorunum: `ESC` -> `Esc`, `PAGEDOWN` -> `PageDown`, `F9` aynen.
        let mut out = String::with_capacity(t.len());
        let mut cap = true;
        for c in t.chars() {
            if cap {
                out.extend(c.to_uppercase());
                cap = false;
            } else {
                out.extend(c.to_lowercase());
            }
        }
        // `F9` gibi kisa kodlar buyuk kalir.
        if t.starts_with('F') && t[1..].chars().all(|c| c.is_ascii_digit()) {
            return Some(t);
        }
        return Some(out);
    }
    None
}

/// Ham girdiyi kanonik forma sokar (`" ctrl + shift + k "` -> `"Ctrl+Shift+K"`).
/// Desteklenmiyorsa `None` (cagiran `kisayol-gecersiz` doner).
pub fn canonical(raw: &str) -> Option<String> {
    let toks: Vec<String> = raw
        .split('+')
        .map(|s| s.trim().to_ascii_uppercase())
        .collect();
    if toks.is_empty() || toks.iter().any(|t| t.is_empty()) {
        return None;
    }
    let mut mods: Vec<&'static str> = Vec::new();
    let mut key: Option<String> = None;
    for t in &toks {
        if key.is_some() {
            return None; // Ana tustan sonra jeton gelemez (sira kuralı).
        }
        if let Some(m) = canon_mod(t) {
            if mods.contains(&m) {
                return None; // Yinelenen degistirici.
            }
            mods.push(m);
        } else if let Some(k) = canon_key(t) {
            key = Some(k);
        } else {
            return None;
        }
    }
    let key = key?;
    if mods.is_empty() {
        return Some(key);
    }
    mods.sort_by_key(|m| MOD_ORDER.iter().position(|o| o == m).unwrap_or(99));
    Some(format!("{}+{}", mods.join("+"), key))
}

/// Destekleniyor mu (bosluk/buyuk-kucuk duyarsiz)?
pub fn is_supported(raw: &str) -> bool {
    canonical(raw).is_some()
}

/// Dosyadan oku; yoksa/bozuksa/desteklenmiyorsa varsayilan.
/// Sirlama YOK (deger ekranda gosterilir; hassas degil).
pub fn load_from_file(path: &std::path::Path) -> String {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(_) => return DEFAULT_HOTKEY.to_string(),
    };
    let v: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return DEFAULT_HOTKEY.to_string(),
    };
    let name = v.get("hotkey").and_then(|x| x.as_str()).unwrap_or("");
    canonical(name).unwrap_or_else(|| DEFAULT_HOTKEY.to_string())
}

/// Dosyaya yaz (yoksa dizini acar). Gecersizde `kisayol-gecersiz`.
pub fn save_to_file(path: &std::path::Path, key: &str) -> Result<(), String> {
    let canon = canonical(key).ok_or_else(|| "kisayol-gecersiz".to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("dizin-hatasi:{e}"))?;
    }
    let text = serde_json::to_string(&serde_json::json!({"hotkey": canon}))
        .map_err(|_| "kayit-hatasi".to_string())?;
    std::fs::write(path, text).map_err(|e| format!("yazma-hatasi:{e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn singles_still_work() {
        assert_eq!(canonical(" f10 "), Some("F10".into()));
        assert_eq!(canonical("f9"), Some("F9".into()));
        assert_eq!(canonical("q"), Some("Q".into()));
        assert_eq!(canonical("5"), Some("5".into()));
        assert_eq!(canonical("space"), Some("Space".into()));
    }

    #[test]
    fn combos_with_aliases() {
        assert_eq!(canonical("ctrl+shift+k"), Some("Ctrl+Shift+K".into()));
        assert_eq!(canonical(" Shift + Alt + KeyQ "), Some("Alt+Shift+Q".into()));
        assert_eq!(canonical("cmd+q"), Some("Super+Q".into()));
        assert_eq!(canonical("win+f12"), Some("Super+F12".into()));
        assert_eq!(canonical("option+space"), Some("Alt+Space".into()));
    }

    #[test]
    fn rejects_bad_shapes() {
        for bad in ["", "   ", "Ctrl", "Shift+Alt", "Ctrl++K", "K+Ctrl", "Ctrl+K+Q", "Ctrl+Ctrl+K", "F13+X", "Ctrl+Foo"] {
            assert!(canonical(bad).is_none(), "{bad} gecersiz olmali");
            assert!(!is_supported(bad));
        }
    }

    #[test]
    fn os_trap_keys_parse_but_may_fail_at_runtime() {
        // Eklenti cozumler; kayit tutmazsa calisma-aninda hata doner.
        assert_eq!(canonical("PrintScreen"), Some("Printscreen".into()));
    }

    #[test]
    fn file_roundtrip_and_fail_open() {
        let dir = std::env::temp_dir().join("whisperexe-test-hotkey2");
        let path = dir.join("hotkey.json");
        let _ = std::fs::remove_file(&path);
        assert_eq!(load_from_file(&path), "F9");
        save_to_file(&path, "ctrl+shift+k").expect("kayit");
        assert_eq!(load_from_file(&path), "Ctrl+Shift+K");
        assert_eq!(save_to_file(&path, "Ctrl"), Err("kisayol-gecersiz".into()));
        std::fs::write(&path, "{bozuk").unwrap();
        assert_eq!(load_from_file(&path), "F9");
        let _ = std::fs::remove_file(&path);
    }
}
