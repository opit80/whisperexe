//! Yönlendirme tablosu: heartbeat + failover + şüpheli-rota (PLAN.md §3, §5).
//!
//! - Offline için N kaçırma (varsayılan 3×15sn), online için M iyi (varsayılan 2).
//!   Tek aksayan heartbeat hat değiştirmez.
//! - Failover yalnızca atama anında: X=3sn timeout → AYNI istek ID ile diğer
//!   hatta bir kez deneme. Inference başlayınca hat ASLA değişmez.
//! - Şüpheli-rota: ilk başarısız istekte rota şüpheli sayılır; heartbeat
//!   düzelene kadar sonraki istekler doğrudan diğer hatta gider (45sn pencere).
//! - Her F9 bağımsız istektir; oturum kavramı yoktur (durum istek-içidir).

use std::collections::HashMap;

/// Hat kimliği.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Lane {
    Home,
    Fallback,
}

impl Lane {
    pub fn other(self) -> Lane {
        match self {
            Lane::Home => Lane::Fallback,
            Lane::Fallback => Lane::Home,
        }
    }
}

/// Yönlendirme sabitleri (PLAN.md §3 "Yönlendirme kararlılığı" + §5).
#[derive(Clone, Copy, Debug)]
pub struct RouteConfig {
    /// N: offline sayılması için üst üste kaçırılan heartbeat.
    pub offline_misses: u32,
    /// M: online sayılması için gereken üst üste iyi heartbeat.
    pub online_hits: u32,
    /// X: atama-anı bağlantı zaman aşımı (sn). Kullanıcı görünür gecikme
    /// arızada en fazla ~3sn'dir.
    pub assign_timeout_secs: u64,
    /// Şüpheli-rota penceresi (sn).
    pub suspect_secs: u64,
}

impl Default for RouteConfig {
    fn default() -> Self {
        Self {
            offline_misses: 3,
            online_hits: 2,
            assign_timeout_secs: 3,
            suspect_secs: 45,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct LaneState {
    hits: u32,
    misses: u32,
    online: bool,
}

impl LaneState {
    fn fresh() -> Self {
        // Başlangıç: hat çevrimiçi varsayılır, N kaçırma görülmeden değişmez.
        Self {
            hits: 0,
            misses: 0,
            online: true,
        }
    }

    fn beat(&mut self, ok: bool, cfg: &RouteConfig) {
        if ok {
            self.hits = self.hits.saturating_add(1);
            self.misses = 0;
            if self.hits >= cfg.online_hits {
                self.online = true;
            }
        } else {
            self.misses = self.misses.saturating_add(1);
            self.hits = 0;
            if self.misses >= cfg.offline_misses {
                self.online = false;
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RouteError {
    /// Inference başladıktan sonra hat değişimi yasak.
    LaneLocked,
}

/// İstek-içi atama kaydı. Her F9 bağımsız istektir; bu yapı oturum değildir,
///
/// sadece tek isteğin hangi hatta atandığını + inference kilidini taşır.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Assignment {
    pub req_id: u64,
    pub lane: Lane,
    pub inference_started: bool,
}

/// Heartbeat'e göre normal-durum rotası + şüpheli-rota + atama-anı failover.
///
/// Zaman `now_secs` (u64 saniye) olarak dışarıdan verilir: deterministik,
/// test edilebilir, duvar-saatine bağlı değil.
pub struct RouteTable {
    cfg: RouteConfig,
    lanes: HashMap<Lane, LaneState>,
    suspect_failed: Option<Lane>,
    suspect_until: u64,
}

impl RouteTable {
    pub fn new(cfg: RouteConfig) -> Self {
        let mut lanes = HashMap::with_capacity(2);
        lanes.insert(Lane::Home, LaneState::fresh());
        lanes.insert(Lane::Fallback, LaneState::fresh());
        Self {
            cfg,
            lanes,
            suspect_failed: None,
            suspect_until: 0,
        }
    }

    pub fn config(&self) -> RouteConfig {
        self.cfg
    }

    /// 15sn periyotlu işçi heartbeat'ini işler (`ok=false` = kaçırma).
    pub fn heartbeat(&mut self, lane: Lane, ok: bool, now_secs: u64) {
        if let Some(st) = self.lanes.get_mut(&lane) {
            st.beat(ok, &self.cfg);
        }
        // Heartbeat düzelince şüphe kalkar: başarısız hat tekrar iyi
        // sinyal verirse normal-durum rotasına dönülür.
        if ok && self.suspect_failed == Some(lane) {
            self.suspect_failed = None;
            self.suspect_until = 0;
        }
        let _ = now_secs;
    }

    pub fn is_online(&self, lane: Lane) -> bool {
        self.lanes.get(&lane).map(|s| s.online).unwrap_or(false)
    }

    /// Normal-durum rotası: ev çevrimiçiyse ev, değilse fallback.
    /// İki hat da çevrimdışıysa fallback seçilir (hızlı-başarısız, ücretsiz ret).
    fn normal_route(&self) -> Lane {
        if self.is_online(Lane::Home) {
            Lane::Home
        } else {
            Lane::Fallback
        }
    }

    pub fn suspect_active(&self, now_secs: u64) -> bool {
        self.suspect_failed.is_some() && now_secs < self.suspect_until
    }

    /// Rota seçimi: şüphe aktifken doğrudan diğer hat, yoksa heartbeat tablosu.
    pub fn pick(&self, now_secs: u64) -> Lane {
        if self.suspect_active(now_secs) {
            // Şüpheli-rota: heartbeat düzelene kadar diğer hat.
            self.suspect_failed.unwrap_or(Lane::Home).other()
        } else {
            self.normal_route()
        }
    }

    /// Atama-anı başarısızlığı: rotayı şüpheli işaretler (45sn pencere açılır).
    pub fn note_assign_failure(&mut self, failed: Lane, now_secs: u64) {
        self.suspect_failed = Some(failed);
        self.suspect_until = now_secs.saturating_add(self.cfg.suspect_secs);
    }

    /// Yeni isteği rotaya atar (kontrol sırası çağrı tarafında:
    /// idempotency → doğrulama → bakiye/bloke → kuyruk).
    pub fn assign(&self, req_id: u64, now_secs: u64) -> Assignment {
        Assignment {
            req_id,
            lane: self.pick(now_secs),
            inference_started: false,
        }
    }

    /// Atanan hatta 3sn içinde bağlanılamazsa: AYNI ID ile diğer hatta
    /// bir kez deneme. Inference başlamışsa hata döner (hat ASLA değişmez).
    /// Çağrı tarafı eski hattaki bloğu senkron çözüp yeni hatta yeniden
    /// koyar (çift ücret yok; deftere iki satır: bkz. `ledger::failover_lines`).
    pub fn assign_timeout(
        &mut self,
        a: &Assignment,
        now_secs: u64,
    ) -> Result<Assignment, RouteError> {
        if a.inference_started {
            return Err(RouteError::LaneLocked);
        }
        self.note_assign_failure(a.lane, now_secs);
        Ok(Assignment {
            req_id: a.req_id,
            lane: a.lane.other(),
            inference_started: false,
        })
    }

    /// Inference başladı: hat mühürlenir, sonrası değişim yasaktır.
    pub fn begin_inference(a: &mut Assignment) {
        a.inference_started = true;
    }

    /// Inference sonrası yeniden-rota denemesi her zaman reddedilir.
    pub fn try_reroute(a: &Assignment) -> Result<Lane, RouteError> {
        if a.inference_started {
            Err(RouteError::LaneLocked)
        } else {
            Ok(a.lane)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> RouteTable {
        RouteTable::new(RouteConfig::default())
    }

    #[test]
    fn tek_aksayan_heartbeat_hat_degistirmez() {
        let mut t = table();
        assert_eq!(t.pick(0), Lane::Home);
        t.heartbeat(Lane::Home, false, 15);
        assert!(t.is_online(Lane::Home));
        assert_eq!(t.pick(15), Lane::Home);
        t.heartbeat(Lane::Home, false, 30);
        assert!(t.is_online(Lane::Home));
        assert_eq!(t.pick(30), Lane::Home);
    }

    #[test]
    fn offline_n3_online_m2() {
        let mut t = table();
        for now in [15, 30, 45] {
            t.heartbeat(Lane::Home, false, now);
        }
        assert!(!t.is_online(Lane::Home));
        assert_eq!(t.pick(45), Lane::Fallback);
        // Tek iyi yetmez (M=2).
        t.heartbeat(Lane::Home, true, 60);
        assert!(!t.is_online(Lane::Home));
        t.heartbeat(Lane::Home, true, 75);
        assert!(t.is_online(Lane::Home));
        assert_eq!(t.pick(75), Lane::Home);
    }

    #[test]
    fn failover_ayni_id_ile_diger_hatta_bir_kez() {
        let mut t = table();
        let a = t.assign(7, 100);
        assert_eq!((a.req_id, a.lane), (7, Lane::Home));
        // 3sn timeout: aynı ID, diğer hat.
        let b = t.assign_timeout(&a, 103).expect("failover serbest");
        assert_eq!((b.req_id, b.lane), (7, Lane::Fallback));
        assert!(!b.inference_started);
        // Şüpheli-rota: sonraki istekler doğrudan diğer hatta.
        let c = t.assign(8, 104);
        assert_eq!(c.lane, Lane::Fallback);
    }

    #[test]
    fn inference_baslayinca_hat_asla_degismez() {
        let mut t = table();
        let mut a = t.assign(9, 200);
        RouteTable::begin_inference(&mut a);
        assert_eq!(t.assign_timeout(&a, 203), Err(RouteError::LaneLocked));
        assert_eq!(RouteTable::try_reroute(&a), Err(RouteError::LaneLocked));
        // Atama değişmedi.
        assert_eq!(a.lane, Lane::Home);
    }

    #[test]
    fn supheli_rota_heartbeat_duzelince_kapanir() {
        let mut t = table();
        let a = t.assign(11, 300);
        let _ = t.assign_timeout(&a, 303).unwrap();
        assert!(t.suspect_active(304));
        // Pencere dolmadan ev toparlanırsa şüphe kalkar.
        t.heartbeat(Lane::Home, true, 310);
        t.heartbeat(Lane::Home, true, 325);
        assert!(!t.suspect_active(326));
        assert_eq!(t.pick(326), Lane::Home);
    }

    #[test]
    fn supheli_pencere_45sn_sonra_kapanir() {
        let mut t = table();
        let a = t.assign(12, 400);
        let _ = t.assign_timeout(&a, 403).unwrap();
        assert!(t.suspect_active(404));
        // 45sn pencere bitince heartbeat tablosu (ev hâlâ online) döner.
        assert!(!t.suspect_active(448));
        assert_eq!(t.pick(448), Lane::Home);
    }
}
