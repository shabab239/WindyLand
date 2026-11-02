"""Upload the finished Word file to academi.cx and download its AI score report.

Runs in the same browser window / profile as the stealthwriter step (the one Tobi opens), so you sign in to academi.cx
yourself ONCE; the browser remembers it. Tobi never types your password.

The site's pages are not known in detail, so every step looks for the thing by what it is (a file input, a button called
Check / Scan / Download ...) and, when something is not found, the page is saved to the debug folder (humanizer_debug) so it can be fixed.
"""
from __future__ import annotations

import re
import time
from pathlib import Path

from .stop import sleep as _sleep
from .stealth_client import SiteError, StealthwriterClient

JS_AI_SCORE = r"""
() => {
  const t = (document.body.innerText || '').replace(/\s+/g, ' ');
  // the report says: "3 % of this text is AI" (the number and the % sign are separate elements)
  let m = t.match(/(\d{1,3}(?:\.\d+)?)\s*%\s*(?:of (?:the |this |your )?(?:text|document|paper|essay|content|file)\s+is\s+AI)/i);
  const ready = !!m || /sentences flagged/i.test(t);
  m = m || t.match(/(\d{1,3}(?:\.\d+)?)\s*%\s*(?:AI\b|of (?:the )?(?:text|document) is AI)/i)
       || t.match(/AI(?: score| detection| probability| likelihood)?\s*[:\-]?\s*(\d{1,3}(?:\.\d+)?)\s*%/i);
  const busy = !ready && (!!document.querySelector('.animate-spin, [aria-busy="true"], [role=progressbar]') || /\b(scanning|analy[sz]ing|processing|checking|uploading)\b/i.test(t.slice(0, 4000)));
  return {ai: m ? parseFloat(m[1]) : null, busy, ready};
}
"""
# Every highlighted piece of text in the report (the page marks AI-likely sentences with a coloured background), with its colour,
# plus the colours of the legend ("High likelihood" / "Medium likelihood") so each highlight can be told apart by the legend itself.
JS_HIGHLIGHTS = r"""
() => {
  const rgb = c => { const m = (c || '').match(/rgba?\(([^)]+)\)/); if (!m) return null; const p = m[1].split(/[ ,\/]+/).filter(Boolean).map(parseFloat); return {r: p[0], g: p[1], b: p[2], a: p.length > 3 ? p[3] : 1}; };
  const hsl = ({r, g, b}) => { r /= 255; g /= 255; b /= 255; const mx = Math.max(r, g, b), mn = Math.min(r, g, b), l = (mx + mn) / 2; const d = mx - mn;
    let h = 0, s = 0; if (d) { s = d / (1 - Math.abs(2 * l - 1)); h = mx === r ? ((g - b) / d) % 6 : mx === g ? (b - r) / d + 2 : (r - g) / d + 4; h = (h * 60 + 360) % 360; } return {h, s, l}; };
  const chroma = c => c && c.a > 0.05 && (() => { const x = hsl(c); return x.s >= 0.2 && x.l > 0.45 && x.l < 0.985; })();
  // legend: the swatch next to the words "High likelihood" / "Medium likelihood"
  const legend = {};
  for (const el of document.querySelectorAll('*')) {
    if (el.children.length > 3) continue;
    const t = (el.innerText || '').trim().toLowerCase();
    const kind = /^high( likelihood)?$/.test(t) ? 'high' : /^medium( likelihood)?$/.test(t) ? 'medium' : null;
    if (!kind || legend[kind]) continue;
    // the swatch is the coloured, text-less element right before the label (or the closest one before it inside the same parent)
    const colourOf = e => { const c = rgb(getComputedStyle(e).backgroundColor); return chroma(c) && !(e.innerText || '').trim() ? c : null; };
    let c = el.previousElementSibling && colourOf(el.previousElementSibling);
    if (!c && el.parentElement) {
      const kids = [...el.parentElement.querySelectorAll('*')];
      const at = kids.indexOf(el);
      for (let k = at - 1; k >= 0 && !c; k--) { if (kids[k].contains(el)) continue; c = colourOf(kids[k]); if (!c && (kids[k].innerText || '').trim()) break; }
    }
    if (!c && el.parentElement && el.parentElement.parentElement) {   // a row: [swatch] [label] wrapped in one more element
      const sib = el.parentElement.previousElementSibling; c = sib && colourOf(sib);
    }
    if (c) legend[kind] = c;
  }
  const dist = (a, b) => Math.hypot(a.r - b.r, a.g - b.g, a.b - b.b);
  const out = [];
  for (const el of document.querySelectorAll('span, mark, em, i, u, b, strong, p, li, div, font, a')) {
    if (el.closest('nav, header, aside, button, [role=tab], [role=tablist], [role=dialog]')) continue;
    const c = rgb(getComputedStyle(el).backgroundColor);
    if (!chroma(c)) continue;
    const text = (el.innerText || el.textContent || '').replace(/\s+/g, ' ').trim();
    if (text.length < 12 || text.split(' ').length < 3 || text.length > 900) continue;
    const par = el.parentElement && rgb(getComputedStyle(el.parentElement).backgroundColor);
    if (par && chroma(par) && dist(par, c) < 8 && (el.parentElement.innerText || '').replace(/\s+/g, ' ').trim().length <= 900) continue;   // keep the outermost element of one highlight
    let kind;
    if (legend.high || legend.medium) {
      const dh = legend.high ? dist(c, legend.high) : 1e9, dm = legend.medium ? dist(c, legend.medium) : 1e9;
      if (Math.min(dh, dm) > 60) continue;
      kind = dh <= dm ? 'high' : 'medium';
    } else { const x = hsl(c); kind = (x.h >= 165 && x.h <= 245) ? 'high' : (x.h > 245 && x.h <= 335) ? 'medium' : null; if (!kind) continue; }
    out.push({text, kind});
  }
  return {items: out, legend: Object.keys(legend)};
}
"""
JS_SCROLL = r"""
(step) => {
  // the element that scrolls the document view: the scrollable ancestor of the first highlighted/long text, else the page
  const cands = [...document.querySelectorAll('*')].filter(e => { const s = getComputedStyle(e); return /(auto|scroll)/.test(s.overflowY) && e.scrollHeight > e.clientHeight + 40 && e.clientHeight > 150; });
  cands.sort((a, b) => (b.scrollHeight - b.clientHeight) - (a.scrollHeight - a.clientHeight));
  const sc = cands.find(e => (e.innerText || '').length > 800) || cands[0] || document.scrollingElement;
  if (step === 'top') { sc.scrollTop = 0; return {top: 0, end: false}; }
  const before = sc.scrollTop; sc.scrollTop = before + sc.clientHeight * 0.8;
  return {top: sc.scrollTop, end: sc.scrollTop + sc.clientHeight >= sc.scrollHeight - 4 || sc.scrollTop === before};
}
"""
JS_FLAG_COUNT = r"""
() => { const t = (document.body.innerText || '').replace(/\s+/g, ' '); const m = t.match(/(\d+)\s*\/\s*(\d+)\s*sentences? flagged/i); return m ? [parseInt(m[1]), parseInt(m[2])] : null; }
"""
DOWNLOAD_RX = re.compile(r"download|export|save (?:as )?(?:pdf|report)|get (?:the )?report|\bpdf\b", re.I)


class AcademiClient(StealthwriterClient):
    def __init__(self, cfg: dict, log=print, notify=None):
        super().__init__(cfg, None, log)
        self.acfg = cfg.get("academi", {})
        self.notify = notify or (lambda *a: None)

    # ---------------------------------------------------------------- sign-in
    def _file_input(self):
        return self.page.locator("input[type=file]").first

    def _has_upload(self) -> bool:
        try:
            return self.page.locator("input[type=file]").count() > 0
        except Exception:
            return False

    def _login_visible(self) -> bool:
        try:
            if re.search(r"login|sign-?in|register|signup|auth", self.page.url, re.I):
                return True
            return self.page.get_by_role("link", name=re.compile(r"^\s*sign in\s*$", re.I)).first.is_visible() or \
                self.page.get_by_role("button", name=re.compile(r"^\s*sign in\s*$", re.I)).first.is_visible()
        except Exception:
            return False

    def open_site(self):
        url = self.acfg.get("url", "https://academi.cx/dashboard")
        self.page.goto(url, wait_until="domcontentloaded")
        deadline = time.time() + int(self.acfg.get("login_timeout_seconds", 600))
        announced = False
        while time.time() < deadline:
            _sleep(1.0)
            if self._has_upload() and not re.search(r"login|sign-?in|register|signup", self.page.url, re.I):
                return
            if not self._login_visible() and not self._has_upload():
                # signed in but this is not the upload page: go to the page that has the upload box
                try:
                    self.page.goto(self.acfg.get("upload_url", "https://academi.cx/"), wait_until="domcontentloaded")
                except Exception:
                    pass
                if self._has_upload():
                    return
            if not announced:
                announced = True
                msg = "Please sign in to academi.cx in the browser window I opened (only needed once). I am waiting..."
                self.log(">>> " + msg)
                self.notify("academi.cx sign-in", msg)
        self.dump("academi-login-timeout")
        raise SiteError("Timed out waiting for you to sign in to academi.cx.")

    # ---------------------------------------------------------------- upload + score
    def _click_if(self, rx: str, role: str = "button", timeout: int = 1500) -> bool:
        try:
            b = self.page.get_by_role(role, name=re.compile(rx, re.I)).first
            if b.is_visible():
                b.click(timeout=timeout)
                return True
        except Exception:
            pass
        return False

    def check_file(self, path: Path, out_dir: Path) -> dict:
        """Returns {"ok", "ai_percent", "report"} - report is the downloaded file (or a screenshot when the site has no download)."""
        path = Path(path)
        self.open_site()
        self._click_if(r"^\s*AI( Detector)?\s*$", "tab") or self._click_if(r"^\s*AI Detector\s*$") or self._click_if(r"^\s*AI Detector\s*$", "link")
        self.log("  uploading the document to academi.cx ...")
        try:
            self._file_input().set_input_files(str(path), timeout=15000)
        except Exception:
            self.dump("academi-no-upload")
            raise SiteError("I could not find the upload box on academi.cx. Saved the page to " + str(self.debug_dir))
        self._pause(1.0, 2.0)
        # some pages start by themselves after the upload, others need a button
        for rx in (r"check (my )?paper", r"^\s*scan\s*$", r"^\s*check\s*$", r"^\s*submit\s*$", r"^\s*upload\s*$", r"^\s*start\s*$"):
            if self._click_if(rx):
                self.log("  started the check")
                break
        score = self._score_with_rechecks()
        if score is None:
            self.dump("academi-no-score")
            raise SiteError("academi.cx did not show an AI score in time. Saved the page to " + str(self.debug_dir))
        self.log(f"  academi.cx AI score: {score:g}% AI")
        self._pause(1.0, 2.0)
        flagged = {"high": [], "medium": []}
        if score > 0:
            try:
                flagged = self.read_flagged()
            except Exception as e:
                self.log(f"  (could not read the highlighted sentences: {str(e)[:120]})")
        report = self._download_report(path, out_dir)
        return {"ok": True, "ai_percent": score, "report": str(report) if report else None, "flagged": flagged}

    def read_flagged(self) -> dict:
        """Scroll through the report and collect the highlighted sentences: {"high": [...], "medium": [...]} (blue = high likelihood, purple = medium)."""
        found: dict[str, dict[str, str]] = {"high": {}, "medium": {}}
        try:
            self.page.evaluate(JS_SCROLL, "top")
        except Exception:
            pass
        _sleep(0.4)
        legend: list = []
        for _ in range(120):
            try:
                r = self.page.evaluate(JS_HIGHLIGHTS)
            except Exception:
                r = {"items": [], "legend": []}
            legend = r.get("legend") or legend
            for it in r.get("items", []):
                key = re.sub(r"\W+", " ", it["text"].lower()).strip()
                found[it["kind"]].setdefault(key, it["text"])
            try:
                pos = self.page.evaluate(JS_SCROLL, "down")
            except Exception:
                break
            _sleep(0.35)
            if pos.get("end"):
                try:
                    r = self.page.evaluate(JS_HIGHLIGHTS)
                    for it in r.get("items", []):
                        key = re.sub(r"\W+", " ", it["text"].lower()).strip()
                        found[it["kind"]].setdefault(key, it["text"])
                except Exception:
                    pass
                break
        high = list(found["high"].values())
        medium = [t for k, t in found["medium"].items() if k not in found["high"]]
        try:
            cnt = self.page.evaluate(JS_FLAG_COUNT)
        except Exception:
            cnt = None
        self.log(f"  highlighted in the report: {len(high)} high-likelihood (blue) and {len(medium)} medium (purple) passages"
                 + (f"; the page says {cnt[0]}/{cnt[1]} sentences flagged" if cnt else "") + ("" if legend else " (legend colours not found, guessed from the hue)"))
        return {"high": high, "medium": medium, "count": cnt}

    def _wait_for_score(self, timeout: int | None = None):
        timeout = int(timeout if timeout is not None else self.acfg.get("score_timeout_seconds", 240))
        t0 = time.time()
        last, same = None, 0
        nextlog = 30
        while time.time() - t0 < timeout:
            try:
                r = self.page.evaluate(JS_AI_SCORE)
            except Exception:
                r = {}
            v = r.get("ai")
            if v is not None and not r.get("busy"):
                same = same + 1 if v == last else 0
                last = v
                if same >= (1 if r.get("ready") else 2):     # "of this text is AI" only appears when the report is finished
                    return v
            else:
                last, same = None, 0
            if time.time() - t0 > nextlog:
                self.log(f"  ...waiting for academi.cx's AI score ({int(time.time() - t0)}s)")
                nextlog += 30
            _sleep(1.0)
        return None

    def _score_with_rechecks(self):
        """The score is not there in time: look again (reload the report page) a few times before giving up."""
        score = self._wait_for_score()
        tries = int(self.acfg.get("score_rechecks", 3))
        for k in range(tries):
            if score is not None:
                return score
            self.log(f"  academi.cx has not shown the AI score yet; checking again ({k + 1}/{tries}) ...")
            self.shot(f"academi-wait-{k + 1}")
            try:
                self.page.reload(wait_until="domcontentloaded")
            except Exception:
                pass
            score = self._wait_for_score(int(self.acfg.get("score_recheck_seconds", 90)))
        return score

    def _download_report(self, src: Path, out_dir: Path) -> Path | None:
        out_dir.mkdir(parents=True, exist_ok=True)
        base = out_dir / f"{src.stem}_AI-score"
        for attempt in range(2):
            cands = self.page.get_by_role("button", name=DOWNLOAD_RX)
            links = self.page.get_by_role("link", name=DOWNLOAD_RX)
            for loc in (cands, links):
                for i in range(min(loc.count(), 4)):
                    el = loc.nth(i)
                    try:
                        if not el.is_visible():
                            continue
                        with self.page.expect_download(timeout=20000) as dl:
                            el.click(timeout=3000)
                        d = dl.value
                        ext = Path(d.suggested_filename).suffix or ".pdf"
                        target = base.with_suffix(ext)
                        n = 1
                        while target.exists():
                            target = out_dir / f"{src.stem}_AI-score ({n}){ext}"
                            n += 1
                        d.save_as(str(target))
                        self.log(f"  downloaded the report: {target.name}")
                        return target
                    except Exception:
                        # a menu may have opened instead of a download: choose the AI report entry
                        for rx in (r"ai.*(report|pdf)", r"\bpdf\b", r"report"):
                            try:
                                it = self.page.get_by_role("menuitem", name=re.compile(rx, re.I)).first
                                if it.is_visible():
                                    with self.page.expect_download(timeout=20000) as dl:
                                        it.click(timeout=3000)
                                    d = dl.value
                                    ext = Path(d.suggested_filename).suffix or ".pdf"
                                    target = base.with_suffix(ext)
                                    d.save_as(str(target))
                                    self.log(f"  downloaded the report: {target.name}")
                                    return target
                            except Exception:
                                continue
            _sleep(2.0)
        # the site has no download button that I can find: keep a picture of the whole report instead
        try:
            target = base.with_suffix(".png")
            self.page.screenshot(path=str(target), full_page=True)
            self.dump("academi-no-download")
            self.log(f"  no download button found; saved a screenshot of the report: {target.name}")
            return target
        except Exception:
            return None
