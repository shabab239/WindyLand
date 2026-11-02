"""Entertainment tools: YouTube music and videos, anime, media keys and volume.

YouTube:  the search page is read and the best matching video is opened directly (it starts playing).
Anime:    the anime site's own search is used to find the series, then an episode page is opened; if
          Playwright is available the page is opened in a controlled browser window and the player is clicked.
"""
from __future__ import annotations

import difflib
import json
import random
import re
import subprocess
import sys
import time
import urllib.parse
import urllib.request
import webbrowser

from . import config, memory
from .config import IS_WIN

UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36"

DEFAULT_TOPICS = [
    "mind blowing nature documentary", "space and universe documentary", "ancient civilizations documentary",
    "deep ocean creatures documentary", "how it's made factory documentary", "mega engineering projects documentary",
    "unsolved mysteries documentary", "wildlife documentary", "history documentary full episode",
    "science explained documentary", "travel documentary full episode", "human body documentary",
    "future technology documentary", "street food around the world", "big cats documentary", "dinosaurs documentary",
]


def _get(url: str, data: bytes | None = None, timeout: int = 25) -> str:
    req = urllib.request.Request(url, data=data, headers={"User-Agent": UA, "Accept-Language": "en-US,en;q=0.9"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.read(4_000_000).decode("utf-8", "replace")


# ------------------------------------------------------------------ YouTube
def _walk(node, key: str, out: list):
    if isinstance(node, dict):
        if key in node:
            out.append(node[key])
        for v in node.values():
            _walk(v, key, out)
    elif isinstance(node, list):
        for v in node:
            _walk(v, key, out)


def _seconds(text: str) -> int:
    parts = [int(p) for p in re.findall(r"\d+", text or "")]
    s = 0
    for p in parts:
        s = s * 60 + p
    return s


def parse_youtube_results(html: str) -> list[dict]:
    """Videos from a YouTube results page, in the order shown (live streams and shorts without a length are skipped)."""
    m = re.search(r"var ytInitialData = (\{.*?\});</script>", html, re.S)
    if not m:
        return []
    try:
        data = json.loads(m.group(1))
    except json.JSONDecodeError:
        return []
    renderers: list = []
    _walk(data, "videoRenderer", renderers)
    out, seen = [], set()
    for v in renderers:
        vid = v.get("videoId")
        length = (v.get("lengthText") or {}).get("simpleText", "")
        if not vid or vid in seen or not length:
            continue
        seen.add(vid)
        title = "".join(r.get("text", "") for r in (v.get("title") or {}).get("runs", []))
        chan = "".join(r.get("text", "") for r in (v.get("ownerText") or {}).get("runs", []))
        out.append({"id": vid, "title": title, "channel": chan, "length": length, "seconds": _seconds(length),
                    "views": (v.get("viewCountText") or {}).get("simpleText", "")})
    return out


def youtube_search(query: str) -> list[dict]:
    url = "https://www.youtube.com/results?search_query=" + urllib.parse.quote_plus(query) + "&sp=EgIQAQ%3D%3D"
    return parse_youtube_results(_get(url))


def _open_url(url: str) -> None:
    webbrowser.open(url)


def play_youtube(ctx, query: str, min_minutes: int = 0, max_minutes: int = 0, pick: str = "first") -> str:
    try:
        results = youtube_search(query)
    except Exception as e:
        _open_url("https://www.youtube.com/results?search_query=" + urllib.parse.quote_plus(query))
        return f"Could not read YouTube's results ({e}); opened the search page instead."
    lo, hi = int(min_minutes) * 60, int(max_minutes) * 60
    good = [r for r in results if r["seconds"] >= lo and (not hi or r["seconds"] <= hi)]
    if not good:
        _open_url("https://www.youtube.com/results?search_query=" + urllib.parse.quote_plus(query))
        return "No video matched the length wanted; opened the YouTube search page so the user can choose."
    choice = random.choice(good[:6]) if pick == "random" else good[0]
    memory.mark_played(choice["id"])
    _open_url("https://www.youtube.com/watch?v=" + choice["id"])
    return f"Playing on YouTube: \"{choice['title']}\" by {choice['channel']} ({choice['length']}). https://www.youtube.com/watch?v={choice['id']}"


def play_something(ctx, mood: str = "eating", topic: str = "") -> str:
    """Pick something interesting to watch: a remembered interest if there is one, otherwise a fascinating documentary topic."""
    likes = memory.interests()
    topics = [topic] if topic else (random.sample(likes, min(len(likes), 3)) + random.sample(DEFAULT_TOPICS, 4))
    seen = set(memory.played())
    for t in topics:
        try:
            results = youtube_search(t)
        except Exception:
            continue
        good = [r for r in results if 12 * 60 <= r["seconds"] <= 100 * 60 and r["id"] not in seen]
        if good:
            c = random.choice(good[:8])
            memory.mark_played(c["id"])
            _open_url("https://www.youtube.com/watch?v=" + c["id"])
            return f"Chose \"{t}\" and started: \"{c['title']}\" by {c['channel']} ({c['length']}). https://www.youtube.com/watch?v={c['id']}"
    return "Could not find anything on YouTube just now (is the internet on?)."


# ------------------------------------------------------------------ anime
def _site(cfg: dict) -> str:
    return (cfg.get("anime_site") or "https://animesalt.cx").rstrip("/")


def _norm(s: str) -> str:
    return " ".join(re.findall(r"[a-z0-9]+", s.lower()))


def find_series(site: str, title: str) -> list[tuple[float, str, str]]:
    """[(score, kind, url)] best first. kind is 'series' or 'movies'."""
    html = _get(f"{site}/?s={urllib.parse.quote_plus(title)}")
    host = re.escape(urllib.parse.urlparse(site).netloc)
    found, seen = [], set()
    for m in re.finditer(rf'href="(https?://{host}/(series|movies)/([^/"#?]+)/)"', html):
        url, kind, slug = m.groups()
        if url in seen:
            continue
        seen.add(url)
        q, sl = _norm(title), _norm(slug.replace("-", " "))
        score = difflib.SequenceMatcher(None, q, sl).ratio() + (0.3 if q and q in sl else 0) - 0.02 * len(found)
        found.append((score, kind, url))
    return sorted(found, reverse=True)


def episodes_of(series_url: str) -> list[tuple[int, int, str]]:
    html = _get(series_url)
    eps, seen = [], set()
    for m in re.finditer(r'href="(https?://[^"]+/episode/[^"/]+-(\d+)x(\d+)/)"', html):
        url, s, e = m.group(1), int(m.group(2)), int(m.group(3))
        if url not in seen:
            seen.add(url)
            eps.append((s, e, url))
    return sorted(eps)


def open_and_press_play(url: str, cfg: dict) -> str:
    """Open the page in a controlled browser window and click the video player. Falls back to the normal browser."""
    if not cfg.get("anime_autoplay", True):
        _open_url(url)
        return "opened in your browser (press play if it does not start)"
    try:
        from playwright.sync_api import sync_playwright

        from .humanizer.stealth_client import _cdp_ready, find_browser

        port = 9223
        if not _cdp_ready(port):
            profile = config.data_dir() / "media_browser_profile"
            profile.mkdir(parents=True, exist_ok=True)
            subprocess.Popen([find_browser(cfg.get("humanizer_browser_path", ""), cfg.get("humanizer_browser", "auto")), f"--remote-debugging-port={port}", f"--user-data-dir={profile}",
                              "--no-first-run", "--no-default-browser-check", "--start-maximized", "about:blank"],
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            for _ in range(60):
                if _cdp_ready(port):
                    break
                time.sleep(0.5)
        with sync_playwright() as pw:
            browser = pw.chromium.connect_over_cdp(f"http://127.0.0.1:{port}")
            ctx = browser.contexts[0]
            page = ctx.new_page()
            page.goto(url, wait_until="domcontentloaded", timeout=45000)
            clicked = False
            try:
                page.wait_for_selector("iframe", state="attached", timeout=15000)
                time.sleep(2.0)
                for fr in page.locator(".video-player iframe, #player iframe, iframe").all():
                    box = fr.bounding_box()
                    if box and box["width"] > 300 and box["height"] > 150:
                        fr.scroll_into_view_if_needed()
                        box = fr.bounding_box()
                        page.mouse.click(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
                        clicked = True
                        break
                time.sleep(2.5)
                for p in ctx.pages:  # ad pop-ups opened by the click
                    if p != page and p.url not in ("about:blank", ""):
                        try:
                            p.close()
                        except Exception:
                            pass
                page.bring_to_front()
            except Exception:
                pass
        return "opened and pressed play" if clicked else "opened (press play if it does not start)"
    except Exception as e:
        _open_url(url)
        return f"opened in your normal browser ({e.__class__.__name__}); press play if it does not start"


def play_anime(ctx, title: str, season: int = 0, episode: int = 0, which: str = "first") -> str:
    cfg = config.load()
    site = _site(cfg)
    try:
        cands = find_series(site, title)
    except Exception as e:
        _open_url(f"{site}/?s={urllib.parse.quote_plus(title)}")
        return f"Could not read {site} ({e}); opened its search page instead."
    if not cands:
        _open_url(f"{site}/?s={urllib.parse.quote_plus(title)}")
        return f"No result for \"{title}\" on {site}; opened the search page so the user can look."
    score, kind, url = cands[0]
    others = ", ".join(u.rstrip("/").rsplit("/", 1)[-1] for _, _, u in cands[1:4])
    if kind == "movies":
        how = open_and_press_play(url, cfg)
        return f"Found the movie {url} and {how}."
    try:
        eps = episodes_of(url)
    except Exception as e:
        eps = []
    if not eps:
        how = open_and_press_play(url, cfg)
        return f"Found the series {url} but could not read its episode list; {how}."
    if episode:
        s = int(season) or 1
        match = [x for x in eps if x[0] == s and x[1] == int(episode)]
        chosen = match[0] if match else None
        if not chosen:
            return f"{url.rstrip('/').rsplit('/', 1)[-1]} has no season {s} episode {episode} on the site (it lists {len(eps)} episodes, up to S{eps[-1][0]}E{eps[-1][1]})."
    elif which == "latest":
        chosen = eps[-1]
    elif which == "random":
        chosen = random.choice(eps)
    else:
        chosen = eps[0]
    how = open_and_press_play(chosen[2], cfg)
    extra = f" (other matches: {others})" if others else ""
    return f"Series {url.rstrip('/').rsplit('/', 1)[-1]}: season {chosen[0]} episode {chosen[1]} {how}. {chosen[2]}{extra}"


# ------------------------------------------------------------------ media keys / volume
VK = {"play_pause": 0xB3, "next": 0xB0, "previous": 0xB1, "stop": 0xB2, "mute": 0xAD, "volume_down": 0xAE, "volume_up": 0xAF}


def media_control(ctx, action: str, steps: int = 1) -> str:
    if action not in VK:
        return "Unknown action. Use one of: " + ", ".join(VK)
    if not IS_WIN:
        return "Media keys are only available on Windows."
    import ctypes

    n = max(1, min(int(steps), 50)) if action.startswith("volume") else 1
    for _ in range(n):
        ctypes.windll.user32.keybd_event(VK[action], 0, 0, 0)
        ctypes.windll.user32.keybd_event(VK[action], 0, 2, 0)
        time.sleep(0.03)
    return f"Pressed {action}" + (f" x{n} (each step is about 2%)" if n > 1 else "") + "."


# ------------------------------------------------------------------ "play X" when it is not said what X is
def play_auto(ctx, query: str) -> str:
    """\"Play Naruto\" -> anime if the anime site has a title that contains it, otherwise a YouTube search."""
    query = " ".join(str(query).split())
    if not query:
        return "Play what?"
    cfg = config.load()
    try:
        cands = find_series(_site(cfg), query)
    except Exception:
        cands = []
    q = _norm(query)
    for score, kind, url in cands[:3]:
        slug = _norm(url.rstrip("/").rsplit("/", 1)[-1].replace("-", " "))
        if q and (q == slug or (len(q) >= 4 and q in slug)):
            return play_anime(ctx, query)
    return play_youtube(ctx, query)
