//! Global FIFO kuyruk: ev hattı tek GPU (PLAN.md §3 "Kuyruk").
//!
//! - Hesap başına eşzamanlı 1 istek; fazlası global FIFO kuyruğa girer.
//! - Tavan derinlik + maksimum bekleme; dolunca/süre aşımında ÜCRETSİZ ret.
//! - Hesap başına kuyrukta en fazla 3 bekleyen; fazlası ücretsiz ret.
//! - Kuyruktan çıkarken bakiye YENİDEN kontrol edilir + o an bloke edilir;
//!   yetmezse inference başlamadan reddedilir, ücret yazılmaz.
//! - Vazgeçme = bloke çözülür, ücretsiz (çağrı tarafı Release satırı yazar).
//! - Tarife kabul anında dondurulur; bloke edilen sürümle kesinleşir.
//!
//! Para hareketi bu crate'te yoktur: `push` öncesi ön-kontrol (bakiye ≥
//! 1 quantum) ve `pop` sonrası yeniden-kontrol çağrı tarafında (ledger F1a)
//! yapılır; sonuçlar parametre olarak alınır.

use std::collections::{HashMap, VecDeque};

use crate::route::Lane;

/// Kuyruk sınırları.
#[derive(Clone, Copy, Debug)]
pub struct QueueConfig {
    /// Global tavan derinlik.
    pub max_depth: usize,
    /// Maksimum bekleme süresi (sn); aşan ücretsiz düşer.
    pub max_wait_secs: u64,
    /// Hesap başına en fazla bekleyen.
    pub per_account_max: usize,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            max_depth: 64,
            max_wait_secs: 300,
            per_account_max: 3,
        }
    }
}

/// Kabul anında dondurulmuş tarife bağlamıyla kuyruk kaydı.
#[derive(Clone, Debug)]
pub struct QueuedReq {
    pub req_id: u64,
    pub account: String,
    pub enqueued_at: u64,
    /// Kabul anında dondurulan tarife sürümü (bloke/settlement bu sürümle).
    pub tariff_version: u32,
    pub lane: Lane,
}

/// Ücretsiz ret nedenleri (ücret yazılmaz, deneme hakkı saklıdır).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EnqueueReject {
    /// Ön-kontrol geçemedi (bakiye < 1 quantum).
    BalanceShort,
    /// Hesap kotası dolu (3 bekleyen).
    AccountFull,
    /// Global tavan dolu.
    GlobalFull,
}

/// Kuyruk çıkış sonucu.
#[derive(Clone, Debug)]
pub enum PopOutcome {
    /// Sıradaki iş; çağrı tarafı bakiyeyi YENİDEN kontrol edip bloke eder.
    /// Yetmezse düşürülür (ücretsiz, inference başlamaz).
    Ready(QueuedReq),
    /// Maksimum beklemeyi aşmış: kuyruktan düşürüldü, ÜCRETSİZ.
    ExpiredFree(QueuedReq),
}

pub struct Queue {
    cfg: QueueConfig,
    items: VecDeque<QueuedReq>,
    per_account: HashMap<String, usize>,
}

impl Queue {
    pub fn new(cfg: QueueConfig) -> Self {
        Self {
            cfg,
            items: VecDeque::new(),
            per_account: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn account_pending(&self, account: &str) -> usize {
        self.per_account.get(account).copied().unwrap_or(0)
    }

    /// Kuyruğa giriş. `precheck_ok` = kabul-öncesi bakiye ≥ 1 quantum.
    /// Başarılıysa global FIFO sırası (1-bazlı konum) döner.
    pub fn push(
        &mut self,
        req: QueuedReq,
        precheck_ok: bool,
        _now_secs: u64,
    ) -> Result<usize, EnqueueReject> {
        if !precheck_ok {
            return Err(EnqueueReject::BalanceShort);
        }
        if self.account_pending(&req.account) >= self.cfg.per_account_max {
            return Err(EnqueueReject::AccountFull);
        }
        if self.items.len() >= self.cfg.max_depth {
            return Err(EnqueueReject::GlobalFull);
        }
        *self.per_account.entry(req.account.clone()).or_insert(0) += 1;
        self.items.push_back(req);
        Ok(self.items.len())
    }

    fn remove_account(&mut self, account: &str) {
        if let Some(n) = self.per_account.get_mut(account) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.per_account.remove(account);
            }
        }
    }

    /// Sıradaki işi çıkarır. `account_busy(account) = true` ise o hesabın
    /// kaydı atlanır (hesap başına eşzamanlı 1). Süresi dolmuş kayıt
    /// `ExpiredFree` olarak düşer (ücretsiz). FIFO: meşgul olmayan ilk kayıt.
    pub fn pop_next(
        &mut self,
        now_secs: u64,
        account_busy: &dyn Fn(&str) -> bool,
    ) -> Option<PopOutcome> {
        let idx = self
            .items
            .iter()
            .position(|q| !account_busy(&q.account))?;
        let q = self.items.remove(idx).expect("konum gecerli");
        self.remove_account(&q.account);
        if now_secs.saturating_sub(q.enqueued_at) > self.cfg.max_wait_secs {
            Some(PopOutcome::ExpiredFree(q))
        } else {
            Some(PopOutcome::Ready(q))
        }
    }

    /// Vazgeçme: kayıt düşer, bloke çağrı tarafında çözülür (ücretsiz).
    /// Dönüş: kayıt bulundu mu.
    pub fn cancel(&mut self, req_id: u64) -> bool {
        if let Some(idx) = self.items.iter().position(|q| q.req_id == req_id) {
            let q = self.items.remove(idx).expect("konum gecerli");
            self.remove_account(&q.account);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> QueueConfig {
        QueueConfig {
            max_depth: 4,
            max_wait_secs: 60,
            per_account_max: 3,
        }
    }

    fn req(id: u64, acc: &str, at: u64) -> QueuedReq {
        QueuedReq {
            req_id: id,
            account: acc.to_string(),
            enqueued_at: at,
            tariff_version: 9,
            lane: Lane::Home,
        }
    }

    fn idle(_: &str) -> bool {
        false
    }

    #[test]
    fn kuyruk_tavani_dolunca_ucretsiz_ret() {
        let mut q = Queue::new(cfg());
        for i in 0..4 {
            // Hesap kotasına takılmamak için farklı hesaplar.
            let r = req(i, &format!("acc{i}"), 0);
            assert!(q.push(r, true, 0).is_ok());
        }
        assert_eq!(q.push(req(99, "diger", 0), true, 0), Err(EnqueueReject::GlobalFull));
    }

    #[test]
    fn hesap_basina_max_3_bekleyen() {
        let mut q = Queue::new(cfg());
        for i in 0..3 {
            assert!(q.push(req(i, "ali", 0), true, 0).is_ok());
        }
        assert_eq!(q.push(req(3, "ali", 0), true, 0), Err(EnqueueReject::AccountFull));
        // Başka hesap etkilenmez.
        assert!(q.push(req(4, "veli", 0), true, 0).is_ok());
    }

    #[test]
    fn bakiye_yetmezse_kuyruga_girmez_ucretsiz() {
        let mut q = Queue::new(cfg());
        assert_eq!(
            q.push(req(1, "ali", 0), false, 0),
            Err(EnqueueReject::BalanceShort)
        );
        assert!(q.is_empty());
    }

    #[test]
    fn maksimum_bekleme_asimi_ucretsiz_duser() {
        let mut q = Queue::new(cfg());
        q.push(req(1, "ali", 0), true, 0).unwrap();
        match q.pop_next(61, &idle).expect("kayit var") {
            PopOutcome::ExpiredFree(r) => assert_eq!(r.req_id, 1),
            PopOutcome::Ready(_) => panic!("suresi dolmus kayit cikmamali"),
        }
        assert!(q.is_empty());
        assert_eq!(q.account_pending("ali"), 0);
    }

    #[test]
    fn tarife_kabul_aninda_donmus_cikar() {
        let mut q = Queue::new(cfg());
        q.push(req(5, "ali", 10), true, 10).unwrap();
        match q.pop_next(20, &idle).expect("kayit var") {
            PopOutcome::Ready(r) => {
                assert_eq!((r.req_id, r.tariff_version), (5, 9));
            }
            PopOutcome::ExpiredFree(_) => panic!("erken dustu"),
        }
    }

    #[test]
    fn mesgul_hesap_atlanir_fifo_korunur() {
        let mut q = Queue::new(cfg());
        q.push(req(1, "ali", 0), true, 0).unwrap();
        q.push(req(2, "veli", 0), true, 0).unwrap();
        let busy = |a: &str| a == "ali";
        match q.pop_next(10, &busy).expect("kayit var") {
            PopOutcome::Ready(r) => assert_eq!(r.req_id, 2),
            PopOutcome::ExpiredFree(_) => panic!("yanlis kayit"),
        }
        // Ali hâlâ kuyrukta.
        assert_eq!(q.account_pending("ali"), 1);
    }

    #[test]
    fn vazgecme_kaydi_duserer_ucretsiz() {
        let mut q = Queue::new(cfg());
        q.push(req(1, "ali", 0), true, 0).unwrap();
        assert!(q.cancel(1));
        assert!(!q.cancel(1));
        assert!(q.is_empty());
    }
}
