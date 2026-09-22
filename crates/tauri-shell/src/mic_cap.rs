//! Gercek mikrofon yakalama (cpal; Windows'ta WASAPI).
//!
//! Kısayol basılıyken secili/varsayılan giristen ses toplanir, birakinca
//! 16kHz mono i16'ya cevrilip `Shell::push_second` dilimlerine bolunur.
//! Donanim yoksa/hata olursa baslangic basarisiz doner ve cagiran mevcut
//! sessiz-toast yoluna duser (kayit engellenmez, ucret yazilmaz).
//!
//! Kalicilik: app-data `mic.json` (`{"device": "<ad>"}` ya da
//! `{"device": null}` = sistem varsayilani).

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Kalici mikrofon dosyasi adi (app-data dizininde).
pub const MIC_FILE_NAME: &str = "mic.json";

/// Kayitli cihazi oku (`None` = sistem varsayilani; yoksa/bozuksa `None`).
pub fn load_from_file(path: &std::path::Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    match v.get("device") {
        None => None,
        Some(serde_json::Value::Null) => None,
        Some(n) => n.as_str().map(|s| s.to_string()),
    }
}

/// Secimi yaz (`None` = varsayilana don).
pub fn save_to_file(path: &std::path::Path, device: Option<&str>) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("dizin-hatasi:{e}"))?;
    }
    let text = serde_json::to_string(&serde_json::json!({"device": device}))
        .map_err(|_| "kayit-hatasi".to_string())?;
    std::fs::write(path, text).map_err(|e| format!("yazma-hatasi:{e}"))?;
    Ok(())
}

/// Islenmemis yakalama dilimi (ornekleme hizi/kanal cihazdan gelir).
#[derive(Debug, Clone)]
pub struct RawChunk {
    pub samples: Vec<f32>,
    pub channels: u16,
    pub rate: u32,
}

/// `cpal::Stream` `!Send` oldugu icin akis, sahibi isci izleginde yasar;
/// komutlar kanal ile verilir (gonderen `Send+Sync` oldugundan `AppState`
/// icinde tasinabilir).
enum Cmd {
    Start {
        device: Option<String>,
        ack: std::sync::mpsc::Sender<Result<(), String>>,
    },
    Stop {
        ack: std::sync::mpsc::Sender<Vec<RawChunk>>,
    },
    /// Tamponu BOSALTMAYAN anlik goruntu (canli sayaç beslemesi icin).
    Snapshot {
        ack: std::sync::mpsc::Sender<Vec<RawChunk>>,
    },
}

struct Worker {
    tx: std::sync::mpsc::Sender<Cmd>,
}

static WORKER: std::sync::OnceLock<std::sync::Mutex<Option<Worker>>> =
    std::sync::OnceLock::new();

fn worker() -> &'static std::sync::Mutex<Option<Worker>> {
    WORKER.get_or_init(|| std::sync::Mutex::new(None))
}

fn ensure_worker() -> Result<std::sync::mpsc::Sender<Cmd>, String> {
    let mut slot = worker().lock().expect("yakalama kilidi");
    if slot.is_none() {
        let (tx, rx) = std::sync::mpsc::channel::<Cmd>();
        std::thread::spawn(move || worker_loop(rx));
        *slot = Some(Worker { tx });
    }
    Ok(slot.as_ref().expect("isci az once kuruldu").tx.clone())
}

fn worker_loop(rx: std::sync::mpsc::Receiver<Cmd>) {
    let mut stream: Option<cpal::Stream> = None;
    let mut buf: std::sync::Arc<std::sync::Mutex<Vec<RawChunk>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Start { device, ack } => {
                drop(stream.take());
                buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
                let r = build_stream(device.as_deref(), &buf).map(|s| {
                    stream = Some(s);
                });
                let _ = ack.send(r);
            }
            Cmd::Stop { ack } => {
                drop(stream.take());
                let chunks = buf.lock().expect("yakalama kilidi").clone();
                let _ = ack.send(chunks);
            }
            Cmd::Snapshot { ack } => {
                // Akis SURER, tampon korunur: yalnizca kopya doner.
                let chunks = buf.lock().expect("yakalama kilidi").clone();
                let _ = ack.send(chunks);
            }
        }
    }
}

fn norm_i16(v: i16) -> f32 {
    (v as f32 / 32768.0).clamp(-1.0, 1.0)
}

fn norm_u16(v: u16) -> f32 {
    (v as f32 / 65535.0 * 2.0 - 1.0).clamp(-1.0, 1.0)
}

/// Cok kanalli ham ornekleri tek kanala indir (kanal ortalamasi).
pub fn to_mono(samples: &[f32], channels: u16) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    if ch == 1 {
        return samples.to_vec();
    }
    samples
        .chunks(ch)
        .map(|f| f.iter().sum::<f32>() / f.len() as f32)
        .collect()
}

/// Dogrusal enterpolasyonla ornekleme hizi cevir (mono).
pub fn resample_linear(mono: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if mono.is_empty() || from_rate == 0 || to_rate == 0 {
        return Vec::new();
    }
    if from_rate == to_rate {
        return mono.to_vec();
    }
    let out_len = ((mono.len() as u64 * to_rate as u64) / from_rate as u64) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 * from_rate as f64 / to_rate as f64;
        let i0 = pos.floor() as usize;
        let frac = (pos - i0 as f64) as f32;
        let a = mono.get(i0).copied().unwrap_or(0.0);
        let b = mono.get(i0 + 1).copied().unwrap_or(a);
        out.push(a + (b - a) * frac);
    }
    out
}

/// Tum dilimleri 16kHz mono i16 tek akisa indirger (kayit beslemesi).
pub fn to_mono16(chunks: &[RawChunk]) -> Vec<i16> {
    let mut native: Vec<f32> = Vec::new();
    let mut rate = 0u32;
    for c in chunks {
        if c.samples.is_empty() || c.rate == 0 {
            continue;
        }
        if rate == 0 {
            rate = c.rate;
        }
        if c.rate != rate {
            continue; // Akis ortasi bicim degisimi: parcayi at (nadir).
        }
        native.extend_from_slice(&to_mono(&c.samples, c.channels));
    }
    resample_linear(&native, rate, 16_000)
        .into_iter()
        .map(|v| (v.clamp(-1.0, 1.0) * 32767.0) as i16)
        .collect()
}

/// Giris cihaz adlari (yakalama icin secilebilir kume).
pub fn input_devices() -> Vec<String> {
    let Ok(host) = std::panic::catch_unwind(cpal::default_host) else {
        return Vec::new();
    };
    let Ok(devs) = host.input_devices() else {
        return Vec::new();
    };
    devs
        .filter_map(|d| d.name().ok())
        .collect()
}

/// Varsayilan giris adi (yoksa `None`).
pub fn default_device_name() -> Option<String> {
    let host = std::panic::catch_unwind(cpal::default_host).ok()?;
    host.default_input_device()?.name().ok()
}

/// Yakalamayi baslat (secili ad ya da varsayilan). Hata = sessiz yol.
pub fn capture_start(device_name: Option<&str>) -> Result<(), String> {
    let tx = ensure_worker()?;
    let (ack_tx, ack_rx) = std::sync::mpsc::channel();
    tx.send(Cmd::Start {
        device: device_name.map(|s| s.to_string()),
        ack: ack_tx,
    })
    .map_err(|_| "yakalama-iscisi-durdu".to_string())?;
    ack_rx.recv().map_err(|_| "yakalama-iscisi-durdu".to_string())?
}

/// Akisi durdurup 16kHz mono i16 akisi dondur (bos olabilir).
pub fn capture_stop() -> Vec<i16> {
    let chunks = (|| -> Option<Vec<RawChunk>> {
        let tx = ensure_worker().ok()?;
        let (ack_tx, ack_rx) = std::sync::mpsc::channel();
        tx.send(Cmd::Stop { ack: ack_tx }).ok()?;
        ack_rx.recv().ok()
    })()
    .unwrap_or_default();
    to_mono16(&chunks)
}

/// Akisi SURDURMEDEN o ana dek birikenin 16kHz mono kopyasi (canli sayac).
/// Donanim yoksa/isci durduysa bos doner (cagiran sayaci oynatmaz).
pub fn capture_snapshot() -> Vec<i16> {
    let chunks = (|| -> Option<Vec<RawChunk>> {
        let tx = ensure_worker().ok()?;
        let (ack_tx, ack_rx) = std::sync::mpsc::channel();
        tx.send(Cmd::Snapshot { ack: ack_tx }).ok()?;
        ack_rx.recv().ok()
    })()
    .unwrap_or_default();
    to_mono16(&chunks)
}

fn build_stream(
    device_name: Option<&str>,
    buf: &std::sync::Arc<std::sync::Mutex<Vec<RawChunk>>>,
) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = match device_name {
        Some(n) if !n.trim().is_empty() => host
            .input_devices()
            .map_err(|e| format!("cihaz-liste-hatasi:{e}"))?
            .find(|d| d.name().map(|x| x == n).unwrap_or(false))
            .ok_or_else(|| "mikrofon-bulunamadi".to_string())?,
        _ => host
            .default_input_device()
            .ok_or_else(|| "mikrofon-yok".to_string())?,
    };
    let cfg = device
        .default_input_config()
        .map_err(|e| format!("mikrofon-acilamadi:{e}"))?;
    let channels = cfg.channels();
    let rate = cfg.sample_rate();
    let buf_cb = buf.clone();
    let err_fn = |e| eprintln!("mikrofon akis hatasi: {e}");
    let stream = match cfg.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &cfg.config(),
            move |data: &[f32], _| {
                buf_cb.lock().expect("yakalama kilidi").push(RawChunk {
                    samples: data.to_vec(),
                    channels,
                    rate: rate.0,
                });
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &cfg.config(),
            move |data: &[i16], _| {
                buf_cb.lock().expect("yakalama kilidi").push(RawChunk {
                    samples: data.iter().map(|&v| norm_i16(v)).collect(),
                    channels,
                    rate: rate.0,
                });
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &cfg.config(),
            move |data: &[u16], _| {
                buf_cb.lock().expect("yakalama kilidi").push(RawChunk {
                    samples: data.iter().map(|&v| norm_u16(v)).collect(),
                    channels,
                    rate: rate.0,
                });
            },
            err_fn,
            None,
        ),
        f => return Err(format!("ornek-bicimi-desteklenmiyor:{f:?}")),
    }
    .map_err(|e| format!("mikrofon-acilamadi:{e}"))?;
    stream.play().map_err(|e| format!("mikrofon-acilamadi:{e}"))?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_averages_channels() {
        assert_eq!(to_mono(&[1.0, -1.0, 0.5, 0.5], 2), vec![0.0, 0.5]);
        assert_eq!(to_mono(&[0.25], 1), vec![0.25]);
    }

    #[test]
    fn resample_keeps_duration() {
        // 1sn 48k -> 1sn 16k.
        let mono = vec![0.5f32; 48_000];
        let out = resample_linear(&mono, 48_000, 16_000);
        assert_eq!(out.len(), 16_000);
        assert!(out.iter().all(|&v| (v - 0.5).abs() < 1e-6));
    }

    #[test]
    fn end_to_end_chunks_to_16k() {
        // 2sn stereo 44.1k -> 2sn 16k mono i16.
        let stereo: Vec<f32> = (0..44_100 * 2).map(|i| if i % 2 == 0 { 0.5 } else { -0.5 }).collect();
        let chunks = vec![RawChunk { samples: stereo, channels: 2, rate: 44_100 }];
        let pcm = to_mono16(&chunks);
        assert_eq!(pcm.len(), 32_000);
        assert!(pcm.iter().all(|&v| v == 0)); // Kanal ortalamasi sifir.
    }

    #[test]
    fn mic_file_roundtrip() {
        let dir = std::env::temp_dir().join("whisperexe-test-mic");
        let path = dir.join("mic.json");
        let _ = std::fs::remove_file(&path);
        assert_eq!(load_from_file(&path), None);
        save_to_file(&path, Some("Dis Mikrofon")).expect("kayit");
        assert_eq!(load_from_file(&path), Some("Dis Mikrofon".into()));
        save_to_file(&path, None).expect("sifirla");
        assert_eq!(load_from_file(&path), None);
        let _ = std::fs::remove_file(&path);
    }
}
