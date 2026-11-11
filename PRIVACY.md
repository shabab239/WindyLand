# Privacy policy

_Last updated: 11 November 2026. Applies to WindyLand 0.1.21 and later; the Widgets tab (weather, Bluetooth, Wi-Fi and sound widgets) comes in the next version._

WindyLand is a free, open-source app that runs on your PC. It has no
accounts, no ads, no analytics and no telemetry. The developer receives no
data about you or how you use it. You can check everything below in the
source code in this repository.

## What stays on your PC

Everything WindyLand keeps is stored as plain text files in
`%APPDATA%\WindyLand` on your own computer:

| File | What's in it |
|---|---|
| `settings.txt` | Your settings (style, position, mascot, sound, and so on) |
| `todo.txt` | Your to-do tasks and reminder times |
| `water.txt` | Your water reminder settings and the amounts you logged |
| `tools.txt` | The colors you picked and your converter choices (calculator history is not saved) |
| `prayer.txt` | Your prayer times settings: city or the coordinates you typed, method and reminders |
| `shelf.txt` | The paths of files you put on the shelf (not the files themselves) |
| `clipboard.txt` | Only clipboard entries you pinned, and snippets you saved |

Nothing in these files is sent anywhere. Uninstalling WindyLand asks whether
to delete this folder too.

## What WindyLand reads, and only uses on your PC

- **Now playing:** the title, artist, album art and playback position that
  Windows shares from media apps (Spotify, browsers and others).
- **Clipboard history** (off until you turn it on): what you copy. Unpinned
  entries and all pictures live only in memory and are gone when WindyLand
  exits. Copies that password managers mark as private are never recorded.
- **Show what I copy** (off until you turn it on): reads each new copy only
  to show a short preview in the notch for about two seconds. Nothing is
  stored, and copies that password managers mark as private are skipped.
- **Windows notifications** (off until you turn it on): the app name, icon
  and text of notifications in Windows' Notification Center, to show them in
  the notch. They are not stored or sent anywhere.
- **Mic and camera dots:** Windows' own record of which apps are using your
  microphone or camera. WindyLand never accesses the microphone or camera
  itself.
- **System status:** battery, network, volume, Caps Lock, keyboard language
  and Bluetooth state, to show alerts.
- **System monitor:** Windows' CPU, memory, network and disk counters, read
  once a second and kept only in memory (the last minute).
- **AI agents** (off until you connect an agent): when you switch on Hermes,
  Claude Code or Codex, WindyLand adds a hook to that agent's own settings
  (Hermes' `hooks:` through `hermes config set` plus its hook allowlist;
  `~/.claude/settings.json`, with a backup kept next to it;
  `~/.codex/config.toml`). The hook passes on only the event name,
  session id, project folder name, tool name and Claude's notice text;
  prompts, code and tool output are never read. Nothing is stored or sent
  anywhere, and switching it off (or uninstalling) removes the hook.
- **Screenshots** (only when you click Screenshot): saved as PNG files in
  your Pictures\Screenshots folder and listed on the Shelf.

## Bluetooth, Wi-Fi and sound widgets

The Widgets tab can show the battery of connected Bluetooth devices, your
Wi-Fi network and signal, and the speaker volume and microphone mute. They
are read from Windows on your PC, only while those widgets are on or the
Widgets tab is open. AirPods don't tell Windows their battery, so for them
WindyLand listens for about 3 seconds once a minute to the battery
broadcast AirPods send nearby. Nothing from any of this is saved or sent.

## When WindyLand goes online

WindyLand connects to only these services, and never sends personal
information to them:

1. **[LRCLIB](https://lrclib.net)** (lyrics), only while the lyrics view is
   open: the title, artist, album and length of the song that's playing, so
   it can find matching lyrics. LRCLIB's own privacy terms apply to that
   request.
2. **GitHub** (updates), shortly after start and about twice a day: it asks
   for the latest release number, and when you click **Update**, downloads
   the installer. GitHub's privacy statement applies to those requests.
   The automatic checks can be turned off in the installer ("Check for
   updates automatically") or in Settings > General; **Check for updates**
   still works when you click it.
3. **[Open-Meteo](https://open-meteo.com)** (weather), only while you have
   the **Weather** widget switched on in the Widgets tab: about every 30
   minutes it asks for the current weather at your place (Widgets > Place),
   with the latitude and longitude rounded to one decimal (about 10 km).
   It's off until you switch it on. When you search for a city on the Place
   page, the text you type goes to Open-Meteo's place search. Open-Meteo's
   own terms apply to those requests.

If you pick **Windows location** on the Place page, WindyLand asks Windows'
location service for your position (on your PC; Windows itself may use
your network to work it out). It's only used for the weather and, if you
choose, prayer times, and is saved in your settings file on your PC.

Like any internet request, these services can see your IP address.
WindyLand sends no identifiers, cookies or usage data with them.

## Children

WindyLand collects no personal information from anyone, including children.

## Changes

Changes to this policy are made in this file and are visible in its history
on GitHub.

## Contact

Questions? Open an issue at
[github.com/shabab239/WindyLand/issues](https://github.com/shabab239/WindyLand/issues).
