# Tobi, your AI assistant

The **Tobi** tab in the notch is a chat: type in the field and press Enter, see his answers and what he is doing, press Allow or No when he asks permission, drop a file on the tab to give it to him. **Window** opens the bigger chat window (the tray menu has it too). It opens a small chat window right
under the notch where you talk to Tobi or give him commands. Tobi runs on your PC, uses the AI model you chose during setup
(OpenAI by default) and does things with a set of tools. **Sleep** in the Tobi tab shuts him down; **Settings** opens his settings.

**Giving Tobi files:** drag a file into his chat window, paste one, use the paperclip, or drop it on the notch while the
Tobi tab is showing. Files dragged into the chat are copied to `Documents\Tobi\Uploads`. When a Word file is attached, a
**Humanize this file** button appears.

## Things to say

| You type | What happens |
|---|---|
| `Play Naruto` (any anime) | Searches **animesalt.cx**, opens season 1 episode 1 in a browser window and presses play. `Play Naruto season 2 episode 5`, `play the latest One Piece` work too. |
| `Play Believer by Imagine Dragons` | Finds the song on **YouTube** and plays it. Also `play lo-fi beats`, `play a 1 hour rain sound`. |
| `I'm eating, play something` / `Play something` | Picks something interesting on YouTube: a documentary or long fascinating video, preferring topics you told Tobi you like, and never the same one twice. |
| `Pause` / `next` / `mute` / `volume up a bit` | Presses the media keys. |
| `Humanize this file` | Humanizes the body text of the Word file with stealthwriter.ai (see below). Tobi remembers every job; ask `what have you humanized?` or `continue the unfinished one`. |
| `Remind me in 20 minutes to stretch` | A reminder pops out of the notch (with a chime) even when the chat is closed. |
| `What's the weather in Dhaka?` | Current weather and a 3 day forecast. |
| `I love space documentaries` | Tobi remembers it (and uses it for "play something"). `What do you know about me?`, `forget the space thing`. |
| `Tidy my Downloads` | Moves loose files into Pictures / Videos / Music / Documents / Archives / Programs / Other sub-folders after you approve. Nothing is deleted. |
| `Lock my PC` / `shut down in 30 minutes` / `cancel the shutdown` | Power controls. Sleep, shutdown and restart need your approval. |
| `What is using my RAM?`, `Open Spotify`, `Find the 10 largest files in Downloads`, `Take a screenshot`, `Search the web for ...` | System info, apps, files, screenshots, clipboard, web search. |
| anything else | Tobi can run PowerShell commands, but asks you first. |

## Safety

- Tobi asks for approval (Allow / Deny in the chat) before it runs a command, changes an existing file, "deletes" a file
  (it moves it to `%APPDATA%\WindyLand\agent\trash`, nothing is permanently deleted), stops a program, tidies a folder or
  shuts the PC down. *Settings > Approvals > Never ask* turns this off. Only do that if you are sure.
- The chat window only works through a secret token and only on this PC (127.0.0.1). Other websites cannot talk to it.
- Text from web pages and files is treated as data, not as instructions.
- Your API key is stored in `%APPDATA%\WindyLand\agent\config.json`, readable only by your Windows user.
  It is sent only to the AI provider you chose.
- Tobi never saves passwords or card numbers to his memory (he is told not to). See what he knows with `What do you know about me?`.

## Anime and music

- YouTube: Tobi reads YouTube's search page, chooses the best match and opens that video, so it starts playing at once.
- Anime: Tobi uses the anime site's own search, opens the episode page in a separate browser window and clicks the player
  to start it. The site shows ads and pop-ups on its own; Tobi closes the extra tabs the click opens, but you may still have to
  press play once. If the browser cannot be controlled, Tobi opens the page in your normal browser instead.
  The site address can be changed in *Settings > Anime site* if it moves.
- Only use sites that are legal where you live. Tobi just opens the page you asked for.

## The humanizer

1. Tobi opens a window of your browser (Chrome, Edge, Brave, Vivaldi or Opera; *Settings > Browser*, default: the one Windows uses for links) on stealthwriter.ai, with its own browser profile.
2. **It works in the background:** you can use other windows meanwhile (do not minimize or close Tobi's browser window).
3. The first time, **sign in with Google yourself** in that window. Tobi never types your password. The login is remembered.
3. He starts at the **first large paragraph** of the text (an Introduction heading is not needed; cover pages, tables, titles,
   captions and the contents are skipped, and he stops before the references). **Lists, bold lead-ins ("Time.") and short sentences are
   humanized too**; a bold lead-in stays bold and untouched. Only paragraphs with images, footnotes, equations or tracked changes are left
   alone, and the chat log says why for each. Each paste has **at least 800 words** (several sections are joined; at most about 900),
   then he selects Level 10, pastes and clicks Humanize.
4. **Rehumanize until 90%:** he reads the *Detection Score* box. If it already says 90% human or more he keeps the result;
   otherwise he presses **Rehumanize** (at most 8 times, then he goes on with the best he has). `site.target_score` changes the 90.
5. He then goes through **every sentence**: opens its "Choose alternative" window and reads all options (the dot next to each one is
   green = reads as human, red = still flagged). The AI **marks every option from 1 to 10** for natural sound, correct grammar and
   unchanged meaning, and a sentence is only taken when an option gets **8 or more**; otherwise he presses "New alternatives" and
   marks again. Options that lose a number, citation, acronym or name, change the length too much or are broken are thrown out before
   marking. **Tobi never rewrites the chosen sentence himself**: it is exactly the option stealthwriter offered. A green sentence with no
   good option stays as it was; **no red sentence may remain** (he keeps asking for new alternatives, and on the last try accepts a 6+).
   The judge is the `claude` command line tool if installed, else Tobi's own model (a small local model judges poorly: use Claude or
   OpenAI for the best sentences). Every changed sentence is written into your document in **red**. The original is kept as
   `<name>.original.docx`.
6. **Claude reviews the whole document, in one chat:** after *all* of the document is humanized and saved, Claude gets the full text and
   only **names the weakest sentences**. It does not reword anything (`claude_check.mode` "flag", the default): those sentences are
   humanized again on stealthwriter and the best option is chosen the same way. Set `claude_check.mode` to `"apply"` if you want
   Claude's own small edits instead. Details go to `<name>.sentence-check.txt`. Order of tools: `claude` command line tool, then
   claude.ai in his browser window with your account (90 seconds at most), then ChatGPT (chatgpt.com), then the AI model you set up (Ollama, OpenAI...). *Settings > Review* can skip Claude.
7. **academi.cx loop:** Tobi uploads the finished Word file to academi.cx (sign in there yourself once, in the same browser window).
   If the AI score is **not 0%**, he asks you: **Remove the AI with Tobi** or **I'll do it myself** (then you get the edited file and the report). On the first answer he scrolls through the report, reads the **blue (high likelihood)** and **purple (medium likelihood)**
   sentences, humanizes exactly those again on stealthwriter (choosing the best options as above), **saves the Word file and checks it on
   academi.cx again**. This repeats up to 3 rounds (`academi.max_rounds`) or until 0% (`academi.target_ai_percent`). Only the newest
   report is kept, as `<name>_AI-score.pdf` next to your file.
8. You get the Word file and the AI score report as cards in the chat (Open / Download), and a notice pops out of the notch.
   If the free daily limit is reached he saves his progress; say `Humanize this file` again tomorrow and he continues.

**The red stop button** pauses the job and asks whether you want the work done so far; yes gives you `<name>_partial.docx`.

Say `Humanize this file without academi` or `... without the Claude check` to skip a step. academi.cx's free plan only takes short papers
(about 350 words, one a day); a longer file needs their paid plan.

The website's page layout was built from screenshots. If stealthwriter.ai changes or a step cannot find a button, the chat shows
what failed and screenshots are saved to `%APPDATA%\WindyLand\agent\humanizer_debug`. Check that your school or employer
allows AI-assisted writing tools before you submit work made with them.

## Talking to Tobi

Everyday chat is answered at once, without the AI model: greetings, "how are you", thanks, jokes, the time and date, "who are you",
"what can you do", good night, bye... Typos and slang are fine ("heyyy", "thx", "wassup").

Teach him your own replies by talking to him:

    when I say "good night" you say "Sleep well, {user}!"
    forget what you say for "good night"
    what conversation skills do you have

or edit `%APPDATA%\WindyLand\agent\conversation.json`:
`{"rules": [{"triggers": ["good night", "gn"], "answers": ["Sleep well!", "Night night!"]}]}` (several answers = he picks one at random;
`{time}`, `{date}`, `{day}`, `{user}` are filled in). Your replies win over the built-in ones. Anything that is not chat goes to the
tools or the AI model as before.

## The knowledge folder

The setup creates **Documents\Tobi\Knowledge** (the folder button next to the paperclip in the notch chat, or the Start Menu shortcut
"Tobi Knowledge Folder", opens it). Put **Word, Excel, PDF, PowerPoint, text, Markdown and CSV files** in it whenever you like. Tobi
reads them, stores what is in them (`knowledge_index.json` in his data folder; nothing leaves your PC) and notices new, changed and
deleted files by himself. Then ask: "What does my contract say about the deadline?", "Search my notes for Valentina", "What's in my
knowledge folder?", "Re-read the knowledge folder". Only the few matching passages are shown to the AI model. Scanned PDFs without
text cannot be read. To use another folder, set `knowledge_folder` in `config.json`.

## Choosing the AI model

Settings (gear icon in the chat) has the provider (OpenAI, Claude, Gemini, OpenRouter, Ollama, or any OpenAI-compatible
server), the model name and the API key. *Test connection* checks it. The default OpenAI model is `gpt-4.1`.

## Files

| | |
|---|---|
| `<install folder>\agent\` | Tobi's program (Python) and its private `venv` |
| `%APPDATA%\WindyLand\agent\config.json` | settings and API key |
| `%APPDATA%\WindyLand\agent\memory.json` | what Tobi remembers about you (edit or delete it any time) |
| `%APPDATA%\WindyLand\agent\humanize_history.json` | past humanizing jobs |
| `%APPDATA%\WindyLand\agent\reminders.json` | reminders waiting to fire |
| `%APPDATA%\WindyLand\agent\history.json` | the current chat |
| `%APPDATA%\WindyLand\agent\agent.log` | log, useful when something goes wrong |
| `Documents\Tobi\Uploads` | files you dragged into the chat |

## For developers

- Rust side: `src/modules/agent.rs` (the tab, the button, files dropped on the notch, reminder pop-outs read from `notify.txt`),
  `src/tray.rs` (menu entry).
- Python side: `agent/windyagent/` — `llm.py` (Claude and OpenAI-style APIs), `tools.py` (registry and the PC tools),
  `media.py` (YouTube, anime, media keys), `home.py` (weather, reminders, power, tidy), `memory.py` (notes, logbooks, notch notices),
  `core.py` (the loop and Tobi's prompt), `server.py` + `web/index.html` (the chat window), `launcher.py` (opens and places the popup),
  `humanizer/` (the stealthwriter workflow).
- Add a tool: write a function `fn(ctx, ...)` and add an entry to `TOOLS` in `tools.py`. Risky tools call `ctx.confirm(...)`.
- Tests: `python agent/tests/test_agent.py` (runs the real server and loop against fake AI servers; the media tests use
  offline copies of YouTube's and the anime site's page structure).
