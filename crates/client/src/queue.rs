//! Kuyruk sırası görünümü + vazgeçme. PLAN §3: hesap başına
//! kuyrukta en fazla 3 bekleyen; fazlası ücretsiz ret. Vazgeçme =
//! bloke çözülür, ücretsiz. Sıra görünümü `client-ui`ya buradan akar.

pub const MAX_PENDING_PER_ACCOUNT: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedRequest {
    pub request_id: String,
    /// Kuyruğa giriş sırası (küçük önce).
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueError {
    /// Hesap kotası dolu: ücretsiz ret.
    AccountFull,
}

impl std::fmt::Display for QueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueueError::AccountFull => {
                write!(f, "Kuyrukta 3 bekleyen istek var, yenisi alinmadi (ucret yok).")
            }
        }
    }
}

/// Hesap kuyruğu görünümü (istemci tarafı ayna).
#[derive(Debug, Default)]
pub struct AccountQueue {
    items: Vec<QueuedRequest>,
    next_seq: u64,
}

impl AccountQueue {
    /// Kuyruğa ekle; sıra numarası (1-bazlı) döner.
    pub fn enqueue(&mut self, request_id: String) -> Result<usize, QueueError> {
        if self.items.len() >= MAX_PENDING_PER_ACCOUNT {
            return Err(QueueError::AccountFull);
        }
        self.next_seq += 1;
        self.items.push(QueuedRequest {
            request_id,
            seq: self.next_seq,
        });
        Ok(self.items.len())
    }

    /// Vazgeçme: istek kuyruktan çıkar (bloke sunucuda çözülür, ücretsiz).
    /// Çıkarıldıysa true.
    pub fn cancel(&mut self, request_id: &str) -> bool {
        let before = self.items.len();
        self.items.retain(|q| q.request_id != request_id);
        self.items.len() != before
    }

    /// Overlay sırası: bekleyen sayısı + bu isteğin konumu.
    pub fn position_of(&self, request_id: &str) -> Option<usize> {
        self.items
            .iter()
            .position(|q| q.request_id == request_id)
            .map(|i| i + 1)
    }

    pub fn pending(&self) -> usize {
        self.items.len()
    }

    pub fn ids(&self) -> Vec<String> {
        self.items.iter().map(|q| q.request_id.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request_id::new_request_id;

    #[test]
    fn caps_at_three_pending() {
        let mut q = AccountQueue::default();
        for _ in 0..3 {
            q.enqueue(new_request_id()).unwrap();
        }
        assert_eq!(q.enqueue(new_request_id()), Err(QueueError::AccountFull));
    }

    #[test]
    fn cancel_frees_slot_and_positions_shift() {
        let mut q = AccountQueue::default();
        let a = new_request_id();
        let b = new_request_id();
        q.enqueue(a.clone()).unwrap();
        q.enqueue(b.clone()).unwrap();
        assert_eq!(q.position_of(&b), Some(2));
        assert!(q.cancel(&a));
        assert_eq!(q.position_of(&b), Some(1));
        assert!(!q.cancel(&a));
        // Boşalan slota yeni istek girer.
        q.enqueue(new_request_id()).unwrap();
        q.enqueue(new_request_id()).unwrap();
        assert_eq!(q.pending(), 3);
    }
}
