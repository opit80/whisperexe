# Bench — referans ses seti (tanım, KOPYA YOK)

Ses dosyaları `C:\projelerim\whisper\server\bench\` altındadır; buraya
**kopyalanmaz**, yalnız referans gösterilir. Eğitim verisi
(`server\kayitlar\serve_audio/` + `serve_log.jsonl`) bench'e DAHİL DEĞİLDİR.

## Set (2026-09 itibarıyla)

| # | Dosya (server\\bench\\) | Tür | Kullanım |
|---|------------------------|-----|----------|
| 1 | `bench-voice.wav` (~4.0MB) | 16kHz WAV (uzun) | Ana metin: RTF + terim isabeti (32 terim) |
| 2 | `bench-short.wav` (~244KB) | 16kHz WAV (kısa) | Hızlı duman testi |
| 3 | `msg1_teknik.mp3` | MP3 teknik konuşma | Format dayanımı (decode yolu) |
| 4 | `msg2_github.mp3` | MP3 | Format dayanımı |
| 5 | `msg3_gunluk.mp3` | MP3 | Format dayanımı |
| 6 | `msg4_kisa.mp3` | MP3 (kısa) | Quantum altı davranış (3sn kuralı) |

Referans transkriptler + geçmiş süreler: `server\bench\sonuclar.txt`
(2026-09-13, large; örn. beam=5 float16 ≈ 6.6–7.2sn, terim 31/32).

## Metrikler

- **RTF** = inference_süresi / ses_süresi (düşük = iyi; F3 kararı için baz).
- **Türkçe doğruluk** = referans transkripte karşı kelime-hata-oranı (WER, küçük
  harf + noktalama soyulmuş) + 32 terimlik listede isabet sayısı.
- Her ölçüm en az **2 tekrar**; tabloya t1/t2/ort yazılır.

## Tek komut

```
python bench\bench.py --run            # set tanımı + ortam kontrolü
python bench\bench.py --wer --ref <ref.txt> --hyp <hyp.txt>   # doğruluk
python bench\bench.py --help           # tüm seçenekler
```

Gerçek inference koşumu F1b/F3'te bağlanır; iskelet bugün ortam + WER
hesabını verir, sonuçlar `bench/sonuclar.md` formatında yazılır.
