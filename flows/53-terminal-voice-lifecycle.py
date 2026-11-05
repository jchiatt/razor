"""Voice leaves answers addressed, queued notes usable, and local programs reaped on quit/hangup."""
import json
import os
import signal
import tempfile
import time
from pathlib import Path
from driver import flow
from model import message, reply
from terminal import Terminal
import server

scratch = Path(tempfile.mkdtemp(prefix='razor-voice-lifecycle-'))
capture, player, captured, played, release = [scratch / n for n in ('capture', 'player', 'captured', 'played', 'release')]
capture.write_text(f"""import os, struct, time
from pathlib import Path
Path('{captured}').write_text(str(os.getpid()))
while True:
    os.write(1, struct.pack('<h', 5000) * 1600)
    time.sleep(.1)
""")
player.write_text(f"""import os, sys, time
from pathlib import Path
with open('{played}', 'a') as f: f.write(str(os.getpid()) + '\\n')
time.sleep(60)
""")
commands = {'capture': ['/usr/bin/python3', str(capture)], 'play': ['/usr/bin/python3', str(player), '{rate}'],
            'transcribe': ['/bin/sh', '-c', 'cat >/dev/null; echo ordinary-voice-input'],
            'speech': ['/bin/sh', '-c', 'cat']}


def config(changes={}):
    return '\n'.join(f'{k} = {json.dumps(v)}' for k, v in (commands | changes).items())


def gone(pid):
    try: os.kill(pid, 0)
    except ProcessLookupError: return True
    return False


for key, why in [('capture', 'No local capture program is set'), ('transcribe', 'This server has no transcriber')]:
    with flow([], config=config({key: []})) as f, Terminal(f) as t:
        t.shown('Message the agent')
        assert 'Voice' not in t.text()
        # Before the server's status arrives /record says so, and is asked again until it has.
        for _ in range(100):
            t.send('/record\r')
            if not t.pump(1, lambda t: 'NOT ANSWERED YET' in t.text().upper()): break
            t.pump(.2)
        t.until(lambda t: why.upper() in t.text().upper())

# A voice input must leave the open ask, its answer text and its answer address intact.
root = 'VOICE_QUESTION_ROOT'
with flow([message(root, 'question', text='Keep this answer addressed?', options=['yes', 'no']),
           reply(root, text='question-open'), reply(root, text='voice-arrived'),
           reply(root, text='answer-arrived')], config=config()) as f:
    server.start(f)
    session = server.post(f, '/sessions', {'cwd': str(f.cwd)})['session']
    server.post(f, f'/sessions/{session}/input', {'text': root})
    server.until(f, session, lambda es: es[-1]['kind'] == 'turn_end')
    with Terminal(f, '--resume', session, rows=80) as t:
        # Enter answers only the band a person sees, and the transcript can draw the question first.
        t.shown('Keep this answer addressed?', 'question-open', 'Enter to answer in words')
        t.send('\rtyped answer\x12')
        t.shown('Recording on')
        t.pump(.2)
        t.send('\r')
        t.shown('ordinary-voice-input', 'voice-arrived', 'typed answer')
        inputs = [e for e in f.record(session) if e['kind'] == 'input']
        assert inputs[-1]['text'] == 'ordinary-voice-input' and 'answer' not in inputs[-1]['from']
        t.send('\r')
        t.shown('answer-arrived')
        question = next(e['id'] for e in f.record(session) if e['kind'] == 'call' and e.get('name') == 'mcp__razor__message')
        assert [e for e in f.record(session) if e['kind'] == 'input'][-1]['from']['answer']['ask'] == question

# A live shell keeps a note queued. Its waveform still plays, and Take Back restores its words.
root = 'VOICE_QUEUE_ROOT'
with flow([reply(root, 'Bash', {'command': f'while ! test -f {release}; do sleep .05; done'}),
           reply(root, text='queue-root-done')], config=config()) as f:
    server.start(f)
    session = server.post(f, '/sessions', {'cwd': str(f.cwd)})['session']
    other = server.post(f, '/sessions', {'cwd': str(f.cwd)})['session']
    assert server.send(f, 'PUT', f'/sessions/{other}/name', 'other').status == 200
    server.post(f, f'/sessions/{session}/input', {'text': root})
    server.until(f, session, lambda es: any(e['kind'] == 'job' for e in es))
    with Terminal(f, '--resume', session, '--model', 'opus', columns=160, rows=80) as t:
        # Voice shows once the server's status says it can transcribe; Ctrl-R waits for it.
        t.shown('Esc stops', 'Voice')
        t.send('\x12')
        t.shown('Recording on')
        t.pump(.2)
        t.send('\x1b')
        t.shown('Esc discards')
        t.send('\x1bOQ')
        t.shown('AGENTS', 'Earlier')
        t.click('Earlier')
        t.shown('other')
        t.click('other')
        t.shown('WAIT BEFORE CHANGING THE ADDRESSED SESSION')
        t.send('\r')
        t.shown('queued', 'voice note', 'ordinary-voice-input')
        t.send('\x1bOQ')
        t.until(lambda t: 'AGENTS' not in t.text())
        t.click('▶ ')
        t.until(lambda _: played.exists())
        pid = int(played.read_text().splitlines()[-1])
        t.send('\x1b')
        t.until(lambda _: gone(pid))
        t.click('take back')
        t.shown('ordinary-voice-input')
        t.until(lambda t: 'take back' not in t.text())
        assert not any(e['kind'] == 'turn_end' for e in f.record(session)), 'Esc stopped the agent'
        release.touch()
        t.shown('queue-root-done')

# Starting capture reaps a player; quitting reaps capture and removes its private file.
with flow([reply(None, text='completed reply')], config=config()) as f:
    server.start(f)
    session = server.post(f, '/sessions', {'cwd': str(f.cwd)})['session']
    server.post(f, f'/sessions/{session}/input', {'text': 'begin'})
    server.until(f, session, lambda es: es[-1]['kind'] == 'turn_end')
    with Terminal(f, '--resume', session) as t:
        t.shown('completed reply', 'Voice')
        before = len(played.read_text().splitlines())
        t.click('▶')
        t.shown('Reading aloud')
        t.until(lambda _: len(played.read_text().splitlines()) > before)
        pid = int(played.read_text().splitlines()[-1])
        t.send('\x12')
        t.shown('Recording on')
        assert gone(pid)
        capture_pid = int(captured.read_text())
        directory = next((f.home / 'tmp').glob('razor-audio-*'))
        assert directory.stat().st_mode & 0o777 == 0o700
        assert all(p.stat().st_mode & 0o777 == 0o600 for p in directory.iterdir())
    assert gone(capture_pid) and not directory.exists()
    with Terminal(f, '--resume', session) as t:
        t.shown('completed reply')
        before = len(played.read_text().splitlines())
        t.send('/listen\r')
        t.shown('Reading aloud')
        t.until(lambda _: len(played.read_text().splitlines()) > before)
        pid = int(played.read_text().splitlines()[-1])
    assert gone(pid)
    # A real terminal hangup enters the same cleanup path, without another key to the app.
    with Terminal(f, '--resume', session) as t:
        t.shown('completed reply', 'Voice')
        t.send('\x12')
        t.shown('Recording on')
        pid = int(captured.read_text())
        os.kill(t.pid, signal.SIGHUP)
        deadline = time.monotonic() + 10
        while True:
            waited, status = os.waitpid(t.pid, os.WNOHANG)
            if waited: break
            assert time.monotonic() < deadline, 'hangup did not exit'
            t.pump(.05)
        assert os.waitstatus_to_exitcode(status) == 0 and gone(pid)
        t.quit = lambda: None  # The hangup already reaped this terminal.
    assert not list((f.home / 'tmp').glob('razor-audio-*'))
