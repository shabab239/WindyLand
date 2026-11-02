"""Chooses the best rewrite of a sentence from the options stealthwriter.ai offers.

Two judges:
  * LLMJudge        - asks the agent's language model (Claude, GPT, Gemini, ...). Best quality.
  * HeuristicJudge  - offline rules (meaning overlap, numbers kept, grammar red flags). Free, weaker.
If no model is configured the agent falls back to the heuristic judge automatically.
"""
from __future__ import annotations

import json
import math
import os
import re
from dataclasses import dataclass, field

from .text_utils import norm


@dataclass
class Candidate:
    idx: int  # position in the dialog (0 = Original when the dialog lists it first)
    label: str  # "Original", "Option 1", ...
    text: str
    human: bool | None = None  # the site's dot: green = reads as human-written, red = still flagged as AI, None = unknown


@dataclass
class JudgeRequest:
    original: str  # the sentence as it was in the Word file
    candidates: list[Candidate]
    before: str = ""  # previous sentence (context)
    after: str = ""  # next sentence (context)
    allow_original: bool = False  # may the unchanged original be chosen?
    force: bool = False  # no more New alternatives are allowed: decide now
    must_replace: bool = False  # the sentence is red (flagged as AI): keeping it is not acceptable, take the best option even if only so-so


@dataclass
class Decision:
    idx: int | None  # Candidate.idx, or None = "none is good, ask for new alternatives"
    reason: str = ""


_STOP = set(
    "a an the and or but if of to in on at by for with from as is are was were be been being it its this that these those "
    "which who whom than then so such not no also can could may might should would will shall do does did has have had "
    "into over under about between during after before more most other some any each both their there here they them".split()
)
_BAD_ENDINGS = {"and", "or", "but", "the", "a", "an", "of", "to", "in", "on", "with", "for", "by", "that", "which", "as"}
_AI_ISMS = ["delve", "tapestry", "testament to", "multifaceted", "it is important to note", "in the realm of", "navigate the complexities"]


def _tokens(s: str) -> set[str]:
    return {w for w in re.findall(r"[a-z0-9]+", s.lower()) if len(w) > 2 and w not in _STOP}


def _must_keep(s: str) -> set[str]:
    """Numbers, acronyms and citations that must survive a rewrite."""
    keep = set(re.findall(r"\b\d[\d.,%]*\b", s))
    keep |= set(re.findall(r"\b[A-Z]{2,}\b", s))
    keep |= {re.sub(r"\s+", "", m) for m in re.findall(r"\([^()]*\d{4}[^()]*\)", s)}
    return keep


def _proper_nouns(s: str) -> set[str]:
    """Capitalised words in the middle of a sentence (author names, places, organisations): a rewrite must keep them."""
    out = set()
    prev_end = True
    for m in re.finditer(r"[A-Za-z][A-Za-z'\u2019\-]*|[.!?]", s):
        w = m.group(0)
        if w in ".!?":
            prev_end = True
            continue
        if not prev_end and len(w) > 2 and w[0].isupper() and not w.isupper():
            out.add(w.lower())
        prev_end = False
    return out


def hard_problems(original: str, cand: str) -> list[str]:
    """Things that make a rewrite unusable whatever its wording: lost numbers, citations, acronyms or names, wrong length, broken brackets."""
    o, c = norm(original), norm(cand)
    probs: list[str] = []
    cc = c.replace(" ", "").lower()
    missing = [k for k in _must_keep(original) if k.replace(" ", "").lower() not in cc]
    if missing:
        probs.append(f"drops {missing[:2]}")
    names = [n for n in _proper_nouns(original) if n not in c.lower()]
    if names:
        probs.append(f"drops name {names[:2]}")
    ratio = len(c) / max(1, len(o))
    if ratio < 0.55 or ratio > 1.7:
        probs.append(f"length x{ratio:.2f}")
    if c.count("(") != c.count(")") or c.count('"') % 2:
        probs.append("unbalanced brackets/quotes")
    if not c or "\ufffd" in cand:
        probs.append("empty or garbled")
    if o[:1].isupper() and c[:1].islower():
        probs.append("starts with a small letter")
    if o and o[-1] in ".!?" and c and c[-1] not in ".!?\"')\u201d":
        probs.append("no full stop at the end")
    if re.search(r"\b(\w{3,})\s+\1\b", c, re.I):
        probs.append("a word is repeated")
    if re.search(r"\s[,.;:!?]|\.\.(?!\.)|,\s*,", cand):
        probs.append("odd punctuation")
    return probs


def dot_bonus(c: "Candidate") -> float:
    """stealthwriter marks every rewrite with a coloured dot: prefer the ones that read as human."""
    return 1.4 if c.human is True else (-1.8 if c.human is False else 0.0)


class HeuristicJudge:
    name = "heuristic"

    def score(self, original: str, cand: str) -> tuple[float, str]:
        o, c = norm(original), norm(cand)
        a, b = _tokens(o), _tokens(c)
        sim = len(a & b) / max(1, len(a | b))
        pen = 0.0
        notes: list[str] = []

        missing = [k for k in _must_keep(original) if k.replace(" ", "") not in c.replace(" ", "")]
        if missing:
            pen += 3 * min(2, len(missing))
            notes.append(f"drops {missing[:2]}")
        ratio = max(0.01, len(c) / max(1, len(o)))
        if ratio < 0.6 or ratio > 1.6:
            pen += 2 * abs(math.log(ratio))
            notes.append(f"length x{ratio:.2f}")
        if re.search(r"\s{2,}|\s[,.;:!?]|\b(\w+)\s+\1\b", cand, re.I):
            pen += 1.5
            notes.append("spacing/repeat")
        if o[:1].isupper() and c[:1].islower():
            pen += 1.0
            notes.append("starts lowercase")
        if c and c[-1] not in '.!?"\')”':
            pen += 1.0
            notes.append("no end punctuation")
        if c.count("(") != c.count(")") or c.count('"') % 2:
            pen += 1.0
            notes.append("unbalanced brackets/quotes")
        last = re.findall(r"[A-Za-z]+", c)[-1:] or [""]
        if last[0].lower() in _BAD_ENDINGS:
            pen += 1.5
            notes.append("dangling ending")
        if "�" in cand:
            pen += 3
        for phrase in _AI_ISMS:
            if phrase in c.lower() and phrase not in o.lower():
                pen += 0.7
                notes.append(f"AI-ism '{phrase}'")
        s = 4 * sim - pen
        if norm(original) == norm(cand):
            notes.append("unchanged")
        return s, f"sim={sim:.2f} " + ", ".join(notes)

    def choose(self, req: JudgeRequest) -> Decision:
        best, best_s, best_note = None, -1e9, ""
        pool = [c for c in req.candidates if not hard_problems(req.original, c.text)]
        if not pool and not req.force:
            return Decision(None, "every option loses a number, citation or name")
        for c in (pool or req.candidates):
            same = norm(c.text) == norm(req.original) or c.label.lower() == "original"
            if same and not req.allow_original:
                continue
            s, note = self.score(req.original, c.text)
            s += dot_bonus(c)
            if s > best_s:
                best, best_s, best_note = c, s, note
        if best is None:
            return Decision(None, "no eligible candidates")
        if best_s < 1.0 and not req.force:
            return Decision(None, f"best score {best_s:.1f} too low ({best_note})")
        if best_s < 0.0 and req.force and not req.must_replace:
            return Decision(None, f"keeping the sentence: best option scores only {best_s:.1f} ({best_note})")
        return Decision(best.idx, f"{best.label}: score {best_s:.1f} ({best_note})")


def _json_objects(text: str) -> list[dict]:
    dec = json.JSONDecoder()
    out, pos = [], 0
    for m in re.finditer(r"\{", text):
        if m.start() < pos:
            continue
        try:
            obj, end = dec.raw_decode(text[m.start():])
        except ValueError:
            continue
        if isinstance(obj, dict):
            out.append(obj)
            pos = m.start() + end
    return out


class ChainLLM:
    """Tries the engines in order (for example Claude's command line tool, then Tobi's own model); one that keeps failing is dropped."""

    def __init__(self, engines: list[tuple[str, object]], log=print):
        self.engines, self.log = engines, log
        self.fails: dict[str, int] = {}

    @property
    def names(self) -> str:
        return " > ".join(n for n, _ in self.engines)

    def ask(self, system: str, user: str, max_tokens: int = 0) -> str:
        last: Exception | None = None
        for name, eng in self.engines:
            if self.fails.get(name, 0) >= 2:
                continue
            try:
                out = eng.ask(system, user, max_tokens)
                self.fails[name] = 0
                return out
            except Exception as e:
                last = e
                self.fails[name] = self.fails.get(name, 0) + 1
                if self.fails[name] == 2:
                    self.log(f"  ({name} is not answering ({str(e)[:100]}); using the next one)")
        raise last or RuntimeError("no AI engine available")


class LLMJudge:
    """Asks an AI model to MARK every rewrite (1-10) and takes the best one only if it is really good.

    A rewrite is accepted only when its mark reaches ``accept`` (8 by default). Otherwise the caller presses New alternatives and asks again;
    when the tries run out a green (human-looking) sentence stays as it was, and a red one gets the best of what there is.
    Rewrites that lose a number, citation, acronym or name are never shown to the model at all.
    """

    name = "llm"

    SYSTEM = (
        "You are a strict, experienced academic copy editor. You get ONE original sentence, the sentences before and after it, and several numbered rewrites "
        "produced by a humanizer tool. Mark EACH rewrite from 1 to 10 as a sentence in this essay:\n"
        "- 10 = reads exactly like a careful native-English student wrote it; fully grammatical, smooth, natural word choice; same meaning, facts, hedging, numbers, names and citations; fits the formal academic tone and flows from the sentence before.\n"
        "- 7-9 = good but with a small flaw (slightly stiff, a bit wordy, a weaker word).\n"
        "- 4-6 = awkward, unnatural or unusual phrasing, odd word order, strange synonyms, filler, or a slightly shifted meaning.\n"
        "- 1-3 = ungrammatical, meaning changed or lost, new claims, fragments, or sounds machine-made or clumsy.\n"
        "Be harsh: most humanizer output is clumsy. Do not reward a rewrite just for being different. Penalise: repeated sentence openings, unnatural inversions, "
        "needless 'It is...that' constructions, vague words replacing precise ones, over-formal or odd vocabulary (e.g. 'utilise', 'albeit', 'thereby' used wrongly), "
        "and any AI-sounding filler ('delve', 'tapestry', 'it is important to note', 'in the realm of').\n"
        "A rewrite with flagged_as_ai=true still reads as machine-written to the detector; mark it normally for quality, the program accounts for it.\n"
        'Reply with JSON only, no other text: {"scores": {"<number>": <mark>, ...}} with one entry per rewrite.'
    )

    def __init__(self, llm, fallback: HeuristicJudge | None = None, log=print, mode: str = "best", accept: float = 8.0, accept_forced: float = 6.0):
        self.llm = llm
        self.fallback = fallback or HeuristicJudge()
        self.log = log
        self.mode = mode
        self.accept, self.accept_forced = float(accept), float(accept_forced)
        self.failures = 0
        self.calls = 0
        self.skipped = 0

    def choose(self, req: JudgeRequest) -> Decision:
        cands = [
            c for c in req.candidates
            if req.allow_original or (c.label.lower() != "original" and norm(c.text) != norm(req.original))
        ]
        if not cands:
            return Decision(None, "no eligible candidates")
        safe = [c for c in cands if not hard_problems(req.original, c.text)]
        if not safe and not req.force:
            return Decision(None, "every option loses a number, citation or name, or is broken")
        cands = safe or cands
        scored = sorted(((self.fallback.score(req.original, c.text)[0] + dot_bonus(c), c) for c in cands), key=lambda x: -x[0])
        cands = [c for _, c in scored[:6]]
        payload = {
            "original_sentence": req.original,
            "sentence_before": req.before[-350:],
            "sentence_after": req.after[:350],
            "rewrites": [{"number": c.idx, "text": c.text, **({"flagged_as_ai": True} if c.human is False else {})} for c in cands],
        }
        try:
            self.calls += 1
            raw = self.llm.ask(self.SYSTEM, json.dumps(payload, ensure_ascii=False), max_tokens=160)
            valid = {c.idx: c for c in cands}
            marks: dict[int, float] = {}
            for obj in _json_objects(raw):
                sc = obj.get("scores") if isinstance(obj.get("scores"), dict) else obj
                for k, v in sc.items():
                    try:
                        if int(k) in valid:
                            marks[int(k)] = max(1.0, min(10.0, float(v)))
                    except (TypeError, ValueError):
                        continue
                if marks:
                    break
                if obj.get("best") is not None and int(obj["best"]) in valid:   # an older style answer
                    marks[int(obj["best"])] = self.accept
                    break
            if not marks:
                n = re.search(r"\b(\d+)\b", raw)
                if n and int(n.group(1)) in valid and "null" not in raw.lower():
                    marks[int(n.group(1))] = self.accept
            if not marks:
                raise ValueError("the answer had no marks")
            ranked = sorted(marks.items(), key=lambda kv: -(kv[1] + (0.3 if valid[kv[0]].human else 0)))
            idx, mark = ranked[0]
            need = self.accept_forced if req.force else self.accept
            if mark >= need or (req.force and req.must_replace):
                return Decision(idx, f"AI mark {mark:g}/10")
            if req.force:
                return Decision(None, f"kept as it was: best option marked only {mark:g}/10")
            return Decision(None, f"best option marked only {mark:g}/10 (need {self.accept:g})")
        except Exception as e:  # network, auth, parsing...
            self.failures += 1
            if self.failures <= 3:
                self.log(f"  (AI judge failed: {e}; using the offline rules for this sentence)")
            return self.fallback.choose(req)


def make_judge(cfg: dict, log=print, llm=None):
    kind = (cfg.get("type") or "llm").lower()
    heur = HeuristicJudge()
    if kind == "heuristic" or llm is None:
        return heur
    return LLMJudge(llm, heur, log, mode="best", accept=float(cfg.get("accept_score", 8)), accept_forced=float(cfg.get("accept_score_last_try", 6)))
