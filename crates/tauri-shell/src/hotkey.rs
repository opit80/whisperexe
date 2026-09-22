//! Bas-konus global kisayol ayari (F9 varsayilan, ayarlardan degisir).
//!
//! Desteklenen tuslar YALNIZCA islev tuslaridir (F1-F12): PrintScreen /
//! ScrollLock gibi OS-tuzakli tuslar bilerek DISARIDA tutulur. Gecersiz
//! degerde komut `"kisayol-gecersiz"` doner (on-yuz `Hata: ...` basar).
//! Kalicilik: app-data `hotkey.json` (`{"hotkey":"F9"}`); yoksa/bozuksa
//! sessizce varsayilana dusulur (acilis engellenmez).

/// Kalici kisayol dosyasi adi (app-data dizininde).
pub const HOTKEY_FILE_NAME: &str = "hotkey.json";

/// Kayit yoksa/bozuksa kullanilan tus.
pub const DEFAULT_HOTKEY: &str = "F9";

/// Guvenli kume: F1-F12 (PrintScreen/ScrollLock YOK).
pub const SUPPORTED: &[&str] = &[
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
];

/// Ham girdiyi kanonik forma sokar (`" f10 "` -> `"F10"`).
/// Desteklenmiyorsa `None` (cagiran `kisayol-gecersiz` doner).
pub fn canonical(raw: &str) -> Option<&'static str> {
    let up = raw.trim().to_ascii_uppercase();
    SUPPORTED.iter().copied().find(|s| *s == up)
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
    canonical(name)
        .map(|s| s.to_string())
        .unwrap_or_else(|| DEFAULT_HOTKEY.to_string())
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
    fn canonical_trims_and_uppercases() {
        assert_eq!(canonical(" f10 "), Some("F10"));
        assert_eq!(canonical("f9"), Some("F9"));
        assert_eq!(canonical("F12"), Some("F12"));
    }

    #[test]
    fn rejects_os_traps_and_junk() {
        for bad in ["PrintScreen", "ScrollLock", "Space", "A", "", "F13", "Ctrl+F9"] {
            assert!(!is_supported(bad), "{bad} desteklenmemeli");
            assert_eq!(canonical(bad), None);
        }
    }

    #[test]
    fn file_roundtrip_and_fail_open() {
        let dir = std::env::temp_dir().join("whisperexe-test-hotkey");
        let path = dir.join("hotkey.json");
        let _ = std::fs::remove_file(&path);
        // Yoksa varsayilan.
        assert_eq!(load_from_file(&path), "F9");
        save_to_file(&path, "f10").expect("kayit");
        assert_eq!(load_from_file(&path), "F10");
        // Gecersiz yazilmaz, hata kodu kisa.
        assert_eq!(save_to_file(&path, "Space"), Err("kisayol-gecersiz".into()));
        // Bozuk dosya -> varsayilan (acilis engellenmez).
        std::fs::write(&path, "{bozuk").unwrap();
        assert_eq!(load_from_file(&path), "F9");
        let _ = std::fs::remove_file(&path);
    }
}
