# Changelog

## 0.1.18

- Windows notifications in the notch (opt-in: Settings > Privacy).
  Other apps' notifications drop out of the notch with the app's icon and
  colour; click to open the app, or mute an app. Message text can be hidden
  (app name only). Notifications that arrive during Do Not Disturb or focus
  (which Windows 11 turns on by itself for games and full-screen apps), or
  during a full-screen game or video, wait and then show as one ("+2 more").
  Windows' own banner keeps showing unless you turn banners off, and
  Settings links straight there. Uses Windows' notification listener (no
  signing or packaging needed) and only looks when Windows' notification
  database changes, so it costs nothing while nothing arrives. App icons
  come from the shell (the app's Start icon), which works for Store apps like
  WhatsApp and desktop apps alike, and a watchdog restarts the watcher if a
  call into Windows ever stops responding.
- Prayer times panel. Times are calculated on your PC from the sun's
  position (nothing is fetched): pick one of 16 Bangladesh cities or type a
  latitude and longitude, choose the method (Karachi, MWL, ISNA, Egypt,
  Makkah) and Shafi or Hanafi Asr, and nudge any prayer by a few minutes to
  match your mosque. Shows the next prayer with a countdown; optional
  reminders per prayer (on time or 5 to 15 minutes before) and a countdown in
  the closed notch for the last 15 minutes. Reminders are off until you turn
  them on.
- Settings > Panels: show or hide any panel. A hidden panel is fully off (no
  tab, no reminders, and clipboard history stops recording). One panel always
  stays.
- The mouse wheel on the open notch switches panels (down for the next).
  Lists that can scroll (to-do, clipboard, lyrics) still scroll, and the
  wheel over the tab bar always switches. The timer's hidden "wheel changes
  the duration" is gone (it has buttons).
- Mascot: **Always** shows it all the time, not only when idle, and
  **Left / Center / Right** places it in the notch, with the clock or the
  live activity (music, timer, prayer countdown) beside it.
- The welcome tour has a new last step, Settings at a glance (style, mascot,
  sounds, panels, notifications, clipboard, prayer times, updates), with an
  **Open Settings** button, and its tips now include the mouse wheel.
- The README demo starts with the welcome tour and shows the mouse wheel,
  the prayer panel and reminder, mascot placement and the Panels page.
- Settings has three pages on the right: General, Privacy (notifications,
  mic and camera dots, clipboard history) and Panels.
- Privacy policy (PRIVACY.md), linked from Settings > General and the
  README.
- Fixed animations stalling to a few frames a second on an otherwise idle
  screen: frames were paced with DwmFlush, which waits for the next desktop
  composition (about 250 ms when nothing else changes). Frames now sleep
  until the display's next refresh, from the compositor's timing.
- New README demo video and screenshots, rebuilt from the real app by
  `docs/tools/make_images.py` (`windyland --render-demo` plays a scripted
  session on a virtual clock).

## 0.1.17

- Update checks keep working when GitHub's API refuses: it allows only 60
  requests an hour per internet address, and many ISPs share one address
  between lots of customers, so checks could silently fail for days. WindyLand
  now falls back to the normal release pages, and retries a failed check
  within the hour instead of 12 hours later.
- The downloaded installer is checked against the SHA-256 that GitHub
  publishes for it before it runs.
- Installers larger than 4 MB no longer fail to download.

## 0.1.16

- People updating from a version without the welcome tour now see it once
  too (it was only shown on brand-new installs). Their current style and
  position are pre-selected, so nothing changes unless they pick.

## 0.1.15

- Much smoother animation. Frames were timed with a Windows timer, which
  ticks in ~15.6 ms steps: tab switches ran at about 21 fps and opening the
  notch at about 63 fps. Transitions (tab switch, highlight glide, opening,
  closing, notices) now draw once per display refresh, so 144 fps on a 144 Hz
  screen, and live content (lyrics, progress, stopwatch) uses a precise
  60 fps timer. Nothing changes when idle: no frames are drawn.
- With WINDYLAND_DEBUG=1, each animation's frame rate and worst gap is
  written to debug.log.

## 0.1.14

- The selected tab icon no longer moves at all: it stays still and only fades
  from its tab colour to white, while the highlight glides to it.

## 0.1.13

- The newly selected tab icon now makes one small, smooth hop instead of
  growing and shrinking (which looked shaky).

## 0.1.12

- Choose the reminder sound in Settings: Chime (as before), Bell, Pop,
  Marimba, your Windows notification sound, or Off. Picking one plays it.
- The self-test no longer plays sounds while building.

## 0.1.11

- Fixed the notch (especially the Pill) sometimes vanishing until the Start
  menu was opened. Windows' "busy" signal also fires while the desktop has
  focus and for some normal windows; WindyLand now hides only when a window
  really fills the screen (games, full-screen video, presentations), never
  for the desktop, Start, the taskbar or a maximized window, and re-checks the
  moment another window comes to the front.
- Fixed reminders chiming while their notice was hidden. A notice that
  arrives while the notch is hidden now waits, and chimes when it actually
  appears. The notch also re-raises itself above other always-on-top windows
  when a notice shows or it opens.

## 0.1.10

- Welcome tour on first run, inside the notch: 1. pick how the closed notch
  looks (Full, Compact, Pill or Hidden, each with a little preview),
  2. pick the edge, 3. three quick tips. Choices apply straight away, and the
  last notice says where to change them later. Skip any time; replay it from
  the tray menu (Welcome tour).

## 0.1.9

- Every tab now opens to the same panel size, so the tab bar and buttons
  stay put while switching (only Music's lyrics view is wider).
- Switching tabs slides the old content out and the new content in from the
  side you moved toward. The highlight glides to the new tab, which glows in
  its own colour and pops briefly.
- Clipboard history is now off until you turn it on (in the tab or in
  Settings), and records text only by default. Settings offers Text,
  Text + files, or Everything, which adds images with thumbnails (images
  stay in memory only and can't be pinned).
- Mic and camera dots: an orange (microphone) or green (camera) dot appears
  beside the notch while any app is using them, with a short notice naming
  the app. Turn it off in Settings.

## 0.1.8

- Clipboard history tab: text and files you copy are listed (newest first,
  up to 30). Click one to copy it again, pin the ones you want to keep across
  restarts, remove or clear the rest, or pause recording. Unpinned entries
  are kept in memory only. Copies that password managers mark as private are
  never recorded.
- The tab bar header drops the date, then the title, when a narrow panel has
  no room, instead of overlapping.

## 0.1.7

- Settings panel inside the notch: click the gear at the end of the tab bar,
  right-click the notch, or left-click the tray icon. It has notch style,
  position, mascot, Start with Windows, volume overlay, updates, data folder,
  GitHub page and Quit. The tray menu is now just Settings, Check for
  updates, About and Exit.
- Mascots: pick Eyes, Cat, Ghost, Robot, Sparky, Sprout, Owl or Panda (or
  Clock for none). They all follow the mouse, blink, doze and wave. Add your
  own in `src/mascot.rs`.
- Smoother resizing when switching tabs: the notch now moves on a soft
  spring, the tab bar glides with it, and the new tab's content fades in
  instead of popping.

## 0.1.6

- Fixed toast buttons (timer Restart/Dismiss, reminders, Pomodoro) being hard
  to click: moving the mouse onto a toast used to open the full panel on top
  of it. A toast with buttons now stays while hovered, and after clicking one
  of its buttons the panel only opens again once the mouse has left.
- Tooltips: resting the mouse on an icon-only button (tabs, music controls,
  undo, settings, delete, +/-) shows what it does.
- `Ui::icon_btn` takes a tooltip, and `Ui::hit_tip` adds one to any hit.

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
