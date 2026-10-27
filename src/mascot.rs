//! The idle companion in the collapsed Full notch. Every mascot has the same
//! behaviour: it looks toward the cursor and blinks (`Eyes` mood), closes its
//! eyes with a floating "z" when you're away (`Doze`), and smiles and waves
//! when you come back (`Wave`).
//!
//! To add a mascot: add a variant to `store::Mascot`, then a `draw_*`
//! function here that returns its half width (so the "z" and the waving hand
//! sit just beside it).

use crate::gfx::*;
use crate::store::Mascot;
use crate::ui::Ui;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mood {
    Clock,
    Eyes,
    Doze,
    Wave,
}

/// How the eyes look right now.
#[derive(Clone, Copy, PartialEq)]
enum EyeState {
    /// Open, 0..1 (blinks close it briefly).
    Open(f32),
    Closed,
    Happy,
}

/// Shared drawing state: centre, scale, where it looks and the eye state.
struct Pose {
    cx: f32,
    cy: f32,
    s: f32,
    look: (f32, f32),
    eye: EyeState,
    t: f64,
}

impl Pose {
    fn x(&self, dx: f32) -> f32 {
        self.cx + dx * self.s
    }
    fn y(&self, dy: f32) -> f32 {
        self.cy + dy * self.s
    }
}

/// Draw `kind` into `area` (about 28 DIPs tall in the Full notch; it scales
/// to fit smaller areas). `look` is the gaze offset (-2.5..2.5, -1.5..1.5).
pub fn draw(ui: &mut Ui, area: Rect, kind: Mascot, mood: Mood, t: f64, look: (f32, f32)) {
    let s = (area.h / 28.0).min(area.w / 60.0).clamp(0.45, 1.6);
    let phase = (t % 4.2) as f32;
    let blink = if phase < 0.12 { (phase / 0.06 - 1.0).abs() } else { 1.0 };
    let eye = match mood {
        Mood::Doze => EyeState::Closed,
        Mood::Wave => EyeState::Happy,
        _ => EyeState::Open(blink),
    };
    let look = if mood == Mood::Doze { (0.0, 0.0) } else { look };
    let p = Pose { cx: area.cx(), cy: area.cy(), s, look, eye, t };
    let half = match kind {
        Mascot::Eyes => eyes(ui, &p),
        Mascot::Cat => cat(ui, &p),
        Mascot::Ghost => ghost(ui, &p),
        Mascot::Robot => robot(ui, &p),
        Mascot::Sparky => sparky(ui, &p),
        Mascot::Sprout => sprout(ui, &p),
        Mascot::Owl => owl(ui, &p),
        Mascot::Panda => panda(ui, &p),
    };
    match mood {
        Mood::Doze => zzz(ui, p.cx + half + 4.0 * s, p.cy, s, t),
        Mood::Wave => hand(ui, p.cx + half + 8.0 * s, p.cy + 2.0 * s, s, t),
        _ => {}
    }
}

/// A pair of eyes at (cx +/- gap, cy) in `c`. Open eyes are ellipses of
/// radius (rx, ry) shifted by the gaze; closed are short lines; happy are ^ arcs.
fn eye_pair(ui: &mut Ui, p: &Pose, gap: f32, cy: f32, rx: f32, ry: f32, c: Color, follow: f32) {
    let s = p.s;
    for side in [-1.0f32, 1.0] {
        let ex = p.x(side * gap);
        match p.eye {
            EyeState::Open(o) => {
                ui.ellipse(ex + p.look.0 * s * follow, p.y(cy) + p.look.1 * s * follow, rx * s, (ry * o).max(0.5) * s, c);
            }
            EyeState::Closed => ui.line(ex - rx * 1.1 * s, p.y(cy + 1.0), ex + rx * 1.1 * s, p.y(cy + 1.0), c.mul_a(0.85), 1.8 * s),
            EyeState::Happy => ui.arc(ex, p.y(cy + 2.5), rx * 1.05 * s, -1.2, 2.4, c, 1.8 * s),
        }
    }
}

fn zzz(ui: &mut Ui, x: f32, cy: f32, s: f32, t: f64) {
    let z = ((t * 0.6) % 1.0) as f32;
    let zr = Rect::new(x + z * 6.0 * s, cy - 10.0 * s - z * 6.0 * s, 14.0 * s, 14.0 * s);
    let a = ui.alpha;
    ui.alpha = a * (1.0 - z);
    ui.text("z", zr, 10.0 * s, col::DIM, Weight::Semi, Align::Left);
    ui.alpha = a;
}

fn hand(ui: &mut Ui, hx: f32, hy: f32, s: f32, t: f64) {
    let wob = ((t * 9.0).sin() * 0.35) as f32;
    ui.circle(hx, hy, 4.0 * s, col::YELLOW);
    for k in 0..3 {
        let a = wob - 0.5 + k as f32 * 0.5;
        ui.line(hx, hy, hx + a.sin() * 7.0 * s, hy - a.cos() * 7.0 * s, col::YELLOW, 1.8 * s);
    }
}

// ------------------------------------------------------------- characters

/// The original: two white eyes.
fn eyes(ui: &mut Ui, p: &Pose) -> f32 {
    eye_pair(ui, p, 13.0, 0.0, 4.2, 6.0, col::TEXT, 1.0);
    18.0 * p.s
}

fn cat(ui: &mut Ui, p: &Pose) -> f32 {
    let s = p.s;
    let fur = hex(0x3A3A40);
    let ear_in = hex(0xFF8FA3);
    // Ears behind the head.
    for side in [-1.0f32, 1.0] {
        ui.polygon(&[(p.x(side * 15.0), p.y(-2.0)), (p.x(side * 12.0), p.y(-14.0)), (p.x(side * 4.0), p.y(-6.0))], fur);
        ui.polygon(&[(p.x(side * 12.5), p.y(-4.0)), (p.x(side * 11.5), p.y(-10.5)), (p.x(side * 7.0), p.y(-6.0))], ear_in);
    }
    let head = Rect::new(p.x(-15.0), p.y(-8.0), 30.0 * s, 20.0 * s);
    ui.fill_rr(head, 9.0 * s, fur);
    ui.stroke_rr(head, 9.0 * s, col::TEXT.with_a(0.35), 1.0 * s);
    // Green eyes with slit pupils.
    eye_pair(ui, p, 6.0, 0.0, 3.0, 3.8, hex(0xB8E05A), 0.6);
    if let EyeState::Open(o) = p.eye {
        if o > 0.5 {
            for side in [-1.0f32, 1.0] {
                let ex = p.x(side * 6.0) + p.look.0 * s * 0.6;
                let ey = p.y(0.0) + p.look.1 * s * 0.6;
                ui.line(ex, ey - 2.6 * s * o, ex, ey + 2.6 * s * o, hex(0x101010), 1.2 * s);
            }
        }
    }
    // Nose, mouth and whiskers.
    ui.polygon(&[(p.x(-1.6), p.y(4.5)), (p.x(1.6), p.y(4.5)), (p.x(0.0), p.y(6.2))], ear_in);
    ui.arc(p.x(-1.5), p.y(6.0), 1.6 * s, 3.4, 1.6, col::TEXT.with_a(0.7), 0.9 * s);
    ui.arc(p.x(1.5), p.y(6.0), 1.6 * s, 1.3, 1.6, col::TEXT.with_a(0.7), 0.9 * s);
    for side in [-1.0f32, 1.0] {
        ui.line(p.x(side * 9.0), p.y(4.0), p.x(side * 21.0), p.y(2.5), col::DIM, 0.9 * s);
        ui.line(p.x(side * 9.0), p.y(6.0), p.x(side * 21.0), p.y(7.0), col::DIM, 0.9 * s);
    }
    21.0 * s
}

fn ghost(ui: &mut Ui, p: &Pose) -> f32 {
    let s = p.s;
    let bob = ((p.t * 2.2).sin() * 1.4) as f32;
    let g = Pose { cy: p.cy + bob * s, ..*p };
    let body = col::TEXT;
    ui.circle(g.x(0.0), g.y(-3.0), 10.0 * s, body);
    ui.fill_rect(Rect::new(g.x(-10.0), g.y(-3.0), 20.0 * s, 12.0 * s), body);
    // Wavy hem: cut two scallops out of the bottom.
    for dx in [-5.0f32, 5.0] {
        ui.circle(g.x(dx), g.y(10.2), 3.0 * s, col::NOTCH);
    }
    for dx in [-10.0f32, 10.0] {
        ui.circle(g.x(dx), g.y(10.2), 2.0 * s, col::NOTCH);
    }
    let ink = hex(0x16161A);
    eye_pair(ui, &g, 4.2, -4.0, 2.1, 3.0, ink, 0.6);
    // Blush, and a little "o" mouth when waving.
    for side in [-1.0f32, 1.0] {
        ui.circle(g.x(side * 7.0), g.y(0.5), 1.6 * s, hex(0xFF8FA3).with_a(0.7));
    }
    if g.eye == EyeState::Happy {
        ui.ellipse(g.x(0.0), g.y(1.5), 1.6 * s, 2.0 * s, ink);
    }
    12.0 * s
}

fn robot(ui: &mut Ui, p: &Pose) -> f32 {
    let s = p.s;
    let led = col::CYAN;
    // Antenna with a pulsing light.
    ui.line(p.x(0.0), p.y(-8.0), p.x(0.0), p.y(-12.0), col::DIM, 1.4 * s);
    let pulse = (0.55 + 0.45 * (p.t * 3.0).sin()) as f32;
    ui.circle(p.x(0.0), p.y(-13.0), 1.9 * s, col::RED.with_a(pulse));
    // Side bolts and head.
    for side in [-1.0f32, 1.0] {
        ui.fill_rr(Rect::new(p.x(side * 14.5 - 1.5), p.y(-3.0), 3.0 * s, 7.0 * s), 1.2 * s, col::DIM);
    }
    let head = Rect::new(p.x(-13.0), p.y(-8.0), 26.0 * s, 19.0 * s);
    ui.fill_rr(head, 5.0 * s, hex(0x24242A));
    ui.stroke_rr(head, 5.0 * s, col::TEXT.with_a(0.8), 1.4 * s);
    // LED eyes.
    for side in [-1.0f32, 1.0] {
        let ex = p.x(side * 5.5) + p.look.0 * s * 0.7;
        let ey = p.y(-1.0) + p.look.1 * s * 0.6;
        match p.eye {
            EyeState::Open(o) => {
                let h = (4.6 * o).max(0.8) * s;
                ui.fill_rr(Rect::new(ex - 2.6 * s, ey - h / 2.0, 5.2 * s, h), 1.4 * s, led);
            }
            EyeState::Closed => ui.line(ex - 2.6 * s, p.y(0.0), ex + 2.6 * s, p.y(0.0), led.with_a(0.5), 1.3 * s),
            EyeState::Happy => ui.arc(p.x(side * 5.5), p.y(1.0), 2.8 * s, -1.2, 2.4, led, 1.6 * s),
        }
    }
    // Grille mouth.
    for k in 0..4 {
        let x = p.x(-4.5 + k as f32 * 3.0);
        ui.line(x, p.y(5.0), x, p.y(7.5), col::DIM, 1.0 * s);
    }
    17.0 * s
}

/// A yellow electric hamster (an original character).
fn sparky(ui: &mut Ui, p: &Pose) -> f32 {
    let s = p.s;
    let fur = hex(0xFFC93C);
    let inner = hex(0xFF8A3D);
    // Round ears.
    for side in [-1.0f32, 1.0] {
        ui.circle(p.x(side * 8.5), p.y(-8.0), 4.2 * s, fur);
        ui.circle(p.x(side * 8.5), p.y(-8.0), 2.2 * s, inner);
    }
    ui.ellipse(p.x(0.0), p.y(1.0), 12.5 * s, 10.5 * s, fur);
    // Lightning bolt on the forehead.
    ui.polygon(
        &[
            (p.x(0.8), p.y(-9.0)),
            (p.x(-2.2), p.y(-4.2)),
            (p.x(-0.2), p.y(-4.2)),
            (p.x(-1.2), p.y(-1.0)),
            (p.x(2.2), p.y(-5.6)),
            (p.x(0.2), p.y(-5.6)),
            (p.x(1.6), p.y(-9.0)),
        ],
        inner,
    );
    let ink = hex(0x1A1410);
    eye_pair(ui, p, 5.0, 0.5, 2.1, 2.6, ink, 0.5);
    if let EyeState::Open(o) = p.eye {
        if o > 0.6 {
            for side in [-1.0f32, 1.0] {
                ui.circle(p.x(side * 5.0 + 0.7) + p.look.0 * s * 0.5, p.y(-0.4) + p.look.1 * s * 0.5, 0.7 * s, col::TEXT);
            }
        }
    }
    // Cheeks, nose and mouth.
    for side in [-1.0f32, 1.0] {
        ui.circle(p.x(side * 8.8), p.y(4.5), 2.4 * s, hex(0xFF6B4A));
    }
    ui.ellipse(p.x(0.0), p.y(3.6), 1.0 * s, 0.7 * s, ink);
    ui.arc(p.x(0.0), p.y(4.2), 2.4 * s, 2.3, 1.7, ink, 0.9 * s);
    // Little sparks when waving.
    if p.eye == EyeState::Happy {
        let k = ((p.t * 6.0) % 1.0) as f32;
        for (dx, dy) in [(-15.0f32, -6.0f32), (14.0, -9.0)] {
            ui.line(p.x(dx), p.y(dy), p.x(dx + 2.0), p.y(dy - 3.0 * k), col::YELLOW, 1.2 * s);
        }
    }
    14.0 * s
}

/// A tiny tree sapling (an original character).
fn sprout(ui: &mut Ui, p: &Pose) -> f32 {
    let s = p.s;
    let wood = hex(0x8B5E3C);
    let bark = hex(0x6B4529);
    let leaf = hex(0x5DBB63);
    let sway = ((p.t * 1.6).sin() * if p.eye == EyeState::Happy { 2.2 } else { 0.8 }) as f32;
    // Two leaves on top.
    for side in [-1.0f32, 1.0] {
        let tip = (side * 11.0 + sway, -13.5);
        ui.polygon(
            &[(p.x(side * 1.0), p.y(-7.0)), (p.x(side * 5.0 + sway * 0.5), p.y(-13.0)), (p.x(tip.0), p.y(tip.1)), (p.x(side * 7.0 + sway * 0.5), p.y(-8.0))],
            leaf,
        );
    }
    ui.line(p.x(0.0), p.y(-7.0), p.x(0.0), p.y(-10.0), leaf, 1.4 * s);
    let face = Rect::new(p.x(-10.5), p.y(-7.5), 21.0 * s, 20.0 * s);
    ui.fill_rr(face, 7.0 * s, wood);
    // Bark lines.
    ui.line(p.x(-7.0), p.y(-4.5), p.x(-5.0), p.y(-2.5), bark, 1.0 * s);
    ui.line(p.x(6.5), p.y(7.0), p.x(8.0), p.y(9.5), bark, 1.0 * s);
    let ink = hex(0x1A120C);
    eye_pair(ui, p, 4.2, 0.0, 1.9, 2.4, ink, 0.5);
    if let EyeState::Open(o) = p.eye {
        if o > 0.6 {
            for side in [-1.0f32, 1.0] {
                ui.circle(p.x(side * 4.2 + 0.6) + p.look.0 * s * 0.5, p.y(-0.8) + p.look.1 * s * 0.5, 0.6 * s, col::TEXT);
            }
        }
    }
    ui.arc(p.x(0.0), p.y(3.0), 3.0 * s, 2.2, 1.9, ink, 1.0 * s);
    13.0 * s
}

fn owl(ui: &mut Ui, p: &Pose) -> f32 {
    let s = p.s;
    let feather = hex(0x8A6240);
    let face = hex(0xB88A5E);
    // Ear tufts and face disc.
    for side in [-1.0f32, 1.0] {
        ui.polygon(&[(p.x(side * 12.0), p.y(-12.5)), (p.x(side * 6.0), p.y(-7.0)), (p.x(side * 12.5), p.y(-4.0))], feather);
    }
    ui.ellipse(p.x(0.0), p.y(1.0), 14.0 * s, 11.5 * s, feather);
    for side in [-1.0f32, 1.0] {
        ui.circle(p.x(side * 5.6), p.y(-0.5), 6.0 * s, face);
    }
    // Big eyes: white, amber ring, pupil that follows the cursor.
    for side in [-1.0f32, 1.0] {
        let ex = p.x(side * 5.6);
        let ey = p.y(-0.5);
        match p.eye {
            EyeState::Open(o) => {
                ui.ellipse(ex, ey, 4.4 * s, (4.4 * o).max(0.5) * s, col::TEXT);
                if o > 0.4 {
                    ui.ellipse(ex + p.look.0 * s * 0.6, ey + p.look.1 * s * 0.6, 2.6 * s, 2.6 * o * s, col::ACCENT);
                    ui.ellipse(ex + p.look.0 * s * 0.7, ey + p.look.1 * s * 0.7, 1.4 * s, 1.4 * o * s, hex(0x101010));
                }
            }
            EyeState::Closed => ui.arc(ex, ey - 1.0, 3.6 * s, 2.0, 2.3, hex(0x3A2A1A), 1.4 * s),
            EyeState::Happy => ui.arc(ex, ey + 2.0, 3.6 * s, -1.2, 2.4, hex(0x3A2A1A), 1.6 * s),
        }
    }
    ui.polygon(&[(p.x(-1.8), p.y(3.5)), (p.x(1.8), p.y(3.5)), (p.x(0.0), p.y(7.0))], col::ACCENT);
    15.0 * s
}

fn panda(ui: &mut Ui, p: &Pose) -> f32 {
    let s = p.s;
    let dark = hex(0x2A2A2E);
    // Ears get a faint rim so they show on the black notch.
    for side in [-1.0f32, 1.0] {
        ui.circle(p.x(side * 9.5), p.y(-8.0), 4.4 * s, hex(0x4A4A50));
        ui.stroke_circle(p.x(side * 9.5), p.y(-8.0), 4.4 * s, col::TEXT.with_a(0.35), 0.8 * s);
    }
    ui.ellipse(p.x(0.0), p.y(1.0), 12.5 * s, 10.8 * s, col::TEXT);
    for side in [-1.0f32, 1.0] {
        ui.ellipse(p.x(side * 5.2), p.y(0.0), 3.8 * s, 4.6 * s, dark);
    }
    eye_pair(ui, p, 5.2, -0.2, 1.4, 1.8, col::TEXT, 0.4);
    ui.ellipse(p.x(0.0), p.y(5.0), 2.0 * s, 1.3 * s, dark);
    ui.arc(p.x(0.0), p.y(5.6), 2.2 * s, 2.4, 1.5, dark, 0.9 * s);
    14.0 * s
}
