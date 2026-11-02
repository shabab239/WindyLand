"""The stop button: every wait in the humanizer sleeps through here, so a stop is noticed within a fraction of a second
(also while waiting for stealthwriter, claude.ai or academi.cx), not only when the next line is logged."""
from __future__ import annotations

import threading
import time

_event: threading.Event | None = None


class Stopped(Exception):
    """Raised when the user pressed the stop button (the agent's own Cancelled is used when it is importable)."""


def set_event(ev: threading.Event | None) -> None:
    global _event
    _event = ev


def check() -> None:
    if _event is not None and _event.is_set():
        try:
            from ..llm import Cancelled
        except Exception:  # pragma: no cover
            raise Stopped()
        raise Cancelled()


def sleep(seconds: float) -> None:
    end = time.time() + max(0.0, seconds)
    while True:
        check()
        left = end - time.time()
        if left <= 0:
            return
        time.sleep(min(0.2, left))
