//! Tahsilat satırları: append-only defter niyetleri (PLAN.md §3).
//!
//! Gerçek defter F1a'nındır (ledger crate'i). Bu modül yalnızca F1b
//! akışlarının (failover) ürettiği satır niyetlerini tanımlar; bakiye
//! türetilmiş alandır, okuma-değiştirme-yazma burada yoktur.
//!
//! Failover kuralı (§5): ilk hattaki bloke SENKRON çözülüp ikinci hatta
//! yeniden konur → deftere İKİ satır işlenir, çift ücret yazılmaz.

use crate::route::Assignment;

/// Defter satır türü.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineKind {
    /// Ölçülen tutar senkron bloke edilir (bakiyeden düşmüş görünür).
    Block,
    /// Bloke çözülür (vazgeçme / failover-eski-hat / zaman aşımı).
    Release,
    /// Bloke içinden düşülür; tasfiye asla bloke üstüne çıkmaz.
    Settle,
    /// Admin yüklemesi (pilot: havale sonrası panelden).
    Load,
}

/// Append-only defter satırı niyeti.
#[derive(Clone, Copy, Debug)]
pub struct LedgerLine {
    pub req_id: u64,
    pub lane: crate::route::Lane,
    pub measured_secs: f64,
    pub tariff_version: u32,
    pub kind: LineKind,
}

/// Failover defter izi: [eski-hat Release, yeni-hat Block].
/// Ücret yazılmaz; yalnızca bloke taşınır.
pub fn failover_lines(
    old: &Assignment,
    new: &Assignment,
    measured_secs: f64,
    tariff_version: u32,
) -> [LedgerLine; 2] {
    debug_assert_eq!(old.req_id, new.req_id);
    debug_assert_ne!(old.lane, new.lane);
    [
        LedgerLine {
            req_id: old.req_id,
            lane: old.lane,
            measured_secs,
            tariff_version,
            kind: LineKind::Release,
        },
        LedgerLine {
            req_id: new.req_id,
            lane: new.lane,
            measured_secs,
            tariff_version,
            kind: LineKind::Block,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::{RouteConfig, RouteTable};

    #[test]
    fn failover_deftere_iki_satir_iade_artı_bloke() {
        let mut t = RouteTable::new(RouteConfig::default());
        let a = t.assign(21, 500);
        let b = t.assign_timeout(&a, 503).unwrap();
        let lines = failover_lines(&a, &b, 12.0, 3);
        assert_eq!(lines[0].kind, LineKind::Release);
        assert_eq!(lines[0].lane, a.lane);
        assert_eq!(lines[1].kind, LineKind::Block);
        assert_eq!(lines[1].lane, b.lane);
        assert_eq!((lines[0].req_id, lines[1].req_id), (21, 21));
        // Settle yok → ücret yazılmadı, yalnızca bloke taşındı.
        assert!(!lines.iter().any(|l| l.kind == LineKind::Settle));
    }
}
