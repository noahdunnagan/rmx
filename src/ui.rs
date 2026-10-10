//! What rmx draws: the list of tabs (a rail when the terminal is wide, the
//! whole screen on a phone), the tab itself, a bottom bar of buttons, and
//! sheets that rise above the bar: the menu, questions, messages.
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::point_to_viewport;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use unicode_width::UnicodeWidthStr;

use crate::app::{Action, App, Confirm, MenuItem, Mode, Prompt, Row, MENU, RAIL_W};
use crate::model::{Model, State, Status};
use crate::render::{Cell, Chrome, Frame, Style, DARK};

const PAD: usize = 2;
const SPIN: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// What a tap on a bar button does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tap {
    Run(Action),
    Menu,
    Back,
    Up,
    Down,
    Bottom,
    Yes,
    No,
    Quit,
}

fn accent() -> Color {
    Color::Named(NamedColor::Cyan)
}

fn dim() -> Style {
    Style::default().dim()
}

pub fn compose(app: &App) -> Frame {
    let mut f = Frame::new(app.cols, app.rows, Chrome::from(app.bg.unwrap_or(DARK)));
    let body = app.rows.saturating_sub(app.bar_h());
    if app.wide() {
        draw_list(app, &mut f, RAIL_W, body);
        draw_pane(app, &mut f);
    } else if matches!(app.mode, Mode::Switch { .. }) || app.view.is_none() {
        draw_list(app, &mut f, app.cols, body);
    } else {
        draw_pane(app, &mut f);
    }
    match &app.mode {
        Mode::Prefix => draw_menu(app, &mut f),
        Mode::Confirm(_) | Mode::Prompt { .. } => draw_question(app, &mut f),
        _ => draw_note(app, &mut f),
    }
    draw_bar(app, &mut f, body);
    f
}

// ---- the list ---------------------------------------------------------

fn glyph(app: &App, status: Option<&Status>) -> (String, Style) {
    match status.map(|s| s.state) {
        Some(State::Working) => (SPIN[app.tick % SPIN.len()].into(), Style::fg(NamedColor::Blue)),
        Some(State::Blocked) => ("●".into(), Style::fg(NamedColor::Yellow).bold()),
        Some(State::Done) => ("●".into(), Style::fg(NamedColor::Green)),
        Some(State::Error) => ("●".into(), Style::fg(NamedColor::Red)),
        Some(State::Idle) | None => ("○".into(), dim()),
    }
}

fn short_cwd(cwd: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let cwd = match cwd.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => cwd.to_string(),
    };
    let parts: Vec<&str> = cwd.split('/').collect();
    if parts.len() > 3 {
        format!("…/{}", parts[parts.len() - 2..].join("/"))
    } else {
        cwd
    }
}

/// The tab's second line: what its program says, else what is running where.
fn detail(model: &Model, session: &str, tab: &str) -> (String, Style) {
    let Some(t) = model.tab(session, tab) else { return (String::new(), dim()) };
    if let Some(st) = model.tab_status(t) {
        let (fallback, style) = match st.state {
            State::Blocked => ("needs you", Style::fg(NamedColor::Yellow)),
            State::Done => ("done", Style::fg(NamedColor::Green).dim()),
            State::Error => ("failed", Style::fg(NamedColor::Red)),
            State::Working => ("working", dim()),
            State::Idle => ("idle", dim()),
        };
        let text = if st.msg.is_empty() { fallback.to_string() } else { st.msg.clone() };
        return (text, style);
    }
    let b = Model::tab_block(t).and_then(|b| model.blocks.get(&b));
    match b {
        Some(b) if !b.cwd.is_empty() => (format!("{} · {}", b.process, short_cwd(&b.cwd)), dim()),
        Some(b) => (b.process.clone(), dim()),
        None => (String::new(), dim()),
    }
}

/// The tab a list line belongs to.
fn card(row: &Row) -> Option<(&str, &str)> {
    match row {
        Row::Tab(s, t) | Row::Detail(s, t) | Row::Gap(s, t) | Row::Block(s, t, _) => Some((s, t)),
        _ => None,
    }
}

fn draw_list(app: &App, f: &mut Frame, width: usize, height: usize) {
    let inner = if app.wide() { width - 1 } else { width };
    let ch = f.chrome;
    let rows = app.rows();
    let sel = match app.mode {
        Mode::Switch { sel } => rows.get(sel),
        _ => None,
    };
    let sel_card = sel.and_then(card);
    let current = app.view.as_ref().map(|v| (v.session.as_str(), v.tab.as_str(), v.block.as_str()));
    let add = Style::color(accent());
    let max = inner.saturating_sub(PAD + 1);
    let (mut tint_selected, mut tint_current) = (Vec::new(), Vec::new());
    for (idx, row) in rows.iter().enumerate().skip(app.list_offset).take(height) {
        let y = idx - app.list_offset;
        let this = card(row);
        let here = this.is_some() && this == current.map(|c| (c.0, c.1));
        if here {
            f.text(PAD - 1, y, 1, "▎", Style::color(accent()));
        }
        if this.is_some() && this == sel_card || sel == Some(row) {
            tint_selected.push(y);
            if app.wide() {
                f.text(0, y, 1, "›", Style::color(accent()).bold());
            }
        } else if here {
            tint_current.push(y);
        }
        match row {
            Row::Header => {
                let (waiting, blocked) = app.waiting_all();
                let right = match waiting {
                    0 => String::new(),
                    n => format!("● {n} need{} you", if n == 1 { "s" } else { "" }),
                };
                let rw = right.width();
                f.text(PAD, y, inner.saturating_sub(PAD + rw + 2), &format!("Rex on {}", crate::host()), Style::default().bold());
                let color = if blocked { NamedColor::Yellow } else { NamedColor::Green };
                f.text(inner.saturating_sub(rw + PAD), y, rw, &right, Style::fg(color).bold());
            }
            Row::Session(s) => {
                let Some(session) = app.model.session(s) else { continue };
                let style = if current.is_some_and(|c| c.0 == s) { Style::color(accent()).bold() } else { Style::default().bold().dim() };
                let w = f.text(PAD, y, max, &session.label, style);
                let n = session.tabs.len();
                let count = format!(" · {n} tab{}", if n == 1 { "" } else { "s" });
                f.text(PAD + w, y, max.saturating_sub(w), &count, dim());
            }
            Row::Tab(s, t) => {
                let Some(tab) = app.model.tab(s, t) else { continue };
                let status = app.model.tab_status(tab);
                let (g, gs) = glyph(app, status);
                f.text(PAD + 1, y, 1, &g, gs);
                let name = app.model.tab_name(tab);
                let chip = status.map(|s| s.app.clone()).filter(|a| !a.eq_ignore_ascii_case(&name)).unwrap_or_default();
                let cw = chip.width().min(12);
                let name_w = inner.saturating_sub(PAD + 4 + if cw > 0 { cw + 2 } else { 0 });
                let style = if here { Style::color(accent()).bold() } else { Style::default().bold() };
                f.text(PAD + 3, y, name_w, &name, style);
                f.text(inner.saturating_sub(cw + PAD), y, cw, &chip, dim());
            }
            Row::Detail(s, t) => {
                let (text, style) = detail(&app.model, s, t);
                f.text(PAD + 3, y, inner.saturating_sub(PAD + 4), &text, style);
            }
            Row::Block(_, _, b) => {
                let info = app.model.blocks.get(b);
                let name = info.map(|i| if i.title.trim().is_empty() { i.process.clone() } else { i.title.trim().to_string() }).unwrap_or_default();
                let style = if current.is_some_and(|c| c.2 == b) { Style::color(accent()) } else { dim() };
                f.text(PAD + 3, y, 2, "├ ", dim());
                f.text(PAD + 5, y, inner.saturating_sub(PAD + 6), &name, style);
            }
            Row::AddTab(_) => {
                f.text(PAD + 3, y, max, "+ New tab", add);
            }
            Row::AddSession if app.model.sessions.is_empty() => {
                f.text(PAD, y, max, "+ New tab", add.bold());
            }
            Row::AddSession => {
                let w = f.text(PAD, y, max, "+ New window", add.bold());
                f.text(PAD + w + 2, y, max.saturating_sub(w + 2), "its own window on the Mac", dim());
            }
            Row::Empty => {
                f.text(PAD, y, max, "No tabs open on the Mac yet.", Style::default());
            }
            Row::Gap(..) | Row::Blank => {}
        }
    }
    // Text replaces whole cells, so tints go on last.
    for y in tint_selected {
        f.tint(0, y, width, 1, ch.selected);
    }
    for y in tint_current {
        f.tint(0, y, width, 1, ch.hover);
    }
    f.tint(0, 0, width, height, ch.surface);
}

// ---- the tab ----------------------------------------------------------

fn draw_pane(app: &App, f: &mut Frame) {
    let (x0, width, height) = app.pane_rect();
    let Some(v) = &app.view else {
        let text = if app.model.sessions.is_empty() { "No tabs open on the Mac yet. + Tab starts one." } else { "Pick a tab on the left." };
        f.text(x0 + PAD, 1, width.saturating_sub(PAD), text, dim());
        return;
    };
    let term = &v.term;
    let content = term.renderable_content();
    let colors = content.colors;
    let offset = content.display_offset;
    // A block taller than the pane shows its bottom, where prompts live.
    let shift = term.screen_lines().saturating_sub(height);
    let cols = term.columns();
    for indexed in content.display_iter {
        let Some(vp) = point_to_viewport(offset, indexed.point) else { continue };
        let (Some(row), col) = (vp.line.checked_sub(shift), vp.column.0) else { continue };
        if row >= height || col >= width.min(cols) {
            continue;
        }
        let cell = indexed.cell;
        let flags = cell.flags & (Flags::INVERSE | Flags::BOLD | Flags::ITALIC | Flags::ALL_UNDERLINES | Flags::DIM | Flags::HIDDEN | Flags::STRIKEOUT | Flags::WIDE_CHAR | Flags::WIDE_CHAR_SPACER);
        let resolve = |c: Color| match c {
            Color::Named(n) if (n as usize) < 16 => colors[n as usize].map(Color::Spec).unwrap_or(c),
            Color::Named(n @ (NamedColor::Foreground | NamedColor::Background)) => colors[n].map(Color::Spec).unwrap_or(c),
            Color::Indexed(i) => colors[i as usize].map(Color::Spec).unwrap_or(c),
            c => c,
        };
        let c = if cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) { ' ' } else { cell.c };
        let zw = cell.zerowidth().filter(|z| !z.is_empty()).map(|z| z.to_vec().into_boxed_slice());
        f.put(x0 + col, row, Cell { c, zw, style: Style { fg: resolve(cell.fg), bg: resolve(cell.bg), flags } });
    }
    let cursor = content.cursor;
    if cursor.shape != CursorShape::Hidden && matches!(app.mode, Mode::Normal) && app.note.is_none() {
        if let Some(vp) = point_to_viewport(offset, cursor.point) {
            if let Some(row) = vp.line.checked_sub(shift).filter(|r| *r < height && vp.column.0 < width) {
                f.cursor = Some((x0 + vp.column.0, row));
                f.cursor_shape = cursor.shape;
                f.cursor_blink = term.cursor_style().blinking;
            }
        }
    }
}

// ---- sheets -----------------------------------------------------------

/// (x, w) of a sheet: the whole width on a phone, the pane's left part when wide.
fn sheet_span(app: &App) -> (usize, usize) {
    let (px, pcols, _) = app.pane_rect();
    if app.wide() {
        (px, pcols.min(60))
    } else {
        (0, app.cols)
    }
}

/// Word-wrap `text` to `width`.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split_whitespace() {
        let line = lines.last_mut().unwrap();
        if !line.is_empty() && line.width() + 1 + word.width() > width {
            lines.push(word.to_string());
        } else {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    lines
}

/// A sheet of text lines resting on the bar, with a blank line above and
/// below. Returns the row the first line sits on.
fn sheet(app: &App, f: &mut Frame, lines: &[(String, Style)]) -> usize {
    let (x, w) = sheet_span(app);
    let bottom = app.pane_rect().2;
    let h = (lines.len() + 2).min(bottom);
    let top = bottom - h;
    f.fill(x, top, w, h, Style::default());
    for (i, (text, style)) in lines.iter().enumerate().take(h.saturating_sub(2)) {
        f.text(x + PAD, top + 1 + i, w.saturating_sub(PAD * 2), text, *style);
    }
    f.tint(x, top, w, h, f.chrome.selected);
    f.cursor = None;
    top + 1
}

fn draw_note(app: &App, f: &mut Frame) {
    let Some(note) = &app.note else { return };
    let style = if note.error { Style::fg(NamedColor::Red).bold() } else { Style::default() };
    let width = sheet_span(app).1.saturating_sub(PAD * 2);
    let mut lines: Vec<(String, Style)> = wrap(&note.text, width).into_iter().map(|l| (l, style)).collect();
    lines.push((String::new(), dim()));
    lines.push(("Tap anywhere to dismiss.".into(), dim()));
    sheet(app, f, &lines);
}

/// Close / restart / rename: what is being asked, with the answers in the bar.
fn draw_question(app: &App, f: &mut Frame) {
    let width = sheet_span(app).1.saturating_sub(PAD * 2);
    let title = |t: String| (t, Style::default().bold());
    let body = |t: &str| wrap(t, width).into_iter().map(|l| (l, Style::default())).collect::<Vec<_>>();
    let mut lines = Vec::new();
    match &app.mode {
        Mode::Confirm(Confirm::Close { name, .. }) => {
            lines.push(title(format!("Close “{name}”?")));
            lines.extend(body("Whatever is running in it stops."));
        }
        Mode::Confirm(Confirm::Restart { tabs }) => {
            lines.push(title("Restart Rex's server?".into()));
            let s = if *tabs == 1 { "" } else { "s" };
            lines.extend(body(&format!(
                "Rex updated, but its old server is still running and can't start new shells. Restarting it fixes that, but closes the {tabs} open tab{s}."
            )));
        }
        Mode::Prompt { kind, buf } => {
            let (what, now) = match kind {
                Prompt::RenameTab(s, t) => ("Rename tab", app.model.tab(s, t).map(|t| app.model.tab_name(t)).unwrap_or_default()),
                Prompt::RenameSession(s) => ("Rename window", app.model.session(s).map(|s| s.label.clone()).unwrap_or_default()),
            };
            lines.push(title(what.into()));
            lines.push((String::new(), dim()));
            lines.push((String::new(), dim()));
            let top = sheet(app, f, &lines);
            let (x, w) = sheet_span(app);
            let y = top + 2;
            let x = x + PAD + f.text(x + PAD, y, 2, "› ", Style::color(accent()));
            if buf.is_empty() {
                f.text(x, y, w.saturating_sub(x + PAD), &now, dim());
                f.cursor = Some((x, y));
            } else {
                let used = f.text(x, y, w.saturating_sub(x + PAD), buf, Style::color(accent()).bold());
                f.cursor = Some((x + used, y));
            }
            f.cursor_shape = CursorShape::Beam;
            return;
        }
        _ => return,
    }
    sheet(app, f, &lines);
}

fn menu_items(app: &App) -> Vec<&'static MenuItem> {
    let phone = !app.wide();
    MENU.iter().filter(|m| (!m.needs_tab || app.view.is_some()) && (m.phone || !phone)).collect()
}

/// (x, y, w, h, row height) of the menu sheet: a title, a blank line, then
/// one slot per item. Slots get a blank line under the label when there is
/// room, so they are easy to tap.
fn menu_rect(app: &App) -> (usize, usize, usize, usize, usize) {
    let (x, w) = sheet_span(app);
    let bottom = app.pane_rect().2;
    let n = menu_items(app).len();
    let row_h = if n * 2 + 3 <= bottom { 2 } else { 1 };
    let h = (n * row_h + 3).min(bottom);
    (x, bottom - h, w, h, row_h)
}

fn draw_menu(app: &App, f: &mut Frame) {
    let (x, y, w, h, row_h) = menu_rect(app);
    f.fill(x, y, w, h, Style::default());
    let title = match &app.view {
        Some(v) if !app.wide() => app.model.tab(&v.session, &v.tab).map(|t| app.model.tab_name(t)).unwrap_or_default(),
        _ => "Menu · or ^B then the key".into(),
    };
    f.text(x + PAD, y + 1, w.saturating_sub(PAD * 2), &title, dim());
    let label_w = 15;
    for (i, item) in menu_items(app).iter().enumerate() {
        let row = y + 3 + i * row_h;
        if row >= y + h {
            break;
        }
        let style = if item.action == Action::CloseTab { Style::fg(NamedColor::Red).bold() } else { Style::default().bold() };
        f.text(x + PAD, row, label_w, item.label, style);
        let keys = if app.wide() { 4 } else { 0 };
        f.text(x + PAD + label_w, row, w.saturating_sub(PAD * 2 + label_w + keys), item.hint, dim());
        if app.wide() {
            f.text(x + w.saturating_sub(PAD + 1), row, 1, item.key, Style::color(accent()));
        }
    }
    f.tint(x, y, w, h, f.chrome.selected);
    f.cursor = None;
}

pub fn menu_hit(app: &App, x: usize, y: usize) -> Option<Action> {
    let (mx, my, w, h, row_h) = menu_rect(app);
    let first = my + 3;
    if x < mx || x >= mx + w || y < first || y >= my + h {
        return None;
    }
    menu_items(app).get((y - first) / row_h).map(|m| m.action)
}

// ---- the bar ----------------------------------------------------------

struct Button {
    text: String,
    tap: Tap,
    style: Style,
}

fn button(text: impl Into<String>, tap: Tap) -> Button {
    Button { text: text.into(), tap, style: Style::color(accent()).bold() }
}

fn danger(text: impl Into<String>, tap: Tap) -> Button {
    Button { style: Style::fg(NamedColor::Red).bold(), ..button(text, tap) }
}

struct Bar {
    left: Vec<Button>,
    text: String,
    style: Style,
    right: Vec<Button>,
    /// Buttons share the whole width (answers to a question).
    fill: bool,
}

fn bar(left: Vec<Button>, text: impl Into<String>, style: Style, right: Vec<Button>) -> Bar {
    Bar { left, text: text.into(), style, right, fill: false }
}

fn answers(buttons: Vec<Button>) -> Bar {
    Bar { left: buttons, text: String::new(), style: dim(), right: vec![], fill: true }
}

fn bar_parts(app: &App) -> Bar {
    let phone = !app.wide();
    if app.lost.is_some() {
        return bar(vec![], "Reconnecting to Rex…", Style::fg(NamedColor::Yellow), vec![button("Exit", Tap::Quit)]);
    }
    match &app.mode {
        Mode::Confirm(Confirm::Close { .. }) => answers(vec![danger("Close tab", Tap::Yes), button("Cancel", Tap::No)]),
        Mode::Confirm(Confirm::Restart { .. }) => answers(vec![danger("Restart", Tap::Yes), button("Cancel", Tap::No)]),
        Mode::Prompt { .. } => answers(vec![button("Save", Tap::Yes), button("Cancel", Tap::No)]),
        Mode::Prefix if phone => answers(vec![button("Cancel", Tap::Menu)]),
        Mode::Scroll => {
            let lines = app.view.as_ref().map(|v| v.term.grid().display_offset()).unwrap_or(0);
            bar(vec![button("▲ Up", Tap::Up), button("▼ Down", Tap::Down)], format!("{lines} lines up"), Style::fg(NamedColor::Yellow), vec![button("Done", Tap::Bottom)])
        }
        Mode::Switch { .. } if phone => {
            let back = app.view.as_ref().map(|v| app.model.tab(&v.session, &v.tab).map(|t| app.model.tab_name(t)).unwrap_or_default());
            let left = back.map(|name| vec![button(format!("‹ {name}"), Tap::Back)]).unwrap_or_default();
            bar(left, "", dim(), vec![button("Exit", Tap::Quit)])
        }
        Mode::Switch { .. } => bar(vec![], "↑↓ enter · c new tab · x close · esc", dim(), vec![button("⋯ Menu", Tap::Menu)]),
        Mode::Normal | Mode::Prefix => {
            let (text, style) = breadcrumb(app);
            let (waiting, blocked) = app.waiting();
            let waiting_style = Style::fg(if blocked { NamedColor::Yellow } else { NamedColor::Green }).bold();
            let mut left = Vec::new();
            let mut right = Vec::new();
            // On a phone the count rides on the Tabs button: room is short,
            // and the list is where those tabs are.
            if phone && waiting > 0 {
                left.push(Button { style: waiting_style, ..button(format!("‹ Tabs ●{waiting}"), Tap::Run(Action::Switcher)) });
            } else if phone {
                left.push(button("‹ Tabs", Tap::Run(Action::Switcher)));
            } else if waiting > 0 {
                right.push(Button { style: waiting_style, ..button(format!("● {waiting}"), Tap::Run(Action::Attention)) });
            }
            right.push(button("+ Tab", Tap::Run(Action::NewTab)));
            let menu = button(if phone { "⋯" } else { "⋯ Menu" }, Tap::Menu);
            right.push(if matches!(app.mode, Mode::Prefix) { Button { style: menu.style.inverse(), ..menu } } else { menu });
            bar(left, text, style, right)
        }
    }
}

/// Where everything in the bar sits: buttons as (x, width, button) and the
/// text's (x, max width). Phone buttons are padded wide; a bar of answers
/// splits the whole width between them.
fn bar_layout(app: &App) -> (Vec<(usize, usize, Button)>, Bar, (usize, usize)) {
    let mut parts = bar_parts(app);
    let pad = if app.wide() { 1 } else { 2 };
    let (cols, mut placed) = (app.cols, Vec::new());
    if parts.fill {
        let n = parts.left.len().max(1);
        let each = (cols.saturating_sub(2 + n - 1)) / n;
        for (i, b) in std::mem::take(&mut parts.left).into_iter().enumerate() {
            placed.push((1 + i * (each + 1), each, b));
        }
        return (placed, parts, (0, 0));
    }
    let mut x = if app.wide() { 0 } else { 1 };
    for b in std::mem::take(&mut parts.left) {
        let w = b.text.width() + pad * 2;
        placed.push((x, w, b));
        x += w + 1;
    }
    let text_x = x + 1;
    let mut end = cols.saturating_sub(if app.wide() { 0 } else { 1 });
    let mut rights = Vec::new();
    for b in std::mem::take(&mut parts.right).into_iter().rev() {
        let w = b.text.width() + pad * 2;
        let Some(start) = end.checked_sub(w).filter(|s| *s > text_x + 2) else { break };
        rights.push((start, w, b));
        end = start.saturating_sub(1);
    }
    placed.extend(rights.into_iter().rev());
    (placed, parts, (text_x, end.saturating_sub(text_x + 1)))
}

pub fn footer_hit(app: &App, x: usize) -> Option<Tap> {
    let (buttons, _, (tx, tw)) = bar_layout(app);
    if let Some((.., b)) = buttons.into_iter().find(|(s, w, _)| x >= *s && x < s + w) {
        return Some(b.tap);
    }
    // On a phone the tab's name is a button too: it leads to the list.
    let on_text = x >= tx && x < tx + tw;
    match app.mode {
        Mode::Normal if on_text && !app.wide() && app.lost.is_none() => Some(Tap::Run(Action::Switcher)),
        Mode::Prefix => Some(Tap::Menu),
        _ => None,
    }
}

fn breadcrumb(app: &App) -> (String, Style) {
    let Some(v) = &app.view else { return ("rmx".into(), dim()) };
    let session = app.model.session(&v.session).map(|s| s.label.clone()).unwrap_or_default();
    let tab = app.model.tab(&v.session, &v.tab).map(|t| app.model.tab_name(t)).unwrap_or_default();
    if v.sized_by_other {
        return ("Mac took the size · type to take it back".into(), Style::fg(NamedColor::Yellow));
    }
    if app.wide() {
        return (format!("{session} › {tab}"), dim());
    }
    (tab, Style::default().bold())
}

fn draw_bar(app: &App, f: &mut Frame, top: usize) {
    let h = app.bar_h();
    let mid = top + h / 2;
    let (buttons, parts, (tx, tw)) = bar_layout(app);
    let mut x = tx;
    if matches!(app.mode, Mode::Normal | Mode::Prefix) && app.lost.is_none() && !parts.fill {
        let status = app.view.as_ref().and_then(|v| app.model.tab(&v.session, &v.tab)).and_then(|t| app.model.tab_status(t));
        let (g, gs) = glyph(app, status);
        f.text(x, mid, 1, &g, gs);
        x += 2;
    }
    f.text(x, mid, (tx + tw).saturating_sub(x), &parts.text, parts.style);
    for (bx, bw, b) in &buttons {
        let tw = b.text.width().min(*bw);
        f.text(bx + (bw - tw) / 2, mid, tw, &b.text, b.style);
        f.tint(*bx, top, *bw, h, f.chrome.selected);
    }
    f.tint(0, top, app.cols, h, f.chrome.surface);
}
