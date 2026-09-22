# TLS — hazırlık belgesi (BELGE-ONLY)

> Bu belge SADECE belgedir. Bu belge için hiçbir komut çalıştırılmadı;
> sertifika istenmedi/üretilmedi, anahtar üretilmedi, firewall/modem
> komutu çalıştırılmadı, canlıya dokunulmadı, broker koduna dokunulmadı.
> Redeem akışı bu belgede değiştirilmez (kod işi G1'indir).
> Kaynaklar: PLAN.md §3 (TLS + Erişim modeli) + §1 (Kesme kuralı) + §8
> (broker makinesi), CUTOVER.md, docs/ARCH.md.

## 1. İlke

- Homelab MERKEZ kalır. Makine değişmez: homelab sunucusu
  `192.168.1.80` (Windows 10 Home, PLAN.md §8) üzerinde broker çalışır.
- Domain yalnızca bu makineye isim + güvenli bağlantı verir; taşıma,
  makine değişimi, yeni sunucu yoktur.
- İstemci sistem CA'sına güvenir. VPN yoktur (PLAN.md §3).
- Eve internetten inbound port yoktur (PLAN.md §3 Erişim modeli).
  Eski direkt yol (`95.70.173.199:8888` → ev) relay açılana kadar
  aynen çalışır, relay açılınca kapatılır.

## 2. Domain + Let's Encrypt adımları (sıralı anlatım, komutsuz)

Aşağıdaki sıra uygulanacak sıradır; bu belgede komut yoktur,
uygulama kesme günü yapacak ekibe aittir.

1. Ucuz bir domain alınır (tek amaç: broker'a isim vermek).
2. Domain'in DNS A kaydı, evin sabit dış IP'sine işaret eder
   (PLAN.md §8: sabit IP var, port yönlendirme yapılabilir).
3. Broker `192.168.1.80` üzerinde kalır; makine, disk, servis düzeni
   değişmez. Broker Windows servisi olarak çalışır (NSSM/WinSW veya
   Zamanlayıcı — PLAN.md §8).
4. Let's Encrypt doğrulaması broker üzerinden yapılır (alan doğrulaması
   dışarıdan erişilebilir broker adresi üzerinden; detay yöntem seçimi
   uygulama anında belirlenir).
5. Sertifika + özel anahtar yalnızca broker makinesinde (`192.168.1.80`)
   tutulur; başka makineye kopyalanmaz.
6. Broker dışa yalnızca TLS ile dinler; düz HTTP ile API/redeem
   sunulmaz. HTTP varsa tek işi TLS'e yönlendirmektir.
7. Yenileme (Let's Encrypt kısa ömürlüdür) broker üzerinde otomatik
   yenilemeye bağlanır; bitiş tarihi panel/log ile izlenir.
8. İstemci tarafında ek kök sertifika kurulumu gerekmez; istemci
   işletim sisteminin sistem CA deposuna güvenir, VPN kurulmaz.

Yapılmayanlar (bu belgede): sertifika isteme/üretme yok, anahtar
üretme yok, modem/firewall komutu yok.

## 3. Broker LAN-only → TLS'li dış erişim geçiş sırası (checklist)

Sıra bozulmaz. Her madde bir sonrakine geçmeden teyit edilir.

- [ ] 1. Broker LAN içinde çalışıyor (hesap, davet, defter, kuyruk
       akışları LAN istemcisiyle doğrulandı).
- [ ] 2. Domain alındı, DNS A kaydı sabit dış IP'yi gösteriyor
       (yayılma beklendi).
- [ ] 3. Broker üzerinde Let's Encrypt sertifikası alındı ve broker
       TLS ile dinliyor (sertifika tarihi/geçerliliği kontrol edildi).
- [ ] 4. LAN istemcisi broker'a TLS'li domain adresi üzerinden bağlandı
       (sistem CA zinciriyle, uyarısız).
- [ ] 5. Dış ağdan tek istemciyle uçtan uca deneme: davet → login →
       F9 → transkript → defter satırı (paralı hat doğrulaması;
       CUTOVER.md Madde 1 ile aynı kapı).
- [ ] 6. Redeem yalnızca TLS üzerinden yapıldı (aşağıdaki §4 kuralı).
- [ ] 7. Kesme günü işleri CUTOVER.md sırasına göre yapıldı
       (bkz. §5; 8888 yönlendirmesi AYNI GÜN kaldırılır).

Not: LAN aşamasında broker eve doğrudan bağlanır (aynı ağ, ~1ms;
docs/ARCH.md Hatlar). Eve internetten inbound port açılmaz.

## 4. Redeem kuralı (KESİN KURAL)

- Davet redeem'i YALNIZCA TLS üzerinden yapılır.
- TLS hazır değilse redeem YÜZ YÜZE yapılır. İstisnası yoktur:
  düz HTTP üzerinden, açık ağ üzerinden veya güvensiz kanaldan
  davet kodu iletilmez/kullanılmaz.
- Kaynak: PLAN.md §1 Kesme kuralı + §3 Erişim modeli,
  docs/ARCH.md "TLS + domain (plan)".

Bu belge redeem akışını değiştirmez; akışın kod karşılığı G1'indir.

## 5. Kesme günüyle ilişki (CUTOVER.md'ye atıf, komut YOK)

- Broker paralı canlıya geçerken eski 8888 yönlendirmesi AYNI GÜN
  kaldırılır; bedava bypass kalmaz (PLAN.md §1 Kesme kuralı).
- Uygulanacak sıra CUTOVER.md'dedir; bu belge sıra/komut tekrarlamaz:
  paralı hattan uçtan uca doğrulama → istemcinin broker adresine
  geçişi → modemdeki 8888 → 192.168.1.14 kaydının silinmesi →
  dışarıdan zaman aşımı teyidi → `wl --serve`'nin LAN-only'ye
  alınması ya da durdurulması (`serve_log.jsonl` + `serve_audio/`
  SİLİNMEDEN) → kesme kaydının işlenmesi.
- Bu belgede modem/firewall komutu yoktur. Komut/ekran işlemleri
  kesme günü CUTOVER.md sahibince yapılır.

## 6. Doğrulama: belge tutarlılık kontrolü (komut çalıştırılmadı)

Komut çalıştırılmadı; yalnızca metin karşılaştırması yapıldı:

- PLAN.md §3 "TLS: ucuz domain + Let's Encrypt, homelab broker
  üzerinde; homelab MERKEZ kalır, makine değişmez; istemci sistem
  CA'sına güvenir, VPN yok" — bu belge §1–§2 ile aynıdır. Çelişki yok.
- PLAN.md §3 "Erişim modeli: LAN aşamasında broker eve doğrudan
  (aynı ağ, TLS); eve inbound port yok; eski 8888 yolu relay açılınca
  kapatılır, o güne kadar aynen çalışır" — bu belge §1 + §3 ile aynıdır.
  Çelişki yok.
- PLAN.md §1 "Kesme kuralı: AYNI GÜN kapatılır; redeem yalnızca TLS,
  TLS yoksa yüz yüze" — bu belge §4 + §5 ile aynıdır. Çelişki yok.
- CUTOVER.md "aynı gün kapatılır; adımlar kesme günü sırayla" —
  bu belge §5 yalnızca atıf yapar, adım/komut kopyalamaz. Çelişki yok.
- docs/ARCH.md "Hatlar: direkt ev yolu YOK, relay tek yoldur" +
  "TLS + domain (plan): redeem yalnız TLS (yoksa yüz yüze)" —
  bu belge §3 + §4 ile aynıdır. Çelişki yok.
- PLAN.md §8 makine bilgisi (`192.168.1.80`, Windows 10 Home,
  servis modeli, sabit IP) aynen alınmıştır; makine değişikliği
  önerilmez. Çelişki yok.

Durum: tutarlı. Sertifika/anahtar/firewall/canlı işlemi yapılmadı;
yapılacak işler kesme gününe bırakıldı.

## 7. Domainsiz seçenek: özel CA (SEÇİLDİ 2026-09-21)

Domain alınmayacağı için Let's Encrypt yolu kapalıdır. Bunun yerine:

- Broker makinesinde (192.168.1.80) özel CA üretildi:
  `C:\whisper\tls\ca.crt` (CN=whisperexe-local-CA, 10 yıl) + sunucu
  sertifikası `server.crt` (CN=whisperexe-broker, ECDSA P-256, 825 gün,
  SAN: 95.70.173.199 + 192.168.1.80, serverAuth). CA özel anahtarı
  (`ca.key`) yalnızca broker makinesindedir.
- İstemci, bu CA'yi **pin'ler** (kendi kodumuz olduğu için harici CA
  kurulumu gerekmez): yalnızca bu CA'den imzalı sertifikaya bağlanır.
  Redeem kuralı (§4) aynen geçerlidir — pin'li TLS üzerinden yapılır.
- TLS sonlandırma broker kodunda değil, önündeki kapıcıda yapılır
  (broker std-only HTTP kalır); kapıcı 443'ü dinleyip 127.0.0.1:8899'a aktarır.
- KURULDU (2026-09-21): 443 SSTP VPN'de, 8443 veilside media-proxy'de
  tutulduğu için kapıcı **9443**'tedir — stunnel 5.82 + NSSM servisi
  (`whisper-tls`, otomatik başlar), minimal yapılandırma
  (`C:\whisper\tls\stunnel.conf`; global `pid/debug/sslVersionMin`
  satırları ayıklanmıştır — OpenSSL 3.x öntanımlısı zaten TLS1.2+).
  El sıkışma LAN'dan doğrulandı: TLSv1.3 + CA zinciri + SAN (2026-09-21).
- Sertifika bitişi panel/log ile izlenir (LE otomasyonu yoktur; yenileme
  aynı prosedürle elle yapılır, bitişten önce).
- Kalan işler: kapıcı kurulumu → broker deploy → modem 443 yönlendirmesi →
  istemci pin'leme → kesme günü (CUTOVER.md sırası aynen geçerlidir).
- BROKER DEPLOY (2026-09-21): release `broker.exe` (741KB, unsuspend +
  vendor/anahtar uçlu) `C:\whisper\bin\`'de, NSSM servisi `whisper-broker`
  (otomatik başlar), sır `C:\whisper\etc\broker-secret` (admin+SYSTEM only),
  defter `C:\whisper\data\ledger.jsonl`. Uçtan uca LAN doğrulandı:
  TLSv1.3 + CA + `GET /v1/version` → ok. Admin şifresi KURULDU (sahip
  belirledi; değer hiçbir dosyaya yazılmadı, giriş 200 doğrulandı).
  Dış erişim de doğrulandı (modem düzeltmesi sonrası 95.70.173.199:9443 → ok).
- İSTEMCİ KARARI (2026-09-21): kilit tanıtma (pin'leme) SAHİP İSTEMEDİ.
  Sonuç: yeni şifreli yol hazır ama boşta bekliyor; arkadaş eski 8888'den
  devam ediyor; kesme günü ERTELENDİ (eski kapı açık kalıyor).
  Alternatif (ileride): arkadaşa CA kurulumu + istemcide küçücük https
  geçişi — pin kodu olmadan, tek seferlik elle kurulumla.
  DIŞ ERİŞİM (2026-09-21): modem kaydı düzeltildi, dış test GEÇTİ —
  internet → 95.70.173.199:9443 → stunnel TLSv1.3 (CA doğrulamalı) →
  broker → `GET /v1/version` ok. Üretim yolu LAN+dışarıda açık.
