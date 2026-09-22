//! Broker HTTP sunucusu (G1): LAN aşaması, TLS YOK, ev-only, mock upstream.
//!
//! Kullanım: `broker [adres]` (varsayılan `127.0.0.1:8899`)
//! Ortam: `BROKER_SECRET` (imza sırrı; yoksa geliştirme sırrı + uyarı),
//! `BROKER_LEDGER_PATH` (varsayılan `ledger.jsonl`).

mod http;
mod state;

use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use http::Server;
use state::BrokerState;

fn main() {
    let addr = std::env::args().nth(1).unwrap_or_else(|| {
        std::env::var("BROKER_ADDR").unwrap_or_else(|_| "127.0.0.1:8899".to_string())
    });
    let secret = match std::env::var("BROKER_SECRET") {
        Ok(s) if !s.is_empty() => s.into_bytes(),
        _ => {
            eprintln!("broker UYARI: BROKER_SECRET yok; gelistirme sirri kullaniliyor (uretiminde ayarlayin)");
            b"whisperexe-dev-secret-degistir".to_vec()
        }
    };
    let ledger_path =
        std::env::var("BROKER_LEDGER_PATH").unwrap_or_else(|_| "ledger.jsonl".to_string());
    let listener = TcpListener::bind(&addr).unwrap_or_else(|e| {
        eprintln!("broker: {addr} dinlenemedi: {e}");
        std::process::exit(1);
    });
    println!("broker dinliyor: {addr} (LAN, TLS yok, ev-only mock) ledger={ledger_path}");
    let state = Arc::new(Mutex::new(BrokerState::new(&secret, ledger_path)));
    Server::new(state).serve_forever(listener);
}
