"""Opening programs by name: matching against Start Menu shortcuts, aliases and the router."""
import os, sys, tempfile
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
D = Path(tempfile.mkdtemp())
(D / "Programs" / "Spotify").mkdir(parents=True)
for n in ("Spotify", "Visual Studio Code", "Adobe Photoshop 2025", "Microsoft Word", "Uninstall Spotify", "Steam", "Brave", "Google Chrome", "Chrome Remote Desktop"):
    (D / "Programs" / f"{n}.lnk").write_text("x")
os.environ["WINDYLAND_APP_DIRS"] = str(D / "Programs")
from windyagent import apps, router, tools

launched = []
apps._launch = lambda a: launched.append(a["name"])
apps.IS_WIN = False
apps._scan_store_apps = lambda: []

def test_matching():
    ix = apps.index(force=True, store=False)
    names = {a["name"] for a in ix}
    assert "Uninstall Spotify" not in names and "Spotify" in names
    top = lambda q: apps.match(q, ix)[0][1]["name"]
    assert top("spotify") == "Spotify" and top("photoshop") == "Adobe Photoshop 2025" and top("word") == "Microsoft Word"
    assert top("visual studio") == "Visual Studio Code" and top("steem") == "Steam"
    assert apps.match("zzzzqq", ix) == []

def test_open():
    launched.clear()
    assert apps.open_app("Spotify") == "Opening Spotify." and launched == ["Spotify"]
    assert apps.open_app("the photoshop app").startswith("Opening Adobe Photoshop") and launched[-1] == "Adobe Photoshop 2025"
    assert apps.open_app("chrome") == "Opening Google Chrome." or "more than one" in apps.open_app("chrome")
    assert "could not find" in apps.open_app("qwertyuiop")
    assert "Which program" in apps.open_app("  ")

def test_router():
    r = lambda t: router.route(t)
    assert r("open spotify") == [{"tool": "open_app", "args": {"name": "spotify"}}]
    assert r("Please launch the Steam app")[0]["tool"] == "open_app"
    assert r("start steam")[0] == {"tool": "open_app", "args": {"name": "steam"}}
    assert r("start notepad")[0]["tool"] == "open_app"                       # alias; used to be treated as "play notepad"
    assert r("open https://example.com")[0]["tool"] == "open_target"
    assert r("open C:\\Users\\me\\a.docx")[0]["tool"] == "open_target"
    assert r("open my report.docx")[0]["tool"] == "open_target"
    assert r("start the music") is None or r("start the music")[0]["tool"] != "open_app"
    assert r("open the downloads folder") is None or r("open the downloads folder")[0]["tool"] == "open_target"
    assert r("play naruto")[0]["tool"] == "play_anime" or r("play naruto")[0]["tool"] == "play"
    assert "open_app" in tools.select_names("hello")

if __name__ == "__main__":
    test_matching(); test_open(); test_router(); print("APP TESTS PASSED")
