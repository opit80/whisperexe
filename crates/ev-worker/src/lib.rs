//! ev-worker: ev PC işçisi adaptörü (F1b).
//!
//! PLAN.md §4: "Model aynen kalır; önüne token kontrolü + 15sn heartbeat +
//! gerçek-süre ölçümü + tek-GPU mutex eklenir (broker kuyruğundan bağımsız,
//! derin savunma). Ölçümde işçi decode'u yetkilidir, broker bayt sayımı
//! çapraz kontroldür; sapmada işçi kazanır, uyarı düşer."
//!
//! SINIR: `server/kod`'a DOKUNULMAZ (canlı `whisper_key` + model aynen).
//! Bu crate yalnızca adaptördür: yetki, nabız, mutex, ölçüm, mutabakat.
//! Gerçek inference bağlantısı `InferenceBackend` arayüzündedir; canlı
//! bağlama ayrı fazda, bu fazda sahte gövdeyle gelir.

use std::sync::atomic::{AtomicBool, Ordering};

/// İşçi heartbeat periyodu: 15sn (PLAN.md §4).
pub const HEARTBEAT_SECS: u64 = 15;
/// Broker bayt-sayımı çapraz kontrol toleransı (sn).
pub const RECONCILE_TOLERANCE_SECS: f64 = 1.0;

/// Token ret nedenleri. Kabul anında geçerli token isteğin sonuna kadar
/// yaşar (ortada ölmez); revoke sonraki kabulde işler — çağrı tarafı
/// kabulde alınan onayı istek sonuna kadar taşır.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TokenReject {
    Invalid,
    Expired,
}

/// İşçi önü token kontrolü (broker imzasını/oturum anahtarını doğrular).
/// Gerçek imza doğrulama auth fazınındır (F1a); burada kabul-kararı arayüzü.
#[derive(Clone, Debug)]
pub struct WorkerAuth {
    expected: String,
    expires_at: u64,
}

impl WorkerAuth {
    pub fn new(expected: &str, expires_at_secs: u64) -> Self {
        Self {
            expected: expected.to_string(),
            expires_at: expires_at_secs,
        }
    }

    pub fn authorize(&self, presented: &str, now_secs: u64) -> Result<(), TokenReject> {
        if now_secs >= self.expires_at {
            return Err(TokenReject::Expired);
        }
        if presented != self.expected {
            return Err(TokenReject::Invalid);
        }
        Ok(())
    }
}

/// 15sn heartbeat izleyici (işçi → broker nabzı).
#[derive(Clone, Copy, Debug)]
pub struct Heartbeat {
    last_beat: u64,
}

impl Heartbeat {
    pub fn new(now_secs: u64) -> Self {
        Self { last_beat: now_secs }
    }

    pub fn beat(&mut self, now_secs: u64) {
        self.last_beat = now_secs;
    }

    /// Nabız gecikti mi (broker tarafı N=3 kaçırmada offline sayar).
    pub fn overdue(&self, now_secs: u64) -> bool {
        now_secs.saturating_sub(self.last_beat) >= HEARTBEAT_SECS
    }
}

/// Tek-GPU mutex: broker kuyruğundan bağımsız derin savunma.
/// İkinci istek beklemez, anında geri çevrilir (broker yeniden sıraya alır).
#[derive(Debug, Default)]
pub struct GpuMutex {
    busy: AtomicBool,
}

pub struct GpuGuard<'a> {
    owner: &'a AtomicBool,
}

impl Drop for GpuGuard<'_> {
    fn drop(&mut self) {
        self.owner.store(false, Ordering::SeqCst);
    }
}

impl GpuMutex {
    pub fn new() -> Self {
        Self {
            busy: AtomicBool::new(false),
        }
    }

    /// GPU boşsa kilitler, doluysa None (bekleme yok).
    pub fn try_lock(&self) -> Option<GpuGuard<'_>> {
        if self
            .busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            Some(GpuGuard { owner: &self.busy })
        } else {
            None
        }
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }
}

/// Gerçek-süre ölçümü: çözülmüş PCM'den saniye (işçi decode'u yetkilidir).
pub fn measure_secs(sample_count: usize, rate_hz: u32) -> f64 {
    if rate_hz == 0 {
        return 0.0;
    }
    sample_count as f64 / rate_hz as f64
}

/// Mutabakat sonucu: sapmada İŞÇİ kazanır + uyarı düşer.
#[derive(Clone, Copy, Debug)]
pub struct Reconciled {
    /// Faturalandırılacak saniye (her zaman işçi ölçümü).
    pub secs: f64,
    /// true ise broker çapraz-kontrol saptı → uyarı loglanır.
    pub worker_wins_warning: bool,
}

/// İşçi ölçümü vs broker bayt-sayımı çapraz kontrolü.
pub fn reconcile(
    worker_secs: f64,
    broker_secs: f64,
    tolerance_secs: f64,
) -> Reconciled {
    let drift = (worker_secs - broker_secs).abs();
    Reconciled {
        secs: worker_secs,
        worker_wins_warning: drift > tolerance_secs,
    }
}

/// Inference arka-ucu arayüzü. Canlı bağlama (`server/kod/whisper_key`,
/// large + CUDA) `WlBackend` ile yapılır; `server/kod` DEĞİŞTİRİLMEZ,
/// yalnızca HTTP ile çağrılır. Bu trait'e dokunan kod `server/kod`'u
/// DEĞİŞTİRMEZ, yalnızca çağırır.
pub trait InferenceBackend {
    fn run(&self, pcm_samples: &[i16], rate_hz: u32) -> String;
}

/// Sahte arka-uç (arayüz + sahte gövde; `EV_BACKEND` ayarlanmadan kullanılır).
pub struct MockBackend;

impl InferenceBackend for MockBackend {
    fn run(&self, pcm_samples: &[i16], rate_hz: u32) -> String {
        format!(
            "[isçi {:.1}sn] ornek transkript",
            measure_secs(pcm_samples.len(), rate_hz)
        )
    }
}

/// Gerçek arka-uç: evdeki `wl --serve` (`POST /v1/audio/transcriptions`,
/// multipart `file` + `response_format=text`, yanıt düz metin).
/// std-only (TcpStream); hata/timeout/200-dışı → BOŞ metin döner
/// (çağrı tarafı ücretsiz sayar) + stderr'e tek satır log düşer.
pub struct WlBackend {
    /// Örn. `127.0.0.1:8888`.
    pub addr: String,
    /// Bağlantı+okuma tavanı (sn).
    pub timeout_secs: u64,
}

impl WlBackend {
    pub fn local() -> Self {
        Self {
            addr: std::env::var("EV_WL_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:8888".to_string()),
            timeout_secs: 300,
        }
    }
}

/// 16-bit mono WAV kodlar (RIFF başlığı elle, std-only).
fn encode_wav_mono16(samples: &[i16], rate_hz: u32) -> Vec<u8> {
    let data_len = samples.len().saturating_mul(2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36u32.saturating_add(data_len)).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate_hz.to_le_bytes());
    out.extend_from_slice(&rate_hz.saturating_mul(2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// multipart/form-data gövdesi: `file` (audio.wav) + `response_format=text` +
/// `language=tr`.
fn multipart_body(boundary: &str, wav: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(wav.len() + 512);
    b.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\ntext\r\n"
        )
        .as_bytes(),
    );
    b.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"language\"\r\n\r\ntr\r\n"
        )
        .as_bytes(),
    );
    b.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    b.extend_from_slice(wav);
    b.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    b
}

/// Ham HTTP yanıtını (durum, gövde) ayırır.
fn split_response(raw: &[u8]) -> Option<(u16, Vec<u8>)> {
    let head_end = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..head_end]).ok()?;
    let status: u16 = head.lines().next()?.split_whitespace().nth(1)?.parse().ok()?;
    Some((status, raw[head_end + 4..].to_vec()))
}

impl InferenceBackend for WlBackend {
    fn run(&self, pcm_samples: &[i16], rate_hz: u32) -> String {
        let fail = |why: &str| {
            eprintln!("ev-worker: wl cagrisi basarisiz: {why}");
            String::new()
        };
        if pcm_samples.is_empty() || rate_hz == 0 {
            return fail("bos pcm");
        }
        let wav = encode_wav_mono16(pcm_samples, rate_hz);
        let boundary = "whisperexe7a9b";
        let body = multipart_body(boundary, &wav);
        let req = format!(
            "POST /v1/audio/transcriptions HTTP/1.1\r\nHost: {}\r\nContent-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.addr,
            body.len()
        );
        let timeout = std::time::Duration::from_secs(self.timeout_secs.max(1));
        let mut stream = match std::net::TcpStream::connect(&self.addr) {
            Ok(s) => s,
            Err(e) => return fail(&format!("baglanti: {e}")),
        };
        if stream.set_write_timeout(Some(timeout)).is_err()
            || stream.set_read_timeout(Some(timeout)).is_err()
        {
            return fail("timeout ayari");
        }
        use std::io::Write;
        if stream.write_all(req.as_bytes()).is_err() || stream.write_all(&body).is_err() {
            return fail("gonderme");
        }
        use std::io::Read;
        let mut raw = Vec::new();
        if stream.read_to_end(&mut raw).is_err() {
            return fail("okuma");
        }
        match split_response(&raw) {
            Some((200, text)) => String::from_utf8_lossy(&text).into_owned(),
            Some((st, _)) => fail(&format!("http-{st}")),
            None => fail("bozuk yanit"),
        }
    }
}

/// Arka-uç seçimi: `EV_BACKEND=wl` → gerçek (`WlBackend`),
/// diğer her şey → sahte (`MockBackend`, güvenli varsayılan).
pub enum Backend {
    Mock(MockBackend),
    Wl(WlBackend),
}

impl InferenceBackend for Backend {
    fn run(&self, pcm_samples: &[i16], rate_hz: u32) -> String {
        match self {
            Backend::Mock(b) => b.run(pcm_samples, rate_hz),
            Backend::Wl(b) => b.run(pcm_samples, rate_hz),
        }
    }
}

pub fn backend() -> Backend {
    match std::env::var("EV_BACKEND").as_deref() {
        Ok("wl") => Backend::Wl(WlBackend::local()),
        _ => Backend::Mock(MockBackend),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_kontrolu_gecerli_gecersiz_suresi_dolmus() {
        let a = WorkerAuth::new("gizli-jeton", 1000);
        assert_eq!(a.authorize("gizli-jeton", 999), Ok(()));
        assert_eq!(
            a.authorize("yanlis", 999),
            Err(TokenReject::Invalid)
        );
        assert_eq!(
            a.authorize("gizli-jeton", 1000),
            Err(TokenReject::Expired)
        );
    }

    #[test]
    fn heartbeat_15sn_gecikince_overdue() {
        let mut h = Heartbeat::new(0);
        assert!(!h.overdue(14));
        assert!(h.overdue(15));
        h.beat(20);
        assert!(!h.overdue(34));
        assert!(h.overdue(35));
    }

    #[test]
    fn tek_gpu_ikinci_kilit_beklemez_reddedilir() {
        let g = GpuMutex::new();
        let guard = g.try_lock().expect("ilk kilit alinir");
        assert!(g.is_busy());
        assert!(g.try_lock().is_none());
        drop(guard);
        assert!(!g.is_busy());
        assert!(g.try_lock().is_some());
    }

    #[test]
    fn gercek_sure_olcumu_ornek_bolü_oran() {
        assert!((measure_secs(16_000, 16_000) - 1.0).abs() < 1e-9);
        assert!((measure_secs(80_000, 16_000) - 5.0).abs() < 1e-9);
        assert_eq!(measure_secs(100, 0), 0.0);
    }

    #[test]
    fn mutabakatta_isci_kazanir_sapmada_uyari() {
        let r = reconcile(10.0, 10.4, RECONCILE_TOLERANCE_SECS);
        assert_eq!(r.secs, 10.0);
        assert!(!r.worker_wins_warning);
        let r2 = reconcile(10.0, 12.5, RECONCILE_TOLERANCE_SECS);
        assert_eq!(r2.secs, 10.0);
        assert!(r2.worker_wins_warning);
    }

    #[test]
    fn sahte_arka_uc_transkript_doner() {
        let pcm = vec![0i16; 32_000];
        let t = MockBackend.run(&pcm, 16_000);
        assert!(!t.trim().is_empty());
    }

    #[test]
    fn wav_basligi_gecerli_riff_mono16() {
        let wav = encode_wav_mono16(&[0i16, 1, -1, 32767], 16_000);
        assert_eq!(wav.len(), 44 + 8);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 8);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
    }

    #[test]
    fn multipart_dosya_ve_metin_alanlari_tasir() {
        let body = multipart_body("SNR123", b"SESWAV");
        let s = String::from_utf8_lossy(&body);
        assert!(s.contains("--SNR123"));
        assert!(s.contains("name=\"file\"; filename=\"audio.wav\""));
        assert!(s.contains("name=\"response_format\""));
        assert!(s.contains("\r\ntext\r\n"));
        assert!(s.contains("name=\"language\""));
        assert!(body.windows(6).any(|w| w == b"SESWAV"));
        assert!(s.ends_with("--SNR123--\r\n"));
    }

    #[test]
    fn yanit_ayirma_200_ve_hata() {
        let ok = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nmerhaba".to_vec();
        assert_eq!(
            split_response(&ok),
            Some((200, b"merhaba".to_vec()))
        );
        let bad = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n".to_vec();
        assert_eq!(split_response(&bad), Some((400, Vec::new())));
        assert_eq!(split_response(b"corrupt"), None);
    }

    #[test]
    fn arka_uc_secimi_varsayilan_sahte_wl_cevresel() {
        std::env::remove_var("EV_BACKEND");
        assert!(matches!(backend(), Backend::Mock(_)));
        std::env::set_var("EV_BACKEND", "wl");
        let b = backend();
        assert!(matches!(b, Backend::Wl(_)));
        if let Backend::Wl(w) = b {
            assert_eq!(w.addr, "127.0.0.1:8888");
        }
        std::env::remove_var("EV_BACKEND");
    }

    #[test]
    fn wl_bos_pcmde_ucretsiz_bos_doner() {
        let b = WlBackend::local();
        assert_eq!(b.run(&[], 16_000), "");
        assert_eq!(b.run(&[1, 2], 0), "");
    }
}
