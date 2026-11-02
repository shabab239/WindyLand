"""Everyday helpers for a home PC: weather, memory, reminders, power, tidying the Downloads folder."""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import time
import urllib.parse
import urllib.request
from datetime import datetime
from pathlib import Path

from . import memory
from .config import IS_WIN

UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) WindyLand-Tobi"
NO_WINDOW = 0x08000000 if IS_WIN else 0


def _json(url: str, timeout: int = 15):
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read(500_000).decode("utf-8", "replace"))


# ------------------------------------------------------------------ weather
WMO = {0: "clear sky", 1: "mostly clear", 2: "partly cloudy", 3: "overcast", 45: "fog", 48: "freezing fog",
       51: "light drizzle", 53: "drizzle", 55: "heavy drizzle", 56: "freezing drizzle", 57: "freezing drizzle",
       61: "light rain", 63: "rain", 65: "heavy rain", 66: "freezing rain", 67: "freezing rain",
       71: "light snow", 73: "snow", 75: "heavy snow", 77: "snow grains", 80: "rain showers", 81: "rain showers",
       82: "violent rain showers", 85: "snow showers", 86: "snow showers", 95: "thunderstorm", 96: "thunderstorm with hail",
       99: "thunderstorm with hail"}


def weather(ctx, city: str) -> str:
    city = city.strip()
    if not city:
        return "Which city?"
    try:
        g = _json("https://geocoding-api.open-meteo.com/v1/search?count=1&name=" + urllib.parse.quote(city))
        place = (g.get("results") or [None])[0]
        if not place:
            return f"I could not find a place called {city}."
        w = _json("https://api.open-meteo.com/v1/forecast?latitude=%s&longitude=%s&timezone=auto&forecast_days=3"
                  "&current=temperature_2m,apparent_temperature,relative_humidity_2m,wind_speed_10m,weather_code"
                  "&daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max"
                  % (place["latitude"], place["longitude"]))
        c, d = w["current"], w["daily"]
        name = ", ".join(x for x in (place.get("name"), place.get("admin1"), place.get("country")) if x)
        lines = [f"{name}: {c['temperature_2m']}°C (feels {c['apparent_temperature']}°C), {WMO.get(c['weather_code'], 'unknown')}, "
                 f"humidity {c['relative_humidity_2m']}%, wind {c['wind_speed_10m']} km/h."]
        for i, day in enumerate(d["time"]):
            lines.append(f"{day}: {d['temperature_2m_min'][i]}–{d['temperature_2m_max'][i]}°C, {WMO.get(d['weather_code'][i], '?')}, "
                         f"rain chance {d['precipitation_probability_max'][i]}%")
        return "\n".join(lines)
    except Exception as e:
        try:  # fallback
            req = urllib.request.Request("https://wttr.in/" + urllib.parse.quote(city) + "?format=3", headers={"User-Agent": "curl/8"})
            with urllib.request.urlopen(req, timeout=15) as r:
                return r.read(2000).decode("utf-8", "replace").strip()
        except Exception:
            return f"Could not get the weather ({e.__class__.__name__}). Is the internet on?"


# ------------------------------------------------------------------ memory
def remember(ctx, text: str, kind: str = "fact") -> str:
    n = memory.remember(text, kind)
    return f"Saved (#{n['id']}, {n['kind']}): {n['text']}"


def recall(ctx, query: str = "") -> str:
    found = memory.recall(query)
    if not found:
        return "Nothing saved" + (f" about \"{query}\"." if query else " yet.")
    return "\n".join(f"#{n['id']} ({n['kind']}) {n['text']}" for n in found[-40:])


def forget(ctx, what: str) -> str:
    gone = memory.forget(what)
    return ("Forgot: " + "; ".join(n["text"] for n in gone)) if gone else "I had nothing matching that."


def humanize_history(ctx, limit: int = 8) -> str:
    items = memory.humanize_history(max(1, min(int(limit), 30)))
    if not items:
        return "No humanizing jobs yet."
    out = []
    for h in items:
        when = time.strftime("%d %b %Y %H:%M", time.localtime(h["ts"]))
        state = "finished" if h.get("complete") else f"unfinished, {h.get('paragraphs_done', 0)}/{h.get('paragraphs_total', '?')} paragraphs"
        err = f" Problem: {h['error']}" if h.get("error") else ""
        out.append(f"{when} | {h.get('file', '?')} | {state} | {h.get('sentences_changed', 0)} sentences changed | output: {h.get('output', '-')}{err}")
    return "\n".join(out)


# ------------------------------------------------------------------ reminders
def _parse_when(at: str) -> float:
    at = at.strip().replace("T", " ")
    for fmt in ("%Y-%m-%d %H:%M", "%Y-%m-%d %H:%M:%S", "%Y-%m-%d"):
        try:
            return datetime.strptime(at, fmt).timestamp()
        except ValueError:
            continue
    raise ValueError("Use the format YYYY-MM-DD HH:MM (local time)")


def set_reminder(ctx, text: str, in_minutes: float = 0, at: str = "") -> str:
    if in_minutes and float(in_minutes) > 0:
        due = time.time() + float(in_minutes) * 60
    elif at:
        due = _parse_when(at)
    else:
        return "Say when: in_minutes (a number) or at (YYYY-MM-DD HH:MM)."
    if due < time.time() - 30:
        return "That time is already in the past."
    r = memory.add_reminder(text, due)
    when = time.strftime("%A %d %b, %H:%M", time.localtime(due))
    return f"Reminder #{r['id']} set for {when}: {r['text']}. It will pop out of the notch even if the chat is closed."


def list_reminders(ctx) -> str:
    items = memory.reminders()
    if not items:
        return "No reminders waiting."
    return "\n".join(f"#{r['id']} {time.strftime('%a %d %b %H:%M', time.localtime(r['due']))}: {r['text']}" for r in items)


def cancel_reminder(ctx, id: int) -> str:
    return "Cancelled." if memory.cancel_reminder(id) else "No reminder with that number."


def fire_due(hub=None) -> list[dict]:
    """Called a few times a minute by the server: pops due reminders out of the notch and the chat."""
    due = memory.due_reminders()
    for r in due:
        memory.notify("Reminder", r["text"])
        if hub:
            hub.emit("reminder", text=r["text"])
    return due


# ------------------------------------------------------------------ power
def power(ctx, action: str, delay_minutes: int = 0) -> str:
    action = action.lower().strip()
    if action not in ("lock", "sleep", "shutdown", "restart", "cancel"):
        return "action must be lock, sleep, shutdown, restart or cancel."
    if not IS_WIN:
        return "Power actions are only available on Windows."
    secs = max(0, min(int(delay_minutes), 600)) * 60
    if action == "cancel":
        r = subprocess.run(["shutdown", "/a"], capture_output=True, text=True, creationflags=NO_WINDOW)
        return "Cancelled the pending shutdown." if r.returncode == 0 else "There was no shutdown waiting."
    if action != "lock":
        what = {"sleep": "put the PC to sleep", "shutdown": "shut the PC down", "restart": "restart the PC"}[action]
        when = f" in {secs // 60} minute(s)" if secs else " now"
        if not ctx.confirm("Power", f"Tobi wants to {what}{when}. Unsaved work in other programs can be lost."):
            from .tools import ToolDenied

            raise ToolDenied()
    if action == "lock":
        subprocess.Popen(["rundll32.exe", "user32.dll,LockWorkStation"], creationflags=NO_WINDOW)
        return "Locked."
    if action == "sleep":
        subprocess.Popen(["rundll32.exe", "powrprof.dll,SetSuspendState", "0,1,0"], creationflags=NO_WINDOW)
        return "Going to sleep."
    flag = "/s" if action == "shutdown" else "/r"
    subprocess.run(["shutdown", flag, "/t", str(secs)], creationflags=NO_WINDOW)
    return f"{action.capitalize()} scheduled" + (f" in {secs // 60} minute(s); say 'cancel the shutdown' to stop it." if secs else ".")


# ------------------------------------------------------------------ tidy downloads
KINDS = {
    "Pictures": {".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp", ".heic", ".svg"},
    "Videos": {".mp4", ".mkv", ".avi", ".mov", ".wmv", ".webm"},
    "Music": {".mp3", ".wav", ".flac", ".m4a", ".aac", ".ogg"},
    "Documents": {".pdf", ".doc", ".docx", ".txt", ".xls", ".xlsx", ".ppt", ".pptx", ".csv", ".odt", ".rtf", ".epub"},
    "Archives": {".zip", ".rar", ".7z", ".tar", ".gz"},
    "Programs": {".exe", ".msi", ".iso", ".apk"},
}


def organize_folder(ctx, folder: str = "~/Downloads", apply: bool = False) -> str:
    root = Path(os.path.expandvars(os.path.expanduser(folder))).resolve()
    if not root.is_dir():
        return f"Folder not found: {root}"
    home = Path.home().resolve()
    if root == home or root.parent == root:
        return "I only tidy one specific folder like Downloads, not the whole home folder or a drive."
    plan: list[tuple[Path, Path]] = []
    for f in root.iterdir():
        if not f.is_file() or f.name.startswith(".") or f.suffix.lower() in (".crdownload", ".part", ".tmp", ".download"):
            continue
        if time.time() - f.stat().st_mtime < 300:  # maybe still downloading
            continue
        for kind, exts in KINDS.items():
            if f.suffix.lower() in exts:
                plan.append((f, root / kind / f.name))
                break
        else:
            plan.append((f, root / "Other" / f.name))
    if not plan:
        return f"{root} is already tidy (nothing to move)."
    counts: dict[str, int] = {}
    for _, dst in plan:
        counts[dst.parent.name] = counts.get(dst.parent.name, 0) + 1
    summary = ", ".join(f"{n} to {k}" for k, n in sorted(counts.items()))
    if not apply:
        return f"I would move {len(plan)} files in {root}: {summary}. Nothing was moved yet; ask again with apply=true after the user agrees."
    if not ctx.confirm("Tidy folder", f"Move {len(plan)} files in {root} into sub-folders ({summary}). Nothing is deleted."):
        from .tools import ToolDenied

        raise ToolDenied()
    moved = 0
    for src, dst in plan:
        dst.parent.mkdir(exist_ok=True)
        target = dst
        i = 1
        while target.exists():
            target = dst.with_name(f"{dst.stem} ({i}){dst.suffix}")
            i += 1
        try:
            shutil.move(str(src), str(target))
            moved += 1
        except OSError:
            pass
    return f"Moved {moved} of {len(plan)} files into sub-folders of {root} ({summary}). Nothing was deleted."
