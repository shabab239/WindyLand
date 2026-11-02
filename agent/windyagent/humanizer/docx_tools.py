"""Word (.docx) side of the humanizer agent.

What it does
  * starts at the first large body paragraph (an "Introduction" heading is NOT required) and takes body paragraphs from there on
  * never touches cover pages, tables, headings, captions, lists, references, anything complex
  * cuts the body text into batches of at most N words (the website allows 1000)
  * writes the humanized text back, keeping paragraph formatting, and colours every
    changed sentence red
"""
from __future__ import annotations

import copy
import re
from dataclasses import dataclass, field
from difflib import SequenceMatcher

from docx.oxml.ns import qn
from docx.shared import RGBColor

from .text_utils import chunk_by_sentences, norm, split_sentences, word_count

INTRO_RE = re.compile(
    r"^\s*(?:(?:chapter|section)\s+)?(?:(?:\d+(?:\.\d+)*|[ivxlc]+)[.):]?\s*)?introduction\s*:?\s*$", re.I
)
DEFAULT_STOP_RE = re.compile(
    r"^\s*(?:(?:chapter|section)\s+)?(?:\d+(?:\.\d+)*[.):]?\s*)?"
    r"(?:references?(?:\s+list)?|reference\s+list|bibliography|works\s+cited|appendix(?:\s+[a-z0-9]+)?|"
    r"appendices|acknowledge?ments?)\s*:?\s*$",
    re.I,
)
CAPTION_RE = re.compile(r"^\s*(figure|fig\.?|table|chart|image|plate|source)\s*[\dIVXivx]+([.:)\-–—]|\s)", re.I)
NUMBERED_HEADING_RE = re.compile(r"^\s*\d+(\.\d+)*[.)]?\s+\S")

# anything in a paragraph that we cannot safely rebuild from plain text
COMPLEX_XPATH = (
    ".//w:hyperlink | .//w:drawing | .//w:pict | .//w:object | .//w:fldChar | .//w:fldSimple | "
    ".//w:instrText | .//w:footnoteReference | .//w:endnoteReference | .//w:ins | .//w:del | "
    ".//w:commentRangeStart | .//w:smartTag | .//w:sdt | .//m:oMath | .//m:oMathPara"
)
# the text of these can be rebuilt from plain text (citation fields, links, content controls): they are flattened when humanizing
FLATTEN_XPATH = ".//w:hyperlink | .//w:fldChar | .//w:fldSimple | .//w:instrText | .//w:sdt | .//w:smartTag"
KEEP_XPATH = (".//w:drawing | .//w:pict | .//w:object | .//w:footnoteReference | .//w:endnoteReference | .//w:ins | .//w:del | "
              ".//w:commentRangeStart | .//m:oMath | .//m:oMathPara")
BODY_CHILDREN = ("w:r", "w:proofErr", "w:hyperlink", "w:fldSimple", "w:sdt", "w:smartTag")
BREAKS_XPATH = "./w:r/w:br | ./w:r/w:cr | ./w:r/w:tab"


class PlanError(Exception):
    pass


@dataclass
class Unit:
    """One piece of text that is pasted into the website as its own paragraph."""

    para_index: int
    part: int
    parts: int
    text: str

    @property
    def words(self) -> int:
        return word_count(self.text)


@dataclass
class Plan:
    start_index: int
    end_index: int  # exclusive
    start_text: str
    stop_text: str | None
    paragraphs: dict[int, str] = field(default_factory=dict)  # para_index -> text (eligible)
    units: list[Unit] = field(default_factory=list)
    skipped: list[tuple[int, str, str]] = field(default_factory=list)  # (index, reason, preview)

    def batches(self, max_words: int, min_words: int = 0) -> list[list[Unit]]:
        """Group paragraphs into pastes of at least ``min_words`` (joining sections when needed) and at most about ``max_words``.
        A single paragraph bigger than the limit stays alone; a short tail is merged into / balanced with the previous batch."""
        hard = max_words + max(150, max_words // 5)
        out: list[list[Unit]] = []
        cur: list[Unit] = []
        words = 0
        for u in self.units:
            if cur and words >= min_words and words + u.words > max_words:
                out.append(cur)
                cur, words = [], 0
            elif cur and words < min_words and words + u.words > hard:
                out.append(cur)   # cannot reach the minimum without blowing the limit
                cur, words = [], 0
            cur.append(u)
            words += u.words
        if cur:
            out.append(cur)
        # a short tail: merge it into the previous batch, or take paragraphs from the previous one to reach the minimum
        while len(out) > 1 and sum(u.words for u in out[-1]) < min_words:
            last, prev = out[-1], out[-2]
            lw, pw = sum(u.words for u in last), sum(u.words for u in prev)
            if lw + pw <= hard:
                out[-2:] = [prev + last]
                continue
            moved = False
            while len(prev) > 1 and lw < min_words and pw - prev[-1].words >= min_words:
                u = prev.pop()
                last.insert(0, u)
                lw += u.words
                pw -= u.words
                moved = True
            if not moved:
                break
            break
        return out

    def skip_counts(self) -> dict[str, int]:
        counts: dict[str, int] = {}
        for _, reason, _ in self.skipped:
            counts[reason] = counts.get(reason, 0) + 1
        return counts


# --------------------------------------------------------------------------- classification
def _style_name(p) -> str:
    try:
        return (p.style.name or "") if p.style is not None else ""
    except Exception:
        return ""


def _outline_level_is_heading(el) -> bool:
    for v in el.xpath("./w:pPr/w:outlineLvl/@w:val"):
        try:
            return int(v) < 9
        except ValueError:
            return False
    return False


def is_heading_style(p) -> bool:
    name = _style_name(p).lower()
    if name.startswith("heading") or name in ("title", "subtitle") or name.startswith("toc heading"):
        return True
    if _outline_level_is_heading(p._p):
        return True
    st = p.style
    depth = 0
    while st is not None and depth < 10:
        if _outline_level_is_heading(st.element):
            return True
        st = st.base_style
        depth += 1
    return False


def is_toc(p) -> bool:
    name = _style_name(p).lower()
    return name.startswith("toc") or name.startswith("table of") or "contents" in name or name.startswith("index")


def looks_like_heading(p, text: str) -> bool:
    """Headings that were made by hand (bold / numbered) instead of with a Heading style."""
    words = text.split()
    if not words or len(words) > 15:
        return False
    if text.rstrip().endswith((".", "!", "?")) and len(words) > 3:
        return False
    runs = [r for r in p.runs if r.text.strip()]
    if runs and all(bool(r.bold) for r in runs):
        return True
    if NUMBERED_HEADING_RE.match(text) and len(words) <= 12 and not text.rstrip().endswith((".", "!", "?")):
        return True
    return False


def is_list_item(p) -> bool:
    return bool(p._p.xpath("./w:pPr/w:numPr")) or _style_name(p).lower().startswith("list")


def _fmt_key(r) -> tuple:
    f = r.font
    try:
        c = f.color
        ckey = (c.type, str(c.rgb) if c.type is not None and c.rgb is not None else None,
                c.theme_color if c.type is not None else None)
    except Exception:
        ckey = None
    try:
        style = r.style.name if r.style is not None else None
    except Exception:
        style = None
    return (
        bool(r.bold), bool(r.italic), r.underline not in (None, False), f.name, f.size, ckey,
        f.highlight_color, bool(f.strike), bool(f.subscript), bool(f.superscript), style,
    )


def dominant_run(p):
    """The text-bearing run that carries most characters, plus whether formatting is mixed."""
    runs = [r for r in p.runs if r.text]
    if not runs:
        return None, False
    weights: dict[tuple, int] = {}
    for r in runs:
        k = _fmt_key(r)
        weights[k] = weights.get(k, 0) + len(r.text)
    best = max(weights, key=weights.get)
    dom = next(r for r in runs if _fmt_key(r) == best)
    return dom, len(weights) > 1



def _text_of(els) -> str:
    """Visible text of some paragraph children. Walks the tree itself: plain lxml elements (hyperlink, sdt, smartTag...)
    have no namespace map, so an XPath with the w: prefix fails on them."""
    wt = qn("w:t")
    return "".join(t.text or "" for el in els for t in el.iter(wt))


def split_lead(par):
    """A bold/italic lead-in at the start of a paragraph ("Time.", "Weeks 1-2, Measure.") is kept as it is.
    Returns (lead_elements, body_elements): the body is everything that is humanized."""
    kids = [k for k in par._p if k.tag != qn("w:pPr")]
    dom, _ = dominant_run(par)
    lead: list = []
    if dom is not None:
        dom_key = _fmt_key(dom)
        from docx.text.run import Run

        for k in kids:
            if k.tag in (qn("w:proofErr"), qn("w:bookmarkStart"), qn("w:bookmarkEnd")):
                lead.append(k)
                continue
            if k.tag != qn("w:r"):
                break
            r = Run(k, par)
            if r.text.strip() and _fmt_key(r) == dom_key:
                break
            lead.append(k)
        while lead and lead[-1].tag != qn("w:r"):
            lead.pop()
        runs = [Run(k, par) for k in lead if k.tag == qn("w:r")]
        text = "".join(r.text for r in runs).strip()
        styled = any(r.bold or r.italic or r.underline for r in runs)
        body_words = len(" ".join(_text_of(kids[len(lead):]).split()).split())
        if not (text and styled and len(text.split()) <= 15 and body_words >= 3):
            lead = []
    body = [k for k in kids[len(lead):]]
    return lead, body


def para_text(par) -> str:
    """All visible text of a paragraph, including links, fields, content controls and smart tags (python-docx's .text misses some)."""
    return " ".join(_text_of([k for k in par._p if k.tag != qn("w:pPr")]).split())


def body_text(par) -> str:
    return " ".join(_text_of(split_lead(par)[1]).split())


# --------------------------------------------------------------------------- planning
LARGE_MIN_WORDS = 40       # a "large paragraph": at least this many words ...
LARGE_MIN_SENTENCES = 2   # ... and at least this many sentences
FRONT_RE = re.compile(
    r"^\s*(?:(?:chapter|section)\s+)?(?:\d+(?:\.\d+)*[.):]?\s*)?(?:acknowledge?ments?|declaration|dedication|copyright|certificate|"
    r"table\s+of\s+contents|contents|list\s+of\s+.+|abbreviations|glossary)\s*:?\s*$", re.I)


def find_start(paragraphs, start_pattern: str | None = None) -> int | None:
    """Index of the heading that matches `start_pattern` (only when the caller gave one), else None."""
    if not start_pattern:
        return None
    rx = re.compile(start_pattern, re.I)
    cands = []
    for i, p in enumerate(paragraphs):
        t = p.text.strip()
        if not t or len(t.split()) > 12 or is_toc(p):
            continue
        if rx.match(t):
            cands.append(i)
    if not cands:
        return None
    for i in cands:
        if is_heading_style(paragraphs[i]):
            return i
    return cands[0]


def is_large_body_paragraph(p, text: str | None = None) -> bool:
    text = " ".join((para_text(p) if text is None else text).split())
    if word_count(text) < LARGE_MIN_WORDS or len(split_sentences(text)) < LARGE_MIN_SENTENCES:
        return False
    if is_toc(p) or is_heading_style(p) or looks_like_heading(p, text) or CAPTION_RE.match(text) or _style_name(p).lower() == "caption":
        return False
    return not is_list_item(p)


def find_first_large_paragraph(paragraphs) -> int | None:
    """The beginning of the real text: the first big body paragraph that is not a cover page, contents list, caption or
    front matter such as acknowledgements. Tables are not part of doc.paragraphs, so they are never candidates."""
    in_front = False
    for i, p in enumerate(paragraphs):
        t = para_text(p)
        if not t:
            continue
        if len(t.split()) <= 8 and (is_heading_style(p) or looks_like_heading(p, t) or FRONT_RE.match(t)):
            in_front = bool(FRONT_RE.match(t))
            continue
        if in_front:
            continue
        if is_large_body_paragraph(p, t):
            return i
    return None


def build_plan(
    doc,
    *,
    max_words: int = 850,
    min_words: int = 15,
    start_pattern: str | None = None,
    stop_patterns: list[str] | None = None,
    include_lists: bool = False,
    flatten_mixed: bool = True,
    flatten_complex: bool = True,
) -> Plan:
    """Where to start: after the heading you named (start_pattern / document.start_heading) if there is one, otherwise at the
    first large paragraph. No 'Introduction' heading is needed."""
    paras = doc.paragraphs
    first = 0
    if start_pattern:
        h = find_start(paras, start_pattern)
        if h is None:
            raise PlanError(f"Could not find a heading matching '{start_pattern}'. Leave --start / document.start_heading empty to "
                            "start at the first large paragraph instead.")
        first = h + 1
        start_text = paras[h].text.strip()
    else:
        f = find_first_large_paragraph(paras)
        if f is None:
            raise PlanError("I could not find a large paragraph of body text (about 40 words, 2 sentences) in this document. "
                            "If the text starts under a heading, tell me which one (for example: start after \"Chapter 1\").")
        first = f
        start_text = " ".join(paras[f].text.split())[:70]
    stop_rx = re.compile("|".join(f"(?:{p})" for p in stop_patterns), re.I) if stop_patterns else DEFAULT_STOP_RE

    end = len(paras)
    stop_text = None
    for i in range(first, len(paras)):
        t = paras[i].text.strip()
        if t and len(t.split()) <= 6 and stop_rx.match(t) and (is_heading_style(paras[i]) or looks_like_heading(paras[i], t)):
            end, stop_text = i, t
            break

    plan = Plan(start_index=first, end_index=end, start_text=start_text, stop_text=stop_text)

    for i in range(first, end):
        p = paras[i]
        text = para_text(p)
        if not text:
            continue
        full = text
        text = body_text(p) or text
        preview = text[:70] + ("..." if len(text) > 70 else "")

        def skip(reason: str) -> None:
            plan.skipped.append((i, reason, preview))

        try:
            if is_toc(p):
                skip("toc")
            elif is_heading_style(p) or looks_like_heading(p, text):
                skip("heading")
            elif CAPTION_RE.match(text) or _style_name(p).lower() == "caption":
                skip("caption")
            elif is_list_item(p) and not include_lists:
                skip("list-item")
            elif p._p.xpath(KEEP_XPATH) or (p._p.xpath(FLATTEN_XPATH) and not flatten_complex):
                skip("complex-content (footnote/image/equation/tracked change" + ("" if flatten_complex else "/link/field") + ")")
            elif p._p.xpath(BREAKS_XPATH):
                skip("has line-breaks or tabs")
            elif word_count(text) < min_words or (word_count(text) < 8 and not text.rstrip().endswith((".", "!", "?", ")", '"', "\u201d"))):
                skip("too short (not a full sentence)")
            else:
                _, mixed = dominant_run(p)
                lead, _ = split_lead(p)
                if mixed and not lead and not flatten_mixed:
                    skip("mixed formatting (bold/italic inside)")
                else:
                    plan.paragraphs[i] = text
        except Exception as e:   # a paragraph that cannot be analysed is left alone, never a crash
            plan.paragraphs.pop(i, None)
            skip(f"unreadable ({e.__class__.__name__})")

    for i, text in plan.paragraphs.items():
        chunks = chunk_by_sentences(text, max_words)
        for k, c in enumerate(chunks):
            plan.units.append(Unit(para_index=i, part=k, parts=len(chunks), text=c))
    return plan


# --------------------------------------------------------------------------- writing back
RED = RGBColor(0xFF, 0x00, 0x00)


def _add_run(paragraph, text: str, base_rpr, red: bool):
    run = paragraph.add_run(text)
    if base_rpr is not None:
        old = run._r.rPr
        if old is not None:
            run._r.remove(old)
        run._r.insert(0, copy.deepcopy(base_rpr))
    if red:
        run.font.color.rgb = RED
    return run


def rewrite_paragraph(par, new_text: str, mark_red: bool = True) -> int:
    """Replace the humanized part of a paragraph (everything after a bold/italic lead-in), colouring changed sentences red.
    Citation fields and links inside it become plain text. Returns the number of changed sentences."""
    lead, body = split_lead(par)
    old_sents = split_sentences(" ".join(_text_of(body).split()))
    new_sents = split_sentences(new_text)
    if not new_sents:
        return 0
    dom, _ = dominant_run(par)
    base_rpr = copy.deepcopy(dom._r.rPr) if dom is not None and dom._r.rPr is not None else None

    sm = SequenceMatcher(a=[norm(s) for s in old_sents], b=[norm(s) for s in new_sents], autojunk=False)
    segments: list[tuple[str, bool]] = []
    for tag, i1, i2, j1, j2 in sm.get_opcodes():
        if tag == "equal":
            segments += [(s, False) for s in old_sents[i1:i2]]  # keep the author's exact original text
        elif tag in ("replace", "insert"):
            segments += [(s, True) for s in new_sents[j1:j2]]
        # "delete": sentence dropped by the humanizer

    for el in body:
        if el.tag in [qn(t) for t in BODY_CHILDREN]:
            par._p.remove(el)
    if lead and not _text_of(lead).endswith((" ", "\u00a0")):
        _add_run(par, " ", base_rpr, False)
    for k, (txt, changed) in enumerate(segments):
        if k:
            _add_run(par, " ", base_rpr, False)
        _add_run(par, txt, base_rpr, changed and mark_red)
    return sum(1 for _, c in segments if c)


def apply_results(doc, plan: Plan, results: dict[int, str], mark_red: bool = True) -> dict:
    """results: paragraph index -> new full paragraph text."""
    paras = doc.paragraphs
    stats = {"paragraphs_changed": 0, "sentences_changed": 0}
    for idx, new_text in results.items():
        if idx not in plan.paragraphs:
            continue
        try:
            n = rewrite_paragraph(paras[idx], new_text, mark_red)
        except Exception as e:   # one odd paragraph must never lose the whole job: it stays as the author wrote it
            stats["failed"] = stats.get("failed", 0) + 1
            stats.setdefault("failed_paragraphs", []).append((idx, f"{e.__class__.__name__}: {e}"))
            continue
        if n:
            stats["paragraphs_changed"] += 1
            stats["sentences_changed"] += n
    return stats
