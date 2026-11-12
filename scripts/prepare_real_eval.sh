#!/bin/bash
# Convert a directory of audio files (any format ffmpeg reads) into the
# 44.1 kHz stereo WAV library used by `examples/06_accuracy_eval.rs --dir`.
#
#   scripts/prepare_real_eval.sh /usr/share/sounds/ubuntu/ringtones data/real_eval
#
# Files shorter than MIN_SECS (default 6) are skipped: the evaluation cuts
# clips of up to 5 s from random offsets and needs room to do so.
set -euo pipefail

SRC="${1:?usage: $0 <source-dir> <out-dir>}"
OUT="${2:?usage: $0 <source-dir> <out-dir>}"
MIN_SECS="${MIN_SECS:-6}"

command -v ffmpeg >/dev/null || { echo "ffmpeg is required" >&2; exit 1; }
command -v ffprobe >/dev/null || { echo "ffprobe is required" >&2; exit 1; }
mkdir -p "$OUT"

count=0
while IFS= read -r -d '' f; do
    dur=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$f" 2>/dev/null </dev/null || echo 0)
    if awk -v d="$dur" -v m="$MIN_SECS" 'BEGIN { exit !(d >= m) }'; then
        name=$(basename "${f%.*}" | tr ' ' '_' | tr -cd '[:alnum:]_-')
        ffmpeg -v error -y -i "$f" -ar 44100 -ac 2 -c:a pcm_s16le "$OUT/$name.wav" </dev/null
        count=$((count + 1))
    fi
done < <(find "$SRC" -type f \( -iname '*.ogg' -o -iname '*.oga' -o -iname '*.wav' -o -iname '*.mp3' -o -iname '*.flac' -o -iname '*.m4a' \) -print0)

echo "Prepared $count tracks in $OUT"
