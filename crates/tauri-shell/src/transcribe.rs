//! Yerel transkripsiyon: F9 birakinca biriken 16kHz mono PCM, calisan
//! `wl --serve`'e (`127.0.0.1:8888/v1/audio/transcriptions`, multipart)
//! gonderilir; metin overlay'de gosterilir. Sunucu yoksa/bozuksa hata
//! toast'i duser (hat asili kalmaz). Ucretsizdir (local hat).

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
