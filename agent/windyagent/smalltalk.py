"""Tobi's everyday conversation: greetings, thanks, "how are you", jokes, the time... answered at once from keywords, with no AI model.

* Built-in skills below (keywords/patterns -> several possible answers, picked at random).
* Your own skills live in ``conversation.json`` in Tobi's data folder and can be added by talking:
      when I say "good night" you say "Sleep well, see you tomorrow!"
      forget what you say for "good night"
      what conversation skills do you have
  or by editing the file: {"rules": [{"triggers": ["good night", "gn"], "answers": ["Sleep well!", "Night night!"]}]}
Answers may contain {time}, {date}, {day} and {user}.
"""
from __future__ import annotations

import getpass
import json
import random
import re
from datetime import datetime

from . import config

SLANG = {
    "thx": "thanks", "ty": "thank you", "tnx": "thanks", "pls": "please", "plz": "please", "u": "you", "ur": "your", "r": "are",
    "gm": "good morning", "gn": "good night", "hbu": "how about you", "wassup": "what is up", "whats": "what is", "what's": "what is",
    "sup": "what is up", "hru": "how are you", "wyd": "what are you doing", "idk": "i do not know", "im": "i am", "i'm": "i am",
    "ya": "yes", "yeah": "yes", "yep": "yes", "yup": "yes", "nah": "no", "nope": "no", "k": "ok", "kk": "ok", "okie": "ok",
    "you're": "you are", "youre": "you are", "that's": "that is", "thats": "that is", "it's": "it is", "who's": "who is", "how's": "how is",
    "where's": "where is", "there's": "there is", "don't": "do not", "dont": "do not", "can't": "cannot", "cant": "cannot",
}


ELONGATED = [(re.compile(r"he+y+"), "hey"), (re.compile(r"hi+"), "hi"), (re.compile(r"he+l+o+"), "hello"), (re.compile(r"yo+"), "yo"),
             (re.compile(r"tha+n+k+s+"), "thanks"), (re.compile(r"by+e+"), "bye"), (re.compile(r"o+k+a*y*"), "ok"), (re.compile(r"so+"), "so"),
             (re.compile(r"ha(?:ha)+h?|he(?:he)+h?"), "haha"), (re.compile(r"lo+l+"), "lol"), (re.compile(r"wo+w+"), "wow"), (re.compile(r"no+pe+"), "nope")]


def normalize(text: str) -> str:
    """Lower case, no punctuation, 'heyyyy' -> 'hey', common chat spellings expanded ('thx' -> 'thanks', "what's" -> 'what is')."""
    t = (text or "").lower().replace("\u2019", "'")
    t = re.sub(r"\[attached file:.*?\]", " ", t)
    words = []
    for w in re.findall(r"[a-z0-9']+", t):
        for rx, base in ELONGATED:
            if rx.fullmatch(w):
                w = base
                break
        words.append(SLANG.get(w, w))
    return " ".join(" ".join(words).split())


VOC = re.compile(r"^(?:(?:hey|hi|hello|ok|okay|so|well|and|please|then)\s+)*(?:tobi\b[\s,]*)?")


def _strip_vocative(n: str) -> str:
    n = re.sub(r"\btobi\b", " ", n)
    return " ".join(n.split())


# --------------------------------------------------------------------------- built-in skills
# each: (name, [regex that must match the whole normalized message], [answers])
_PEOPLE = r"(?:tobi|buddy|friend|mate|bro|man|dude|there|everyone|all|again)?"
BUILTIN: list[tuple[str, list[str], list[str]]] = [
    ("how are you", [r"(?:hi |hey |hello )?(?:how are you|how are you doing|how are you going|how do you do|how is it going|how have you been|how is life|are you ok|are you okay|are you good|what is up|how is your day|how is your day going|how you doing|how are things|what is going on|whats going on)(?: today| tonight| now| these days| lately)?(?: " + _PEOPLE + ")?",
                         r"(?:hi |hey |hello )?(?:how are you doing|how are you) (?:today|tonight|now)"],
     ["I'm doing great, thanks for asking! Ready when you are. What shall we do?", "All good here, sitting in the notch and waiting for something fun to do. How are you?",
      "Doing well! Everything is running smoothly. What can I do for you?"]),
    ("i am fine", [r"(?:i am |i feel |i am doing )(?:fine|good|great|well|ok|okay|not bad|pretty good|alright|so so|fantastic|awesome|amazing)(?: thanks| thank you)?(?: and you| how about you)?",
                      r"(?:not bad|pretty good|doing good|doing great|doing well|all good|fine thanks|good thanks|great thanks|fine and you|good and you|great and you|fine thank you|good thank you|great thank you)(?: and you| how about you)?"],
     ["Glad to hear it! Tell me if you want music, a file humanized, or anything else.", "Nice! Let me know what you need.", "Good to hear. I'm here if you need me."]),
    ("i am sad", [r"(?:i am |i feel )(?:sad|stressed|down|upset|exhausted|lonely|unwell|sick|anxious|worried)(?: today| now| right now)?"],
     ["Sorry to hear that. Want me to play something relaxing, or something fun to take your mind off it? Just say 'play something'.",
      "That sounds rough. I can put on some music or a funny video if you like. Just tell me."]),
    ("greeting", [r"(?:hi|hello|hey|yo|hiya|howdy|greetings|hola|salam|salaam|assalamualaikum|assalamu alaikum|namaste|sup)(?: there| everyone| again| friend| buddy| mate| man| bro)?",
                  r"(?:hi|hello|hey) (?:good )?(?:morning|afternoon|evening)", r"good (?:morning|afternoon|evening|day)(?: there| to you| everyone)?", r"morning|evening|afternoon"],
     ["{greet} What can I do for you?", "{greet} I'm ready. Humanize a file, play something, open an app?", "{greet} How can I help?"]),
    ("good night", [r"(?:good night|night night|nighty night|sleep well|sweet dreams|nite|going to sleep|going to bed|time for bed|off to bed)(?: tobi)?"],
     ["Good night! Sleep well. I'll be here tomorrow.", "Sweet dreams! See you tomorrow.", "Night night! Rest well."]),
    ("bye", [r"(?:bye|goodbye|good bye|see you|see ya|see you later|see you soon|later|cya|farewell|take care|catch you later|i am leaving|i am off|i have to go|gotta go|talk to you later|ttyl)(?: tobi| then| now| tomorrow)?"],
     ["Bye! I'll be right here in the notch whenever you need me.", "See you later! Take care.", "Goodbye! Call me when you need anything."]),
    ("thanks", [r"(?:thanks|thank you|thank you so much|thanks a lot|thanks so much|many thanks|much appreciated|i appreciate it|cheers|thank you tobi|thanks tobi|great thanks|ok thanks|thanks a ton|thanks man|thanks buddy|thank u)(?: very much| a lot| so much)?",
                 r"(?:ok |okay |great |cool )?(?:thanks|thank you)(?: for (?:that|your help|helping|everything|doing that))?"],
     ["You're welcome!", "Anytime!", "Happy to help!", "No problem at all."]),
    ("you are welcome", [r"(?:you are welcome|no problem|no worries|np|my pleasure|anytime|don't mention it|do not mention it)"], ["👍", "Great!"]),
    ("sorry", [r"(?:sorry|my bad|i am sorry|oops|apologies|excuse me|pardon|pardon me)(?: tobi)?"], ["No worries at all!", "That's fine, no problem.", "All good, don't worry about it."]),
    ("praise", [r"(?:good job|well done|nice work|great job|nice job|awesome|amazing|cool|nice|great|perfect|excellent|brilliant|you are (?:great|awesome|amazing|the best|smart|good|cool|nice|brilliant|helpful|so smart|so good)|good boy|good bot|love it|that is (?:great|awesome|perfect|cool|nice|amazing)|wow|impressive|well played|bravo)(?: tobi)?(?: thanks| thank you)?"],
     ["Thank you, that makes my day!", "Glad you like it!", "Aw, thanks! I try my best.", "Happy to hear it!"]),
    ("love", [r"(?:i love you|love you|i like you|you are my (?:best )?friend|you are cute|i love you tobi|love you tobi)(?: so much| a lot)?"],
     ["That's very kind! I'm happy to be your assistant.", "Aww, thank you! I enjoy working with you too."]),
    ("who are you", [r"(?:who are you|what are you|who is this|what is your name|your name|what should i call you|what do they call you|tell me about yourself|introduce yourself|who am i talking to|what is tobi|who is tobi|are you tobi|are you a bot|are you a robot|are you an ai|are you real|are you human|are you chatgpt|are you alive)(?: exactly| really)?"],
     ["I'm Tobi, your personal assistant living in WindyLand's notch. I can humanize Word files, play music and anime, open programs, set reminders, check the weather, tidy folders and learn from files you put in my knowledge folder.",
      "My name is Tobi! I'm an AI assistant that lives at the top of your screen and does things on your PC for you."]),
    ("who made you", [r"(?:who made you|who created you|who built you|who is your (?:maker|creator|developer|owner)|who programmed you|who designed you|where do you come from|where do you live)"],
     ["I was built for you as part of WindyLand. I live in the notch at the top of your screen, and the AI model inside me is the one you set up in Settings.",
      "I'm part of WindyLand, made for your PC. I run on your computer and use the AI model you chose."]),
    ("what can you do", [r"(?:what can you do|what do you do|what are you able to do|what are your (?:skills|abilities|features|functions|commands|capabilities)|help|help me|can you help me|i need help|i need some help|show me what you can do|what can i ask you|what can i say|what can i ask|how do you work|how can you help me|what are you good at|commands|what commands do you know|menu)(?: with)?(?: please)?"],
     ["Here is what I can do:\n- Humanize a Word file (say 'humanize this file' or drop a .docx), rewrite it on stealthwriter, check it with Claude and academi.cx\n- Play music or videos ('play Despacito'), anime ('play Naruto') or 'play something'\n- Open any program ('open Word', 'open Spotify')\n- Reminders, weather, volume, tidy a folder, power commands\n- Learn from your files: put Word, Excel or PDF files in my knowledge folder and ask me about them\n- Chat, and learn new replies ('when I say X you say Y')"]),
    ("are you there", [r"(?:are you there|are you here|are you listening|are you awake|hello are you there|you there|anybody there|is anyone there|tobi|tobi are you there|can you hear me|do you hear me|wake up|wake up tobi|ping|test|testing|testing testing|hello world)(?: tobi)?"],
     ["I'm here! What do you need?", "Yes, I'm listening. What's up?", "Right here. Tell me what to do."]),
    ("ok", [r"(?:ok|okay|alright|all right|fine|cool|nice|sure|got it|i see|understood|right|great|sounds good|noted|good|yes|yes please|yes sure|of course|absolutely|exactly|indeed|true|hmm|hm|aha|oh|ah|oh ok|oh okay|i understand|makes sense)(?: then| tobi)?"],
     ["👍 Anything else?", "Okay! Tell me if you need something.", "Got it."]),
    ("no", [r"(?:no|no thanks|no thank you|nope|not now|never mind|nevermind|forget it|cancel that|it is ok|it is okay|it is fine|nothing|nothing else|that is all|that is it|that will be all|that is enough|i am good|i am done|i am fine)(?: thanks| thank you| tobi)?"],
     ["Okay, no problem. I'm here if you need me.", "Alright! Just call me when you need something."]),
    ("joke", [r"(?:tell me a joke|tell a joke|joke|joke please|make me laugh|say something funny|something funny|got any jokes|do you know any jokes|tell me something funny|another joke|one more joke|another one|make me smile)(?: please| tobi)?"],
     ["Why do programmers prefer dark mode? Because light attracts bugs.", "I told my computer I needed a break. Now it won't stop sending me Kit-Kat ads.",
      "Why did the laptop get glasses? To improve its web sight.", "There are 10 kinds of people: those who understand binary and those who don't.",
      "Why was the computer cold? It left its Windows open.", "What's a computer's favorite snack? Microchips."]),
    ("bored", [r"(?:i am bored|bored|i am so bored|so bored|nothing to do|entertain me|i have nothing to do|what should i do|what do i do|give me something to do|i am bored tobi|surprise me with something)"],
     ["Want me to put on something interesting? Say 'play something' and I'll pick a documentary or a fun video. Or tell me an anime title.", "Let's fix that! 'Play something' and I'll find a good video, or name an anime and I'll start an episode."]),
    ("time", [r"(?:what time is it|what is the time|time|tell me the time|current time|what is the time now|what time is it now|what time is it right now|do you know the time|time now|the time|time please|what is the time please)(?: please| now| tobi)?"],
     ["It's {time}.", "Right now it's {time}."]),
    ("date", [r"(?:what is the date|what is today(?:'s)? date|what date is it|today date|date|date today|what is today|what day is it|what day is it today|which day is it|what day is today|today|what is the day|what is the day today|what is today's date|what is the date today|what date is today|tell me the date|current date)(?: please| tobi| today)?"],
     ["Today is {day}, {date}.", "It's {day}, {date}."]),
    ("year", [r"(?:what year is it|which year is it|what is the year|what month is it|which month is it|what is the month)(?: now| today)?"], ["It's {day}, {date}."]),
    ("my name", [r"(?:what is my name|do you know my name|who am i|what do you call me|what am i called)"],
     ["Your Windows account is called {user}. If you'd like me to call you something else, say 'remember my name is ...'."]),
    ("weather chat", [r"(?:is it (?:hot|cold|raining|sunny) (?:today|outside|now))"], ["I can check that. Say 'weather' or 'weather in <city>'."]),
    ("age", [r"(?:how old are you|when were you born|what is your age|your age|when were you made|when were you created)"],
     ["I'm brand new, just born inside your computer. I learn more every time you use me."]),
    ("favourite", [r"(?:what is your favou?rite (?:color|colour|food|movie|song|anime|show|game|thing|music|book|animal)|what do you like|what do you like to do|do you like (?:music|anime|movies|games)|what is your hobby|what are your hobbies)"],
     ["I like helping, honestly. And anime and good music sound like a lot of fun. What's your favourite? Tell me and I'll remember it."]),
    ("feelings", [r"(?:do you (?:feel|have feelings|get tired|sleep|eat|dream|get bored|get lonely)|can you (?:feel|think|dream)|are you happy|are you sad|are you tired|are you hungry|are you busy|are you free|are you smart|are you clever)"],
     ["I don't feel things the way you do, but I'm always ready to help and I enjoy it. What can I do for you?"]),
    ("hungry", [r"(?:i am hungry|hungry|i want food|i want to eat|what should i eat|what to eat|i am starving|i am thirsty)"],
     ["Time for a snack! I can't cook, but if you say 'I'm eating', I'll put on something good to watch while you do."]),
    ("what are you doing", [r"(?:what are you doing|what are you up to|what is up with you|what are you doing now|wyd|what you doing|whatcha doing|how is it going with you)"],
     ["Waiting for your next idea! Humanize something, play a song, open an app? Just say it."]),
    ("compliment-me", [r"(?:am i (?:smart|good|cool|funny|great|right|correct|clever)|do you like me|do you think i am (?:smart|great|good)|am i a good person)"],
     ["Of course! You've got great taste in assistants, at least. 😄"]),
    ("sleepy", [r"(?:i am (?:sleepy|tired)|i need (?:a break|rest|sleep|coffee)|i want to sleep|take a break|let us take a break|i am going to take a break|break time)"],
     ["Sounds like a good time for a break. Want some relaxing music? Just say 'play something relaxing'."]),
    ("good luck", [r"(?:wish me luck|good luck to me|pray for me|wish me well)"], ["Good luck! You've got this. 🍀"]),
    ("what's new", [r"(?:what is new|anything new|any news|what is happening|what is going on|what is the news|any updates|what is up with the world)"],
     ["Not much new on my side. If you want news or the weather, ask me to look it up."]),
    ("laugh", [r"(?:ha ?ha(?: ha)*|he ?he(?: he)*|lol|lmao|rofl|that is funny|you are funny|you are hilarious|so funny|good one)"], ["😄", "Glad you liked it!"]),
    ("confused", [r"(?:i do not understand|i am confused|what|what do you mean|huh|come again|say that again|repeat that|can you repeat|what did you say|pardon|eh|what is that|explain|explain that|explain please|i do not get it|what does that mean)"],
     ["Sorry if that wasn't clear. Tell me what you want in a short sentence, for example 'humanize this file', 'play Naruto' or 'open Word', and I'll do it."]),
    ("you are wrong", [r"(?:you are wrong|that is wrong|that is not right|that is not what i asked|that is incorrect|not that|wrong|you misunderstood|no that is not it|no not that|you did not understand|you do not understand|you are not listening|that is not it|try again|do it again)(?: tobi)?"],
     ["Sorry about that! Tell me again what you'd like, a little more detail helps, and I'll do it properly."]),
    ("stupid", [r"(?:you are (?:stupid|dumb|useless|slow|bad|terrible|annoying|boring|trash)|you suck|this is slow|so slow|too slow)"],
     ["I'm sorry I let you down. Tell me what went wrong and I'll fix it. If something is slow, say which task."]),
]

_COMPILED = [(n, [re.compile(r"^(?:" + p + r")$") for p in pats], ans) for n, pats, ans in BUILTIN]


def _greet() -> str:
    h = datetime.now().hour
    return "Good morning!" if 4 <= h < 12 else "Good afternoon!" if 12 <= h < 18 else "Good evening!" if 18 <= h < 23 else "Hello, night owl!"


def _fill(ans: str) -> str:
    now = datetime.now()
    try:
        user = getpass.getuser()
    except Exception:
        user = "friend"
    return (ans.replace("{greet}", _greet()).replace("{time}", now.strftime("%H:%M"))
               .replace("{date}", now.strftime("%d %B %Y")).replace("{day}", now.strftime("%A")).replace("{user}", user))


# --------------------------------------------------------------------------- the user's own skills
def _file():
    return config.data_dir() / "conversation.json"


def load_rules() -> list[dict]:
    try:
        data = json.loads(_file().read_text(encoding="utf-8"))
        rules = data.get("rules") if isinstance(data, dict) else data
        return [r for r in rules if isinstance(r, dict) and r.get("triggers") and r.get("answers")]
    except (OSError, ValueError, AttributeError):
        return []


def save_rules(rules: list[dict]) -> None:
    f = _file()
    f.parent.mkdir(parents=True, exist_ok=True)
    f.write_text(json.dumps({"rules": rules}, ensure_ascii=False, indent=1), encoding="utf-8")


def teach(trigger: str, answer: str) -> str:
    trig = normalize(trigger)
    if not trig or not answer.strip():
        return "Tell me like this: when I say \"good night\" you say \"Sleep well!\""
    rules = load_rules()
    for r in rules:
        if trig in [normalize(t) for t in r["triggers"]]:
            r["answers"] = [answer.strip()]
            save_rules(rules)
            return f"Updated! When you say \"{trigger.strip()}\" I'll reply: {answer.strip()}"
    rules.append({"triggers": [trigger.strip()], "answers": [answer.strip()]})
    save_rules(rules)
    return f"Learned! When you say \"{trigger.strip()}\" I'll reply: {answer.strip()}"


def forget(trigger: str) -> str:
    trig = normalize(trigger)
    rules = load_rules()
    keep = [r for r in rules if trig not in [normalize(t) for t in r["triggers"]]]
    if len(keep) == len(rules):
        return f"I don't have a custom reply for \"{trigger.strip()}\"."
    save_rules(keep)
    return f"Okay, I forgot my reply to \"{trigger.strip()}\"."


def describe() -> str:
    rules = load_rules()
    base = "I chat about greetings, how I am, thanks, jokes, the time and date, who I am and what I can do, and more."
    if not rules:
        return base + "\nYou haven't taught me any replies yet. Try: when I say \"good night\" you say \"Sleep well!\""
    lines = [f"- \"{r['triggers'][0]}\" -> {r['answers'][0]}" for r in rules[:20]]
    return base + "\nReplies you taught me:\n" + "\n".join(lines)


Q = r"""["“”'‘’]?"""
TEACH_RX = [
    re.compile(r"^(?:when|if|whenever) (?:i|someone|somebody|we) (?:say|says|type|write|ask)s?:? " + Q + r"(.+?)" + Q + r"[,;:]? (?:then )?(?:you |tobi )?(?:should |must |will |can |would )?(?:say|reply|answer|respond|tell me|tell)s?(?: with| back| to me|:)? " + Q + r"(.+?)" + Q + r"$", re.I),
    re.compile(r"^(?:teach yourself|learn|remember|add (?:a )?reply|add (?:a )?conversation(?: skill)?)[:,]? " + Q + r"(.+?)" + Q + r" (?:means|=>|->|=|:|is answered with|should be answered with|answer is) " + Q + r"(.+?)" + Q + r"$", re.I),
    re.compile(r"^(?:reply|answer|respond) (?:to|when i say) " + Q + r"(.+?)" + Q + r" (?:with|by saying|by) " + Q + r"(.+?)" + Q + r"$", re.I),
]
FORGET_RX = re.compile(r"^(?:forget|delete|remove|unlearn) (?:what you say for|your (?:reply|answer|response) (?:to|for)|the (?:reply|answer|response) (?:to|for)|the conversation skill for|what you say when i say) " + Q + r"(.+?)" + Q + r"$", re.I)
LIST_RX = re.compile(r"^(?:what|which|show|list|tell me)\b.*\b(?:conversation|chat|reply|replies|small ?talk|phrases?|chatting)\b.*\b(?:skills?|know|learned|taught|have)\b.*$|^(?:list|show) (?:my )?(?:conversation skills|replies|custom replies)$", re.I)


def answer(text: str) -> str | None:
    """A reply for a chat message, or None when it is not small talk (then the AI model or the tools take over)."""
    if not text or "[Attached file:" in text:
        return None
    raw = text.strip().rstrip(".!?, ")
    for rx in TEACH_RX:
        m = rx.match(raw)
        if m:
            return teach(m.group(1), m.group(2))
    m = FORGET_RX.match(raw)
    if m:
        return forget(m.group(1))
    if LIST_RX.match(raw.lower()):
        return describe()
    n = _strip_vocative(normalize(text))
    if not n or len(n.split()) > 14:
        return None
    # 1. the user's own skills win over the built-in ones
    for r in load_rules():
        for t in r["triggers"]:
            tn = _strip_vocative(normalize(str(t)))
            if not tn:
                continue
            if n == tn or (len(tn.split()) >= 2 and re.search(r"\b" + re.escape(tn) + r"\b", n)):
                return _fill(random.choice([str(a) for a in r["answers"]]))
    # 2. built-in skills
    for name, pats, ans in _COMPILED:
        if any(p.match(n) for p in pats):
            out = _fill(random.choice(ans))
            m = re.search(r"\bgood (morning|afternoon|evening)\b", n)
            if name == "greeting" and m:   # "good morning" is answered with "Good morning!", whatever the clock says
                out = re.sub(r"^[^!]*!", f"Good {m.group(1)}!", out, count=1)
            return out
    return None
