"""Humanize a Word file through stealthwriter.ai (the flow the agent runs for "Humanize this file").

run_job() reads the .docx, picks the body paragraphs from the first large paragraph on, sends them in batches to
stealthwriter.ai through the user's own browser, lets the AI pick the best sentence alternatives, writes the
result back and colours every changed sentence red.
"""
from __future__ import annotations

import copy
import hashlib
import json
import os
import re
import shutil
import sys
import time
from pathlib import Path

from docx import Document

from .docx_tools import PlanError, apply_results, build_plan
from .judge import make_judge
from . import stop as _stop

try:
    from ..llm import Cancelled
except Exception:  # pragma: no cover - the humanizer used on its own
    class Cancelled(Exception):
        pass


def _fmt(sec: float) -> str:
    sec = int(sec)
    return f"{sec // 60}m {sec % 60:02d}s" if sec >= 60 else f"{sec}s"

DEFAULTS = {
    "site": {
        "url": "https://www.stealthwriter.ai/dashboard/humanizer",
        "level": 10,
        "google_account_email": "",
        "humanize_timeout_seconds": 240,
        "target_score": 90,       # press Rehumanize until the Detection Score says at least this % human
        "max_rehumanize": 8,
    },
    "browser": {
        "path": "",
        "choice": "auto",       # auto = your default browser | brave | chrome | edge | vivaldi | opera | chromium
        "debug_port": 9222,
        "profile_dir": "~/.humanizer_agent/browser_profile",
        "login_timeout_seconds": 300,
        "human_delay": [0.03, 0.12],
        "close_when_done": False,
    },
    "document": {
        "start_heading": "",
        "stop_headings": [],
        "max_words_per_batch": 900,
        "min_words_per_batch": 800,      # combine sections until a paste has at least this many words
        "min_words_per_paragraph": 4,       # every real sentence counts, also short ones
        "include_lists": True,             # numbered / bulleted items are humanized too (their numbers stay)
        "flatten_mixed_formatting": True,    # bold/italic inside a paragraph: humanize it anyway (a bold lead-in such as "Time." is kept)
        "flatten_complex_content": True,    # citation fields / links inside a paragraph become plain text
        "mark_changed_red": True,
        "output": "in_place",  # in_place (a backup copy is kept) | copy (writes <name>_humanized.docx)
        "open_in_word_when_done": True,
    },
    "review": {
        "sentences": "all",  # all | red | green
        "max_new_alternatives": 3,
        "max_new_alternatives_red": 6,   # a RED sentence must get a green option: New alternatives up to this many times
        "red_passes": 2,                 # then go over anything still red again
        "allow_original_when_green": False,
        "sentence_selector": "",
    },
    "judge": {"type": "llm", "engine": "auto",      # auto: Claude (command line tool) if installed, else Tobi's model; cli | tobi
              "accept_score": 8, "accept_score_last_try": 6},   # a rewrite is only taken when the AI marks it this good (1-10)
    # a second opinion on the rewritten sentences by Claude (the `claude` tool installed on this PC), or Tobi's own model
    "claude_check": {"enabled": True, "engine": "auto", "mode": "flag", "min_similarity": 0.75, "max_edit_share": 0.1,   # engine: auto | cli | web | tobi
                     "web_url": "https://claude.ai/new", "chatgpt": True, "chatgpt_url": "https://chatgpt.com/", "login_timeout_seconds": 600, "web_answer_seconds": 90},
    # at the end: upload the finished file to academi.cx and download its AI score report (sign in there once)
    "academi": {"enabled": True, "url": "https://academi.cx/dashboard", "upload_url": "https://academi.cx/",
                "login_timeout_seconds": 600, "score_timeout_seconds": 240, "score_rechecks": 3, "score_recheck_seconds": 90,
                "max_rounds": 3, "target_ai_percent": 0},   # while above the target: redo the highlighted sentences on stealthwriter and check again
    "debug_dir": "debug",
}


def _document_review(plan, per_para, ccfg, llm, log, cfg=None, notify=None):
    """The whole humanized document goes to Claude in ONE chat; Claude fine-tunes only its worst sentences (small edits only).
    The accepted edits are written into ``per_para``. Returns the result dict, or None when nothing could review.
    Engines, in the order `auto` tries them: the `claude` command-line tool, claude.ai in the browser (your own account), Tobi's model."""
    from . import claude_check as cc

    if not per_para:
        return None
    engine = (ccfg.get("engine") or "auto").lower()
    mode = (ccfg.get("mode") or "flag").lower()   # flag: Claude only NAMES weak sentences, stealthwriter picks the better version | apply: Claude's small edits are written in
    sim = float(ccfg.get("min_similarity", 0.75))
    share = float(ccfg.get("max_edit_share", 0.1))
    words = sum(len(t.split()) for t in per_para.values())
    res = None

    def good(r):
        return r is not None and r.get("answered")

    exe = cc.find_claude() if engine in ("auto", "cli", "claude") else None
    if exe:
        name = "Claude (command line tool on this PC)"
        log(f"Giving the whole document ({words} words) to {name} in one go ...")
        res = cc.review_document(per_para, cc.ClaudeCLI(exe, timeout=900).ask, name, log, max_edit_share=share, min_similarity=sim, mode=mode)
    if not good(res) and engine in ("auto", "web", "claude") and cfg is not None:
        name = "Claude (claude.ai, your account)"
        log(f"Giving the whole document ({words} words) to {name} in one chat ...")
        try:
            from .claude_web import ClaudeWebClient

            with ClaudeWebClient(cfg, log, notify) as cw:
                res = cc.review_document(per_para, cw.ask, name, log, max_edit_share=share, min_similarity=sim, mode=mode)
        except Exception as e:
            log(f"  claude.ai did not work ({str(e)[:260]}).")
            res = None
    if not good(res) and engine in ("auto", "web", "claude") and cfg is not None and ccfg.get("chatgpt", True):
        name = "ChatGPT (chatgpt.com)"
        log(f"Claude did not answer; asking {name} instead ...")
        try:
            from .claude_web import ChatGPTWebClient

            with ChatGPTWebClient(cfg, log, notify) as gw:
                res = cc.review_document(per_para, gw.ask, name, log, max_edit_share=share, min_similarity=sim, mode=mode)
        except Exception as e:
            log(f"  chatgpt.com did not work ({str(e)[:200]}).")
            res = None
    if not good(res) and llm is not None and engine in ("auto", "web", "cli", "tobi", "claude"):
        label = getattr(llm, "name", "") or getattr(llm, "label", "") or "ChatGPT / your AI provider"
        name = f"Tobi's AI model ({label})"
        if engine != "tobi":
            log("  Claude did not answer, so the review is done by " + name + " instead.")
        log(f"Reviewing the document with {name} ...")
        res = cc.review_document(per_para, llm.ask, name, log, chunk_words=int(ccfg.get("tobi_chunk_words", 1200)),
                                 max_edit_share=share, min_similarity=sim, mode=mode)
    if not good(res):
        log("The document review gave no answer; going on without it.")
        return None
    if mode == "flag":
        res["flagged"] = cc.find_sentences(per_para, [(e["before"], e.get("why", "")) for e in res["edits"]], limit=res["max_edits"])
        log(f"Review by {res['engine']}: overall {res['overall']}/100; {len(res['flagged'])} weak sentence(s) named (Claude does not reword anything).")
    else:
        res["applied"], res["rejected"] = cc.apply_edits(per_para, res["edits"], res["max_edits"], sim)
        log(f"Review by {res['engine']}: overall {res['overall']}/100; {len(res['applied'])} sentence(s) fine-tuned, "
            f"{len(res['rejected'])} suggestion(s) refused (too drastic or unsafe).")
    res["report"] = None
    return res


def _judge_llm(cfg: dict, llm, log):
    """The AI that marks the sentence options: Claude's command line tool when it is installed (best), then Tobi's own model."""
    from . import claude_check as cc
    from .judge import ChainLLM

    engine = (cfg.get("judge", {}).get("engine") or "auto").lower()
    engines: list[tuple[str, object]] = []
    if engine in ("auto", "cli", "claude"):
        exe = cc.find_claude()
        if exe:
            engines.append(("Claude (command line tool)", cc.ClaudeCLI(exe, timeout=150)))
    if llm is not None and engine in ("auto", "tobi", "llm", "web"):
        engines.append((f"your AI model ({getattr(llm, 'name', '') or 'Tobi'})", llm))
    if not engines:
        return llm
    chain = ChainLLM(engines, log)
    log(f"Sentences are chosen by: {chain.names}")
    return chain


def _refine(plan, per_para, found, cfg, judge, factory, log, label, max_words):
    """Send sentences back through stealthwriter and take the best option it offers for each (nothing is reworded by Tobi).
    ``found`` = [(paragraph index, sentence as in the document, why)]. Returns how many sentences were replaced."""
    from .stealth_client import LimitReached, MappingError, SiteError
    from .text_utils import norm, split_sentences, word_count

    items = [(i, snt) for i, snt, _ in found if i in per_para]
    if not items:
        return 0
    groups, cur, words = [], [], 0
    for it in items:
        w = word_count(it[1])
        if cur and words + w > max_words:
            groups.append(cur)
            cur, words = [], 0
        cur.append(it)
        words += w
    groups.append(cur)
    log(f"Sending {len(items)} sentence(s) back through stealthwriter to choose better versions ...")
    replaced = 0
    with factory(cfg, judge, log) as client:
        for g, group in enumerate(groups, 1):
            try:
                parts = client.humanize_batch([t for _, t in group], label=f"{label}{g}")
            except LimitReached as e:
                log(f"!! {e}")
                break
            except (MappingError, SiteError) as e:
                log(f"!! could not redo {len(group)} sentence(s): {e}")
                continue
            for (idx, old), new in zip(group, parts):
                new = " ".join(new.split())
                if not new or norm(new) == norm(old):
                    continue
                sents = split_sentences(per_para[idx])
                for k, snt in enumerate(sents):
                    if norm(snt) == norm(old):
                        sents[k] = new
                        per_para[idx] = " ".join(sents)
                        replaced += 1
                        break
            try:
                client.review_log = []
            except Exception:
                pass
    log(f"  {replaced} of {len(items)} sentence(s) replaced.")
    return replaced


def _academi_loop(cfg, plan, per_para, save, out_getter, judge, factory, log, notify, max_words, ask=None):
    """Check on academi.cx; while the AI score is above the target, take the highlighted sentences (blue = high, purple = medium),
    send them back through stealthwriter, save the Word file and check again. Returns (last result, list of rounds)."""
    from . import claude_check as cc

    acfg = cfg.get("academi", {})
    max_rounds = int(acfg.get("max_rounds", 3))
    target = float(acfg.get("target_ai_percent", 0))
    rounds: list[dict] = []
    last: dict = {}
    prev_report = None
    for rnd in range(1, max_rounds + 2):
        out = out_getter()
        last = _academi_step(cfg, out, log, notify)
        entry = {"round": rnd, "ok": last.get("ok"), "ai_percent": last.get("ai_percent"), "error": last.get("error")}
        rounds.append(entry)
        if not last.get("ok"):
            break
        if prev_report and last.get("report") and prev_report != last["report"]:
            try:
                Path(prev_report).unlink()   # only the newest report is kept
            except OSError:
                pass
        prev_report = last.get("report") or prev_report
        ai = float(last.get("ai_percent") or 0)
        fl = last.get("flagged") or {}
        entry["high"], entry["medium"] = len(fl.get("high", [])), len(fl.get("medium", []))
        if ai <= target:
            log(f"academi.cx says {ai:g}% AI: done.")
            break
        if rnd == 1 and ask is not None:
            fl_n = len(fl.get("high", [])) + len(fl.get("medium", []))
            cnt = fl.get("count")
            flagged_txt = f"{cnt[0]} of {cnt[1]} sentences are flagged" if cnt else (f"{fl_n} passages are highlighted" if fl_n else "some sentences are highlighted")
            wants = ask(f"academi.cx still finds {ai:g}% AI",
                        f"There is some AI left ({flagged_txt}). 1) Remove the AI with Tobi: he finds the blue and purple sentences in the academi.cx report, "
                        "humanizes them again on stealthwriter and checks the file again.  2) Remove it yourself: you get the edited Word file and the report now.",
                        ["Remove the AI with Tobi", "I'll do it myself"])
            if not wants:
                entry["declined"] = True
                log(f"You chose to remove the remaining {ai:g}% AI yourself. Here is the edited file and the academi.cx report.")
                break
        if rnd > max_rounds:
            log(f"academi.cx still says {ai:g}% AI after {max_rounds} improvement round(s); stopping here.")
            break
        texts = [(t, "high likelihood (blue)") for t in fl.get("high", [])] + [(t, "medium likelihood (purple)") for t in fl.get("medium", [])]
        found = cc.find_sentences(per_para, texts, min_ratio=0.8)
        entry["matched"] = len(found)
        if not found:
            log("  I could not match the highlighted passages to sentences of the document; stopping here.")
            break
        log(f"academi.cx says {ai:g}% AI. Round {rnd}: improving {len(found)} highlighted sentence(s) ...")
        n = _refine(plan, per_para, found, cfg, judge, factory, log, f"ac{rnd}-", max_words)
        entry["refined"] = n
        if not n:
            log("  nothing could be improved; stopping here.")
            break
        save()
    return last, rounds


def _academi_step(cfg, out_path, log, notify):
    from .academi import AcademiClient

    t_ac = time.time()
    try:
        with AcademiClient(cfg, log, notify) as ac:
            r = ac.check_file(out_path, out_path.parent)
        log(f"academi.cx check took {_fmt(time.time() - t_ac)}.")
        log(f"academi.cx: {r['ai_percent']:g}% AI. Report: {Path(r['report']).name if r.get('report') else '(none)'}")
        return r
    except Exception as e:
        msg = f"{e.__class__.__name__}: {e}"
        log(f"!! The academi.cx step failed: {msg}")
        return {"ok": False, "error": msg}


def deep_merge(base: dict, extra: dict) -> dict:
    out = copy.deepcopy(base)
    for k, v in (extra or {}).items():
        if isinstance(v, dict) and isinstance(out.get(k), dict):
            out[k] = deep_merge(out[k], v)
        else:
            out[k] = v
    return out


def make_config(data_dir: Path, overrides: dict | None = None) -> dict:
    """Default settings for the humanizer, with its browser profile and debug files inside the agent's data folder."""
    cfg = deep_merge(DEFAULTS, overrides or {})
    cfg["browser"]["profile_dir"] = str(Path(data_dir) / "browser_profile")
    cfg["debug_dir"] = str(Path(data_dir) / "humanizer_debug")
    cfg["judge"]["type"] = "llm"
    cfg["judge"].setdefault("mode", "best")
    return cfg


# --------------------------------------------------------------------------- command parsing
FILE_RX = re.compile(r"""["']?([A-Za-z]:\\[^"'\n]+?\.docx?|[^\s"']+\.docx?)["']?""", re.I)


def parse_command(command: str, file: str | None = None) -> tuple[str, str | None]:
    """Returns (action, file). Only 'humanize' exists for now."""
    c = (command or "").strip().lower()
    if c and not re.search(r"humani[sz]e", c) and not FILE_RX.search(command):
        raise ValueError(f"I only know the command 'Humanize this file' so far (got: {command!r}).")
    if not file:
        m = FILE_RX.search(command or "")
        file = m.group(1) if m else None
    return "humanize", file


# --------------------------------------------------------------------------- helpers
def _batch_key(texts: list[str], level: int) -> str:
    return hashlib.sha1(("\x00".join(texts) + f"|L{level}").encode("utf-8")).hexdigest()[:16]


class State:
    """Remembers finished batches so a run stopped by the daily limit can continue later."""

    def __init__(self, path: Path):
        self.path = path
        self.data: dict = {}
        if path.exists():
            try:
                self.data = json.loads(path.read_text(encoding="utf-8"))
            except Exception:
                self.data = {}

    def get(self, key):
        return self.data.get(key)

    def put(self, key, value):
        self.data[key] = value
        self.path.write_text(json.dumps(self.data, ensure_ascii=False, indent=1), encoding="utf-8")

    def clear(self):
        self.data = {}
        if self.path.exists():
            self.path.unlink()


def open_in_word(path: Path, log):
    try:
        if sys.platform.startswith("win"):
            os.startfile(str(path))  # type: ignore[attr-defined]
        elif sys.platform == "darwin":
            os.system(f'open "{path}"')
        else:
            log(f"(Not on Windows: open {path} in Word yourself.)")
            return
        log("Opened the result in Word.")
    except Exception as e:
        log(f"Could not open Word automatically: {e}")


def _make_client(cfg, judge, log):
    from .stealth_client import StealthwriterClient

    return StealthwriterClient(cfg, judge, log)


# --------------------------------------------------------------------------- the job
def run_job(
    file: str,
    command: str = "Humanize this file",
    *,
    config: dict | None = None,
    dry_run: bool = False,
    start: str | None = None,
    output: str | None = None,
    reset: bool = False,
    client_factory=None,
    llm=None,
    log=print,
    notify=None,
    ask=None,
    cancel=None,
) -> dict:
    cfg = config or make_config(Path.home() / ".windyland" / "agent")
    _stop.set_event(cancel)
    t_job = time.time()
    action, parsed_file = parse_command(command, file)
    file = file or parsed_file
    if not file:
        raise ValueError("No file given.")
    src = Path(os.path.expanduser(file)).resolve()
    if not src.exists():
        raise FileNotFoundError(f"File not found: {src}")
    if src.suffix.lower() != ".docx":
        raise ValueError("Only .docx files are supported (save .doc files as .docx first).")

    dcfg = cfg["document"]
    doc = Document(str(src))
    try:
        plan = build_plan(
            doc,
            max_words=int(dcfg["max_words_per_batch"]),
            min_words=int(dcfg["min_words_per_paragraph"]),
            start_pattern=start or dcfg.get("start_heading") or None,
            stop_patterns=dcfg.get("stop_headings") or None,
            include_lists=bool(dcfg["include_lists"]),
            flatten_mixed=bool(dcfg["flatten_mixed_formatting"]),
            flatten_complex=bool(dcfg.get("flatten_complex_content", True)),
        )
    except PlanError as e:
        return {"ok": False, "error": str(e)}

    batches = plan.batches(int(dcfg["max_words_per_batch"]), int(dcfg.get("min_words_per_batch", 800)))
    total_words = sum(u.words for u in plan.units)
    log(f"File: {src.name}")
    log(f"Starting at the paragraph: '{plan.start_text}'" + (f"  | stopping before: '{plan.stop_text}'" if plan.stop_text else ""))
    log(f"{len(plan.paragraphs)} body paragraphs, {total_words} words, {len(batches)} batch(es) of up to {dcfg['max_words_per_batch']} words.")
    skips = plan.skip_counts()
    if skips:
        log("Left untouched: " + ", ".join(f"{n} x {r}" for r, n in skips.items()))
        for i, reason, prev in [x for x in plan.skipped if x[1] not in ("heading", "caption", "toc")][:12]:
            log(f"    paragraph {i}: {reason}: {prev}")

    if dry_run:
        for b, batch in enumerate(batches, 1):
            log(f"  batch {b}: {sum(u.words for u in batch)} words, paragraphs {sorted({u.para_index for u in batch})}")
        return {"ok": True, "dry_run": True, "batches": len(batches), "words": total_words,
                "paragraphs": len(plan.paragraphs), "skipped": skips}
    if not batches:
        return {"ok": False, "error": "Nothing to humanize: every paragraph after the start was skipped (too short, headings, lists, links or mixed formatting)."}

    state = State(src.with_suffix(".humanize_state.json"))
    if reset:
        state.clear()

    judge = make_judge(cfg["judge"], log, _judge_llm(cfg, llm, log))
    level = int(cfg["site"]["level"])
    factory = client_factory or _make_client
    results_parts: dict[tuple[int, int], str] = {}
    failures: list[str] = []
    limit_hit = False
    review_log: list[dict] = []

    from .stealth_client import LimitReached, MappingError, SiteError

    cancelled = False
    client = None
    t_stage = time.time()
    try:
        client_cm = factory(cfg, judge, log)
        with client_cm as client:
            for b, batch in enumerate(batches, 1):
                _stop.check()
                texts = [u.text for u in batch]
                key = _batch_key(texts, level)
                cached = state.get(key)
                if cached and len(cached) == len(batch):
                    log(f"[batch {b}/{len(batches)}] already done earlier, reusing.")
                    parts = cached
                else:
                    t_b = time.time()
                    try:
                        parts = client.humanize_batch(texts, label=f"b{b}")
                        state.put(key, parts)
                        review_log += getattr(client, "review_log", [])
                        client.review_log = []
                        log(f"[batch {b}/{len(batches)}] done in {_fmt(time.time() - t_b)}.")
                    except LimitReached as e:
                        log(f"!! {e} Stopping here; run the same command tomorrow to continue.")
                        limit_hit = True
                        break
                    except (MappingError, SiteError) as e:
                        log(f"!! batch {b} failed: {e}")
                        failures.append(f"batch {b}: {e}")
                        continue
                for u, t_ in zip(batch, parts):
                    results_parts[(u.para_index, u.part)] = t_
    except Cancelled:
        cancelled = True
    if not cancelled:
        log(f"Stealthwriter step finished in {_fmt(time.time() - t_stage)}.")

    # paragraphs only count as done when every one of their parts came back
    per_para: dict[int, str] = {}
    for idx in plan.paragraphs:
        n_parts = next(u.parts for u in plan.units if u.para_index == idx)
        got = [results_parts.get((idx, k)) for k in range(n_parts)]
        if all(g is not None for g in got):
            per_para[idx] = " ".join(got)  # type: ignore[arg-type]

    stats = {"paragraphs_changed": 0, "sentences_changed": 0}
    if cancelled:   # stopped while humanizing: what is finished goes into a separate copy (your file is not touched)
        res = {"ok": False, "cancelled": True, "complete": False, "paragraphs_total": len(plan.paragraphs),
               "paragraphs_done": len(per_para), "output": None, **stats}
        if per_para:
            part_path = src.with_name(src.stem + "_partial.docx")
            try:
                d = Document(str(src))
                res.update({k: v for k, v in apply_results(d, plan, per_para, mark_red=bool(dcfg["mark_changed_red"])).items()
                            if k in ("paragraphs_changed", "sentences_changed")})
                d.save(str(part_path))
                res["output"], res["ok"] = str(part_path), True
                log(f"Stopped. {len(per_para)} of {len(plan.paragraphs)} paragraphs were done; saved as {part_path.name}.")
            except Exception as e:
                log(f"Stopped. The finished part could not be saved: {e}")
        else:
            log("Stopped before the first section was finished.")
        return res
    out_path = src
    sentence_check = None
    pristine = src   # where the untouched original lives (used again if Claude's touch-ups are applied afterwards)

    def save_humanized(first: bool):
        nonlocal out_path, pristine, stats
        d = Document(str(pristine))
        stats = apply_results(d, plan, per_para, mark_red=bool(dcfg["mark_changed_red"]))
        for idx, why in stats.get("failed_paragraphs", []):
            log(f"!! paragraph {idx} could not be written back and was left as it was: {why}")
        mode = output or dcfg["output"]
        if first:
            if mode == "copy":
                out_path = src.with_name(src.stem + "_humanized.docx")
            else:
                backup = src.with_name(src.stem + ".original.docx")
                if not backup.exists():
                    shutil.copy2(src, backup)
                    log(f"Backup of your original: {backup.name}")
                pristine = backup
        try:
            d.save(str(out_path))
        except PermissionError:
            out_path = src.with_name(src.stem + "_humanized.docx")
            d.save(str(out_path))
            log(f"The file is open in Word, so I saved a copy instead: {out_path.name}")
        log(f"Saved: {out_path}  ({stats['sentences_changed']} sentences changed, shown in red)")

    complete_now, refined_by_claude = False, 0
    try:
        complete_now = len(per_para) == len(plan.paragraphs) and not failures and not limit_hit
        refined_by_claude = 0
        if per_para:
            save_humanized(True)   # 1. the humanized text goes into the Word file first
        if per_para and cfg.get("claude_check", {}).get("enabled"):
            if complete_now:   # 2. Claude only sees the document once ALL of it is humanized
                t_rev = time.time()
                sentence_check = _document_review(plan, per_para, cfg["claude_check"], llm, log, cfg, notify)
                log(f"Document review finished in {_fmt(time.time() - t_rev)}.")
                if sentence_check and sentence_check.get("applied"):
                    save_humanized(False)   # mode "apply": the whole file again, with Claude's small touch-ups
                elif sentence_check and sentence_check.get("flagged"):
                    # mode "flag": Claude only names the weak sentences; the better version comes from stealthwriter's own options
                    refined_by_claude = _refine(plan, per_para, sentence_check["flagged"], cfg, judge, factory, log, "claude", int(dcfg["max_words_per_batch"]))
                    if refined_by_claude:
                        save_humanized(False)
            else:
                log("Not all of the document is humanized yet, so Claude's review waits until the rest is done.")

    except Cancelled:
        cancelled = True
    if sentence_check and per_para:
        from . import claude_check as _cc
        try:
            sentence_check["report"] = str(_cc.write_review_report(sentence_check, out_path.with_suffix(".sentence-check.txt")))
        except OSError:
            pass
    done = len(per_para)
    complete = done == len(plan.paragraphs) and not failures and not limit_hit
    if complete:
        state.clear()
    report = {
        "ok": done > 0, "complete": complete, "output": str(out_path), "paragraphs_total": len(plan.paragraphs),
        "paragraphs_done": done, "limit_reached": limit_hit, "failures": failures, **stats,
        "scores": getattr(client, "scores", []),
    }
    if sentence_check:
        report["claude_check"] = {k: sentence_check[k] for k in ("engine", "overall", "report")}
        report["claude_check"]["edits"] = len(sentence_check.get("applied", []))
        report["claude_check"]["weak"] = len(sentence_check.get("flagged", []))
        report["claude_check"]["refined"] = refined_by_claude
        report["claude_check"]["refused"] = len(sentence_check.get("rejected", []))

    def write_report():
        try:
            Path(cfg["debug_dir"]).mkdir(parents=True, exist_ok=True)
            (Path(cfg["debug_dir"]) / "last_report.json").write_text(
                json.dumps({**report, "choices": review_log}, ensure_ascii=False, indent=1, default=str), encoding="utf-8")
        except Exception:
            pass

    write_report()
    if cancelled:
        pass
    else:
      try:
          if done and complete and cfg.get("academi", {}).get("enabled"):
              last, rounds = _academi_loop(cfg, plan, per_para, lambda: save_humanized(False), lambda: out_path, judge, factory, log, notify,
                                           int(dcfg["max_words_per_batch"]), ask)
              report["academi"], report["academi_rounds"] = last, rounds
              report["academi_declined"] = any(r.get("declined") for r in rounds)
              report.update({k: v for k, v in stats.items() if k in ("paragraphs_changed", "sentences_changed")})
              report["output"] = str(out_path)
              write_report()
      except Cancelled:
        cancelled = True
    report["cancelled"] = cancelled
    report["seconds"] = int(time.time() - t_job)
    log(f"Total time: {_fmt(time.time() - t_job)}.")
    if done and dcfg.get("open_in_word_when_done") and not cancelled:
        open_in_word(out_path, log)
    if not complete:
        log(f"Finished {done}/{len(plan.paragraphs)} paragraphs. Run the same command again to continue the rest.")
    return report
