"""Browser choice: auto (default browser, else the first installed), a named browser, a missing one."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from windyagent.humanizer import stealth_client as sc

fake = {"chrome": "C:/c/chrome.exe", "brave": "C:/b/brave.exe", "edge": "C:/e/msedge.exe"}
sc.installed_browsers = lambda: dict(fake)

sc.default_browser_key = lambda: "brave"
assert sc.find_browser() == fake["brave"]                      # auto: the PC's default browser
assert sc.find_browser("", "chrome") == fake["chrome"]         # chosen by the user
assert sc.find_browser("", "vivaldi") == fake["brave"]         # chosen but not installed -> default browser
sc.default_browser_key = lambda: None
assert sc.find_browser() == fake["chrome"]                     # no default we can drive -> first installed
fake.pop("chrome"); assert sc.find_browser() == fake["edge"]
fake.clear()
try:
    sc.find_browser(); raise SystemExit("should fail")
except sc.SiteError as e:
    assert "Chromium-based" in str(e)
assert sc.browser_key("C:\\x\\BraveSoftware\\Brave-Browser\\Application\\brave.exe") == "brave"
assert sc.browser_key("C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe") == "chrome"
assert sc.browser_key("C:\\y\\msedge.exe") == "edge"
from windyagent import config
pub = config.public(config.DEFAULTS)
assert pub["browser"] == "auto" and "installed" in pub["browsers"]
print("browser choice OK (auto = default browser, or pick Brave / Chrome / Edge / Vivaldi / Opera)")
