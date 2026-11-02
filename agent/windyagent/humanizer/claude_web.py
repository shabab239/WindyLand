"""Ask Claude (your own account) through claude.ai in the browser window Tobi controls.

The Claude desktop app cannot be driven from outside reliably, but it is the same account as claude.ai, so Tobi opens claude.ai in his
browser window (you sign in there once, like on stealthwriter), types the question and reads the answer. Used by the sentence check.
"""
from __future__ import annotations

import json
import re
import time

from .stop import sleep as _sleep
from .stealth_client import SiteError, StealthwriterClient

EDITOR = 'div[contenteditable="true"], textarea'


def last_scored_array(text: str) -> str:
    """The last JSON list in `text` whose items carry a "score" (the answer, not the question that also contains lists)."""
    dec = json.JSONDecoder()
    best = ""
    for m in re.finditer(r"\[", text):
        try:
            obj, end = dec.raw_decode(text[m.start():])
        except ValueError:
            continue
        if isinstance(obj, list) and obj and all(isinstance(x, dict) and "score" in x for x in obj):
            best = json.dumps(obj, ensure_ascii=False)
    return best


QUOTES = str.maketrans({"\u201c": '"', "\u201d": '"', "\u201e": '"', "\u201f": '"', "\u00ab": '"', "\u00bb": '"'})
LOOSE_HEAD = re.compile(r'"overall"\s*:\s*(\d{1,3}(?:\.\d+)?)\s*,\s*"weak"\s*:\s*\[')
LOOSE_ITEM = re.compile(r'"sentence"\s*:\s*"(.*?)"\s*,\s*"why"\s*:\s*"(.*?)"\s*\}', re.S)


def _strict_answers(t: str) -> list[tuple[int, int, str]]:
    dec = json.JSONDecoder()
    out, pos = [], 0
    for m in re.finditer(r"[\{\[]", t):
        if m.start() < pos:
            continue
        try:
            obj, end = dec.raw_decode(t[m.start():])
        except ValueError:
            continue
        ok = (isinstance(obj, dict) and (isinstance(obj.get("edits"), list) or isinstance(obj.get("weak"), list))) or \
             (isinstance(obj, list) and obj and all(isinstance(x, dict) and "score" in x for x in obj))
        if ok:
            out.append((m.start(), m.start() + end, json.dumps(obj, ensure_ascii=False)))
            pos = m.start() + end
    return out


def _loose_answers(t: str) -> list[tuple[int, int, str]]:
    """claude.ai shows the JSON as ordinary text with curly quotes and sentences that contain quotes of their own: the quotes cannot
    be parsed as JSON, so the {"overall", "weak": [{"sentence", "why"}]} shape is read by its keys."""
    out = []
    for m in LOOSE_HEAD.finditer(t):
        end = t.find("]}", m.end())
        if end < 0:                           # still being written
            continue
        seg = t[m.end():end + 1]
        weak = [{"sentence": s.strip(), "why": w.strip()} for s, w in LOOSE_ITEM.findall(seg)]
        out.append((m.start(), end + 2, json.dumps({"overall": float(m.group(1)), "weak": weak}, ensure_ascii=False)))
    return out


def all_answers(text: str) -> list[str]:
    """Every answer-shaped JSON in the page, in order, not overlapping: review objects (with an "edits"/"weak" list) and scored lists.
    Curly quotes (how claude.ai shows plain text) are read as straight ones."""
    t = text.translate(QUOTES)
    strict = _strict_answers(t)
    loose = [x for x in _loose_answers(t) if not any(s <= x[0] < e for s, e, _ in strict)]
    return [j for _, _, j in sorted(strict + loose)]


def last_review_object(text: str) -> str:
    """The last review answer (the instructions only show a shape that is not valid JSON)."""
    rev = [a for a in all_answers(text) if a.startswith("{")]
    return rev[-1] if rev else ""


def extract_answer(text: str) -> str:
    a = all_answers(text)
    return a[-1] if a else ""


class ClaudeWebClient(StealthwriterClient):
    def __init__(self, cfg: dict, log=print, notify=None):
        super().__init__(cfg, None, log)
        self.ccfg = cfg.get("claude_check", {})
        self.notify = notify or (lambda *a: None)
        self.ready = False
        self.seen = 0   # how many answers the chat already holds (a repeated question can get an identical answer)

    def _editor(self):
        return self.page.locator(EDITOR).first

    def _on_login(self) -> bool:
        try:
            return bool(re.search(r"/login|/signup|/logout|magic-link|accounts\.google", self.page.url, re.I)) or \
                self.page.get_by_text(re.compile(r"continue with google|log in|sign in", re.I)).first.is_visible() and not self._editor().is_visible()
        except Exception:
            return False

    def open_new_chat(self):
        url = self.ccfg.get("web_url", "https://claude.ai/new")
        self.page.goto(url, wait_until="domcontentloaded")
        deadline = time.time() + int(self.ccfg.get("login_timeout_seconds", 600))
        announced = False
        while time.time() < deadline:
            try:
                if self._editor().is_visible() and not re.search(r"/login|/logout", self.page.url, re.I):
                    self.ready = True
                    self._prefer_fast_model()
                    return
            except Exception:
                pass
            if not announced:
                announced = True
                msg = "Please sign in to claude.ai in the browser window I opened (only needed once). I am waiting..."
                self.log(">>> " + msg)
                self.notify("claude.ai sign-in", msg)
            _sleep(1.5)
        self.dump("claude-login-timeout")
        raise SiteError("Timed out waiting for you to sign in to claude.ai.")

    def _prefer_fast_model(self):
        """Best effort: a bigger model (Opus, with thinking) takes minutes for this job; Haiku / Sonnet answer in seconds, and extended
        thinking is switched off. Set claude_check.web_model to "" to leave claude.ai's model alone, or to e.g. "Sonnet"."""
        want = [w.strip() for w in str(self.ccfg.get("web_model", "Haiku,Sonnet")).split(",") if w.strip()]
        if not want:
            return
        pick = '[role="menuitem"], [role="option"], [role="menuitemradio"], [role="menuitemcheckbox"]'
        try:
            btn = self.page.locator('[data-testid="model-selector-dropdown"], button[aria-label*="model" i]').first
            btn.wait_for(state="visible", timeout=4000)
            current = (btn.inner_text() or "").strip()
            fast_now = any(w.lower() in current.lower() for w in want)
            if not fast_now:
                btn.click(timeout=3000)
                _sleep(0.5)
                chosen = False
                for attempt in (0, 1):
                    for w in want:
                        item = self.page.locator(pick).filter(has_text=re.compile(w, re.I)).first
                        if item.is_visible():
                            item.click(timeout=3000)
                            self.log(f"  (claude.ai: using the faster model '{w}' for the review)")
                            chosen = True
                            break
                    if chosen:
                        break
                    more = self.page.locator(pick).filter(has_text=re.compile(r"more models|other models", re.I)).first
                    if attempt == 0 and more.is_visible():
                        more.click(timeout=3000)
                        _sleep(0.5)
                if not chosen:
                    self.page.keyboard.press("Escape")
                _sleep(0.4)
            self._thinking_off()
        except Exception:
            try:
                self.page.keyboard.press("Escape")
            except Exception:
                pass

    def _thinking_off(self):
        """Switches 'extended thinking' off when claude.ai shows it as on (it makes every answer much slower)."""
        try:
            for sel in ('button[role="switch"][aria-label*="thinking" i]', '[role="switch"][aria-label*="thinking" i]',
                        'button[aria-label*="extended thinking" i]'):
                sw = self.page.locator(sel).first
                if sw.is_visible() and (sw.get_attribute("aria-checked") == "true" or sw.get_attribute("aria-pressed") == "true"):
                    sw.click(timeout=2000)
                    self.log("  (claude.ai: extended thinking switched off to answer faster)")
                    return
        except Exception:
            pass

    def _editor_len(self) -> int:
        try:
            return len((self._editor().inner_text() or "").strip())
        except Exception:
            return 0

    def _send(self, ed) -> bool:
        """Presses Send and makes sure it went (a long paste can take a moment to be accepted); True when claude.ai took the message."""
        for attempt in range(4):
            try:
                send = self.page.locator('button[aria-label*="Send" i]').first
                if send.is_visible() and send.is_enabled():
                    send.click(timeout=3000)
                else:
                    self.page.keyboard.press("Enter")
            except Exception:
                try:
                    self.page.keyboard.press("Enter")
                except Exception:
                    pass
            for _ in range(10):
                _sleep(0.5)
                if self._busy() or self._editor_len() < 20:
                    return True
            self.log("  (claude.ai has not taken the message yet; sending again)")
        return False

    def _busy(self) -> bool:
        try:
            return self.page.locator('button[aria-label*="Stop" i]').first.is_visible()
        except Exception:
            return False

    def ask(self, system: str, user: str, max_tokens: int = 0) -> str:
        """Asks in ONE chat (opened on the first question and kept for the next ones); returns the JSON Claude answered with."""
        if not self.ready:
            self.open_new_chat()
        prompt = system + "\n\nAnswer straight away with the JSON only (put it in one ```json code block; keep it short; no extended thinking, no thinking out loud, no explanation, nothing else).\n\n" + user
        try:
            self.seen = len(all_answers(self.page.inner_text("body")))
        except Exception:
            self.seen = 0
        ed = self._editor()
        ed.click()
        self.page.keyboard.insert_text(prompt)
        _sleep(0.8)
        sent = self._send(ed)
        timeout = int(self.ccfg.get("web_answer_seconds", 90))   # then Tobi uses his own model instead of waiting minutes
        t0 = time.time()
        started, last, same = False, "", 0
        nextlog = 20
        n = 0
        while time.time() - t0 < timeout:
            _sleep(0.6)
            n += 1
            if time.time() - t0 > nextlog:
                self.log(f"  ...waiting for Claude ({int(time.time() - t0)}s)")
                nextlog += 20
            busy = self._busy()
            started = started or busy
            if busy and n % 4:           # still writing: reading the whole (long) page every moment only slows the browser down
                continue
            try:
                body = self.page.inner_text("body")
            except Exception:
                body = ""
            found = all_answers(body)
            ans = found[-1] if len(found) > self.seen else ""
            if ans:
                same = same + 1 if ans == last else 0
                last = ans
                if same >= (1 if not busy else 3):      # an answer that stopped changing is final even if the Stop button seems stuck
                    self.log(f"  Claude answered in {int(time.time() - t0)}s.")
                    return ans
            else:
                last, same = "", 0
            if re.search(r"usage limit|message limit|rate limit|reached your", body[-1500:], re.I) and not ans and not busy and time.time() - t0 > 20:
                self.dump("claude-limit")
                raise SiteError("claude.ai says a usage limit was reached.")
        info = f"message sent: {'yes' if sent else 'NO'}, claude.ai was writing: {'yes' if started else 'no'}"
        try:
            stop = self.page.locator('button[aria-label*="Stop" i]').first
            if stop.is_visible():
                stop.click(timeout=2000)
        except Exception:
            pass
        self.dump("claude-no-answer")
        raise SiteError(f"claude.ai gave no readable answer within {timeout}s ({info}; page saved to {self.debug_dir})")


class ChatGPTWebClient(ClaudeWebClient):
    """The same job on chatgpt.com (your own ChatGPT account, or without one): used when claude.ai does not answer."""
    SITE = "ChatGPT"

    def _prefer_fast_model(self):
        pass

    def open_new_chat(self):
        url = self.ccfg.get("chatgpt_url", "https://chatgpt.com/")
        self.page.goto(url, wait_until="domcontentloaded")
        deadline = time.time() + int(self.ccfg.get("login_timeout_seconds", 600))
        announced = False
        while time.time() < deadline:
            try:
                if self._editor().is_visible():
                    self.ready = True
                    return
            except Exception:
                pass
            if not announced:
                announced = True
                msg = "ChatGPT needs a moment (or a sign-in) in the browser window I opened. I am waiting..."
                self.log(">>> " + msg)
                self.notify("ChatGPT", msg)
            _sleep(1.5)
        self.dump("chatgpt-login-timeout")
        raise SiteError("Timed out waiting for chatgpt.com.")
