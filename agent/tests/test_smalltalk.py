"""Conversation skills: built-in keywords/answers, skills taught by the user, and that they reach the chat without any AI model."""
import os, sys, tempfile
from pathlib import Path
HOME = tempfile.mkdtemp(prefix="wl-st-"); os.environ["WINDYLAND_AGENT_HOME"] = HOME
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from windyagent import smalltalk as st, router


def test_builtin():
    cases = {"hi": "ready|help|do for you", "Heyyyy Tobi!!": "ready|help|do for you", "hello tobi how are you?": "Ready|good|great|well|doing|How are you", "good morning": "Good morning",
             "thx": "welcome|Anytime|Happy|problem", "thank you so much": "welcome|Anytime|Happy|problem", "bye": "Bye|later|Goodbye|care",
             "what time is it": r"\d\d:\d\d", "what's the date today": r"\d{4}", "tell me a joke": "", "who are you": "Tobi", "what can you do": "Humanize",
             "help": "Humanize", "are you there?": "", "lol": "", "i'm bored": "play something|Play something", "good night": "", "sorry": "", "ok": "", "you are awesome": ""}
    import re
    for q, pat in cases.items():
        a = st.answer(q)
        assert a, q
        assert re.search(pat, a, re.I), (q, a)
    for q in ["open word", "play naruto", "humanize this file", "remind me in 20 minutes to call mom", "what is the capital of france", "weather in london",
              "write me an essay about climate change and its effects on farming", "pause", "[Attached file: C:\\a.docx] hi"]:
        assert st.answer(q) is None, (q, st.answer(q))
    print("built-in conversation skills OK")


def test_teaching():
    assert "Learned" in st.answer('when I say "ping pong" you say "table tennis time!"')
    assert st.answer("ping pong") == "table tennis time!" and st.answer("Ping   Pong!!") == "table tennis time!"
    assert "Updated" in st.answer("when I say ping pong, reply: nice rally")
    assert st.answer("ping pong") == "nice rally"
    assert "Learned" in st.answer("if i say good night you say sleep well {user}")
    assert st.answer("good night").startswith("sleep well ") and "{user}" not in st.answer("good night")          # own skills beat the built-in ones
    assert "Learned" in st.answer("teach yourself: open sesame => the cave is open")
    assert st.answer("hey tobi, open sesame!") == "the cave is open"
    assert "ping pong" in st.answer("what conversation skills do you have")
    assert "forgot" in st.answer('forget what you say for "ping pong"') and st.answer("ping pong") is None
    assert "don't have" in st.answer("forget what you say for nonsense words")
    # by editing the file directly
    import json
    (Path(HOME) / "conversation.json").write_text(json.dumps({"rules": [{"triggers": ["wassup bro", "yo bro"], "answers": ["Yo! Ready."]}]}), encoding="utf-8")
    assert st.answer("yo bro") == "Yo! Ready." and st.answer("Wassup bro?") == "Yo! Ready."
    (Path(HOME) / "conversation.json").write_text("{not json", encoding="utf-8")
    assert st.answer("hi")                                                    # a broken file never breaks the built-ins
    print("teaching conversation skills OK")


def test_reaches_the_chat_without_ai():
    from windyagent import core
    hub = core.Hub()
    app = core.AgentCore(hub)
    app._save = lambda: None
    app.busy = True
    app._run("thanks a lot!")
    ev = [e for e in hub._events]
    kinds = [e["type"] if "type" in e else e.get("kind") for e in ev]
    assert "assistant" in kinds and "error" not in kinds, ev            # answered, and no 'no AI model set up' error
    app.busy = True; app._run("when I say \"sing\" you say \"la la la\"")
    app.busy = True; app._run("sing")
    assert [e for e in hub._events if e.get("text") == "la la la"], hub._events
    print("small talk answers inside the chat with no AI model set up OK")


if __name__ == "__main__":
    test_builtin(); test_teaching(); test_reaches_the_chat_without_ai()
