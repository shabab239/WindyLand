"""A second pair of eyes: Claude reads the WHOLE humanized document in ONE chat and fine-tunes only its worst sentences.

It is told not to make drastic changes (heavy rewriting would make the text look like AI again): every edit must be a small touch-up of
one sentence. Tobi checks each edit (similar enough, numbers kept) before it goes into the Word file.
Engines: the `claude` command line tool, claude.ai in Tobi's browser (same account as the Claude app), or Tobi's own model.
The older per-sentence scoring (`check`) is kept for scripts and tests.
"""
from __future__ import annotations

import difflib
import json
import math
import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

from .text_utils import norm, split_sentences

NO_WINDOW = 0x08000000 if os.name == "nt" else 0

SYSTEM = (
    "You are a strict copy editor. Each numbered item has an ORIGINAL sentence and a REWRITE of it. Score the REWRITE from 1 to 10: "
    "10 = natural, grammatical, same meaning, same facts, numbers, names and citations; deduct for grammar mistakes, awkward or odd wording, "
    "changed meaning or lost numbers. When the score is below 7, give a corrected version of the rewrite in \"fix\" (keep the numbers and "
    "citations); otherwise leave \"fix\" empty. Reply with a JSON list only, no other text: "
    '[{"id": 1, "score": 8, "issue": "", "fix": ""}, ...]'
)


def find_claude() -> str | None:
    w = shutil.which("claude")
    if w:
        return w
    home = Path.home()
    cands = [
        home / ".local" / "bin" / "claude.exe", home / ".local" / "bin" / "claude",
        Path(os.environ.get("APPDATA", "")) / "npm" / "claude.cmd",
        Path(os.environ.get("LOCALAPPDATA", "")) / "Programs" / "claude" / "claude.exe",
        Path(os.environ.get("LOCALAPPDATA", "")) / "AnthropicClaude" / "claude.exe",
    ]
    for c in cands:
        try:
            if c.is_file():
                return str(c)
        except OSError:
            pass
    return None


class ClaudeCLI:
    name = "Claude"

    def __init__(self, exe: str, timeout: int = 300):
        self.exe, self.timeout = exe, timeout

    def ask(self, system: str, user: str, max_tokens: int = 2000) -> str:
        prompt = system + "\n\n" + user
        last = ""
        for flags in (["-p", "--output-format", "text", "--max-turns", "1"], ["-p"]):
            r = subprocess.run([self.exe, *flags], input=prompt, capture_output=True, text=True, encoding="utf-8", errors="replace",
                               timeout=self.timeout, cwd=tempfile.gettempdir(), creationflags=NO_WINDOW)
            if r.returncode == 0 and r.stdout.strip():
                return r.stdout
            last = (r.stderr or r.stdout or "").strip()[:200]
        raise RuntimeError(f"claude exited with an error: {last}")


def changed_pairs(orig: dict[int, str], new: dict[int, str]) -> list[dict]:
    pairs = []
    for idx in sorted(new):
        old_s = split_sentences(orig.get(idx, ""))
        old_n = {norm(s) for s in old_s}
        for s in split_sentences(new[idx]):
            if norm(s) in old_n:
                continue
            before = max(old_s, key=lambda o: difflib.SequenceMatcher(None, norm(o), norm(s)).ratio(), default="")
            pairs.append({"id": len(pairs) + 1, "para": idx, "before": before, "after": s})
    return pairs


def _parse(raw: str) -> list[dict]:
    m = re.search(r"\[.*\]", raw, re.S)
    if not m:
        return []
    try:
        data = json.loads(m.group(0))
    except json.JSONDecodeError:
        return []
    return [d for d in data if isinstance(d, dict) and "id" in d and "score" in d]


def check(pairs: list[dict], ask, engine: str, log=print, batch: int = 20, min_score: float = 7.0) -> dict:
    scored, errors = {}, 0
    for i in range(0, len(pairs), batch):
        chunk = pairs[i:i + batch]
        payload = json.dumps([{"id": p["id"], "original": p["before"], "rewrite": p["after"]} for p in chunk], ensure_ascii=False)
        try:
            for d in _parse(ask(SYSTEM, payload, 2500)):
                scored[int(d["id"])] = d
        except Exception as e:
            errors += 1
            log(f"  (the sentence check failed for one group: {str(e)[:160]})")
    by_id = {p["id"]: p for p in pairs}
    rows = []
    for pid, d in scored.items():
        p = by_id.get(pid)
        if not p:
            continue
        try:
            sc = max(1.0, min(10.0, float(d["score"])))
        except (TypeError, ValueError):
            continue
        rows.append({**p, "score": sc, "issue": str(d.get("issue") or "")[:200], "fix": str(d.get("fix") or "").strip()})
    overall = round(sum(r["score"] for r in rows) / len(rows) * 10) if rows else None
    weak = sorted((r for r in rows if r["score"] < min_score), key=lambda r: r["score"])
    return {"engine": engine, "overall": overall, "checked": len(rows), "total": len(pairs), "weak": weak, "errors": errors, "min_score": min_score}


def apply_fixes(per_para: dict[int, str], weak: list[dict]) -> int:
    n = 0
    for w in weak:
        fix = w.get("fix")
        if fix and w["para"] in per_para and w["after"] in per_para[w["para"]] and len(fix) > 15:
            per_para[w["para"]] = per_para[w["para"]].replace(w["after"], fix, 1)
            n += 1
    return n


def write_report(res: dict, path: Path) -> Path:
    lines = [f"Sentence check by {res['engine']}", f"Overall sentence score: {res['overall']}/100  ({res['checked']} of {res['total']} rewritten sentences scored)", ""]
    if res["weak"]:
        lines.append(f"Sentences scoring under {res['min_score']:g}/10:")
        for w in res["weak"]:
            lines += ["", f"- {w['score']:g}/10  {w['issue']}", f"  was:  {w['before']}", f"  now:  {w['after']}"]
            if w["fix"]:
                lines.append(f"  idea: {w['fix']}")
    else:
        lines.append("No weak sentences.")
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return path


# --------------------------------------------------------------------------- whole-document review (one chat)
REVIEW_SYSTEM = (
    "You are a careful copy editor. Below is an essay/report whose paragraphs were just rewritten by a humanizer so that AI detectors score it as "
    "human-written. Paragraphs are labelled [P<number>].\n"
    "Your job: FINE-TUNE ONLY THE WORST SENTENCES (grammar mistakes, clumsy or odd wording, a changed meaning, lost numbers, names or citations).\n"
    "Rules: do NOT rewrite the document and do NOT make drastic changes - heavy rewriting would make an AI detector flag the text again. "
    "Each edit must be the smallest touch-up that fixes the problem: change at most a few words (never more than a third of the sentence), keep the sentence structure, length, tone and every number, name and citation exactly. "
    "Never rewrite, reorder or merge sentences, never add new ideas, and never touch the reference list. "
    "Leave every sentence that is acceptable exactly as it is. Make at most {max_edits} edits.\n"
    'Reply with JSON only, in this shape: {{"overall": <your quality score for the whole document, 0-100>, '
    '"edits": [{{"before": "<the exact sentence copied from the text>", "after": "<the fine-tuned sentence>", "why": "<a few words>"}}]}}. '
    'Use an empty list for "edits" when nothing needs fixing.'
)


FLAG_SYSTEM = (
    "You are a careful copy editor. Below is an essay/report whose paragraphs were just rewritten by a humanizer so that AI detectors score it as "
    "human-written. Paragraphs are labelled [P<number>].\n"
    "Your job: point out ONLY THE WORST SENTENCES - the ones that sound unnatural or clumsy, are ungrammatical, say something different from what the "
    "paragraph clearly means, or lost a number, name or citation. Do NOT rewrite anything and do NOT suggest new wording: the program sends the sentences you "
    "name back through the humanizer and picks a better version itself. Copy each sentence EXACTLY as it appears. Name at most {max_edits} sentences, and none "
    "if the text is fine. Never name anything from the reference list.\n"
    'Reply with JSON only, in this shape: {{"overall": <your quality score for the whole document, 0-100>, '
    '"weak": [{{"sentence": "<the exact sentence copied from the text>", "why": "<a few words>"}}]}}.'
)


def doc_text(paras: dict[int, str]) -> str:
    return "\n\n".join(f"[P{i}] {t}" for i, t in sorted(paras.items()))


def _last_review_obj(raw: str) -> dict | None:
    dec = json.JSONDecoder()
    best = None
    for m in re.finditer(r"\{", raw):
        try:
            obj, _ = dec.raw_decode(raw[m.start():])
        except ValueError:
            continue
        if isinstance(obj, dict) and (isinstance(obj.get("edits"), list) or isinstance(obj.get("weak"), list)):
            best = obj
    return best


def _digits(t: str) -> list[str]:
    return sorted(re.findall(r"\d+(?:[.,]\d+)?", t))


def edit_ok(before: str, after: str, min_similarity: float = 0.75) -> str | None:
    """None when the edit is a small, safe touch-up; otherwise the reason it is refused."""
    b, a = norm(before), norm(after)
    if not a or a == b:
        return "no change"
    if _digits(b) != _digits(a):
        return "numbers changed"
    ratio = len(a) / max(1, len(b))
    if not 0.6 <= ratio <= 1.6:
        return "length changed too much"
    if difflib.SequenceMatcher(None, b.lower(), a.lower()).ratio() < min_similarity:
        return "too drastic"
    bw, aw = b.lower().split(), a.lower().split()
    if difflib.SequenceMatcher(None, bw, aw, autojunk=False).ratio() < 0.65:   # more than about a third of the words changed
        return "too drastic"
    return None


def apply_edits(per_para: dict[int, str], edits: list[dict], max_edits: int, min_similarity: float = 0.75) -> tuple[list[dict], list[dict]]:
    applied, rejected = [], []
    for e in edits:
        before, after = str(e.get("before") or "").strip(), str(e.get("after") or "").strip()
        if len(applied) >= max_edits:
            rejected.append({**e, "reason": "over the edit limit"})
            continue
        hit = next((i for i, t in per_para.items() if before and before in t), None)
        if hit is None:  # Claude may have changed a quote or a space: find the closest sentence
            best, best_r = None, 0.0
            for i, t in per_para.items():
                for sent in split_sentences(t):
                    r = difflib.SequenceMatcher(None, norm(sent).lower(), norm(before).lower()).ratio()
                    if r > best_r:
                        best, best_r = (i, sent), r
            if best and best_r >= 0.92:
                hit, before = best[0], best[1]
        if hit is None:
            rejected.append({**e, "reason": "sentence not found"})
            continue
        why = edit_ok(before, after, min_similarity)
        if why:
            rejected.append({**e, "reason": why})
            continue
        per_para[hit] = per_para[hit].replace(before, after, 1)
        applied.append({"para": hit, "before": before, "after": after, "why": str(e.get("why") or "")[:120]})
    return applied, rejected


def review_document(paras: dict[int, str], ask, engine: str, log=print, chunk_words: int = 0, max_edit_share: float = 0.08,
                    min_similarity: float = 0.75, mode: str = "flag") -> dict:
    """One conversation: the whole document goes to Claude (``chunk_words`` > 0 splits it for small local models).
    Returns {engine, overall, edits, rejected, sentences}; the edits are NOT applied yet (see apply_edits)."""
    sentences = sum(len(split_sentences(t)) for t in paras.values())
    max_edits = max(3, math.ceil(sentences * max_edit_share))
    groups: list[dict[int, str]] = []
    if chunk_words and sum(len(t.split()) for t in paras.values()) > chunk_words:
        cur, w = {}, 0
        for i, t in sorted(paras.items()):
            if cur and w + len(t.split()) > chunk_words:
                groups.append(cur)
                cur, w = {}, 0
            cur[i] = t
            w += len(t.split())
        if cur:
            groups.append(cur)
    else:
        groups = [paras]
    edits, scores, errors = [], [], 0
    for g in groups:
        per_group = max(2, math.ceil(max_edits * len(g) / max(1, len(paras)))) if len(groups) > 1 else max_edits
        try:
            raw = ask((FLAG_SYSTEM if mode == "flag" else REVIEW_SYSTEM).format(max_edits=per_group), doc_text(g), 4000)
        except Exception as e:
            errors += 1
            log(f"  (the review failed: {str(e)[:160]})")
            continue
        obj = _last_review_obj(raw or "")
        if not obj:
            errors += 1
            log("  (the review answer could not be read)")
            continue
        edits += [e for e in obj.get("edits") or [] if isinstance(e, dict)]
        edits += [{"before": str(w.get("sentence") or w.get("before") or ""), "after": "", "why": str(w.get("why") or "")}
                  for w in obj.get("weak") or [] if isinstance(w, dict)]
        try:
            scores.append(max(0.0, min(100.0, float(obj.get("overall")))))
        except (TypeError, ValueError):
            pass
    overall = round(sum(scores) / len(scores)) if scores else None
    return {"engine": engine, "overall": overall, "edits": edits, "max_edits": max_edits, "sentences": sentences, "errors": errors,
            "applied": [], "rejected": [], "min_similarity": min_similarity, "answered": errors < len(groups), "mode": mode}


def find_sentences(per_para: dict[int, str], texts: list[str], min_ratio: float = 0.9, limit: int = 0) -> list[tuple[int, str, str]]:
    """Locate sentences (as copied by Claude or read from a web page) in the document: [(paragraph index, sentence as in the document, why)]."""
    pool = [(i, s) for i, t in per_para.items() for s in split_sentences(t)]
    found: list[tuple[int, str, str]] = []
    seen = set()
    for item in texts:
        why = ""
        if isinstance(item, tuple):
            item, why = item
        f = norm(item).lower()
        if len(f) < 8:
            continue
        best, best_r = None, 0.0
        for i, snt in pool:
            n = norm(snt).lower()
            r = 1.0 if (f == n or (len(f) >= 25 and f in n) or (len(n) >= 25 and n in f)) else difflib.SequenceMatcher(None, f, n).ratio()
            if r > best_r:
                best, best_r = (i, snt), r
                if r >= 1.0:
                    break
        if best and best_r >= min_ratio and best not in seen:
            seen.add(best)
            found.append((best[0], best[1], why))
            if limit and len(found) >= limit:
                break
    return found


def write_review_report(res: dict, path: Path) -> Path:
    lines = [f"Document review by {res['engine']}", f"Overall score: {res['overall']}/100"]
    if res.get("mode") == "flag":
        lines += [f"{len(res.get('flagged', []))} weak sentence(s) were sent back through stealthwriter to pick better ones (nothing was reworded by Tobi or Claude).", ""]
        for _, snt, why in res.get("flagged", []):
            lines += [f"- {why}", f"  {snt}"]
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        return path
    lines += [f"{len(res['applied'])} sentence(s) fine-tuned, {len(res['rejected'])} suggestion(s) refused as too drastic or unsafe.", ""]
    for a in res["applied"]:
        lines += [f"- {a['why']}", f"  was:  {a['before']}", f"  now:  {a['after']}"]
    if res["rejected"]:
        lines += ["", "Refused:"]
        for r in res["rejected"]:
            lines += [f"- ({r.get('reason')}) {r.get('before', '')}"]
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return path
