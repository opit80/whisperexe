# bench/

- `dataset.md` — referans ses seti tanımı (dosyalar `server\bench\` altında,
  kopya YOK) + metrikler (RTF, Türkçe WER + terim isabeti) + tek komut.
- `bench.py` — tekrarlanabilir bench komutu (stdlib only): `run` (set varlık +
  16kHz mono kilit kontrolü), `wer` (doğruluk), `rtf` (hesap). Inference
  backend'i F1b/F3'te bağlanır.
- `sonuclar.md` — sonuç tablosu iskeleti (2 tekrar zorunlu).
