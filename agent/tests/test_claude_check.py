"""The Claude sentence check: pairs of changed sentences, scoring, weak ones, fixes, the `claude` command line tool."""
import json, os, stat, sys, tempfile
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from windyagent.humanizer import claude_check as cc

ORIG = {3: "Climate change is a big issue. Governments need to act now. Farms are at risk.", 5: "Nothing changes here."}
NEW = {3: "Climate change is a big issue. Governments must act at once. Farms is at risk.", 5: "Nothing changes here."}

def fake_ask(system, user, max_tokens=0):
    items = json.loads(user)
    return "Here you go:\n" + json.dumps([{"id": i["id"], "score": 9 if "Farms is" not in i["rewrite"] else 4,
                                          "issue": "" if "Farms is" not in i["rewrite"] else "grammar", "fix": "" if "Farms is" not in i["rewrite"] else "Farms are at risk of failing."} for i in items])

def test_pairs_and_scores():
    pairs = cc.changed_pairs(ORIG, NEW)
    assert [p["after"] for p in pairs] == ["Governments must act at once.", "Farms is at risk."], pairs
    assert pairs[0]["before"] == "Governments need to act now." and pairs[1]["before"] == "Farms are at risk."
    res = cc.check(pairs, fake_ask, "test")
    assert res["overall"] == 65 and len(res["weak"]) == 1 and res["weak"][0]["fix"].startswith("Farms are"), res
    per = dict(NEW)
    assert cc.apply_fixes(per, res["weak"]) == 1 and "Farms are at risk of failing." in per[3]
    out = cc.write_report({**res, "report": None}, Path(tempfile.mkdtemp()) / "r.txt")
    assert "65/100" in out.read_text() and "Farms is at risk." in out.read_text()

def test_bad_replies_are_survived():
    pairs = cc.changed_pairs(ORIG, NEW)
    assert cc.check(pairs, lambda *a: "sorry, no", "t")["overall"] is None
    def boom(*a): raise RuntimeError("down")
    assert cc.check(pairs, boom, "t")["errors"] == 1

def test_claude_command_line_tool():
    d = Path(tempfile.mkdtemp())
    exe = d / "claude"
    exe.write_text("#!/bin/sh\ncat >/dev/null\necho '[{\"id\":1,\"score\":8,\"issue\":\"\",\"fix\":\"\"},{\"id\":2,\"score\":6,\"issue\":\"odd\",\"fix\":\"Farms are at risk.\"}]'\n")
    exe.chmod(exe.stat().st_mode | stat.S_IEXEC)
    old = os.environ["PATH"]; os.environ["PATH"] = str(d) + os.pathsep + old
    try:
        found = cc.find_claude(); assert found and found.endswith("claude"), found
        res = cc.check(cc.changed_pairs(ORIG, NEW), cc.ClaudeCLI(found).ask, "Claude")
        assert res["overall"] == 70 and res["weak"][0]["score"] == 6, res
    finally:
        os.environ["PATH"] = old

if __name__ == "__main__":
    test_pairs_and_scores(); test_bad_replies_are_survived(); test_claude_command_line_tool(); print("CLAUDE CHECK TESTS PASSED")


def test_review_document_and_safe_edits():
    doc = {0: "Governments must act at once. Farms is at risk. Prices rose by 12 percent in 2020.", 1: "Another paragraph sits here."}
    seen = []
    def ask(system, user, mt=0):
        seen.append((system, user))
        return ('```json\n{"overall": 80, "edits": ['
                '{"before": "Farms is at risk.", "after": "Farms are at risk.", "why": "grammar"},'
                '{"before": "Governments must act at once.", "after": "Everybody else should now do something very different.", "why": "drastic"},'
                '{"before": "Prices rose by 12 percent in 2020.", "after": "Prices rose by 15 percent in 2020.", "why": "number"},'
                '{"before": "Missing sentence here.", "after": "Anything", "why": "ghost"}]}\n```')
    res = cc.review_document(doc, ask, "t", mode="apply")
    assert len(seen) == 1 and "[P0] Governments" in seen[0][1] and "[P1] Another" in seen[0][1] and "drastic changes" in seen[0][0]   # ONE call, whole document
    assert res["overall"] == 80 and len(res["edits"]) == 4 and res["max_edits"] >= 3
    applied, rejected = cc.apply_edits(doc, res["edits"], res["max_edits"])
    assert [a["after"] for a in applied] == ["Farms are at risk."] and doc[0].count("Farms are at risk.") == 1
    assert sorted(r["reason"] for r in rejected) == ["length changed too much", "numbers changed", "sentence not found"], rejected
    # small models: the document is split, each part still gets its own answer
    big = {i: ("Word " * 300).strip() + "." for i in range(5)}
    calls = []
    res = cc.review_document(big, lambda s, u, m=0: calls.append(u) or '{"overall": 70, "edits": []}', "t", chunk_words=700)
    assert len(calls) == 3 and res["overall"] == 70
    assert cc.review_document(doc, lambda *a: "no json", "t")["answered"] is False
    # flag mode: Claude only NAMES sentences; they are located in the document (also when slightly misquoted)
    calls = []
    res = cc.review_document(doc, lambda s, u, m=0: calls.append(s) or '{"overall": 60, "weak": [{"sentence": "Another paragraph sits here", "why": "grammar"}, {"sentence": "Nothing like this anywhere", "why": "x"}]}', "t")
    assert "do not rewrite" in calls[0].lower() or "Do NOT rewrite" in calls[0]
    found = cc.find_sentences(doc, [(e["before"], e["why"]) for e in res["edits"]])
    assert [(i, s) for i, s, _ in found] == [(1, "Another paragraph sits here.")], found

if __name__ == "__main__":
    test_review_document_and_safe_edits(); print("REVIEW TESTS PASSED")
