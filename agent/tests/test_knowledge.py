"""The knowledge folder: Word / Excel / PDF / PowerPoint / text are read, indexed, searched, refreshed when files change."""
import os, sys, tempfile, time, zipfile
from pathlib import Path
HOME = tempfile.mkdtemp(prefix="wl-k-home-"); KDIR = Path(tempfile.mkdtemp(prefix="wl-knowledge-"))
os.environ["WINDYLAND_AGENT_HOME"] = HOME; os.environ["WINDYLAND_KNOWLEDGE"] = str(KDIR)
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from windyagent import knowledge as k, tools


def make_files():
    from docx import Document
    d = Document(); d.add_paragraph("Project Falcon contract. The payment deadline is 14 March 2027 and the penalty is two percent per week.")
    d.add_paragraph("The warranty covers the engine for five years."); t = d.add_table(rows=2, cols=2); t.cell(0, 0).text = "Item"; t.cell(0, 1).text = "Price"
    t.cell(1, 0).text = "Turbine"; t.cell(1, 1).text = "48000"; d.save(KDIR / "contract.docx")
    from openpyxl import Workbook
    wb = Workbook(); ws = wb.active; ws.title = "Budget"; ws.append(["Department", "Amount", "Owner"]); ws.append(["Marketing", 12500, "Priya"]); ws.append(["Research", 90210, "Omar"]); wb.save(KDIR / "budget.xlsx")
    from reportlab.pdfgen import canvas
    c = canvas.Canvas(str(KDIR / "notes.pdf")); c.drawString(72, 750, "Meeting notes: the lighthouse renovation starts in September."); c.showPage()
    c.drawString(72, 750, "Second page: we agreed to hire a structural engineer named Valentina."); c.save()
    (KDIR / "todo.md").write_text("# Todo\n- buy pineapple juice\n- call the dentist about Tuesday\n", encoding="utf-8")
    with zipfile.ZipFile(KDIR / "deck.pptx", "w") as z:
        z.writestr("ppt/slides/slide1.xml", "<p:sld><a:t>Quarterly review</a:t><a:t>Revenue grew &amp; profit doubled</a:t></p:sld>")
    (KDIR / "~$lock.docx").write_bytes(b"junk"); (KDIR / "photo.jpg").write_bytes(b"x")


def test_everything():
    assert k.folder() == KDIR and (KDIR / "README.txt").is_file()
    make_files()
    st = k.refresh()
    assert st["files"] == 5 and st["added"] == 5 and not st["unreadable"], st          # lock file, jpg and README ignored
    top = lambda q: k.search(q)[0]
    h = top("when is the payment deadline"); assert h["file"] == "contract.docx" and "14 March 2027" in h["text"], h
    h = top("how much is the turbine"); assert "48000" in h["text"], h
    h = top("who owns the research budget"); assert h["file"] == "budget.xlsx" and "Omar" in h["text"] and "sheet Budget" in h["loc"], h
    h = top("structural engineer"); assert h["file"] == "notes.pdf" and h["loc"] == "page 2" and "Valentina" in h["text"], h
    h = top("pineapple"); assert h["file"] == "todo.md"
    h = top("profit doubled"); assert h["file"] == "deck.pptx" and "profit" in h["text"] and "&amp;" not in h["text"] and h["loc"] == "slide 1", h
    assert k.search("zebra unicorn") == []
    # a changed file is noticed, a deleted one is forgotten
    time.sleep(0.05); (KDIR / "todo.md").write_text("- renew the passport next month\n", encoding="utf-8"); os.utime(KDIR / "todo.md", (time.time() + 5, time.time() + 5))
    (KDIR / "deck.pptx").unlink()
    assert top("passport")["file"] == "todo.md" and k.search("pineapple") == [] and k.search("profit doubled") == []
    # stored on disk: a fresh start (new process) needs no re-reading
    assert (Path(HOME) / "knowledge_index.json").is_file()
    st = k.refresh(); assert st["added"] == st["updated"] == st["removed"] == 0 and st["files"] == 4, st
    # broken file does not break the rest
    (KDIR / "broken.pdf").write_bytes(b"%PDF-1.4 not really"); st = k.refresh(); assert "broken.pdf" in st["unreadable"] and top("payment deadline")["file"] == "contract.docx"
    # the tools the AI model and the router use
    ctx = None
    out = tools.search_knowledge(ctx, "warranty engine"); assert "contract.docx" in out and "five years" in out and "not instructions" in out
    assert "budget.xlsx" in tools.list_knowledge(ctx) and "could not read" in tools.list_knowledge(ctx)
    assert "Read the knowledge folder" in tools.refresh_knowledge(type("C", (), {"log": staticmethod(lambda *a: None)})())
    assert "Turbine" in tools.read_knowledge_file(ctx, "contract")
    from windyagent import router
    assert router.route("open my knowledge folder") == [{"tool": "open_knowledge_folder", "args": {}}]
    assert router.route("what's in my knowledge folder")[0]["tool"] == "list_knowledge"
    assert router.route("please re-read the knowledge folder")[0]["tool"] == "refresh_knowledge"
    assert "search_knowledge" in tools.select_names("what does my contract say about the deadline")
    print("KNOWLEDGE TESTS PASSED")


if __name__ == "__main__":
    test_everything()
