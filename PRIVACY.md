# Privacy policy

_Last updated: 27 October 2026. Applies to WindyLand 0.1.18 and later._

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
| `prayer.txt` | Your prayer times settings: city or the coordinates you typed, method and reminders |
| `shelf.txt` | The paths of files you put on the shelf (not the files themselves) |
| `clipboard.txt` | Only clipboard entries you pinned |

Nothing in these files is sent anywhere. Uninstalling WindyLand asks whether
to delete this folder too.

## What WindyLand reads, and only uses on your PC

- **Now playing:** the title, artist, album art and playback position that
  Windows shares from media apps (Spotify, browsers and others).
- **Clipboard history** (off until you turn it on): what you copy. Unpinned
  entries and all pictures live only in memory and are gone when WindyLand
  exits. Copies that password managers mark as private are never recorded.
- **Windows notifications** (off until you turn it on): the app name, icon
  and text of notifications in Windows' Notification Center, to show them in
  the notch. They are not stored or sent anywhere.
- **Mic and camera dots:** Windows' own record of which apps are using your
  microphone or camera. WindyLand never accesses the microphone or camera
  itself.
- **System status:** battery, network, volume, Caps Lock, keyboard language
  and Bluetooth state, to show alerts.

## When WindyLand goes online

WindyLand connects to only two services, and never sends personal
information to either:

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
