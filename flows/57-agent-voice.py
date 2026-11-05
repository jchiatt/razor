"""Agent speech through message, settings owner, persistent daily count, silence and real files.
The model, speech program and Apple are vendor stand-ins; Razor runs through its real API."""
import json
import sys
import tempfile
import threading
import time
from dataclasses import replace
from pathlib import Path
from urllib.parse import urlencode
from apns import apple
from driver import REPLIES, flow
from responses import PLAIN, reply
from terminal import Terminal
import server
from server import wait


def api(home, method, path, body=None, expected=200, raw=False, headers=None):
    response = server.send(home, method, path, None if method == "GET" else json.dumps(body), {"Content-Type": "application/json", **(headers or {})})
    text = response.read()
    assert response.status == expected, (path,response.status,text)
    return (dict(response.getheaders()),text) if raw else json.loads(text)


def restart(f, home):
    server.stop(home)
    assert f.run("devices").returncode == 0


scratch = Path(tempfile.mkdtemp(prefix="voice-program-"))
program = scratch / "speech.py"
player, played = scratch / "play.py", scratch / "played"
player.write_text('''import json,os,sys,time
from pathlib import Path
with open(sys.argv[1],"a") as log:
    log.write(json.dumps({"terminal":os.getppid(),"rate":sys.argv[2],"audio":Path(sys.argv[3]).read_bytes().hex()})+"\\n")
while Path(sys.argv[1]+".hold").exists(): time.sleep(.05)
time.sleep(.3)
''')
program.write_text('''import json,os,sys
from pathlib import Path
if sys.argv[1:] == ["--voices"]:
    print('[{"id":"iris","name":"Iris","description":"Flow voice","preview_url":null,"labels":{}}]')
    sys.exit()
text = sys.stdin.read()
with open(os.environ["VOICE_RUNS"],"a") as log:
    log.write(json.dumps({"text":text,"argv":sys.argv[1:],**{k:v for k,v in os.environ.items() if k.startswith("RAZOR_")}})+"\\n")
if "FAIL51" in text:
    sys.exit("stand-in speech failed")
if path := os.environ.get("RAZOR_WORDS"):
    path = Path(path)
    path.parent.mkdir(parents=True,exist_ok=True)
    path.write_text(json.dumps({"duration_ms":300,"levels":[0,200,30],"words":[{"start_ms":0,"end_ms":300,"text":text}]}))
sys.stdout.buffer.write(Path(os.environ["VOICE_MP3"]).read_bytes())
''')
spoken = "[excited] The check passed. You can open the result."
replies = []
for marker,text,requested in (("VOICE51",spoken,False),("OFF51","off",True),("DESK51","desk",False),
                              ("ASKED51","asked",True),("QUIET51","quiet",True),("MEMO51","x"*301,False),
                              ("LONG51","m"*301,False),("FAIL51","FAIL51",False))+(
                              ("HEAR54","The terminal can hear this note.",False),
                              ("SILENT54","Quiet note.",False),("PHONE54","Phone note.",False),
                              ("OTHER54","Another session's note.",False),("SSH54","SSH note.",False),("SWITCH54","One terminal speaks.",False))+(("MAC54","Mac login input.",False),)*(sys.platform == "darwin"):
    replies += [reply(marker,"message",type="note",to="parent",text=text,voice=True,requested=requested),
                reply(marker,"Finished.")]
with apple(scratch) as apple_server:
    config = PLAIN + f'''speech = {json.dumps([sys.executable,str(program)])}
play = {json.dumps([sys.executable,str(player),str(played),"{rate}"])}
[server]
address = "127.0.0.1:0"
[push]
team = "FLOWTEAM48"
bundle = "com.example.razor"
endpoint = "{apple_server.url}"
[push.development]
key_id = "FLOWKEY48"
key = {json.dumps(str(apple_server.key))}
'''
    # PLAIN opens a TOML table: machine settings must precede it.
    config = config.replace(PLAIN,"")+PLAIN
    with flow(replies,model="sol",config=config,env={"VOICE_RUNS":str(scratch/"runs"),
              "VOICE_MP3":str(REPLIES/"voice.mp3")}) as f:
        server.start(f)
        home = f.home / ".razor"
        settings = api(home,"GET","/voice")
        assert api(home,"POST","/voice/reserve")["settings"] == settings
        assert "HH:MM" in api(home,"PUT","/voice",{**settings,"quiet":{"from":"25:00","to":"08:00"}},expected=400)["error"]
        enrolled = f.run("devices","add","phone")
        assert enrolled.returncode == 0, enrolled.stderr
        phone_key = enrolled.stdout.splitlines()[1]
        assert server.send(f,"PUT","/devices/self/push",{"token":"aa51","environment":"development"},key=phone_key).status == 200
        # At `all`, every finish is sent, so the check below sees the note take its turn's place.
        assert server.send(f,"PUT","/devices/self/notifications",{"level":"all"},key=phone_key).status == 200
        made = server.post(f,"/sessions",{"cwd":str(f.cwd)})["session"]

        def turn(marker):
            before = sum(e["kind"] == "turn_end" for e in f.record(made))
            server.post(f,f"/sessions/{made}/input",{"text":marker})
            entries = server.until(f,made,lambda es: sum(e["kind"] == "turn_end" for e in es)>before)
            call = [e for e in entries if e["kind"] == "call"][-1]
            result = [e for e in entries if e["kind"] == "result"][-1]
            return call,result

        def pushes(entry):
            return [p["body"] for p in apple_server.received if p["body"]["razor"].get("entry") == entry
                    and p["body"]["razor"]["kind"] == "notify"]

        call,result = turn("VOICE51")
        assert result["outcome"] == "success" and json.loads(result["output"]) == {"id":[made,call["id"]]},result
        delivery = api(home,"GET","/deliveries?local=true")[0]
        assert (delivery["entry"],delivery["line"],delivery["sound"]) == (call["id"],spoken,"phone"),delivery
        assert delivery["voice"]["duration_ms"] == 300 and "words" not in delivery
        path = delivery["voice"]["file"]
        headers,audio = api(home,"GET",f"/sessions/{made}/files/{path}",raw=True)
        assert headers["content-type"] == "audio/mpeg" and audio == (REPLIES/"voice.mp3").read_bytes()
        headers,audio = api(home,"GET",f"/sessions/{made}/files/{path}",raw=True,
                          headers={"Range":"bytes=0-3"},expected=206)
        assert audio == (REPLIES/"voice.mp3").read_bytes()[:4]
        _,meta = api(home,"GET",f"/sessions/{made}/files/{path[:-3]}json",raw=True)
        assert json.loads(meta)["words"][0]["text"] == spoken
        push = wait(lambda: pushes(call["id"]))[0]
        assert push["aps"]["sound"] == "default" and spoken not in json.dumps(push)
        api(home,"PUT",f"/sessions/{made}/deliveries/{call['id']}/read",{})
        assert api(home,"GET","/deliveries?local=true")[0]["read"]
        receipt = f"/sessions/{made}/deliveries/{call['id']}"
        stored = [json.loads(row) for row in (home/"deliveries.jsonl").read_text().splitlines()][-1]
        assert stored["project"] == f.cwd.name and not stored["archived"], stored
        stream = server.send(home,"GET","/status?local=true")
        assert json.loads(stream.readline())["kind"] == "agents"
        for method,field,read,archived in (("DELETE","read",False,False),("PUT","archive",False,True),
                                          ("PUT","read",True,True),("DELETE","read",False,True),
                                          ("DELETE","archive",False,False)):
            api(home,method,receipt+"/"+field)
            row = next(json.loads(line) for line in iter(stream.readline,b"")
                       if json.loads(line)["kind"] == "delivery")
            assert (row["read"],row["archived"],row["at"]) == (read,archived,delivery["at"]),row
            api(home,method,receipt+"/"+field)  # Repeating changes neither state nor index bytes.
            rows = api(home,"GET","/deliveries?local=true")
            assert len(rows) == 1 and (rows[0]["read"],rows[0]["archived"]) == (read,archived),rows
        assert len((home/"deliveries.jsonl").read_text().splitlines()) == 7
        stream.close()
        restart(f,home)
        assert [(d["read"],d["archived"]) for d in api(home,"GET","/deliveries?local=true")] == [(False,False)]
        for query in ("CHECK passed",f.cwd.name.upper(),delivery["name"].upper(),"check "+f.cwd.name):
            assert len(api(home,"GET","/deliveries?"+urlencode({"q":query,"limit":1}))) == 1,query
        assert api(home,"GET","/deliveries?q=check+absent57") == []
        api(home,"PUT",receipt+"/read")
        assert api(home,"GET","/voice")["used_today"] == len(spoken)

        settings.update(when="never",used_today=0)
        settings = api(home,"PUT","/voice",settings)
        assert settings["used_today"] == len(spoken)
        off,result = turn("OFF51")
        assert api(home,"GET","/deliveries?q=check&limit=1")[0]["entry"] == call["id"]
        assert result["outcome"] == "error"
        # A turn that delivered a voice note sends no finished alert behind the note's own.
        finished = lambda: [p for p in apple_server.received if p["body"]["razor"]["kind"] == "finished"]
        assert len(wait(finished)) == 1 and finished()[0]["body"]["razor"]["entry"] > off["id"], finished()
        settings.update(when="away",voice="chosen",gain_db=100,expressiveness=-1)
        settings = api(home,"PUT","/voice",settings)
        assert (settings["gain_db"],settings["expressiveness"]) == (20,0)
        stream = server.send(home,"GET",f"/sessions/{made}/events?terminal=true")
        assert json.loads(stream.readline())["kind"] == "attached"
        api(home,"PUT","/voice/activity",{"terminal":"sample51","active":True})
        assert turn("DESK51")[1]["outcome"] == "error"
        call,result = turn("ASKED51")
        assert result["outcome"] == "success",result
        assert api(home,"GET","/deliveries?local=true")[0]["sound"] == "terminal"
        assert "sound" not in wait(lambda: pushes(call["id"]))[0]["aps"]
        now = time.localtime()
        minute = now.tm_hour*60+now.tm_min
        clock = lambda n: f"{n//60:02}:{n%60:02}"
        settings.update(quiet={"from":clock((minute-1)%1440),"to":clock((minute+1)%1440)})
        api(home,"PUT","/voice",settings)
        call,result = turn("QUIET51")
        assert result["outcome"] == "success"
        assert api(home,"GET","/deliveries?local=true")[0]["sound"] == "silent"
        assert "sound" not in wait(lambda: pushes(call["id"]))[0]["aps"]
        stream.close()
        api(home,"PUT","/voice/activity",{"terminal":"sample51","active":False})
        settings.update(when="always",quiet=None)
        api(home,"PUT","/voice",settings)
        assert turn("MEMO51")[1]["outcome"] == "error"
        settings["memos"] = True
        api(home,"PUT","/voice",settings)
        assert turn("LONG51")[1]["outcome"] == "success"
        used = api(home,"GET","/voice")["used_today"]
        assert turn("FAIL51")[1]["outcome"] == "error"
        assert api(home,"GET","/voice")["used_today"] == used+6
        assert len(api(home,"GET","/deliveries?local=true")) == 4
        restart(f,home)
        assert api(home,"GET","/voice")["used_today"] == used+6

        # A real terminal reports local keys, and notes from any root choose that player.
        old = next(d for d in api(home,"GET","/deliveries?local=true") if d["sound"] == "terminal")
        f.env["RAZOR_IDLE_SECONDS"] = "999999"  # The Mac timer must not overwrite key reports.
        played_count = lambda: len(played.read_text().splitlines()) if played.exists() else 0
        with Terminal(f,"--resume",made,rows=55,columns=120) as t:
            t.shown("Finished.")
            t.pump(1)
            assert not played.exists(), "an old or non-terminal delivery auto-played on attach"
            assert not next(d for d in api(home,"GET","/deliveries?local=true") if d["entry"] == old["entry"])["read"]
            t.send("x")
            t.shown("x")
            t.pump(1)
            held_player = played.with_suffix(".hold")
            held_player.touch()
            synthesized = len((scratch/"runs").read_text().splitlines())
            call,result = turn("HEAR54")
            assert result["outcome"] == "success",result
            t.shown("The terminal can hear this note.","▶")
            heard = lambda: next(d for d in api(home,"GET","/deliveries?local=true") if d["entry"] == call["id"])
            assert heard()["sound"] == "terminal" and heard()["terminal"].startswith(str(t.pid)+"-"), heard()
            wait(lambda: played_count() == 1)
            assert not heard()["read"], "a note was marked read before playback finished"
            held_player.unlink()
            wait(lambda: heard()["read"])
            runs = [json.loads(line) for line in played.read_text().splitlines()]
            assert runs == [{"terminal":t.pid,"rate":"1.2","audio":(REPLIES/"voice.mp3").read_bytes().hex()}], [(r["rate"],len(r["audio"])//2) for r in runs]
            assert len((scratch/"runs").read_text().splitlines()) == synthesized+1, "playback synthesized a reply instead of fetching the note"
            note_receipt = f"/sessions/{made}/deliveries/{call['id']}"
            api(home,"PUT",note_receipt+"/archive")
            api(home,"DELETE",note_receipt+"/read")
            # A later-opened conversation keeps its archived note, with the same voice and receipt.
            with Terminal(f,"--resume",made,rows=55,columns=120) as archived:
                archived.shown("The terminal can hear this note.")
                archived.until(lambda t: any("▶" in line and "▁" in line for line in t.screen.display))
                y,line = [(y,line) for y,line in enumerate(archived.screen.display) if "▶" in line and "▁" in line][-1]
                x = line.index("▶")
                assert archived.screen.buffer[y][x].fg == archived.screen.buffer[y][line.index("▌")].fg
                archived.send(f"\x1b[<0;{x+1};{y+1}M\x1b[<0;{x+1};{y+1}m")
                wait(lambda: played_count() == 2)
                assert json.loads(played.read_text().splitlines()[-1])["rate"] == "1.2"
                assert len((scratch/"runs").read_text().splitlines()) == synthesized+1
                wait(lambda: heard()["read"])
            api(home,"DELETE",note_receipt+"/read")
            api(home,"DELETE",note_receipt+"/archive")
            t.pump(1)
            assert played_count() == 2 and not heard()["read"], "unarchiving replayed the cached note"
            # Quiet hours never start a player.
            settings["quiet"] = {"from":"00:00","to":"23:59"} if time.localtime().tm_hour < 23 else {"from":"23:00","to":"00:00"}
            api(home,"PUT","/voice",settings)
            call,_ = turn("SILENT54")
            assert heard()["sound"] == "silent" and "terminal" not in heard()
            settings["quiet"] = None
            api(home,"PUT","/voice",settings)
            # Two terminals on the same agent: only the most recently used one may play.
            with Terminal(f,"--resume",made,rows=55,columns=120) as second:
                second.shown("Finished.")
                second.send("y")
                second.shown("y")
                second.pump(1)
                call,_ = turn("SWITCH54")
                wait(lambda: played_count() >= 3)
                t.pump(1)
                second.pump(1)
                assert played_count() == 3, "two terminals played the same note"
                assert heard()["terminal"].startswith(str(second.pid)+"-"), heard()
                assert json.loads(played.read_text().splitlines()[-1])["terminal"] == second.pid
                wait(lambda: heard()["read"])
            # Closing the newest terminal restores the other; an unrelated agent also speaks there.
            other_root = server.post(f,"/sessions",{"cwd":str(f.cwd)})["session"]
            server.post(f,f"/sessions/{other_root}/input",{"text":"OTHER54"})
            server.until(f,other_root,lambda es:any(e["kind"] == "turn_end" for e in es))
            wait(lambda: played_count() >= 4)
            note = next(d for d in api(home,"GET","/deliveries?local=true") if d["session"] == other_root)
            assert note["sound"] == "terminal" and note["terminal"].startswith(str(t.pid)+"-"),note
            assert json.loads(played.read_text().splitlines()[-1])["terminal"] == t.pid
            wait(lambda: next(d for d in api(home,"GET","/deliveries?local=true") if d["session"] == other_root)["read"])
        call,_ = turn("PHONE54")
        assert heard()["sound"] == "phone" and "terminal" not in heard(), heard()
        # SSH input must leave the note on the phone and cannot start a local player.
        f.env["SSH_CONNECTION"] = "flow"
        with Terminal(f,"--resume",made,rows=55,columns=120) as t:
            t.shown("Finished.")
            t.send("z")
            t.shown("z")
            t.pump(1)
            call,_ = turn("SSH54")
            assert heard()["sound"] == "phone", heard()
            t.pump(1)
            assert played_count() == 4
            assert not heard()["read"]
        del f.env["SSH_CONNECTION"]
        if sys.platform == "darwin":
            f.env["RAZOR_IDLE_SECONDS"] = "0"
            with Terminal(f,"--resume",made,rows=55,columns=120) as t:
                t.shown("Finished.")
                t.pump(16)  # One production login-input sample, without any terminal key.
                call,_ = turn("MAC54")
                wait(lambda: heard()["read"])
                assert heard()["terminal"].startswith(str(t.pid)+"-"),heard()
        used = api(home,"GET","/voice")["used_today"]-6

        # One owner across two paired servers; simultaneous reservations cannot overspend.
        other = f.home/".razor-b"
        other.mkdir()
        for name in ("auth.json","config.toml"):
            (other/name).write_bytes((home/name).read_bytes())
        beta = replace(f,env={**f.env,"RAZOR_HOME":str(other)})
        assert beta.run("devices").returncode == 0
        address = json.loads((other/"server.json").read_text())["address"]
        paired = f.run("devices","add","b","--address",address)
        counterpart = paired.stdout.splitlines()[-1].split()
        assert beta.run("devices","add","a",*counterpart[4:]).returncode == 0
        owner_id = (home/"server.id").read_text().strip()
        config_path = other/"config.toml"
        config_path.write_text(config_path.read_text().replace('[server]\n',f'[server]\nshared = "{owner_id}"\n'))
        try:
            api(other,"GET","/voice",expected=400)
            settings.update(daily_characters=used+7+20)
            api(home,"PUT","/voice",settings)
            answers = []
            # A remote note must fetch its permission from this owner; the peer's file never appears.
            def servers():
                return {s["id"]:s["shared"] for s in json.loads(server.send(other,"GET","/status").readline())["servers"]}
            # Status names the keeper of shared settings: the owner, never the home naming it.
            assert wait(lambda: owner_id in (seen := servers()) and seen) == {
                owner_id:True,(other/"server.id").read_text().strip():False}
            remote = api(other,"POST","/sessions",{"cwd":str(f.cwd)})["session"]
            api(home,"PUT","/voice/activity",{"terminal":"owner-terminal","active":True})
            api(other,"PUT","/voice/activity",{"terminal":"paired-terminal","active":True})
            api(other,"PUT","/voice/activity",{"terminal":"looped","active":True},expected=400,headers={"x-razor-forwarded":"1"})
            # A terminal connected through the non-owner reports to the owner, and plays a
            # delivery from another server's agent through the real paired delivery stream.
            before = played_count()
            with Terminal(beta,"--resume",remote,rows=55,columns=120) as listener:
                listener.shown("Message the agent")
                listener.send("r")
                listener.shown("r")
                listener.pump(1)
                replies += [reply("PAIR54","message",type="note",to="parent",text="remote",voice=True),reply("PAIR54","Finished.")]
                assert f.run("--resume",made,"--print=json","PAIR54").returncode == 0
                wait(lambda: played_count() > before)
                routed = next(d for d in api(home,"GET","/deliveries?local=true") if d["line"] == "remote")
                assert routed["sound"] == "terminal" and routed["terminal"].startswith(str(listener.pid)+"-"),routed
                query = urlencode({"q":"REMOTE "+f.cwd.name.upper(),"limit":1})
                merged = wait(lambda: api(other,"GET","/deliveries?"+query))
                assert merged[0]["session"].endswith("@a") and merged[0]["project"] == f.cwd.name,merged
                assert api(other,"GET","/deliveries?q=remote+absent57") == []
                peer_receipt = f"/sessions/{merged[0]['session']}/deliveries/{merged[0]['entry']}"
                api(other,"PUT",peer_receipt+"/archive")
                wait(lambda: api(other,"GET","/deliveries?"+query)[0]["archived"])
                api(other,"DELETE",peer_receipt+"/archive")
                api(other,"DELETE",peer_receipt+"/read")
                wait(lambda: not api(other,"GET","/deliveries?"+query)[0]["read"])
                api(other,"PUT",peer_receipt+"/read")
                assert json.loads(played.read_text().splitlines()[-1])["terminal"] == listener.pid
                wait(lambda: next(d for d in api(home,"GET","/deliveries?local=true") if d["entry"] == routed["entry"])["read"])
            routed = api(other,"POST",f"/sessions/{remote}/voice/499",{"text":"r","requested":False})
            assert (routed["sound"],routed["terminal"]) == ("terminal","paired-terminal"),routed
            api(other,"PUT","/voice/activity",{"terminal":"paired-terminal","active":False})
            assert not (other/"voice.json").exists()
            assert api(home,"GET","/voice")["used_today"] == used+13
            # Two more 14-character attempts: one may reserve, one must hit the same owner cap.
            def attempt(where,session):
                response = server.send(where,"POST",f"/sessions/{session}/voice/501",{"text":"c"*14,"requested":False})
                answer = json.loads(response.read())
                if response.status == 200:
                    assert (answer["sound"],answer["terminal"]) == ("terminal","owner-terminal"),answer
                answers.append(response.status)
            threads = [threading.Thread(target=attempt,args=(where,session)) for where,session in [(home,made),(other,remote)]]
            for t in threads: t.start()
            for t in threads: t.join()
            assert sorted(answers) == [200,400],answers
            api(home,"PUT","/voice/activity",{"terminal":"owner-terminal","active":False})
            assert api(home,"GET","/voice")["used_today"] == used+27
            # Family voice applies to read-aloud too; losing the owner falls back to program defaults.
            names = [line.split()[0].removeprefix("%actor/").removesuffix("/0") for line in
                     (Path(__file__).resolve().parents[1]/"src/terminal/data/stage.txt").read_text().splitlines()
                     if line.startswith("%actor/") and line.split()[0].endswith("/0")]
            settings["families"] = dict.fromkeys(names,"family-voice")
            api(home,"PUT","/voice",settings)
            replies.append(reply("READ51","Read me."))
            read = beta.run("--resume",remote,"--print=json","READ51")
            assert read.returncode == 0,read.stderr
            record = lambda: list(map(json.loads,(other/"sessions"/remote/"record.jsonl").read_text().splitlines()))
            text_entry = [e for e in record() if e["kind"] == "text"][-1]["id"]
            api(other,"GET",f"/sessions/{remote}/speech/{text_entry}",raw=True)
            last = json.loads((scratch/"runs").read_text().splitlines()[-1])
            assert (last["RAZOR_VOICE"],last["argv"]) == ("family-voice",[remote]), last
            original = config_path.read_text()
            config_path.write_text(original.replace(owner_id,"unknown-owner"))
            assert "unknown" in api(other,"POST",f"/sessions/{remote}/voice/600",{"text":"unknown","requested":True},expected=400)["error"]
            config_path.write_text(original)
            server.stop(home)
            assert "unavailable" in api(other,"POST",f"/sessions/{remote}/voice/601",{"text":"offline","requested":True},expected=400)["error"]
            replies.append(reply("AFTER51","Read the next reply."))
            assert beta.run("--resume",remote,"--print=json","AFTER51").returncode == 0
            text_entry = [e for e in record() if e["kind"] == "text"][-1]["id"]
            api(other,"GET",f"/sessions/{remote}/speech/{text_entry}",raw=True)
            assert "RAZOR_VOICE" not in json.loads((scratch/"runs").read_text().splitlines()[-1])
            server.start(f)
            assert api(home,"GET","/voice")["used_today"] == used+27
            kept = json.loads((home/"voice.json").read_text())
            kept["day"] = "1970-01-01"
            (home/"voice.json").write_text(json.dumps(kept))
            assert api(home,"GET","/voice")["used_today"] == 0
            (home/"voice.json").write_text("invalid")
            api(home,"GET","/voice",expected=500)
        finally:
            server.stop(other)
