<p align="center"><img src="docs/images/hero.png" alt="WindyLand" width="100%"></p>

# WindyLand

A living notch for Windows 10 and 11, like the Dynamic Island. Music with
synced lyrics, your Windows notifications, a timer, Pomodoro, to-do
reminders, a water reminder, a file shelf, clipboard history and mic and
camera in-use dots, all in a small pill at the edge of your screen. Hover to
open, move away to close. Free and open source.

<p align="center"><img src="docs/images/demo.gif" alt="WindyLand demo: notch styles, music and lyrics, every tab, notifications, settings" width="900"></p>

## Features

| | |
|---|---|
| <img src="docs/images/lyrics.png" alt="Synced lyrics"> | <img src="docs/images/notifications.png" alt="Windows notifications"> |
| **Now playing + synced lyrics** from any media app | **Windows notifications** in the notch, with the app's icon |
| <img src="docs/images/timer.png" alt="Timer"> | <img src="docs/images/pomodoro.png" alt="Pomodoro"> |
| **Timer and stopwatch** with presets, alerts and laps | **Pomodoro** focus sessions |
| <img src="docs/images/todo.png" alt="To-do"> | <img src="docs/images/water.png" alt="Water reminder"> |
| **To-do** with timed reminders | **Water reminder** with an hourly chart |
| <img src="docs/images/shelf.png" alt="File shelf"> | <img src="docs/images/clipboard.png" alt="Clipboard history"> |
| **File shelf**: drop in, drag out | **Clipboard history**: text, files or images, pin to keep |

<img src="docs/images/styles.png" alt="Notch styles">

<img src="docs/images/alerts.png" alt="Alerts">

Battery, network, Caps Lock, keyboard language, volume and Bluetooth alerts are
built in, and a dot shows while any app uses your microphone or camera.

**Prayer times** are calculated on your PC (no internet), for 16 Bangladesh
cities or any latitude and longitude, with Karachi, MWL, ISNA, Egypt or Makkah
methods, Shafi or Hanafi Asr, per-prayer minute nudges and optional reminders.
Hide any panel you don't use in Settings > Panels, and roll the mouse wheel on
the open notch to switch panels.

<img src="docs/images/settings.png" alt="Settings">

| | |
|---|---|
| <img src="docs/images/mascot.png" alt="Mascots"> | <img src="docs/images/positions.png" alt="Any edge"> |
| **Mascots** that follow your mouse when nothing's on | **Any edge** of any screen |

Your data stays on your PC. The only network requests are to
[LRCLIB](https://lrclib.net) for lyrics and to GitHub to check for updates.
Notifications, clipboard history and the mic and camera dots are read
locally, and notifications and clipboard history are off until you turn them
on. See the [privacy policy](PRIVACY.md).

## Install

Download `WindyLandSetup-<version>.exe` from
[Releases](https://github.com/shabab239/WindyLand/releases) and run it. It
installs per user, no admin rights needed. A short welcome tour lets you pick
the look, and later versions install with one click from the notch.

**Quick start:** hover the notch to open it, right-click it for settings,
right-click and drag to move it. See the **[User guide](docs/USER_GUIDE.md)**.

**"Smart App Control blocked an app"?** Releases aren't code signed yet (see
[Code signing policy](#code-signing-policy)), and Windows 11's Smart App
Control blocks unsigned apps with no way to allow just one. Until signed
releases are out, the only way to install on such a PC is to turn Smart App
Control off in Windows Security > App & browser control. Note that on many
Windows versions it can't be turned back on without resetting Windows. The
blue "Windows protected your PC" screen is different: click **More info** >
**Run anyway**.

## Build

Needs Rust (stable, GNU or MSVC toolchain).

```powershell
cargo build --release                       # target\release\windyland.exe
.\target\release\windyland.exe --selftest   # headless checks
.\build.ps1                                 # build + icon + self-test + installer (Inno Setup 6)
python docs\tools\make_images.py            # regenerate the screenshots and demo (Pillow, numpy, ffmpeg)
```

Releases are built by GitHub Actions from the version tag
(`.github/workflows/release.yml`).

## Code signing policy

Windows releases are being set up for free code signing provided by
[SignPath.io](https://about.signpath.io), certificate by
[SignPath Foundation](https://signpath.org). Signed releases start with the
first release after the project is accepted; until then installers are
unsigned.

- Only files built by this repository's GitHub Actions release workflow from
  its own source code are signed. Nothing built elsewhere is signed.
- Committers and reviewers: [Shabab](https://github.com/shabab239)
  (maintainer). Changes from others come in as pull requests and are reviewed
  before merging.
- Approvers: [Shabab](https://github.com/shabab239). Every signing request is
  approved by hand.
- Privacy: this program will not transfer any information to other networked
  systems unless specifically requested by the user or the person installing
  or operating it. The only online features (lyrics, and update checks that
  can be turned off in the installer or Settings) are described in the
  [privacy policy](PRIVACY.md).

## Adding a feature

Each feature is one file in `src/modules/` implementing the `Module` trait.
Copy `src/modules/example.rs` and register it in `modules::all()`.
Mascots live in `src/mascot.rs`: add a variant to `store::Mascot` and a draw
function.

## Credits

WindyLand was inspired by [Nusa](https://rama2555.github.io/nusa/), a
living notch for Windows by rama2555. WindyLand is written from scratch and shares no
code with Nusa.

## License

[MIT](LICENSE)
