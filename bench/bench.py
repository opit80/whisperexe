"""whisperexe bench iskeleti (F0): tekrarlanabilir tek komut + WER.

Kilitli kararlar: 16kHz mono PCM inference, 180sn tavan, 3sn minimum quantum.
Ses dosyalarini KOPYALAMAZ; server\\bench altindaki dosyalara referans verir.
Gercek inference backend'i F1b/F3'te baglanir.
Standart kutuphane disinda bagimlilik YOK.
"""

import argparse
import os
import sys
import wave

SERVER_BENCH = os.path.join("C:", os.sep, "projelerim", "whisper", "server", "bench")

DATASET = [
    ("bench-voice.wav", "wav-uzun", "ana metin: RTF + terim isabeti"),
    ("bench-short.wav", "wav-kisa", "duman testi"),
    ("msg1_teknik.mp3", "mp3", "decode yolu dayanimi"),
    ("msg2_github.mp3", "mp3", "decode yolu dayanimi"),
    ("msg3_gunluk.mp3", "mp3", "decode yolu dayanimi"),
    ("msg4_kisa.mp3", "mp3-kisa", "quantum alti davranis"),
]


def normalize_tr(s):
    import string

    s = s.lower()
    for p in string.punctuation + "“”‘’—…":
        s = s.replace(p, " ")
    return s.split()


def wer(ref_text, hyp_text):
    r, h = normalize_tr(ref_text), normalize_tr(hyp_text)
    if not r:
        return 1.0 if h else 0.0
    prev = list(range(len(h) + 1))
    for i, rw in enumerate(r, 1):
        cur = [i]
        for j, hw in enumerate(h, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (rw != hw)))
        prev = cur
    return prev[len(h)] / len(r)


def wav_seconds(path):
    with wave.open(path, "rb") as w:
        assert w.getnchannels() == 1 and w.getframerate() == 16000, (
            "kilit: 16kHz mono PCM, dosya: %s" % path
        )
        return w.getnframes() / float(w.getframerate())


def cmd_run():
    print("referans kok: %s" % SERVER_BENCH)
    missing = False
    for fname, kind, note in DATASET:
        p = os.path.join(SERVER_BENCH, fname)
        ok = os.path.exists(p)
        missing = missing or not ok
        extra = ""
        if ok and fname.endswith(".wav"):
            try:
                extra = " %.1fs" % wav_seconds(p)
            except AssertionError as e:
                extra = " UYARI: %s" % e
        print("[%s] %-18s %-9s %s%s" % ("OK " if ok else "YOK", fname, kind, note, extra))
    if missing:
        print("UYARI: eksik referans dosya var (kopyalama YOK, kaynagi kontrol et).")
        return 1
    print("iskele OK: inference backend'i F1b/F3'te baglanacak.")
    return 0


def cmd_wer(args):
    with open(args.ref, encoding="utf-8") as f:
        ref = f.read()
    with open(args.hyp, encoding="utf-8") as f:
        hyp = f.read()
    print("WER: %.3f" % wer(ref, hyp))


def cmd_rtf(args):
    rtf = args.infer_s / args.audio_s
    print("RTF: %.3f (infer %.1fs / ses %.1fs)" % (rtf, args.infer_s, args.audio_s))


def main(argv=None):
    ap = argparse.ArgumentParser(
        prog="bench.py",
        description="whisperexe bench iskeleti: set kontrolu + RTF + TR WER.",
    )
    sub = ap.add_subparsers(dest="cmd")
    sub.add_parser("run", help="referans set varlik + WAV kilit (16kHz mono) kontrolu")
    p_wer = sub.add_parser("wer", help="referans vs hipotez WER")
    p_wer.add_argument("--ref", required=True)
    p_wer.add_argument("--hyp", required=True)
    p_rtf = sub.add_parser("rtf", help="RTF = infer_s / audio_s")
    p_rtf.add_argument("--infer-s", type=float, required=True)
    p_rtf.add_argument("--audio-s", type=float, required=True)
    args = ap.parse_args(argv)
    if args.cmd == "wer":
        cmd_wer(args)
        return 0
    if args.cmd == "rtf":
        cmd_rtf(args)
        return 0
    return cmd_run()


if __name__ == "__main__":
    sys.exit(main())
