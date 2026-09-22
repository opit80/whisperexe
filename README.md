# whisperexe

Rust tabanlı dikte ürünü: istemci (tek EXE) + broker (homelab merkez) + ev işçisi (mevcut GPU PC).
Ana plan ve kilitli kararlar: `PLAN.md` (§1–§8).

## Mimari (3 parça)

1. **Broker (homelab 192.168.1.80, merkez):** hesap + tek kullanımlık davet kodu,
   kısa ömürlü JWT (15dk access / 30 gün döner refresh), HWID cihaz kilidi (varsayılan
   2 slot), TL defter (append-only bloke/kesinleştir), global FIFO kuyruk, sağlayıcı
   adaptörleri, heartbeat yönlendirme tablosu. İlerde off-site VPS'e taşınabilir
   yazılır. TLS + domain planı belgede durur (istemci sistem CA'sına güvenir, VPN yok).
2. **Yönetim paneli (broker'da):** hesap açma, bakiye/limit, tarifeler (ev TL/dk,
   fallback çarpan/sabit — sonraki isteklere uygulanır), HWID sıfırlama, log/disk/arşiv,
   acil şalter, imzalı .msi yayınlama (imza anahtarı offline, panelde yalnız public key).
3. **Ev işçisi + istemci:** Ev işçisi mevcut model aynen (token kontrolü + 15sn
   heartbeat + gerçek-süre ölçümü + tek-GPU mutex eklenir). İstemci Rust tek EXE
   (Tauri önerilir), F9 bas-konuş + overlay + bakiye göstergesi.

Kilitli hatlar: hat üstü **Opus** (180sn ≈ 0.5MB), inference için **16kHz mono PCM**,
tek istek **tavan 180sn**, **minimum quantum 3sn** (fallback'ta `max(3sn, upstream
minimumu)`), metin dönmediyse **ücret yok**, fatura sunucunun ölçtüğü gerçek
saniyeye göre.

## Fazlar (özet)

- **F0 (bu iskelet):** bench + repo iskeleti + kesme planı. Sonuçlar `bench/sonuclar.md`.
- **F1a:** protokol + auth (davet, JWT, HWID) + TL defter.
- **F1b:** yönlendirme + kuyruk + ev-adaptör + sağlayıcı arayüzü (önce ev-only).
- **F1c:** panel + sürüm beslemesi.
- **F2:** Rust istemci + UI.
- **F3 (opsiyonel):** Rust inference — yalnız bench gerek gösterirse.

Kesme kuralı: broker paralı canlıya geçerken eski 8888 modem yönlendirmesi
**AYNI GÜN** kaldırılır (bkz. `CUTOVER.md`).
