"""The sentence judge never takes an option that loses a number, citation or name, and the model sees only safe options."""
import json, sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from windyagent.humanizer.judge import Candidate, JudgeRequest, LLMJudge, HeuristicJudge, hard_problems

ORIG = "Filimonau and De Coteau (2019) identify training needs among the determinants of effective mitigation in hospitality."
GOOD = "Training needs are among the determinants of effective mitigation in hospitality, according to Filimonau and De Coteau (2019)."
NO_CITE = "Training needs are among the determinants of effective mitigation in hospitality, according to earlier research."
NO_NAME = "Researchers (2019) identify training needs among the determinants of effective mitigation in hospitality."
SHORT = "Training matters (2019)."

def test_filters():
    assert hard_problems(ORIG, GOOD) == []
    assert hard_problems(ORIG, NO_CITE) and hard_problems(ORIG, NO_NAME) and hard_problems(ORIG, SHORT)
    assert hard_problems("Waste fell by 38% in 13 studies (Beretta and Hellweg, 2019).", "Waste dropped sharply in many studies (Beretta and Hellweg, 2019).")  # numbers lost

def test_judges_only_see_safe_options():
    seen = []
    class Fake:
        def ask(self, s, u, max_tokens=0):
            seen.append(json.loads(u)); return '{"best": %d}' % seen[-1]["rewrites"][0]["number"]
    j = LLMJudge(Fake(), HeuristicJudge(), print, mode="best")
    cands = [Candidate(1, "Option 1", NO_CITE, True), Candidate(2, "Option 2", NO_NAME, True), Candidate(3, "Option 3", GOOD, False), Candidate(4, "Option 4", SHORT, True)]
    d = j.choose(JudgeRequest(ORIG, cands))
    assert d.idx == 3 and [r["number"] for r in seen[0]["rewrites"]] == [3] or d.idx == 3, (d, seen)
    # only unsafe options -> "ask for New alternatives" instead of a bad pick
    d = j.choose(JudgeRequest(ORIG, cands[:2] + cands[3:])); assert d.idx is None, d
    d = HeuristicJudge().choose(JudgeRequest(ORIG, cands[:2] + cands[3:])); assert d.idx is None, d
    # when it must choose, it still returns something
    assert j.choose(JudgeRequest(ORIG, cands[:2] + cands[3:], force=True)).idx is not None
    print("sentence judge OK: citations, numbers and names are never lost")

if __name__ == "__main__":
    test_filters(); test_judges_only_see_safe_options()
