"""Small text helpers shared by the Word side and the browser side."""
from __future__ import annotations

import re

_ABBREVS = [
    "e.g.", "i.e.", "etc.", "et al.", "vs.", "fig.", "figs.", "no.", "dr.", "mr.", "mrs.", "ms.",
    "prof.", "inc.", "ltd.", "approx.", "cf.", "st.", "eq.", "sec.", "ref.", "vol.", "pp.", "u.s.", "u.k.",
]
_PROT = "\u0001"
_ABBR_RE = re.compile(
    r"(?<![A-Za-z])(" + "|".join(re.escape(a) for a in sorted(_ABBREVS, key=len, reverse=True)) + ")",
    re.I,
)
_DECIMAL_RE = re.compile(r"(?<=\d)\.(?=\d)")
_INITIAL_RE = re.compile(r"\b([A-Z])\.(?=\s+[A-Z])")
_ELLIPSIS_RE = re.compile(r"\.{2,}")
# A boundary is whitespace that follows . ! ? (optionally followed by closing quotes/brackets)
# and precedes something that looks like the start of a sentence.
_BOUNDARY = re.compile(
    r"(?:(?<=[.!?])|(?<=[.!?][\"'”’)\]]))\s+(?=[\"'“‘(\[]*[A-Z0-9])"
)
_TRANSLATE = str.maketrans(
    {
        "‘": "'", "’": "'", "“": '"', "”": '"',
        " ": " ", "–": "-", "—": "-",
    }
)


def _protect(t: str) -> str:
    t = _ABBR_RE.sub(lambda m: m.group(0).replace(".", _PROT), t)
    t = _DECIMAL_RE.sub(_PROT, t)
    t = _INITIAL_RE.sub(lambda m: m.group(1) + _PROT, t)
    t = _ELLIPSIS_RE.sub(lambda m: _PROT * len(m.group(0)), t)
    return t


def split_sentences(text: str) -> list[str]:
    """Split a paragraph into sentences (rule based, handles common abbreviations)."""
    t = " ".join(text.split())
    if not t:
        return []
    parts = _BOUNDARY.split(_protect(t))
    return [p.replace(_PROT, ".").strip() for p in parts if p.strip()]


def norm(s: str) -> str:
    """Normalise quotes, dashes and whitespace so cosmetic changes do not count as edits."""
    return " ".join(s.translate(_TRANSLATE).split())


def word_count(text: str) -> int:
    return len(text.split())


def chunk_by_sentences(text: str, max_words: int) -> list[str]:
    """Cut a very long paragraph into pieces of at most max_words, on sentence boundaries."""
    if word_count(text) <= max_words:
        return [text]
    chunks: list[str] = []
    cur: list[str] = []
    cur_words = 0
    for s in split_sentences(text):
        w = word_count(s)
        if cur and cur_words + w > max_words:
            chunks.append(" ".join(cur))
            cur, cur_words = [], 0
        cur.append(s)
        cur_words += w
    if cur:
        chunks.append(" ".join(cur))
    return chunks


def split_paragraphs(text: str) -> list[str]:
    """Split pasted/copied text into paragraphs (blank line separated)."""
    return [p.strip() for p in re.split(r"\n\s*\n", text.strip()) if p.strip()]
