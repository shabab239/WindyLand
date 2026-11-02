"""Tobi's knowledge folder.

A folder on the PC (default: Documents\\Tobi\\Knowledge, created when Tobi is installed or first started). Drop Word, Excel, PDF,
PowerPoint, text, Markdown or CSV files into it at any time. Tobi reads them, stores what is in them in a small search index
(``knowledge_index.json`` in his data folder) and uses it to answer questions about your own documents.

* Changed and new files are picked up automatically (checked by modification time); removed files are dropped from the index.
* Everything stays on this PC. Only the few passages that match a question are shown to the AI model that answers it.
* No extra services: plain keyword search (BM25) over passages of ~150 words.
"""
from __future__ import annotations

import json
import math
import os
import re
import threading
import zipfile
from pathlib import Path

from . import config

EXTS = {".docx", ".xlsx", ".xlsm", ".pdf", ".pptx", ".txt", ".md", ".csv", ".json", ".log", ".rtf", ".html", ".htm"}
CHUNK_WORDS, OVERLAP = 150, 30
MAX_FILE_BYTES = 60 * 1024 * 1024
STOP = set("a an and are as at be but by for from has have he her his i if in into is it its me my of on or our she so that the their them "
           "then there these they this to was we were what when where which who will with you your do does did can could would should about "
           "tell show find give please tobi".split())
README = ("This is Tobi's MEMORY FOLDER.\r\n\r\nDrop any file in here and Tobi will read it and memorize it.\r\n\r\nPut Word (.docx), Excel (.xlsx), PDF, PowerPoint (.pptx), text, Markdown or CSV files here.\r\n"
          "Tobi reads them and remembers what is in them. Then ask him things like:\r\n"
          "  \"What does my report say about the budget?\"   \"Search my notes for the deadline\"   \"What files are in my knowledge folder?\"\r\n"
          "You can add, change or delete files whenever you like; Tobi notices by himself.\r\n")
_lock = threading.RLock()


# --------------------------------------------------------------------------- where it lives
def _documents_dir() -> Path:
    if os.name == "nt":
        try:
            import ctypes
            from ctypes import wintypes

            class GUID(ctypes.Structure):
                _fields_ = [("a", wintypes.DWORD), ("b", wintypes.WORD), ("c", wintypes.WORD), ("d", ctypes.c_ubyte * 8)]

            g = GUID(0xFDD39AD0, 0x238F, 0x46AF, (ctypes.c_ubyte * 8)(0xAD, 0xB4, 0x6C, 0x85, 0x48, 0x03, 0x69, 0xC7))
            out = ctypes.c_wchar_p()
            if ctypes.windll.shell32.SHGetKnownFolderPath(ctypes.byref(g), 0, None, ctypes.byref(out)) == 0 and out.value:
                p = Path(out.value)
                ctypes.windll.ole32.CoTaskMemFree(out)
                return p
        except Exception:
            pass
    return Path.home() / "Documents"


def folder(create: bool = True) -> Path:
    env = os.environ.get("WINDYLAND_KNOWLEDGE")
    custom = ""
    if not env:
        try:
            custom = str(config.load().get("knowledge_folder") or "")
        except Exception:
            custom = ""
    p = Path(os.path.expandvars(os.path.expanduser(env or custom))) if (env or custom) else _documents_dir() / "Tobi" / "Knowledge"
    if create:
        try:
            p.mkdir(parents=True, exist_ok=True)
            r = p / "README.txt"
            if not r.exists():
                r.write_text(README, encoding="utf-8")
        except OSError:
            pass
    return p


def _index_file() -> Path:
    return config.data_dir() / "knowledge_index.json"


# --------------------------------------------------------------------------- reading files
def _words(t: str) -> list[str]:
    return t.split()


def _read_docx(p: Path) -> list[tuple[str, str]]:
    from docx import Document

    d = Document(str(p))
    out = [("", para.text) for para in d.paragraphs if para.text.strip()]
    for ti, tb in enumerate(d.tables, 1):
        for row in tb.rows:
            cells = []
            for c in row.cells:
                if c.text.strip() and (not cells or cells[-1] != c.text.strip()):
                    cells.append(c.text.strip())
            if cells:
                out.append((f"table {ti}", " | ".join(cells)))
    return out


def _read_xlsx(p: Path) -> list[tuple[str, str]]:
    from openpyxl import load_workbook

    wb = load_workbook(str(p), read_only=True, data_only=True)
    out = []
    try:
        for ws in wb.worksheets:
            header: list[str] = []
            for n, row in enumerate(ws.iter_rows(values_only=True), 1):
                vals = ["" if v is None else str(v).strip() for v in row]
                if not any(vals):
                    continue
                if not header:
                    header = vals
                    out.append((f"sheet {ws.title}", " | ".join(v for v in vals if v)))
                    continue
                cells = [f"{header[i]}: {v}" if i < len(header) and header[i] else v for i, v in enumerate(vals) if v]
                out.append((f"sheet {ws.title} row {n}", "; ".join(cells)))
                if n > 20000:
                    break
    finally:
        wb.close()
    return out


def _read_pdf(p: Path) -> list[tuple[str, str]]:
    from pypdf import PdfReader

    r = PdfReader(str(p))
    if r.is_encrypted:
        try:
            r.decrypt("")
        except Exception:
            return []
    out = []
    for i, page in enumerate(r.pages, 1):
        try:
            t = page.extract_text() or ""
        except Exception:
            t = ""
        if t.strip():
            out.append((f"page {i}", t))
    return out


def _read_pptx(p: Path) -> list[tuple[str, str]]:
    out = []
    with zipfile.ZipFile(p) as z:
        slides = sorted((n for n in z.namelist() if re.fullmatch(r"ppt/slides/slide\d+\.xml", n)), key=lambda n: int(re.findall(r"\d+", n)[-1]))
        for n in slides:
            xml = z.read(n).decode("utf-8", "replace")
            text = " ".join(re.findall(r"<a:t>([^<]*)</a:t>", xml))
            text = text.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
            num = re.findall(r"\d+", n)[-1]
            if text.strip():
                out.append((f"slide {int(num)}", text))
    return out


def _read_plain(p: Path) -> list[tuple[str, str]]:
    raw = p.read_bytes()
    for enc in ("utf-8-sig", "utf-16", "cp1252", "latin-1"):
        try:
            t = raw.decode(enc)
            break
        except UnicodeError:
            continue
    else:
        return []
    if p.suffix.lower() in (".html", ".htm"):
        t = re.sub(r"(?is)<(script|style).*?</\1>", " ", t)
        t = re.sub(r"<[^>]+>", " ", t)
    if p.suffix.lower() == ".rtf":
        t = re.sub(r"\\'[0-9a-f]{2}|\\[a-z]+-?\d* ?|[{}]", " ", t)
    return [("", ln) for ln in t.splitlines() if ln.strip()]


READERS = {".docx": _read_docx, ".xlsx": _read_xlsx, ".xlsm": _read_xlsx, ".pdf": _read_pdf, ".pptx": _read_pptx}


def extract(p: Path) -> list[tuple[str, str]]:
    """[(location, text), ...] for one file; raises on unreadable files."""
    return READERS.get(p.suffix.lower(), _read_plain)(p)


def chunk(parts: list[tuple[str, str]]) -> list[dict]:
    """Passages of ~150 words that remember where they came from (page, sheet, slide...)."""
    chunks: list[dict] = []
    buf: list[str] = []
    loc0 = ""
    nwords = 0

    def flush(keep_tail: bool):
        nonlocal buf, nwords
        if buf:
            text = " ".join(buf).strip()
            if text:
                chunks.append({"loc": loc0, "text": text})
            w = _words(text)
            buf = [" ".join(w[-OVERLAP:])] if keep_tail and len(w) > OVERLAP else []
            nwords = len(_words(buf[0])) if buf else 0

    for loc, text in parts:
        text = " ".join(text.split())
        if not text:
            continue
        if not buf:
            loc0 = loc
        elif loc != loc0 and loc.split(" row")[0] != loc0.split(" row")[0]:
            flush(False)   # new page / sheet / slide: a fresh passage, so the location stays exact
            loc0 = loc
        for w in _words(text):
            buf.append(w)
            nwords += 1
            if nwords >= CHUNK_WORDS:
                flush(True)
                loc0 = loc
    flush(False)
    return chunks


# --------------------------------------------------------------------------- the index
def _load_index() -> dict:
    try:
        d = json.loads(_index_file().read_text(encoding="utf-8"))
        if isinstance(d, dict) and isinstance(d.get("files"), dict):
            return d
    except (OSError, ValueError):
        pass
    return {"folder": "", "files": {}}


def _save_index(idx: dict) -> None:
    f = _index_file()
    f.parent.mkdir(parents=True, exist_ok=True)
    tmp = f.with_suffix(".tmp")
    tmp.write_text(json.dumps(idx, ensure_ascii=False), encoding="utf-8")
    tmp.replace(f)


def _scan(root: Path) -> dict[str, Path]:
    out = {}
    if not root.is_dir():
        return out
    for dp, dn, fn in os.walk(root):
        dn[:] = [d for d in dn if not d.startswith((".", "~"))]
        for n in fn:
            p = Path(dp) / n
            if n.startswith(("~$", ".")) or p.suffix.lower() not in EXTS or n.lower() == "readme.txt":
                continue
            out[str(p.relative_to(root)).replace("\\", "/")] = p
    return out


def refresh(force: bool = False, log=None) -> dict:
    """Bring the index in line with the folder. Returns {added, updated, removed, unreadable, files, chunks}."""
    with _lock:
        root = folder()
        idx = _load_index()
        if idx.get("folder") != str(root):
            idx = {"folder": str(root), "files": {}}
        files = _scan(root)
        stats = {"added": 0, "updated": 0, "removed": 0, "unreadable": []}
        for rel in [r for r in idx["files"] if r not in files]:
            del idx["files"][rel]
            stats["removed"] += 1
        for rel, p in files.items():
            try:
                st = p.stat()
            except OSError:
                continue
            old = idx["files"].get(rel)
            if old and not force and old.get("mtime") == st.st_mtime and old.get("size") == st.st_size and not old.get("error"):
                continue
            if st.st_size > MAX_FILE_BYTES:
                idx["files"][rel] = {"mtime": st.st_mtime, "size": st.st_size, "chunks": [], "error": "file too large"}
                stats["unreadable"].append(rel)
                continue
            try:
                chunks = chunk(extract(p))
                idx["files"][rel] = {"mtime": st.st_mtime, "size": st.st_size, "chunks": chunks}
                stats["updated" if old else "added"] += 1
                if log:
                    log(f"knowledge: read {rel} ({len(chunks)} passages)")
            except Exception as e:
                idx["files"][rel] = {"mtime": st.st_mtime, "size": st.st_size, "chunks": [], "error": f"{e.__class__.__name__}: {str(e)[:100]}"}
                stats["unreadable"].append(rel)
        if stats["added"] or stats["updated"] or stats["removed"] or stats["unreadable"] or force:
            _save_index(idx)
        stats["files"] = len(idx["files"])
        stats["chunks"] = sum(len(f["chunks"]) for f in idx["files"].values())
        return stats


def _tokens(t: str) -> list[str]:
    out = []
    for w in re.findall(r"[a-z0-9À-ɏ]+", t.lower()):
        if w in STOP or (len(w) < 2 and not w.isdigit()):
            continue
        out.append(w[:-1] if len(w) > 4 and w.endswith("s") and not w.endswith("ss") else w)
    return out


def search(query: str, limit: int = 5) -> list[dict]:
    refresh()
    idx = _load_index()
    q = _tokens(query)
    if not q:
        return []
    docs = []   # (rel, loc, text, tokens)
    for rel, f in idx["files"].items():
        for c in f["chunks"]:
            docs.append((rel, c["loc"], c["text"], _tokens(c["text"] + " " + rel)))
    if not docs:
        return []
    n = len(docs)
    avg = sum(len(d[3]) for d in docs) / n or 1
    df: dict[str, int] = {}
    for d in docs:
        for w in set(d[3]):
            df[w] = df.get(w, 0) + 1
    qset = set(q)
    scored = []
    for rel, loc, text, toks in docs:
        tf: dict[str, int] = {}
        for w in toks:
            if w in qset:
                tf[w] = tf.get(w, 0) + 1
        if not tf:
            continue
        s = 0.0
        for w, f in tf.items():
            idf = math.log(1 + (n - df[w] + 0.5) / (df[w] + 0.5))
            s += idf * (f * 2.2) / (f + 1.2 * (0.25 + 0.75 * len(toks) / avg))
        s *= 1 + 0.25 * (len(tf) - 1)   # passages containing more of the different query words rank higher
        scored.append((s, rel, loc, text))
    scored.sort(key=lambda x: -x[0])
    out, per_file = [], {}
    for s, rel, loc, text in scored:
        if per_file.get(rel, 0) >= 3:
            continue
        per_file[rel] = per_file.get(rel, 0) + 1
        out.append({"file": rel, "loc": loc, "text": text, "score": round(s, 2)})
        if len(out) >= limit:
            break
    return out


def files_summary() -> list[dict]:
    refresh()
    idx = _load_index()
    return [{"file": rel, "passages": len(f["chunks"]), "size": f["size"], "error": f.get("error")} for rel, f in sorted(idx["files"].items())]


def read_file_text(name: str, max_chars: int = 12000) -> str | None:
    refresh()
    idx = _load_index()
    rels = [r for r in idx["files"] if r.lower() == name.lower()] or [r for r in idx["files"] if name.lower() in r.lower()]
    if not rels:
        return None
    f = idx["files"][rels[0]]
    text = "\n".join((f"[{c['loc']}] " if c["loc"] else "") + c["text"] for c in f["chunks"])
    return f"{rels[0]}\n{text[:max_chars]}" + ("\n... (cut)" if len(text) > max_chars else "")


def prompt_hint() -> str:
    """One line for the system prompt: which documents Tobi knows about."""
    try:
        idx = _load_index()
        names = list(idx["files"])[:12]
        return ("Files in the knowledge folder: " + ", ".join(names) + (" ..." if len(idx["files"]) > 12 else "")) if names else ""
    except Exception:
        return ""
