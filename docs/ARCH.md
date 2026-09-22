# ARCH — mimari iskelet (kısa)

Detay PLAN.md §4–§5'tedir. Burası F1+ ajanları için dosya/hat haritasıdır.

## Hatlar

- İstemci → **broker** (tek giriş; "direkt ev" yolu YOK, relay tek yoldur).
- Broker → ev işçisi (aynı LAN, ~1ms; eve internetten inbound port YOK).
- Broker → fallback sağlayıcı (F1b'de arayüz + sahte gövde, gerçek seçim sonra).

## Kilitli sayılar

- Opus hat formatı (180sn ≈ 0.5MB) → sunucu 16kHz mono PCM'e çözer.
- Tavan 180sn/istek, minimum quantum 3sn, hesap başı eşzamanlı 1 + kuyrukta ≤3.
- Failover: atamada 3sn bağlanamazsa aynı ID ile diğer hat (çift ücret yok);
  inference başlayınca hat ASLA değişmez. Heartbeat: offline 3×15sn, online 2 iyi.
- Yetki kritik yolu (JWT + bakiye + idempotency) ≈ 1–5ms; yanıt sonrası
  kesinleştirme/log/panel asenkron.

## Crate haritası (F1+ sahipleri)

- `crates/protocol` — hat protokolü (Opus istek, ID + ses_hash, sonuç önbellek 24sa).
- `crates/whisper-auth` — davet (tek kullanımlık, 7 gün), JWT, HWID slot.
- `crates/ledger` — append-only defter (bloke/kesinleştir/iade; bakiye türetilmiş).
- `crates/broker-core` — kuyruk + yönlendirme + tarife dondurma.
- `crates/provider` — sağlayıcı arayüzü (ev + fallback).
- `crates/ev-worker` — ev işçisi adaptörü (token kontrol, heartbeat, süre ölçümü).
- `crates/panel` — yönetim paneli + sürüm beslemesi (imza: public key only).
- `crates/client` — Rust tek EXE istemci (F2).

## TLS + domain (plan)

Ucuz domain + Let's Encrypt, homelab broker üzerinde; homelab MERKEZ kalır.
Davet redeem'i yalnız TLS üzerinden (TLS yoksa yüz yüze).
