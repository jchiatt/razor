"""A terminal meeting a line of an unknown kind (a proxy adds one) says so once and shows the rest;
a paired server's status reader skips it and still hears the peer."""
import json
import select
import socket
import socketserver
import threading
import time
from driver import flow
from terminal import Terminal
from responses import done
import server

LINE = b'{"kind":"from_a_newer_server","id":99,"at":1}\n'

with flow([done()], model="gpt-6.1-sol") as f:
    server.start(f)
    session = server.post(f, '/sessions', {'cwd': str(f.cwd)})['session']
    upstream, path = f.home / '.razor/upstream.sock', f.home / '.razor/server.sock'
    path.rename(upstream)

    class Handler(socketserver.BaseRequestHandler):
        def handle(self):
            # An HTTP/1.0 answer has no chunk framing, so a line can be added to the stream.
            with socket.socket(socket.AF_UNIX) as up:
                up.connect(str(upstream))
                up.sendall(self.request.recv(65536).replace(b'HTTP/1.1', b'HTTP/1.0', 1))
                while True:
                    for source in select.select([self.request, up], [], [], 1)[0]:
                        data = source.recv(65536)
                        if not data:
                            return
                        if source is up and b'"kind":"attached"' in data:
                            end = data.index(b'\n', data.index(b'"kind":"attached"')) + 1
                            data = data[:end] + LINE + data[end:]
                        (up if source is self.request else self.request).sendall(data)

    threading.Thread(target=socketserver.ThreadingUnixStreamServer(str(path), Handler).serve_forever, daemon=True).start()
    with Terminal(f, '--resume', session) as t:
        t.shown('Message the agent')
        t.send('hello\r')
        t.shown('flow done', 'Newer server', '`razor --continue`')
        text = ' '.join(t.text().split())
        assert text.count('Newer server') == 1 and 'invalid stream line' not in text, text


# A newer paired server's status stream opens on the same line; the reader skips it and hears the rest.
peer = socket.create_server(('127.0.0.1', 0))


def answer():
    while True:
        conn = peer.accept()[0]
        status = conn.recv(65536).startswith(b'GET /v1/status')
        conn.sendall(b'HTTP/1.0 200 OK\r\n\r\n' + (LINE + b'{"kind":"server","id":"s58","label":"S","name":"","machine":"","realm":"","transcribe":false,"shared":false,"video":false}\n' if status else b'[]'))
        threading.Timer(30 * status, conn.close).start()


threading.Thread(target=answer, daemon=True).start()
with flow([], config='[server]\naddress = "127.0.0.1:0"') as f:
    server.start(f)
    assert f.run('devices', 'add', 'stand', '--address', '127.0.0.1:%d' % peer.getsockname()[1]).returncode == 0
    deadline = time.monotonic() + 15
    while not any(s['name'] == 'stand' and s['silent'] is None for s in json.loads(server.send(f, 'GET', '/status').readline())['servers']):
        assert time.monotonic() < deadline, "the peer's unknown line kept it silent"
        time.sleep(0.2)
