"""Fast path for plain commands: "Humanize this file" (with a file attached), "Play Naruto", "pause", "remind me in 20 minutes to ...".

These are understood with simple rules, so they run at once, cost no AI tokens and do not depend on how good the
language model is (a small local model often picks the wrong tool). Anything the rules do not recognise goes to the model.
"""
from __future__ import annotations

import json
import os
import re

ATTACH_RX = re.compile(r"\[Attached file: (.+?)\]")


def split_attachments(text: str) -> tuple[list[str], str]:
    files = ATTACH_RX.findall(text)
    return files, ATTACH_RX.sub("", text).strip()


def _words(s: str) -> int:
    return len(s.split())


NUM = {"a": 1, "an": 1, "one": 1, "two": 2, "three": 3, "four": 4, "five": 5, "ten": 10, "fifteen": 15, "twenty": 20, "thirty": 30,
       "forty": 40, "sixty": 60}
FILLER = re.compile(r"^(?:(?:please|pls|plz|hey tobi|tobi|hey|hi|yo|ok|okay|so|well|now|then|just|kindly|go ahead and|can you|could you|would you|will you|would you mind|do you mind|can u|could u|i want you to|i need you to|i would like you to|i want to|i wanna|i'd like to|i would like to|i need to|let's|lets|let us|you should|you can)[,\s]+)+", re.I)


def _minutes(n: str, unit: str) -> float:
    v = float(NUM.get(n.lower(), n if n.replace(".", "", 1).isdigit() else 1))
    u = unit.lower()
    return v / 60 if u.startswith("sec") else v * 60 if u.startswith(("hour", "hr", "h")) else v


def route(text: str) -> list[dict] | None:
    """A list of steps [{"tool", "args"}] (an arg of "$file" means: the path chosen by the previous pick_file step), or None."""
    files, body = split_attachments(text)
    t = FILLER.sub("", body.strip()).strip().rstrip(".!")
    low = t.lower()
    docs = [f for f in files if f.lower().endswith((".docx", ".doc"))]

    # ---- humanizing
    if re.search(r"\bhumani[sz]", low):
        if re.search(r"\b(what|which|show|list|history|did you|have you|so far|before|unfinished|continue)\b", low) and not docs:
            return [{"tool": "humanize_history", "args": {}}]
        if _words(low) <= 14:
            extra: dict = {}
            if re.search(r"\b(without|skip|no|don'?t|do not)\b[^.]*\b(academi|upload|ai score)", low):
                extra["academi"] = False
            if re.search(r"\b(without|skip|no|don'?t|do not)\b[^.]*\b(claude|sentence (?:check|score))", low):
                extra["claude_check"] = False
            if docs:
                return [{"tool": "humanize_document", "args": {"path": docs[0], **extra}}]
            m = re.search(r"""["']?([A-Za-z]:\\[^"'\n]+?\.docx?)["']?""", t)
            if m:
                return [{"tool": "humanize_document", "args": {"path": m.group(1), **extra}}]
            return [{"tool": "pick_file", "args": {"kind": "docx"}}, {"tool": "humanize_document", "args": {"path": "$file", **extra}}]
        return None
    if files:  # files were attached for something else: let the model decide
        return None

    # ---- the knowledge folder
    if re.search(r"\bknowledge\b", low) and _words(low) <= 12:
        if re.search(r"\b(open|show me|go to|where)\b", low) and re.search(r"\b(folder|directory)\b", low):
            return [{"tool": "open_knowledge_folder", "args": {}}]
        if re.search(r"\b(refresh|reload|re-?read|update|rescan|scan|learn|study|read)\b", low):
            return [{"tool": "refresh_knowledge", "args": {}}]
        if re.search(r"\b(list|what|which|show)\b", low):
            return [{"tool": "list_knowledge", "args": {}}]

    # ---- media keys / volume
    m = re.fullmatch(r"(pause|resume|play|continue|stop|mute|unmute|next|skip|previous|go back|back|volume up|volume down|louder|quieter|"
                     r"turn (?:it |the volume )?(?:up|down)|(?:increase|decrease|lower|raise) (?:the )?volume)"
                     r"(?: (?:the )?(?:music|song|video|track|episode|playback|it|this|a bit|a little|a lot|slightly))*", low)
    if m:
        w = m.group(1)
        steps = 10 if "a lot" in low else 3 if re.search(r"a bit|a little|slightly", low) else 5
        if w in ("pause", "resume", "play", "continue", "stop"):
            return [{"tool": "media_control", "args": {"action": "play_pause"}}]
        if w in ("mute", "unmute"):
            return [{"tool": "media_control", "args": {"action": "mute"}}]
        if w in ("next", "skip"):
            return [{"tool": "media_control", "args": {"action": "next"}}]
        if w in ("previous", "go back", "back"):
            return [{"tool": "media_control", "args": {"action": "previous"}}]
        down = any(k in w for k in ("down", "quieter", "decrease", "lower"))
        return [{"tool": "media_control", "args": {"action": "volume_down" if down else "volume_up", "steps": steps}}]

    # ---- play something / while eating
    eating = re.search(r"\b(i(?:'m| am)? eating|while i(?:'m| am)? eating|having (?:my )?(?:dinner|lunch|breakfast|food|a meal)|dinner time|lunch time)\b", low)
    if re.search(r"\b(play|put on|watch|show me|start)\b", low) and (eating or re.search(
            r"\b(play|put on|show me|start)\s+(me\s+)?(something|anything|a random (video|documentary)|a video|a documentary|surprise me)\b", low)) \
            or low in ("surprise me", "play something", "play anything"):
        topic = ""
        m = re.search(r"\b(?:about|on|of)\s+(.+)$", low)
        if m and "youtube" not in m.group(1):
            topic = m.group(1)
        return [{"tool": "play_something", "args": {"topic": topic} if topic else {}}]

    m = re.match(r"(?:open|launch|run|start|fire up|boot up|bring up)\s+(?:up\s+)?(.+)", t, re.I)
    if m and _words(m.group(1)) <= 4 and not re.search(r"\b(and|then|to|for|playing|my files?|folder|directory|document|file)\b", m.group(1), re.I) \
            and not re.match(r"https?://|[A-Za-z]:\\|[~/\\]", m.group(1)) and not re.search(r"\.\w{2,4}$", m.group(1).strip()):
        target = re.sub(r"^(the |my |an? )", "", m.group(1).strip(), flags=re.I).strip("\"'")
        verb = t.split()[0].lower()
        if verb not in ("start", "run") or _looks_like_app(target):
            return [{"tool": "open_app", "args": {"name": target}}]

    m = re.match(r"(?:play|put on|start playing|start)\s+(.+)", t, re.I)
    if m:
        what = m.group(1).strip()
        wl = what.lower()
        what = re.sub(r"^(me\s+|some\s+|a\s+|an\s+|the\s+|for me\s+)+", "", what, flags=re.I).strip() or what
        yt = re.search(r"\s+(?:on|in|from|using)\s+youtube\b|\byoutube\b", what, re.I)
        what_clean = re.sub(r"\s*(?:on|in|from|using)\s+youtube\b|\byoutube\b", "", what, flags=re.I).strip(" ,")
        if re.search(r"\b(anime|episode|ep\.?\s*\d+|season|s\d+\s*e\d+)\b", wl) and not yt:
            return [_anime_step(what)]
        if yt or re.search(r"\b(song|songs|music|lofi|lo-fi|playlist|remix|lyrics|mix|beats|soundtrack|ost|album|karaoke|instrumental)\b|\bby\b", wl):
            return [{"tool": "play_youtube", "args": {"query": what_clean or what}}]
        if re.search(r"\bdocumentar", wl):
            return [{"tool": "play_youtube", "args": {"query": what_clean or what, "min_minutes": 20, "pick": "random"}}]
        if len(what.split()) <= 8:
            return [{"tool": "play", "args": {"query": what}}]
        return None

    # ---- reminders / timers
    m = re.match(r"(?:remind me|set (?:a )?reminder)\s+(?:in|after)\s+(\d+(?:\.\d+)?|an?|one|two|three|four|five|ten|fifteen|twenty|thirty|forty|sixty)\s*"
                 r"(seconds?|secs?|minutes?|mins?|hours?|hrs?)\s*(?:to|that|about|:|-)?\s*(.*)", t, re.I)
    if m:
        what = m.group(3).strip() or "Time is up"
        return [{"tool": "set_reminder", "args": {"text": what[:1].upper() + what[1:], "in_minutes": _minutes(m.group(1), m.group(2))}}]
    m = re.match(r"(?:set (?:a )?(?:timer|alarm)|timer)\s+(?:for\s+)?(\d+(?:\.\d+)?|an?|one|five|ten|fifteen|twenty|thirty)\s*(seconds?|secs?|minutes?|mins?|hours?|hrs?)", t, re.I)
    if m:
        return [{"tool": "set_reminder", "args": {"text": "Your timer is up", "in_minutes": _minutes(m.group(1), m.group(2))}}]
    if re.fullmatch(r"(?:what are my |show (?:me )?my |list (?:my )?)?reminders", low):
        return [{"tool": "list_reminders", "args": {}}]

    # ---- weather
    m = re.match(r"(?:what(?:'s| is) the |how(?:'s| is) the |show (?:me )?the |tell me the )?(?:weather|forecast)\s+(?:in|at|for|of)\s+(.+?)(?:\s+(?:today|now|right now|tomorrow))?$", t, re.I)
    if m:
        return [{"tool": "weather", "args": {"city": m.group(1).strip(" ?")}}]

    # ---- memory
    if re.fullmatch(r"what do you (?:know|remember) about me\??|what have you (?:saved|remembered)\??|show (?:me )?(?:my )?memory", low):
        return [{"tool": "recall", "args": {}}]
    m = re.match(r"forget\s+(?:about\s+|that\s+)?(.+)", t, re.I)
    if m and _words(m.group(1)) <= 8:
        return [{"tool": "forget", "args": {"what": m.group(1).strip()}}]
    m = re.match(r"(?:remember|note)(?: that)?\s+(.+)", t, re.I)
    if m:
        what = m.group(1).strip()
        kind = "interest" if re.search(r"\b(like|love|enjoy|favou?rite|fan of|into)\b", what, re.I) else "fact"
        return [{"tool": "remember", "args": {"text": what, "kind": kind}}]
    m = re.match(r"i (?:really |absolutely |also )?(?:love|like|enjoy|adore|am a fan of|am into)\s+(.+)", t, re.I)
    if m and "?" not in body and _words(m.group(1)) <= 10 and not re.search(r"\b(you|to (?:play|open|run|do|have|ask))\b", m.group(1), re.I):
        return [{"tool": "remember", "args": {"text": "likes " + m.group(1).strip(), "kind": "interest"}}]

    # ---- power
    if re.fullmatch(r"lock (?:my |the )?(?:pc|computer|laptop|screen|windows)", low):
        return [{"tool": "power", "args": {"action": "lock"}}]
    if re.fullmatch(r"cancel (?:the |my )?(?:shut ?down|restart|reboot)", low):
        return [{"tool": "power", "args": {"action": "cancel"}}]
    m = re.fullmatch(r"(shut ?down|restart|reboot|sleep|put (?:the |my )?(?:pc|computer|laptop) to sleep)(?: (?:my |the )?(?:pc|computer|laptop))?(?: in (\d+) (?:minutes?|mins?))?", low)
    if m:
        w = m.group(1)
        action = "shutdown" if w.startswith("shut") else "restart" if w in ("restart", "reboot") else "sleep"
        args = {"action": action}
        if m.group(2):
            args["delay_minutes"] = int(m.group(2))
        return [{"tool": "power", "args": args}]

    # ---- simple PC commands
    if re.fullmatch(r"(?:take (?:a )?)?screenshot(?: of (?:my |the )?screen)?|screenshot", low):
        return [{"tool": "screenshot", "args": {}}]
    if re.search(r"\bwhat(?:'s| is)\s+(?:using|eating|taking)\b.*\b(ram|memory|cpu)\b", low):
        return [{"tool": "list_processes", "args": {"sort_by": "cpu" if "cpu" in low else "memory"}}]
    m = re.match(r"open\s+(.+)", t, re.I)
    if m and _words(m.group(1)) <= 5 and not re.search(r"\b(and|then)\b", low):
        return [{"tool": "open_target", "args": {"target": re.sub(r"^(the |my )", "", m.group(1).strip(), flags=re.I).strip("\"'")}}]
    return None


def _looks_like_app(name: str) -> bool:
    """'start X' is only an app launch when X is a program installed here or a known alias (else it is 'start the music')."""
    try:
        from . import apps
        if apps._norm(name) in apps.ALIASES or name.lower() in apps.ALIASES:
            return True
        hits = apps.match(name, apps.index(store=False), 1)
        return bool(hits and hits[0][0] >= 0.88)
    except Exception:
        return False


def _anime_step(what: str) -> dict:
    args: dict = {}
    low = what.lower()
    m = re.search(r"\bs(\d+)\s*e(\d+)\b", low) or re.search(r"\bseason\s*(\d+)\D+(?:episode|ep\.?)\s*(\d+)", low)
    if m:
        args["season"], args["episode"] = int(m.group(1)), int(m.group(2))
    else:
        m = re.search(r"\b(?:episode|ep\.?)\s*(\d+)", low)
        if m:
            args["episode"] = int(m.group(1))
        m = re.search(r"\bseason\s*(\d+)", low)
        if m:
            args["season"] = int(m.group(1))
    if re.search(r"\b(latest|newest|last)\b", low):
        args["which"] = "latest"
    title = re.sub(r"\bs\d+\s*e\d+\b|\b(?:season|episode|ep\.?)\s*\d+\b|\b(?:latest|newest|last|first)\b|\banime\b|\bepisode\b|\bof\b|\bon animesalt(?:\.cx)?\b", " ", what, flags=re.I)
    title = re.sub(r"(?:\s+(?:on|in|at|from|for))+\s*$", "", " ".join(title.split()).strip(" ,-"), flags=re.I)
    args["title"] = title.strip(" ,-") or what
    return {"tool": "play_anime", "args": args}


def summarize(tool: str, ok: bool, result: str) -> str:
    """The one-or-two-line answer shown after a routed command (no AI needed)."""
    if not ok:
        return "Okay, I won't do that." if result.startswith("The user said no") else result
    if tool == "humanize_document":
        try:
            r = json.loads(result)
        except Exception:
            return result[:600]
        if r.get("cancelled"):
            if r.get("kept"):
                return (f"Stopped. Here is the work so far: {r.get('paragraphs_done', 0)} of {r.get('paragraphs_total', 0)} paragraphs "
                        f"are humanized in {os.path.basename(str(r.get('output', '')))} (changed sentences are red).")
            return "Stopped. I did not keep the unfinished work." + (" The finished part is saved next to your file." if r.get("output") else "")
        if not r.get("ok"):
            return "I could not humanize the file: " + str(r.get("error") or "; ".join(r.get("failures") or []) or "unknown problem")
        out = os.path.basename(str(r.get("output", "")))
        head = "Done!" if r.get("complete") else f"Partly done ({r.get('paragraphs_done')}/{r.get('paragraphs_total')} paragraphs)."
        tail = " The free limit was reached; say 'humanize this file' again later and I will continue." if r.get("limit_reached") else ""
        lines = [f"{head} {r.get('sentences_changed', 0)} sentences were rewritten and are shown in red in {out}. "
                 f"Your original is saved as .original.docx.{tail}"]
        sc = [x for x in r.get("scores") or [] if x.get("final") is not None]
        if sc:
            lines.append("Detection score on stealthwriter: " + ", ".join(
                f"{x['final']}% human" + (f" (after {x['rehumanized']} Rehumanize)" if x.get("rehumanized") else "") for x in sc) + ".")
        cc = r.get("claude_check")
        if cc:
            if cc.get("edits"):
                lines.append(f"Claude's review of the whole document ({cc.get('engine')}): {cc.get('overall')}/100, {cc.get('edits', 0)} sentence(s) lightly fine-tuned"
                             + (f" (details in {os.path.basename(cc['report'])})." if cc.get("report") else "."))
            else:
                lines.append(f"Claude's review of the whole document ({cc.get('engine')}): {cc.get('overall')}/100; it named {cc.get('weak', 0)} weak sentence(s), "
                             f"{cc.get('refined', 0)} redone with better stealthwriter options"
                             + (f" (details in {os.path.basename(cc['report'])})." if cc.get("report") else "."))
        ac = r.get("academi")
        rounds = r.get("academi_rounds") or []
        if ac:
            if ac.get("ok") and ac.get("report"):
                trail = " -> ".join(f"{x['ai_percent']:g}%" for x in rounds if x.get("ai_percent") is not None)
                lines.append(f"academi.cx AI score: {ac['ai_percent']:g}% AI" + (f" (rounds: {trail})" if len(rounds) > 1 else "")
                             + f". Report: {os.path.basename(ac['report'])}.")
                if r.get("academi_declined"):
                    lines.append("You chose to remove the remaining AI yourself: the edited Word file and the report are above. "
                                 "Say 'humanize this file' again if you want me to try after all.")
            else:
                lines.append(f"academi.cx step did not finish: {ac.get('error', 'unknown problem')}")
        return "\n".join(lines)
    return result[:700]
