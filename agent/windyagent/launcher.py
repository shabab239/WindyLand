"""Started by the WindyLand button:
    pythonw -m windyagent.launcher [--settings | --stop | --no-window] [--rect l,t,r,b] [--attach FILE...]

Makes sure Tobi's server is running, then opens the chat as a small popup window under the notch
(Microsoft Edge, Chrome or Brave in --app mode; Edge ships with Windows). If the popup is already open it is
brought to the front instead. --rect is the notch's screen rectangle; --attach hands files to the chat."""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

from . import config

NO_WINDOW = 0x08000000
DETACHED = 0x00000008
NEW_GROUP = 0x00000200


def _port() -> int:
    try:
        return int((config.data_dir() / "port.txt").read_text().strip())
    except Exception:
        return int(config.load().get("port", config.DEFAULT_PORT))


def server_alive(port: int | None = None) -> bool:
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port or _port()}/api/ping", timeout=1.5) as r:
            return json.loads(r.read()).get("app") == "windyland-agent"
    except Exception:
        return False


def start_server() -> bool:
    if server_alive():
        return True
    py = Path(sys.executable)
    if py.name.lower() == "python.exe" and (py.parent / "pythonw.exe").exists():
        py = py.parent / "pythonw.exe"
    kwargs: dict = {"cwd": str(Path(__file__).resolve().parent.parent), "stdin": subprocess.DEVNULL,
                    "stdout": subprocess.DEVNULL, "stderr": subprocess.DEVNULL}
    if config.IS_WIN:
        kwargs["creationflags"] = NO_WINDOW | DETACHED | NEW_GROUP
    else:
        kwargs["start_new_session"] = True
    subprocess.Popen([str(py), "-m", "windyagent.server"], **kwargs)
    for _ in range(80):
        time.sleep(0.25)
        if server_alive():
            return True
    return False


def find_app_browser() -> str | None:
    if not config.IS_WIN:
        for n in ("microsoft-edge", "google-chrome", "chromium", "chromium-browser", "brave-browser"):
            if shutil.which(n):
                return shutil.which(n)
        return None
    roots = [os.environ.get(k, "") for k in ("ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA")]
    rel = [r"Microsoft\Edge\Application\msedge.exe", r"Google\Chrome\Application\chrome.exe",
           r"BraveSoftware\Brave-Browser\Application\brave.exe"]
    for r in rel:
        for root in roots:
            p = Path(root) / r if root else None
            if p and p.exists():
                return str(p)
    return None


WIN_W, WIN_H = 400, 640   # logical pixels of the popup
TITLE = "Tobi"


def _win_api():
    """ctypes handles for the few Win32 calls used to find and place the popup (None off Windows)."""
    if not config.IS_WIN:
        return None
    import ctypes
    from ctypes import wintypes

    try:
        ctypes.windll.shcore.SetProcessDpiAwareness(2)
    except Exception:
        try:
            ctypes.windll.user32.SetProcessDPIAware()
        except Exception:
            pass
    return ctypes, wintypes, ctypes.windll.user32


def find_popup() -> int:
    """Window handle of Tobi's open popup (an app window whose title is exactly 'Tobi'), or 0."""
    api = _win_api()
    if not api:
        return 0
    ctypes, wintypes, user32 = api
    found = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def cb(hwnd, _):
        if user32.IsWindowVisible(hwnd):
            buf = ctypes.create_unicode_buffer(64)
            user32.GetWindowTextW(hwnd, buf, 64)
            if buf.value == TITLE:
                cls = ctypes.create_unicode_buffer(64)
                user32.GetClassNameW(hwnd, cls, 64)
                if cls.value.startswith("Chrome_WidgetWin"):
                    found.append(hwnd)
        return True

    user32.EnumWindows(cb, 0)
    return found[0] if found else 0


def popup_geometry(rect: tuple[int, int, int, int] | None) -> tuple[int, int, int, int]:
    """(x, y, w, h) in physical pixels: centred under the notch and kept on the screen it is on."""
    api = _win_api()
    if not api:
        return (100, 80, WIN_W, WIN_H)
    ctypes, wintypes, user32 = api

    class MONITORINFO(ctypes.Structure):
        _fields_ = [("cbSize", wintypes.DWORD), ("rcMonitor", wintypes.RECT), ("rcWork", wintypes.RECT), ("dwFlags", wintypes.DWORD)]

    if rect:
        cx, top, bottom = (rect[0] + rect[2]) // 2, rect[1], rect[3]
    else:
        cx, top, bottom = user32.GetSystemMetrics(0) // 2, 0, 0
    mon = user32.MonitorFromPoint(wintypes.POINT(cx, top), 2)  # nearest
    mi = MONITORINFO()
    mi.cbSize = ctypes.sizeof(mi)
    user32.GetMonitorInfoW(mon, ctypes.byref(mi))
    try:
        dx, dy = ctypes.c_uint(), ctypes.c_uint()
        ctypes.windll.shcore.GetDpiForMonitor(mon, 0, ctypes.byref(dx), ctypes.byref(dy))
        scale = (dx.value or 96) / 96.0
    except Exception:
        scale = 1.0
    w, h = int(WIN_W * scale), int(WIN_H * scale)
    wa = mi.rcWork
    h = min(h, wa.bottom - wa.top - 16)
    w = min(w, wa.right - wa.left - 16)
    x = min(max(cx - w // 2, wa.left + 8), wa.right - w - 8)
    y = max(bottom + int(8 * scale), wa.top + 8)
    if y + h > wa.bottom - 8:       # not enough room below the notch: slide up
        y = max(wa.top + 8, wa.bottom - h - 8)
    return x, y, w, h


def place_and_focus(hwnd: int, rect) -> None:
    api = _win_api()
    if not api or not hwnd:
        return
    ctypes, wintypes, user32 = api
    x, y, w, h = popup_geometry(rect)
    user32.ShowWindow(hwnd, 9)  # SW_RESTORE
    user32.SetWindowPos(hwnd, wintypes.HWND(-1), x, y, w, h, 0x0040)  # topmost + show
    user32.SetWindowPos(hwnd, wintypes.HWND(-2), x, y, w, h, 0x0040)  # then back to normal, keeps the position
    user32.SetForegroundWindow(hwnd)


def open_window(url: str, rect=None) -> None:
    exe = find_app_browser()
    if exe:
        profile = config.data_dir() / "ui_profile"
        x, y, w, h = popup_geometry(rect)
        subprocess.Popen(
            [exe, f"--app={url}", f"--window-size={w},{h}", f"--window-position={x},{y}", f"--user-data-dir={profile}",
             "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--disable-features=Translate"],
        )
        if config.IS_WIN:  # Chrome rounds / re-scales the flags above on mixed-DPI screens: place it exactly
            for _ in range(40):
                time.sleep(0.25)
                hwnd = find_popup()
                if hwnd:
                    place_and_focus(hwnd, rect)
                    break
    else:
        import webbrowser

        webbrowser.open(url)


def stop_server() -> None:
    cfg = config.load()
    port = _port()
    if not server_alive(port):
        return
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}/api/quit", data=b"{}", method="POST",
        headers={"Content-Type": "application/json", "Cookie": f"wl_agent={cfg['token']}"},
    )
    try:
        urllib.request.urlopen(req, timeout=5).read()
    except Exception:
        pass


def _post(path: str, body: dict) -> dict:
    cfg = config.load()
    req = urllib.request.Request(
        f"http://127.0.0.1:{_port()}{path}", data=json.dumps(body).encode(), method="POST",
        headers={"Content-Type": "application/json", "Cookie": f"wl_agent={cfg['token']}"})
    try:
        return json.loads(urllib.request.urlopen(req, timeout=8).read())
    except Exception:
        return {}


def _arg_list(args: list[str], flag: str) -> list[str]:
    """Values after `flag` up to the next --option."""
    if flag not in args:
        return []
    out = []
    for a in args[args.index(flag) + 1:]:
        if a.startswith("--"):
            break
        out.append(a)
    return out


def main(argv=None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    if "--stop" in args:
        stop_server()
        return 0
    if "--knowledge" in args:   # open the knowledge folder in File Explorer (it is created when it does not exist yet)
        from . import knowledge

        p = knowledge.folder()
        if os.name == "nt":
            os.startfile(str(p))  # type: ignore[attr-defined]
        return 0
    if "--knowledge-init" in args:   # used by the installer: just create the folder
        from . import knowledge

        print(knowledge.folder())
        return 0
    if not start_server():
        return 1
    if "--no-window" in args:
        return 0
    rect = None
    r = _arg_list(args, "--rect")
    if r:
        try:
            rect = tuple(int(v) for v in r[0].split(","))[:4]
            rect = rect if len(rect) == 4 else None
        except ValueError:
            rect = None
    files = _arg_list(args, "--attach")
    hwnd = find_popup()
    if files:
        _post("/api/attach", {"paths": files, "live": bool(hwnd)})
    if hwnd:   # already open: bring it forward (and show the settings if asked)
        if "--settings" in args:
            _post("/api/ui", {"action": "settings"})
        place_and_focus(hwnd, rect)
        return 0
    cfg = config.load()
    url = f"http://127.0.0.1:{_port()}/?t={cfg['token']}" + ("&settings=1" if "--settings" in args else "")
    open_window(url, rect)
    return 0


if __name__ == "__main__":
    sys.exit(main())
