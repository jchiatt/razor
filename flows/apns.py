"""A stand-in for Apple's push service: HTTP/2 with prior knowledge on loopback. It records each
request's path, headers and body, checks each provider token's ES256 signature against the public
half of a key it makes with openssl, and answers 200, or 410 for a token a flow calls gone."""

from contextlib import contextmanager, suppress
from pathlib import Path
import base64
import json
import socket
import subprocess
import threading

import h2.config
import h2.connection
import h2.events


class Apple:
    """`key` is the `.p8` file Razor signs with; `received` every request as {path, headers,
    body}, in arrival order; tokens in `gone` are answered 410 Unregistered."""

    def __init__(self, folder: Path, key_id: str, team: str):
        self.key, self.public = folder / f"AuthKey_{key_id}.p8", folder / "public.pem"
        openssl = lambda *args: subprocess.run(["openssl", *args], check=True, capture_output=True)
        openssl("genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-256", "-out", str(self.key))
        openssl("pkey", "-in", str(self.key), "-pubout", "-out", str(self.public))
        self.folder, self.key_id, self.team = folder, key_id, team
        self.received, self.gone, self.errors = [], set(), []

    def signed(self, authorization: str) -> bool:
        """The bearer token's header and claims name this key and team, and its signature holds."""
        head, claims, signature = authorization.removeprefix("bearer ").split(".")
        decode = lambda part: base64.urlsafe_b64decode(part + "=" * (-len(part) % 4))
        if json.loads(decode(head)) != {"alg": "ES256", "kid": self.key_id}:
            return False
        if json.loads(decode(claims))["iss"] != self.team:
            return False
        raw = decode(signature)

        def integer(value: bytes) -> bytes:
            value = value.lstrip(b"\0") or b"\0"
            value = b"\0" + value if value[0] & 0x80 else value
            return bytes([2, len(value)]) + value

        body = integer(raw[:32]) + integer(raw[32:])
        (self.folder / "signed").write_bytes(f"{head}.{claims}".encode())
        (self.folder / "signature").write_bytes(bytes([0x30, len(body)]) + body)
        checked = subprocess.run(["openssl", "dgst", "-sha256", "-verify", str(self.public), "-signature",
                                  str(self.folder / "signature"), str(self.folder / "signed")], capture_output=True)
        return checked.returncode == 0

    def answer(self, connection, stream: int, headers: dict, body: bytes):
        token = headers[":path"].removeprefix("/3/device/")
        if not self.signed(headers.get("authorization", "")):
            self.errors.append(f"a push to {token} was not signed by the key")
        self.received.append({"path": headers[":path"], "headers": headers, "raw": body, "body": json.loads(body)})
        status, reply = (410, b'{"reason":"Unregistered"}') if token in self.gone else (200, b"")
        connection.send_headers(stream, [(":status", str(status))], end_stream=not reply)
        if reply:
            connection.send_data(stream, reply, end_stream=True)

    def serve(self, client: socket.socket):
        connection = h2.connection.H2Connection(h2.config.H2Configuration(client_side=False, header_encoding="utf-8"))
        connection.initiate_connection()
        client.sendall(connection.data_to_send())
        streams = {}
        # Razor closes its connection when its server stops.
        with client, suppress(OSError):
            while data := client.recv(65536):
                for event in connection.receive_data(data):
                    if isinstance(event, h2.events.RequestReceived):
                        streams[event.stream_id] = (dict(event.headers), bytearray())
                    elif isinstance(event, h2.events.DataReceived):
                        streams[event.stream_id][1].extend(event.data)
                        connection.acknowledge_received_data(event.flow_controlled_length, event.stream_id)
                    elif isinstance(event, h2.events.StreamEnded):
                        self.answer(connection, event.stream_id, *streams.pop(event.stream_id))
                client.sendall(connection.data_to_send())


@contextmanager
def apple(folder: Path, key_id: str = "FLOWKEY48", team: str = "FLOWTEAM48"):
    """Yields the stand-in, its URL in `url`; a clean exit fails if a request was not signed."""
    folder.mkdir(exist_ok=True)
    stand_in = Apple(folder, key_id, team)
    listener = socket.create_server(("127.0.0.1", 0))
    stand_in.url = f"http://127.0.0.1:{listener.getsockname()[1]}"

    def accept():
        while True:
            try:
                client = listener.accept()[0]
            except OSError:
                return
            threading.Thread(target=stand_in.serve, args=(client,), daemon=True).start()

    threading.Thread(target=accept, daemon=True).start()
    try:
        yield stand_in
    finally:
        listener.close()
    assert not stand_in.errors, "; ".join(stand_in.errors)
