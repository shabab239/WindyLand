"""Used by the installer: writes the settings the user typed in the setup wizard.

Values come from environment variables (so the API key never appears on a command line):
WL_PROVIDER, WL_MODEL, WL_API_KEY, WL_BASE_URL.   `--test` checks that the model answers.
"""
from __future__ import annotations

import os
import sys

from . import config
from .llm import LLM


def main(argv=None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    if "--test" in args:
        cfg = config.load()
        if not config.is_configured(cfg):
            print("No API key entered; set it later in the agent window (gear icon).")
            return 0
        try:
            out = LLM(cfg).ask("Reply with the single word: OK", "ping", max_tokens=20)
            print("Connection to the AI model works. Reply:", out.strip()[:40])
        except Exception as e:
            print("Could not reach the AI model:", e)
            print("You can fix the key or model later in the agent window (gear icon).")
        return 0
    ch: dict = {"setup_done": True}
    prov = os.environ.get("WL_PROVIDER", "").strip()
    if prov in config.PROVIDERS:
        ch["provider"] = prov
    for env, key in (("WL_MODEL", "model"), ("WL_BASE_URL", "base_url"), ("WL_API_KEY", "api_key")):
        v = os.environ.get(env, "").strip()
        if v:
            ch[key] = v
    config.update(ch)
    print("Settings saved to", config.data_dir())
    return 0


if __name__ == "__main__":
    sys.exit(main())
