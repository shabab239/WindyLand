"""Settings and paths of Tobi, the WindyLand AI Agent.

Everything lives in  %APPDATA%\\WindyLand\\agent\\  (override with WINDYLAND_AGENT_HOME):
    config.json    provider, model, API key, access token, options
    port.txt       the port the local server listens on (WindyLand reads it)
    history.json   the current conversation
    agent.log      log file (the agent runs without a console window)
"""
from __future__ import annotations

import json
import os
import secrets
import subprocess
import threading
from pathlib import Path

IS_WIN = os.name == "nt"
DEFAULT_PORT = 47615

# provider id -> label, API style, default base URL, default model, needs a key?
PROVIDERS: dict[str, dict] = {
    "openai": {
        "label": "OpenAI (GPT)",
        "style": "openai",
        "base_url": "https://api.openai.com/v1",
        "model": "gpt-4.1",
        "needs_key": True,
        "key_help": "platform.openai.com > API keys",
    },
    "anthropic": {
        "label": "Anthropic (Claude)",
        "style": "anthropic",
        "base_url": "https://api.anthropic.com",
        "model": "claude-sonnet-5-5",
        "needs_key": True,
        "key_help": "console.anthropic.com > API keys",
    },
    "gemini": {
        "label": "Google Gemini",
        "style": "openai",
        "base_url": "https://generativelanguage.googleapis.com/v1beta/openai",
        "model": "gemini-2.5-flash",
        "needs_key": True,
        "key_help": "aistudio.google.com > Get API key",
    },
    "openrouter": {
        "label": "OpenRouter (many models)",
        "style": "openai",
        "base_url": "https://openrouter.ai/api/v1",
        "model": "anthropic/claude-sonnet-4.5",
        "needs_key": True,
        "key_help": "openrouter.ai > Keys",
    },
    "ollama": {
        "label": "Ollama (local, no key)",
        "style": "ollama",
        "base_url": "http://localhost:11434",
        "model": "llama3.1",
        "needs_key": False,
        "key_help": "no key needed; install Ollama first",
    },
    "custom": {
        "label": "Custom (OpenAI-compatible)",
        "style": "openai",
        "base_url": "",
        "model": "",
        "needs_key": False,
        "key_help": "any server that speaks the OpenAI chat API",
    },
}

DEFAULTS: dict = {
    "provider": "openai",
    "model": "",
    "api_key": "",
    "base_url": "",
    "port": DEFAULT_PORT,
    "token": "",
    "confirm_mode": "ask",  # ask = approve risky actions in the chat | auto = never ask
    "max_steps": 15,
    "setup_done": False,
    "anime_site": "https://animesalt.cx",
    "num_ctx": 8192,          # Ollama: how much text the local model reads at once (its own default of 4096 is too small)
    "knowledge_folder": "",           # empty = Documents\\Tobi\\Knowledge
    "humanize_claude_check": True,   # after humanizing: Claude reads the whole document in one chat and fine-tunes the worst sentences
    "humanize_academi": True,        # after humanizing: upload to academi.cx and download its AI score report
    "review_engine": "auto",          # who reviews the finished document: auto (Claude, then your AI provider e.g. ChatGPT) | tobi (your AI provider only: fastest)
    "humanizer_browser": "auto",      # browser Tobi drives: auto (your default) | brave | chrome | edge | vivaldi | opera | chromium
    "humanizer_browser_path": "",     # or the full path of a Chromium-based browser's .exe
    "anime_autoplay": True,   # open the episode in an automation-controlled browser window and press play
}

_lock = threading.RLock()


def data_dir() -> Path:
    env = os.environ.get("WINDYLAND_AGENT_HOME")
    if env:
        p = Path(env)
    elif IS_WIN:
        p = Path(os.environ.get("APPDATA") or (Path.home() / "AppData" / "Roaming")) / "WindyLand" / "agent"
    else:
        p = Path.home() / ".windyland" / "agent"
    p.mkdir(parents=True, exist_ok=True)
    return p


def _lock_down(path: Path) -> None:
    """Only the current user may read the file (it holds the API key)."""
    try:
        if IS_WIN:
            user = os.environ.get("USERNAME", "")
            if user:
                subprocess.run(
                    ["icacls", str(path), "/inheritance:r", "/grant:r", f"{user}:F"],
                    capture_output=True, timeout=15, creationflags=0x08000000,
                )
        else:
            os.chmod(path, 0o600)
    except Exception:
        pass


def load() -> dict:
    with _lock:
        cfg = dict(DEFAULTS)
        f = data_dir() / "config.json"
        if f.exists():
            try:
                cfg.update(json.loads(f.read_text(encoding="utf-8")))
            except Exception:
                pass
        if not cfg.get("token"):
            cfg["token"] = secrets.token_urlsafe(24)
            save(cfg)
        return cfg


def save(cfg: dict) -> None:
    with _lock:
        f = data_dir() / "config.json"
        tmp = f.with_suffix(".tmp")
        tmp.write_text(json.dumps(cfg, indent=2), encoding="utf-8")
        _lock_down(tmp)
        os.replace(tmp, f)


def update(changes: dict) -> dict:
    with _lock:
        cfg = load()
        cfg.update(changes)
        save(cfg)
        return cfg


def resolved(cfg: dict) -> dict:
    """The provider settings with defaults filled in: style, base_url, model."""
    p = PROVIDERS.get(cfg.get("provider", "openai"), PROVIDERS["custom"])
    base = (cfg.get("base_url") or p["base_url"]).rstrip("/")
    if p["style"] == "ollama" and base.endswith("/v1"):   # Ollama's own API has no /v1
        base = base[:-3]
    return {
        "provider": cfg.get("provider", "openai"),
        "style": p["style"],
        "base_url": base,
        "model": cfg.get("model") or p["model"],
        "api_key": cfg.get("api_key", ""),
        "needs_key": p["needs_key"],
    }


def is_local(cfg: dict) -> bool:
    """A small local model (Ollama): gets a shorter prompt, fewer tools and the fast judge."""
    return resolved(cfg)["style"] == "ollama"


def is_configured(cfg: dict) -> bool:
    r = resolved(cfg)
    if not r["base_url"] or not r["model"]:
        return False
    return bool(r["api_key"]) or not r["needs_key"]


def _browser_choices() -> dict:
    try:
        from .humanizer.stealth_client import BROWSERS, default_browser_key, installed_browsers

        inst = installed_browsers()
        d = default_browser_key()
        return {"default": (BROWSERS[d][0] if d else ""), "installed": {k: BROWSERS[k][0] for k in inst}}
    except Exception:
        return {"default": "", "installed": {}}


def public(cfg: dict) -> dict:
    """Settings that are safe to send to the chat window (never the key or token)."""
    r = resolved(cfg)
    return {
        "provider": r["provider"],
        "model": cfg.get("model", ""),
        "base_url": cfg.get("base_url", ""),
        "has_key": bool(cfg.get("api_key")),
        "confirm_mode": cfg.get("confirm_mode", "ask"),
        "anime_site": cfg.get("anime_site", DEFAULTS["anime_site"]),
        "browser": cfg.get("humanizer_browser", "auto"),
        "review_engine": cfg.get("review_engine", "auto"),
        "browsers": _browser_choices(),
        "configured": is_configured(cfg),
        "effective_model": r["model"],
        "providers": {
            k: {x: v[x] for x in ("label", "base_url", "model", "needs_key", "key_help")} for k, v in PROVIDERS.items()
        },
    }
