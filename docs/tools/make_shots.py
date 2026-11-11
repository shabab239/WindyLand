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

    def page_chips(top, title, sub, chips):
        """A finished image on top, then the title, a line of text and
        chips that wrap onto as many rows as they need."""
        f = font(22, "semibold")
        probe = ImageDraw.Draw(top)
        rows, row, x = [], [], PAD
        for c in chips:
            tw = probe.textlength(c, font=f) + 36
            if x + tw > W - PAD and row:
                rows.append(row)
                row, x = [], PAD
            row.append(c)
            x += tw + 12
        if row:
            rows.append(row)
        h = top.height + 110 + 50 + len(rows) * 54 + 30
        im = Image.new("RGBA", (W, h), BG + (255,))
        im.alpha_composite(top, (0, 0))
        d = ImageDraw.Draw(im)
        y = top.height + 6
        d.text((PAD + 4, y), title, font=font(54, "bold"), fill=TEXT)
        y += 72
        d.text((PAD + 4, y), sub, font=font(28), fill=DIM)
        y += 56
        for row in rows:
            x = PAD
            for c in row:
                tw = d.textlength(c, font=f)
                d.rounded_rectangle((x, y, x + tw + 36, y + 42), 21, fill=CHIP)
                d.text((x + 18, y + 21), c, font=f, fill=TEXT, anchor="lm")
                x += tw + 48
            y += 54
        return im.convert("RGB")

    def single(name, raw, title, sub, chips, min_h=420):
        """One render at the top centre of a card."""
        img = shot(raw)
        h = max(min_h, img.height + 140)
        c, m = card(h)
        c.alpha_composite(img, ((c.width - img.width) // 2, 0))
        page(c, m, title, sub, chips).save(IMAGES / f"{name}.png")
        print("  ", name)

    # Hero: every panel at a glance (a collage), so the first image shows
    # how much is in it.
    pairs = [
        [("panel_agents", "AI agents: Hermes, Claude Code, Codex"), ("panel_tools_color", "Tools: color picker, calculator, QR, converter")],
        [("panel_monitor", "System monitor"), ("panel_clipboard", "Clipboard: click to paste")],
        [("panel_timer", "Timer and stopwatch"), ("panel_pomodoro", "Pomodoro")],
        [("panel_todo", "To-do with reminders"), ("panel_water", "Water reminder")],
        [("panel_prayer", "Prayer times"), ("panel_shelf", "File shelf and screenshots")],
    ]
    cw = W - 2 * PAD
    gap = 22
    big_w = (cw - gap) * 58 // 100
    side_w = cw - gap - big_w
    col2 = (cw - gap) // 2

    def badge(img, label, h, top=False, right=False):
        d = ImageDraw.Draw(img)
        f = font(21, "semibold")
        tw = d.textlength(label, font=f)
        y0 = 14 if top else h - 52
        x0 = img.width - tw - 46 if right else 16
        d.rounded_rectangle((x0, y0, x0 + tw + 30, y0 + 38), 19, fill=(0, 0, 0, 150))
        d.text((x0 + 15, y0 + 19), label, font=f, fill=TEXT, anchor="lm")

    def tile(raw, label, w, h, seed):
        """A desktop tile with the panel dropping from its top edge."""
        wp = wallpaper(w, h, seed)
        img = shot(raw)
        scale = min((w - 36) / img.width, (h - 64) / img.height, 1.0)
        img = img.resize((round(img.width * scale), round(img.height * scale)), Image.LANCZOS)
        wp.alpha_composite(img, ((w - img.width) // 2, 0))
        badge(wp, label, h)
        return wp

    # Row 1: lyrics (big) and two alerts + the closed notch on the side.
    big_h = 420
    row1 = tile("panel_lyrics", "Music + synced lyrics", big_w, big_h, 0.0)
    side = wallpaper(side_w, big_h, 1.3)
    y = 0
    side_imgs = [shot(r) for r in ["collapsed_full_music", "toast_agent", "toast_notification"]]
    fit = min((side_w - 30) / max(i.width for i in side_imgs), (big_h - 70) / sum(i.height + 6 for i in side_imgs), 1.0)
    for img in side_imgs:
        img = img.resize((round(img.width * fit), round(img.height * fit)), Image.LANCZOS)
        side.alpha_composite(img, ((side_w - img.width) // 2, y))
        y += img.height + 6
    badge(side, "Alerts drop out of the notch", big_h)

    # Last row: the mascot on the Windows taskbar, the panel opening above it.
    small_h = 330
    tb_h = 360

    def taskbar_tile(w, h):
        wp = wallpaper(w, h, 2.2)
        bar = 64
        layer = Image.new("RGBA", (w, h), (0, 0, 0, 0))
        d = ImageDraw.Draw(layer)
        d.rectangle((0, h - bar, w, h), fill=(28, 30, 38, 255))
        d.line((0, h - bar, w, h - bar), fill=(70, 74, 88, 255), width=1)
        # Centred app icons, then the clock on the right, like Windows 11.
        icons = [(0, 120, 215), (240, 240, 240), (255, 185, 0), (16, 124, 16), (232, 17, 35), (125, 85, 199), (0, 183, 195)]
        s, g = 38, 14
        x = (w - (len(icons) * s + (len(icons) - 1) * g)) // 2
        for c in icons:
            d.rounded_rectangle((x, h - bar + 13, x + s, h - bar + 13 + s), 9, fill=c + (255,))
            x += s + g
        f = font(17)
        d.text((w - 24, h - bar / 2 - 10), "4:20 PM", font=f, fill=TEXT, anchor="rm")
        d.text((w - 24, h - bar / 2 + 11), "9 Nov 2026", font=f, fill=DIM, anchor="rm")
        wp.alpha_composite(layer)
        # The mascot in the corner.
        m = shot("taskbar_mascot_cat")
        k = (bar - 10) / m.height
        m = m.resize((round(m.width * k), round(m.height * k)), Image.LANCZOS)
        wp.alpha_composite(m, (14, h - bar + 5))
        # The panel above it.
        p = shot("edge_bottom_panel")
        k = min((h - bar - 12) / p.height, (w * 0.56) / p.width, 1.0)
        p = p.resize((round(p.width * k), round(p.height * k)), Image.LANCZOS)
        wp.alpha_composite(p, (6, h - bar - p.height))
        d = ImageDraw.Draw(wp)
        # Text on the right half, where the screen is empty.
        tx = round(w * 0.6)
        d.text((tx, 70), "New: a mascot on your taskbar", font=font(34, "bold"), fill=TEXT)
        d.text((tx, 122), "It lives in the empty corner, like the weather.", font=font(22), fill=TEXT)
        d.text((tx, 154), "Hover it and the panel opens above it.", font=font(22), fill=TEXT)
        return wp

    total_h = big_h + len(pairs) * (small_h + gap) + tb_h + gap
    collage = Image.new("RGBA", (cw, total_h), (0, 0, 0, 0))
    collage.paste(row1, (0, 0), rounded_mask(big_w, big_h, 22))
    collage.paste(side, (big_w + gap, 0), rounded_mask(side_w, big_h, 22))
    y = big_h + gap
    for r, row in enumerate(pairs):
        for k, (raw, label) in enumerate(row):
            t = tile(raw, label, col2, small_h, 0.4 + r * 0.9 + k * 0.33)
            collage.paste(t, (k * (col2 + gap), y), rounded_mask(col2, small_h, 22))
        y += small_h + gap
    collage.paste(taskbar_tile(cw, tb_h), (0, y), rounded_mask(cw, tb_h, 22))
    # The website shows the collage alone (its own title is above it).
    collage.save(IMAGES / "hero-collage.png")
    hero = Image.new("RGBA", (W, total_h + 2 * PAD), BG + (255,))
    hero.alpha_composite(collage, (PAD, PAD))
    page_img = page_chips(hero, "WindyLand", "A Dynamic Island for Windows: music, AI agents, tools, reminders and more. Free and open source.",
                          ["Music + lyrics", "AI agents", "Color picker", "Calculator", "QR codes", "Converter", "System monitor", "Clipboard",
                           "Screenshots", "File shelf", "Notifications", "Prayer times", "Timer", "Pomodoro", "To-do", "Water", "8 mascots", "Taskbar mascot"])
    page_img.save(IMAGES / "hero.png")
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
    single("prayer", "panel_prayer", "Prayer times", "Worked out on your PC for your city, with optional reminders.", ["16 cities or any place", "Hanafi or Shafi Asr", "Minute nudges"])
    single("agents", "panel_agents", "AI agents", "See when Hermes, Claude Code or Codex is working, needs your OK, or is done.", ["Connect with one click", "Status only, stays on your PC", "Alerts when it needs you"])
    single("monitor", "panel_monitor", "System monitor", "CPU, memory and network speed, with a minute of history.", ["Free disk space", "Optional on the closed notch"])
    single("widgets", "panel_widgets_order", "Widgets", "Choose what the closed notch shows, and drag it into the order you want.", ["Battery and AirPods", "Two side by side", "Drag to reorder"])
    single("tools", "panel_tools_color", "Color picker", "Pick any color on your screen, then copy it as HEX, RGB or HSL.", ["Magnifier", "Recent colors", "Contrast check"])
    single("tools-calc", "panel_tools_calc", "Calculator", "Type a sum and see the answer as you type.", ["15% and +10%", "sqrt, sin, 5!", "Hex and binary"])
    single("tools-qr", "panel_tools_qr", "QR code", "Whatever you copy, as a QR code for your phone.", ["From clipboard or typed", "Copy as image", "Works offline"])
    single("tools-convert", "panel_tools_convert", "Converter", "Units, including katha, bigha and maund, and world time zones.", ["Length to data", "Bangladesh units", "12 cities"])
    single("tools-zones", "panel_tools_zones", "Time zones", "What time it is in 12 cities, now or an hour from now.", ["Daylight saving handled", "Pick any city as base"])
    single("settings-privacy", "settings_privacy", "Privacy settings", "Notifications, mic and camera dots, and clipboard history: all opt-in.", ["Windows notifications", "Hide message text", "Mute apps"])
    single("tour", "tour_style", "Welcome tour", "First run asks how the notch should look, then where it sits.", ["Full", "Slim", "Compact", "Pill", "Hidden"])
    single("notifications", "toast_notification", "Windows notifications", "Other apps' notifications drop out of the notch, with their icon and colour.", ["Click to open the app", "Mute an app", "Respects Do Not Disturb"], min_h=300)

    # Styles: five small cards, two per row.
    cw = (W - 2 * PAD - 30) // 2
    grid = Image.new("RGBA", (W - 2 * PAD, 3 * 230 + 60), (0, 0, 0, 0))
    styles = [("collapsed_full_music", "Full"), ("collapsed_slim_music", "Slim"), ("collapsed_compact_music", "Compact"), ("collapsed_pill_music", "Pill"), (None, "Hidden")]
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
    toasts = ["toast_notification", "toast_agent", "toast_timer", "toast_water", "toast_todo", "toast_pomodoro", "toast_charging", "toast_volume", "toast_mic", "toast_caps", "toast_bt", "toast_copy", "toast_agent_done"]
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
