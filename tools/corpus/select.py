#!/usr/bin/env python3
"""Pick a genre-spread subset of the reference stick and copy/transcode it into the oracle corpus.

Read-only on the source. Usage:
  python3 -I tools/corpus/select.py /Volumes/REKORDBOX/Contents ~/re-work/corpus/real --count 100

Writes <out>/<format>/<file>, <out>/selection.json. Deterministic (fixed seed).
"""
import argparse, json, os, random, subprocess, sys, unicodedata

# (format dir, extension, ffmpeg args, keeps artwork). Order = assignment priority.
FORMATS = [
    ("aiff", "aiff", None, True),  # None = byte copy of the original AIFF
    ("flac", "flac", ["-c:a", "flac"], True),
    ("mp3_cbr320", "mp3", ["-c:a", "libmp3lame", "-b:a", "320k", "-id3v2_version", "3"], True),
    ("mp3_vbr_v2", "mp3", ["-c:a", "libmp3lame", "-q:a", "2", "-id3v2_version", "3"], True),
    ("m4a_aac256", "m4a", ["-c:a", "aac_at", "-b:a", "256k"], True),
    ("m4a_alac", "m4a", ["-c:a", "alac_at"], True),
    ("wav", "wav", ["-c:a", "pcm_s16le"], False),
]
# Share of tracks per format (sums to 1).
WEIGHTS = {"aiff": 0.30, "flac": 0.15, "mp3_cbr320": 0.15, "mp3_vbr_v2": 0.10,
           "m4a_aac256": 0.15, "m4a_alac": 0.10, "wav": 0.05}


def probe(path):
    out = subprocess.run(["ffprobe", "-v", "error", "-print_format", "json", "-show_format",
                          "-show_streams", path], capture_output=True, text=True)
    if out.returncode != 0:
        return None
    data = json.loads(out.stdout)
    fmt = data.get("format", {})
    tags = {k.lower(): v for k, v in fmt.get("tags", {}).items()}
    audio = next((s for s in data.get("streams", []) if s.get("codec_type") == "audio"), {})
    return {
        "duration": float(fmt.get("duration", 0)),
        "genre": tags.get("genre", "").strip() or "(none)",
        "artist": tags.get("artist", ""),
        "title": tags.get("title", ""),
        "bpm": tags.get("tbpm") or tags.get("bpm"),
        "sample_rate": int(audio.get("sample_rate", 0)),
        "codec": audio.get("codec_name"),
        "has_art": any(s.get("codec_type") == "video" for s in data.get("streams", [])),
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("src")
    ap.add_argument("out")
    ap.add_argument("--count", type=int, default=100)
    args = ap.parse_args()
    out = os.path.expanduser(args.out)

    files = []
    for root, _, names in os.walk(args.src):
        for n in sorted(names):
            if n.startswith("._") or not n.lower().endswith((".aiff", ".aif", ".flac", ".mp3", ".m4a", ".wav")):
                continue
            files.append(os.path.join(root, n))
    files.sort()
    print(f"probing {len(files)} files", file=sys.stderr)
    tracks = []
    for p in files:
        info = probe(p)
        if info and 60 <= info["duration"] <= 600:
            info["source"] = p
            tracks.append(info)

    by_genre = {}
    for t in tracks:
        by_genre.setdefault(t["genre"].lower(), []).append(t)
    rng = random.Random(20261011)
    for lst in by_genre.values():
        rng.shuffle(lst)
    picked = []
    genres = sorted(by_genre)
    while len(picked) < args.count and any(by_genre.values()):
        for g in genres:
            if by_genre[g] and len(picked) < args.count:
                picked.append(by_genre[g].pop())

    slots = []
    for name, *_ in FORMATS:
        slots += [name] * round(WEIGHTS[name] * len(picked))
    slots = (slots + ["aiff"] * len(picked))[: len(picked)]
    rng.shuffle(slots)
    spec = {f[0]: f for f in FORMATS}

    selection = []
    for track, fmt in zip(picked, slots):
        name, ext, ff, keeps_art = spec[fmt]
        stem = unicodedata.normalize("NFC", os.path.splitext(os.path.basename(track["source"]))[0])
        dst_dir = os.path.join(out, name)
        os.makedirs(dst_dir, exist_ok=True)
        dst = os.path.join(dst_dir, f"{stem}.{ext}")
        if not os.path.exists(dst):
            if ff is None and track["source"].lower().endswith((".aiff", ".aif")):
                subprocess.run(["cp", track["source"], dst], check=True)
            else:
                cmd = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-i", track["source"],
                       "-map", "0:a:0", "-map_metadata", "0"]
                if keeps_art and track["has_art"]:
                    cmd += ["-map", "0:v:0", "-c:v", "copy", "-disposition:v:0", "attached_pic"]
                cmd += (ff or ["-c:a", "pcm_s16be"]) + [dst]
                subprocess.run(cmd, check=True)
        selection.append({**track, "format": fmt, "path": dst})
    with open(os.path.join(out, "selection.json"), "w") as f:
        json.dump(selection, f, indent=1, ensure_ascii=False)
    counts = {}
    for s in selection:
        counts[s["format"]] = counts.get(s["format"], 0) + 1
    print(json.dumps({"tracks": len(selection), "genres": len(genres), "formats": counts}))


if __name__ == "__main__":
    main()
