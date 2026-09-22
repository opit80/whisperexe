//! broker-core: yonlendirme + kuyruk + tahsilat-satirlari (F1b).
//!
//! PLAN.md §3 (kuyruk, yonlendirme, tahsilat) + §5 (gecikme + yonlendirme).
//! Kritik yol kontrolleri (JWT + bakiye + idempotency) O(1) tutulur, hedef 1-5ms:
//! bu crate'te heap tahsisi kritik yolda yapilmaz, defter/log/panel yazimi
//! cagri tarafinda yanit sonrasina birakilir (asenkron).

pub mod ledger;
pub mod queue;
pub mod route;

pub use ledger::{failover_lines, LedgerLine, LineKind};
pub use queue::{EnqueueReject, PopOutcome, Queue, QueueConfig, QueuedReq};
pub use route::{Assignment, Lane, RouteConfig, RouteError, RouteTable};
