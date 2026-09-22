//! whisperexe Tauri Windows kabuğu (tek EXE, kullanıcı-seviyesi kurulum).
//!
//! Bu crate Tauri'ye BAĞIMLI DEĞİLDİR: F9 → kayıt → overlay akışını
//! `client` çekirdeği + `client-ui` durum makinesi üzerinde kurar.
//! Tauri tarafı (global-shortcut kaydı, pencere, updater) yalnızca bu
//! mantığı çağırır; kablolama `tauri.conf.json` + `docs/TAURI.md`'dedir.
//! Özel indirici YOKTUR: güncelleme beslemesi broker'dan gelir.
//!
//! Tasarım checkpoint'i (interface-design / impeccable / frontend-design /
//! apple-design / design-motion-principles):
//! ```text
//! Intent:     Dikte eden insan F9'a basılı tutarken gözü klavyededir;
//!             overlay çevresel görüşe oynar: tek odak (durum noktası).
//! Hierarchy:  Kazanan durum noktasıdır (boyut/kontrast/boşluk); süre
//!             ikincil (tabular-nums), rozet/toast üçüncül.
//! Palette:    Tek vurgu #7aa2ff; anlam dışı renk yok (sessizlik=uyarı,
//!             engel=uyarı; hepsi overlay makinesinden gelir).
//! Depth:      Yarı-saydam tek hap yüzey + bulanıklaştırma; gölge yok,
//!             1px düşük-opaklıklı kenarlık.
//! Surfaces:   Koyu nötr tek yüzey; açık/koyu varyantı WebView2 temasından.
//! Typography: system-ui; süre 600 ağırlık + tabular-nums; rozet 500.
//! Spacing:    8px ızgara; hap içi 12px/16px; yoğunluk panel (12-16px).
//! Motion:     Girdi anında geri bildirim (<200ms ease-out); tekrarlı F9
//!             akışında giriş animasyonu yok; `prefers-reduced-motion`'da
//!             nabız yerine statik rozet (UiState::recording_pulse).
//! ```
//!
//! Kilitli davranışlar (PLAN §3-§5):
//! - F9 global-shortcut → [`flow::Shell`] (`client::hotkey` + `record`).
//! - Açılışta broker sürüm sorusu ([`version`], `/v1/version`): taban altı
//!   girişte engeller, zorunlu kurulum YENİDEN BAŞLATMADA uygulanır
//!   (dictation ortasında ASLA — kayıt bitmeden overlay çalınmaz).
//! - Opus encode 180sn tavan (~2MB gövde): üstü istemcide kesilir.
//! - Sessizlikte gönderme YOK + ücretsiz toast; ücret yazılmaz.
//! - Gerçek mikrofon yoksa `MockEncoder` yolu korunur (mock kayıt yolu).

pub mod flow;
#[cfg(feature = "tauri")]
pub mod commands;
#[cfg(feature = "tauri")]
pub mod net;
pub mod overlay;
pub mod session;
#[cfg(feature = "tauri")]
pub mod tauri_app;
pub mod version;

pub use flow::{BootApply, Shell};
pub use overlay::{render as render_overlay, OverlayView};
pub use version::{endpoint_url, BootGate, VersionError, VERSION_ENDPOINT};
