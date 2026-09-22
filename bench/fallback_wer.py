"""Fallback vendor WER karsilastirmasi (docs/FALLBACK.md §5 protokolu).

KURALLAR (kilitli):
- API anahtari YALNIZCA ortam degiskeninden okunur (GROQ_API_KEY /
  OPENAI_API_KEY). Dosyadan, argumandan, koddan okunmaz; hicbir yere
  yazilmaz, repoya girmez, loga basilMAZ.
- Ses dosyalari KOPYALANMAZ; server\\bench referanslari okunarak gonderilir.
- Anahtar yoksa "anahtar yok — canli test atlandi" yazip 0 ile cikar (CI kirmaz).
- Referans transkript (*.ref.txt) yoksa o dosya skorsuz gecilir (hipotez yine
  kaydedilir); karar verilmez, SADECE tablo basilir (vendor degisimi insandadir).
- Standart kutuphane disinda bagimlilik YOK.

Kullanim:
    py -3 bench/fallback_wer.py --dry-run        # ag yok: plan + maliyet tablosu
    py -3 bench/fallback_wer.py                  # anahtar varsa olc, yoksa atla
"""

import argparse
import http.client
import io
import json
import mimetypes
import os
import sys
import urllib.parse

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bench import DATASET, SERVER_BENCH, wer, wav_seconds  # noqa: E402

GROQ_URL = "https://api.groq.com/openai/v1/audio/transcriptions"
GROQ_MODEL = "whisper-large-v3-turbo"
OPENAI_URL = "https://api.openai.com/v1/audio/transcriptions"
DEFAULT_OPENAI_MODEL = "whisper-1"
TIMEOUT_S = 180

# Birim fiyat ($/dk) — docs/FALLBACK.md §2'deki teyitli degerler.
PRICE_USD_PER_MIN = {
    "groq-turbo": 0.04 / 60.0,
    "whisper-1": 0.006,
    "gpt-4o-mini-transcribe": 0.003,
}


def encode_multipart(fields, file_field, filename, file_bytes, mime):
    boundary = "----whisperexe%d" % os.getpid()
    buf = io.BytesIO()
    for k, v in fields.items():
        buf.write(
            (
                '--%s\r\nContent-Disposition: form-data; name="%s"\r\n\r\n%s\r\n'
                % (boundary, k, v)
            ).encode("utf-8")
        )
    buf.write(
        (
            '--%s\r\nContent-Disposition: form-data; name="%s"; filename="%s"\r\n'
            'Content-Type: %s\r\n\r\n' % (boundary, file_field, filename, mime)
        ).encode("utf-8")
    )
    buf.write(file_bytes)
    buf.write(("--%s--\r\n" % boundary).encode("utf-8"))
    return buf.getvalue(), boundary


def post_transcription(url, api_key, model, audio_path):
    """Sesi gonder, hipotez metni don. Anahtar disari sizmaz (header disinda)."""
    u = urllib.parse.urlparse(url)
    with open(audio_path, "rb") as f:
        data = f.read()
    mime = mimetypes.guess_type(audio_path)[0] or "application/octet-stream"
    body, boundary = encode_multipart(
        {"model": model, "response_format": "json"},
        "file",
        os.path.basename(audio_path),
        data,
        mime,
    )
    conn = http.client.HTTPSConnection(u.hostname, u.port or 443, timeout=TIMEOUT_S)
    try:
        conn.request(
            "POST",
            u.path,
            body=body,
            headers={
                "Authorization": "Bearer " + api_key,
                "Content-Type": "multipart/form-data; boundary=" + boundary,
            },
        )
        resp = conn.getresponse()
        raw = resp.read()
        status = resp.status
    finally:
        conn.close()
    if status != 200:
        raise RuntimeError("HTTP %d: %s" % (status, raw[:200].decode("utf-8", "replace")))
    return json.loads(raw.decode("utf-8"))["text"]


def durations_secs(ref_dir):
    """wav icin gercek sure; mp3 suresi stdlib ile bilinemez -> None."""
    out = {}
    for fname, _kind, _note in DATASET:
        p = os.path.join(ref_dir, fname)
        if not os.path.exists(p):
            out[fname] = None
        elif fname.endswith(".wav"):
            try:
                out[fname] = wav_seconds(p)
            except Exception:  # kilit disi wav / bozuk baslik -> suresiz
                out[fname] = None
        else:
            out[fname] = None
    return out


def print_cost_plan(durs, price_key, price_usd_min, kur):
    print("maliyet plani: %s @ $%.4f/dk, kur %.1f" % (price_key, price_usd_min, kur))
    total_min = 0.0
    unknown = []
    for fname, _k, _n in DATASET:
        s = durs.get(fname)
        if s is None:
            unknown.append(fname)
            print("  %-18s suresi bilinmiyor (mp3/eksik) -> maliyete dahil degil" % fname)
        else:
            m = s / 60.0
            total_min += m
            print(
                "  %-18s %7.1fs -> $%.4f / %.2f kr"
                % (fname, s, m * price_usd_min, m * price_usd_min * kur * 100)
            )
    print(
        "  TOPLAM (bilinen): %.2f dk -> $%.4f / %.2f TL"
        % (total_min, total_min * price_usd_min, total_min * price_usd_min * kur)
    )
    if unknown:
        print("  NOT: %d dosyanin suresi bilinmiyor; gercek tutar yukaridaki TOPLAMdan buyuktur." % len(unknown))
    print("  KOTA: olcum oncesi panel fallback gunluk tavanini bu tutara cek, salter acik kalsin.")


def main(argv=None):
    ap = argparse.ArgumentParser(prog="fallback_wer.py", description=__doc__.splitlines()[0])
    ap.add_argument("--dry-run", action="store_true", help="ag yok: plan + maliyet")
    ap.add_argument("--local", action="store_true",
                    help="anahtarsiz: provider mock anlamini yerelde calistir "
                         "(tesisat + tarife matematigi testi; kalite skoru anlamsiz)")
    ap.add_argument("--models", default="groq,openai", help="groq,openai alt kumesi")
    ap.add_argument("--openai-model", default=DEFAULT_OPENAI_MODEL)
    ap.add_argument("--ref-dir", default=SERVER_BENCH)
    ap.add_argument("--out", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), "out"))
    ap.add_argument("--kur", type=float, default=48.7)
    args = ap.parse_args(argv)

    want = {m.strip() for m in args.models.split(",") if m.strip()}
    targets = []
    if "groq" in want:
        targets.append(("groq-turbo", GROQ_URL, GROQ_MODEL, "GROQ_API_KEY"))
    if "openai" in want:
        targets.append((args.openai_model, OPENAI_URL, args.openai_model, "OPENAI_API_KEY"))

    have = [(n, u, m, os.environ.get(e)) for (n, u, m, e) in targets]
    if args.dry_run:
        print("dry-run: ag cagrisi YOK.")
        durs = durations_secs(args.ref_dir)
        for name, _u, _m, _k in have:
            key = "groq-turbo" if name == "groq-turbo" else (
                name if name in PRICE_USD_PER_MIN else "whisper-1"
            )
            print_cost_plan(durs, name, PRICE_USD_PER_MIN[key], args.kur)
        print("ref kontrol: *.ref.txt dosyalari '%s' altinda elle yazilir (su an yoksa skorsuz)." % args.ref_dir)
        return 0

    live = [(n, u, m, k) for (n, u, m, k) in have if k]
    mocks = []
    durs = {}
    if args.local:
        # crates/provider/src/lib.rs MockGroq/MockOpenAi anlaminin birebir
        # karsiligi (kutulu metin + ayni faturalandirma): ag yok, anahtar yok.
        durs = durations_secs(args.ref_dir)
        if "groq" in want:
            mocks.append(("yerel-groq", None, "MockGroq", None))
        if "openai" in want:
            mocks.append(("yerel-openai", None, "MockOpenAi", None))
    if not live and not mocks:
        print("anahtar yok — canli test atlandi (GROQ_API_KEY / OPENAI_API_KEY ortamda yok).")
        print("ipucu: tesisat testi icin --local kullan (anahtar gerekmez).")
        return 0
    if mocks:
        print("yerel-mock modu: hipotezler kutulu metindir; WER tablosu tesisati dogrular, kalite karsilastirmaz.")

    results = {}
    for name, url, model, _key in live + mocks:
        is_mock = model.startswith("Mock")
        results[name] = {}
        for fname, _kind, _note in DATASET:
            src = os.path.join(args.ref_dir, fname)
            if not os.path.exists(src):
                results[name][fname] = ("ses-yok", None)
                print("[%s] %s: ses dosyasi yok, atlandi." % (name, fname))
                continue
            try:
                if is_mock:
                    s = durs.get(fname)
                    if s is None:
                        results[name][fname] = ("mp3: stdlib decode yok, atlandi", None)
                        print("[%s] %s: mp3 decode stdlib ile yok, atlandi." % (name, fname))
                        continue
                    import math
                    floor = 10.0 if model == "MockGroq" else 3.0
                    billed = max(math.ceil(s), floor)
                    tag = "groq" if model == "MockGroq" else "openai"
                    hyp = "[%s %.1fsn] ornek transkript (yerel-mock, faturalı %.0fsn)" % (tag, s, billed)
                else:
                    key = os.environ.get("GROQ_API_KEY" if name == "groq-turbo" else "OPENAI_API_KEY")
                    hyp = post_transcription(url, key, model, src)
            except Exception as e:  # noqa: BLE001 - dosya bazinda devam
                results[name][fname] = ("hata: %s" % e, None)
                print("[%s] %s: cagri hatasi: %s" % (name, fname, e))
                continue
            outdir = os.path.join(args.out, name)
            os.makedirs(outdir, exist_ok=True)
            with open(os.path.join(outdir, fname + ".hyp.txt"), "w", encoding="utf-8") as f:
                f.write(hyp)
            refp = os.path.join(args.ref_dir, os.path.splitext(fname)[0] + ".ref.txt")
            if not os.path.exists(refp):
                results[name][fname] = ("ref-yok (hipotez kaydedildi)", None)
                print("[%s] %s: ref yok, skorsuz." % (name, fname))
                continue
            with open(refp, encoding="utf-8") as f:
                ref = f.read()
            w = wer(ref, hyp)
            results[name][fname] = ("WER %.3f" % w, w)
            print("[%s] %s: WER %.3f" % (name, fname, w))

    print("\nmodel x dosya:")
    for name, rows in results.items():
        scores = [v for (_s, v) in rows.values() if v is not None]
        for fname, (s, _v) in rows.items():
            print("  %-22s %-18s %s" % (name, fname, s))
        if scores:
            print(
                "  %-22s %s" % (name, "ort %.3f min %.3f max %.3f (n=%d)"
                                % (sum(scores) / len(scores), min(scores), max(scores), len(scores)))
            )
    print("NOT: tek metrige gore vendor degismez (dinleme notu + WER birlikte, insan karari).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
