"""Tobi's long-term memory and logbooks, all small JSON files in the agent's data folder.

  memory.json            notes about you (likes, preferences, facts) that Tobi reads at the start of every chat
  humanize_history.json  every humanizing job: which file, when, how far it got
  played.json            videos already played, so "play something" does not repeat itself
  reminders.json         reminders waiting to fire (their due time is a unix timestamp)
  notify.txt             messages for the notch (WindyLand shows each line as a pop-out)
"""
from __future__ import annotations

import json
import os
import threading
import time

from . import config

_lock = threading.RLock()


def _file(name: str):
    return config.data_dir() / name


def _read(name: str, default):
    try:
        return json.loads(_file(name).read_text(encoding="utf-8"))
    except Exception:
        return default


def _write(name: str, data) -> None:
    tmp = _file(name).with_suffix(".tmp")
    tmp.write_text(json.dumps(data, ensure_ascii=False, indent=1), encoding="utf-8")
    os.replace(tmp, _file(name))


# ------------------------------------------------------------------ notes
KINDS = ("interest", "preference", "fact")


def notes() -> list[dict]:
    with _lock:
        return _read("memory.json", {"notes": []}).get("notes", [])


def remember(text: str, kind: str = "fact") -> dict:
    text = " ".join(str(text).split())[:300]
    kind = kind if kind in KINDS else "fact"
    with _lock:
        data = _read("memory.json", {"notes": [], "next": 1})
        for n in data["notes"]:  # no duplicates
            if n["text"].lower() == text.lower():
                return n
        note = {"id": data.get("next", 1), "kind": kind, "text": text, "ts": time.time()}
        data["notes"].append(note)
        data["next"] = note["id"] + 1
        data["notes"] = data["notes"][-200:]
        _write("memory.json", data)
        return note


def forget(key: str) -> list[dict]:
    """Remove notes by id or by a phrase they contain. Returns what was removed."""
    key = str(key).strip().lower()
    with _lock:
        data = _read("memory.json", {"notes": [], "next": 1})
        gone = [n for n in data["notes"] if str(n["id"]) == key or (len(key) >= 3 and key in n["text"].lower())]
        if gone:
            data["notes"] = [n for n in data["notes"] if n not in gone]
            _write("memory.json", data)
        return gone


def recall(query: str = "") -> list[dict]:
    q = query.strip().lower()
    return [n for n in notes() if not q or q in n["text"].lower() or q == n["kind"]]


def interests() -> list[str]:
    return [n["text"] for n in notes() if n["kind"] == "interest"]


def prompt_block(limit_chars: int = 2500) -> str:
    """What goes into the system prompt."""
    lines, used = [], 0
    for n in reversed(notes()):
        line = f"- ({n['kind']}) {n['text']}"
        if used + len(line) > limit_chars:
            break
        lines.append(line)
        used += len(line)
    return "\n".join(reversed(lines)) or "(nothing saved yet)"


# ------------------------------------------------------------------ humanize logbook
def log_humanize(entry: dict) -> None:
    with _lock:
        hist = _read("humanize_history.json", [])
        hist.append({"ts": time.time(), **entry})
        _write("humanize_history.json", hist[-100:])


def humanize_history(limit: int = 10) -> list[dict]:
    with _lock:
        return _read("humanize_history.json", [])[-limit:]


def humanize_prompt_block() -> str:
    out = []
    for h in humanize_history(4):
        when = time.strftime("%d %b %H:%M", time.localtime(h["ts"]))
        state = "finished" if h.get("complete") else f"unfinished ({h.get('paragraphs_done', 0)}/{h.get('paragraphs_total', '?')} paragraphs)"
        out.append(f"- {when}: {os.path.basename(h.get('file', '?'))}, {state}, {h.get('sentences_changed', 0)} sentences changed")
    return "\n".join(out) or "(none yet)"


# ------------------------------------------------------------------ played videos
def played() -> list[str]:
    with _lock:
        return _read("played.json", [])


def mark_played(video_id: str) -> None:
    with _lock:
        p = [v for v in _read("played.json", []) if v != video_id] + [video_id]
        _write("played.json", p[-80:])


# ------------------------------------------------------------------ reminders
def reminders() -> list[dict]:
    with _lock:
        return sorted(_read("reminders.json", []), key=lambda r: r["due"])


def add_reminder(text: str, due: float) -> dict:
    with _lock:
        items = _read("reminders.json", [])
        rid = max([r["id"] for r in items] + [0]) + 1
        r = {"id": rid, "due": due, "text": " ".join(text.split())[:200]}
        items.append(r)
        _write("reminders.json", items)
        return r


def cancel_reminder(rid: int) -> bool:
    with _lock:
        items = _read("reminders.json", [])
        keep = [r for r in items if r["id"] != int(rid)]
        if len(keep) != len(items):
            _write("reminders.json", keep)
            return True
        return False


def due_reminders(now: float | None = None) -> list[dict]:
    """Remove and return the reminders whose time has come."""
    now = now or time.time()
    with _lock:
        items = _read("reminders.json", [])
        due = [r for r in items if r["due"] <= now]
        if due:
            _write("reminders.json", [r for r in items if r["due"] > now])
        return due


# ------------------------------------------------------------------ the notch
def notify(title: str, message: str) -> None:
    """Show a pop-out in the notch (WindyLand reads this file once a second)."""
    clean = lambda s: " ".join(str(s).replace("\t", " ").split())[:160]
    with _lock:
        with open(_file("notify.txt"), "a", encoding="utf-8") as f:
            f.write(f"{clean(title)}\t{clean(message)}\n")
