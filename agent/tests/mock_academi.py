"""A pretend academi.cx: sign-in page (the 'user' signs in after 3 s), upload box with a hidden file input, a Check button,
a scan that takes a moment, an AI score and a Download button.  ?mode=nodownload removes the button."""
import http.server
import threading

HOME = """<!doctype html><html><head><title>academi</title></head><body>
<nav><a href="/">academi.cx</a> <a>Pricing</a> <a href="/dashboard">Dashboard</a></nav>
<div role="tablist"><button role="tab" id="t1">AI Detector</button><button role="tab">Similarity Detector</button></div>
<div id="drop"><b>Upload your paper</b> PDF, DOC, DOCX or TXT<input type="file" accept=".pdf,.doc,.docx,.txt" class="hidden" id="f" style="display:none"/></div>
<button id="chk" disabled>Check my paper</button>
<div id="res"></div>
<script>
const nodl = location.search.includes('nodownload');
document.getElementById('f').onchange = e => { window.__file = e.target.files[0].name; document.getElementById('chk').disabled = false; };
document.getElementById('chk').onclick = () => {
  const r = document.getElementById('res'); r.innerHTML = '<div class="animate-spin">Scanning your paper...</div>';
  setTimeout(async () => {
    const res = await (await fetch('/result.json')).json();
    const hl = (t, kind) => '<span style="background:' + (kind === 'high' ? 'rgb(185, 232, 238)' : 'rgb(241, 238, 252)') + '">' + t + '</span> ';
    let doc = '';
    for (let i = 0; i < 40; i++) doc += '<p>Filler paragraph number ' + i + ' of the uploaded paper with some ordinary words in it.</p>';
    doc += res.high.map(t => '<p>' + hl(t, 'high') + '</p>').join('') + res.medium.map(t => '<p>' + hl(t, 'medium') + '</p>').join('');
    for (let i = 40; i < 70; i++) doc += '<p>Closing filler paragraph number ' + i + ' of the paper, nothing flagged here.</p>';
    r.innerHTML = '<h2>AI Report</h2><p><div class="gauge"><span style="font-size:40px">' + res.ai + '</span><sup>%</sup><div>of this text is AI</div></div></p><p>Sentence by sentence...</p>' + (nodl ? '' : '<button id="dl">Download report</button>') +
      '<aside><b>' + (res.high.length + res.medium.length) + '/76</b> sentences flagged <span style="display:inline-block;width:10px;height:10px;background:rgb(185, 232, 238)"></span> <span>High likelihood</span> ' +
      '<span style="display:inline-block;width:10px;height:10px;background:rgb(241, 238, 252)"></span> <span>Medium likelihood</span></aside>' +
      '<div id="viewer" style="height:260px;overflow-y:auto;border:1px solid #ccc">' + doc + '</div>';
    if (!nodl) document.getElementById('dl').onclick = () => {
      const a = document.createElement('a'); a.href = URL.createObjectURL(new Blob(['%PDF-1.4 fake report for ' + window.__file], {type: 'application/pdf'}));
      a.download = 'ai-report.pdf'; document.body.appendChild(a); a.click();
    };
  }, 2000);
};
</script></body></html>"""
LOGIN = """<!doctype html><html><body><h1>Sign in</h1><button>Sign in with email</button>
<script>setTimeout(() => { document.cookie = 'auth=1; path=/'; location.href = '/dashboard' + location.search; }, 3000);</script></body></html>"""


import json
STATE = {"ai": 34, "high": [], "medium": [], "scans": 0}   # what the next scan shows (tests set it)


class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        path = self.path.split("?")[0]
        authed = "auth=1" in (self.headers.get("Cookie") or "")
        if path in ("/dashboard", "/") and not authed and path == "/dashboard":
            self.send_response(302); self.send_header("Location", "/login" + ("?" + self.path.split("?", 1)[1] if "?" in self.path else "")); self.end_headers(); return
        if path == "/result.json":
            STATE["scans"] += 1
            body = json.dumps({k: STATE[k] for k in ("ai", "high", "medium")}).encode()
            self.send_response(200); self.send_header("Content-Type", "application/json"); self.send_header("Content-Length", str(len(body))); self.end_headers()
            self.wfile.write(body); return
        body = (LOGIN if path == "/login" else HOME).encode()
        self.send_response(200); self.send_header("Content-Type", "text/html; charset=utf-8"); self.send_header("Content-Length", str(len(body))); self.end_headers()
        self.wfile.write(body)


def start():
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, srv.server_address[1]
