"""A pretend claude.ai: sign-in (the 'user' signs in after 2 s), a contenteditable box, a Stop button while it 'thinks', and an answer."""
import http.server
import threading

PAGE = """<!doctype html><html><body>
<main><div id="log"></div></main>
<div id="ed" contenteditable="true" role="textbox" style="min-height:40px;border:1px solid #888"></div>
<button aria-label="Send message" id="send">Send</button><button aria-label="Stop response" id="stop" style="display:none">Stop</button>
<script>
const ed = document.getElementById('ed'), log = document.getElementById('log');
function go() {
  const prompt = ed.innerText.trim(); if (!prompt) return; ed.innerText = '';
  log.innerHTML += '<div class="user"></div>'; log.lastChild.textContent = prompt;
  document.getElementById('stop').style.display = 'inline';
  setTimeout(() => {
    window.__prompts = (window.__prompts || 0) + 1;
    const doc = prompt.slice(prompt.search(/\\[P\\d+\\]/));
    const sents = doc.replace(/\\[P\\d+\\] /g, '').split(/(?<=[.!?]) +/);
    const edits = [];
    if (sents.length > 1) edits.push({before: sents[0], after: sents[0].replace(/\\.$/, '!'), why: 'tone'});
    if (sents.length > 2) edits.push({before: sents[1], after: 'Completely different words are used for this idea now.', why: 'drastic'});
    if (sents.length > 3) edits.push({before: 'This sentence does not exist anywhere.', after: 'x', why: 'ghost'});
    const out = prompt.includes('"weak"') ? {overall: 82, weak: [{sentence: sents[0], why: 'clumsy'}, {sentence: 'Ghost sentence that is nowhere in the text.', why: 'x'}]} : {overall: 82, edits: edits};
    document.getElementById('stop').style.display = 'none';
    log.innerHTML += '<div class="assistant"><pre></pre></div>'; log.lastChild.firstChild.textContent = new URLSearchParams(location.search).has('curly') ? JSON.stringify(out).replace(/"((?:[^"\\\\]|\\\\.)*)"/g, '\u201c$1\u201d') : JSON.stringify(out, null, 1);
    window.__asked = (window.__asked || 0) + 1;
  }, 1500);
}
document.getElementById('send').onclick = go;
ed.addEventListener('keydown', e => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); go(); } });
</script></body></html>"""
LOGIN = """<!doctype html><html><body><h1>Log in</h1><button>Continue with Google</button>
<script>setTimeout(() => { document.cookie = 'cauth=1; path=/'; location.href = '/new'; }, 2000);</script></body></html>"""


HITS = {"new": 0}


class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        authed = "cauth=1" in (self.headers.get("Cookie") or "")
        if self.path.startswith("/new") and not authed:
            self.send_response(302); self.send_header("Location", "/login"); self.end_headers(); return
        if self.path.startswith("/new") and authed:
            HITS["new"] += 1
        body = (LOGIN if self.path.startswith("/login") else PAGE).encode()
        self.send_response(200); self.send_header("Content-Type", "text/html; charset=utf-8"); self.send_header("Content-Length", str(len(body))); self.end_headers()
        self.wfile.write(body)


def start():
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, srv.server_address[1]
