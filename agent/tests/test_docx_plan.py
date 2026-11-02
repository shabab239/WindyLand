"""Where the humanizer starts: at the first large paragraph, Introduction heading optional."""
import sys, tempfile
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1])); sys.path.insert(0, str(Path(__file__).parent))
from docx import Document
from sample_doc import make_doc, make_doc_no_intro, BODY1, BODY3
from windyagent.humanizer.docx_tools import PlanError, build_plan

W = Path(tempfile.mkdtemp())

def test_no_intro():
    f = W / "n.docx"; make_doc_no_intro(f)
    plan = build_plan(Document(str(f)))
    texts = list(plan.paragraphs.values())
    assert texts[0].startswith("Climate change") and len(texts) == 3, texts          # starts at the first big paragraph
    assert all("thank my teachers" not in t and "table cell" not in t and "Rahman, A." not in t and "caption-like" not in t for t in texts)
    assert plan.stop_text == "Bibliography" and plan.start_text.startswith("Climate change"), (plan.stop_text, plan.start_text)
    assert [u.para_index for u in plan.units] == sorted(plan.paragraphs)

def test_with_intro_heading_still_works():
    f = W / "i.docx"; make_doc(f)
    plan = build_plan(Document(str(f)))
    assert len(plan.paragraphs) == 3 and plan.stop_text == "References", plan.paragraphs
    assert BODY1[:30] in list(plan.paragraphs.values())[0]

def test_explicit_start_heading_and_errors():
    f = W / "i2.docx"; make_doc(f)
    plan = build_plan(Document(str(f)), start_pattern=r"^background$")
    assert list(plan.paragraphs.values()) == [" ".join(BODY3.split())], plan.paragraphs
    try:
        build_plan(Document(str(f)), start_pattern="nonexistent heading"); raise AssertionError
    except PlanError as e:
        assert "start at the first large paragraph" in str(e)
    d = Document(); d.add_paragraph("Only a title"); d.add_paragraph("Too short."); g = W / "e.docx"; d.save(str(g))
    try:
        build_plan(Document(str(g))); raise AssertionError
    except PlanError as e:
        assert "large paragraph" in str(e)

if __name__ == "__main__":
    test_no_intro(); test_with_intro_heading_still_works(); test_explicit_start_heading_and_errors()
    print("DOCX PLAN TESTS PASSED")


def test_batches_of_at_least_800_words():
    from windyagent.humanizer.docx_tools import Plan, Unit
    def mk(ws):
        p = Plan(0, 0, "", None); p.units = [Unit(i, 0, 1, "w " * w) for i, w in enumerate(ws)]; return p
    sizes = lambda ws: [sum(u.words for u in b) for b in mk(ws).batches(900, 800)]
    assert sizes([300] * 5) == [900, 600]          # 1500 words cannot make two batches of 800 without splitting a paragraph
    assert sizes([200, 250, 300]) == [750]          # everything in one paste when the total is small
    assert sizes([400, 400, 400, 400, 150]) == [800, 950]
    assert sizes([1200]) == [1200]
    assert all(s >= 800 for s in sizes([250] * 12)[:-1]) and sum(sizes([250] * 12)) == 3000
    print("batches of 800+ words OK")

if __name__ == "__main__":
    test_batches_of_at_least_800_words()
