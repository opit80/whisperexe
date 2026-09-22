# client — whisperexe Windows istemci çekirdeği

Tauri bağımsız çekirdek. Tauri sonradan `client-ui` durum makinesini
sürerek bağlanır; özel indirici yazılmadı, güncelleme beslemesi
broker'dan (`updater::Manifest`) gelir.

## Modüller

- `auth`: davet kodu → şifre belirleme + HWID bağlama; access 15dk +
  refresh 30 gün dönerli (`TokenStore::rotate` eskiyi unutur);
  5 hata → 5dk kilit.
- `hwid`: Windows MachineGuid, yoksa geçici kimlik.
- `hotkey`: F9 soyutlaması (`MockHotkey` ile test).
- `mic`: seviye eşiği — altında `Silent` (gönderme, ücret yok),
  ara bant `LowWarn`.
- `record`: bas-konuş oturumu; 180sn tavanında istemcide keser
  (`StopReason::Capped180`); konuşma öncesi sessizlik reddedilir.
- `audio_enc`: Opus çerçeveleme/tavan (48kHz mono, 20ms, 180sn ≈ 0.5MB,
  gövde tavanı ~2MB). Üretim kodlayıcı Tauri derlemesinde `audiopus`
  ile `Encoder` arayüzüne bağlanır; burada `MockEncoder` tavan
  mantığını birebir uygular.
- `request_id`: tekil ID (uuid v4).
- `queue`: hesap başına en fazla 3 bekleyen; sıra görünümü + vazgeçme.
- `balance`: düşük bakiye (~10dk altı) o anki hat tarifesiyle.
- `updater`: imzalı bildiri (ed25519, imza önce) + sha256; taban altı
  girişte engellenir; zorunlu kurulum yeniden başlatmada.

## WebView2 notu (F2)

Tauri Windows'ta WebView2 gerektirir. Windows 10/11'de genellikle
hazır kuruludur; yoksa Tauri bootstrapper indirir. Kullanıcı-seviyesi
kurulumdur, admin gerekmez (PLAN §4). Çevrimdışı makinede
`MicrosoftEdgeWebView2RuntimeInstallerX64.exe` önden kurulmalıdır.
