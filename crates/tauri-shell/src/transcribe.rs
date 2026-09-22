//! Yerel transkripsiyon: F9 birakinca biriken 16kHz mono PCM, calisan
//! `wl --serve`'e (`127.0.0.1:8888/v1/audio/transcriptions`, multipart)
//! gonderilir; metin overlay'de gosterilir. Yerel kapida degilse broker
//! relay hattina dusulur (`POST {broker}/v1/transcribe`, ham i16 LE govde;
//! broker LAN-fazinda govdeyi 16kHz mono PCM sayar).
//! Sunucu yoksa/bozuksa hata toast'i duser (hat asili kalmaz).

use std::sync::atomic::{AtomicU64, Ordering};

/// Hedef: yerel `wl --serve` transkripsiyon ucu.
pub const LOCAL_TRANSCRIBE_URL: &str = "http://127.0.0.1:8888/v1/audio/transcriptions";
/// `wl` model alani (sunucu ayari `large` ile calisir; deger yoksayilir).
pub const TRANSCRIBE_MODEL: &str = "large";
/// Gorunen metin tavani (overlay toast'i sismez).
pub const TEXT_PREVIEW_CHARS: usize = 160;

/// 16kHz mono i16 PCM -> WAV bayti (44B baslik + LE ornekler).
pub fn wav_bytes_16k_mono(pcm: &[i16]) -> Vec<u8> {
    let data_len = pcm.len() * 2;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt boyu
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&16_000u32.to_le_bytes()); // hiz
    out.extend_from_slice(&32_000u32.to_le_bytes()); // bayt/sn
    out.extend_from_slice(&2u16.to_le_bytes()); // blok
    out.extend_from_slice(&16u16.to_le_bytes()); // bit
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// multipart/form-data govdesi (`file` + `model`).
pub fn multipart_body(boundary: &str, wav: &[u8], model: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    out.extend_from_slice(
        b"Content-Disposition: form-data; name=\"model\"\r\n\r\n",
    );
    out.extend_from_slice(model.as_bytes());
    out.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
    out.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n",
    );
    out.extend_from_slice(wav);
    out.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    out
}

/// Yanit govdesinden metni cikar (`{"text": "..."}`).
pub fn parse_text(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("text")?
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Metin onizlemesi (karakter tavanli, Unicode guvenli).
pub fn preview(text: &str) -> String {
    let mut out = String::new();
    for (i, c) in text.chars().enumerate() {
        if i >= TEXT_PREVIEW_CHARS {
            out.push('…');
            break;
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------------------
// Broker relay hatti (uzak makine: yerel `wl --serve` yoksa)
// ---------------------------------------------------------------------------

/// Broker govde tavani payli (protokol 2MB; broker govdeyi ham i16 LE sayar,
/// yani tavan ~65sn sese denktir. Gercek Opus kodlayici gelene dek gecerli).
pub const BROKER_BODY_CAP_BYTES: usize = 2 * 1024 * 1024 - 1024;

/// 16kHz mono i16 -> ham LE bayt (broker relay govdesi).
pub fn pcm16_bytes(pcm: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pcm.len() * 2);
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Govdeyi tavana sigdir (bastan keser; kisa dikte etkilenmez).
pub fn fit_broker_body(pcm: &[i16]) -> Vec<u8> {
    let max_samples = BROKER_BODY_CAP_BYTES / 2;
    let take = pcm.len().min(max_samples);
    pcm16_bytes(&pcm[..take])
}

/// Baytlarin SHA-256 hex ozeti (`X-Audio-Hash` basligi).
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    const H: &[u8; 16] = b"0123456789abcdef";
    let dg = Sha256::digest(data);
    let mut s = String::with_capacity(64);
    for x in dg {
        s.push(H[(x >> 4) as usize] as char);
        s.push(H[(x & 15) as usize] as char);
    }
    s
}

/// Tekil istek kimligi (`[A-Za-z0-9._:-]`, <=128): `req-<ms>-<sayac>`.
pub fn new_request_id() -> String {
    static CTR: AtomicU64 = AtomicU64::new(0);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let n = CTR.fetch_add(1, Ordering::Relaxed);
    format!("req-{ms}-{n}")
}

/// Broker basarili yaniti: metin + kesilen tutar (bilgi toast'i icin).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokerText {
    pub text: String,
    pub cost_kurus: Option<i64>,
}

/// Broker `code` -> kisa Turkce yuzey metni (ucret-durumu cumlede gecer).
pub fn friendly_broker_error(code: &str) -> String {
    match code {
        "insufficient_balance" => "Bakiye yetersiz, yükleme gerekli (ücret yazılmadı).".into(),
        "silent_audio" => "Ses duyulmadı, gönderilmedi (ücret yok).".into(),
        "queue_full" | "queue_wait_timeout" => {
            "Kuyruk dolu, birazdan tekrar dene (ücret yok).".into()
        }
        "account_busy" => "Hesabın işleyen isteği var, bitince dene (ücret yok).".into(),
        "empty_transcript" => "Metin çıkmadı (ücret yazılmadı).".into(),
        "unauthorized" | "forbidden_hwid" | "giris-gerekli" => {
            "Giriş gerekli: tekrar giriş yap (ücret yok).".into()
        }
        "payload_too_large" => "Kayıt çok uzun, daha kısa dene (ücret yok).".into(),
        "maintenance" => "Sunucu bakımda (ücret yok).".into(),
        "baglanti-hatasi" => "Broker'a ulaşılamadı, bağlantıyı denetle (ücret yok).".into(),
        _ => format!("Gönderilemedi ({code}) (ücret yok)."),
    }
}

fn broker_error_code(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("error")?
        .get("code")?
        .as_str()
        .map(|s| s.to_string())
}

/// Broker yanitini coz: `Ok(metin)` ya da kullaniciya gosterilecek hazir metin.
pub fn parse_broker_response(status: u16, body: &str) -> Result<BrokerText, String> {
    if (200..300).contains(&status) {
        let v: serde_json::Value =
            serde_json::from_str(body).map_err(|_| "bozuk-yanit (ücret yok).".to_string())?;
        let text = v
            .get("text")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "Metin çıkmadı (ücret yazılmadı).".to_string())?;
        let cost = v.get("cost_kurus").and_then(|x| x.as_i64());
        return Ok(BrokerText {
            text: text.to_string(),
            cost_kurus: cost,
        });
    }
    let code = broker_error_code(body).unwrap_or_else(|| format!("http-{status}"));
    Err(friendly_broker_error(&code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_valid_pcm() {
        let pcm = vec![0i16, 1000, -1000, 32767];
        let b = wav_bytes_16k_mono(&pcm);
        assert_eq!(b.len(), 44 + 8);
        assert_eq!(&b[0..4], b"RIFF");
        assert_eq!(&b[8..12], b"WAVE");
        assert_eq!(&b[12..16], b"fmt ");
        assert_eq!(u16::from_le_bytes([b[20], b[21]]), 1); // PCM
        assert_eq!(u16::from_le_bytes([b[22], b[23]]), 1); // mono
        assert_eq!(u32::from_le_bytes([b[24], b[25], b[26], b[27]]), 16_000);
        assert_eq!(&b[36..40], b"data");
        assert_eq!(u32::from_le_bytes([b[40], b[41], b[42], b[43]]), 8);
        assert_eq!(i16::from_le_bytes([b[44], b[45]]), 0);
        assert_eq!(i16::from_le_bytes([b[50], b[51]]), 32767);
    }

    #[test]
    fn multipart_has_both_fields() {
        let body = multipart_body("SINIR123", &[1, 2, 3], "large");
        let s = String::from_utf8_lossy(&body);
        assert!(s.contains("--SINIR123\r\n"));
        assert!(s.contains("name=\"model\""));
        assert!(s.contains("name=\"file\"; filename=\"audio.wav\""));
        assert!(s.ends_with("--SINIR123--\r\n"));
    }

    #[test]
    fn parses_transcript() {
        assert_eq!(
            parse_text(r#"{"text": " Merhaba. "}"#).as_deref(),
            Some("Merhaba.")
        );
        assert_eq!(parse_text(r#"{"nope": 1}"#), None);
        assert_eq!(parse_text("bozuk"), None);
    }

    #[test]
    fn preview_truncates() {
        let long = "a".repeat(200);
        let p = preview(&long);
        assert!(p.ends_with('…'));
        assert_eq!(p.chars().count(), TEXT_PREVIEW_CHARS + 1);
    }

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn request_id_unique_and_wire_safe() {
        let a = new_request_id();
        let b = new_request_id();
        assert_ne!(a, b);
        for id in [a, b] {
            assert!(id.len() <= 128);
            assert!(id.bytes().all(|c| c.is_ascii_alphanumeric()
                || c == b'.'
                || c == b'_'
                || c == b'-'
                || c == b':'));
        }
    }

    #[test]
    fn broker_body_roundtrip_and_cap() {
        let pcm = vec![1i16, -2, 300];
        let body = fit_broker_body(&pcm);
        assert_eq!(body.len(), 6);
        assert_eq!(i16::from_le_bytes([body[0], body[1]]), 1);
        // Tavan: asiri uzun kayit kesilir, cift baytta kalir.
        let long = vec![0x40i16; BROKER_BODY_CAP_BYTES];
        let fit = fit_broker_body(&long);
        assert_eq!(fit.len(), BROKER_BODY_CAP_BYTES);
        assert_eq!(fit.len() % 2, 0);
    }

    #[test]
    fn broker_ok_parses_text_and_cost() {
        let r = parse_broker_response(
            200,
            r#"{"ok":true,"cached":false,"text":" Merhaba. ","cost_kurus":6}"#,
        )
        .expect("ok cozulmeli");
        assert_eq!(r.text, "Merhaba.");
        assert_eq!(r.cost_kurus, Some(6));
    }

    #[test]
    fn broker_ok_empty_text_is_error() {
        assert!(parse_broker_response(200, r#"{"ok":true,"text":"  "}"#).is_err());
        assert!(parse_broker_response(200, "bozuk").is_err());
    }

    #[test]
    fn broker_errors_map_to_turkish() {
        let e = parse_broker_response(
            402,
            r#"{"ok":false,"error":{"code":"insufficient_balance","message":"x"}}"#,
        )
        .unwrap_err();
        assert!(e.contains("Bakiye"), "metin: {e}");
        let e = parse_broker_response(
            400,
            r#"{"ok":false,"error":{"code":"silent_audio","message":"x"}}"#,
        )
        .unwrap_err();
        assert!(e.contains("duyulmad"), "metin: {e}");
        let e = parse_broker_response(503, "duz yazi").unwrap_err();
        assert!(e.contains("http-503"), "metin: {e}");
    }

    #[cfg(feature = "tauri")]
    #[test]
    fn live_local_transcribe_roundtrip() {
        // Canli `wl --serve` varsa uctan uca yoklanir; yoksa sessizce gecilir.
        let tone: Vec<i16> = (0..16_000)
            .map(|i| ((i as f32 * 440.0 * 2.0 * std::f32::consts::PI / 16000.0).sin() * 0.3 * 32767.0) as i16)
            .collect();
        let wav = wav_bytes_16k_mono(&tone);
        let boundary = "whisptest";
        let body = multipart_body(boundary, &wav, TRANSCRIBE_MODEL);
        let res = crate::net::download_agent()
            .post(LOCAL_TRANSCRIBE_URL)
            .header("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
            .send(&body[..]);
        let Ok(res) = res else {
            eprintln!("yerel sunucu yok, test atlandi");
            return;
        };
        let text = res.into_body().read_to_string().unwrap_or_default();
        assert!(text.contains("\"text\""), "yanit metin tasimali: {text}");
    }
}
