"""The bundled ElevenLabs program against the vendor's HTTP shape, including a bare machine.
No credentials or paid synthesis: the vendor stand-in returns a short real MP3 and alignment."""
import base64
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from driver import REPLIES

program = Path(__file__).resolve().parents[1]/"scripts/speech.py"
mp3 = (REPLIES/"voice.mp3").read_bytes()
requests = []


class Vendor(BaseHTTPRequestHandler):
    def do_GET(self):
        assert self.headers["xi-api-key"] == "flow-key"
        if self.path == "/v1/user":
            body = {"subscription":{"character_limit":100000,"character_count":0}}
        else:
            assert self.path.startswith("/v2/voices?"),self.path
            body = {"voices":[{"voice_id":"iris","name":"Iris","labels":{"accent":"American"}}],"has_more":False}
        self.send(body)

    def do_POST(self):
        assert self.path == "/v1/text-to-speech/chosen/with-timestamps?output_format=mp3_44100_64"
        requests.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
        body = {"audio_base64":base64.b64encode(mp3).decode(),"alignment":{
            "characters":list("hello world"),"character_start_times_seconds":[i*.02 for i in range(11)],
            "character_end_times_seconds":[(i+1)*.02 for i in range(11)]}}
        if requests[-1]["text"] == "no alignment":
            del body["alignment"]
        self.send(body)

    def send(self,body):
        self.send_response(200)
        self.send_header("Content-Type","application/json")
        self.end_headers()
        self.wfile.write(json.dumps(body).encode())

    def log_message(self,*args):
        pass


vendor = ThreadingHTTPServer(("127.0.0.1",0),Vendor)
threading.Thread(target=vendor.serve_forever,daemon=True).start()
try:
    with tempfile.TemporaryDirectory(prefix="speech-program-") as folder:
        words = Path(folder)/"speech/1.json"
        env = {**os.environ,"ELEVENLABS_API_KEY":"flow-key","ELEVENLABS_BASE_URL":f"http://127.0.0.1:{vendor.server_port}",
               "RAZOR_VOICE":"chosen","RAZOR_GAIN_DB":"9.5","RAZOR_EXPRESSIVENESS":"0.2","RAZOR_WORDS":str(words)}
        no_ssl = "import runpy,sys; sys.modules['ssl'] = None; sys.argv = sys.argv[1:]; runpy.run_path(sys.argv[0],run_name='__main__')"
        unsafe = subprocess.run([sys.executable,"-c",no_ssl,str(program)],input=b"hello",
                                env={**env,"ELEVENLABS_BASE_URL":"https://api.elevenlabs.invalid"},capture_output=True)
        assert unsafe.returncode == 1 and b"has no working SSL module" in unsafe.stderr,unsafe.stderr
        listed = subprocess.run([sys.executable,str(program),"--voices"],env=env,capture_output=True,check=True)
        assert json.loads(listed.stdout) == [{"id":"iris","name":"Iris","description":"","preview_url":None,"labels":{"accent":"American"}}]
        text = "[excited] Hello. https://example.invalid /tmp/private `x=2` abcdef0123 [pleased] [loud]"
        bare = subprocess.run([sys.executable,str(program)],input=text.encode(),env={**env,"PATH":""},capture_output=True,check=True)
        assert bare.stdout == mp3,bare.stderr
        meta = json.loads(words.read_text())
        assert meta == {"duration_ms":220,"levels":[],"words":[{"start_ms":0,"end_ms":100,"text":"hello"},{"start_ms":120,"end_ms":220,"text":"world"}]},meta
        body = requests[-1]
        assert body["model_id"] == "eleven_v4_turbo" and body["voice_settings"] == {"stability":.8,"similarity_boost":.75}
        assert all(part not in body["text"] for part in ("https://","/tmp/","abcdef0123","[loud]")),body
        assert "x=2" in body["text"]
        for prose,expected in [("~2 minutes","~2 minutes"),("1000000 users","1000000 users"),
                               ("20261101","20261101"),("defaced and effaced","defaced and effaced"),
                               ("Run `cargo build`","Run cargo build")]:
            subprocess.run([sys.executable,str(program)],input=prose.encode(),env={**env,"PATH":""},capture_output=True,check=True)
            assert requests[-1]["text"] == expected,(prose,requests[-1])
        words.unlink()
        unaligned = subprocess.run([sys.executable,str(program)],input=b"no alignment",env={**env,"PATH":""},capture_output=True,check=True)
        assert unaligned.stdout == mp3 and not words.exists()
        boosted = subprocess.run([sys.executable,str(program)],input=b"hello",env=env,capture_output=True,check=True)
        assert boosted.stdout[:3] == b"ID3" and json.loads(words.read_text())["duration_ms"] == 220
        levels = json.loads(words.read_text())["levels"]
        if shutil.which("ffmpeg"):
            assert 1 <= len(levels) <= 64 and max(levels) > 0 and boosted.stdout != mp3
        else:
            assert levels == [] and boosted.stdout == mp3
finally:
    vendor.shutdown()
    vendor.server_close()
print("PASS 55-speech-program")
