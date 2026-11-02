"""Open any program installed on this PC by its name: "open Spotify", "launch Photoshop", "start Excel".

Where the names come from (Windows):
  1. the Start Menu shortcuts (all users + this user), which is what the Windows search box finds,
  2. Microsoft Store / packaged apps (PowerShell `Get-StartApps`),
  3. a few well-known aliases (calculator, task manager, word, vs code ...) and web sites (youtube, gmail ...).
Names are matched loosely ("vs code" finds "Visual Studio Code", "word" finds "Word"). Nothing is run except
the shortcut / app the user asked for.
"""
from __future__ import annotations

import difflib
import json
import os
import re
import subprocess
import time
import webbrowser
from pathlib import Path

from .config import IS_WIN

NO_WINDOW = 0x08000000 if IS_WIN else 0

# name -> command that `start` understands
ALIASES = {
    "notepad": "notepad", "calculator": "calc", "calc": "calc", "paint": "mspaint", "command prompt": "cmd", "cmd": "cmd",
    "terminal": "wt", "windows terminal": "wt", "powershell": "powershell", "task manager": "taskmgr", "file explorer": "explorer",
    "explorer": "explorer", "my computer": "explorer", "this pc": "explorer", "control panel": "control", "settings": "ms-settings:",
    "windows settings": "ms-settings:", "snipping tool": "snippingtool", "word": "winword", "microsoft word": "winword",
    "excel": "excel", "microsoft excel": "excel", "powerpoint": "powerpnt", "microsoft powerpoint": "powerpnt", "outlook": "outlook",
    "onenote": "onenote", "chrome": "chrome", "google chrome": "chrome", "edge": "msedge", "microsoft edge": "msedge",
    "firefox": "firefox", "brave": "brave", "vs code": "code", "vscode": "code", "visual studio code": "code",
    "device manager": "devmgmt.msc", "disk management": "diskmgmt.msc", "registry editor": "regedit", "resource monitor": "resmon",
    "character map": "charmap", "wordpad": "wordpad", "sticky notes": "shell:AppsFolder\\Microsoft.MicrosoftStickyNotes_8wekyb3d8bbwe!App",
}
SITES = {
    "youtube": "https://www.youtube.com", "gmail": "https://mail.google.com", "google": "https://www.google.com",
    "github": "https://github.com", "whatsapp web": "https://web.whatsapp.com", "chatgpt": "https://chatgpt.com",
    "claude": "https://claude.ai", "stealthwriter": "https://www.stealthwriter.ai/dashboard/humanizer",
    "academi": "https://academi.cx", "netflix": "https://www.netflix.com", "facebook": "https://www.facebook.com",
    "instagram": "https://www.instagram.com", "twitter": "https://x.com", "reddit": "https://www.reddit.com",
}

_cache: dict = {"t": 0.0, "apps": []}
_TTL = 600.0


def _norm(s: str) -> str:
    s = s.lower().replace("&", " and ")
    s = re.sub(r"\.(lnk|exe|desktop|app)$", "", s)
    s = re.sub(r"[^a-z0-9+# ]+", " ", s)
    s = re.sub(r"\b(microsoft|app|application|the|my|program|software)\b", " ", s)
    return " ".join(s.split())


def _start_menu_dirs() -> list[Path]:
    env = os.environ.get("WINDYLAND_APP_DIRS")
    if env:
        return [Path(p) for p in env.split(os.pathsep) if p]
    out = []
    for var in ("ProgramData", "APPDATA"):
        base = os.environ.get(var)
        if base:
            out.append(Path(base) / "Microsoft" / "Windows" / "Start Menu" / "Programs")
    return out


def _scan_shortcuts() -> list[dict]:
    apps: dict[str, dict] = {}
    for root in _start_menu_dirs():
        if not root.is_dir():
            continue
        for p in root.rglob("*"):
            if p.suffix.lower() not in (".lnk", ".url", ".appref-ms", ".desktop", ".exe") or not p.is_file():
                continue
            name = p.stem
            if re.search(r"\b(uninstall|remove|readme|release notes|help|documentation|license|website|manual)\b", name, re.I):
                continue
            apps.setdefault(_norm(name), {"name": name, "kind": "shortcut", "target": str(p)})
    return list(apps.values())


def _scan_store_apps() -> list[dict]:
    if not IS_WIN:
        return []
    try:
        r = subprocess.run(["powershell", "-NoProfile", "-NonInteractive", "-Command", "Get-StartApps | ConvertTo-Json -Compress"],
                           capture_output=True, text=True, timeout=25, creationflags=NO_WINDOW)
        data = json.loads(r.stdout or "[]")
    except Exception:
        return []
    if isinstance(data, dict):
        data = [data]
    return [{"name": d["Name"], "kind": "appid", "target": d["AppID"]} for d in data if d.get("Name") and d.get("AppID")]


def index(force: bool = False, store: bool = True) -> list[dict]:
    if not force and _cache["apps"] and time.time() - _cache["t"] < _TTL:
        return _cache["apps"]
    apps = _scan_shortcuts()
    if store:
        seen = {_norm(a["name"]) for a in apps}
        apps += [a for a in _scan_store_apps() if _norm(a["name"]) not in seen]
    _cache.update(t=time.time(), apps=apps)
    return apps


def match(query: str, apps: list[dict] | None = None, limit: int = 5) -> list[tuple[float, dict]]:
    """Best matches first, as (score 0..1, app)."""
    apps = index() if apps is None else apps
    q = _norm(query)
    if not q:
        return []
    qt = q.split()
    out = []
    for a in apps:
        n = _norm(a["name"])
        if not n:
            continue
        words = n.split()
        extra = max(0, len(words) - len(qt))
        if n == q:
            s = 1.0
        elif all(any(w == t or w.startswith(t) for w in words) for t in qt):
            s = max(0.8, 0.95 - 0.05 * extra)            # every word asked for is in the name; fewer extra words = better
        elif n.startswith(q) or q.startswith(n):
            s = 0.82
        elif len(q) >= 4 and q in n:
            s = 0.78
        else:
            s = difflib.SequenceMatcher(None, q, n).ratio() * 0.9
            if s < 0.72:
                continue
        out.append((s, a))
    out.sort(key=lambda x: (-x[0], len(x[1]["name"])))
    return out[:limit]


def _launch(a: dict) -> None:
    if a["kind"] == "appid":
        subprocess.Popen(["explorer.exe", f"shell:AppsFolder\\{a['target']}"], creationflags=NO_WINDOW)
    elif IS_WIN:
        os.startfile(a["target"])  # type: ignore[attr-defined]
    else:
        subprocess.Popen(["xdg-open", a["target"]])


def open_app(name: str) -> str:
    """Opens the program called `name`. Returns a sentence for the user."""
    q = re.sub(r"^(?:the|my|an?)\s+|\s+(?:app|application|program|software)$", "", name.strip(), flags=re.I).strip(" \"'")
    if not q:
        return "Which program should I open?"
    nq = _norm(q)
    if re.match(r"^https?://", q, re.I):
        webbrowser.open(q)
        return f"Opened {q} in your browser."
    found = match(q, index(store=False))
    if not found or found[0][0] < 0.8:
        found = match(q, index(store=True)) or found      # Microsoft Store apps are looked up only when needed
    if found and found[0][0] >= 0.8 and (len(found) == 1 or found[0][0] - found[1][0] >= 0.03 or _norm(found[0][1]["name"]) == nq):
        a = found[0][1]
        try:
            _launch(a)
        except Exception as e:
            return f"I found {a['name']} but could not start it: {e}"
        return f"Opening {a['name']}."
    key = nq if nq in ALIASES else q.lower() if q.lower() in ALIASES else None
    if key:
        cmd = ALIASES[key]
        try:
            if IS_WIN:
                subprocess.Popen(["cmd", "/c", "start", "", cmd], creationflags=NO_WINDOW)
            else:
                subprocess.Popen([cmd])
            return f"Opening {q}."
        except Exception as e:
            return f"Could not start {q}: {e}"
    for site, url in SITES.items():
        if nq == site or nq == site.replace(" ", ""):
            webbrowser.open(url)
            return f"Opened {site} in your browser."
    if found and found[0][0] >= 0.8:      # several equally good matches: ask instead of guessing
        names = ", ".join(a["name"] for _, a in found[:4])
        return f"I found more than one: {names}. Which one do you mean?"
    if IS_WIN and re.fullmatch(r"[\w.\-]{2,40}", q):    # last try: anything Windows itself knows by name (App Paths, PATH)
        try:
            r = subprocess.run(["cmd", "/c", "where", q], capture_output=True, text=True, timeout=5, creationflags=NO_WINDOW)
            if r.returncode == 0:
                subprocess.Popen(["cmd", "/c", "start", "", q], creationflags=NO_WINDOW)
                return f"Opening {q}."
        except Exception:
            pass
    near = [a["name"] for _, a in (match(q, index(), 4) or [])]
    return f"I could not find a program called '{q}'." + (f" Did you mean: {', '.join(near)}?" if near else "")


def list_apps(query: str = "", limit: int = 60) -> str:
    apps = index()
    if query:
        apps = [a for _, a in match(query, apps, 40)]
    names = sorted({a["name"] for a in apps}, key=str.lower)[: int(limit)]
    return ("\n".join(names)) if names else "No installed program matches that."
