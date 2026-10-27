# Changelog

## 0.1.5

- Update check: shortly after start and twice a day WindyLand looks for a
  newer GitHub release and offers Update / Later / Skip. Update downloads
  the installer, upgrades in place and restarts. Also in the tray menu as
  Check for updates. About now shows the version.
- Bug report and feature request forms on GitHub.
- Fixed album art sometimes staying on the previous song: players publish
  the new title before the new cover, so the cover is now re-checked for a
  while after each track change.

## 0.1.4

- Fixed the Timer's -1 min button being cut off and the Water settings
  buttons being squashed.
- README screenshots, demo GIF and user guide (`docs/`), rendered from the
  real app with `--render-docs` and `docs/tools/make_images.py`.

## 0.1.3

- Put the notch on any screen edge (top, bottom, left, right) and anywhere
  along it, on any monitor. Right-click and drag the notch to move it, or use
  Position in the tray menu. Side edges get a vertical tab; panels stay upright.

## 0.1.2

- Timer tab: countdown with presets (1 min to 1 h), +/-1 min and scroll to
  adjust, chime and Restart/Dismiss toast when done. Stopwatch with laps.
  Both show in the collapsed notch while running.
- The self-test can now run while the app is open.

## 0.1.1

- Notch style setting in the tray menu: Full, Compact, Pill (thin bar, tinted
  while music or a timer is live) and Hidden (hover the top edge).
- Smaller styles open only after the mouse rests briefly, so reaching for
  browser tabs doesn't open the notch.

## 0.1.0

- First release: notch window, now playing, synced lyrics, Pomodoro, water
  reminder, to-do, file shelf, system overlays, mascot, tray menu, installer.
