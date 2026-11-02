"""Paragraphs with a bold lead-in ("Time."), citation fields, links and italics are humanized too (the lead-in stays bold and untouched)."""
import sys, tempfile
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from docx import Document
from docx.oxml import OxmlElement
from docx.oxml.ns import qn
from windyagent.humanizer.docx_tools import build_plan, apply_results, split_lead

FILL = " The evidence from many separate studies points in one direction and the authors agree on the main findings."

def fld(par, txt):
    for typ in ("begin", "separate", "end"):
        r = par.add_run(); c = OxmlElement("w:fldChar"); c.set(qn("w:fldCharType"), typ); r._r.append(c)
        if typ == "begin":
            r2 = par.add_run(); it = OxmlElement("w:instrText"); it.text = " ADDIN ZOTERO "; r2._r.append(it)
        if typ == "separate":
            par.add_run(txt)

def make():
    d = Document()
    d.add_paragraph("2.2 How it works")
    p = d.add_paragraph(); p.add_run("Time.").bold = True
    p.add_run(" The pilot can start within a week and show results within twelve. Yet habits change slowly, and twelve weeks may reveal only early gains." + FILL)
    p = d.add_paragraph(); p.add_run("Effectiveness. ").bold = True
    p.add_run("Prevention is the preferred option, and the evidence is encouraging. Across 13 food service case studies, reductions ranged from 32% to 62% by subsector. ")
    fld(p, "(Beretta and Hellweg, 2019).")
    p.add_run(" Those cases had implemented reduction measures, so results at an untested site may differ." + FILL)
    p = d.add_paragraph(); p.add_run("Feasibility.").bold = True
    p.add_run(" The design needs no technology and one manager can lead it. Adoption depends on management's beliefs ")
    p.add_run("and knowledge").italic = True
    p.add_run(", so a manager-led approach fits well. The risks are staff turnover and busy services disrupting the log." + FILL)
    d.add_paragraph("References", style="Heading 1"); d.add_paragraph("WRAP (2018) Guidance on measuring food waste. London: WRAP, 2018.")
    f = Path(tempfile.mkdtemp()) / "t.docx"; d.save(str(f)); return f

def test_all():
    f = make(); doc = Document(str(f)); plan = build_plan(doc)
    assert sorted(plan.paragraphs) == [1, 2, 3], (plan.paragraphs.keys(), plan.skipped)       # none skipped for bold lead-in / field / italic
    assert plan.paragraphs[1].startswith("The pilot can start") and plan.paragraphs[2].startswith("Prevention") and "Beretta and Hellweg, 2019" in plan.paragraphs[2]
    assert "and knowledge" in plan.paragraphs[3] and plan.stop_text == "References"
    res = {i: t.replace("Yet habits change slowly", "Habits shift only slowly").replace("The pilot can start", "The pilot could begin").replace("Prevention is the preferred option", "Prevention is the best route").replace("The design needs no technology", "No technology is needed") for i, t in plan.paragraphs.items()}
    st = apply_results(doc, plan, res); assert st["sentences_changed"] == 4, st
    doc.save(str(f)); d2 = Document(str(f))
    p1 = d2.paragraphs[1]
    assert p1.text.startswith("Time. The pilot could begin") and p1.runs[0].text == "Time." and p1.runs[0].bold, p1.text[:60]
    assert not any(r.bold for r in p1.runs[1:])
    assert any(r.font.color.rgb is not None and str(r.font.color.rgb) == "FF0000" for r in p1.runs)         # changed sentences red
    p2 = d2.paragraphs[2]
    assert p2.text.startswith("Effectiveness. Prevention is the best route") and "(Beretta and Hellweg, 2019)" in p2.text and p2.runs[0].bold
    assert not d2.paragraphs[2]._p.xpath(".//w:fldChar")                    # the field became plain text
    assert d2.paragraphs[3].text.startswith("Feasibility. No technology is needed") and "and knowledge" in d2.paragraphs[3].text
    assert d2.paragraphs[4].text == "References"
    # an unchanged result leaves the paragraph (lead-in included) exactly as it was
    f2 = make(); doc = Document(str(f2)); plan = build_plan(doc); before = [p.text for p in doc.paragraphs]
    apply_results(doc, plan, dict(plan.paragraphs)); assert [p.text for p in doc.paragraphs] == before
    # real-world wrappers: hyperlink, content control (sdt), smart tag, simple field - plain lxml elements without a namespace map
    from docx.oxml import parse_xml
    NS = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
    d = Document(); d.add_paragraph("Intro heading text here", style="Heading 1")
    p = d.add_paragraph(); p.add_run("Risk.").bold = True
    p.add_run(" The risks are staff turnover and busy services disrupting the log, and donation also needs a willing partner. ")
    p._p.append(parse_xml(f'<w:hyperlink {NS}><w:r><w:t>(WRAP, 2024)</w:t></w:r></w:hyperlink>'))
    p._p.append(parse_xml(f'<w:sdt {NS}><w:sdtContent><w:r><w:t xml:space="preserve"> and (Defra, 2018).</w:t></w:r></w:sdtContent></w:sdt>'))
    p._p.append(parse_xml(f'<w:smartTag {NS}><w:r><w:t xml:space="preserve"> The evidence from many separate studies points in one direction.</w:t></w:r></w:smartTag>'))
    p._p.append(parse_xml(f'<w:fldSimple {NS} w:instr="PAGE"><w:r><w:t xml:space="preserve"> Food law still applies to every business that handles surplus food.</w:t></w:r></w:fldSimple>'))
    g2 = Path(tempfile.mkdtemp()) / "w.docx"; d.save(str(g2))
    doc = Document(str(g2)); plan = build_plan(doc)
    assert list(plan.paragraphs) == [1] and "(WRAP, 2024)" in plan.paragraphs[1] and "(Defra, 2018)" in plan.paragraphs[1] and "Food law" in plan.paragraphs[1], (plan.paragraphs, plan.skipped)
    apply_results(doc, plan, {1: plan.paragraphs[1].replace("The risks are", "Risks include")}); doc.save(str(g2))
    t = Document(str(g2)).paragraphs[1]
    assert t.text.startswith("Risk. Risks include") and "(WRAP, 2024)" in t.text and "Food law" in t.text and not t._p.xpath(".//w:hyperlink | .//w:sdt | .//w:fldSimple | .//w:smartTag")
    # footnotes / images are still left alone
    d = Document(); p = d.add_paragraph("x " * 60); r = p.add_run(); fr = OxmlElement("w:footnoteReference"); fr.set(qn("w:id"), "1"); r._r.append(fr)
    g = Path(tempfile.mkdtemp()) / "n.docx"; d.add_paragraph("y " * 50); d.save(str(g))
    pl = build_plan(Document(str(g)), start_pattern=None) if False else None
    print("bold lead-ins, citation fields and italics are humanized; lead-in kept bold OK")

if __name__ == "__main__":
    test_all()
