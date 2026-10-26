<p align="center"><img src="docs/images/hero.png" alt="WindyLand" width="100%"></p>

# WindyLand

A living notch for Windows 10 and 11. Music, synced lyrics, a timer, Pomodoro,
to-do reminders, a water reminder and a file shelf, all in a small pill at the
edge of your screen. Hover to open, move away to close.

<p align="center"><img src="docs/images/demo.gif" alt="Hover to open, lyrics, timer" width="780"></p>

## Features

| | |
|---|---|
| <img src="docs/images/lyrics.png" alt="Synced lyrics"> | <img src="docs/images/timer.png" alt="Timer"> |
| **Now playing + synced lyrics** from any media app | **Timer and stopwatch** with presets, alerts and laps |
| <img src="docs/images/todo.png" alt="To-do"> | <img src="docs/images/pomodoro.png" alt="Pomodoro"> |
| **To-do** with timed reminders | **Pomodoro** focus sessions |
| <img src="docs/images/water.png" alt="Water reminder"> | <img src="docs/images/shelf.png" alt="File shelf"> |
| **Water reminder** with an hourly chart | **File shelf**: drop in, drag out |

<img src="docs/images/positions.png" alt="Any edge">

<img src="docs/images/styles.png" alt="Notch styles">

<img src="docs/images/alerts.png" alt="Alerts">

Battery, network, Caps Lock, keyboard language, volume and Bluetooth alerts are
built in. Your data stays on your PC; the only network request is to
[LRCLIB](https://lrclib.net) for lyrics.

## Install

Download `WindyLandSetup-<version>.exe` from
[Releases](https://github.com/shabab239/WindyLand/releases) and run it. It
installs per user, no admin rights needed.

**Quick start:** hover the notch to open it, right-click it for settings,
right-click and drag to move it. See the **[User guide](docs/USER_GUIDE.md)**.

## Build

Needs Rust (stable, GNU or MSVC toolchain).

```powershell
cargo build --release                       # target\release\windyland.exe
.\target\release\windyland.exe --selftest   # headless checks
.\build.ps1                                 # build + icon + self-test + installer (Inno Setup 6)
python docs\tools\make_images.py           # regenerate these screenshots (Pillow)
```

## Adding a feature

Each feature is one file in `src/modules/` implementing the `Module` trait.
Copy `src/modules/example.rs` and register it in `modules::all()`.

## License

[MIT](LICENSE)
