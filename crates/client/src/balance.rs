//! Düşük bakiye uyarısı. PLAN §3: kalan bakiye ~10 dakikalık
//! karşılığın altına düşünce overlay + bildirim; hesap O ANKİ hattın
//! tarifesiyle yapılır (fallback'teyken ev dakikasıyla yanıltılmaz).

/// Uyarı eşiği: kaç dakikalık kullanım kaldıysa uyar.
pub const LOW_BALANCE_MINUTES: f64 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    Home,
    Fallback,
}

/// TL/dk tarifeler (panelden gelir; burada hesap girişi).
#[derive(Debug, Clone, Copy)]
pub struct Tariffs {
    pub home_per_min: f64,
    pub fallback_per_min: f64,
}

impl Tariffs {
    pub fn rate(&self, line: Line) -> f64 {
        match line {
            Line::Home => self.home_per_min,
            Line::Fallback => self.fallback_per_min,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BalanceView {
    pub balance_tl: f64,
    /// Kalan bakiyenin o anki hatla kaç dakikası kaldığı.
    pub minutes_left: f64,
    pub low: bool,
}

pub fn evaluate(balance_tl: f64, line: Line, tariffs: Tariffs) -> BalanceView {
    let rate = tariffs.rate(line);
    let minutes_left = if rate > 0.0 { balance_tl / rate } else { 0.0 };
    BalanceView {
        balance_tl,
        minutes_left,
        low: minutes_left < LOW_BALANCE_MINUTES,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tariffs() -> Tariffs {
        Tariffs {
            home_per_min: 1.0,
            fallback_per_min: 5.0,
        }
    }

    #[test]
    fn warns_below_10_minutes_on_current_line() {
        // Ev hattında 9 TL = 9dk → uyar.
        assert!(evaluate(9.0, Line::Home, tariffs()).low);
        // Aynı 9 TL fallback hattında 1.8dk → uyar (ev dakikasıyla yanıltma yok).
        let v = evaluate(9.0, Line::Fallback, tariffs());
        assert!(v.low);
        assert!((v.minutes_left - 1.8).abs() < 1e-9);
    }

    #[test]
    fn no_warning_when_covered() {
        assert!(!evaluate(10.0, Line::Home, tariffs()).low);
        assert!(!evaluate(60.0, Line::Fallback, tariffs()).low);
    }
}
