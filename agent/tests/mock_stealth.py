"""A small imitation of stealthwriter.ai's Humanizer page (built from the screenshots) for testing the browser code.

Structure copied from the real page (saved from a real run): the result card has a toolbar row (Humanize More / Rehumanize /
Deep Scan On / Compare / Copy) and, as a SIBLING below it, a block of <span role="button"> sentences; options in the dialog are
shuffled (Option 1, Option 2, Original, Option 3 ...) and the top box says "ORIGINAL - CLICK TO EDIT".
Behaviours that matter:
  * generation takes a few seconds (the Humanize button shows a busy state),
  * the result is a list of clickable coloured sentences; clicking one opens "Pick a rewrite for this sentence"
    with Original / Option N and a "New alternatives" button,
  * the side bar has clickable links (they must not be mistaken for sentences).
Query string:  ?delay=3   seconds to "generate"      &mode=ok | nobutton | limit | error | opaque   &iconcopy=1  icon-only Copy button   &placeholder=1  early empty card   &vanish=N  the dialog closes by itself right after opening, N times (window lost focus)   &stay=1  dialog stays open after a pick   &plain=1  sentences without pointer cursor / colours
"""
import http.server
import threading

PAGE = r"""<!doctype html><html><head><meta charset="utf-8"><title>Humanizer</title>
<style>
body{font-family:sans-serif;margin:0;display:flex;background:#111;color:#eee} aside{width:200px;padding:12px;border-right:1px solid #333}
aside a{display:block;padding:6px;cursor:pointer;color:#eee;text-decoration:none} main{padding:20px;flex:1;max-width:820px}
.lv button{padding:4px 8px} textarea{width:100%;height:160px} .banner{color:#f66;border:1px solid #f66;padding:8px;margin:8px 0}
.card{border:1px solid #333;border-radius:12px;margin:16px 0;overflow:hidden}
.hd{padding:12px;border-bottom:1px solid #333} .p6{padding:24px} .ft{display:flex;justify-content:space-between;padding:10px 16px;border-top:1px solid #333;font-size:11px}
.cursor-pointer{cursor:pointer;padding:0 2px;border-radius:2px} .bg-emerald-100{background:rgba(16,185,129,.2)} .bg-red-100{background:rgba(239,68,68,.2)}
.ws{white-space:pre-wrap;font-size:15px}
#ov{position:fixed;inset:0;background:rgba(0,0,0,.6);display:none;z-index:9} #dlg{position:fixed;left:25%;top:5%;width:50%;max-height:90vh;overflow:auto;background:#1d1d1f;border:1px solid #555;border-radius:12px;padding:0;z-index:10;display:none}
#dlg .top{padding:14px} #dlg .orig{border:1px dashed #555;margin:8px 0;padding:8px;min-height:50px}
.opt{display:flex;gap:10px;width:100%;text-align:left;padding:10px 14px;cursor:pointer;border:0;background:none;color:#eee}.opt .flex{display:flex;flex-direction:column}.size-1\.5,.inline-block{display:inline-block;width:6px;height:6px;border-radius:50%;margin-left:6px}.bg-red-500{background:#ef4444}.bg-emerald-500{background:#10b981}
.opt .lab{font-size:11px;color:#aaa} #opts{}
</style></head><body>
<aside><b>stealthwriter</b><a>Dashboard</a><a>Humanizer</a><a>AI Detector</a><a>Account</a><a>Billing</a></aside>
<main>
<h1>Humanizer</h1>
<div class="lv">Level <button>1</button><button>2</button><button>3</button><button>4</button><button>5</button>
<button>6</button><button>7</button><button>8</button><button>9</button><button>10</button></div>
<textarea id="ta" placeholder="Paste your AI-generated text here..."></textarea>
<div class="banner" id="ban">Your text contains in-text citations in (Author et al., Year) format.</div>
<div><span id="wc">0/1,000 words</span> <button>Check for AI</button> <button id="hum">Humanize</button></div>
<div id="slot"></div>
<div class="card"><div class="hd"><h3>Detection Score</h3></div><div class="p6"></div></div>
</main>
<div id="ov"></div>
<div id="dlg"><div class="top"><div><b>Choose alternative</b> <button id="close" aria-label="Close">x</button></div><div>Pick a rewrite for this sentence</div>
<div class="orig"><span style="font-size:11px;text-transform:uppercase;display:block">Original — click to edit</span><textarea id="origtxt" rows="2"></textarea></div></div>
<div id="opts"></div><div style="padding:10px"><button id="more" style="width:100%;padding:8px">New alternatives</button></div></div>
<script>
const q = new URLSearchParams(location.search), mode = q.get('mode') || 'ok', delay = +(q.get('delay') || 3);
const scores = (q.get('scores') || '100').split(',').map(Number);
const $ = id => document.getElementById(id);
window.__rehumanized = 0;
if (mode === 'opaque') { const st = document.createElement('style'); st.textContent = '.cursor-pointer{cursor:default !important;background:none !important}'; document.head.appendChild(st); }
$('ta').oninput = () => { $('wc').textContent = $('ta').value.split(/\s+/).filter(Boolean).length + '/1,000 words'; };
let target = null, gen = 0, round = 0, source = '';
const lowerFirst = s => s.charAt(0).toLowerCase() + s.slice(1);
const SUF = ['', ' indeed', ' truly', ' really', ' again'];
function variants(orig, r) {
  const base = orig.replace(/[.!?]+$/, '');
  const g = (+q.get('nogreen') || 0) > r ? 'red' : 'green';   // nogreen=N: the first N batches of options hold no green one
  return [ ['In practice, ' + lowerFirst(base) + '.', 'red'], [base.replace(' and ', ' as well as ') + (base.includes(' and ') ? '.' : ' overall.'), g],
           [base + ', as noted.', g], ['Seas rising and' + (r ? ' again' : ''), g] ];
}
function setScore(v, scanning) {
  $('score').innerHTML = scanning ? '<div class="animate-spin">...</div>' : '<span class="text-2xl">' + v + '%</span><span>human</span><b>' + (v >= 100 ? 'Looks Human' : 'Mixed') + '</b>';
}
function render(text, rnd, scoreDelay) {
  const icon = q.get('iconcopy') ? '<span aria-label="Copy"></span>' : 'Copy';
  $('slot').innerHTML = '<div class="card"><div class="hd"><h3>Humanized Result</h3><div><div style="display:inline-flex"><button>Humanize More</button><button aria-haspopup="menu">v</button></div><button id="re">Rehumanize</button>' +
    '<button>Deep Scan On</button><button>Compare</button><button id="copy">' + icon + '</button></div></div>' +
    '<div class="p6"><div class="ws" id="out"></div></div><div class="ft"><p>Green = high human impact, Red = high AI impact. Click sentences to swap alternatives.</p><p>239 words</p></div></div>';
  if (q.get('iconcopy')) { $('copy').setAttribute('aria-label', 'Copy'); $('copy').innerHTML = '<svg width="12" height="12"></svg>'; }
  const out = $('out');
  text.split(/\n\s*\n/).forEach((par, pi) => {
    if (pi) out.appendChild(Object.assign(document.createElement('span'), {textContent: '\n\n'}));
    (par.match(/[^.!?]+[.!?]+(\s|$)/g) || [par]).forEach((sent, i) => {
      if (i) out.appendChild(Object.assign(document.createElement('span'), {textContent: ' '}));
      const clean = sent.trim().replace(/[.!?]+$/, '') + (SUF[rnd % SUF.length]) + '.';
      const sp = document.createElement('span'); sp.setAttribute('role', 'button'); sp.tabIndex = 0;
      sp.className = 'cursor-pointer ' + (i % 3 === 2 ? 'bg-red-100' : 'bg-emerald-100');
      sp.textContent = clean; sp.dataset.orig = sent.trim(); sp.onclick = () => openDlg(sp); out.appendChild(sp);
    });
  });
  $('copy').onclick = () => { navigator.clipboard.writeText(out.innerText.replace(/[ \t]+\n/g, '\n').trim()); };
  $('re').onclick = () => {
    window.__rehumanized++;
    $('re').disabled = true; $('out').style.opacity = .4; setScore(0, true);
    setTimeout(() => { round++; render(source, round); setScore(scores[Math.min(round, scores.length - 1)], false); }, delay * 1000);
  };
}
const scoreBox = document.querySelector('main .card:last-child');
scoreBox.id = 'scorecard'; scoreBox.querySelector('.p6').id = 'score'; scoreBox.querySelector('.p6').textContent = '';
$('hum').onclick = () => {
  if (mode === 'nobutton') return;
  if (mode === 'limit') { const d = document.createElement('div'); d.textContent = 'You have reached your daily limit. Upgrade to continue.'; $('slot').appendChild(d); return; }
  const text = $('ta').value.trim(); if (!text) return;
  source = text; round = 0;
  $('hum').textContent = 'Humanizing...'; $('hum').disabled = true;
  if (q.get('placeholder')) $('slot').innerHTML = '<div class="card"><div class="hd"><h3>Humanized Result</h3></div><div class="p6">Your text will appear here.</div></div>';
  setTimeout(() => {
    $('hum').textContent = 'Humanize'; $('hum').disabled = false;
    if (mode === 'error') { const t = document.createElement('div'); t.setAttribute('role', 'alert'); t.textContent = 'Something went wrong. Please try again.'; document.body.appendChild(t); return; }
    render(text, 0); setScore(0, true); setTimeout(() => setScore(scores[0], false), 1500);
  }, delay * 1000);
};
function showOpts(sp, r) {
  const o = $('opts'); o.innerHTML = ''; $('origtxt').value = sp.dataset.orig;
  const vs = variants(sp.dataset.orig, r);
  const list = [['Option 1', vs[0]], ['Option 2', vs[1]], ['Option 3', vs[2]], ['Original', [sp.dataset.orig, 'green']], ['Option 4', vs[3]]];
  list.forEach(([lab, [t, dot]]) => { const b = document.createElement('button'); b.className = 'opt';
    b.innerHTML = '<div class="ic">o</div><div class="flex"><span class="lab">' + lab + '<span aria-hidden="true" class="inline-block size-1.5 rounded-full ' + (dot === 'red' ? 'bg-red-500' : 'bg-emerald-500') + '"></span></span><span class="tx"></span></div>';
    b.querySelector('.tx').textContent = t;
    b.onclick = () => { sp.textContent = t; sp.className = 'cursor-pointer ' + (dot === 'red' ? 'bg-red-100' : 'bg-emerald-100'); window.__picked = (window.__picked || []).concat([[lab, dot]]); if (!q.get('stay')) { $('dlg').style.display = 'none'; $('ov').style.display = 'none'; } };
    o.appendChild(b); });
}
function openDlg(sp) { if (!document.getElementById('dlg') && window.__dlg) document.body.appendChild(window.__dlg); target = sp; gen = 0; showOpts(sp, 0); $('dlg').style.display = 'block'; $('ov').style.display = 'block';
  if (q.get('vanish') && (window.__vanished || 0) < +q.get('vanish')) { window.__vanished = (window.__vanished || 0) + 1; setTimeout(() => { window.__dlg = $('dlg'); $('dlg').remove(); $('ov').style.display = 'none'; }, 120); } }
$('more').onclick = () => { gen++; window.__more = (window.__more || 0) + 1; showOpts(target, gen); };
$('close').onclick = () => { $('dlg').style.display = 'none'; $('ov').style.display = 'none'; };
document.addEventListener('keydown', e => { if (e.key === 'Escape') $('close').click(); });
</script></body></html>"""


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_GET(self):
        body = PAGE.encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def start():
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, srv.server_address[1]
