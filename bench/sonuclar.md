# Bench sonuçları

Her koşum aşağıdaki tabloya **2 tekrarlı** işlenir. Geçmiş baz:
`server\bench\sonuclar.txt` (2026-09-13, large; float16 beam=5 ≈ 6.6–7.2sn,
terim 31/32; float32 beam=5 ≈ 8.7sn).

## Format

| tarih | backend | model | beam | ses | ses_s | t1 | t2 | ort | RTF | WER | terim |
|-------|---------|-------|------|-----|-------|----|----|-----|-----|-----|-------|
| (yok — ilk koşum F1b/F3'te) | | | | | | | | | | | |

- `terim`: 32 terimlik listede isabet (baz: 31/32).
- Boş transkript = başarısız = ücretsiz (PLAN.md §3); tabloya `BOŞ` yazılır.
- 180sn tavan / 3sn quantum dışı koşum geçersiz sayılır.
