"""Audio refusal/retry, live machine settings, SSH, a fast capture limit and quitting in flight.
Transport faults go through a proxy to the real server, which still owns transcription/admission.
"""
import base64
import json
import select
import socket
import socketserver
import tempfile
import threading
from pathlib import Path
from driver import flow
from terminal import Terminal
from claude import reply
import server

scratch = Path(tempfile.mkdtemp(prefix='razor-audio-edges-'))
capture, play, transcribe, hold, mode, played, heard, runs = [scratch / n for n in
    ('capture', 'play', 'transcribe', 'hold', 'mode', 'played', 'heard', 'runs')]
capture.write_text(f"""import os, struct, time
from pathlib import Path
for n in range(6000):
    mode = Path('{mode}').read_text()
    os.write(1, struct.pack('<h', 1 if mode == 'quiet' else [1, 5000, 30000][n % 3]) * 1600)
    if mode != 'fast': time.sleep(.1)
""")
play.write_text(f"""import json, os, sys, time
from pathlib import Path
with open('{played}', 'a') as f: f.write(json.dumps([os.getpid(), *sys.argv[1:]]) + '\\n')
with open('{heard}', 'a') as f: f.write(Path(sys.argv[-1]).read_bytes().hex() + '\\n')
if Path('{mode}').read_text() == 'bad-player': raise SystemExit(3)
time.sleep(.3)
""")
transcribe.write_text(f"""echo run >> {runs}
cat >/dev/null
while test -f {hold}; do sleep .05; done
if test "$(cat {mode})" = fail; then echo 'engine refused this recording.' >&2; exit 3; fi
echo words-from-the-note
""")
commands = {'capture': ['/usr/bin/python3', str(capture)],
    'play': ['/usr/bin/python3', str(play), '{rate}'], 'transcribe': ['/bin/sh', str(transcribe)],
    'speech': ['/bin/sh', '-c', 'cat']}


def config(commands=commands):
    return '\n'.join(f'{key} = {json.dumps(value)}' for key, value in commands.items())


class Proxy(socketserver.ThreadingUnixStreamServer):
    daemon_threads = True

    def __init__(self, f):
        self.upstream = f.home / '.razor/upstream.sock'
        path = f.home / '.razor/server.sock'
        path.rename(self.upstream)
        self.inputs, self.uploads = [], 0  # At most the four attempts this flow makes.
        proxy = self

        class Handler(socketserver.BaseRequestHandler):
            def handle(self):
                head = b''
                while b'\r\n\r\n' not in head:
                    part = self.request.recv(4096)
                    if not part: return
                    head += part
                    assert len(head) < 64 * 1024
                headers, body = head.split(b'\r\n\r\n', 1)
                # One request per connection keeps every retry visible to this proxy.
                headers = b'\r\n'.join(line for line in headers.split(b'\r\n')
                    if not line.lower().startswith(b'connection:')) + b'\r\nConnection: close'
                first = headers.split(b'\r\n', 1)[0]
                if first.startswith(b'POST') and b'/files/voice.wav ' in first:
                    proxy.uploads += 1
                if first.startswith(b'POST') and b'/input ' in first:
                    length = next(int(line.split(b':')[1]) for line in headers.split(b'\r\n')
                                  if line.lower().startswith(b'content-length:'))
                    while len(body) < length: body += self.request.recv(length - len(body))
                    proxy.inputs.append(json.loads(body))
                    assert len(proxy.inputs) <= 8
                    if len(proxy.inputs) == 1:
                        self.request.sendall(b'HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n')
                        return
                    if len(proxy.inputs) == 2:
                        # The real server starts its program; the client loses the answer.
                        with socket.socket(socket.AF_UNIX) as upstream:
                            upstream.connect(str(proxy.upstream))
                            upstream.sendall(headers + b'\r\n\r\n' + body)
                        return
                with socket.socket(socket.AF_UNIX) as upstream:
                    upstream.connect(str(proxy.upstream))
                    upstream.sendall(headers + b'\r\n\r\n' + body)
                    while True:
                        readable, _, _ = select.select([self.request, upstream], [], [], 1)
                        for source in readable:
                            try: data = source.recv(65536)
                            except OSError: return
                            if not data: return
                            try: (upstream if source is self.request else self.request).sendall(data)
                            except OSError: return

        super().__init__(str(path), Handler)
        self.thread = threading.Thread(target=self.serve_forever, daemon=True)
        self.thread.start()


mode.write_text('quiet')
with flow([], config=config(), env={'SSH_CONNECTION': 'flow'}) as f:
    with Terminal(f) as t:
        t.shown('Message the agent')
        assert 'Voice' not in t.text()
        t.send('\x12')
        t.shown('RECORDING IS UNAVAILABLE OVER SSH')
        assert not list((f.home / 'tmp').glob('razor-audio-*'))

# Quiet, nonzero PCM is valid; failed transcription keeps the same uploaded file for Enter.
with flow([reply(text=['first spoken reply', 'second spoken reply']), reply(text='uncached reply')], config=config()) as f:
    f.base()
    with Terminal(f, columns=40) as t:
        t.shown('Voice Ctrl-R')
        image = f.cwd / 'pixel.png'
        image.write_bytes(base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aMk0AAAAASUVORK5CYII='))
        t.send('parked draft \x1b[200~' + str(image) + '\x1b[201~')
        t.shown('[Image #1]')
        t.send('\x12')
        t.shown('Recording on', '0:00')
        t.send('\x12')
        t.pump(.15)
        assert 'Recording on' in t.text()
        t.send('\x1b')
        t.shown('Esc discards', '[Image #1]', 'parked draft')
        mode.write_text('fail')
        t.send('\r')
        t.shown("Couldn't transcribe it:", 'refused this recording.', 'Enter tries again')
        assert "Couldn't transcribe it: engine refused this recording." in " ".join(t.text().split())
        session = next((f.home / '.razor/sessions').iterdir()).name
        files = list((f.home / '.razor/sessions' / session).rglob('voice.wav'))
        assert len(files) == 1, files
        mode.write_text('quiet')
        t.send('\r')
        t.shown('words-from-the-note', '[Image #1]')
        assert list((f.home / '.razor/sessions' / session).rglob('voice.wav')) == files
        assert sum(e['kind'] == 'input' for e in f.record(session)) == 1
        t.resize(40, 120)
        t.shown('└' + '─' * 118 + '┘', 'first spoken reply', 'second spoken reply')
        played.write_text('')
        t.send('\x15/listen\r')
        t.until(lambda _: len(played.read_text().splitlines()) == 2)
        calls = [json.loads(line) for line in played.read_text().splitlines()]
        assert [bytes.fromhex(line) for line in heard.read_text().splitlines()] == [b'first spoken reply', b'second spoken reply']
        assert [call[1] for call in calls] == ['1.2', '1.2'], calls
        t.until(lambda t: 'Reading aloud' not in t.text())
        cfg = f.home / '.razor/config.toml'
        cfg.write_text(cfg.read_text().replace(json.dumps(commands['play']), json.dumps(commands['play'][:-1])))
        t.send('/listen\r')
        t.until(lambda _: len(played.read_text().splitlines()) >= 3)
        assert len(json.loads(played.read_text().splitlines()[2])) == 2  # pid and appended path, no rate.
        t.send('\x1b')
        t.until(lambda t: 'Reading aloud' not in t.text())
        mode.write_text('bad-player')
        t.send('/listen\r')
        t.shown('local player failed', 'Enter tries again')
        mode.write_text('quiet')
        t.send('\r')
        t.shown('Reading aloud')
        t.send('\x1b')
        t.until(lambda t: 'Reading aloud' not in t.text())
        cfg.write_text(cfg.read_text().replace(json.dumps(commands['speech']), '[]'))
        # A new text entry has no cached speech, so missing speech remains a visible refusal.
        server.post(f, f'/sessions/{session}/input', {'text': 'need new speech'})
        t.shown('uncached reply')
        t.send('/listen\r')
        t.shown('No speech program is configured', 'Enter tries again')
        cfg.write_text(cfg.read_text().replace('speech = []', 'speech = ["/bin/sh", "-c", "exit 3"]'))
        t.send('\r')
        t.shown("Couldn't read it aloud", 'Enter tries again')
        t.send('\x1b')
        t.until(lambda t: 'Enter tries again' not in t.text())

# The real server's four busy programs refuse a fifth; a live config change then refuses 503.
mode.write_text('quiet')
hold.touch()
runs.write_text('')
with flow([reply(text=f'busy note {n}') for n in range(5)], config=config()) as f:
    server.start(f)
    session = server.post(f, '/sessions', {'cwd': str(f.cwd)})['session']
    threads, results, sessions = [], [], []
    for n in range(4):
        busy = server.post(f, '/sessions', {'cwd': str(f.cwd)})['session']
        sessions.append(busy)
        path = json.loads(server.send(f, 'POST', f'/sessions/{busy}/files/voice.wav', b'audio').read())['path']
        body = {'text': '', 'request': f'busy-{n}', 'voice': {'file': path, 'duration_ms': 100, 'levels': [1]}}
        thread = threading.Thread(target=lambda body=body, busy=busy: results.append(server.post(f, f'/sessions/{busy}/input', body)))
        thread.start()
        threads.append(thread)
    try:
        with Terminal(f, '--resume', session) as t:
            t.shown('Voice')
            t.until(lambda _: len(runs.read_text().splitlines()) == 4)
            t.send('\x12')
            t.shown('Recording on')
            t.pump(.2)
            t.send('\r')
            t.shown('Four voice notes and replies are being processed', 'Enter tries again')
            cfg = f.home / '.razor/config.toml'
            original = cfg.read_text()
            cfg.write_text(original.replace(json.dumps(commands['transcribe']), '[]'))
            t.send('\r')
            t.shown('This server has no transcriber', 'Enter tries again')
            cfg.write_text(original)
            hold.unlink()
            for thread in threads: thread.join()
            assert len(results) == 4
            for busy in sessions:
                server.until(f, busy, lambda es: es[-1]['kind'] == 'turn_end')
            t.send('\r')
            t.shown('words-from-the-note')
            server.until(f, session, lambda es: sum(e['kind'] == 'turn_end' for e in es) >= 1)
            assert sum(e['kind'] == 'input' for e in f.record(session)) == 1
    finally:
        hold.unlink(missing_ok=True)
        for thread in threads: thread.join()

# Limit is measured PCM duration; a fast device can fill ten minutes without a ten-minute test.
mode.write_text('fast')
with flow([], config=config()) as f:
    with Terminal(f, columns=40) as t:
        t.shown('Voice Ctrl-R')
        t.send('\x12')
        t.shown('10:00', 'Esc discards')
        assert not list((f.home / '.razor/sessions').glob('*/record.jsonl'))
        t.send('\x03')
        t.shown('DISCARD', 'AUDIO.', 'Esc stays')
        t.send('\x1b')
        t.until(lambda t: 'PRESS CTRL-C' not in t.text())
    assert not list((f.home / 'tmp').glob('razor-audio-*'))

# A dropped/paired-style answer re-asks one id/file, while drafts and quit stay responsive.
mode.write_text('quiet')
hold.touch()
with flow([reply(text='late note admitted')], config=config()) as f:
    server.start(f)
    session = server.post(f, '/sessions', {'cwd': str(f.cwd)})['session']
    proxy = Proxy(f)
    try:
        with Terminal(f, '--resume', session) as t:
            t.shown('Voice')
            t.send('before\x12')
            t.shown('Recording on')
            t.send('\r')
            t.shown('Transcribing')
            t.send(' latest')
            draft = f.home / '.razor/sessions' / session / 'draft'
            t.until(lambda _: draft.exists() and draft.read_text() == 'before latest')
            t.until(lambda _: len(proxy.inputs) >= 3)
            assert proxy.uploads == 1
            assert len({json.dumps((i['request'], i['voice']), sort_keys=True) for i in proxy.inputs}) == 1
            t.send('\x1b')
            t.shown('THE VOICE NOTE MAY STILL ARRIVE')
            t.until(lambda t: 'Transcribing' not in t.text())
            t.until(lambda _: not list((f.home / 'tmp').glob('razor-audio-*/*')))
        hold.unlink()
        server.until(f, session, lambda es: any(e['kind'] == 'turn_end' for e in es))
        draft = server.send(f, 'GET', f'/sessions/{session}/draft')
        assert (draft.status, draft.read()) == (200, b'before latest')
        assert sum(e['kind'] == 'input' for e in f.record(session)) == 1
        assert not list((f.home / 'tmp').glob('razor-audio-*'))
    finally:
        proxy.shutdown()
        proxy.server_close()
