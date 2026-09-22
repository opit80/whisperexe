//! whisperexe istemci çekirdeği (Tauri bağımsız).
//!
//! UI katmanı (`client-ui` crate'i) yalnızca bu modüllerdeki tiplere ve
//! olaylara bağlanır; Tauri sonradan `client-ui` üzerine eklenir.
//! Gerçek mikrofon donanımı yoksa `record::MockSource` kullanılır.

pub mod auth;
pub mod audio_enc;
pub mod balance;
pub mod hotkey;
pub mod hwid;
pub mod mic;
pub mod queue;
pub mod record;
pub mod request_id;
pub mod updater;
