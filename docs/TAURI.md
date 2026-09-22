# TAURI — Windows kabuğu (tek EXE, kullanıcı-seviyesi kurulum)

Kabuk kodu: `crates/tauri-shell/` (`src/flow.rs` akış, `src/version.rs` açılış
kapısı, `src/overlay.rs` görünüm yükü, `ui/overlay.html` WebView2 varlığı,
`tauri.conf.json` paketleme). Güncelleme beslemesi broker'dandır; **özel
indirici yoktur** (Tauri updater + `https://BROKER/v1/feed`).

## WebView2 gereksinimi (F2 notu)

- Tauri Windows'ta **WebView2 Runtime** gerektirir. Windows 10/11'de
  genellikle **hazır kuruludur**; bir şey yapmaya gerek yoktur.
- Kurulu değilse `tauri.conf.json` → `bundle.windows.webviewInstallMode:
  downloadBootstrapper` sayesinde kurulum sırasında **bootstrapper**
  indirilir ve kurulur (çevrimiçi makine).
- **Çevrimdışı makine:** önceden `MicrosoftEdgeWebView2RuntimeInstallerX64.exe`
  kurulmalıdır (sistem geneli, bir kez). Kurulumun kendisi kullanıcı
  seviyesidir.

## İkonlar

- `crates/tauri-shell/icons/`: `icon.ico` (16–256 çoklu) + `icon.icns` +
  `32x32.png` + `128x128.png` + `128x128@2x.png` (256). Koyu zemin +
  tek vurgu `#7aa2ff` (overlay ile aynı dil). `tauri.conf.json` →
  `bundle.icon` bu seti gösterir.

## Kurulum (kullanıcı seviyesi, admin gerekmez)

- Hedef: `nsis`, `installMode: currentUser` (PLAN §4: kullanıcı-seviyesi
  kurulum). `%LOCALAPPDATA%` altına kurulur, UAC istemez.
- Dağıtım: imzalı `.msi`/`.nsis.exe` panele yüklenir (sürüm + değişiklik
  notu + zorunlu bayrağı + taban). İmzalama anahtarı offline'dadır;
  istemcide/panelde yalnızca public key durur.

## F9 + overlay

- F9 global-shortcut (`tauri` özelliği + `tauri-plugin-global-shortcut`):
  basıldı → `Shell::hotkey_down()`, bırakıldı → `Shell::hotkey_up()`;
  her değişimde `overlay::render` çıktısı `overlay` penceresine
  `emit("overlay", view)` edilir, pencere görünürlüğü senkronlanır
  (görünür = durum ≠ hidden VEYA toast VEYA rozet var; açılış `hidden` =
  normal). Gerçek mikrofon yoksa mock kayıt yolu korunur (`MockEncoder`;
  donanım `Encoder` arayüzüne bağlanır). Kablo: `src/tauri_app.rs`
  (yalnızca `tauri` özelliğiyle derlenir; kapalıyken saf mantık aynen test edilir).
- Overlay durum makinesi `client-ui`dir: kayıt / sending / queued / done,
  bakiye rozeti (~10dk altı, **o anki hat tarifesiyle**), toast,
  `UpdatePending` / `UpdateBlocked`. Kayıt ortasında overlay çalınmaz.
- Görünüm yükü `overlay::render()` ile `overlay` penceresine
  `emit("overlay", view)` edilir; `ui/overlay.html` + `ui/overlay.js` çizer
  (tek odak, tek vurgu `#7aa2ff`, tabular-nums, <200ms, reduced-motion'da
  statik rozet). JS harici dosyadadır (CSP `script-src 'self'` inline'a
  izin vermez) ve `app.withGlobalTauri` üzerinden `listen("overlay")`
  ile bağlanır; `window.applyState(view)` tarayıcı önizlemede aynen çalışır.

## Derleme-zamanı broker ayarı (host/pubkey hardcode YOK)

- `tauri.conf.json` içindeki `plugins.updater.endpoints` +
  `plugins.updater.pubkey` **placeholder** durur (`__TAURI_BROKER_HOST__`,
  `__TAURI_UPDATER_PUBKEY__`). Gerçek değerler `tauri` derlemesinde
  ortamdan doldurulur; yoksa/geçersizse derleme açık hatayla durur:
  - `TAURI_BROKER_HOST=https://broker.ornek.test` (taban URL; feed ucuna
    `/v1/feed` eklenir; açılış sürüm sorusu `WHISPER_BROKER_BASE` ile gömülür)
  - `TAURI_UPDATER_PUBKEY=<offline anahtarın PUBLIC karşılığı>`
- Mekanizma: `crates/tauri-shell/build.rs` (doğrula → `tauri.conf.json`'u
  işle → `tauri_build::build()`). Sürüm alanı paket sürümünden yazılır.
- **Özel anahtar ASLA dosyaya/ortama/repoya girmez.** Release imzalama,
  anahtarı elinde tutanın makinesinde `TAURI_SIGNING_PRIVATE_KEY` ile
  `cargo tauri build` sırasında yapılır.

## Güncelleme politikası

- Açılışta broker'a sürüm sorulur (`GET /v1/version`; `version` +
  `min_supported` + imza alanları `client::updater::Manifest` şeklindedir).
- **Taban altı:** girişte engellenir (`UpdateBlocked`); sürmekte olan iş
  bitirilir, yeni F9 açılmaz.
- **Zorunlu yenilik:** bayrak kurulur, **yeniden başlatmada** uygulanır;
  dictation ortasında ASLA (kayıt/sending/queued sırasında overlay'e
  dokunulmaz).
- Geri dönüş = önceki sürümü yeniden yayınlama; taban düşüşü de imzalı
  manifestle gelir (taban sunucunun bildirdiğidir, deadlock olmaz).

## Ses sınırları

- Hat formatı Opus, tek istek tavanı **180sn (~2MB gövde)**; üstü istemcide
  kesilir (`Capped180` + bilgi toast'ı).
- Konuşma öncesi sessizlik: **gönderilmez, ücret yazılmaz** (ücretsiz toast).
  Sessizlik ücretli hatta gitmez.

## Tauri kablolama taslağı (as-built: `src/tauri_app.rs`, `tauri` özelliği)

```rust
// F9: tauri-plugin-global-shortcut ile "F9" kısayolu ->
//   basıldı:  shell.hotkey_down() + emit_overlay(app)
//   bırakıldı: shell.hotkey_up()   + emit_overlay(app)
// Açılış: broker GET /v1/version -> shell.boot(VERSION, json) (5sn tavan,
//   fail-open + toast; taban altı = BlockedBelowFloor, zorunlu = PendingRestart)
// Overlay: overlay::render(shell.ui()) -> pencereye emit("overlay", view)
//   (+ show/hide senkronu; toast/rozet Hidden üstünde de görünür).
// Updater: endpoints ["{TAURI_BROKER_HOST}/v1/feed"], pubkey build.rs'ten.
//   Özel indirici YAZILMAZ; zorunlu kurulum YALNIZCA boşta uygulanır
//   (kayıt/in-flight yokken check -> download_and_install -> restart).
// Bakiye: broker yanıtındaki bakiye -> shell.balance(view) (~10dk rozeti).
```

## Doğrulama

```powershell
cargo build -p tauri-shell
cargo test -p tauri-shell
# Gercek kabuk (F9 + updater kablosu derlenir; env ZORUNLU):
$env:TAURI_BROKER_HOST='https://broker.ornek.test'
$env:TAURI_UPDATER_PUBKEY='<public key>'
cargo check -p tauri-shell --features tauri
cargo test -p tauri-shell --features tauri
# Paket: crates/tauri-shell dizininde `cargo tauri build`
# (NSIS toolchain + TAURI_SIGNING_PRIVATE_KEY release makinesinde gerekir)
```

Kapsanan testler: overlay press→result akışı, sessizlikte göndermeme
(ücretsiz toast), taban-altı-bloke (girişte engel + işi bitirme), zorunlu
güncellemenin kaydı bölmemesi, 180sn tavan kesmesi, görünüm yükü
(reduced-motion dahil).
