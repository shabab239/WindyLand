"""Screenshots for the README and user guide (run via make_images.py).

Every image is 1600 px wide: a desktop "screen" card with the real
WindyLand render at its top edge, then a title, a line of text and a row of
feature chips, on a dark page. Collections (styles, alerts, mascots,
positions) arrange several renders on one card.
"""

import shutil
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent.parent
IMAGES = ROOT / "docs" / "images"
BG = (18, 20, 25)
TEXT = (245, 245, 247)
DIM = (160, 164, 176)
CHIP = (38, 42, 52)
W = 1600
PAD = 40


def make(data, run, read_raw, wallpaper, fake_windows, rounded_mask, font):
    out = Path(data).parent / "docs"
    if out.exists():
        shutil.rmtree(out)
    print("rendering stills...")
    run(["--render-docs", str(out)], data)

    def shot(name):
        return read_raw(out / f"{name}.raw")

    def card(h, windows=True):
        cw = W - 2 * PAD
        wp = wallpaper(cw, h, 0.0)
        if windows:
            fake_windows(wp)
        return wp, rounded_mask(cw, h, 26)

    def page(card_img, mask, title, sub, chips):
        """Card on the page, then the title block."""
        ch = card_img.height
        h = PAD + ch + 36 + 70 + 44 + (64 if chips else 0) + 24
        im = Image.new("RGBA", (W, h), BG + (255,))
        im.paste(card_img, (PAD, PAD), mask)
        d = ImageDraw.Draw(im)
        y = PAD + ch + 36
        d.text((PAD + 4, y), title, font=font(54, "bold"), fill=TEXT)
        y += 72
        d.text((PAD + 4, y), sub, font=font(28), fill=DIM)
        y += 52
        x = PAD
        f = font(22, "semibold")
        for c in chips:
            tw = d.textlength(c, font=f)
            d.rounded_rectangle((x, y, x + tw + 36, y + 42), 21, fill=CHIP)
            d.text((x + 18, y + 21), c, font=f, fill=TEXT, anchor="lm")
            x += tw + 48
        return im.convert("RGB")

    def single(name, raw, title, sub, chips, min_h=420):
        """One render at the top centre of a card."""
        img = shot(raw)
        h = max(min_h, img.height + 140)
        c, m = card(h)
        c.alpha_composite(img, ((c.width - img.width) // 2, 0))
        page(c, m, title, sub, chips).save(IMAGES / f"{name}.png")
        print("  ", name)

    # Hero: lyrics view, big title.
    img = shot("panel_lyrics")
    c, m = card(560)
    c.alpha_composite(img, ((c.width - img.width) // 2, 0))
    hero = page(c, m, "WindyLand", "A living notch for Windows. Free and open source.",
                ["Music + lyrics", "Notifications", "Timer", "Pomodoro", "To-do", "Water", "File shelf", "Clipboard"])
    hero.save(IMAGES / "hero.png")
    print("   hero")

    single("music", "panel_music", "Now playing", "Album art, seek bar and controls for whatever is playing.", ["Spotify", "Browsers", "Media Player", "Any app in the Windows media overlay"])
    single("lyrics", "panel_lyrics", "Synced lyrics", "Tap the lyrics button and the panel splits in two. The current line lights up as it is sung.", ["Click a line to jump there", "Scroll to browse", "Lyrics by LRCLIB"])
    single("timer", "panel_timer", "Timer", "Presets or your own time. The notch tells you when it's done.", ["Presets", "+1 min", "Restart from the alert"])
    single("stopwatch", "panel_stopwatch", "Stopwatch", "Tenths of a second, right in the notch.", ["Start / pause", "Reset"])
    single("pomodoro", "panel_pomodoro", "Pomodoro", "Focus sessions with short and long breaks.", ["25 / 5 / 15", "Sessions today", "Break alerts"])
    single("todo", "panel_todo", "To-do with reminders", "Jot a task and give it a time. The notch taps you on the shoulder when it is due.", ["Add Task", "Click a task to edit", "Tick to complete", "Done tasks clear the next day"])
    single("todo-editor", "panel_todo_editor", "Add a task", "Type it, pick when to be reminded, done.", ["+15m", "+30m", "+1h", "Tonight", "Tomorrow"])
    single("water", "panel_water", "Water reminder", "A gentle nudge to drink, and a chart of your day.", ["One-tap log", "Daily goal", "Refill alerts"])
    single("water-settings", "panel_water_settings", "Water settings", "Bottle size, daily goal and reminder hours.", ["Bottle", "Goal", "Sip", "Hours"])
    single("shelf", "panel_shelf", "File shelf", "Drop files on the notch to keep them handy, drag them out when you need them.", ["Drag in", "Drag out", "Open with a click"])
    single("clipboard", "panel_clipboard", "Clipboard history", "Everything you copy, newest first. Off until you turn it on.", ["Text, files or images", "Pin to keep", "Skips password managers"])
    single("settings", "settings_general", "Settings", "Right-click the notch. Style, position, mascot, sound and more.", ["Notch style", "Position", "Mascot", "Sound", "Start with Windows"])
    single("settings-privacy", "settings_privacy", "Privacy & alerts", "Notifications, mic and camera dots, and clipboard history: all opt-in.", ["Windows notifications", "Hide message text", "Mute apps"])
    single("tour", "tour_style", "Welcome tour", "First run asks how the notch should look, then where it sits.", ["Full", "Compact", "Pill", "Hidden"])
    single("notifications", "toast_notification", "Windows notifications", "Other apps' notifications drop out of the notch, with their icon and colour.", ["Click to open the app", "Mute an app", "Respects Do Not Disturb"], min_h=300)

    # Styles: four small cards.
    cw = (W - 2 * PAD - 30) // 2
    grid = Image.new("RGBA", (W - 2 * PAD, 2 * 230 + 30), (0, 0, 0, 0))
    styles = [("collapsed_full_music", "Full"), ("collapsed_compact_music", "Compact"), ("collapsed_pill_music", "Pill"), (None, "Hidden")]
    for i, (raw, label) in enumerate(styles):
        wp = wallpaper(cw, 230, i * 0.7)
        if raw:
            img = shot(raw)
            wp.alpha_composite(img, ((cw - img.width) // 2, 0))
        else:
            ImageDraw.Draw(wp).text((cw // 2, 115), "Rest the mouse at the edge", font=font(26), fill=TEXT, anchor="mm")
        d = ImageDraw.Draw(wp)
        f = font(22, "semibold")
        tw = d.textlength(label, font=f)
        d.rounded_rectangle((20, 230 - 62, 20 + tw + 32, 230 - 20), 21, fill=(0, 0, 0, 150))
        d.text((36, 230 - 41), label, font=f, fill=TEXT, anchor="lm")
        grid.paste(wp, ((i % 2) * (cw + 30), (i // 2) * 260), rounded_mask(cw, 230, 22))
    im = Image.new("RGBA", (W, grid.height + 2 * PAD + 150), BG + (255,))
    im.alpha_composite(grid, (PAD, PAD))
    d = ImageDraw.Draw(im)
    d.text((PAD + 4, PAD + grid.height + 30), "Notch styles", font=font(54, "bold"), fill=TEXT)
    d.text((PAD + 4, PAD + grid.height + 102), "Right-click the notch, then pick a style. The welcome tour asks first.", font=font(28), fill=DIM)
    im.convert("RGB").save(IMAGES / "styles.png")
    print("   styles")

    # Alerts: toasts in two columns.
    toasts = ["toast_notification", "toast_timer", "toast_water", "toast_todo", "toast_pomodoro", "toast_charging", "toast_volume", "toast_mic", "toast_caps", "toast_bt"]
    imgs = [shot(t) for t in toasts]
    row_h = max(i.height for i in imgs) + 40
    rows = (len(imgs) + 1) // 2
    grid = Image.new("RGBA", (W - 2 * PAD, rows * (row_h + 24)), (0, 0, 0, 0))
    for i, img in enumerate(imgs):
        wp = wallpaper(cw, row_h, i * 0.37)
        wp.alpha_composite(img, ((cw - img.width) // 2, 0))
        grid.paste(wp, ((i % 2) * (cw + 30), (i // 2) * (row_h + 24)), rounded_mask(cw, row_h, 22))
    im = Image.new("RGBA", (W, grid.height + 2 * PAD + 150), BG + (255,))
    im.alpha_composite(grid, (PAD, PAD))
    d = ImageDraw.Draw(im)
    d.text((PAD + 4, PAD + grid.height + 20), "Alerts and overlays", font=font(54, "bold"), fill=TEXT)
    d.text((PAD + 4, PAD + grid.height + 92), "The notch drops down for a moment, then gets out of the way.", font=font(28), fill=DIM)
    im.convert("RGB").save(IMAGES / "alerts.png")
    print("   alerts")

    # Mascots: eight closed notches in a row of four.
    from_store = ["eyes", "cat", "ghost", "robot", "sparky", "sprout", "owl", "panda"]
    mw = (W - 2 * PAD - 3 * 24) // 4
    grid = Image.new("RGBA", (W - 2 * PAD, 2 * 170 + 24), (0, 0, 0, 0))
    for i, k in enumerate(from_store):
        img = shot(f"mascot_{k}")
        if img.width > mw - 60:
            img = img.resize((mw - 60, round(img.height * (mw - 60) / img.width)), Image.LANCZOS)
        wp = wallpaper(mw, 170, i * 0.5)
        wp.alpha_composite(img, ((mw - img.width) // 2, 0))
        ImageDraw.Draw(wp).text((mw // 2, 130), k.capitalize(), font=font(26, "semibold"), fill=TEXT, anchor="mm")
        grid.paste(wp, ((i % 4) * (mw + 24), (i // 4) * 194), rounded_mask(mw, 170, 20))
    im = Image.new("RGBA", (W, grid.height + 2 * PAD + 150), BG + (255,))
    im.alpha_composite(grid, (PAD, PAD))
    d = ImageDraw.Draw(im)
    d.text((PAD + 4, PAD + grid.height + 20), "Pick a companion", font=font(54, "bold"), fill=TEXT)
    d.text((PAD + 4, PAD + grid.height + 92), "In Full style, when nothing is going on. They follow your mouse, blink, doze and wave.", font=font(28), fill=DIM)
    im.convert("RGB").save(IMAGES / "mascot.png")
    print("   mascot")

    # Positions: one screen with the notch on four edges.
    c, m = card(720, windows=False)
    top, left, right, bottom = shot("collapsed_full_music"), shot("edge_left_full"), shot("edge_right_compact"), shot("edge_bottom_pill")
    c.alpha_composite(top, ((c.width - top.width) // 2, 0))
    c.alpha_composite(left, (0, (c.height - left.height) // 2))
    c.alpha_composite(right, (c.width - right.width, (c.height - right.height) // 2))
    c.alpha_composite(bottom, ((c.width - bottom.width) // 2, c.height - bottom.height))
    d = ImageDraw.Draw(c)
    d.text((c.width // 2, c.height // 2 - 24), "Right-click and drag", font=font(56, "bold"), fill=TEXT, anchor="mm")
    d.text((c.width // 2, c.height // 2 + 34), "to any edge of any screen, like the taskbar", font=font(30), fill=TEXT, anchor="mm")
    page(c, m, "Top, bottom, left or right", "Or pick a position in Settings. Panels always open upright.", []).save(IMAGES / "positions.png")
    print("   positions")
    shutil.rmtree(out)
