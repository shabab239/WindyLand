//! To-do list with optional due times. When a task is due, the notch taps
//! you on the shoulder with a toast (Done / Snooze).

use crate::gfx::*;
use crate::modules::{Ctx, Module, Rel, Toast};
use crate::store::{self, esc, unesc};
use crate::time::{fmt_mod, unix_now, Now};
use crate::ui::Ui;

#[derive(Clone)]
struct Task {
    id: u32,
    title: String,
    /// Unix time when due.
    due: Option<f64>,
    done: bool,
    done_day: String,
    reminded: bool,
}

struct Editor {
    id: Option<u32>,
    text: String,
    /// Due as minutes-of-day today (or tomorrow if earlier than now).
    due: Option<u32>,
}

const A_ADD: u32 = 1;
const A_SAVE: u32 = 2;
const A_CANCEL: u32 = 3;
const A_FIELD: u32 = 4;
const A_NO_TIME: u32 = 5;
const A_MINUS: u32 = 6;
const A_PLUS: u32 = 7;
const A_CLEAR_DONE: u32 = 8;
const A_QUICK: u32 = 10; // + index into QUICK
const QUICK: [(u32, &str); 4] = [(15, "+15m"), (30, "+30m"), (60, "+1h"), (120, "+2h")];

// Per-task action ids: kind << 24 | task id
const K_CHECK: u32 = 1;
const K_EDIT: u32 = 2;
const K_DELETE: u32 = 3;
const K_T_DONE: u32 = 4;
const K_T_SNOOZE: u32 = 5;

fn tid(kind: u32, id: u32) -> u32 {
    (kind << 24) | (id & 0xFF_FFFF)
}

pub struct Todo {
    tasks: Vec<Task>,
    next_id: u32,
    scroll: f32,
    editor: Option<Editor>,
}

impl Todo {
    pub fn load() -> Todo {
        let today = Now::get().day_key();
        let mut tasks = Vec::new();
        let mut next_id = 1;
        for line in store::read("todo.txt").lines() {
            let p: Vec<&str> = line.split('\t').collect();
            if p.len() < 5 {
                continue;
            }
            let id: u32 = p[0].parse().unwrap_or(0);
            let task = Task {
                id,
                done: p[1] == "1",
                due: p[2].parse::<f64>().ok().filter(|d| *d > 0.0),
                done_day: p[3].to_string(),
                title: unesc(p[4]),
                reminded: p.get(5).map_or(false, |v| *v == "1"),
            };
            // Completed tasks from earlier days are cleared automatically.
            if task.done && task.done_day != today {
                continue;
            }
            next_id = next_id.max(id + 1);
            tasks.push(task);
        }
        Todo { tasks, next_id, scroll: 0.0, editor: None }
    }

    fn save(&self) {
        let s: String = self
            .tasks
            .iter()
            .map(|t| {
                format!(
                    "{}\t{}\t{}\t{}\t{}\t{}\n",
                    t.id,
                    t.done as u8,
                    t.due.unwrap_or(0.0),
                    t.done_day,
                    esc(&t.title),
                    t.reminded as u8
                )
            })
            .collect();
        store::write("todo.txt", &s);
    }

    fn find(&mut self, id: u32) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    fn set_done(&mut self, id: u32, done: bool) {
        let today = Now::get().day_key();
        if let Some(t) = self.find(id) {
            t.done = done;
            t.done_day = if done { today } else { String::new() };
        }
        self.save();
    }

    fn open_editor(&mut self, id: Option<u32>, ctx: &mut Ctx) {
        let (text, due) = match id.and_then(|i| self.tasks.iter().find(|t| t.id == i)) {
            Some(t) => {
                let due = t.due.map(|d| {
                    let now = Now::get();
                    let mins = ((d - now.unix) / 60.0).round() as i64 + now.minute_of_day() as i64;
                    mins.rem_euclid(1440) as u32
                });
                (t.title.clone(), due)
            }
            None => (String::new(), None),
        };
        self.editor = Some(Editor { id, text, due });
        ctx.want_keyboard(true);
    }

    fn commit(&mut self, ctx: &mut Ctx) {
        let Some(ed) = self.editor.take() else { return };
        ctx.want_keyboard(false);
        let title = ed.text.trim().to_string();
        if title.is_empty() {
            return;
        }
        let due = ed.due.map(|m| {
            let now = Now::get();
            let mut delta = m as f64 - now.minute_of_day() as f64;
            if delta <= 0.0 {
                delta += 1440.0;
            }
            now.unix - now.sec as f64 + delta * 60.0
        });
        match ed.id.and_then(|i| self.tasks.iter_mut().find(|t| t.id == i)) {
            Some(t) => {
                t.title = title;
                if t.due != due {
                    t.reminded = false;
                }
                t.due = due;
            }
            None => {
                let id = self.next_id;
                self.next_id += 1;
                self.tasks.push(Task { id, title, due, done: false, done_day: String::new(), reminded: false });
            }
        }
        self.save();
    }

    fn cancel(&mut self, ctx: &mut Ctx) {
        self.editor = None;
        ctx.want_keyboard(false);
    }

    fn draw_list(&mut self, ui: &mut Ui, mut r: Rect) {
        let today = Now::get().day_key();
        let done = self.tasks.iter().filter(|t| t.done && t.done_day == today).count();
        let total = self.tasks.len();
        let mut head = r.cut_top(26.0, 8.0);
        ui.button_accent(head.cut_right(92.0, 8.0), "Add Task", A_ADD, col::YELLOW);
        if done > 0 {
            ui.button(head.cut_right(84.0, 8.0), "Clear done", A_CLEAR_DONE);
        }
        ui.text(&format!("{done}/{total} Task Done Today"), head, 12.0, col::DIM, Weight::Regular, Align::Left);

        if self.tasks.is_empty() {
            ui.text("No tasks. Add one and give it a time to get a reminder.", r, 12.0, col::FAINT, Weight::Regular, Align::Center);
            return;
        }

        // Undone first (by due time), done last.
        let mut order: Vec<Task> = self.tasks.clone();
        order.sort_by(|a, b| {
            a.done
                .cmp(&b.done)
                .then(a.due.unwrap_or(f64::MAX).partial_cmp(&b.due.unwrap_or(f64::MAX)).unwrap())
                .then(a.id.cmp(&b.id))
        });
        let row_h = 30.0;
        let max_scroll = (order.len() as f32 * row_h - r.h).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        ui.push_clip(r);
        let now = unix_now();
        for (i, t) in order.iter().enumerate() {
            let y = r.y + i as f32 * row_h - self.scroll;
            if y + row_h < r.y || y > r.b() {
                continue;
            }
            let mut row = Rect::new(r.x, y, r.w, row_h - 2.0);
            if ui.hovered(row) {
                ui.fill_rr(row, 8.0, col::TEXT.with_a(0.05));
            }
            let cb = row.cut_left(28.0, 4.0);
            ui.check(cb.cx(), cb.cy(), t.done, col::YELLOW);
            ui.hit(cb, tid(K_CHECK, t.id));
            if ui.hovered(row) || ui.hovered(Rect::new(row.r() - 30.0, row.y, 30.0, row.h)) {
                let del = row.cut_right(26.0, 4.0);
                ui.icon_btn(del.centered(24.0, 24.0), glyph::DELETE, tid(K_DELETE, t.id), 11.0, col::DIM, None);
            }
            if let Some(due) = t.due {
                let label = {
                    let n = Now::get();
                    let mins = ((due - n.unix) / 60.0).round() as i64 + n.minute_of_day() as i64;
                    let tomorrow = mins >= 1440;
                    format!("{}{}", fmt_mod(mins.rem_euclid(1440) as u32), if tomorrow { " +1d" } else { "" })
                };
                let w = ui.measure(&label, 11.0, Weight::Semi) + 16.0;
                let chip = row.cut_right(w, 6.0);
                let overdue = !t.done && due <= now;
                let c = if t.done { col::FAINT } else if overdue { col::RED } else { col::YELLOW };
                ui.fill_rr(chip.centered(chip.w, 20.0), 10.0, c.with_a(0.16));
                ui.text(&label, chip, 11.0, c, Weight::Semi, Align::Center);
            }
            let fg = if t.done { col::FAINT } else { col::TEXT };
            ui.text(&t.title, row, 13.0, fg, Weight::Regular, Align::Left);
            if t.done {
                let w = ui.measure(&t.title, 13.0, Weight::Regular).min(row.w);
                ui.line(row.x, row.cy() + 0.5, row.x + w, row.cy() + 0.5, col::FAINT, 1.0);
            }
            ui.hit(row, tid(K_EDIT, t.id));
        }
        ui.pop_clip();
    }

    fn draw_editor(&mut self, ui: &mut Ui, mut r: Rect) {
        let Some(ed) = &self.editor else { return };
        let title = if ed.id.is_some() { "Edit Task" } else { "Add Task" };
        ui.text(title, r.cut_top(18.0, 6.0), 12.0, col::DIM, Weight::Regular, Align::Left);

        let field = r.cut_top(34.0, 10.0);
        ui.fill_rr(field, 10.0, col::TEXT.with_a(0.08));
        ui.stroke_rr(field, 10.0, col::YELLOW.with_a(0.6), 1.0);
        ui.hit(field, A_FIELD);
        let tr = field.inset_xy(12.0, 0.0);
        if ed.text.is_empty() {
            ui.text("Task title", tr, 13.0, col::FAINT, Weight::Regular, Align::Left);
        }
        let shown = ed.text.clone();
        let tw = ui.measure(&shown, 13.0, Weight::Regular);
        // Keep the caret visible for long titles by right-aligning overflow.
        let align = if tw > tr.w - 6.0 { Align::Right } else { Align::Left };
        ui.text(&shown, Rect::new(tr.x, tr.y, tr.w - 4.0, tr.h), 13.0, col::TEXT, Weight::Regular, align);
        if (ui.t * 1.8) as i64 % 2 == 0 {
            let cx = if align == Align::Right { tr.r() - 4.0 } else { tr.x + tw + 1.0 };
            ui.line(cx, tr.cy() - 8.0, cx, tr.cy() + 8.0, col::YELLOW, 1.5);
        }

        let mut row = r.cut_top(26.0, 10.0);
        ui.icon(glyph::ALARM, row.cut_left(18.0, 6.0), 12.0, col::DIM);
        ui.chip(row.cut_left(70.0, 6.0), "No time", A_NO_TIME, ed.due.is_none(), col::YELLOW);
        for (i, (_, label)) in QUICK.iter().enumerate() {
            ui.chip(row.cut_left(48.0, 6.0), label, A_QUICK + i as u32, false, col::YELLOW);
        }
        if let Some(m) = ed.due {
            let mut right = row;
            ui.icon_btn(right.cut_right(24.0, 4.0), glyph::ADD, A_PLUS, 10.0, col::TEXT, Some(col::BTN));
            let tl = right.cut_right(50.0, 4.0);
            ui.text(&fmt_mod(m), tl, 13.0, col::YELLOW, Weight::Semi, Align::Center);
            ui.icon_btn(right.cut_right(24.0, 4.0), "\u{E738}", A_MINUS, 10.0, col::TEXT, Some(col::BTN));
        }

        let mut row = r.cut_top(30.0, 0.0);
        ui.button_accent(row.cut_right(90.0, 8.0), "Save", A_SAVE, col::YELLOW);
        ui.button(row.cut_right(80.0, 8.0), "Cancel", A_CANCEL);
        ui.text("Enter to save · Esc to cancel", row, 11.0, col::FAINT, Weight::Regular, Align::Left);
    }
}

impl Module for Todo {
    fn title(&self) -> &'static str {
        "TodoList"
    }
    fn icon(&self) -> &'static str {
        glyph::TODO
    }
    fn size(&self) -> (f32, f32) {
        if self.editor.is_some() {
            (460.0, 150.0)
        } else {
            let rows = self.tasks.len().clamp(1, 5) as f32;
            (460.0, 34.0 + rows * 30.0)
        }
    }

    fn tick(&mut self, ctx: &mut Ctx) {
        let now = ctx.now.unix;
        let mut changed = false;
        for t in self.tasks.iter_mut() {
            if !t.done && !t.reminded {
                if let Some(due) = t.due {
                    if now >= due {
                        t.reminded = true;
                        changed = true;
                        ctx.toast(
                            Toast::new("todo", glyph::ALARM, col::YELLOW, t.title.clone(), "Task due now")
                                .action("Done", tid(K_T_DONE, t.id))
                                .action("Snooze 10m", tid(K_T_SNOOZE, t.id))
                                .secs(20.0)
                                .chime(),
                        );
                    }
                }
            }
        }
        if changed {
            self.save();
        }
    }

    fn draw(&mut self, ui: &mut Ui, r: Rect) {
        if self.editor.is_some() {
            self.draw_editor(ui, r);
        } else {
            self.draw_list(ui, r);
        }
    }

    fn action(&mut self, id: u32, _rel: Rel, ctx: &mut Ctx) {
        let kind = id >> 24;
        let task = id & 0xFF_FFFF;
        if kind != 0 {
            match kind {
                K_CHECK => {
                    let done = self.tasks.iter().find(|t| t.id == task).map_or(false, |t| t.done);
                    self.set_done(task, !done);
                }
                K_EDIT => self.open_editor(Some(task), ctx),
                K_DELETE => {
                    self.tasks.retain(|t| t.id != task);
                    self.save();
                }
                K_T_DONE => self.set_done(task, true),
                K_T_SNOOZE => {
                    if let Some(t) = self.find(task) {
                        t.due = Some(unix_now() + 600.0);
                        t.reminded = false;
                    }
                    self.save();
                }
                _ => {}
            }
            return;
        }
        match id {
            A_ADD => self.open_editor(None, ctx),
            A_SAVE => self.commit(ctx),
            A_CANCEL => self.cancel(ctx),
            A_FIELD => ctx.want_keyboard(true),
            A_CLEAR_DONE => {
                self.tasks.retain(|t| !t.done);
                self.save();
            }
            A_NO_TIME => {
                if let Some(e) = &mut self.editor {
                    e.due = None;
                }
            }
            A_MINUS | A_PLUS => {
                if let Some(e) = &mut self.editor {
                    if let Some(m) = e.due {
                        let d: i64 = if id == A_PLUS { 15 } else { -15 };
                        e.due = Some((m as i64 + d).rem_euclid(1440) as u32);
                    }
                }
            }
            i if (A_QUICK..A_QUICK + QUICK.len() as u32).contains(&i) => {
                if let Some(e) = &mut self.editor {
                    let add = QUICK[(i - A_QUICK) as usize].0;
                    let now = Now::get().minute_of_day();
                    let base = e.due.unwrap_or(now);
                    e.due = Some((base + add) % 1440);
                }
                ctx.want_keyboard(true);
            }
            _ => {}
        }
    }

    fn scroll(&mut self, delta: f32) {
        self.scroll -= delta * 30.0;
    }

    fn key(&mut self, vk: u16, ctrl: bool, ctx: &mut Ctx) -> bool {
        let Some(ed) = &mut self.editor else { return false };
        match vk {
            0x0D => self.commit(ctx),         // Enter
            0x1B => self.cancel(ctx),         // Esc
            0x08 => {
                // Backspace (Ctrl+Backspace deletes a word)
                if ctrl {
                    let trimmed = ed.text.trim_end().to_string();
                    let cut = trimmed.rfind(' ').map(|i| i + 1).unwrap_or(0);
                    ed.text.truncate(cut);
                } else {
                    ed.text.pop();
                }
            }
            0x56 if ctrl => {
                // Ctrl+V
                if let Some(s) = clipboard_text() {
                    ed.text.push_str(s.lines().next().unwrap_or(""));
                }
            }
            _ => return false,
        }
        true
    }

    fn chr(&mut self, c: char, _ctx: &mut Ctx) -> bool {
        if let Some(ed) = &mut self.editor {
            if ed.text.chars().count() < 120 {
                ed.text.push(c);
            }
            true
        } else {
            false
        }
    }

    fn animating(&self) -> bool {
        self.editor.is_some()
    }

    fn live(&self) -> Option<u8> {
        let now = unix_now();
        let overdue = self.tasks.iter().any(|t| !t.done && t.due.map_or(false, |d| d <= now));
        if overdue {
            Some(20)
        } else {
            None
        }
    }

    fn live_color(&self) -> Color {
        col::YELLOW
    }

    fn draw_live(&mut self, ui: &mut Ui, left: Rect, right: Rect) {
        let now = unix_now();
        let n = self.tasks.iter().filter(|t| !t.done && t.due.map_or(false, |d| d <= now)).count();
        ui.icon(glyph::ALARM, Rect::new(left.x, left.y, left.h, left.h), 13.0, col::YELLOW);
        ui.text("Due", Rect::new(left.x + left.h + 4.0, left.y, left.w, left.h), 12.0, col::YELLOW, Weight::Semi, Align::Left);
        ui.text(&format!("{n} task{}", if n == 1 { "" } else { "s" }), right, 12.0, col::TEXT, Weight::Semi, Align::Right);
    }
}

fn clipboard_text() -> Option<String> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    unsafe {
        OpenClipboard(None).ok()?;
        let out = (|| {
            let h = GetClipboardData(13).ok()?; // CF_UNICODETEXT
            let hg = HGLOBAL(h.0);
            let p = GlobalLock(hg) as *const u16;
            if p.is_null() {
                return None;
            }
            let mut len = 0;
            while *p.add(len) != 0 && len < 4096 {
                len += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
            let _ = GlobalUnlock(hg);
            Some(s)
        })();
        let _ = CloseClipboard();
        out
    }
}
