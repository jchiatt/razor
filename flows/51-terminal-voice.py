"""Real PCM through the terminal's recorder, server admission, saved rows and one player.
The audio programs stand in for devices, never for Razor code; audible playback is a live check.
"""
import json
import os
import tempfile
import wave
from pathlib import Path
from driver import REPLIES, Reply, flow
from terminal import Terminal
from claude import reply
import server

scratch = Path(tempfile.mkdtemp(prefix="razor-audio-flow-"))
capture, play, transcribe, mode, hold, played = [scratch / n for n in ('capture', 'play', 'transcribe', 'mode', 'hold', 'played')]
capture.write_text(f"""import os, struct, time
from pathlib import Path
Path('{scratch}/capture-pid').write_text(str(os.getpid()))
if Path('{mode}').read_text() == 'partial':
    os.write(1, struct.pack('<h', 5000) * 800)
    Path('{scratch}/partial').touch()
    time.sleep(60)
for n in range(6000):
    mode = Path('{mode}').read_text()
    if mode == 'none': time.sleep(10)
    if mode == 'exit': raise SystemExit(3)
    amp = 0 if mode == 'zero' else ([1, 800, 8000, 30000][n % 4])
    os.write(1, struct.pack('<h', amp) * 1600)
    time.sleep(.1)
""")
play.write_text(f"""import json, os, sys, time
from pathlib import Path
with open('{played}', 'a') as f: f.write(json.dumps([os.getpid(), *sys.argv[1:]]) + '\\n')
time.sleep(10)
""")
transcribe.write_text(f"cat >/dev/null; while test -f {hold}; do sleep .1; done; echo spoken-words\n")
config = '\n'.join(f'{k} = {json.dumps(v)}' for k, v in {
    'capture': ['/usr/bin/python3', str(capture)], 'play': ['/usr/bin/python3', str(play), '{rate}'],
    'transcribe': ['/bin/sh', str(transcribe)], 'speech': ['/bin/sh', '-c', 'cat >/dev/null; printf audio']}.items())
mode.write_text('sound')
with flow([Reply((REPLIES / 'hello.sse').read_bytes()), reply(text='new reply arrived')], config=config) as f:
    f.base()
    with Terminal(f, columns=40) as t:
        t.shown('Voice Ctrl-R')
        t.send('my draft\x12')
        t.shown('Recording on', 'Enter sends')
        t.send('\x1b')
        t.shown('Esc discards', 'my draft')
        t.send('\x1b')
        t.until(lambda t: 'voice note' not in t.text())
        t.send('\x12')
        t.shown('Recording on')
        t.shown('0:01')
        hold.touch()
        t.send('\r')
        t.shown('Transcribing', 'my draft')
        t.send(' latest')
        hold.unlink()
        t.shown('voice note', 'spoken-words', 'flow reply')
        session = next((f.home / '.razor/sessions').iterdir()).name
        voice = next(e['voice'] for e in f.record(session) if e['kind'] == 'input')
        assert len(voice['levels']) == 48 and max(voice['levels']) > min(voice['levels']), voice
        y, line = next((y, line) for y, line in enumerate(t.screen.display) if '▶' in line and '▁' in line)
        assert t.screen.buffer[y][line.index('▁')].fg == '3fd9f2', 'saved waveform lost its cyan'
        assert t.screen.buffer[y][line.index('▁')].bg == '123440', 'saved waveform lost the YOU band'
        saved = f.home / '.razor/sessions' / session / voice['file']
        with wave.open(str(saved), 'rb') as wav:
            assert (wav.getnchannels(), wav.getsampwidth(), wav.getframerate(), wav.getnframes()) == (1, 2, 16000, voice['duration_ms'] * 16)
        response = server.send(f, 'GET', f"/sessions/{session}/files/{voice['file']}", headers={'Range': 'bytes=0-3'})
        assert (response.status, response.getheader('Content-Type'), response.read()) == (206, 'audio/wav', b'RIFF')
        t.resize(40, 120)
        # Wait for the resized frame before clicking the play button that moved with it.
        t.until(lambda t: t.screen.display[0][119] == '┐')
        t.click('▶ ')
        t.shown('■')
        t.until(lambda _: played.exists())
        first = json.loads(played.read_text().splitlines()[-1])
        assert first[1] == '1.0', first
        assert Path(first[-1]).read_bytes() == (f.home / '.razor/sessions' / session / voice['file']).read_bytes()
        t.send('\x1b')
        t.until(lambda _: not Path(first[-1]).exists())
        t.send('\x15')
        for _ in range(24):
            t.send('\t')
            t.pump(.05)
            if any('›' in line and '▶' in line and '▁' in line for line in t.screen.display): break
        else: raise AssertionError('Tab never reached the note waveform')
        t.send('\r')
        t.until(lambda _: len(played.read_text().splitlines()) == 2)
        note = json.loads(played.read_text().splitlines()[-1])
        assert note[1] == '1.0'
        t.send('/listen\r')
        t.shown('Reading aloud')
        t.until(lambda _: len(played.read_text().splitlines()) == 3)
        second = json.loads(played.read_text().splitlines()[-1])
        assert second[1] == '1.2', second
        try: os.kill(note[0], 0)
        except ProcessLookupError: pass
        else: raise AssertionError('replaced player survived')
        server.post(f, f'/sessions/{session}/input', {'text': 'another turn'})
        t.shown('new reply arrived', 'Reading aloud')
        os.kill(second[0], 0)  # A newly completed reply cannot interrupt the earlier one.
        t.send('\x1b')
        t.until(lambda t: 'Reading aloud' not in t.text())
        for value in ('zero', 'none', 'exit'):
            mode.write_text(value)
            t.send('\x12')
            t.shown('check', 'Esc discards')
            t.send('\x1b')
            t.until(lambda t: 'Esc discards' not in t.text())
        mode.write_text('partial')
        t.send('\x12')
        t.until(lambda _: (scratch / 'partial').exists())
        t.send('\x1b')
        t.shown('voice note', 'Esc discards')
        local = list((f.home / 'tmp').glob('razor-audio-*/*'))
        assert len(local) == 1 and local[0].stat().st_size == 1644, 'partial PCM window was lost'
        t.send('\x1b')
        t.until(lambda t: 'Esc discards' not in t.text())
        mode.write_text('sound')
        t.send('\x12')
        t.shown('Recording on')
        t.send('\x03')
        t.shown('DISCARD AUDIO')
        t.send('\x1b')
        t.until(lambda t: 'DISCARD AUDIO' not in t.text())
