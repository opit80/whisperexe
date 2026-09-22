//! Bas-güncelle: GitHub son sürüm kurulumunu indirip çalıştırır.
//!
//! İmza doğrulaması YOKTUR (sahip kararı: basitlik önde). Kaynak sabittir:
//! yalnız `github.com/opit80/whisperexe` sürümleri, yalnız `-setup.exe`
//! ile biten dosya. Boyut tavanı 100MB; yönlendirme en fazla 5 izlenir.

/// Son sürüm bilgi ucu (sabit).
pub const RELEASES_LATEST: &str =
    "https://api.github.com/opit80/whisperexe/releases/latest";

/// API yanıt vermezse kullanılacak doğrudan adres (sabit).
pub const SETUP_DIRECT_URL: &str =
    "https://github.com/opit80/whisperexe/releases/latest/download/whisperexe_0.1.0_x64-setup.exe";

/// İndirme tavanı: 100MB.
pub const MAX_BYTES: usize = 100 * 1024 * 1024;

/// Tauri updater açılış denetimi için uygulanan-besleme işareti.
///
/// İkili sürüm `0.1.0` sabittir (kurulum dosya adı değişmez); besleme sürümü
/// (`vX.Y.Z` tag'i) ise ilerler. Updater `check()` bu yüzden her açılışta
/// yenilik bulur — işaret olmasa her girişte yeniden kurulup yeniden
/// başlatılır. Bu dosya (app-data dizininde) son kurulan/bilinen besleme
/// sürümünü tutar:
/// - dosya yoksa o anki besleme benimsenir (temiz kurulumda aynı kurulum
///   yeniden indirilip kurulmaz; açılış duman testi korunur),
/// - dosya varsa ve besleme daha yeniyse bekleyen güncelleme kurulur.
pub const APPLIED_FILE_NAME: &str = "update-applied.json";

/// İşaret dosyası yolu (oturum dosyasıyla aynı dizinde tutulur).
pub fn applied_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join(APPLIED_FILE_NAME)
}

/// Kayıtlı besleme sürümü (yoksa/bozuksa `None` → arayan benimser).
pub fn read_applied_version(dir: &std::path::Path) -> Option<String> {
    let raw = std::fs::read_to_string(applied_path(dir)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let s = v.get("version")?.as_str()?;
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// Besleme sürümünü işler (hata sessiz: fail-open, akış kesilmez).
pub fn write_applied_version(dir: &std::path::Path, version: &str) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(
        applied_path(dir),
        serde_json::json!({"version": version}).to_string(),
    );
}

/// Besleme, kayıtlı sürümden daha mı yeni? (`v` öneki toleranslı.)
pub fn feed_is_newer(feed: &str, applied: &str) -> bool {
    cmp_tag(feed, applied) == std::cmp::Ordering::Greater
}

fn tag_parts(v: &str) -> Vec<u64> {
    v.trim()
        .trim_start_matches(['v', 'V'])
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

fn cmp_tag(a: &str, b: &str) -> std::cmp::Ordering {
    let pa = tag_parts(a);
    let pb = tag_parts(b);
    let n = pa.len().max(pb.len());
    for i in 0..n {
        let x = pa.get(i).copied().unwrap_or(0);
        let y = pb.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            std::cmp::Ordering::Equal => continue,
            o => return o,
        }
    }
    std::cmp::Ordering::Equal
}

/// `releases/latest` yanıtından kurulum dosyasını seç: (etiket, url).
pub fn pick_setup_asset(meta: &serde_json::Value) -> Option<(String, String)> {
    let tag = meta.get("tag_name")?.as_str()?.to_string();
    let assets = meta.get("assets")?.as_array()?;
    let url = assets.iter().find_map(|a| {
        let name = a.get("name")?.as_str()?;
        if name.ends_with("-setup.exe") {
            a.get("browser_download_url")?.as_str().map(|s| s.to_string())
        } else {
            None
        }
    })?;
    Some((tag, url))
}

#[cfg(feature = "tauri")]
fn file_name_of(url: &str) -> String {
    url.rsplit('/').next().unwrap_or("whisperexe-setup.exe").to_string()
}

/// Yönlendirmeleri izleyerek indir (en fazla 5 halka).
#[cfg(feature = "tauri")]
fn download(url: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut next = url.to_string();
    for _ in 0..6 {
        let res = crate::net::download_agent()
            .get(&next)
            .header("Accept", "application/octet-stream")
            .header("User-Agent", "whisperexe")
            .call()
            .map_err(|e| format!("indirme-hatasi:{e}"))?;
        let status = res.status().as_u16();
        if (300..400).contains(&status) {
            let loc = res
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| "yonlendirme-hatasi".to_string())?
                .to_string();
            next = loc;
            continue;
        }
        if !(200..300).contains(&status) {
            return Err(format!("http-{status}"));
        }
        let body = res.into_body().into_reader();
        let mut buf = Vec::new();
        body
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut buf)
            .map_err(|e| format!("okuma-hatasi:{e}"))?;
        if buf.len() > MAX_BYTES {
            return Err("dosya-cok-buyuk".to_string());
        }
        if buf.is_empty() {
            return Err("bos-dosya".to_string());
        }
        return Ok(buf);
    }
    Err("cok-yonlendirme".to_string())
}

/// Son sürümü indirip geçici dizine yazar: (etiket, dosya yolu).
/// Ağ işi çağıran iş parçacığında olur (komut tarafı çağırır).
#[cfg(feature = "tauri")]
fn latest_meta() -> Option<serde_json::Value> {
    let res = crate::net::agent()
        .get(RELEASES_LATEST)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "whisperexe")
        .call()
        .ok()?;
    if !(200..300).contains(&res.status().as_u16()) {
        return None;
    }
    let text = res.into_body().read_to_string().ok()?;
    serde_json::from_str(&text).ok()
}

/// Son sürümü indirip geçici dizine yazar: (etiket, dosya yolu).
/// Önce API'den dosya seçilir, API susarsa doğrudan adres denenir.
/// Ağ işi çağıran iş parçacığında olur (komut tarafı çağırır).
#[cfg(feature = "tauri")]
pub fn fetch_and_stage() -> Result<(String, std::path::PathBuf), String> {
    let (tag, url) = latest_meta()
        .and_then(|m| pick_setup_asset(&m))
        .unwrap_or_else(|| ("son sürüm".to_string(), SETUP_DIRECT_URL.to_string()));
    let bytes = download(&url)?;
    let dir = std::env::temp_dir().join("whisperexe-update");
    std::fs::create_dir_all(&dir).map_err(|e| format!("dizin-hatasi:{e}"))?;
    let path = dir.join(file_name_of(&url));
    std::fs::write(&path, &bytes).map_err(|e| format!("yazma-hatasi:{e}"))?;
    Ok((tag, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_meta() -> serde_json::Value {
        serde_json::json!({
            "tag_name": "v0.2.2",
            "assets": [
                {"name": "latest.json", "browser_download_url": "https://x/latest.json"},
                {"name": "whisperexe_0.1.0_x64-setup.exe", "browser_download_url": "https://y/kurulum.exe"}
            ]
        })
    }

    #[test]
    fn kurulum_dosyasi_secilir() {
        assert_eq!(
            pick_setup_asset(&sample_meta()),
            Some(("v0.2.2".to_string(), "https://y/kurulum.exe".to_string()))
        );
    }

    #[test]
    fn kurulum_yoksa_yok() {
        let m = serde_json::json!({"tag_name": "v1", "assets": []});
        assert_eq!(pick_setup_asset(&m), None);
        assert_eq!(pick_setup_asset(&serde_json::json!({})), None);
    }

    #[test]
    fn besleme_isareti_yoksa_yoktur_yazinca_okunur() {
        let dir = std::env::temp_dir().join(format!(
            "whisperexe-test-isaret-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("saat")
                .as_nanos()
        ));
        assert_eq!(read_applied_version(&dir), None);
        write_applied_version(&dir, "v0.2.7");
        assert_eq!(
            read_applied_version(&dir),
            Some("v0.2.7".to_string())
        );
        // Bozuk dosya fail-open: None (benimse), akış kesilmez.
        std::fs::write(applied_path(&dir), "{bozuk").expect("yaz");
        assert_eq!(read_applied_version(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn besleme_yeniligi_tag_karsilastirir() {
        assert!(feed_is_newer("v0.2.7", "v0.2.6"));
        assert!(feed_is_newer("0.2.7", "v0.2.6"));
        assert!(!feed_is_newer("v0.2.6", "v0.2.6"));
        assert!(!feed_is_newer("v0.2.6", "v0.2.7"));
        assert!(feed_is_newer("v0.2.10", "v0.2.9"));
    }

    /// Canlı yol (ağ ister; varsayılan koşuda atlanır).
    #[test]
    #[ignore]
    #[cfg(feature = "tauri")]
    fn canli_github_kurulum_iner() {
        let (tag, path) = fetch_and_stage().expect("github indirme");
        assert!(!tag.is_empty());
        let n = std::fs::metadata(&path).expect("dosya").len();
        assert!(n > 1_000_000, "kurulum kucuk: {n}");
        let _ = std::fs::remove_file(&path);
    }
}
