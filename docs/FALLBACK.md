# Fallback sağlayıcı seçimi — karar belgesi (madde 4)

> Tarih: 2026-09-21. Kapsam: MEMORY.md §3 madde 4 ("ertelenmişti; fresh-context
> test/tartışmasıyla karar verilecek"). Canlı anahtar yoktu; canlı API çağrısı
> YAPILMADI, kalite ölçümü YOK. Kod değişikliği YOK (mock gövdeler durur).

## 1. Karar

- **Varsayılan fallback: Groq `whisper-large-v3-turbo`.**
- **Alternatif: OpenAI** (mevcut kod hedefi `whisper-1`; fiyat alternatifi
  `gpt-4o-mini-transcribe` panelde sabit tarife olarak tanımlanabilir, kod ister).
- Gerekçe: upstream maliyet Groq turbo ≈ OpenAI `whisper-1`'in **9'da biri**,
  `gpt-4o-mini-transcribe`'ın **4,5'te biri** (§3'te hesap). Kısa-dikte 10sn
  minimum cezası kuruşun altında kalır (5sn istekte Groq upstream ≈ 0,54 kuruş).
  Hız tarafı da Groq lehine (belirtilen 216–228x gerçek-zaman; karar için ikincil
  gerekçe, fiyat birincil).
- **Türkçe kalite (WER): ölçülmedi.** Elde WER verisi yok; kalite iddiası YOK.
  Groq turbo "yeterince iyi" varsayılmıyor — §5 protokolüyle ölçülmeden kalite
  gerekçesiyle karar tersine çevrilmez; şikâyet/akışında OpenAI'ya geçiş §4'teki
  koşullarda yapılır.

## 2. Fiyat doğrulaması (PLAN §7 vs güncel)

PLAN §7 (2026-09) sayıları değişmedi:

| Hat | PLAN §7 | Güncel (2026-09-21) | Kaynak |
|---|---|---|---|
| Groq `whisper-large-v3-turbo` | $0.04/saat, istek başına min 10sn | $0.04/saat; "minimum of 10 seconds per segment" | https://console.groq.com/docs/model/whisper-large-v3-turbo |
| Groq `whisper-large-v3` | $0.111/saat | $0.111/saat | https://console.groq.com/docs/model/whisper-large-v3 |
| OpenAI `whisper-1` | $0.006/dk ($0.36/saat), sn yuvarlama, min yok | $0.006/dk ("Whisper ... $0.006 / minute") | https://platform.openai.com/docs/pricing (RESMİ, 2026-09-21 doğrulandı) |
| OpenAI `gpt-4o-mini-transcribe` | $0.003/dk | $0.003/dk ("gpt-4o-mini-transcribe ... $0.003 / minute") | https://platform.openai.com/docs/pricing (RESMİ, 2026-09-21 doğrulandı) |
| Yeni (PLAN'da yok, kapsam dışı): `gpt-transcribe` $0.0045/dk, `gpt-4o-transcribe` $0.006/dk | — | bilgi amaçlı (resmi sayfada aynen: $0.0045 / $0.006 per minute) | https://platform.openai.com/docs/pricing (RESMİ, 2026-09-21 doğrulandı) |

Kur: USD/TRY ≈ **48,7** (18–20 Eyl 2026 bandı 48,5–48,8).
Kaynak: https://tradingeconomics.com/turkey/currency (18 Eyl: 48,76),
https://www.ofx.com/en-us/exchange-rates/try (17 Eyl: 48,65).
Kodda kur sabiti YOK — PLAN §7 "kur günlük güncellenir" operasyon kuralıdır,
hesap aşağıda 48,7 ile tekrarlanabilir.

## 3. Kuruş/dk matematiği (kur 48,7; `py -3` ile doğrulandı)

Upstream maliyet (marjsız):

| Hat | $/dk | TL kuruş/dk |
|---|---|---|
| Groq turbo | 0,04/60 = 0,000667 | **3,25** |
| OpenAI `whisper-1` | 0,006 | **29,22** |
| OpenAI `gpt-4o-mini-transcribe` | 0,003 | **14,61** |

Panel formülü (uydurma yok — koddan): `Tariff::fallback_krs_per_min`
ev × çarpan veya sabit tutar (`crates/panel/src/tariffs.rs:37-44`); varsayılan
ev 120 kuruş/dk + 5x çarpan = **fallback 600 kuruş/dk**
(`crates/broker/src/state.rs:47-50`: `DEFAULT_HOME_KRS_PER_MIN = 120`,
`DEFAULT_FALLBACK_BP = 50000`). Upstream-maliyete endeksli otomatik formül
kodda YOK — marj, panelden elle ayarlanan çarpan/sabittir.

Tipik istekler — faturalı süre kodu uygular (`crates/provider/src/lib.rs:200-208`:
yukarı-yuvarla + hat tabanı; Groq taban 10sn, OpenAI taban 3sn):

| İstek | Faturalı sn (Groq / OpenAI) | Upstream maliyet kuruş (Groq / whisper-1 / mini) | Panel satış kuruş @5x (Groq taban / OpenAI taban) |
|---|---|---|---|
| 5sn | 10 / 5 | 0,54 / 2,44 / 1,22 | 100 / 50 |
| 15sn | 15 / 15 | 0,81 / 7,31 / 3,65 | 150 / 150 |
| 60sn | 60 / 60 | 3,25 / 29,22 / 14,61 | 600 / 600 |

Okuma: kısa diktede Groq'un 10sn cezası satışta 50 kuruş fark yaratır
(100 vs 50), ama upstream maliyeti ikisinde de 3 kuruşun altındadır; marj her
iki hatta da maliyeti kat kat örter. Maliyet kararı Groq lehine net.

Önemli bulgu (değişiklik YOK, orkestratör onayı gerekir): panel fallback
kotası vendor-agnostiktir — `quantum_secs(Line::Fallback) =
max(3, upstream_min_secs)`, varsayılan `DEFAULT_UPSTREAM_MIN_SECS = 10`
(`crates/panel/src/tariffs.rs:18,53-58`). Yani panel bugün OpenAI hattına da
10sn taban uygular; provider katmanı ise hatta göre ayırır
(`GROQ_MIN_SECS = 10.0`, `OPENAI_MIN_SECS = 0.0` → 3sn taban,
`crates/provider/src/lib.rs:26-28`). OpenAI varsayılan/alternatif tarifesi
açılırsa tablo "uygulanacak değişiklik"teki satır gerekir.

## 4. Alternatif ne zaman seçilir (OpenAI'ya geçiş koşulları)

- Türkçe WER ölçümü (§5) Groq aleyhine anlamlı fark gösterirse (eşik orkestratör
  kararı; sayı uydurulmuyor).
- Kullanıcı şikâyetleri teknik-terim/günlük-konuşma setlerinde yoğunlaşırsa.
- Groq kesinti/zam yaparsa: acil şalter + hat tavanları vendor-agnostik çalışır
  (`crates/panel/src/routing.rs:56-110` — şalter kapalı/sadece-ev/tavan
  retleri ücretsizdir); şalter "Groq'u kapat, OpenAI'ya al" değil "fallback'i
  durdur" anahtarıdır — vendor geçişi tarife/upstream-min ayarıyla yapılır.
- Geçiş panelden fiyatsal olarak hazırdır: `set_tariff` + `set_upstream_min`
  sürümü artırır, sonraki isteklere uygulanır (`crates/panel/src/tariffs.rs:102-120`).

## 5. Canlı kalite-testi protokolü (tekrarlanabilir; anahtarsız)

Kural: API anahtarı istenmez, dosyaya yazılmaz, koda gömülmez, repoya girmez.
Anahtar yalnızca ortam değişkeninden okunur (`GROQ_API_KEY` / `OPENAI_API_KEY`).

1. Bench ses seti: `bench/bench.py` `DATASET` referansları
   (`bench/bench.py:16-23`: `bench-voice.wav`, `bench-short.wav`,
   `msg1_teknik.mp3`, `msg2_github.mp3`, `msg3_gunluk.mp3`, `msg4_kisa.mp3` —
   kaynak kök `C:\projelerim\whisper\server\bench`, KOPYALAMA YOK) +
   her dosyanın referans transkripti (`*.ref.txt`, aynı dizinde, elle yazılır).
2. `bench/` altina eklenen komut (`bench/fallback_wer.py`, YAZILDI 2026-09-21;
   dry-run + anahtarsiz-cikis dogrulandi):
   her ses dosyasını Groq turbo VE OpenAI hedefine gönder → hipotez metinlerini
   `bench/out/<model>/<dosya>.hyp.txt` altına yaz → mevcut `wer()` fonksiyonuyla
   (`bench/bench.py:35-45`) WER hesapla → tablo bas (model × dosya, ort + min/max).
   İstemci anahtarı `os.environ` DIŞINDA hiçbir yerden okumaz; anahtar yoksa
   komut "anahtar yok — canlı test atlandı" deyip sıfırdan farklı KODLA ÇIKMAZ
   (CI'ı kırmaz), eksik referans dosyada `bench.py run` kuralı geçerli.
3. Kota güvenliği: toplam ses dakikası × birim fiyat önceden hesaplanır, panel
   fallback günlük tavanı test hesabında bu değere çekilir; şalter testi bitene
   kadar açık kalır.
4. Karar eşiği: WER farkı + kullanıcı-kör dinleme notu birlikte raporlanır;
   tek metriğe göre vendor değişmez. Sonuç bu dosyaya §1 altına işlenir.

## 6. Uygulanacak değişiklikler (KISMEN YAPILDI 2026-09-21 — orkestratör "1A" kararı)

1. Vendor seçimi (A şıkkı) YAPILDI: panelde hat-seçici alan
   (`FallbackVendor::{Groq, OpenAi}`, varsayılan Groq) + hat-bazlı ayrı
   fiyat/taban (`crates/panel/src/tariffs.rs`: `VendorCfg`, `set_vendor`,
   `set_vendor_price`, `set_vendor_upstream_min`; sürüm her değişimde artar,
   sonraki isteklere uygulanır). Uçlar: `GET/POST /v1/fallback/vendor`,
   `PUT /v1/fallback/tariffs`, `PUT /v1/fallback/upstream`
   (`crates/broker/src/http.rs`; `ROUTES` tablosunda).
   Anahtar kasası YAPILDI: `POST/DELETE /v1/fallback/keys` — değerler
   yalnızca bellek-içi, yanıta/audit'e/deftere/dosyaya YAZILMAZ; yanıtlarda
   yalnız VAR/YOK döner. Boşken ortam değişkenine düşülür (gerçek
   adaptörler yazılınca). Yeniden başlatmada silinir, panelden yeniden girilir.
2. OpenAI hattı açılırsa: panelden `PUT /v1/fallback/upstream`
   `{"vendor":"openai","secs":3}` (kelepçe ≥3sn). Groq varsayılanında
   mevcut `10` doğrudur, dokunulmadı. `set_upstream_min` artık hat-bazlıdır.
3. `OPENAI_MIN_SECS = 0.0` sabiti doğrudur (OpenAI minimumsuz → ev 3sn tabanı
   işler); değişiklik gerekmez.
4. Mock gövdeler (`MockHome`/`MockGroq`/`MockOpenAi`) durur; gerçek HTTP
   adaptörü bu kararın parçası değildir. Anahtarsız tesisat testi için
   `bench/fallback_wer.py --local` (mock anlamının birebir karşılığı) yazıldı.

## 7. Kanıt

- `cargo build -p provider` → exit 0 (`Finished dev profile`).
- `cargo test -p provider` → **8/8 geçti**, 0 failed (çıktı oturumda görüldü:
  `kapi_*`, `sessizlik_*`, `gecerli_ses_*`, `groq_10sn_*`, `bos_transkript_*`,
  `sure_kapisi_*`).
- Fiyat kaynak URL'leri §2 tablosunda; kur §2'de; matematik `py -3` çıktısıyla
  doğrulandı (§3).

## 8. MEMORY.md'ye işlenecek satırlar (YAZILMADI — orkestratöre liste)

- §1'e ek: "Fallback kararı: Groq turbo varsayılan, OpenAI alternatif
  (`docs/FALLBACK.md`, 2026-09-21); kod değişmedi, WER ölçülmedi."
- §3 madde 4: "erte…" satırı → "✅ karar verildi (belge-only; WER + vendor-seçim
  config'i açık)" olarak güncellenecek.
- §3'e açık iş olarak ekle: "§5 WER protokolü + panel vendor-seçim tasarımı
  (§6) orkestratör onayı bekliyor."

## 9. Uygulama notu (2026-09-21, kod-degisikligi disi isler)

- OpenAI fiyatlari RESMI kaynaktan teyit edildi
  (`platform.openai.com/docs/pricing`: Whisper $0.006/dk, mini $0.003/dk,
  `gpt-transcribe` $0.0045/dk); §2 tablosu resmi URL'lere cevrilmistir.
- `bench/fallback_wer.py` yazildi: stdlib-only, anahtar yalnizca ortamdan,
  anahtarsiz cikis 0, `--dry-run` ag'a dokunmaz (dogrulandi).
- `bench/fallback_wer.py --local` eklendi: anahtarsiz tesisat testi
  (mock anlami birebir; WER 1.000 kutulu-metin karsisinda skor dongusunu
  dogrular, kalite karsilastirmaz — dogrulandi, cikti temizlendi).
- `server/bench/bench-voice.ref.TASLAK.txt` uretildi: `sonuclar.txt` en iyi
  kosumdan (float32 beam=10, terim 31/32) birebir kopya; TEYITSIZ TASLAKTIR,
  `.ref.txt` degildir (betik otomatik kullanmaz). Kulak teyidi + yeniden
  adlandirma insandadir; ozellikle "on dort otuz" ve "mizkan/miskan" bolgeleri.
  Diger 5 dosyanin kaynagi yok — kulakla yazim insandadir.
- `set_upstream_min(3)` uygulanMADI — dogru karardir: Groq varsayilaninda
  mevcut `10` gecerlidir; degisiklik yalnizca OpenAI hatti acilirsa gerekir.
- Bekleyen (insan/orkestrator): `*.ref.txt` referanslari (kulakla yazim),
  anahtarli olcum, vendor-secim tasarimi (§6) onayi.
