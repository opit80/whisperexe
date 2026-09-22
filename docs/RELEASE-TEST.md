# RELEASE-TEST — yayın öncesi yerel doğrulama (zorunlu)

Kural: release **önce localde denenir**, sonra `gh release create` yapılır.
Son istisnasız uygulanan sürüm: `v0.2.6`.

## 1. Otomatik kapı (hepsi yeşil olmalı)

```powershell
cargo test --workspace
cargo check -p tauri-shell
$env:TAURI_BROKER_HOST='https://95.70.173.199:9443'
$env:TAURI_UPDATER_PUBKEY='<public key>'
cargo check -p tauri-shell --features tauri
node --check crates/tauri-shell/ui/main.js
node --check crates/tauri-shell/ui/overlay.js
git status --porcelain   # tauri.conf.json kirlenmemeli
```

## 2. Kurulum derlemesi (üretim env ile)

```powershell
$env:PATH += ";C:\Program Files (x86)\NSIS"
$env:TAURI_BROKER_HOST='https://95.70.173.199:9443'
$env:TAURI_UPDATER_PUBKEY='<public key>'
cargo tauri build   # crates/tauri-shell dizininde
```

## 3. Duman testi (temiz makinede, sırayla)

1. Kurulum çalışır, pencere ≤15sn'de açılır (broker yoksa **hata yazar,
   donmaz**; açılış toast'ı 8sn'de söner).
2. Davetle aç / giriş yapılır → pencere kapatılıp açılınca **hesap
   hatırlanır** (giriş formu gelmez, bakiye tazelenir).
3. Access bitiminde (15dk) yeniden giriş istenmez, sessiz `user_refresh`
   olur; çıkışta hatırlama temizlenir.
4. F9 bas-konuş → overlay kayıt → bırakınca gönderim; sessizlikte
   gönderme yok + ücretsiz toast.
5. Yönetim sekmesi: davet üret, bakiye yükle (harf girince `Hata:`
   yazar, NaN gitmez).
6. Güncelle düğmesi son kurulumu indirip çalıştırır.

## 4. Yayın

```powershell
git push origin master
git tag -a vX.Y.Z -m "vX.Y.Z <kısa not>"
git push origin vX.Y.Z
gh release create vX.Y.Z target/release/bundle/nsis/whisperexe_0.1.0_x64-setup.exe `
  --title "vX.Y.Z arkadas" --notes "<1-2 cümle>"
gh release view vX.Y.Z --json tagName,name,assets --jq "{tag,name,assets:[.assets[].name]}"
```

Not: ikili sürümü `0.1.0` sabit tutulur (kurulum dosya adı değişmez);
ilerleyen tag `vX.Y.Z` release işaretidir.
