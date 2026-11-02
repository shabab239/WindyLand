from pathlib import Path
from docx import Document
from docx.shared import Pt
sys_path = None
from windyagent.humanizer.text_utils import split_sentences

BODY1 = ("Climate change is one of the most pressing issues of our time. Rising temperatures affect farming across "
         "the world, e.g. in Bangladesh, where 12.5 million people live near the coast. Governments need to act now. "
         "This study therefore examines adaptation strategies used by small farms between 2010 and 2020 in detail.")
BODY2 = ("The research questions are listed below and guide every later chapter of this report. Each chapter builds "
         "on the previous one and ends with a short summary of the main findings and their limits for readers.")
BODY3 = ("Previous work has mostly focused on large commercial farms rather than smallholders (Rahman, 2019). "
         "However, smallholders produce most of the food in the region and face the greatest exposure to risk.")


def make_doc(path: Path):
    d = Document()
    d.add_paragraph("A Study of Farm Adaptation")
    d.add_paragraph("Submitted by: Some Student, in partial fulfilment of the requirements for the degree of Bachelor")
    t = d.add_table(rows=1, cols=2)
    t.rows[0].cells[0].text = "Name"
    t.rows[0].cells[1].text = "This table cell text is long enough to look like a paragraph but must never be touched at all"
    d.add_heading("Introduction", 1)
    p = d.add_paragraph(); r = p.add_run(BODY1); r.font.name = "Times New Roman"; r.font.size = Pt(12)
    d.add_paragraph(BODY2)
    d.add_heading("Background", 2)
    d.add_paragraph(BODY3)
    d.add_heading("References", 1)
    d.add_paragraph("Rahman, A. (2019). Smallholders and climate risk in Bangladesh. Journal of Farming, 4(2), 1-20.")
    d.save(str(path))


class FakeClient:
    """Pretends to be stealthwriter: rewrites the 2nd sentence of every paragraph."""
    def __init__(self, cfg, judge, log):
        self.review_log = []
    def __enter__(self): return self
    def __exit__(self, *a): pass
    def humanize_batch(self, paragraphs, label="x"):
        out = []
        for p in paragraphs:
            s = split_sentences(p)
            if len(s) > 1:
                s[1] = "Reworded sentence number two that keeps the meaning of the original text."
            out.append(" ".join(s))
        return out


def make_doc_no_intro(path: Path):
    """No 'Introduction' heading: cover page, contents lines, a table, acknowledgements, then big paragraphs straight away."""
    d = Document()
    d.add_paragraph("Marketing Strategy of a Coffee Chain")
    d.add_paragraph("Student ID: 12197 | Unit: Business 101")
    d.add_heading("Table of Contents", 1)
    d.add_paragraph("1. Overview ........ 2")
    d.add_heading("Acknowledgements", 1)
    d.add_paragraph("I would like to thank my teachers and my family for supporting me through every stage of this long project, "
                    "and my friends for reading the early drafts. Their patience and advice made the work possible.")
    t = d.add_table(rows=1, cols=1)
    t.rows[0].cells[0].text = "A table cell that is long enough to look like a paragraph but must never be touched by the humanizer at all"
    d.add_heading("Overview of the company", 1)      # any heading except 'Introduction' - the text just starts
    d.add_paragraph(BODY1)
    d.add_paragraph(BODY2)
    d.add_paragraph("Short caption-like line.")
    d.add_paragraph(BODY3)
    d.add_heading("Bibliography", 1)
    d.add_paragraph("Rahman, A. (2019). Smallholders and climate risk in Bangladesh. Journal of Farming, 4(2), 1-20.")
    d.save(str(path))
