"""End-to-end test of the agent: real server + real loop + tools, against fake AI model servers
(one speaking the Anthropic format, one the OpenAI format).  Run: python tests/test_agent.py"""
import http.client
import http.server
import json
import os
import re
import sys
import tempfile
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
HOME = tempfile.mkdtemp(prefix="wl-agent-")
os.environ["WINDYLAND_AGENT_HOME"] = HOME
UPLOADS = tempfile.mkdtemp(prefix="wl-uploads-")
os.environ["WINDYLAND_UPLOADS"] = UPLOADS

from windyagent import config, home, launcher, media, memory, router, tools   # noqa: E402
from windyagent.server import AgentServer                  # noqa: E402

WORK = Path(tempfile.mkdtemp(prefix="wl-work-"))
(WORK / "hello.txt").write_text("hi")


# ------------------------------------------------------------------ fake models
def decide(user_text, last_is_tool_result, tool_text):
    """What the fake model does: returns ('tool', name, args) or ('text', str)."""
    if last_is_tool_result:
        return ("text", "Done. Tool said: " + tool_text[:80].replace("\n", " "))
    if "list files" in user_text:
        return ("tool", "list_directory", {"path": str(WORK)})
    if "run echo" in user_text:
        return ("tool", "run_command", {"command": "echo agent-ok"})
    if "humanize" in user_text.lower():
        m = re.search(r"\[Attached file: (.+?)\]", user_text)
        return ("tool", "humanize_document", {"path": m.group(1), "open_in_word": False})
    return ("text", "Hello from the fake model.")


class FakeAnthropic(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        assert self.headers["x-api-key"] == "test-key" and self.path == "/v1/messages"
        last = body["messages"][-1]
        tr = isinstance(last["content"], list) and last["content"] and last["content"][0].get("type") == "tool_result"
        users = [m for m in body["messages"] if m["role"] == "user" and isinstance(m["content"], str)]
        d = decide(users[-1]["content"], tr, last["content"][0]["content"] if tr else "")
        if d[0] == "tool":
            content = [{"type": "text", "text": "Working on it."}, {"type": "tool_use", "id": "tu_%d" % time.time_ns(), "name": d[1], "input": d[2]}]
        else:
            content = [{"type": "text", "text": d[1]}]
        out = json.dumps({"content": content, "stop_reason": "tool_use" if d[0] == "tool" else "end_turn"}).encode()
        self.send_response(200); self.send_header("Content-Length", str(len(out))); self.end_headers(); self.wfile.write(out)


class FakeOpenAI(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        assert self.headers["Authorization"] == "Bearer test-key" and self.path == "/v1/chat/completions"
        msgs = body["messages"]
        assert msgs[0]["role"] == "system" and (not body.get("tools") or body["tools"][0]["type"] == "function")
        last = msgs[-1]
        tr = last["role"] == "tool"
        users = [m for m in msgs if m["role"] == "user"]
        d = decide(users[-1]["content"], tr, last["content"] if tr else "")
        if d[0] == "tool":
            msg = {"role": "assistant", "content": None, "tool_calls": [{"id": "call_1", "type": "function",
                   "function": {"name": d[1], "arguments": json.dumps(d[2])}}]}
        else:
            msg = {"role": "assistant", "content": d[1]}
        out = json.dumps({"choices": [{"message": msg}]}).encode()
        self.send_response(200); self.send_header("Content-Length", str(len(out))); self.end_headers(); self.wfile.write(out)


OLLAMA_SEEN: list[dict] = []


class FakeOllama(http.server.BaseHTTPRequestHandler):
    """Ollama's native /api/chat: tool calls come with dict arguments and no ids."""
    def log_message(self, *a): pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.path == "/api/generate":
            out = b"{}"
        else:
            assert self.path == "/api/chat" and body["stream"] is False, self.path
            OLLAMA_SEEN.append(body)
            msgs = body["messages"]
            last = msgs[-1]
            users = [m for m in msgs if m["role"] == "user"]
            d = decide(users[-1]["content"], last["role"] == "tool", last["content"] if last["role"] == "tool" else "")
            if d[0] == "tool":
                msg = {"role": "assistant", "content": "", "tool_calls": [{"function": {"name": d[1], "arguments": d[2]}}]}
            else:
                msg = {"role": "assistant", "content": d[1]}
            out = json.dumps({"message": msg, "done": True}).encode()
        self.send_response(200); self.send_header("Content-Length", str(len(out))); self.end_headers(); self.wfile.write(out)


def serve(handler):
    s = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=s.serve_forever, daemon=True).start()
    return s, s.server_address[1]


# ------------------------------------------------------------------ http helpers
class Client:
    def __init__(self, port, token):
        self.port, self.cookie = port, f"wl_agent={token}"

    def req(self, method, path, body=None, cookie=True, host=None, ctype="application/json"):
        c = http.client.HTTPConnection("127.0.0.1", self.port, timeout=30)
        h = {"Host": host or f"127.0.0.1:{self.port}"}
        if cookie: h["Cookie"] = self.cookie
        data = None
        if body is not None:
            data = json.dumps(body); h["Content-Type"] = ctype
        c.request(method, path, data, h)
        r = c.getresponse(); raw = r.read()
        try: j = json.loads(raw)
        except Exception: j = raw
        return r.status, j

    def wait_idle(self, timeout=60):
        t0 = time.time()
        while time.time() - t0 < timeout:
            st = self.req("GET", "/api/state")[1]
            if not st["busy"]:
                return st
            time.sleep(0.2)
        raise AssertionError("agent stayed busy")


def events(c, after=0):
    """Read the SSE stream for a moment and return the events."""
    conn = http.client.HTTPConnection("127.0.0.1", c.port, timeout=3)
    conn.request("GET", f"/api/events?after={after}", headers={"Cookie": c.cookie, "Host": f"127.0.0.1:{c.port}"})
    r = conn.getresponse(); out = []
    try:
        buf = b""
        t0 = time.time()
        while time.time() - t0 < 1.5:
            chunk = r.fp.readline()
            if not chunk: break
            buf += chunk
            if chunk.startswith(b"data: "):
                out.append(json.loads(chunk[6:]))
    except Exception:
        pass
    conn.close()
    return out


def run_style(provider, fake_handler):
    fake, fport = serve(fake_handler)
    config.update({"humanize_academi": False, "humanize_claude_check": False, "provider": provider, "api_key": "test-key", "model": "fake", "base_url": f"http://127.0.0.1:{fport}" + ("/v1" if provider != "anthropic" else ""),
                   "confirm_mode": "ask"})
    app = AgentServer()
    port = app.bind(48000)
    threading.Thread(target=app.serve, daemon=True).start()
    c = Client(port, config.load()["token"])
    print(f"== {provider} style on port {port}")

    # --- security
    assert c.req("GET", "/api/state", cookie=False)[0] == 403
    assert c.req("GET", "/api/state", host="evil.example:%d" % port)[0] == 403          # DNS rebinding
    assert c.req("POST", "/api/send", {"text": "x"}, ctype="text/plain")[0] in (403, 415)  # not JSON
    assert c.req("GET", "/api/ping")[1]["app"] == "windyland-agent"
    code, _ = c.req("GET", f"/?t={config.load()['token']}", cookie=False)
    assert code == 302
    code, page = c.req("GET", "/")
    assert code == 200 and b"<title>Tobi</title>" in page

    # --- settings never leak the key
    st = c.req("GET", "/api/state")[1]
    assert "test-key" not in json.dumps(st) and st["settings"]["has_key"] and st["settings"]["configured"]
    ok = c.req("POST", "/api/test", {})[1]
    assert ok["ok"], ok

    # --- plain chat
    assert c.req("POST", "/api/send", {"text": "explain in one sentence why the sky is blue"})[0] == 200
    st = c.wait_idle()
    assert any(i["kind"] == "assistant" and "fake model" in i["text"] for i in st["history"]), st["history"]

    # --- tool call loop (list_directory)
    c.req("POST", "/api/send", {"text": "please list files"}); st = c.wait_idle()
    tool = [i for i in st["history"] if i["kind"] == "tool"][-1]
    assert tool["name"] == "list_directory" and "hello.txt" in tool["result"], tool
    assert st["history"][-1]["kind"] == "assistant" and st["history"][-1]["text"].startswith("Done.")

    # --- the notch's own chat (Rust) talks to the server with raw HTTP over a socket: same bytes as agent.rs sends
    import socket as _s
    def raw(method, path, body=""):
        so = _s.create_connection(("127.0.0.1", port), timeout=10)
        so.sendall((f"{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: wl_agent={config.load()['token']}\r\n"
                    f"Content-Type: application/json\r\nContent-Length: {len(body)}\r\nConnection: close\r\n\r\n{body}").encode())
        data = b""
        while True:
            chunk = so.recv(65536)
            if not chunk: break
            data += chunk
        head, _, rest = data.decode().partition("\r\n\r\n")
        return int(head.split()[1]), rest
    code, body = raw("GET", "/api/state"); st0 = json.loads(body)
    assert code == 200 and "status" in st0 and "pending" in st0 and "history" in st0 and "configured" in st0["settings"], st0.keys()
    assert raw("POST", "/api/send", json.dumps({"text": "hi from the notch"}))[0] == 200
    c.wait_idle()
    assert any(i["kind"] == "user" and i["text"] == "hi from the notch" for i in json.loads(raw("GET", "/api/state")[1])["history"])
    assert raw("GET", "/api/state")[0] == 200 and raw("POST", "/api/stop", "{}")[0] == 200

    # --- confirmation: deny, then allow
    for approve in (False, True):
        c.req("POST", "/api/send", {"text": "run echo please"})
        for _ in range(100):
            pend = c.req("GET", "/api/state")[1]["pending"]
            if pend: break
            time.sleep(0.1)
        assert pend and pend[0]["detail"] == "echo agent-ok", pend
        c.req("POST", "/api/confirm", {"id": pend[0]["id"], "approve": approve})
        st = c.wait_idle()
        res = [i for i in st["history"] if i["kind"] == "tool" and i["name"] == "run_command"][-1]
        if approve: assert "agent-ok" in res["result"], res
        else: assert "user said no" in res["result"], res
    print("   chat, tools, approvals OK")

    # --- humanize through the real tool with a fake website client
    from windyagent.humanizer import runner
    sys.path.insert(0, str(Path(__file__).parent))
    from sample_doc import make_doc, FakeClient
    doc = WORK / f"thesis-{provider}.docx"; make_doc(doc)
    runner._make_client = lambda cfg, judge, log: FakeClient(cfg, judge, log)
    c.req("POST", "/api/send", {"text": f"[Attached file: {doc}]\nhumanize this file"}); st = c.wait_idle(90)
    res = [i for i in st["history"] if i["kind"] == "tool" and i["name"] == "humanize_document"][-1]
    assert '"complete": true' in res["result"] and '"sentences_changed": 3' in res["result"], res["result"]
    from docx import Document
    from docx.shared import RGBColor
    d = Document(str(doc))
    red = [r.text for p in d.paragraphs for r in p.runs if r.font.color and r.font.color.rgb == RGBColor(255, 0, 0)]
    assert len(red) == 3, red
    # the finished file is handed to the chat: a "file" event, a download link and an Open endpoint that only opens delivered files
    code, blob = c.req("GET", f"/files/{doc.name}")
    assert code == 200 and blob[:2] == b"PK", (code, blob[:20])                     # the .docx itself
    assert c.req("POST", "/api/open-file", {"name": doc.name})[0] in (200, 500)    # delivered: allowed (500 only if the OS cannot open it)
    assert c.req("POST", "/api/open-file", {"name": "config.json"})[0] == 404       # anything else: refused
    print("   humanize tool OK (3 sentences red)")
    hist = memory.humanize_history(5)
    assert hist and hist[-1]["complete"] and hist[-1]["sentences_changed"] == 3 and doc.name in hist[-1]["file"], hist
    assert "Humanizing finished" in (config.data_dir() / "notify.txt").read_text(encoding="utf-8")
    (config.data_dir() / "notify.txt").unlink()
    print("   humanize logged in Tobi's memory + notch notice OK")

    # --- upload (drag and drop in the chat window) and attach (drop on the notch)
    def upload(name, data, headers=None, cookie=True):
        cn = http.client.HTTPConnection("127.0.0.1", c.port, timeout=10)
        h = {"Host": f"127.0.0.1:{c.port}", "Content-Type": "application/octet-stream"}
        if cookie: h["Cookie"] = c.cookie
        if name is not None: h["X-Filename"] = name
        h.update(headers or {})
        cn.request("POST", "/api/upload", data, h)
        r = cn.getresponse(); raw = r.read(); cn.close()
        return r.status, json.loads(raw)
    code, j = upload("my%20thesis%20%C3%A9.docx", b"PK-fake-docx")
    assert code == 200 and Path(j["path"]).read_bytes() == b"PK-fake-docx" and Path(j["path"]).parent == Path(UPLOADS), j
    code, j2 = upload("my%20thesis%20%C3%A9.docx", b"second")
    assert code == 200 and j2["path"] != j["path"]                           # never overwrites
    code, j = upload("..%2F..%2Fevil.txt", b"x")
    assert code == 200 and Path(j["path"]).parent == Path(UPLOADS) and Path(j["path"]).name.startswith("evil") and ".." not in Path(j["path"]).name, j   # no path tricks
    assert upload(None, b"x")[0] == 400                                       # needs the custom header
    assert upload("a.txt", b"x", cookie=False)[0] == 403
    assert upload("a.txt", b"x", headers={"Origin": "http://evil.example"})[0] == 403
    assert upload("a.txt", b"")[0] == 413
    f1 = WORK / "dropped.docx"; f1.write_bytes(b"x")
    assert c.req("POST", "/api/attach", {"paths": [str(f1), "/does/not/exist"], "live": False})[1]["files"] == [str(f1)]
    st = c.req("GET", "/api/state")[1]
    assert st["attach"] == [str(f1)] and c.req("GET", "/api/state")[1]["attach"] == []   # delivered once
    last = c.req("GET", "/api/state")[1]["last_event"]
    c.req("POST", "/api/attach", {"paths": [str(f1)], "live": True})
    assert any(e["type"] == "attach" and e["paths"] == [str(f1)] for e in events(c, last))
    assert c.req("POST", "/api/attach", {"paths": []})[0] == 400
    print("   upload + attach OK")

    # --- reminders fire by themselves: notch notice + chat event
    last = c.req("GET", "/api/state")[1]["last_event"]
    memory.add_reminder("stretch your legs", time.time() - 1)
    for _ in range(40):
        if (config.data_dir() / "notify.txt").exists(): break
        time.sleep(0.25)
    assert "Reminder\tstretch your legs" in (config.data_dir() / "notify.txt").read_text(encoding="utf-8")
    assert any(e["type"] == "reminder" for e in events(c, last)) and memory.reminders() == []
    (config.data_dir() / "notify.txt").unlink()
    print("   reminder scheduler OK")

    # --- the system prompt is Tobi, with what he knows
    memory.remember("loves space documentaries", "interest")
    from windyagent.core import AgentCore
    sysmsg = app.core._system()
    assert "You are Tobi" in sysmsg and "loves space documentaries" in sysmsg and "thesis" in sysmsg, sysmsg[:300]
    print("   Tobi persona + memory in prompt OK")

    # --- stop button during a pending approval
    c.req("POST", "/api/send", {"text": "run echo again"}); time.sleep(0.8)
    c.req("POST", "/api/stop", {}); st = c.wait_idle(20)
    assert not st["pending"]

    # --- new chat
    c.req("POST", "/api/new", {}); assert c.req("GET", "/api/state")[1]["history"] == []

    # --- events stream carries things
    c.req("POST", "/api/send", {"text": "hi again"}); c.wait_idle()
    evs = events(c, 0)
    assert any(e["type"] == "assistant" for e in evs), [e["type"] for e in evs]

    c.req("POST", "/api/quit", {}); time.sleep(0.6)
    fake.shutdown()


def test_tools():
    ctx = tools.ToolContext({"confirm_mode": "ask"}, lambda *_: True, lambda *_: None, lambda *_: None, threading.Event())
    ok, out = tools.execute(ctx, "system_info", {}); assert ok and "cpu_percent" in out, out
    ok, out = tools.execute(ctx, "list_processes", {"limit": 3}); assert ok and out.count("\n") == 2, out
    ok, out = tools.execute(ctx, "find_files", {"root": str(WORK), "pattern": "*.txt"}); assert ok and "hello.txt" in out
    ok, out = tools.execute(ctx, "write_text_file", {"path": str(WORK / "n.txt"), "content": "abc"}); assert ok
    ok, out = tools.execute(ctx, "read_text_file", {"path": str(WORK / "n.txt")}); assert out == "abc"
    ok, out = tools.execute(ctx, "delete_path", {"path": str(WORK / "n.txt")}); assert ok and not (WORK / "n.txt").exists() and "trash" in out
    ok, out = tools.execute(ctx, "read_text_file", {"path": str(config.data_dir() / "config.json")}); assert "off limits" in out
    ok, out = tools.execute(ctx, "fetch_url", {"url": "http://127.0.0.1:1/"}); assert "not a public" in out
    ok, out = tools.execute(ctx, "fetch_url", {"url": "file:///etc/passwd"}); assert "Only http" in out
    ok, out = tools.execute(ctx, "nope", {}); assert not ok
    ok, out = tools.execute(ctx, "list_directory", {"bogus": 1}); assert not ok and "Bad arguments" in out
    deny = tools.ToolContext({"confirm_mode": "ask"}, lambda *_: False, lambda *_: None, lambda *_: None, threading.Event())
    ok, out = tools.execute(deny, "run_command", {"command": "echo hi"}); assert not ok and "said no" in out
    print("tools OK")


YT_PAGE = ("<html><script>var ytInitialData = " + json.dumps({"contents": {"x": [
    {"videoRenderer": {"videoId": "AAAAAAAAAAA", "title": {"runs": [{"text": "Short song"}]}, "ownerText": {"runs": [{"text": "Band"}]},
                       "lengthText": {"simpleText": "3:37"}, "viewCountText": {"simpleText": "1 views"}}},
    {"videoRenderer": {"videoId": "LIVE0000000", "title": {"runs": [{"text": "Live now"}]}}},                       # no length: skipped
    {"videoRenderer": {"videoId": "BBBBBBBBBBB", "title": {"runs": [{"text": "Great doc"}]}, "ownerText": {"runs": [{"text": "Docs"}]},
                       "lengthText": {"simpleText": "1:02:03"}}},
    {"videoRenderer": {"videoId": "AAAAAAAAAAA", "title": {"runs": [{"text": "dup"}]}, "lengthText": {"simpleText": "1:00"}}},
]}}) + ";</script></html>")


def test_media_and_home():
    ctx = tools.ToolContext({"confirm_mode": "ask"}, lambda *_: True, lambda *_: None, lambda *_: None, threading.Event())
    # YouTube parser
    r = media.parse_youtube_results(YT_PAGE)
    assert [v["id"] for v in r] == ["AAAAAAAAAAA", "BBBBBBBBBBB"] and r[1]["seconds"] == 3723 and r[0]["seconds"] == 217, r
    assert media.parse_youtube_results("<html>nothing</html>") == []
    opened = []
    media._open_url = lambda u: opened.append(u)
    media.youtube_search = lambda q: media.parse_youtube_results(YT_PAGE)
    out = media.play_youtube(ctx, "some song")
    assert opened[-1] == "https://www.youtube.com/watch?v=AAAAAAAAAAA" and "Short song" in out
    out = media.play_youtube(ctx, "documentary", min_minutes=30)
    assert opened[-1].endswith("BBBBBBBBBBB"), opened
    out = media.play_youtube(ctx, "documentary", min_minutes=500)
    assert "search_query=documentary" in opened[-1] and "No video matched" in out
    # play_something: prefers remembered interests, skips what was played, falls back to topics
    memory.remember("deep sea creatures", "interest")
    seen_queries = []
    def fake_search(q):
        seen_queries.append(q)
        return [{"id": "ZZZZZZZZZZZ", "title": "Deep sea", "channel": "Ch", "length": "45:00", "seconds": 2700, "views": ""}]
    media.youtube_search = fake_search
    out = media.play_something(ctx, "eating")
    assert opened[-1].endswith("ZZZZZZZZZZZ") and seen_queries, out
    out = media.play_something(ctx, "eating", topic="volcanoes")           # ZZZ already played -> nothing new
    assert "Could not find" in out and seen_queries[-1] == "volcanoes"
    # anime resolver (offline copies of the site's structure)
    SEARCH = ('<a href="https://animesalt.cx/series/naruto-shippuden/">x</a><a href="https://animesalt.cx/series/naruto/">x</a>'
              '<a href="https://animesalt.cx/series/boruto/">x</a><a href="https://animesalt.cx/series/naruto/">dup</a>')
    SERIES = "".join(f'<a href="https://animesalt.cx/episode/naruto-{s}x{e}/">e</a>' for s in (1, 2) for e in (1, 2, 3))
    media._get = lambda url, *a, **k: SEARCH if "?s=" in url else SERIES
    c = media.find_series("https://animesalt.cx", "naruto")
    assert c[0][2].endswith("/series/naruto/") and len(c) == 3, c
    assert [e[:2] for e in media.episodes_of("https://animesalt.cx/series/naruto/")] == [(1, 1), (1, 2), (1, 3), (2, 1), (2, 2), (2, 3)]
    played = []
    media.open_and_press_play = lambda url, cfg: (played.append(url), "pressed play")[1]
    out = media.play_anime(ctx, "Naruto")
    assert played[-1] == "https://animesalt.cx/episode/naruto-1x1/" and "season 1 episode 1" in out, out
    media.play_anime(ctx, "Naruto", season=2, episode=3); assert played[-1].endswith("naruto-2x3/")
    media.play_anime(ctx, "Naruto", which="latest"); assert played[-1].endswith("naruto-2x3/")
    out = media.play_anime(ctx, "Naruto", season=9, episode=1); assert "no season 9 episode 1" in out and not played[-1].endswith("9x1/")
    media._get = lambda *a, **k: "<html>nothing</html>"
    opened.clear(); out = media.play_anime(ctx, "zzzz"); assert "No result" in out and "?s=zzzz" in opened[0]
    print("media OK (YouTube parser, play_youtube, play_something, anime resolver)")

    # memory tools
    assert "Saved" in home.remember(ctx, "likes lo-fi beats while studying", "interest")
    assert "lo-fi" in home.recall(ctx, "lo-fi") and "Nothing saved" in home.recall(ctx, "zzzz")
    assert home.remember(ctx, "likes lo-fi beats while studying", "interest").startswith("Saved") and len(memory.recall("lo-fi")) == 1   # no duplicates
    assert "Forgot" in home.forget(ctx, "lo-fi") and "nothing matching" in home.forget(ctx, "lo-fi")
    # reminders
    assert "Reminder #" in home.set_reminder(ctx, "call mum", in_minutes=30)
    assert "past" in home.set_reminder(ctx, "x", at="2001-01-01 10:00")
    assert "Say when" in home.set_reminder(ctx, "x")
    assert "call mum" in home.list_reminders(ctx)
    rid = memory.reminders()[0]["id"]
    assert home.cancel_reminder(ctx, rid) == "Cancelled." and "No reminders" in home.list_reminders(ctx)
    # weather (network mocked)
    def fake_json(url, timeout=15):
        if "geocoding" in url:
            return {"results": [{"name": "Dhaka", "admin1": "Dhaka", "country": "Bangladesh", "latitude": 23.7, "longitude": 90.4}]}
        return {"current": {"temperature_2m": 31, "apparent_temperature": 36, "relative_humidity_2m": 70, "wind_speed_10m": 9, "weather_code": 2},
                "daily": {"time": ["2026-10-29", "2026-10-30", "2026-10-31"], "weather_code": [2, 61, 95], "temperature_2m_max": [32, 30, 29],
                          "temperature_2m_min": [25, 24, 24], "precipitation_probability_max": [10, 60, 80]}}
    home._json = fake_json
    w = home.weather(ctx, "Dhaka"); assert "Dhaka" in w and "31°C" in w and "thunderstorm" in w, w
    # tidy folder: preview first, then apply with approval; never deletes; skips fresh files
    dl = WORK / "Downloads"; dl.mkdir(exist_ok=True)
    old = time.time() - 3600
    for n in ("a.pdf", "b.jpg", "c.mp3", "d.zip", "e.unknownext", "f.mp4"):
        (dl / n).write_text("x"); os.utime(dl / n, (old, old))
    (dl / "fresh.pdf").write_text("x")
    out = home.organize_folder(ctx, str(dl)); assert "would move 6" in out and (dl / "a.pdf").exists(), out
    deny = tools.ToolContext({"confirm_mode": "ask"}, lambda *_: False, lambda *_: None, lambda *_: None, threading.Event())
    ok, out = tools.execute(deny, "organize_folder", {"folder": str(dl), "apply": True}); assert not ok and (dl / "a.pdf").exists()
    out = home.organize_folder(ctx, str(dl), apply=True); assert "Moved 6 of 6" in out
    assert (dl / "Documents" / "a.pdf").exists() and (dl / "Pictures" / "b.jpg").exists() and (dl / "Other" / "e.unknownext").exists() and (dl / "fresh.pdf").exists()
    assert "only tidy one specific folder" in home.organize_folder(ctx, str(Path.home()))
    # Windows-only things say so instead of crashing
    if os.name != "nt":
        assert "only available on Windows" in home.power(ctx, "lock") and "only available on Windows" in media.media_control(ctx, "mute")
    assert "Unknown action" in media.media_control(ctx, "explode")
    assert "action must be" in home.power(ctx, "explode")
    # every new tool is registered with a valid schema
    names = {t["name"] for t in tools.TOOLS}
    for n in ("play_youtube", "play_something", "play_anime", "media_control", "weather", "remember", "recall", "forget", "humanize_history",
              "set_reminder", "list_reminders", "cancel_reminder", "power", "organize_folder"):
        assert n in names, n
    for t in tools.TOOLS:
        assert t["schema"]["type"] == "object" and t["description"], t["name"]
    print("home tools OK (memory, reminders, weather, tidy, power)")


def test_router():
    r = router.route
    doc = "C:\\Users\\me\\Documents\\Tobi\\Uploads\\AJ 12197.docx"
    # the exact case from the screenshot: attached file + "humanize this"
    assert r(f"[Attached file: {doc}]\nhumanize this") == [{"tool": "humanize_document", "args": {"path": doc}}]
    assert r(f"[Attached file: {doc}]\nHumanize this file please")[0]["tool"] == "humanize_document"
    assert [x["tool"] for x in r("Humanize this file")] == ["pick_file", "humanize_document"]
    assert r("what have you humanized so far?")[0]["tool"] == "humanize_history"
    assert r(f"[Attached file: {doc}]\nsummarize this") is None                  # attached for something else -> the model
    # media
    assert r("Play Naruto") == [{"tool": "play", "args": {"query": "Naruto"}}]
    assert r("play naruto season 2 episode 5 on anime") == [{"tool": "play_anime", "args": {"season": 2, "episode": 5, "title": "naruto"}}]
    assert r("play the latest one piece episode")[0]["args"] == {"which": "latest", "title": "one piece"}
    assert r("Play Believer by Imagine Dragons")[0] == {"tool": "play_youtube", "args": {"query": "Believer by Imagine Dragons"}}
    assert r("play some lofi music")[0]["tool"] == "play_youtube"
    assert r("play despacito on youtube") == [{"tool": "play_youtube", "args": {"query": "despacito"}}]
    assert r("I am eating, play something")[0]["tool"] == "play_something"
    assert r("I'm eating, put on something interesting")[0]["tool"] == "play_something"
    assert r("play something about space") == [{"tool": "play_something", "args": {"topic": "space"}}]
    assert r("surprise me")[0]["tool"] == "play_something"
    assert r("pause") == [{"tool": "media_control", "args": {"action": "play_pause"}}]
    assert r("volume down a bit") == [{"tool": "media_control", "args": {"action": "volume_down", "steps": 3}}]
    assert r("next song")[0]["args"] == {"action": "next"}
    # reminders, weather, memory, power, pc
    assert r("remind me in 20 minutes to stretch") == [{"tool": "set_reminder", "args": {"text": "Stretch", "in_minutes": 20.0}}]
    assert r("remind me in an hour to call mum")[0]["args"]["in_minutes"] == 60.0
    assert r("set a timer for 10 minutes")[0]["args"]["in_minutes"] == 10.0
    assert r("what's the weather in Dhaka?") == [{"tool": "weather", "args": {"city": "Dhaka"}}]
    assert r("I love space documentaries") == [{"tool": "remember", "args": {"text": "likes space documentaries", "kind": "interest"}}]
    assert r("remember that my exam is on Friday") == [{"tool": "remember", "args": {"text": "my exam is on Friday", "kind": "fact"}}]
    assert r("what do you know about me?")[0]["tool"] == "recall"
    assert r("forget the space thing")[0] == {"tool": "forget", "args": {"what": "the space thing"}}
    assert r("lock my pc")[0]["args"] == {"action": "lock"} and r("shut down in 30 minutes")[0]["args"] == {"action": "shutdown", "delay_minutes": 30}
    assert r("cancel the shutdown")[0]["args"] == {"action": "cancel"}
    assert r("take a screenshot")[0]["tool"] == "screenshot" and r("open spotify") == [{"tool": "open_app", "args": {"name": "spotify"}}]
    # things for the model
    for q in ("hello", "why is the sky blue?", "write me a poem about rain", "I like to move it, can you explain this code?", "open the file and tell me what it says and then delete it"):
        assert r(q) is None, q
    # one-line answers
    out = router.summarize("humanize_document", True, json.dumps({"ok": True, "complete": True, "sentences_changed": 12, "output": "C:/x/thesis.docx"}))
    assert out.startswith("Done!") and "12 sentences" in out and "thesis.docx" in out
    assert "won't" in router.summarize("power", False, "The user said no. Do not retry")
    print("router OK")


def test_tool_selection_and_judge():
    names = tools.select_names("please list files in my downloads")
    assert {"run_command", "open_target", "list_directory"} <= names and "play_anime" not in names and "weather" not in names
    assert "humanize_document" in tools.select_names("humanize my thesis.docx")
    assert {"play", "media_control"} <= tools.select_names("play something")
    assert len(tools.select_names("zzz")) >= 6                                   # nothing recognised -> a general set
    sm = tools.schemas(select_for="what's the weather", compact=True)
    assert [t["name"] for t in sm if t["name"] == "weather"] and len(sm) < 8
    assert len(json.dumps(tools.schemas(select_for="play naruto", compact=True))) < len(json.dumps(tools.schemas())) / 3
    # judge: rules decide clear cases alone, the model is asked only about close calls, and gets a short question
    from windyagent.humanizer.judge import Candidate, JudgeRequest, LLMJudge
    class CountingLLM:
        def __init__(self, reply): self.n, self.reply, self.last = 0, reply, ""
        def ask(self, system, user, max_tokens=500): self.n += 1; self.last = user; return self.reply
    orig = "Climate change affects coastal cities in 2020 through rising seas."
    good = Candidate(1, "Option 1", "Climate change affects coastal cities in 2020 through rising seas and floods.")
    bad = Candidate(2, "Option 2", "Seas rising and")
    llm = CountingLLM('{"scores": {"1": 9, "2": 2}}')
    j = LLMJudge(llm, mode="best")
    d = j.choose(JudgeRequest(orig, [good, bad]))
    assert d.idx == 1 and llm.n == 1, (d, llm.n)                                 # the model marks every option 1-10
    close1 = Candidate(1, "Option 1", "Rising seas affect coastal cities in 2020 through climate change.")
    close2 = Candidate(2, "Option 2", "In 2020 climate change affects coastal cities by rising seas.")
    llm = CountingLLM("2"); j = LLMJudge(llm, mode="best")                       # a small model answering with a bare number
    d = j.choose(JudgeRequest(orig, [close1, close2]))
    assert llm.n == 1 and d.idx == 2, (d, llm.n)
    llm = CountingLLM('{"best": 1}'); j = LLMJudge(llm, mode="best")
    j.choose(JudgeRequest(orig, [good, bad])); assert llm.n == 1                 # "best" always asks
    llm = CountingLLM('{"scores": {"1": 5, "2": 6}}'); j = LLMJudge(llm, mode="best")
    assert j.choose(JudgeRequest(orig, [close1, close2])).idx is None             # nothing marked 8+ -> ask for new alternatives
    llm = CountingLLM('{"scores": {"1": 5, "2": 7}}'); j = LLMJudge(llm, mode="best")
    assert j.choose(JudgeRequest(orig, [close1, close2], force=True)).idx == 2     # last try accepts 6+
    llm = CountingLLM('{"scores": {"1": 3, "2": 4}}'); j = LLMJudge(llm, mode="best")
    assert j.choose(JudgeRequest(orig, [close1, close2], force=True)).idx is None  # poor options: kept as it was
    assert j.choose(JudgeRequest(orig, [close1, close2], force=True, must_replace=True)).idx == 2  # unless a red sentence must go
    # play: anime if the site has the title, otherwise YouTube
    ctx = tools.ToolContext({"confirm_mode": "ask"}, lambda *_: True, lambda *_: None, lambda *_: None, threading.Event())
    calls = []
    media.play_anime = lambda c, title, **k: calls.append(("anime", title)) or "anime"
    media.play_youtube = lambda c, query, **k: calls.append(("yt", query)) or "yt"
    media.find_series = lambda site, t: [(1.26, "series", "https://animesalt.cx/series/naruto/")] if "naruto" in t.lower() else []
    media.play_auto(ctx, "Naruto"); media.play_auto(ctx, "Believer"); media.play_auto(ctx, "naru")
    assert calls == [("anime", "Naruto"), ("yt", "Believer"), ("yt", "naru")], calls
    print("tool selection, judge, play OK")


def test_stop_asks_whether_to_keep_the_work_so_far():
    from windyagent.humanizer import runner
    f = WORK / "half.docx"; f.write_bytes(b"x"); part = WORK / "half_partial.docx"; part.write_bytes(b"y")
    asked, files = [], []
    cancel = threading.Event(); cancel.set()                                   # the red button was pressed
    ctx = tools.ToolContext({"confirm_mode": "auto"}, lambda t, d, labels=None, force=False: asked.append((t, labels, force)) or True,
                            lambda *_: None, lambda *_: None, cancel, None, files.append)
    orig = runner.run_job
    runner.run_job = lambda *a, **k: {"ok": True, "cancelled": True, "complete": False, "paragraphs_done": 2, "paragraphs_total": 5, "output": str(part)}
    try:
        out = tools.humanize_document(ctx, str(f))
    finally:
        runner.run_job = orig
    assert asked and asked[0][0] == "You stopped the task" and asked[0][2] is True and asked[0][1][0].startswith("Yes"), asked   # asked although stop was pressed
    assert files == [part] and not cancel.is_set() and json.loads(out)["kept"], (files, out)
    assert "work so far" in router.summarize("humanize_document", True, out)
    print("stop button -> 'do you want the work so far?' -> the partial file is delivered")


def test_ollama():
    """Tobi on a small local model: native API, bigger context, few tools, short prompt; plain commands skip the model."""
    fake, fport = serve(FakeOllama)
    config.update({"provider": "ollama", "api_key": "", "model": "qwen2.5:7b", "base_url": f"http://127.0.0.1:{fport}/v1", "confirm_mode": "ask"})
    assert config.resolved(config.load())["base_url"] == f"http://127.0.0.1:{fport}"          # old /v1 address still works
    assert config.is_configured(config.load()) and config.is_local(config.load())
    app = AgentServer(); port = app.bind(48100)
    threading.Thread(target=app.serve, daemon=True).start()
    c = Client(port, config.load()["token"])
    assert c.req("POST", "/api/test", {})[1]["ok"]
    OLLAMA_SEEN.clear()
    c.req("POST", "/api/send", {"text": "please list files"}); st = c.wait_idle()
    tool = [i for i in st["history"] if i["kind"] == "tool"][-1]
    assert tool["name"] == "list_directory" and "hello.txt" in tool["result"]
    first = OLLAMA_SEEN[0]
    assert first["options"]["num_ctx"] == 8192 and first["keep_alive"] == "30m", first["options"]
    names = {t["function"]["name"] for t in first["tools"]}
    assert "list_directory" in names and "play_anime" not in names and len(names) < 12, names
    assert len(first["messages"][0]["content"]) < 1800 and "You are Tobi" in first["messages"][0]["content"]
    second = OLLAMA_SEEN[1]                                                             # tool result goes back in Ollama's format
    assert second["messages"][-1]["role"] == "tool" and second["messages"][-1]["tool_name"] == "list_directory"
    assert second["messages"][-2]["tool_calls"][0]["function"]["arguments"] == {"path": str(WORK)}
    # the screenshot case: attached file + "humanize this" never reaches the model, never opens the file picker
    from windyagent.humanizer import runner
    sys.path.insert(0, str(Path(__file__).parent))
    from sample_doc import make_doc, FakeClient
    doc = WORK / "AJ 12197.docx"; make_doc(doc)
    runner._make_client = lambda cfg, judge, log: FakeClient(cfg, judge, log)
    OLLAMA_SEEN.clear()
    c.req("POST", "/api/send", {"text": f"[Attached file: {doc}]\nhumanize this"}); st = c.wait_idle(90)
    assert OLLAMA_SEEN == [], "the model should not be asked"
    names = [i["name"] for i in st["history"] if i["kind"] == "tool"]
    assert names[-1] == "humanize_document" and "pick_file" not in names, names
    assert st["history"][-1]["kind"] == "assistant" and st["history"][-1]["text"].startswith("Done!"), st["history"][-1]
    # and chatting afterwards still works (history stayed valid)
    c.req("POST", "/api/send", {"text": "tell me something about volcanoes"}); st = c.wait_idle()
    assert "fake model" in st["history"][-1]["text"] and OLLAMA_SEEN
    # unreachable Ollama gives a clear message
    config.update({"base_url": "http://127.0.0.1:9"})
    c.req("POST", "/api/send", {"text": "tell me more about volcanoes"}); c.wait_idle()
    evs = events(c, 0)
    assert any(e["type"] == "error" and "Ollama is not running" in e["text"] for e in evs), [e for e in evs if e["type"] == "error"]
    c.req("POST", "/api/quit", {}); time.sleep(0.5); fake.shutdown()
    print("ollama OK (native API, context, tool selection, routed humanize without the model)")


def test_launcher():
    assert launcher._arg_list(["--rect", "1,2,3,4", "--attach", "a.docx", "b b.pdf", "--settings"], "--attach") == ["a.docx", "b b.pdf"]
    assert launcher._arg_list(["--rect", "1,2,3,4"], "--attach") == []
    x, y, w, h = launcher.popup_geometry((400, 0, 1200, 90))
    assert (w, h) == (launcher.WIN_W, launcher.WIN_H) or os.name == "nt"
    assert launcher.find_popup() == 0 or os.name == "nt"
    print("launcher helpers OK")


if __name__ == "__main__":
    test_tools()
    test_media_and_home()
    test_router()
    test_tool_selection_and_judge()
    test_stop_asks_whether_to_keep_the_work_so_far()
    test_launcher()
    run_style("anthropic", FakeAnthropic)
    run_style("openai", FakeOpenAI)
    test_ollama()
    print("ALL AGENT TESTS PASSED")
