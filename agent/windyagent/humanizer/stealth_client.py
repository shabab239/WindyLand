"""Drives stealthwriter.ai in YOUR browser (Brave / Chrome / Edge) through Playwright.

How the sign-in works
  The agent starts your browser with its own profile folder and a debugging port, then attaches to it.
  The first time, you sign in with Google yourself (the agent never types your password).
  The profile folder remembers the session, so later runs go straight to the Humanizer.

The site-specific part (finding buttons, sentences and the "Pick a rewrite" dialog) is written with
text-based locators, but it was built from screenshots. If anything is not found, run
    python agent.py --inspect
and send me the files it saves in ./debug - I can then fix the selectors in minutes.
"""
from __future__ import annotations

import json
import os
import random
import re
import shutil
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

from .judge import Candidate, JudgeRequest
from .text_utils import split_paragraphs

LIMIT_RX = re.compile(
    r"(limit (reached|exceeded)|reached your (daily )?limit|out of (credits|humanizations)|upgrade to (continue|unlock)|exceeded)",
    re.I,
)


from .stop import sleep as _sleep  # noqa: E402


class LimitReached(Exception):
    """The free plan's daily limit was hit."""


class MappingError(Exception):
    """The humanized text could not be mapped back onto the Word paragraphs."""


class SiteError(Exception):
    """Something on the website was not found / did not behave as expected."""


# --------------------------------------------------------------------------- page scripts
JS_TAG_SENTENCES = r"""
(card, selector) => {
  card.querySelectorAll('[data-sw-s]').forEach(e => e.removeAttribute('data-sw-s'));
  let leafs = [];
  let method = 'selector';
  if (selector) {
    leafs = Array.from(card.querySelectorAll(selector)).filter(e => (e.innerText || '').trim().length > 2);
  } else {
    const all = Array.from(card.querySelectorAll('span, mark, a, p, div, [role=button]'));
    const clickable = all.filter(e => {
      const t = (e.innerText || '').trim();
      if (t.length < 3) return false;
      if (e.closest('button, a, nav, aside, header, [role=dialog], [role=navigation]')) return false;
      return getComputedStyle(e).cursor === 'pointer';
    });
    leafs = clickable.filter(e => !clickable.some(o => o !== e && e.contains(o)));
    method = 'pointer';
    if (!leafs.length) {
      const tinted = all.filter(e => {
        const t = (e.innerText || '').trim();
        if (t.length < 3 || e.closest('button, a, nav, aside, header, [role=dialog]')) return false;
        const bg = getComputedStyle(e).backgroundColor;
        return bg && bg !== 'rgba(0, 0, 0, 0)' && bg !== 'transparent';
      });
      leafs = tinted.filter(e => !tinted.some(o => o !== e && e.contains(o)));
      method = 'background';
    }
    if (!leafs.length) {   // last resort: sentence-sized spans / marks that carry the text
      const sp = Array.from(card.querySelectorAll('span, mark')).filter(e => {
        const t = (e.innerText || '').trim();
        return t.length >= 12 && /[.!?]["')\u201d]?$/.test(t) && !e.closest('button, a, nav, aside, header, [role=dialog]');
      });
      leafs = sp.filter(e => !sp.some(o => o !== e && e.contains(o)));
      method = 'spans';
    }
  }
  const colorOf = (bg) => {
    const m = (bg || '').match(/rgba?\((\d+),\s*(\d+),\s*(\d+)/);
    if (!m) return 'other';
    const r = +m[1], g = +m[2], b = +m[3];
    if (g > r + 15 && g > b) return 'green';
    if (r > g + 15 && r > b) return 'red';
    return 'other';
  };
  leafs.forEach((e, i) => e.setAttribute('data-sw-s', String(i)));
  return {method, items: leafs.map((e, i) => ({
    i, text: (e.innerText || '').replace(/\s+/g, ' ').trim(),
    color: colorOf(getComputedStyle(e).backgroundColor)
  }))};
}
"""

JS_READ_OPTIONS = r"""
(dlg) => {
  dlg.querySelectorAll('[data-sw-o]').forEach(e => e.removeAttribute('data-sw-o'));
  const rx = /^(Original|Option\s+\d+)\s*\n?([\s\S]*)$/;
  const els = Array.from(dlg.querySelectorAll('label, button, [role=radio], [role=option], li, div, section'));
  let found = els.filter(e => {
    const t = (e.innerText || '').trim();
    if (/click to edit/i.test(t)) return false;
    const m = t.match(rx);
    return m && m[2].replace(/\s+/g, ' ').trim().length > 8;
  });
  found = found.filter(e => !found.some(o => o !== e && e.contains(o)));
  const dotOf = (e) => {
    const d = Array.from(e.querySelectorAll('span[aria-hidden], i[aria-hidden], span[class*="rounded-full"]'))
      .find(x => !(x.innerText || '').trim() && x.getBoundingClientRect().width < 20);
    if (!d) return 'none';
    const cls = String(d.className || '');
    if (/emerald|green|lime|teal/.test(cls)) return 'green';
    if (/red|rose|orange|amber|yellow/.test(cls)) return 'red';
    const m = getComputedStyle(d).backgroundColor.match(/rgba?\((\d+),\s*(\d+),\s*(\d+)/);
    if (!m) return 'none';
    const r = +m[1], g = +m[2], b = +m[3];
    return (g > r + 15 && g > b) ? 'green' : (r > g + 15 && r > b) ? 'red' : 'none';
  };
  return found.map((e, i) => {
    e.setAttribute('data-sw-o', String(i));
    const m = (e.innerText || '').trim().match(rx);
    return {i, label: m[1], text: m[2].replace(/\s+/g, ' ').trim(), dot: dotOf(e)};
  });
}
"""

JS_SCORE = r"""
() => {
  const h = Array.from(document.querySelectorAll('h1, h2, h3, h4, p, span, div')).find(e => e.children.length === 0 && /^\s*detection score\s*$/i.test(e.innerText || ''));
  if (!h) return {found: false};
  let card = null;
  for (let a = h; a && a !== document.body; a = a.parentElement) {
    const t = a.innerText || '';
    if (t.length > 700 || /humanized result/i.test(t)) break;      // climbed out of the score box
    if (/\d{1,3}\s*%/.test(t)) { card = a; break; }
  }
  if (!card) return {found: true, human: null, scanning: !!document.querySelector('.animate-spin'), text: ''};
  const text = (card.innerText || '').replace(/\s+/g, ' ').trim();
  const scanning = !!card.querySelector('.animate-spin, [aria-busy="true"]') || /\b(scanning|analy[sz]ing|checking|detecting)\b/i.test(text);
  let human = null;
  let m = text.match(/(\d{1,3})\s*%\s*human/i);
  if (m) human = +m[1];
  else if ((m = text.match(/(\d{1,3})\s*%\s*(ai|detected|ai-generated)/i))) human = 100 - +m[1];
  else if ((m = text.match(/(\d{1,3})\s*%/))) human = +m[1];
  return {found: true, human, scanning, text: text.slice(0, 200)};
}
"""

JS_COMMON_ANCESTOR_TEXT = r"""
(card) => {
  const els = Array.from(card.querySelectorAll('[data-sw-s]'));
  if (!els.length) return '';
  let a = els[0];
  while (a && !els.every(e => a.contains(e))) a = a.parentElement;
  return a ? a.innerText : '';
}
"""


JS_STATE = r"""
() => {
  const vis = e => { const r = e.getBoundingClientRect(); const s = getComputedStyle(e);
                     return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none'; };
  const btns = Array.from(document.querySelectorAll('button, [role=button]')).filter(vis);
  const hb = btns.find(b => /^\s*(humaniz(e|ing)|processing|generating|working|rewriting)\b/i.test(b.innerText || '') && !/humanizer/i.test(b.innerText || ''));
  const copy = btns.filter(b => /^\s*copy\s*$/i.test(b.innerText || b.getAttribute('aria-label') || b.getAttribute('title') || ''));
  const spinner = !!document.querySelector('.animate-spin, [aria-busy="true"], [role=progressbar], svg[class*="spin"]');
  const alerts = Array.from(document.querySelectorAll('[role=alert], [role=status], [data-sonner-toast], .toast, [class*="toast"]'))
      .filter(vis).map(e => (e.innerText || '').replace(/\s+/g, ' ').trim()).filter(Boolean);
  return {btn: hb ? (hb.innerText || '').trim() : null, btnDisabled: hb ? (hb.disabled || hb.getAttribute('aria-disabled') === 'true') : null,
          spinner, copyEnabled: copy.some(b => !b.disabled && b.getAttribute('aria-disabled') !== 'true'), copyCount: copy.length, alerts,
          scrollY: window.scrollY, height: document.documentElement.scrollHeight};
}
"""


def _cdp_ready(port: int) -> bool:
    try:
        urllib.request.urlopen(f"http://127.0.0.1:{port}/json/version", timeout=1.5).read()
        return True
    except Exception:
        return False


# Chromium-based browsers Tobi can drive (Playwright talks to them over the debugging port). Firefox and Safari cannot be driven this way.
BROWSERS = {
    "brave":    ("Brave",   [r"BraveSoftware\Brave-Browser\Application\brave.exe"], ["brave", "brave-browser"], ("BraveHTML", "BraveFile")),
    "chrome":   ("Chrome",  [r"Google\Chrome\Application\chrome.exe"], ["google-chrome", "chrome", "google-chrome-stable"], ("ChromeHTML",)),
    "edge":     ("Edge",    [r"Microsoft\Edge\Application\msedge.exe"], ["microsoft-edge", "msedge"], ("MSEdgeHTM", "MSEdgeMHT")),
    "vivaldi":  ("Vivaldi", [r"Vivaldi\Application\vivaldi.exe"], ["vivaldi"], ("VivaldiHTM",)),
    "opera":    ("Opera",   [r"Opera\opera.exe", r"Programs\Opera\opera.exe", r"Opera GX\opera.exe", r"Programs\Opera GX\opera.exe"], ["opera"], ("OperaStable", "OperaGXStable")),
    "chromium": ("Chromium", [r"Chromium\Application\chrome.exe"], ["chromium", "chromium-browser"], ("ChromiumHTM",)),
}
AUTO_ORDER = ("chrome", "edge", "brave", "vivaldi", "opera", "chromium")   # when the default browser cannot be used; Edge ships with Windows


def installed_browsers() -> dict[str, str]:
    """{key: path} of every supported browser found on this PC."""
    roots = [os.environ.get(k, "") for k in ("ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA")]
    out: dict[str, str] = {}
    for key, (_label, rels, names, _prog) in BROWSERS.items():
        found = ""
        for rel in rels:
            for root in roots:
                if root and Path(root, rel).exists():
                    found = str(Path(root, rel))
                    break
            if found:
                break
        if not found:
            for n in names:
                w = shutil.which(n)
                if w:
                    found = w
                    break
        if found:
            out[key] = found
    return out


def default_browser_key() -> str | None:
    """The browser Windows opens links with, if it is one Tobi can drive."""
    try:
        import winreg

        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice") as k:
            prog = str(winreg.QueryValueEx(k, "ProgId")[0])
    except Exception:
        return None
    for key, (_l, _r, _n, progs) in BROWSERS.items():
        if any(prog.startswith(p) for p in progs):
            return key
    return None


def browser_key(exe: str) -> str:
    low = str(exe).lower().replace("/", "\\")
    if "chromium" in low:
        return "chromium"
    for key, (_l, rels, names, _p) in BROWSERS.items():
        if key != "chromium" and any(re.split(r"[\\/]", r)[-1].lower() in low.split("\\")[-1] for r in rels):
            return key
    for key, (_l, rels, names, _p) in BROWSERS.items():
        if any(n in low.split("\\")[-1] for n in names):
            return key
    return "other"


def find_browser(configured: str = "", choice: str = "auto") -> str:
    """The browser to drive. `configured` is an exact path; `choice` is auto (your default browser, else the first one installed),
    or brave / chrome / edge / vivaldi / opera / chromium."""
    if configured and Path(os.path.expanduser(configured)).exists():
        return os.path.expanduser(configured)
    choice = (choice or "auto").strip().lower()
    found = installed_browsers()
    if choice in found:
        return found[choice]
    if choice not in ("auto", "") and choice not in BROWSERS:
        p = Path(os.path.expanduser(choice))
        if p.exists():
            return str(p)
    # "auto", or the chosen browser is not installed on this PC
    d = default_browser_key()
    if d and d in found:
        return found[d]
    for k in AUTO_ORDER:
        if k in found:
            return found[k]
    raise SiteError("Could not find a Chromium-based browser (Chrome, Edge, Brave, Vivaldi or Opera). Install one, or choose it in Tobi's settings. "
                    "Firefox cannot be controlled.")


class StealthwriterClient:
    def __init__(self, cfg: dict, judge, log=print):
        self.scores: list[dict] = []
        self.red_left = 0
        self.cfg = cfg
        self.judge = judge
        self.log = log
        self.site = cfg["site"]
        self.rev = cfg["review"]
        self.br = cfg["browser"]
        self.debug_dir = Path(cfg.get("debug_dir", "debug"))
        self.review_log: list[dict] = []
        self._pw = self._browser = self.ctx = self.page = None

    # ------------------------------------------------------------------ lifecycle
    def __enter__(self):
        self.start()
        return self

    def __exit__(self, *exc):
        self.stop()

    # flags that keep the page working at full speed while it is behind other windows, or you are using something else
    BACKGROUND_FLAGS = [
        "--disable-background-timer-throttling", "--disable-renderer-backgrounding", "--disable-backgrounding-occluded-windows",
        "--disable-features=CalculateNativeWinOcclusion,IntensiveWakeUpThrottling,TabFreeze,BackForwardCache",
        "--disable-ipc-flooding-protection",
    ]
    FLAGS_VERSION = "bg1"

    def _launch_browser(self, port: int, profile: Path, exe: str):
        self.log(f"Starting browser: {exe}")
        subprocess.Popen(
            [exe, f"--remote-debugging-port={port}", f"--user-data-dir={profile}",
             "--no-first-run", "--no-default-browser-check", *self.BACKGROUND_FLAGS, *self.br.get("extra_args", []), "about:blank"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        try:
            (profile / ".tobi-flags").write_text(self.FLAGS_VERSION, encoding="utf-8")
        except Exception:
            pass
        for _ in range(60):
            if _cdp_ready(port):
                return
            _sleep(0.5)
        raise SiteError("The browser did not open its debugging port. Close all windows of that browser and retry.")

    def start(self):
        port = int(self.br["debug_port"])
        exe = find_browser(self.br.get("path", ""), self.br.get("choice", "auto"))
        profile = Path(os.path.expanduser(self.br["profile_dir"]))
        key = browser_key(exe)
        if key not in ("brave",):                 # each browser keeps its own sign-ins (profiles of different browsers must not be mixed)
            profile = profile.with_name(f"{profile.name}_{key}")
        profile.mkdir(parents=True, exist_ok=True)
        if _cdp_ready(port):
            # a window that an older Tobi opened has no "keep working in the background" flags: close it and start it again
            flags = profile / ".tobi-flags"
            if not (flags.exists() and flags.read_text(encoding="utf-8").strip() == self.FLAGS_VERSION):
                try:
                    from playwright.sync_api import sync_playwright

                    with sync_playwright() as pw:
                        br = pw.chromium.connect_over_cdp(f"http://127.0.0.1:{port}")
                        br.new_browser_cdp_session().send("Browser.close")
                    for _ in range(20):
                        if not _cdp_ready(port):
                            break
                        _sleep(0.5)
                except Exception:
                    pass
        if not _cdp_ready(port):
            self._launch_browser(port, profile, exe)
        from playwright.sync_api import sync_playwright

        self._pw = sync_playwright().start()
        self._browser = self._pw.chromium.connect_over_cdp(f"http://127.0.0.1:{port}")
        self.ctx = self._browser.contexts[0] if self._browser.contexts else self._browser.new_context()
        try:
            self.ctx.grant_permissions(["clipboard-read", "clipboard-write"], origin=self.site["url"].split("/dashboard")[0])
        except Exception:
            pass
        self.page = self.ctx.new_page()
        self.page.set_default_timeout(20000)
        self._keep_alive_in_background()

    def _keep_alive_in_background(self):
        """The page behaves as if it always had the focus, even while you click on another window (a dialog or a Copy button would
        otherwise stop working)."""
        try:
            cdp = self.ctx.new_cdp_session(self.page)
            cdp.send("Emulation.setFocusEmulationEnabled", {"enabled": True})
        except Exception:
            pass

    def stop(self):
        try:
            if self.page and self.br.get("close_when_done"):
                self.page.close()
        finally:
            if self._pw:
                self._pw.stop()

    # ------------------------------------------------------------------ small helpers
    def _pause(self, lo=None, hi=None):
        d = self.br.get("human_delay", [0.4, 1.2])
        _sleep(random.uniform(lo if lo is not None else d[0], hi if hi is not None else d[1]))

    def shot(self, name: str):
        try:
            self.debug_dir.mkdir(parents=True, exist_ok=True)
            self.page.screenshot(path=str(self.debug_dir / f"{name}.png"))
        except Exception:
            pass

    def dump(self, name: str, with_card: bool = False):
        """Everything needed to see what the page looked like: full-page screenshot, text and HTML (for fixing selectors)."""
        try:
            self.debug_dir.mkdir(parents=True, exist_ok=True)
            try:
                self.page.screenshot(path=str(self.debug_dir / f"{name}-full.png"), full_page=True)
            except Exception:
                pass
            (self.debug_dir / f"{name}.txt").write_text(self.page.inner_text("body"), encoding="utf-8")
            (self.debug_dir / f"{name}.html").write_text(self.page.content(), encoding="utf-8")
            if with_card:
                (self.debug_dir / f"{name}-card.html").write_text(self._result_card().evaluate("e => e.outerHTML"), encoding="utf-8")
        except Exception:
            pass

    def _state(self) -> dict:
        try:
            return self.page.evaluate(JS_STATE)
        except Exception:
            return {}

    def _textarea(self):
        return self.page.get_by_placeholder(re.compile("paste your ai", re.I)).first

    # ------------------------------------------------------------------ sign-in / navigation
    def open_humanizer(self):
        self.page.goto(self.site["url"], wait_until="domcontentloaded")
        deadline = time.time() + int(self.br.get("login_timeout_seconds", 300))
        announced = False
        clicked_google = False
        while time.time() < deadline:
            try:
                if self._textarea().is_visible():
                    return
            except Exception:
                pass
            url = self.page.url
            if "accounts.google.com" in url:
                email = self.site.get("google_account_email", "")
                if email:  # only picks your account tile on Google's chooser; passwords are always typed by you
                    try:
                        self.page.get_by_text(email, exact=False).first.click(timeout=1500)
                    except Exception:
                        pass
            elif not clicked_google and re.search(r"login|sign-?in|auth|signup|register", url, re.I) or (
                "stealthwriter.ai" in url and "dashboard" not in url
            ):
                try:
                    btn = self.page.get_by_role("button", name=re.compile("google", re.I)).first
                    if btn.is_visible():
                        btn.click()
                        clicked_google = True
                except Exception:
                    pass
            if not announced:
                self.log(">>> Please finish signing in to stealthwriter.ai in the browser window "
                         "(choose your Google account / type your password if asked). Waiting...")
                announced = True
            _sleep(1.5)
            if "stealthwriter.ai/dashboard" in self.page.url and not self._textarea().is_visible():
                self.page.goto(self.site["url"], wait_until="domcontentloaded")
        self.shot("login-timeout")
        raise SiteError("Timed out waiting for the Humanizer page (sign-in not completed?).")

    def select_level(self, level: int):
        label = str(level)
        for finder in (
            lambda: self.page.get_by_role("button", name=label, exact=True),
            lambda: self.page.get_by_role("radio", name=label, exact=True),
            lambda: self.page.get_by_role("tab", name=label, exact=True),
            lambda: self.page.get_by_text(label, exact=True),
        ):
            loc = finder()
            if loc.count():
                loc.first.click()
                self._pause(0.2, 0.5)
                return
        self.shot("level-not-found")
        raise SiteError(f"Could not find the Level {level} button.")

    def paste_text(self, text: str):
        ta = self._textarea()
        ta.click()
        ta.fill(text)
        self._pause(0.1, 0.25)

    def click_humanize(self):
        """Press Humanize and wait until the rewritten text is REALLY there: clickable sentences and a working Copy button.
        (The heading "Humanized Result" can already be on the page, empty, before anything was generated.)"""
        from playwright.sync_api import expect

        btn = self.page.get_by_role("button", name=re.compile(r"^\s*Humanize\s*$", re.I)).first
        expect(btn).to_be_enabled(timeout=15000)
        self._pause(0.05, 0.15)
        btn.click()
        timeout = int(self.site.get("humanize_timeout_seconds", 240))
        t0 = time.time()
        saw_loading = False
        retried = 0
        stable = 0
        last_n = -1
        empty_since = 0.0
        last_log = 0.0
        dumped = False
        while time.time() - t0 < timeout:
            if self.page.is_closed():
                raise SiteError("The browser window was closed.")
            st = self._state()
            elapsed = time.time() - t0
            loading = bool(st.get("spinner")) or bool(st.get("btnDisabled")) or bool(re.search(r"ing\b|\.\.\.|…", st.get("btn") or "") and (st.get("btn") or "").lower() != "humanize")
            saw_loading = saw_loading or loading
            if self._limit_visible():
                self.dump("limit")
                raise LimitReached("The website says the daily limit was reached.")
            bad = [a for a in st.get("alerts", []) if re.search(r"error|failed|went wrong|try again|invalid|too (short|long)|not enough|minimum|maximum", a, re.I)]
            if bad and not loading:
                self.dump("humanize-error")
                raise SiteError("stealthwriter.ai says: " + bad[0][:200])
            n = 0
            if st.get("copyEnabled") and not loading:
                try:
                    if self._result_card().count():
                        n = len(self._tag_sentences()[2])
                except Exception:
                    n = 0
            if st.get("copyEnabled") and not loading and n == 0:
                empty_since = empty_since or time.time()
                if time.time() - empty_since > 12:
                    self.dump("result-no-sentences", with_card=True)
                    raise SiteError("The result is on the page but I cannot find its sentences to click. "
                                    "Saved the page to " + str(self.debug_dir) + " (result-no-sentences.html).")
            else:
                empty_since = 0.0
            if n >= 1 and n == last_n:
                stable += 1
                if stable >= 2:
                    self.log(f"  result is ready ({n} sentences, {int(elapsed)}s)")
                    try:
                        self._result_card().scroll_into_view_if_needed(timeout=3000)
                    except Exception:
                        pass
                    return
            else:
                stable = 0
            last_n = n
            if elapsed - last_log >= 15:
                last_log = elapsed
                self.log(f"  waiting for stealthwriter... {int(elapsed)}s ({'working' if loading else 'idle'}, {n} sentences)")
            # the click did not start anything: try again a different way, once or twice
            if not saw_loading and n == 0 and elapsed > 8 + 10 * retried and retried < 2:
                retried += 1
                self.log("  nothing started after the click; trying again")
                self.dump(f"humanize-noclick{retried}")
                try:
                    banner = self.page.get_by_text(re.compile("in-text citations", re.I)).first
                    if banner.is_visible():
                        banner.locator("xpath=ancestor::*[.//button][1]").get_by_role("button").first.click(timeout=2000)
                except Exception:
                    pass
                try:
                    btn.scroll_into_view_if_needed(timeout=3000)
                    if retried == 1:
                        btn.click(timeout=5000)
                    else:
                        btn.evaluate("b => b.click()")
                except Exception:
                    pass
            if elapsed > 25 and not dumped:
                dumped = True
                self.dump("humanize-slow")
            _sleep(1)
        self.dump("humanize-timeout")
        raise SiteError("Timed out waiting for the humanized text" + ("" if saw_loading else " (the Humanize button seemed to do nothing)")
                        + ". Saved the page to " + str(self.debug_dir))

    # ------------------------------------------------------------------ overall score + Rehumanize
    def read_score(self) -> dict:
        try:
            return self.page.evaluate(JS_SCORE)
        except Exception:
            return {"found": False}

    def _wait_score(self, timeout: int = 60):
        """The % human shown in the 'Detection Score' box once the scan has finished (None if it never shows)."""
        t0 = time.time()
        last, same = None, 0
        while time.time() - t0 < timeout:
            sc = self.read_score()
            v = sc.get("human") if not sc.get("scanning") else None
            if v is not None and v == last:
                same += 1
                if same >= 2:
                    return v
            else:
                same = 0
            last = v
            _sleep(0.5)
        return last

    def _signature(self) -> str:
        try:
            return "|".join(i["text"] for i in self._tag_sentences()[2])
        except Exception:
            return ""

    def _click_rehumanize(self) -> bool:
        card = self._result_card()
        btn = card.get_by_role("button", name=re.compile(r"^\s*Rehumanize\s*$", re.I)).first
        if not btn.count():
            btn = self.page.get_by_role("button", name=re.compile(r"^\s*Rehumanize\s*$", re.I)).first
        for _ in range(30):
            try:
                if btn.is_enabled():
                    break
            except Exception:
                pass
            _sleep(0.5)
        self._click(btn, "Rehumanize button")
        return True

    def _wait_rehumanized(self, old_sig: str) -> bool:
        timeout = int(self.site.get("humanize_timeout_seconds", 240))
        t0 = time.time()
        stable, last, saw_loading, retried = 0, None, False, False
        while time.time() - t0 < timeout:
            if self._limit_visible():
                raise LimitReached("The website says the daily limit was reached.")
            st = self._state()
            loading = bool(st.get("spinner")) or bool(st.get("btnDisabled"))
            saw_loading = saw_loading or loading
            sig = self._signature()
            if sig and sig != old_sig and not loading and st.get("copyEnabled"):
                stable = stable + 1 if sig == last else 0
                if stable >= 2:
                    return True
            else:
                stable = 0
            last = sig
            el = time.time() - t0
            if not saw_loading and sig == old_sig and el > 20 and not retried:
                retried = True
                self.log("  Rehumanize did not start; clicking it again")
                try:
                    self._click_rehumanize()
                except Exception:
                    pass
            if not saw_loading and sig == old_sig and el > 70:
                self.dump("rehumanize-nothing", with_card=True)
                return False
            _sleep(1.0)
        self.dump("rehumanize-timeout", with_card=True)
        return False

    def reach_target_score(self, label: str = "") -> dict:
        """Press Rehumanize until the Detection Score says 100% human (or the try limit is used up)."""
        target = int(self.site.get("target_score", 100))
        max_tries = int(self.site.get("max_rehumanize", 8))
        info = {"batch": label, "target": target, "start": None, "final": None, "rehumanized": 0}
        score = self._wait_score()
        info["start"] = info["final"] = score
        if score is None:
            self.log("  (could not read the Detection Score; going on without it)")
            return info
        self.log(f"  Detection Score: {score}% human")
        while score < target and info["rehumanized"] < max_tries:
            info["rehumanized"] += 1
            self.log(f"  {score}% human is under {target}%: pressing Rehumanize ({info['rehumanized']}/{max_tries})")
            old = self._signature()
            try:
                self._click_rehumanize()
                ok = self._wait_rehumanized(old)
            except LimitReached:
                self.log("  The daily limit was reached while rehumanizing; keeping the current version.")
                break
            except SiteError as e:
                self.log(f"  Rehumanize problem: {e}")
                break
            if not ok:
                self.log("  Rehumanize gave no new text; keeping the current version.")
                break
            new = self._wait_score()
            if new is None:
                break
            self.log(f"  Detection Score is now {new}% human")
            score = new
            info["final"] = score
        if score >= target:
            self.log(f"  Reached {score}% human.")
        else:
            self.log(f"  Stopped at {score}% human (goal {target}%).")
        return info

    def _limit_visible(self) -> bool:
        try:
            return self.page.get_by_text(LIMIT_RX).first.is_visible()
        except Exception:
            return False

    # ------------------------------------------------------------------ result card + sentences
    def _result_card(self):
        """The whole result card: heading, toolbar (Copy ...) AND the block with the sentences.
        On the real page the toolbar and the sentences are siblings, so the closest block that holds a Copy button is only the toolbar;
        we need the closest block that holds a Copy button and clickable sentences (role=button / pointer text), never the input box."""
        title = self.page.get_by_text("Humanized Result", exact=True).first
        copy = "button[normalize-space()='Copy' or @aria-label='Copy' or @title='Copy']"
        for xp in (
            f"xpath=ancestor::*[.//{copy} and .//*[@role='button' and not(self::button)] and not(.//textarea)][1]",
            f"xpath=ancestor::*[.//{copy} and string-length(normalize-space(.)) > 160 and not(.//textarea)][1]",
            f"xpath=ancestor::*[.//{copy} and not(.//textarea)][1]",
        ):
            loc = title.locator(xp)
            try:
                if loc.count():
                    return loc
            except Exception:
                continue
        return title.locator(f"xpath=ancestor::*[.//{copy}][1]")

    def _tag_sentences(self):
        card = self._result_card()
        data = card.evaluate(JS_TAG_SENTENCES, self.rev.get("sentence_selector") or None)
        return card, data["method"], data["items"]

    def _click(self, loc, what: str = "button"):
        """Click that survives overlays and elements outside the viewport: normal click, then forced click, then a DOM click."""
        try:
            loc.scroll_into_view_if_needed(timeout=3000)
        except Exception:
            pass
        for how in ("normal", "force", "js"):
            try:
                if how == "normal":
                    loc.click(timeout=4000)
                elif how == "force":
                    loc.click(timeout=3000, force=True)
                else:
                    loc.evaluate("e => e.click()")
                return
            except Exception as e:
                last = e
        raise SiteError(f"Could not click the {what}: {str(last)[:150]}")

    def _dialog(self):
        marker = self.page.get_by_text("Pick a rewrite for this sentence").first
        return marker.locator("xpath=ancestor::*[.//button[contains(normalize-space(), 'New alternatives')]][1]")

    def _wait_dialog_open(self):
        self.page.get_by_text("Pick a rewrite for this sentence").first.wait_for(state="visible", timeout=8000)

    def _dialog_open(self) -> bool:
        try:
            return self.page.get_by_text("Pick a rewrite for this sentence").first.is_visible()
        except Exception:
            return False

    def _read_options(self) -> list[dict]:
        return self._dialog().evaluate(JS_READ_OPTIONS, timeout=6000)

    def _new_alternatives(self, before: list[str]):
        dlg = self._dialog()
        btn = dlg.get_by_role("button", name=re.compile("New alternatives", re.I)).first
        self._click(btn, "'New alternatives' button")
        t0 = time.time()
        while time.time() - t0 < 30:
            _sleep(0.35)
            try:
                cur = [o["text"] for o in self._read_options()]
            except Exception as e:
                self.log(f"    (reading options failed: {str(e)[:120]})")
                continue
            if cur and cur != before:
                return
        self.log("    (new alternatives did not change in 30s; continuing with what is shown)")

    def _pick_option(self, k: int):
        dlg = self._dialog()
        target = dlg.locator(f'[data-sw-o="{k}"]').first
        self._click(target, "option")
        self._pause(0.05, 0.12)
        for _ in range(20):  # most likely the dialog closes by itself
            if not self._dialog_open():
                return
            _sleep(0.1)
        # otherwise confirm with an apply-like button, else close it with the X
        try:
            dlg = self._dialog()
            for rx in (r"^(apply|use|select|confirm|done|save|ok)\b", r"^(close|cancel)$"):
                b = dlg.get_by_role("button", name=re.compile(rx, re.I))
                if b.count():
                    self._click(b.first, "dialog button")
                    break
            else:
                self.page.keyboard.press("Escape")
        except Exception:
            self.page.keyboard.press("Escape")
        self._pause(0.05, 0.12)

    def review_sentences(self, label: str):
        """Open every sentence, read all its options and choose the best one (New alternatives when none is good).
        Red sentences must end up with a GREEN option: New alternatives is pressed until one shows up. Then a second look at anything still red."""
        card, method, items = self._tag_sentences()
        n = len(items)
        self.log(f"  {n} clickable sentences found (method: {method}; {sum(1 for i in items if i['color'] == 'red')} red).")
        if n == 0:
            self.shot(f"{label}-no-sentences")
            raise SiteError("No clickable sentences found in the result. Run --inspect to calibrate selectors.")
        policy = self.rev.get("sentences", "all")
        todo = [i for i in range(n) if not ((policy == "red" and items[i]["color"] != "red") or (policy == "green" and items[i]["color"] != "green"))]
        for pass_no in range(1 + int(self.rev.get("red_passes", 2))):
            if pass_no:
                card, _, items = self._tag_sentences()
                if len(items) != n:
                    break
                todo = [i for i, it in enumerate(items) if it["color"] == "red"]
                if not todo:
                    break
                self.log(f"  {len(todo)} sentence(s) are still red; going over them again")
            for i in todo:
                card, _, items = self._tag_sentences()
                if len(items) != n:
                    self.log(f"  sentence count changed ({n} -> {len(items)}); stopping review for this batch.")
                    return
                self._review_one(label, card, items, i, n)
        card, _, items = self._tag_sentences()
        left = sum(1 for it in items if it["color"] == "red")
        self.red_left = left
        self.log("  no red sentences left." if left == 0 else f"  {left} sentence(s) are still red (no green option was offered).")

    def _review_one(self, label, card, items, i, n):
        """One sentence, with up to 3 tries: when the page lost the window (you clicked elsewhere, it re-rendered), open it again."""
        last = None
        for attempt in range(3):
            try:
                return self._review_one_once(label, card, items, i, n)
            except Exception as e:
                if e.__class__.__name__ in ("LimitReached", "Cancelled", "MappingError"):
                    raise
                last = e
                self.log(f"  sentence {i + 1}: the page lost the window ({str(e)[:90]}); trying again ({attempt + 1}/3)")
                self.shot(f"{label}-s{i}-retry{attempt}")
                try:
                    if self._dialog_open():
                        self.page.keyboard.press("Escape")
                except Exception:
                    pass
                _sleep(1.0 + attempt)
                try:
                    card, _, items2 = self._tag_sentences()
                    if len(items2) == n:
                        items = items2
                except Exception:
                    pass
        self.log(f"  sentence {i + 1}: could not be changed ({str(last)[:120]}); it stays as it was.")

    def _review_one_once(self, label, card, items, i, n):
        color = items[i]["color"]
        el = card.locator(f'[data-sw-s="{i}"]').first
        self._click(el, "sentence")
        try:
            self._wait_dialog_open()
            if not getattr(self, "_dialog_dumped", False):   # keep one copy of the real dialog's markup for later fixes
                self._dialog_dumped = True
                self.shot(f"{label}-dialog")
                try:
                    (self.debug_dir / "first-dialog.html").write_text(self._dialog().evaluate("e => e.outerHTML"), encoding="utf-8")
                except Exception:
                    pass
        except Exception:
            self.shot(f"{label}-s{i}-no-dialog")
            self.log(f"  sentence {i + 1}: dialog did not open, skipping.")
            return
        need_green = color == "red"
        max_regen = int(self.rev.get("max_new_alternatives_red", 6)) if need_green else int(self.rev.get("max_new_alternatives", 3))
        regen = 0
        while True:
            opts = self._read_options()
            if not opts:
                self.shot(f"{label}-s{i}-no-options")
                self.log(f"  sentence {i + 1}: no options readable, keeping as is.")
                break
            orig = next((o["text"] for o in opts if o["label"] == "Original"), items[i]["text"])
            allow_orig = (color == "green" and bool(self.rev.get("allow_original_when_green", False)))
            eligible = [o for o in opts if o["label"] != "Original" or allow_orig]
            greens = [o for o in eligible if o.get("dot") == "green"]
            knows_dots = any(o.get("dot") in ("green", "red") for o in opts)
            pool = greens if knows_dots else eligible   # a red-dot option would turn the sentence red: never pick one over a green one
            if knows_dots and not greens:
                if regen < max_regen:
                    regen += 1
                    self.log(f"  sentence {i + 1} [{color}]: no green option; New alternatives ({regen}/{max_regen})")
                    self._new_alternatives([o["text"] for o in opts])
                    continue
                if not need_green:   # green already and nothing better that is also green: leave it as it is
                    self.log(f"  sentence {i + 1} [green]: no green alternative offered, keeping it.")
                    break
            cands = [Candidate(o["i"], o["label"], o["text"], {"green": True, "red": False}.get(o.get("dot"))) for o in pool]
            force = regen >= max_regen
            ctx = dict(original=orig, before=items[i - 1]["text"] if i > 0 else "", after=items[i + 1]["text"] if i + 1 < n else "",
                       allow_original=allow_orig, must_replace=need_green)
            decision = self.judge.choose(JudgeRequest(candidates=cands, force=force, **ctx)) if cands else None
            if decision is None or decision.idx is None:
                if not force:
                    regen += 1
                    why = decision.reason if decision else "nothing eligible"
                    self.log(f"  sentence {i + 1}: not good enough ({why}); New alternatives ({regen}/{max_regen})")
                    self._new_alternatives([o["text"] for o in opts])
                    continue
                if need_green and not cands and eligible:   # a red sentence and no green option ever came: the best of what is shown
                    every = [Candidate(o["i"], o["label"], o["text"], {"green": True, "red": False}.get(o.get("dot"))) for o in eligible]
                    decision = self.judge.choose(JudgeRequest(candidates=every, force=True, **ctx))
                if decision is None or decision.idx is None:
                    self.log(f"  sentence {i + 1} [{color}]: no option was good enough, keeping it as it was ({decision.reason if decision else 'no options'}).")
                    break
            chosen = next(o for o in opts if o["i"] == decision.idx)
            self._pick_option(decision.idx)
            self.review_log.append({"batch": label, "sentence": i + 1, "color": color, "picked": chosen["label"], "dot": chosen.get("dot"),
                                    "text": chosen["text"], "regenerations": regen, "why": decision.reason})
            self.log(f"  sentence {i + 1}/{n} [{color}] -> {chosen['label']} ({chosen.get('dot') or '-'} dot; {decision.reason[:70]})")
            break
        if self._dialog_open():  # nothing was picked: close without changing anything
            self.page.keyboard.press("Escape")
            self._pause(0.05, 0.15)

    def copy_result(self) -> str:
        card = self._result_card()
        card.get_by_role("button", name=re.compile(r"^\s*Copy\s*$", re.I)).first.click()
        self._pause(0.15, 0.3)
        try:
            text = self.page.evaluate("navigator.clipboard.readText()")
            if text and text.strip():
                return text
        except Exception:
            pass
        self.log("  (clipboard not readable; reading the text from the result panel instead)")
        card, _, _ = self._tag_sentences()
        return card.evaluate(JS_COMMON_ANCESTOR_TEXT)

    # ------------------------------------------------------------------ public API
    def humanize_batch(self, paragraphs: list[str], label: str = "batch") -> list[str]:
        """Humanize several paragraphs in one go; returns one string per input paragraph."""
        text = "\n\n".join(paragraphs)
        self.log(f"[{label}] opening the Humanizer ({len(text.split())} words, {len(paragraphs)} paragraphs)")
        self.open_humanizer()
        self.select_level(int(self.site.get("level", 10)))
        self.paste_text(text)
        self.shot(f"{label}-pasted")
        self.click_humanize()
        self._pause(0.2, 0.4)
        sc = self.reach_target_score(label)
        self.shot(f"{label}-result")
        self.dump(f"{label}-result", with_card=True)
        try:
            self.review_sentences(label)
            after = self.read_score()
            if after.get("human") is not None and not after.get("scanning"):
                sc["after_review"] = after["human"]
                self.log(f"  Detection Score after choosing sentences: {after['human']}% human")
            self.scores.append(sc)
            out = self.copy_result()
        except Exception:
            self.dump(f"{label}-review-failed", with_card=True)
            raise
        out = re.sub(r"([a-z]{2}[.!?])([A-Z][a-z])", r"\1 \2", out)  # repair missing spaces between sentences
        parts = split_paragraphs(out)
        if len(parts) != len(paragraphs):
            single = [p.strip() for p in out.strip().split("\n") if p.strip()]
            if len(single) == len(paragraphs):
                parts = single
            else:
                dump = self.debug_dir / f"{label}-unmapped.txt"
                self.debug_dir.mkdir(parents=True, exist_ok=True)
                dump.write_text(out, encoding="utf-8")
                raise MappingError(
                    f"Sent {len(paragraphs)} paragraphs but the copied text has {len(parts)}. Saved it to {dump}."
                )
        return parts

    # ------------------------------------------------------------------ calibration helper
    def inspect(self):
        """Interactive helper: saves page structure so selectors can be verified/fixed."""
        self.debug_dir.mkdir(parents=True, exist_ok=True)
        self.open_humanizer()
        self.select_level(int(self.site.get("level", 10)))
        buttons = self.page.evaluate(
            "Array.from(document.querySelectorAll('button,[role=radio],[role=tab]')).map(b=>(b.innerText||b.getAttribute('aria-label')||'').trim()).filter(Boolean)"
        )
        (self.debug_dir / "inspect-buttons.json").write_text(json.dumps(buttons, indent=2), encoding="utf-8")
        (self.debug_dir / "inspect-1-humanizer.html").write_text(self.page.content(), encoding="utf-8")
        self.shot("inspect-1-humanizer")
        print("\nSaved the Humanizer page structure.")
        input("STEP 1: In the browser, paste a short test text and click Humanize. When the result is showing, press Enter here... ")
        self.shot("inspect-2-result")
        try:
            card, method, items = self._tag_sentences()
            (self.debug_dir / "inspect-2-result-card.html").write_text(card.evaluate("e => e.outerHTML"), encoding="utf-8")
            (self.debug_dir / "inspect-2-sentences.json").write_text(json.dumps({"method": method, "items": items}, indent=2), encoding="utf-8")
            print(f"Found {len(items)} clickable sentences via '{method}'.")
        except Exception as e:
            print("Could not analyse the result card:", e)
        input("STEP 2: Click ONE sentence so the 'Pick a rewrite' window opens, then press Enter here... ")
        self.shot("inspect-3-dialog")
        try:
            dlg = self._dialog()
            (self.debug_dir / "inspect-3-dialog.html").write_text(dlg.evaluate("e => e.outerHTML"), encoding="utf-8")
            opts = self._read_options()
            (self.debug_dir / "inspect-3-options.json").write_text(json.dumps(opts, indent=2, ensure_ascii=False), encoding="utf-8")
            print(f"Found {len(opts)} options in the dialog:", [o["label"] for o in opts])
        except Exception as e:
            print("Could not analyse the dialog:", e)
        print(f"\nDone. Send me the files in: {self.debug_dir.resolve()}")
