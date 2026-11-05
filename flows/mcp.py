"""External MCP 2026-07-28/OAuth server; no Razor implementation is replaced here."""
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlencode, urlsplit
import base64
import hashlib
import http.client
import json
import os
import sys
import threading

PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aX1sAAAAASUVORK5CYII="
SCHEMA = {"type": "object", "properties": {"value": {"type": "string"}}, "additionalProperties": False}
NAMES = ("rich", "form", "url", "hidden", "where", "view", "hang")
# An MCP Apps tool only its app may call: Razor keeps it from every model.
APP = {"ui": {"resourceUri": "ui://flow/view", "visibility": ["app"]}}


def result(params, base):
    """MCP calls return rich content, or one stateless elicitation round."""
    name = params["name"]
    if name in ("form", "url"):
        if not params.get("requestState"):
            question = {"mode": name, "message": f"MCP {name} consent"}
            if name == "form":
                question["requestedSchema"] = {"type": "object", "properties": {
                    "name": {"type": "string"}, "count": {"type": "integer"}}, "required": ["name", "count"]}
            else:
                question.update(url=base + "/consent", elicitationId="consent-37")
            return {"resultType": "input_required", "requestState": "opaque/37+state=", "inputRequests": {
                "answer": {"method": "elicitation/create", "params": question}}}
        assert params["requestState"] == "opaque/37+state=", params
        answer = params["inputResponses"]["answer"]
        assert answer["action"] == "accept", answer
        if name == "form":
            assert answer["content"] == {"name": "Trey", "count": 2}, answer
        return {"resultType": "complete", "content": [{"type": "text", "text": f"{name} accepted"}]}
    text = os.getcwd() + ":" + os.environ.get("MCP_LITERAL", "") if name == "where" else "MCP_RICH_37"
    return {"resultType": "complete", "content": [{"type": "text", "text": text},
            {"type": "image", "data": PNG, "mimeType": "image/png"}],
            "structuredContent": {"marker": "MCP_STRUCTURED_37", "arguments": params.get("arguments", {})}}


class Mcp:
    """One external server, including a PKCE-enforcing issuer with rotating refresh tokens."""

    def __init__(self):
        self.requests, self.errors, self.codes, self.refresh = [], [], {}, set()
        self.tokens, self.registrations, self.exchanges, self.rotations = set(), [], [], []
        self.url, self.large = "", False
        self.closed = threading.Event()

    def authorize(self, url):
        """Visit the authorization page as a browser would, without following its callback."""
        parsed = urlsplit(url)
        connection = http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=10)
        connection.request("GET", parsed.path + "?" + parsed.query)
        response = connection.getresponse()
        response.read()
        assert response.status == 302, response.status
        return response.getheader("Location")

    def rpc(self, body):
        method = body["method"]
        if method == "server/discover":
            return {"resultType": "complete", "supportedVersions": ["2026-07-28"],
                    "capabilities": {"tools": {}}, "serverInfo": {"name": "flow-mcp", "version": "1"},
                    "instructions": "Flow rich tools. More instructions are not the purpose.",
                    "ttlMs": 0, "cacheScope": "private"}
        if method == "tools/list":
            schema = json.loads(json.dumps(SCHEMA))
            if self.large:
                schema["properties"]["value"]["description"] = "catalog schema " * 700
            return {"resultType": "complete", "tools": [
                {"name": name, "description": f"Flow {name} tool", "inputSchema": schema,
                 **({"_meta": APP} if name == "view" else {})} for name in NAMES]}
        assert method == "tools/call", body
        if body["params"]["name"] == "hang":
            self.closed.wait(15)
            return None
        return result(body["params"], self.url)

    def token(self, values):
        grant = values["grant_type"][0]
        if grant == "authorization_code":
            code = self.codes.pop(values["code"][0])
            digest = base64.urlsafe_b64encode(hashlib.sha256(values["code_verifier"][0].encode()).digest()).decode().rstrip("=")
            assert digest == code["code_challenge"][0], "PKCE verifier does not match"
            assert values["redirect_uri"] == code["redirect_uri"], values
            self.exchanges.append(values)
        else:
            assert grant == "refresh_token", values
            old = values["refresh_token"][0]
            assert old in self.refresh, "old refresh token replayed"
            self.refresh.remove(old)
            self.rotations.append(old)
        n = len(self.exchanges) + len(self.rotations)
        access, refresh = f"access-secret-{n}", f"refresh-secret-{n}"
        self.tokens.add(access)
        self.refresh.add(refresh)
        return {"access_token": access, "token_type": "Bearer", "refresh_token": refresh,
                "expires_in": 1 if grant == "authorization_code" else 3600}


@contextmanager
def serve():
    """Serve JSON-RPC and OAuth on a private loopback port; retain the received wire requests."""
    mcp = Mcp()

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def send(self, status, value, **headers):
            data = json.dumps(value).encode()
            self.send_response(status)
            for key, val in {"Content-Type": "application/json", "Content-Length": str(len(data)), **headers}.items():
                self.send_header(key, val)
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            parsed = urlsplit(self.path)
            if parsed.path.startswith("/.well-known/oauth-protected-resource"):
                return self.send(200, {"resource": mcp.url + "/secure", "authorization_servers": [mcp.url]})
            if parsed.path.startswith("/.well-known/"):
                return self.send(200, {"issuer": mcp.url, "authorization_endpoint": mcp.url + "/authorize",
                    "token_endpoint": mcp.url + "/token", "registration_endpoint": mcp.url + "/register",
                    "response_types_supported": ["code"], "grant_types_supported": ["authorization_code", "refresh_token"],
                    "token_endpoint_auth_methods_supported": ["none", "client_secret_basic"],
                    "code_challenge_methods_supported": ["S256"]})
            if parsed.path == "/authorize":
                query = parse_qs(parsed.query)
                assert query["code_challenge_method"] == ["S256"] and query["code_challenge"][0], query
                code = f"code-{len(mcp.codes)}-{len(mcp.exchanges)}"
                mcp.codes[code] = query
                redirect = query["redirect_uri"][0] + "?" + urlencode({"code": code, "state": query["state"][0], "iss": mcp.url})
                return self.send(302, {}, Location=redirect)
            self.send(401, {}, **{"WWW-Authenticate": f'Bearer resource_metadata="{mcp.url}/.well-known/oauth-protected-resource"'})

        def do_POST(self):
            raw = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            if self.path == "/register":
                value = json.loads(raw)
                mcp.registrations.append(value)
                return self.send(201, {**value, "client_id": "flow-dynamic", "token_endpoint_auth_method": "none"})
            if self.path == "/token":
                return self.send(200, mcp.token(parse_qs(raw.decode())))
            body = json.loads(raw)
            assert len(mcp.requests) < 512, "runaway MCP client"
            mcp.requests.append({"body": body, "headers": dict(self.headers), "path": self.path})
            if self.path == "/secure" and self.headers.get("Authorization", "").removeprefix("Bearer ") not in mcp.tokens:
                return self.do_GET()
            result = mcp.rpc(body)
            if result is not None:
                self.send(200, {"jsonrpc": "2.0", "id": body["id"], "result": result})

    with ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
        mcp.url = f"http://127.0.0.1:{server.server_port}"
        # A client that hangs up mid-reply is no stand-in error, as for a real server.
        server.handle_error = lambda *_: isinstance(sys.exc_info()[1], ConnectionError) or mcp.errors.append(repr(sys.exc_info()[1]))
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield mcp
        finally:
            mcp.closed.set()
            server.shutdown()
            thread.join(timeout=5)
    assert not mcp.errors, mcp.errors


def stdio():
    """A separate legacy external server proves Auto's fallback and local execution directory."""
    for line in sys.stdin:
        body = json.loads(line)
        method = body.get("method")
        if "id" not in body:
            if method == "notifications/cancelled" and os.environ.get("MCP_CANCEL_FILE"):
                with open(os.environ["MCP_CANCEL_FILE"], "w") as out:
                    json.dump(body, out)
            continue
        answer = {"jsonrpc": "2.0", "id": body["id"]}
        if method == "server/discover":
            answer["error"] = {"code": -32601, "message": "Legacy server"}
        elif method == "initialize":
            answer["result"] = {"protocolVersion": "2025-11-25", "capabilities": {"tools": {}},
                                "serverInfo": {"name": "legacy-flow", "version": "1"}}
        elif method == "tools/list":
            answer["result"] = {"tools": [{"name": name, "description": name, "inputSchema": SCHEMA}
                                          for name in ("where", "hang")]}
        elif method == "tools/call" and body["params"]["name"] == "hang":
            continue
        else:
            answer["result"] = result(body["params"], "")
            answer["result"].pop("resultType", None)
            if body["params"]["name"] == "where":
                answer["result"]["structuredContent"]["inherited_web_keys"] = [key for key in
                    ("EXA_API_KEY", "FIRECRAWL_API_KEY", "PARALLEL_API_KEY") if key in os.environ]
        print(json.dumps(answer), flush=True)


if __name__ == "__main__":
    stdio()
