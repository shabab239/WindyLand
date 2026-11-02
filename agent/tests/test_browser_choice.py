"""Which browser Tobi drives: your choice, else your default, else the first Chromium-based one found."""
import os, sys, tempfile
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from windyagent.humanizer import stealth_client as sc


def main():
    root = Path(tempfile.mkdtemp())
    pf = root / "pf"
    for rel in (r"Google\Chrome\Application\chrome.exe", r"BraveSoftware\Brave-Browser\Application\brave.exe"):
        # on Linux the backslash path is one file name; build the Windows layout with real folders
        f = pf.joinpath(*rel.split("\\")); f.parent.mkdir(parents=True, exist_ok=True); f.write_text("x")
    # make the module's joining work on this OS
    real_installed = sc.installed_browsers
    def fake():
        out = {}
        for key, (_l, rels, _n, _p) in sc.BROWSERS.items():
            for rel in rels:
                f = pf.joinpath(*rel.split("\\"))
                if f.exists(): out[key] = str(f); break
        return out
    sc.installed_browsers = fake
    sc.default_browser_key = lambda: None
    assert sc.browser_key(sc.find_browser("", "brave")) == "brave"
    assert sc.browser_key(sc.find_browser("", "chrome")) == "chrome"
    assert sc.browser_key(sc.find_browser("", "auto")) == "chrome"                 # no default known: Chrome before Brave
    sc.default_browser_key = lambda: "brave"
    assert sc.browser_key(sc.find_browser("", "auto")) == "brave"                  # your default browser wins
    assert sc.browser_key(sc.find_browser("", "edge")) == "brave"                  # chosen one missing: falls back to the default
    mine = pf / "mine.exe"; mine.write_text("x")
    assert sc.find_browser(str(mine), "chrome") == str(mine)                       # an exact path always wins
    sc.installed_browsers = lambda: {}
    sc.default_browser_key = lambda: None
    try:
        sc.find_browser("", "auto"); raise AssertionError("should fail")
    except sc.SiteError as e:
        assert "Chromium-based" in str(e)
    print("browser choice OK (Brave / Chrome / Edge / Vivaldi / Opera, default browser, exact path)")


if __name__ == "__main__":
    main()
