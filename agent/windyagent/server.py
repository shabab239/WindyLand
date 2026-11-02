"""The agent's local web server: serves the chat window and its JSON/SSE API on 127.0.0.1 only.

Security: every call needs the secret token (set as an HttpOnly cookie when the window is opened with
?t=<token>), the Host header must be localhost (blocks DNS rebinding), and POSTs must be JSON
(a web page in another tab cannot send those without permission).
"""
from __future__ import annotations

import hmac
import json
import mimetypes
import os
import socket
import subprocess
import sys
import threading
import time
import urllib.parse
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from . import config, home, tools
from .core import AgentCore, Hub
from .llm import LLM, LLMError

WEB = Path(__file__).parent / "web"
COOKIE = "wl_agent"
UPLOAD_MAX = 300_000_000


def uploads_dir() -> Path:
    """Files given to Tobi through the chat window land here (visible in Documents, so nothing is hidden)."""
    p = Path(os.environ.get("WINDYLAND_UPLOADS") or (Path.home() / "Documents" / "Tobi" / "Uploads"))
    p.mkdir(parents=True, exist_ok=True)
    return p


class Handler(BaseHTTPRequestHandler):
    server_version = "WindyAgent"
    protocol_version = "HTTP/1.1"

    # ------------------------------------------------------------------ plumbing
    def log_message(self, fmt, *args):  # quiet
        pass

    @property
    def app(self) -> "AgentServer":
        return self.server.app  # type: ignore[attr-defined]

    def _send(self, code: int, body: bytes, ctype: str = "application/json", headers: dict | None = None):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Referrer-Policy", "no-referrer")
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)

    def _json(self, obj, code: int = 200):
        self._send(code, json.dumps(obj, ensure_ascii=False).encode("utf-8"))

    def _host_ok(self) -> bool:
        host = (self.headers.get("Host") or "").lower()
        port = str(self.app.port)
        if host not in (f"127.0.0.1:{port}", f"localhost:{port}"):
            return False
        origin = self.headers.get("Origin")
        return not origin or origin in (f"http://127.0.0.1:{port}", f"http://localhost:{port}")

    def _cookie(self) -> str:
        for part in (self.headers.get("Cookie") or "").split(";"):
            k, _, v = part.strip().partition("=")
            if k == COOKIE:
                return v
        return ""

    def _authed(self) -> bool:
        return self._host_ok() and hmac.compare_digest(self._cookie(), self.app.token())

    def _body(self) -> dict:
        n = int(self.headers.get("Content-Length") or 0)
        if n > 2_000_000:
            return {}
        try:
            return json.loads(self.rfile.read(n) or b"{}")
        except Exception:
            return {}

    # ------------------------------------------------------------------ GET
    def do_GET(self):
        u = urllib.parse.urlparse(self.path)
        path, q = u.path, urllib.parse.parse_qs(u.query)
        if path == "/api/ping":
            if not self._host_ok():
                return self._json({"ok": False}, 403)
            return self._json({"ok": True, "app": "windyland-agent", "busy": self.app.core.busy})
        if path == "/":
            if not self._host_ok():
                return self._send(403, b"Forbidden", "text/plain")
            t = (q.get("t") or [""])[0]
            if t and hmac.compare_digest(t, self.app.token()):
                dest = "/" + ("#settings" if (q.get("settings") or [""])[0] else "")
                return self._send(302, b"", "text/plain", {
                    "Location": dest,
                    "Set-Cookie": f"{COOKIE}={t}; Path=/; HttpOnly; SameSite=Strict",
                })
            if not self._authed():
                return self._send(403, b"Open Tobi from WindyLand (the Tobi tab > Chat with Tobi).", "text/plain")
            return self._send(200, (WEB / "index.html").read_bytes(), "text/html; charset=utf-8")
        if path == "/icon.png":
            f = WEB / "icon.png"
            return self._send(200, f.read_bytes(), "image/png") if f.exists() else self._send(404, b"", "text/plain")
        if not self._authed():
            return self._json({"error": "not allowed"}, 403)
        if path == "/api/state":
            cfg = config.load()
            return self._json({
                "settings": config.public(cfg), "busy": self.app.core.busy, "status": self.app.core.status_line if self.app.core.busy else "", "last_event": self.app.hub.last_id,
                "history": self.app.core.history_items(), "pending": self.app.core.pending_confirms(),
                "attach": self.app.take_attach(), "uploads": str(uploads_dir()),
            })
        if path == "/api/events":
            return self._events(int((q.get("after") or ["0"])[0] or 0))
        if path.startswith("/files/"):
            name = Path(urllib.parse.unquote(path[7:])).name
            f = config.data_dir() / "screens" / name
            if not f.is_file():
                f = config.data_dir() / "deliver" / name
            if f.is_file():
                extra = {} if f.parent.name == "screens" else {"Content-Disposition": f'attachment; filename="{urllib.parse.quote(name)}"'}
                return self._send(200, f.read_bytes(), mimetypes.guess_type(name)[0] or "application/octet-stream", extra)
            return self._send(404, b"", "text/plain")
        self._json({"error": "not found"}, 404)

    def _events(self, after: int):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Connection", "close")
        self.end_headers()
        self.close_connection = True
        try:
            last = after
            while not self.app.stopping:
                evs = self.app.hub.wait(last, 15.0)
                if not evs:
                    self.wfile.write(b": keep-alive\n\n")
                    self.wfile.flush()
                    continue
                for e in evs:
                    last = e["seq"]
                    self.wfile.write(f"id: {e['seq']}\ndata: {json.dumps(e, ensure_ascii=False)}\n\n".encode("utf-8"))
                self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError, OSError):
            pass

    # ------------------------------------------------------------------ POST
    def do_POST(self):
        path = urllib.parse.urlparse(self.path).path
        raw_len = int(self.headers.get("Content-Length") or 0)
        if not self._authed():
            self.close_connection = True
            return self._json({"error": "not allowed"}, 403)
        if path == "/api/upload":
            return self._upload(raw_len)
        if "application/json" not in (self.headers.get("Content-Type") or ""):
            self.close_connection = True
            return self._json({"error": "JSON only"}, 415)
        d = self._body()
        core = self.app.core
        if path == "/api/send":
            text = str(d.get("text") or "").strip()
            if not text:
                return self._json({"error": "empty"}, 400)
            if not core.send(text):
                return self._json({"error": "The agent is busy. Press Stop first."}, 409)
            return self._json({"ok": True})
        if path == "/api/attach":
            paths = [str(x) for x in (d.get("paths") or []) if isinstance(x, str)][:20]
            paths = [x for x in paths if os.path.isfile(x)]
            if not paths:
                return self._json({"error": "no readable files"}, 400)
            self.app.attach(paths, live=bool(d.get("live")))
            return self._json({"ok": True, "files": paths})
        if path == "/api/ui":
            if d.get("action") == "settings":
                self.app.hub.emit("ui", action="settings")
            return self._json({"ok": True})
        if path == "/api/open-file":
            target = core.delivered.get(Path(str(d.get("name") or "")).name)
            if not target or not os.path.isfile(target):
                return self._json({"error": "not a file Tobi delivered"}, 404)
            try:
                if d.get("reveal") and config.IS_WIN:
                    subprocess.Popen(["explorer", "/select,", target])
                elif config.IS_WIN:
                    os.startfile(target)  # type: ignore[attr-defined]
            except OSError as e:
                return self._json({"error": str(e)}, 500)
            return self._json({"ok": True})
        if path == "/api/stop":
            core.stop()
            return self._json({"ok": True})
        if path == "/api/new":
            core.new_chat()
            return self._json({"ok": True})
        if path == "/api/confirm":
            return self._json({"ok": core.answer(str(d.get("id", "")), bool(d.get("approve")))})
        if path == "/api/pick-file":
            ctx = tools.ToolContext(config.load(), lambda *_: True, lambda *_: None, lambda *_: None, threading.Event())
            return self._json({"path": tools.pick_file(ctx)})
        if path == "/api/settings":
            return self._json(self._save_settings(d))
        if path == "/api/test":
            return self._json(self._test())
        if path == "/api/quit":
            self._json({"ok": True})
            threading.Thread(target=self.app.shutdown, daemon=True).start()
            return
        self._json({"error": "not found"}, 404)

    def _upload(self, length: int):
        """A file dragged into the chat: raw bytes, name in X-Filename (a custom header, so other web pages cannot send it)."""
        raw_name = self.headers.get("X-Filename")
        if not raw_name:
            self.close_connection = True
            return self._json({"error": "missing X-Filename"}, 400)
        if length <= 0 or length > UPLOAD_MAX:
            self.close_connection = True
            return self._json({"error": f"File is empty or larger than {UPLOAD_MAX // 1_000_000} MB."}, 413)
        name = "".join(c for c in Path(urllib.parse.unquote(raw_name)).name if c not in '<>:"/\\|?*\x00')[:120].strip(". ") or "file"
        folder = uploads_dir()
        dest = folder / name
        i = 1
        while dest.exists():
            dest = folder / f"{Path(name).stem} ({i}){Path(name).suffix}"
            i += 1
        left = length
        try:
            with open(dest, "wb") as f:
                while left > 0:
                    chunk = self.rfile.read(min(left, 1 << 20))
                    if not chunk:
                        raise OSError("connection closed")
                    f.write(chunk)
                    left -= len(chunk)
        except Exception as e:
            dest.unlink(missing_ok=True)
            self.close_connection = True
            return self._json({"error": f"Upload failed: {e}"}, 500)
        self._json({"ok": True, "path": str(dest), "name": dest.name, "size": length})

    def _save_settings(self, d: dict) -> dict:
        cur = config.load()
        ch: dict = {}
        if d.get("provider") in config.PROVIDERS:
            ch["provider"] = d["provider"]
        for k in ("model", "base_url"):
            if k in d:
                ch[k] = str(d[k]).strip()
        if "anime_site" in d:
            site = str(d["anime_site"]).strip().rstrip("/")
            ch["anime_site"] = site if site.startswith(("https://", "http://")) else config.DEFAULTS["anime_site"]
        if d.get("review_engine") in ("auto", "tobi"):
            ch["review_engine"] = d["review_engine"]
        if "browser" in d:
            b = str(d["browser"]).strip().lower()
            from .humanizer.stealth_client import BROWSERS
            ch["humanizer_browser"] = b if (b in BROWSERS or b == "auto") else "auto"
        if str(d.get("api_key") or "").strip():
            ch["api_key"] = str(d["api_key"]).strip()
        if d.get("clear_key"):
            ch["api_key"] = ""
        if d.get("confirm_mode") in ("ask", "auto"):
            ch["confirm_mode"] = d["confirm_mode"]
        ch["setup_done"] = True
        cur = config.update(ch)
        threading.Thread(target=self.app.warm_up, daemon=True).start()
        return {"ok": True, "settings": config.public(cur)}

    def _test(self) -> dict:
        cfg = config.load()
        if not config.is_configured(cfg):
            return {"ok": False, "error": "Enter the provider, model and API key first."}
        try:
            t0 = time.time()
            out = LLM(cfg).ask("Reply with the single word: OK", "ping", max_tokens=20)
            return {"ok": True, "reply": out.strip()[:60], "ms": int((time.time() - t0) * 1000)}
        except LLMError as e:
            return {"ok": False, "error": str(e)}
        except Exception as e:
            return {"ok": False, "error": f"{e.__class__.__name__}: {e}"}


class AgentServer:
    def __init__(self):
        self.hub = Hub()
        self.core = AgentCore(self.hub)
        self.stopping = False
        self.httpd: ThreadingHTTPServer | None = None
        self.port = 0
        self._token = config.load()["token"]
        self._attach: list[str] = []
        self._attach_lock = threading.Lock()

    def token(self) -> str:
        return self._token

    def attach(self, paths: list[str], live: bool) -> None:
        """Files dropped on the notch. An open chat window gets an event; otherwise they wait for the window to load."""
        if live:
            self.hub.emit("attach", paths=paths)
        else:
            with self._attach_lock:
                self._attach += [p for p in paths if p not in self._attach]

    def take_attach(self) -> list[str]:
        with self._attach_lock:
            out, self._attach = self._attach, []
        return out

    def warm_up(self):
        """A local (Ollama) model takes a while to load; do it now so the first chat is quick."""
        try:
            cfg = config.load()
            if config.is_local(cfg):
                LLM(cfg).warm_up()
        except Exception:
            pass

    def reminder_loop(self):
        while not self.stopping:
            try:
                home.fire_due(self.hub)
            except Exception as e:
                print("reminder error:", e, flush=True)
            for _ in range(10):
                if self.stopping:
                    return
                time.sleep(0.5)

    def bind(self, preferred: int) -> int:
        for port in range(preferred, preferred + 25):
            try:
                srv = ThreadingHTTPServer(("127.0.0.1", port), Handler)
            except OSError:
                continue
            srv.daemon_threads = True
            srv.app = self  # type: ignore[attr-defined]
            self.httpd, self.port = srv, port
            return port
        raise OSError("No free port for the agent server")

    def serve(self):
        assert self.httpd
        (config.data_dir() / "port.txt").write_text(str(self.port))
        (config.data_dir() / "server.pid").write_text(str(os.getpid()))
        threading.Thread(target=self.reminder_loop, daemon=True).start()
        threading.Thread(target=self.warm_up, daemon=True).start()
        threading.Thread(target=self._knowledge_init, daemon=True).start()
        try:
            self.httpd.serve_forever(poll_interval=0.25)
        finally:
            for f in ("port.txt", "server.pid"):
                (config.data_dir() / f).unlink(missing_ok=True)

    def _knowledge_init(self):
        """Make sure the knowledge folder exists, read what is in it, and notice new files every few minutes."""
        from . import knowledge

        while not self.stopping:
            try:
                knowledge.refresh()
            except Exception as e:
                print("knowledge refresh failed:", e, flush=True)
            for _ in range(120):
                if self.stopping:
                    return
                time.sleep(2.5)

    def shutdown(self):
        self.stopping = True
        self.core.stop()
        self.hub.emit("bye")
        time.sleep(0.2)
        if self.httpd:
            self.httpd.shutdown()


def _redirect_output():
    """pythonw has no console: send prints and errors to agent.log."""
    try:
        log = open(config.data_dir() / "agent.log", "a", encoding="utf-8", buffering=1)
        sys.stdout = sys.stderr = log
    except Exception:
        pass


def main(argv=None) -> int:
    _redirect_output()
    # one agent per user: if one already answers, leave it alone
    try:
        port = int((config.data_dir() / "port.txt").read_text())
        with socket.create_connection(("127.0.0.1", port), timeout=0.5):
            print("agent already running on", port)
            return 0
    except Exception:
        pass
    app = AgentServer()
    port = app.bind(int(config.load().get("port", config.DEFAULT_PORT)))
    print(time.strftime("%Y-%m-%d %H:%M:%S"), "agent server on", port, flush=True)
    app.serve()
    return 0


if __name__ == "__main__":
    sys.exit(main())
