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
/// large + CUDA) ayrı fazda yapılır; bu fazda sahte gövde kullanılır.
/// Bu trait'e dokunan kod `server/kod`'u DEĞİŞTİRMEZ, yalnızca çağırır.
pub trait InferenceBackend {
    fn run(&self, pcm_samples: &[i16], rate_hz: u32) -> String;
}

/// Sahte arka-uç (arayüz + sahte gövde; gerçek bağlama sonra).
pub struct MockBackend;

impl InferenceBackend for MockBackend {
    fn run(&self, pcm_samples: &[i16], rate_hz: u32) -> String {
        format!(
            "[isçi {:.1}sn] ornek transkript",
            measure_secs(pcm_samples.len(), rate_hz)
        )
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
}
