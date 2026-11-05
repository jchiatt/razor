#!/usr/bin/env python3
"""Turnkey ElevenLabs speech program: text stdin, MP3 stdout, optional RAZOR_WORDS metadata.
Requires ELEVENLABS_API_KEY; --voices lists available voices. Without ffmpeg, volume is unchanged
and waveform levels are empty. All vendor knowledge lives in this program, outside Razor core."""
import array
import base64
import json
import os
import re
import shutil
import subprocess
import sys
import urllib.parse
import urllib.request
from pathlib import Path

BASE = os.environ.get("ELEVENLABS_BASE_URL", "https://api.elevenlabs.io")
# George, an ElevenLabs premade voice, which every account has.
VOICE = os.environ.get("RAZOR_VOICE") or "JBFqnCBsd6RMkjVDRZzb"
MODEL = os.environ.get("ELEVENLABS_MODEL") or "eleven_v4_turbo"
# RAZOR_TEMPO: a voice's speed, made in the recording (eleven_v3 ignores ElevenLabs'); the phone's applies on top.
MOST, RESERVE, TEMPO = 5000, 30000, float(os.environ.get("RAZOR_TEMPO") or 1)
TABLE = "There's a table in the reply."
FFMPEG = shutil.which("ffmpeg")

def ear(text):
    """The reply as it should sound: headings as sentences, a link as its words, code blocks and tables
    named rather than read, list and emphasis marks dropped; audio tags such as [pleased] kept."""
    text = re.sub(r"```.*?(```|\Z)", "\nThere's some code in the reply.\n", text, flags=re.S)
    lines = []
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("|"):
            if not lines or lines[-1] != TABLE:
                lines.append(TABLE)
            continue
        line = re.sub(r"^#+\s*(.*?)[\s#]*$", lambda m: m[1].rstrip(".:") + ".", line)
        lines.append(re.sub(r"^(?:[-*+>]|\d+[.)])\s+", "", line))
    text = "\n".join(lines)
    text = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"https?://\S+", "a link", text)
    text = re.sub(r"`([^`]*)`", r"\1", text)
    text = re.sub(r"(?<!\w)(?:~/|/|[A-Z]:\\)[^\s,;!?]+", "a file", text)
    text = re.sub(r"\b(?=[0-9a-f]*[0-9])(?=[0-9a-f]*[a-f])[0-9a-f]{7,64}\b", "a revision", text)
    tags = []
    def tag(match):
        tags.append(match[0])
        return match[0] if len(tags) <= 2 else ""
    text = re.sub(r"\[[^]\n]{1,40}\]", tag, text)
    text = re.sub(r"(\*\*|\*|~~)(?=\S)(.+?)(?<=\S)\1", r"\2", text)
    text = re.sub(r"(?<!\w)(__|_)(?=\S)(.+?)(?<=\S)\1(?!\w)", r"\2", text)
    text = re.sub(r"\n{3,}", "\n\n", text).strip()
    if len(text) > MOST:
        text = text[:MOST].rsplit(" ", 1)[0] + "... The rest is in the app."
    return text



def call(path, body=None):
    key = os.environ.get("ELEVENLABS_API_KEY") or sys.exit("speech: there is no ElevenLabs key; run razor keys to add one.")
    headers = {"xi-api-key": key, "Content-Type": "application/json"}
    data = None if body is None else json.dumps(body).encode()
    with urllib.request.urlopen(urllib.request.Request(BASE + path, data, headers), timeout=60) as answer:
        return json.load(answer)


def voices():
    rows, token = [], ""
    while len(rows) < 1000:
        page = call("/v2/voices?" + urllib.parse.urlencode({"page_size":100,"next_page_token":token}))
        rows.extend({"id":v["voice_id"], "name":v["name"], "description":v.get("description") or "",
                     "preview_url":v.get("preview_url"), "labels":v.get("labels") or {}} for v in page["voices"])
        if not page.get("has_more"):
            break
        token = page["next_page_token"]
    print(json.dumps(rows[:1000]))


def audio(mp3):
    if not FFMPEG:
        return mp3, []
    gain = float(os.environ.get("RAZOR_GAIN_DB", "9.5"))
    filters = (f"atempo={TEMPO}," if TEMPO != 1 else "")+f"volume={gain}dB,alimiter=limit=0.891:attack=5:release=80:level=disabled"
    def run(*args):
        result = subprocess.run([FFMPEG,"-v","error","-i","pipe:0",*args,"pipe:1"],
                                input=mp3, capture_output=True, timeout=60)
        if result.returncode:
            raise ValueError("ffmpeg could not process speech")
        return result.stdout
    try:
        pcm = array.array("h",run("-af",filters,"-ac","1","-ar","8000","-f","s16le"))
        if sys.byteorder != "little":
            pcm.byteswap()
        step = max(1,(len(pcm)+63)//64)
        levels = [min(255,round(max(abs(x) for x in pcm[i:i+step])/32768*255))
                  for i in range(0,len(pcm),step)]
        return run("-af",filters,"-b:a","64k","-f","mp3"), levels
    except (OSError,ValueError,subprocess.TimeoutExpired):
        print("speech: ffmpeg failed; keeping original audio volume and no waveform.",file=sys.stderr)
        return mp3, []


def metadata(alignment, tempo=1):
    words, word = [], None
    for char,start,end in zip(alignment["characters"],alignment["character_start_times_seconds"],
                              alignment["character_end_times_seconds"]):
        if char.isspace():
            if word:
                words.append(word)
                word = None
        else:
            if word is None:
                word = {"start_ms":round(start*1000/tempo),"end_ms":0,"text":""}
            word["text"] += char
            word["end_ms"] = round(end*1000/tempo)
    if word:
        words.append(word)
    ends = alignment["character_end_times_seconds"]
    return {"duration_ms":round(ends[-1]*1000/tempo) if ends else 0,"words":words}


def main():
    if sys.argv[1:] == ["--voices"]:
        return voices()
    text = ear(sys.stdin.read())
    if not text:
        sys.exit("speech: the reply has no words to read.")
    plan = call("/v1/user")["subscription"]
    left = plan["character_limit"] - plan["character_count"]
    if left-len(text) < RESERVE:
        sys.exit(f"speech: {left:,} credits cannot cover this reply and the {RESERVE:,} reserve; nothing spoken.")
    settings = {"stability":1-float(os.environ.get("RAZOR_EXPRESSIVENESS","0.5")),"similarity_boost":0.75}
    body = {"text":text,"model_id":MODEL,"voice_settings":settings}
    result = call(f"/v1/text-to-speech/{urllib.parse.quote(VOICE,safe='')}/with-timestamps?output_format=mp3_44100_64",body)
    mp3, levels = audio(base64.b64decode(result["audio_base64"]))
    if (path := os.environ.get("RAZOR_WORDS")) and (alignment := result.get("normalized_alignment") or result.get("alignment")):
        meta = metadata(alignment, TEMPO if levels else 1)
        Path(path).parent.mkdir(parents=True,exist_ok=True)
        Path(path).write_text(json.dumps({**meta,"levels":levels}))
    sys.stdout.buffer.write(mp3)
    print(f"speech: {sys.argv[-1] if sys.argv[1:] else 'a session'}: {len(text):,} characters spoken, {left:,} credits left before",file=sys.stderr)


if __name__ == "__main__":
    try:
        main()
    except (OSError,KeyError,ValueError) as error:
        sys.exit(f"speech: {error}")
