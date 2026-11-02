"""Runs the REAL browser code (Playwright + Chromium + StealthwriterClient + the whole humanize job) against the mock site.
Run:  python tests/test_humanizer_browser.py"""
import json
import os
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).parent))
HOME = tempfile.mkdtemp(prefix="wl-hb-")
os.environ["WINDYLAND_AGENT_HOME"] = HOME

from docx import Document                                   # noqa: E402
from docx.shared import RGBColor                            # noqa: E402
from playwright.sync_api import sync_playwright             # noqa: E402

import mock_claude                                          # noqa: E402
import mock_academi                                         # noqa: E402
import mock_stealth                                         # noqa: E402
from sample_doc import make_doc, make_doc_no_intro                             # noqa: E402
from windyagent.humanizer import runner                     # noqa: E402
from windyagent.humanizer.stealth_client import LimitReached, SiteError, StealthwriterClient   # noqa: E402
from windyagent.humanizer.judge import make_judge           # noqa: E402

srv, port = mock_stealth.start()
asrv, aport = mock_academi.start()
csrv, cport = mock_claude.start()
with sync_playwright() as pw:
    CHROMIUM = pw.chromium.executable_path
WORK = Path(tempfile.mkdtemp(prefix="wl-hb-work-"))
PORT = [9400]


def make_cfg(query: str, timeout: int = 60, **site):
    PORT[0] += 1
    return runner.make_config(Path(HOME), {
        "site": {"url": f"http://127.0.0.1:{port}/dashboard/humanizer?{query}", "humanize_timeout_seconds": timeout, **site},
        "browser": {"path": CHROMIUM, "debug_port": PORT[0], "profile_dir": str(WORK / f"profile{PORT[0]}"),
                    "extra_args": ["--headless=new", "--no-sandbox", "--disable-gpu"], "human_delay": [0.05, 0.1]},
        "document": {"open_in_word_when_done": False},
        "academi": {"enabled": False, "url": f"http://127.0.0.1:{aport}/dashboard", "upload_url": f"http://127.0.0.1:{aport}/",
                    "login_timeout_seconds": 60, "score_timeout_seconds": 60},
        "judge": {"engine": "tobi"},
        "claude_check": {"enabled": False, "web_url": f"http://127.0.0.1:{cport}/new", "login_timeout_seconds": 60, "web_answer_seconds": 60},
    })


def kill_browsers():
    import psutil
    for pr in psutil.process_iter(["cmdline", "exe"]):
        try:
            cl = pr.info["cmdline"] or []
            if pr.pid != os.getpid() and cl and os.path.basename(cl[0]).startswith(("chrom", "headless")) and any(a.startswith("--remote-debugging-port=94") for a in cl):
                pr.kill()
        except Exception:
            pass
    time.sleep(0.5)


def test_full_job():
    doc = WORK / "thesis.docx"; make_doc(doc)
    logs = []
    cfg = make_cfg("delay=4&mode=ok")
    t0 = time.time()
    res = runner.run_job(str(doc), "Humanize this file", config=cfg, llm=None, log=lambda m="", *a: logs.append(str(m)))
    took = time.time() - t0
    print("\n".join("   | " + l for l in logs[-14:]))
    assert res["ok"] and res["complete"], res
    assert res["paragraphs_done"] == res["paragraphs_total"] == 3 and res["sentences_changed"] >= 3, res
    assert took >= 4, "must have waited for the generation (took %.1fs)" % took
    d = Document(str(doc))
    red = [r.text for p in d.paragraphs for r in p.runs if r.font.color and r.font.color.rgb == RGBColor(255, 0, 0)]
    assert len(red) == res["sentences_changed"] and all("Seas rising" not in t for t in red), red
    texts = [p.text for p in d.paragraphs]
    assert texts[0] == "A Study of Farm Adaptation" and "Rahman, A. (2019)" in texts[-1] and "Submitted by" in texts[1], texts   # untouched parts
    assert any(t.startswith("Introduction") or t == "Introduction" for t in texts)
    assert (WORK / "thesis.original.docx").exists()
    print(f"full job OK: {res['sentences_changed']} sentences rewritten and red, waited {took:.1f}s for the site")
    kill_browsers()


def test_full_job_without_introduction():
    doc = WORK / "nointro.docx"; make_doc_no_intro(doc)
    logs = []
    res = runner.run_job(str(doc), "Humanize this file", config=make_cfg("delay=2&mode=ok"), llm=None, log=lambda m="", *a: logs.append(str(m)))
    assert res["ok"] and res["complete"] and res["paragraphs_done"] == 3, res
    d = Document(str(doc)); texts = [p.text for p in d.paragraphs]
    assert any("thank my teachers" in t for t in texts) and not any("As noted" in t for t in texts if "thank" in t)
    assert any("Starting at the paragraph: 'Climate change" in l for l in logs), logs[:6]
    print("full job without an Introduction heading OK (starts at the first large paragraph)")
    kill_browsers()


def test_rehumanize_until_100():
    cfg = make_cfg("delay=2&scores=60,85,100&placeholder=1"); cfg["site"]["target_score"] = 100
    logs = []
    with StealthwriterClient(cfg, make_judge({"type": "heuristic"}, lambda *a: None, None), lambda m="", *a: logs.append(str(m))) as c:
        out = c.humanize_batch(["First sentence goes right here. Second sentence follows it closely.", "A second paragraph of text sits here."], "r")
        assert c.page.evaluate("window.__rehumanized") == 2, logs
        assert c.scores[-1]["start"] == 60 and c.scores[-1]["final"] == 100 and c.scores[-1]["rehumanized"] == 2, c.scores
        picked = c.page.evaluate("window.__picked")
    assert all(dot == "green" for _, dot in picked), picked                  # never took a red-dot (still AI-flagged) option
    assert len(out) == 2 and out[0].count(".") == 2, out
    assert any("60% human is under 100%" in l for l in logs) and any("Reached 100% human" in l for l in logs), logs
    print("Rehumanize until 100% OK (60 -> 85 -> 100), only green-dot options chosen")
    kill_browsers()


def test_red_sentences_end_green():
    # every third sentence is red; for the first 2 rounds of options NO green option exists -> New alternatives must be pressed until one does
    cfg = make_cfg("delay=1&scores=95&nogreen=2")
    logs = []
    text = "First sentence goes right here. Second sentence follows it closely. Third sentence is the red one. Fourth sentence ends the text."
    with StealthwriterClient(cfg, make_judge({"type": "heuristic"}, lambda *a: None, None), lambda m="", *a: logs.append(str(m))) as c:
        out = c.humanize_batch([text], "red")
        picked = c.page.evaluate("window.__picked"); more = c.page.evaluate("window.__more")
        colors = c.page.evaluate("[...document.querySelectorAll('#out span[role=button]')].map(s => s.className.includes('red') ? 'red' : 'green')")
        assert c.scores[-1]["start"] == 95 and not c.scores[-1].get("rehumanized"), c.scores     # 95 >= 90: kept, no Rehumanize
    assert more >= 2, (more, logs)
    assert all(dot == "green" for _, dot in picked), picked
    assert "red" not in colors, colors
    assert any("no green option; New alternatives" in l for l in logs) and any("no red sentences left" in l for l in logs), logs
    print("red sentences: New alternatives pressed until a green option, none left red; score 95 kept without Rehumanize")
    kill_browsers()


def test_dialog_lost_in_the_background_is_reopened():
    # the "Pick a rewrite" window closes by itself twice (you clicked elsewhere): the sentence is opened again, nothing fails
    cfg = make_cfg("delay=1&scores=95&vanish=2")
    logs = []
    text = "First sentence goes right here. Second sentence follows it closely. Third sentence ends the text."
    with StealthwriterClient(cfg, make_judge({"type": "heuristic"}, lambda *a: None, None), lambda m="", *a: logs.append(str(m))) as c:
        out = c.humanize_batch([text], "vanish")
        picked = c.page.evaluate("window.__picked")
    assert len(picked) == 3 and any("trying again" in l for l in logs), (picked, logs)
    assert not any("stays as it was" in l for l in logs), logs
    print("a dialog lost while the window was in the background is reopened; all 3 sentences still chosen")
    kill_browsers()


def test_browser_started_with_background_flags():
    cfg = make_cfg("delay=1&scores=95")
    with StealthwriterClient(cfg, make_judge({"type": "heuristic"}, lambda *a: None, None), lambda *a, **k: None) as c:
        assert "--disable-renderer-backgrounding" in StealthwriterClient.BACKGROUND_FLAGS
        assert c.page.evaluate("document.hasFocus()") is True            # focus emulation: the page works while you use another window
    print("browser flags for background work + focus emulation OK")
    kill_browsers()


def test_rehumanize_stops_at_limit_of_tries():
    cfg = make_cfg("delay=1&scores=40,50,60", max_rehumanize=2)
    with StealthwriterClient(cfg, make_judge({"type": "heuristic"}, lambda *a: None, None), lambda *a: None) as c:
        c.open_humanizer(); c.select_level(10); c.paste_text("Some sentence is here for testing. Another sentence is here too.")
        c.click_humanize(); info = c.reach_target_score("x")
        assert info["rehumanized"] == 2 and info["final"] == 60 and c.page.evaluate("window.__rehumanized") == 2, info
    print("Rehumanize stops after the maximum number of tries")
    kill_browsers()


def test_academi_upload_and_download():
    from windyagent.humanizer.academi import AcademiClient
    doc = WORK / "forup.docx"; make_doc(doc)
    logs = []
    cfg = make_cfg("delay=1")
    with AcademiClient(cfg, lambda m="", *a: logs.append(str(m))) as ac:
        r = ac.check_file(doc, WORK / "out1")
    assert r["ok"] and r["ai_percent"] == 34, r
    rep = Path(r["report"]); assert rep.name == "forup_AI-score.pdf" and b"fake report for forup.docx" in rep.read_bytes(), r
    assert any("sign in to academi.cx" in l for l in logs), logs               # it waited for the user to sign in
    kill_browsers()
    # no download button: a screenshot of the report is kept instead
    cfg = make_cfg("delay=1"); cfg["academi"]["url"] = f"http://127.0.0.1:{aport}/dashboard?nodownload=1"
    cfg["academi"]["upload_url"] = f"http://127.0.0.1:{aport}/?nodownload=1"
    with AcademiClient(cfg, lambda *a: None) as ac:
        r = ac.check_file(doc, WORK / "out2")
    assert r["ok"] and r["report"].endswith(".png") and Path(r["report"]).stat().st_size > 1000, r
    print("academi.cx: waits for sign-in, uploads, reads 34% AI, downloads the report (screenshot when there is no download)")
    kill_browsers()


def test_claude_through_claude_ai_in_the_browser():
    from windyagent.humanizer import claude_check as cc, runner as rn
    from windyagent.humanizer.claude_web import ClaudeWebClient, last_review_object
    # the answer is picked out of a page that also contains the instructions' own (invalid) JSON shape
    assert json.loads(last_review_object('{"overall": <0-100>, "edits": [{"before": "<x>"}]} ok {"overall": 80, "edits": [{"before": "a", "after": "b"}]}'))["overall"] == 80
    assert json.loads(last_review_object('{"overall": <1>, "weak": []} ok {"overall": 70, "weak": [{"sentence": "a b c d e f", "why": "x"}]}'))["overall"] == 70
    DOC = "Governments must act at once. Farms are in danger. Prices keep rising every year. Nobody expected this outcome."
    cfg = make_cfg("delay=1")
    logs = []
    mock_claude.HITS["new"] = 0
    with ClaudeWebClient(cfg, lambda m="", *a: logs.append(str(m))) as cw:
        r1 = cc.review_document({0: DOC}, cw.ask, "Claude (claude.ai)", lambda *a: None)                       # flag mode (default)
        r2 = cc.review_document({0: DOC}, cw.ask, "Claude (claude.ai)", lambda *a: None, mode="apply")         # a second question stays in the SAME chat
    assert r1["overall"] == 82 and [e["before"] for e in r1["edits"]][0] == "Governments must act at once." and r1["edits"][0]["after"] == "", r1
    assert r2["overall"] == 82 and len(r2["edits"]) == 3, r2
    assert any("sign in to claude.ai" in l for l in logs), logs
    assert mock_claude.HITS["new"] == 1, mock_claude.HITS   # one chat only
    per = {0: DOC}
    applied, rejected = cc.apply_edits(per, r2["edits"], r2["max_edits"])
    assert len(applied) == 1 and per[0].startswith("Governments must act at once!") and {r["reason"] for r in rejected} == {"length changed too much", "sentence not found"}, (applied, rejected)
    kill_browsers()
    # the whole fallback chain: no `claude` command -> claude.ai in the browser -> (never needed) Tobi's model
    cc.find_claude = lambda: None
    cfg = make_cfg("delay=1"); cfg["claude_check"].update(enabled=True, engine="auto")
    per = {0: DOC}
    got = rn._document_review(None, per, cfg["claude_check"], None, print, cfg, None)
    assert got and got["engine"].startswith("Claude (claude.ai") and got["overall"] == 82 and per[0] == DOC, got      # nothing was reworded
    assert [f[1] for f in got["flagged"]] == ["Governments must act at once."], got["flagged"]                     # the ghost sentence is dropped
    kill_browsers()
    # claude.ai unreachable -> Tobi's own model takes over instead of failing the job
    cfg["claude_check"]["web_url"] = "http://127.0.0.1:9/new"; cfg["claude_check"]["login_timeout_seconds"] = 3
    cfg["claude_check"]["chatgpt_url"] = "http://127.0.0.1:9/new"
    class FakeLLM:
        def ask(self, s, u, m=0): return 'Sure: {"overall": 75, "weak": []}'
    per = {0: DOC}
    got = rn._document_review(None, per, cfg["claude_check"], FakeLLM(), lambda *a: None, cfg, None)
    assert got and "AI model" in got["engine"] and got["overall"] == 75 and not got["flagged"], got
    print("Claude review through claude.ai OK (one chat, sign-in wait, weak sentences named, nothing reworded, fallback to Tobi's model)")
    kill_browsers()


def test_claude_curly_quotes_and_chatgpt_fallback():
    """claude.ai shows its JSON answer as plain text with curly quotes; and when Claude is out, chatgpt.com is asked next."""
    from windyagent.humanizer import claude_check as cc, runner as rn
    DOC = "Governments must act at once. Farms are in danger. Prices keep rising every year. Nobody expected this outcome."
    cc.find_claude = lambda: None
    cfg = make_cfg("delay=1"); cfg["claude_check"].update(enabled=True, engine="auto", web_url=f"http://127.0.0.1:{cport}/new?curly=1")
    per = {0: DOC}
    got = rn._document_review(None, per, cfg["claude_check"], None, lambda *a: None, cfg, None)
    assert got and got["engine"].startswith("Claude (claude.ai") and [f[1] for f in got["flagged"]] == ["Governments must act at once."], got
    kill_browsers()
    cfg["claude_check"].update(web_url="http://127.0.0.1:9/new", login_timeout_seconds=30, chatgpt_url=f"http://127.0.0.1:{cport}/new")
    logs = []
    got = rn._document_review(None, {0: DOC}, cfg["claude_check"], None, lambda m="", *a: logs.append(str(m)), cfg, None)
    assert got and got["engine"].startswith("ChatGPT") and got["overall"] == 82, (got, logs)
    kill_browsers()
    print("Claude's curly-quote answer is read; chatgpt.com is asked when Claude does not answer")


def _fake_llm():
    """Tobi's model for the tests: marks every sentence option 9/10, and names the first sentence of the first paragraph as weak."""
    class FakeLLM:
        def ask(self, system, user, max_tokens=0):
            import json as _j, re as _re
            from windyagent.humanizer.text_utils import split_sentences
            if "original_sentence" in user:
                d = _j.loads(user)
                return _j.dumps({"scores": {str(r["number"]): 9 for r in d["rewrites"]}})
            first = _re.search(r"\[P\d+\] (.+)", user).group(1)
            return _j.dumps({"overall": 88, "weak": [{"sentence": split_sentences(first)[0], "why": "clumsy"}]})
    return FakeLLM()


def test_academi_reads_highlighted_sentences():
    import mock_academi
    from windyagent.humanizer.academi import AcademiClient
    mock_academi.STATE.update(ai=34, high=["Governments must act at once on this large problem.", "Farms are in serious danger from the rising seas."],
                              medium=["Prices keep rising every single year for everyone."], scans=0)
    doc = WORK / "hl.docx"; make_doc(doc)
    cfg = make_cfg("delay=1")
    with AcademiClient(cfg, lambda *a: None) as ac:
        r = ac.check_file(doc, WORK / "out_hl")
    assert r["ai_percent"] == 34 and r["flagged"]["high"] == mock_academi.STATE["high"] and r["flagged"]["medium"] == mock_academi.STATE["medium"], r["flagged"]
    assert r["flagged"]["count"] == [3, 76], r["flagged"]
    print("academi.cx: the blue (high) and purple (medium) sentences are read from the scrolling report, told apart by the legend colours")
    mock_academi.STATE.update(ai=34, high=[], medium=[], scans=0)
    kill_browsers()


def test_academi_loop_redoes_highlighted_sentences_until_zero():
    import mock_academi
    from windyagent.humanizer import academi as am
    from docx import Document
    from windyagent.humanizer.text_utils import split_sentences
    doc = WORK / "loop.docx"; make_doc(doc)
    logs = []
    mock_academi.STATE.update(ai=34, high=[], medium=[], scans=0)
    orig_check = am.AcademiClient.check_file
    seen_files = []
    def check_file(self, path, out_dir):
        # what academi would highlight in THIS upload: the first two sentences of the first body paragraph (blue) and one of the second (purple)
        d = Document(str(path)); paras = [p.text for p in d.paragraphs if len(p.text.split()) > 30]
        seen_files.append([split_sentences(p)[:2] for p in paras[:2]])
        if mock_academi.STATE["scans"] == 0:
            s1, s2 = split_sentences(paras[0]), split_sentences(paras[1])
            mock_academi.STATE.update(ai=34, high=s1[:2], medium=s2[:1])
        else:
            mock_academi.STATE.update(ai=0, high=[], medium=[])
        return orig_check(self, path, out_dir)
    am.AcademiClient.check_file = check_file
    try:
        cfg = make_cfg("delay=2&scores=95")
        cfg["academi"]["enabled"] = True
        res = runner.run_job(str(doc), "Humanize this file", config=cfg, llm=_fake_llm(), log=lambda m="", *a: logs.append(str(m)))
    finally:
        am.AcademiClient.check_file = orig_check
    assert res["ok"] and res["complete"], res
    rounds = res["academi_rounds"]
    assert [r["ai_percent"] for r in rounds] == [34, 0], rounds
    assert rounds[0]["high"] == 2 and rounds[0]["medium"] == 1 and rounds[0]["matched"] == 3 and rounds[0]["refined"] >= 1, rounds
    assert res["academi"]["ai_percent"] == 0 and Path(res["academi"]["report"]).is_file()
    assert len(list(Path(res["output"]).parent.glob("loop_AI-score*"))) == 1, "only the newest report is kept"
    first, second = seen_files[0], seen_files[1]
    assert first[0] != second[0], "the highlighted sentences were replaced in the second upload"
    assert any("improving 3 highlighted" in l for l in logs) and any("done" in l for l in logs), logs
    from windyagent import router
    assert "34% -> 0%" in router.summarize("humanize_document", True, json.dumps(res)), router.summarize("humanize_document", True, json.dumps(res))
    print("academi loop OK: 34% -> highlighted sentences redone on stealthwriter -> saved -> 0%")
    kill_browsers()


def _academi_highlights_first_upload():
    """Patches academi so that the first upload gets highlighted sentences (34%) and the next ones are clean (0%)."""
    from windyagent.humanizer import academi as am
    from docx import Document
    from windyagent.humanizer.text_utils import split_sentences
    import mock_academi
    mock_academi.STATE.update(ai=34, high=[], medium=[], scans=0)
    orig = am.AcademiClient.check_file
    def check_file(self, path, out_dir):
        d = Document(str(path)); paras = [p.text for p in d.paragraphs if len(p.text.split()) > 30]
        if mock_academi.STATE["scans"] == 0:
            mock_academi.STATE.update(ai=34, high=split_sentences(paras[0])[:2], medium=split_sentences(paras[1])[:1])
        else:
            mock_academi.STATE.update(ai=0, high=[], medium=[])
        return orig(self, path, out_dir)
    am.AcademiClient.check_file = check_file
    return lambda: setattr(am.AcademiClient, "check_file", orig)


def test_user_chooses_after_the_first_academi_check():
    """AI found -> Tobi asks: remove it with Tobi, or by yourself. 'By myself' = the edited file and report now; 'with Tobi' = the loop."""
    import mock_academi
    asked = []
    for answer, label in ((False, "decline"), (True, "accept")):
        doc = WORK / f"choice_{label}.docx"; make_doc(doc)
        undo = _academi_highlights_first_upload()
        try:
            cfg = make_cfg("delay=2&scores=95"); cfg["academi"]["enabled"] = True
            res = runner.run_job(str(doc), "Humanize this file", config=cfg, llm=_fake_llm(), log=lambda m="", *a: None,
                                 ask=lambda title, detail, labels, a=answer: (asked.append((title, labels)) or a))
        finally:
            undo()
        rounds = res["academi_rounds"]
        if not answer:
            assert res["academi_declined"] and [r["ai_percent"] for r in rounds] == [34] and "refined" not in rounds[0], rounds
            assert res["academi"]["ai_percent"] == 34 and Path(res["output"]).is_file() and Path(res["academi"]["report"]).is_file(), res
            from windyagent import router
            assert "yourself" in router.summarize("humanize_document", True, json.dumps(res))
        else:
            assert not res.get("academi_declined") and [r["ai_percent"] for r in rounds] == [34, 0], rounds
        kill_browsers()
    assert len(asked) == 2 and "34% AI" in asked[0][0] and asked[0][1] == ["Remove the AI with Tobi", "I'll do it myself"], asked
    print("after the first academi check Tobi asks: remove the AI with Tobi / by yourself - both answers work")


def test_stop_button_saves_the_finished_part():
    """Stop during the job: the finished paragraphs go into <name>_partial.docx (your own file is untouched) and the job returns at once."""
    import threading
    from sample_doc import FakeClient
    from windyagent.humanizer import stop
    doc = WORK / "stopme.docx"; make_doc(doc)
    before = doc.read_bytes()
    ev = threading.Event()
    calls = []
    class StopAfterFirst(FakeClient):
        def humanize_batch(self, paragraphs, label="x"):
            out = FakeClient.humanize_batch(self, paragraphs, label); calls.append(label)
            if len(calls) == 1: ev.set()          # the red button is pressed while the 1st batch is being finished
            return out
    cfg = make_cfg("delay=1"); cfg["document"].update(max_words_per_batch=60, min_words_per_batch=1); cfg["academi"]["enabled"] = False
    cfg["claude_check"]["enabled"] = False
    logs = []
    res = runner.run_job(str(doc), "Humanize this file", config=cfg, client_factory=lambda c, j, l: StopAfterFirst(c, j, l),
                         log=lambda m="", *a: logs.append(str(m)), cancel=ev)
    stop.set_event(None)
    assert res["cancelled"] and res["ok"] and res["output"].endswith("_partial.docx") and Path(res["output"]).is_file(), res
    assert 0 < res["paragraphs_done"] < res["paragraphs_total"], res
    assert doc.read_bytes() == before and len(calls) == 1, "the original file was not touched and no further batch ran"
    # a wait is interrupted within a moment
    import time as _t
    ev2 = threading.Event(); stop.set_event(ev2); threading.Timer(0.3, ev2.set).start(); t0 = _t.time()
    try:
        stop.sleep(30); raise SystemExit("not interrupted")
    except Exception as e:
        assert e.__class__.__name__ == "Cancelled" and _t.time() - t0 < 2, e
    stop.set_event(None)
    print("stop button: finished part saved as _partial.docx, original untouched, waits end within a moment")


def test_full_job_with_claude_check_and_academi():
    import mock_academi
    mock_academi.STATE.update(ai=34, high=[], medium=[], scans=0)
    doc = WORK / "extras.docx"; make_doc(doc)
    logs = []
    cfg = make_cfg("delay=2&scores=70,100", )
    cfg["academi"]["enabled"] = True
    cfg["claude_check"].update(enabled=True, engine="tobi")
    res = runner.run_job(str(doc), "Humanize this file", config=cfg, llm=_fake_llm(), log=lambda m="", *a: logs.append(str(m)))
    assert res["ok"] and res["complete"], res
    cc_ = res["claude_check"]
    assert cc_["overall"] == 88 and cc_["edits"] == 0 and cc_["weak"] == 1 and cc_["refined"] == 1 and Path(cc_["report"]).is_file(), res
    assert res["academi"]["ok"] and res["academi"]["ai_percent"] == 34 and Path(res["academi"]["report"]).name == "extras_AI-score.pdf", res
    assert res["scores"] and res["scores"][0]["final"] == 100 and res["scores"][0]["rehumanized"] == 1, res["scores"]
    assert Path(str(doc).replace(".docx", ".original.docx")).is_file()
    order = [l for l in logs if "Saved:" in l or "Reviewing the document" in l or "back through stealthwriter" in l or "academi.cx:" in l]
    assert "Saved" in order[0] and any("Reviewing" in l for l in order) and any("back through" in l for l in order), order     # saved, reviewed, weak ones redone
    from windyagent import router
    summary = router.summarize("humanize_document", True, json.dumps(res))
    assert "100% human" in summary and "34% AI" in summary and "weak sentence" in summary, summary
    print("full job with Rehumanize + one-chat Claude review (weak sentences redone on stealthwriter) + academi.cx OK:\n   " + summary.replace("\n", "\n   "))
    kill_browsers()


def test_waits_for_generation():
    """The old code returned as soon as the 'Humanized Result' heading was visible (it is there from the start)."""
    cfg = make_cfg("delay=5&mode=ok&placeholder=1")
    judge = make_judge(cfg["judge"], lambda *a: None, None)
    with StealthwriterClient(cfg, judge, lambda *a: None) as c:
        c.open_humanizer(); c.select_level(10); c.paste_text("First sentence is here. Second sentence is here. Third one too.")
        t0 = time.time(); c.click_humanize(); took = time.time() - t0
        assert took >= 4.5, took
        items = c._tag_sentences()[2]
        assert [i["text"] for i in items] == ["First sentence is here.", "Second sentence is here.", "Third one too."], items   # no sidebar links
        assert [i["color"] for i in items] == ["green", "green", "red"], items
    print(f"waits for the generation OK ({took:.1f}s), sidebar links ignored, colours read")
    kill_browsers()


def test_card_contains_the_sentences():
    """The user's real failure: _result_card() returned only the toolbar (heading + Copy), the sentences are a sibling block."""
    cfg = make_cfg("delay=2&mode=ok&placeholder=1")
    judge = make_judge(cfg["judge"], lambda *a: None, None)
    with StealthwriterClient(cfg, judge, lambda *a: None) as c:
        c.open_humanizer(); c.select_level(10); c.paste_text("First sentence is here. Second sentence is here.")
        c.click_humanize()
        card = c._result_card()
        assert card.locator("[role=button]").count() >= 2, "the card must contain the clickable sentences"
        assert card.get_by_text("Click sentences to swap").count() == 1
        # what the old code selected: nearest ancestor with a Copy button -> only the toolbar
        old = c.page.get_by_text("Humanized Result", exact=True).first.locator(
            "xpath=ancestor::*[.//button[normalize-space()='Copy' or @aria-label='Copy'] and not(.//textarea)][1]")
        assert old.locator("[role=button]").count() == 0, "(documenting the old bug) the toolbar alone has no sentences"
    print("result card includes toolbar AND sentences (old selection was toolbar only)")
    kill_browsers()


class ScriptedJudge:
    """Rejects the first set of options, then picks the last-but-one: exercises 'New alternatives'."""
    name = "scripted"
    def __init__(self): self.rounds = {}; self.log = []
    def choose(self, req):
        from windyagent.humanizer.judge import Decision
        k = req.original
        self.rounds[k] = self.rounds.get(k, 0) + 1
        if self.rounds[k] == 1 and not req.force:
            return Decision(None, "first set is poor")
        pick = [c for c in req.candidates if c.label.startswith("Option 2")][0]
        self.log.append((k, pick.text))
        return Decision(pick.idx, "scripted")


def test_new_alternatives_and_plain_markup():
    cfg = make_cfg("delay=2&mode=opaque")
    j = ScriptedJudge()
    with StealthwriterClient(cfg, j, lambda *a: None) as c:
        out = c.humanize_batch(["Alpha beta gamma delta epsilon. Zeta eta theta iota kappa.", "Lambda mu nu xi omicron pi rho sigma."], "t")
    assert len(out) == 2 and all("as well as" in o or "overall" in o for o in out), out        # Option 2 was used everywhere
    assert max(j.rounds.values()) == 2 and len(j.log) == 3, (j.rounds, j.log)                  # asked for new alternatives once each
    assert [r["regenerations"] for r in c.review_log] == [1, 1, 1], c.review_log
    print("new alternatives OK; sentences found even without pointer cursor / colour styling")
    kill_browsers()


def test_icon_copy_button_and_fail_fast():
    cfg = make_cfg("delay=2&mode=ok&iconcopy=1")
    with StealthwriterClient(cfg, make_judge({"type": "heuristic"}, lambda *a: None, None), lambda *a: None) as c:
        out = c.humanize_batch(["One sentence goes here for sure. Another sentence goes here too."], "i")
    assert len(out) == 1 and out[0].count(".") == 2, out
    kill_browsers()
    # sentences with no pointer cursor, no colour and not in spans -> found nothing: must fail within ~25s with page dumps
    # sentences with no role, no pointer cursor, no colour and not in spans -> found nothing: must fail within ~25s with page dumps
    cfg = make_cfg("delay=2&mode=opaque", timeout=120)
    mock_html = mock_stealth.PAGE
    mock_stealth.PAGE = mock_html.replace("const sp = document.createElement('span'); sp.setAttribute('role', 'button'); sp.tabIndex = 0;",
                                          "const sp = document.createElement('i');")
    assert mock_stealth.PAGE != mock_html
    try:
        t0 = time.time()
        with StealthwriterClient(cfg, make_judge({"type": "heuristic"}, lambda *a: None, None), lambda *a: None) as c:
            c.open_humanizer(); c.select_level(10); c.paste_text("Some sentence is written here. Another sentence follows it.")
            try:
                c.click_humanize(); raise AssertionError("should fail")
            except SiteError as e:
                assert "cannot find its sentences" in str(e), e
        assert time.time() - t0 < 40, time.time() - t0
        assert (Path(cfg["debug_dir"]) / "result-no-sentences.html").exists() and (Path(cfg["debug_dir"]) / "result-no-sentences-card.html").exists()
    finally:
        mock_stealth.PAGE = mock_html
    print("icon-only Copy button OK; unrecognisable result fails fast with the page saved")
    kill_browsers()


def test_failures_are_explained():
    judge = make_judge({"type": "heuristic"}, lambda *a: None, None)
    for query, exc, needle in (("mode=nobutton", SiteError, "seemed to do nothing"), ("mode=limit", LimitReached, "limit"),
                               ("mode=error", SiteError, "went wrong")):
        cfg = make_cfg(query, timeout=22)
        with StealthwriterClient(cfg, judge, lambda *a: None) as c:
            c.open_humanizer(); c.select_level(10); c.paste_text("Some text to humanize here. And another.")
            try:
                c.click_humanize(); raise AssertionError("should have failed: " + query)
            except exc as e:
                assert needle in str(e), (query, str(e))
        dbg = Path(cfg["debug_dir"])
        names = sorted(p.name for p in dbg.iterdir())
        assert any(n.endswith(".html") for n in names) and any(n.endswith("-full.png") for n in names), (query, names)
        kill_browsers()
    print("failures OK: 'button did nothing', daily limit and site errors are reported with page dumps saved")


if __name__ == "__main__":
    test_card_contains_the_sentences()
    test_waits_for_generation()
    test_rehumanize_until_100()
    test_red_sentences_end_green()
    test_dialog_lost_in_the_background_is_reopened()
    test_browser_started_with_background_flags()
    test_rehumanize_stops_at_limit_of_tries()
    test_new_alternatives_and_plain_markup()
    test_icon_copy_button_and_fail_fast()
    test_full_job()
    test_full_job_without_introduction()
    test_academi_upload_and_download()
    test_claude_through_claude_ai_in_the_browser()
    test_claude_curly_quotes_and_chatgpt_fallback()
    test_academi_reads_highlighted_sentences()
    test_academi_loop_redoes_highlighted_sentences_until_zero()
    test_user_chooses_after_the_first_academi_check()
    test_stop_button_saves_the_finished_part()
    test_full_job_with_claude_check_and_academi()
    test_failures_are_explained()
    print("ALL HUMANIZER BROWSER TESTS PASSED")
