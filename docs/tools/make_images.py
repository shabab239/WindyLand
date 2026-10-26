"""Build the README / user-guide images from the real app.

    python docs/tools/make_images.py            (after `cargo build --release`)

1. Seeds demo data (tasks, water log, shelf files, album art) in a temp folder.
2. Runs `windyland.exe --render-docs`, which renders every view off-screen
   with a fixed 16:20 clock and a made-up song.
3. Composes the renders into docs/images/*.png and a hover GIF.

Needs Pillow. Windows only (uses Segoe UI fonts).
"""

import datetime
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

LANCZOS = Image.Resampling.LANCZOS

ROOT = Path(__file__).resolve().parents[2]
EXE = ROOT / "target" / "release" / "windyland.exe"
OUT = ROOT / "docs" / "images"
FONTS = Path(os.environ.get("WINDIR", r"C:\Windows")) / "Fonts"


def font(name, size):
    return ImageFont.truetype(str(FONTS / name), size)


F_TITLE = lambda s: font("seguibl.ttf", s)
F_SEMI = lambda s: font("seguisb.ttf", s)
F_REG = lambda s: font("segoeui.ttf", s)

TEXT = (245, 245, 247)
DIM = (160, 164, 176)
ACCENT = (255, 159, 10)

# ---------------------------------------------------------------- demo data


def seed(data: Path):
    data.mkdir(parents=True, exist_ok=True)
    now = time.time()
    today = datetime.date.today().isoformat()
    (data / "settings.txt").write_text("style=full\npersonality=1\nvolume_overlay=1\n")
    tasks = [
        (1, 1, 0, today, "Revise the report"),
        (2, 0, now + 40 * 60, "", "Reply to Dina"),
        (3, 0, now + 100 * 60, "", "Send the weekly update"),
        (4, 0, 0, "", "Pick up groceries"),
        (5, 1, 0, today, "Water the plants"),
    ]
    (data / "todo.txt").write_text("".join(f"{i}\t{d}\t{due}\t{dd}\t{t}\t0\n" for i, d, due, dd, t in tasks))
    logs = [(9 * 60 + 10, 250), (10 * 60 + 5, 250), (11 * 60 + 30, 350), (12 * 60 + 45, 250), (14 * 60, 250), (15 * 60 + 20, 150)]
    water = "bottle=750\ngoal=2000\nsip=250\nstart=9\nend=21\nreminders=1\nsince_refill=400\n"
    water += "".join(f"log {today},{m},{ml}\n" for m, ml in logs)
    (data / "water.txt").write_text(water)
    files = data / "demo-files"
    files.mkdir(exist_ok=True)
    with open(files / "Beach trip.jpg", "wb") as f:
        f.truncate(2_516_582)
    with open(files / "Quarterly report.pdf", "wb") as f:
        f.truncate(348_160)
    (files / "Project files").mkdir(exist_ok=True)
    (files / "lrclib.net.url").write_text("[InternetShortcut]\nURL=https://lrclib.net\n")
    shelf = [files / "Beach trip.jpg", files / "Project files", files / "Quarterly report.pdf", files / "lrclib.net.url"]
    (data / "shelf.txt").write_text("".join(f"{p}\n" for p in shelf))


def album_art(path: Path):
    """A soft sunset with paper birds: original art for the demo track."""
    n = 512
    img = Image.new("RGB", (n, n))
    d = ImageDraw.Draw(img, "RGBA")
    for y in range(n):
        t = y / n
        d.line((0, y, n, y), fill=(int(255 * (0.98 - 0.3 * t)), int(150 + 60 * (1 - t) - 90 * t), int(120 + 110 * t)))
    d.ellipse((150, 220, 362, 432), fill=(255, 226, 150, 255))
    d.rectangle((0, 360, n, n), fill=(40, 52, 110, 230))
    for k in range(5):
        y = 372 + k * 26
        d.line((0, y, n, y + 6), fill=(255, 200, 160, 70), width=4)
    for x, y, s in [(120, 120, 34), (190, 90, 26), (330, 140, 30), (390, 100, 22)]:
        d.polygon([(x - s, y), (x, y - s * 0.4), (x + s, y), (x, y + s * 0.15)], fill=(255, 255, 255, 220))
    img.save(path)


# ------------------------------------------------------------------- helpers


def load_raw(p: Path) -> Image.Image:
    b = p.read_bytes()
    w, h = struct.unpack("<II", b[:8])
    # "BGRa" = premultiplied BGRA; Pillow converts it to straight RGBA.
    return Image.frombuffer("RGBA", (w, h), b[8:], "raw", "BGRa", 0, 1).copy()


def wallpaper(w, h, seed=0):
    """Deep sky gradient with soft wind-like light blobs."""
    base = Image.new("RGB", (w, h))
    d = ImageDraw.Draw(base)
    top, bottom = (24, 32, 72), (12, 74, 98)
    for y in range(h):
        t = y / max(1, h - 1)
        d.line((0, y, w, y), fill=tuple(int(top[i] + (bottom[i] - top[i]) * t) for i in range(3)))
    glow = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    g = ImageDraw.Draw(glow)
    blobs = [(0.15, 0.85, 0.45, (90, 200, 220, 90)), (0.8, 0.25, 0.4, (150, 110, 255, 80)), (0.55, 1.0, 0.5, (40, 180, 160, 70))]
    for i, (cx, cy, r, c) in enumerate(blobs):
        cx = (cx + 0.07 * seed * (1 if i % 2 else -1)) % 1.0
        R = r * max(w, h)
        g.ellipse((cx * w - R, cy * h - R * 0.6, cx * w + R, cy * h + R * 0.6), fill=c)
    glow = glow.filter(ImageFilter.GaussianBlur(max(w, h) / 9))
    base.paste(glow, (0, 0), glow)
    return base  # RGB, so RGBA drawing blends instead of overwriting


def rounded_mask(w, h, r):
    m = Image.new("L", (w, h), 0)
    ImageDraw.Draw(m).rounded_rectangle((0, 0, w - 1, h - 1), r, fill=255)
    return m


def screen_card(w, h, seed=0, title_bar=True):
    """A mock screen: wallpaper plus a faint window strip at the very top."""
    card = wallpaper(w, h, seed)
    if title_bar:
        d = ImageDraw.Draw(card, "RGBA")
        d.rectangle((0, 0, w, 44), fill=(20, 22, 30, 200))
        for i, label in enumerate(["Inbox", "Docs", "Music"]):
            x = 20 + i * 190
            d.rounded_rectangle((x, 8, x + 170, 40), 9, fill=(255, 255, 255, 18))
            d.text((x + 16, 12), label, font=F_REG(18), fill=(200, 205, 215, 200))
    return card


def shadow(img, radius=24, alpha=150):
    a = img.split()[3].point(lambda v: alpha if v > 8 else 0)
    pad = radius * 2
    big = Image.new("L", (img.width + 2 * pad, img.height + 2 * pad), 0)
    big.paste(a, (pad, pad))
    return big.filter(ImageFilter.GaussianBlur(radius)), pad


def paste_notch(card, notch, x, y, with_shadow=True):
    if with_shadow:
        mask, pad = shadow(notch, 18, 120)
        card.paste((0, 0, 0), (x - pad, y - pad + 8), mask)
    card.paste(notch, (x, y), notch)


def page(w, h):
    return Image.new("RGB", (w, h), (13, 17, 23))


def chip(d, x, y, text, size=24, fg=TEXT, bg=(255, 255, 255, 22)):
    f = F_SEMI(size)
    tw = d.textlength(text, font=f)
    d.rounded_rectangle((x, y, x + tw + 32, y + size + 22), (size + 22) // 2, fill=bg)
    d.text((x + 16, y + 8), text, font=f, fill=fg)
    return x + tw + 32


def caption(img, x, y, title, body, tips=(), width=1400):
    d = ImageDraw.Draw(img, "RGBA")
    d.text((x, y), title, font=F_TITLE(54), fill=TEXT)
    y += 76
    for line in wrap(d, body, F_REG(28), width):
        d.text((x, y), line, font=F_REG(28), fill=DIM)
        y += 40
    if tips:
        y += 14
        cx = x
        for t in tips:
            if cx + d.textlength(t, font=F_SEMI(22)) + 40 > x + width:
                cx = x
                y += 58
            cx = chip(d, cx, y, t, 22, fg=TEXT) + 12
    return y


def wrap(d, text, f, width):
    words, lines, cur = text.split(), [], ""
    for w in words:
        nxt = (cur + " " + w).strip()
        if d.textlength(nxt, font=f) > width and cur:
            lines.append(cur)
            cur = w
        else:
            cur = nxt
    if cur:
        lines.append(cur)
    return lines


def save(img, name):
    OUT.mkdir(parents=True, exist_ok=True)
    img.convert("RGB").save(OUT / name, optimize=True)
    print("wrote", name, img.size)


# ---------------------------------------------------------------- compose


def feature(raw, name, render, title, body, tips, scale=0.8, card_h=None):
    notch = raw[render]
    notch = notch.resize((int(notch.width * scale), int(notch.height * scale)), LANCZOS)
    W = 1600
    ch = card_h or max(420, notch.height + 110)
    card = screen_card(1440, ch, seed=hash(name) % 5)
    paste_notch(card, notch, (card.width - notch.width) // 2, 0)
    img = page(W, ch + 360)
    img.paste(card, (80, 60), rounded_mask(card.width, card.height, 28))
    caption(img, 90, ch + 110, title, body, tips)
    save(img, name)


def main():
    if not EXE.exists():
        sys.exit(f"build first: {EXE} not found")
    tmp = Path(tempfile.mkdtemp(prefix="windyland-docs-"))
    data, raws = tmp / "data", tmp / "raw"
    seed(data)
    album_art(tmp / "art.png")
    env = dict(os.environ, WINDYLAND_DATA_DIR=str(data), WINDYLAND_DEMO_ART=str(tmp / "art.png"))
    subprocess.run([str(EXE), "--render-docs", str(raws)], env=env, check=True)
    print((raws / "render.log").read_text().strip())

    raw = {p.stem: load_raw(p) for p in raws.glob("*.raw") if not p.stem.startswith("anim_")}
    if OUT.exists():
        shutil.rmtree(OUT)

    # Hero: lyrics split panel.
    hero = screen_card(1600, 900, seed=1, title_bar=False)
    n = raw["panel_lyrics"]
    n = n.resize((int(n.width * 0.68), int(n.height * 0.68)), LANCZOS)
    paste_notch(hero, n, (1600 - n.width) // 2, 0)
    d = ImageDraw.Draw(hero, "RGBA")
    d.text((90, 540), "WindyLand", font=F_TITLE(110), fill=TEXT)
    d.text((96, 680), "A living notch for Windows.", font=F_SEMI(40), fill=(220, 230, 240))
    x = 96
    for t in ["Music + lyrics", "Timer", "Pomodoro", "To-do", "Water", "File shelf"]:
        x = chip(d, x, 760, t, 24, bg=(255, 255, 255, 30)) + 12
    save(hero, "hero.png")

    feature(raw, "music.png", "panel_music", "Now playing",
            "Works with any app in the Windows media overlay: Spotify, YouTube in your browser, Media Player and more.",
            ["Hover the notch to open", "Click the bar to seek", "Prev / Play / Next"])
    feature(raw, "lyrics.png", "panel_lyrics", "Synced lyrics",
            "Tap the lyrics button and the panel splits in two. The current line lights up as it is sung.",
            ["Click a line to jump there", "Scroll to browse", "Lyrics by LRCLIB"], scale=0.66)
    feature(raw, "timer.png", "panel_timer", "Timer",
            "Pick a preset or nudge the time, then start. The countdown stays visible in the closed notch and chimes when done.",
            ["Presets: 1 min to 1 h", "+/- 1 min while running", "Scroll to adjust", "Restart from the alert"])
    feature(raw, "stopwatch.png", "panel_stopwatch", "Stopwatch",
            "Switch to Stopwatch for elapsed time with tenths and lap splits.",
            ["Start / Pause", "Lap", "Reset"])
    feature(raw, "pomodoro.png", "panel_pomodoro", "Pomodoro",
            "25 minute focus sessions with 5 minute breaks and a long break after every fourth.",
            ["Start / Pause / Skip", "Countdown in the notch", "Chime at each phase"])
    feature(raw, "todo.png", "panel_todo", "To-do with reminders",
            "Jot a task and give it a time. The notch taps you on the shoulder when it is due.",
            ["Add Task", "Click a task to edit", "Tick to complete", "Done tasks clear the next day"])
    feature(raw, "todo-editor.png", "panel_todo_editor", "Add a task with a time",
            "Type a title, tap +15m / +30m / +1h / +2h or fine-tune with - and +, then press Enter.",
            ["Enter saves", "Esc cancels", "No time = no reminder"])
    feature(raw, "water.png", "panel_water", "Water reminder",
            "Track what you drink against a daily goal. Reminders are spread across your active hours.",
            ["Drink logs a sip", "Undo the last one", "Hourly chart"])
    feature(raw, "water-settings.png", "panel_water_settings", "Water settings",
            "Set bottle size, goal, sip size and active hours. Refill prompts tell you when the bottle is probably empty.",
            ["Bottle 500 ml to 1.5 L", "Goal 1.5 to 3 L", "Hours"], )
    feature(raw, "shelf.png", "panel_shelf", "File shelf",
            "Drop files, folders or links on the notch to keep them close. Click to open, drag them back out into any app.",
            ["Drop to add", "Click to open", "Drag out", "Clear"])

    # Styles: four mini screens.
    img = page(1600, 760)
    d = ImageDraw.Draw(img, "RGBA")
    d.text((80, 50), "Notch styles", font=F_TITLE(54), fill=TEXT)
    d.text((80, 126), "Right-click the notch or the tray icon, then Notch style.", font=F_REG(28), fill=DIM)
    items = [("Full", "collapsed_full_music"), ("Compact", "collapsed_compact_music"), ("Pill", "collapsed_pill_music"), ("Hidden", None)]
    for i, (label, key) in enumerate(items):
        x, y = 80 + (i % 2) * 730, 200 + (i // 2) * 270
        card = screen_card(700, 220, seed=i)
        if key:
            n = raw[key]
            n = n.resize((int(n.width * 0.75), int(n.height * 0.75)), LANCZOS)
            paste_notch(card, n, (700 - n.width) // 2, 0, with_shadow=False)
        else:
            cd = ImageDraw.Draw(card, "RGBA")
            cd.text((350, 120), "Rest the mouse at the edge", font=F_REG(22), fill=(220, 230, 240, 200), anchor="mm")
        img.paste(card, (x, y), rounded_mask(700, 220, 20))
        chip(d, x + 16, y + 160, label, 22, bg=(0, 0, 0, 120))
    save(img, "styles.png")

    # Positions: one big screen with the notch on every edge.
    W, H = 1440, 820
    scr = wallpaper(W, H, seed=3)
    def put(key, pos, scale=0.75):
        n = raw[key]
        n = n.resize((int(n.width * scale), int(n.height * scale)), LANCZOS)
        x, y = pos(n.width, n.height)
        paste_notch(scr, n, x, y)
    put("collapsed_full_music", lambda w, h: ((W - w) // 2, 0))
    put("edge_left_full", lambda w, h: (0, (H - h) // 2))
    put("edge_right_compact", lambda w, h: (W - w, (H - h) // 2 - 120))
    put("edge_bottom_pill", lambda w, h: (int(W * 0.7) - w // 2, H - h))
    sd = ImageDraw.Draw(scr, "RGBA")
    sd.text((W // 2, H // 2 - 20), "Right-click and drag", font=F_TITLE(46), fill=(255, 255, 255, 235), anchor="mm")
    sd.text((W // 2, H // 2 + 36), "to any edge of any screen, like the taskbar", font=F_REG(28), fill=(220, 230, 240, 220), anchor="mm")
    img = page(1600, 1040)
    img.paste(scr, (80, 60), rounded_mask(W, H, 28))
    d = ImageDraw.Draw(img, "RGBA")
    d.text((90, 920), "Top, bottom, left or right", font=F_TITLE(46), fill=TEXT)
    d.text((90, 984), "Or use Position in the tray menu. Panels always open upright.", font=F_REG(26), fill=DIM)
    save(img, "positions.png")

    # Toasts grid.
    keys = ["toast_timer", "toast_water", "toast_todo", "toast_pomodoro", "toast_charging", "toast_volume", "toast_caps", "toast_bt"]
    img = page(1600, 1280)
    d = ImageDraw.Draw(img, "RGBA")
    d.text((80, 50), "Alerts and overlays", font=F_TITLE(54), fill=TEXT)
    d.text((80, 126), "The notch drops down for a moment, then gets out of the way.", font=F_REG(28), fill=DIM)
    for i, k in enumerate(keys):
        x, y = 80 + (i % 2) * 730, 200 + (i // 2) * 265
        card = screen_card(700, 240, seed=i, title_bar=False)
        n = raw[k]
        n = n.resize((int(n.width * 0.8), int(n.height * 0.8)), LANCZOS)
        paste_notch(card, n, (700 - n.width) // 2, 0, with_shadow=False)
        img.paste(card, (x, y), rounded_mask(700, 240, 20))
    save(img, "alerts.png")

    # Mascot.
    img = page(1600, 520)
    d = ImageDraw.Draw(img, "RGBA")
    d.text((80, 50), "A small companion", font=F_TITLE(54), fill=TEXT)
    d.text((80, 126), "When idle, the clock becomes eyes. They doze off when you are away and wave when you return.", font=F_REG(28), fill=DIM)
    for i, (k, label) in enumerate([("mascot_eyes", "Watching"), ("mascot_doze", "Dozing"), ("mascot_wave", "Hello again")]):
        x = 80 + i * 487
        card = screen_card(460, 240, seed=i, title_bar=False)
        n = raw[k]
        n = n.resize((int(n.width * 0.8), int(n.height * 0.8)), LANCZOS)
        paste_notch(card, n, (460 - n.width) // 2, 0, with_shadow=False)
        img.paste(card, (x, 200), rounded_mask(460, 240, 20))
        chip(d, x + 16, 380, label, 22, bg=(0, 0, 0, 120))
    save(img, "mascot.png")

    # Hover animation GIF.
    frames = sorted(raws.glob("anim_*.raw"))
    bg = wallpaper(780, 260, seed=2)
    holds = {0, 19, 38, 57, 76}
    out, durs = [], []
    for i, p in enumerate(frames):
        f = bg.copy()
        fr = load_raw(p)
        f.paste(fr, (0, 0), fr)
        out.append(f.convert("RGB").quantize(colors=200, method=Image.Quantize.MEDIANCUT))
        durs.append(1600 if i in holds else 33)
    out[0].save(OUT / "demo.gif", save_all=True, append_images=out[1:], duration=durs, loop=0, optimize=True)
    print("wrote demo.gif", len(out), "frames")
    shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
