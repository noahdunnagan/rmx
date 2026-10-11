//! rmx's state and everything that changes it: Rex events, keys, taps.
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Instant;

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::Rgb;
use serde_json::{json, Value};

use crate::input::{self, Key, KeyPress, Mouse, Token};
use crate::model::{Model, State};
use crate::rex::{self, Control};
use crate::ui::Tap;
use crate::view::View;
use crate::Msg;

pub const RAIL_W: usize = 30;
/// Narrower than this and the rail gives way to a full-screen list.
pub const WIDE: usize = 100;
pub const SHELL: &str = "com.superlogical.terminal.shell";

pub enum Mode {
    Normal,
    /// The menu is up (^B or the ⋯ button); the next key or tap picks.
    Prefix,
    /// The list of tabs: `sel` indexes `rows()`. On a phone this is home.
    Switch { sel: usize },
    Scroll,
    Prompt { kind: Prompt, buf: String },
    Confirm(Confirm),
}

pub enum Prompt {
    RenameTab(String, String),
    RenameSession(String),
}

pub enum Confirm {
    Close { session: String, tab: String, name: String },
    /// Rex's server cannot start shells; restarting it ends `tabs` tabs.
    Restart { tabs: usize },
}

/// One line of the rail / list.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Row {
    /// Phone only: which Mac, and how many tabs need you.
    Header,
    Session(String),
    Tab(String, String),
    Detail(String, String),
    Block(String, String, String),
    /// Phone only: the blank line closing a tab's card, so the whole card
    /// is a comfortable tap target.
    Gap(String, String),
    AddTab(String),
    AddSession,
    /// "No Rex sessions yet."
    Empty,
    Blank,
}

impl Row {
    fn selectable(&self) -> bool {
        matches!(self, Row::Tab(..) | Row::Block(..) | Row::AddTab(_) | Row::AddSession)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Switcher,
    NewTab,
    NewSession,
    NextTab,
    PrevTab,
    TabN(usize),
    Attention,
    CloseTab,
    RenameTab,
    RenameSession,
    Scroll,
    ToggleRail,
    Refresh,
    Quit,
    Literal,
}

pub struct MenuItem {
    pub key: &'static str,
    pub label: &'static str,
    /// Said after the label, dimmed: what the item means.
    pub hint: &'static str,
    pub action: Action,
    /// Only makes sense with a tab open.
    pub needs_tab: bool,
    /// Shown on a phone too. The rest have a button of their own there.
    pub phone: bool,
}

const fn item(key: &'static str, label: &'static str, hint: &'static str, action: Action, needs_tab: bool, phone: bool) -> MenuItem {
    MenuItem { key, label, hint, action, needs_tab, phone }
}

/// The menu, in the order it is shown. A Rex session is what the Mac shows
/// as one window of tabs, so rmx calls it a window.
pub const MENU: [MenuItem; 11] = [
    item("s", "All tabs", "", Action::Switcher, false, false),
    item("c", "New tab", "next to this one", Action::NewTab, false, false),
    item("o", "Needs you", "next tab waiting on you", Action::Attention, false, false),
    item("[", "Scroll back", "read earlier output", Action::Scroll, true, true),
    item("r", "Rename tab", "", Action::RenameTab, true, true),
    item("$", "Rename window", "", Action::RenameSession, true, false),
    item("x", "Close tab", "stops what runs in it", Action::CloseTab, true, true),
    item("S", "New window", "its own window on the Mac", Action::NewSession, false, true),
    item("g", "Redraw", "if the screen looks off", Action::Refresh, true, true),
    item("b", "Sidebar", "show or hide", Action::ToggleRail, false, false),
    item("d", "Exit rmx", "your tabs keep running", Action::Quit, false, true),
];

fn prefixed(k: &KeyPress) -> Option<Action> {
    if k.ctrl('b') {
        return Some(Action::Literal);
    }
    if k.mods.ctrl || k.mods.alt {
        return None;
    }
    Some(match k.key {
        Key::Char('s' | 'w' | ' ') | Key::Tab => Action::Switcher,
        Key::Char('c' | 't') => Action::NewTab,
        Key::Char('S' | 'C' | 'N') => Action::NewSession,
        Key::Char('n') | Key::Right | Key::Down => Action::NextTab,
        Key::Char('p') | Key::Left | Key::Up => Action::PrevTab,
        Key::Char(c @ '1'..='9') => Action::TabN(c as usize - '1' as usize),
        Key::Char('o') => Action::Attention,
        Key::Char('x') => Action::CloseTab,
        Key::Char('r' | ',') => Action::RenameTab,
        Key::Char('$') => Action::RenameSession,
        Key::Char('[') | Key::PageUp => Action::Scroll,
        Key::Char('b') => Action::ToggleRail,
        Key::Char('g') => Action::Refresh,
        Key::Char('d' | 'q') => Action::Quit,
        _ => return None,
    })
}

/// A message above the footer, kept until the next key or tap.
pub struct Note {
    pub text: String,
    pub error: bool,
}

pub struct App {
    pub rex: Control,
    socket: PathBuf,
    /// Using Rex's own server (not `--server`), so rmx may start it.
    manage_server: bool,
    tx: Sender<Msg>,
    pub control_gen: u64,
    pub model: Model,
    pub view: Option<View>,
    generation: u64,
    pub mode: Mode,
    pub cols: usize,
    pub rows: usize,
    pub rail: bool,
    pub bg: Option<Rgb>,
    pub tick: usize,
    pub note: Option<Note>,
    /// When the server went away; rmx keeps trying to get it back.
    pub lost: Option<Instant>,
    pub quit: bool,
    /// First row of the rail / list on screen.
    pub list_offset: usize,
    /// Bytes for the outer terminal besides the frame: mode switches.
    pub out: Vec<u8>,
    mirrored: TermMode,
    stale: HashSet<String>,
    added: HashSet<String>,
}

impl App {
    pub fn new(rex: Control, socket: PathBuf, manage_server: bool, tx: Sender<Msg>, model: Model, cols: usize, rows: usize) -> App {
        App {
            rex,
            socket,
            manage_server,
            tx,
            control_gen: 0,
            model,
            view: None,
            generation: 0,
            mode: Mode::Normal,
            cols,
            rows,
            rail: true,
            bg: None,
            tick: 0,
            note: None,
            lost: None,
            quit: false,
            list_offset: 0,
            out: Vec::new(),
            mirrored: TermMode::empty(),
            stale: HashSet::new(),
            added: HashSet::new(),
        }
    }

    pub fn wide(&self) -> bool {
        self.rail && self.cols >= WIDE
    }

    /// Rows of the bottom bar. On a phone it is tall enough to tap
    /// comfortably, and gives way when the keyboard makes the screen short.
    pub fn bar_h(&self) -> usize {
        match self.rows {
            _ if self.wide() => 1,
            r if r >= 30 => 3,
            r if r >= 18 => 2,
            _ => 1,
        }
    }

    /// Where the block is drawn: (x, cols, rows).
    pub fn pane_rect(&self) -> (usize, usize, usize) {
        let x = if self.wide() { RAIL_W } else { 0 };
        (x, self.cols.saturating_sub(x).max(10), self.rows.saturating_sub(self.bar_h()).max(2))
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.note = Some(Note { text: text.into(), error: true });
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.note = Some(Note { text: text.into(), error: false });
    }

    // ---- viewing ------------------------------------------------------

    /// Where rmx starts: wide, straight into the last tab; on a phone, the
    /// list with the last tab picked.
    pub fn home(&mut self, last: Option<(String, String)>) {
        if self.wide() {
            return self.open_initial(last);
        }
        let rows = self.rows();
        let tab = last.and_then(|(s, b)| Some((s.clone(), self.model.session(&s)?.tabs.iter().find(|t| t.blocks.contains(&b))?.id.clone())));
        let sel = tab.and_then(|(s, t)| rows.iter().position(|r| *r == Row::Tab(s.clone(), t.clone()))).unwrap_or(0);
        self.mode = Mode::Switch { sel: self.nearest_selectable(sel) };
    }

    /// Open the tab Rex had focused last time, else the first one there is.
    pub fn open_initial(&mut self, last: Option<(String, String)>) {
        let pick = last
            .and_then(|(s, b)| {
                let tab = self.model.session(&s)?.tabs.iter().find(|t| t.blocks.contains(&b))?;
                Some((s, tab.id.clone(), b))
            })
            .or_else(|| {
                let (s, t) = self.model.all_tabs().into_iter().next()?;
                let b = Model::tab_block(self.model.tab(&s, &t)?)?;
                Some((s, t, b))
            });
        if let Some((s, t, b)) = pick {
            self.open(&s, &t, &b);
        }
    }

    pub fn open_tab(&mut self, session: &str, tab: &str) {
        if let Some(b) = self.model.tab(session, tab).and_then(Model::tab_block) {
            self.open(session, tab, &b);
        }
    }

    pub fn open(&mut self, session: &str, tab: &str, block: &str) {
        if self.view.as_ref().is_some_and(|v| v.block == block) {
            return;
        }
        self.leave();
        let (_, cols, rows) = self.pane_rect();
        self.generation += 1;
        match View::open(&self.rex, &self.socket, session, tab, block, (cols, rows), self.generation, self.tx.clone()) {
            Ok(v) => {
                crate::save_last(session, block);
                self.view = Some(v);
            }
            Err(e) => self.error(format!("Couldn't open that tab: {e}")),
        }
        self.mirror_modes();
    }

    /// Stop viewing, handing the block's size back to whoever wants it.
    pub fn leave(&mut self) {
        if let Some(v) = self.view.take() {
            self.rex.post_block(&v.session, &v.block, "resize", json!({"mode": "release"}));
        }
    }

    /// The outer terminal changed size (or the rail came or went).
    pub fn resized(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;
        let (_, pc, pr) = self.pane_rect();
        if let Some(v) = &mut self.view {
            if v.want != (pc, pr) {
                v.want = (pc, pr);
                self.rex.post_block(&v.session, &v.block, "resize", json!({"columns": pc, "rows": pr, "mode": "take"}));
            }
        }
        self.settle();
    }

    /// Make the outer terminal encode keys the way the block's program asked
    /// for: arrows, keypad, paste brackets, focus reports, kitty flags.
    pub fn mirror_modes(&mut self) {
        let want = self.view.as_ref().map(|v| *v.term.mode()).unwrap_or(TermMode::empty());
        let pairs = [
            (TermMode::APP_CURSOR, "\x1b[?1h", "\x1b[?1l"),
            (TermMode::APP_KEYPAD, "\x1b=", "\x1b>"),
            (TermMode::BRACKETED_PASTE, "\x1b[?2004h", "\x1b[?2004l"),
            (TermMode::FOCUS_IN_OUT, "\x1b[?1004h", "\x1b[?1004l"),
        ];
        for (mode, on, off) in pairs {
            if want.contains(mode) != self.mirrored.contains(mode) {
                self.out.extend_from_slice(if want.contains(mode) { on } else { off }.as_bytes());
            }
        }
        let kitty = |m: TermMode| (m & TermMode::KITTY_KEYBOARD_PROTOCOL).bits() >> TermMode::DISAMBIGUATE_ESC_CODES.bits().trailing_zeros();
        if kitty(want) != kitty(self.mirrored) {
            self.out.extend_from_slice(format!("\x1b[={};1u", kitty(want)).as_bytes());
        }
        self.mirrored = want & (TermMode::APP_CURSOR | TermMode::APP_KEYPAD | TermMode::BRACKETED_PASTE | TermMode::FOCUS_IN_OUT | TermMode::KITTY_KEYBOARD_PROTOCOL);
    }

    // ---- the server -----------------------------------------------------

    /// Connect again (starting Rex's server if rmx manages it) and pick up
    /// where we were.
    pub fn reconnect(&mut self) -> Result<(), String> {
        let generation = self.control_gen + 1;
        let rex = crate::connect(&self.socket, self.manage_server, generation, self.tx.clone())?;
        let model = Model::load(&rex)?;
        let was = self.view.take().map(|v| (v.session, v.block));
        (self.rex, self.model, self.control_gen, self.lost) = (rex, model, generation, None);
        if !matches!(self.mode, Mode::Switch { .. }) {
            self.mode = Mode::Normal;
        }
        if was.is_some() {
            self.open_initial(was);
        }
        self.settle();
        Ok(())
    }

    /// Restart Rex's server with the installed app's binary, then try
    /// `retry` again.
    fn restart_server(&mut self, retry: Action) {
        let Some(bin) = rex::find_rex() else {
            return self.error("Rex's server needs a restart, but rmx can't find the Rex app. Quit and reopen Rex on the Mac.");
        };
        self.leave();
        if let Err(e) = rex::stop_server(&bin).and_then(|_| rex::start_server(&bin)) {
            return self.error(format!("Couldn't restart Rex's server: {e}"));
        }
        match self.reconnect() {
            Ok(()) => {
                self.info("Restarted Rex's server.");
                self.run(retry);
            }
            Err(e) => self.error(format!("Restarted Rex's server but couldn't reconnect: {e}")),
        }
    }

    /// A tab or session could not be made.
    fn create_failed(&mut self, err: String) {
        // An old server whose own executable moved in a Rex update: it can
        // no longer spawn anything until it restarts.
        if !err.contains("posix_spawn") {
            return self.error(format!("Rex couldn't start that: {err}"));
        }
        if !self.manage_server {
            return self.error("Rex can't start new shells: its server is older than the Rex app (Rex updated). Restart Rex's server.");
        }
        match self.model.all_tabs().len() {
            0 => self.restart_server(Action::NewTab),
            tabs => self.mode = Mode::Confirm(Confirm::Restart { tabs }),
        }
    }

    // ---- rex events ---------------------------------------------------

    pub fn data(&mut self, generation: u64, bytes: &[u8]) {
        let me = self.rex.client_id.clone();
        if let Some(v) = self.view.as_mut().filter(|v| v.generation == generation) {
            v.feed(bytes, &me);
        }
    }

    pub fn data_gone(&mut self, generation: u64) {
        if let Some(v) = self.view.as_ref().filter(|v| v.generation == generation) {
            self.stale.insert(v.session.clone());
            self.view = None;
        }
    }

    pub fn event(&mut self, ev: &Value) {
        let p = &ev["payload"];
        match ev["type"].as_str().unwrap_or_default() {
            "session_created" => {
                self.added.insert(p["session"]["session_id"].as_str().unwrap_or_default().to_string());
            }
            "session_destroyed" => self.model.remove(p["session_id"].as_str().unwrap_or_default()),
            "session_event" => {
                let session = p["session_id"].as_str().unwrap_or_default();
                let inner = &p["event"];
                match inner["type"].as_str().unwrap_or_default() {
                    "block_event" => {
                        let b = &inner["payload"];
                        self.model.block_event(b["block_id"].as_str().unwrap_or_default(), b["name"].as_str().unwrap_or_default(), &b["payload"]);
                    }
                    "client_connected" | "client_disconnected" | "client_connection_changed" | "active_window_changed" => {}
                    _ => {
                        self.stale.insert(session.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    /// After a batch of messages: re-read what changed shape, and find
    /// something to show if the viewed block went away.
    pub fn settle(&mut self) {
        for s in std::mem::take(&mut self.added) {
            self.model.add(&self.rex, &s);
        }
        for s in std::mem::take(&mut self.stale) {
            self.model.refresh(&self.rex, &s);
        }
        if self.view.as_ref().is_some_and(|v| !self.model.blocks.contains_key(&v.block)) {
            self.view = None;
        }
        if self.view.is_none() {
            if self.wide() {
                self.open_initial(None);
            } else if matches!(self.mode, Mode::Normal | Mode::Scroll) {
                self.mode = Mode::Switch { sel: 0 };
            }
        }
        if let Mode::Switch { sel } = self.mode {
            self.mode = Mode::Switch { sel: self.nearest_selectable(sel) };
        }
        self.mirror_modes();
    }

    // ---- the list -----------------------------------------------------

    pub fn rows(&self) -> Vec<Row> {
        let phone = !self.wide();
        let mut rows = Vec::new();
        if phone {
            rows.extend([Row::Header, Row::Blank]);
        }
        if self.model.sessions.is_empty() {
            rows.extend([Row::Empty, Row::Blank]);
        }
        for s in &self.model.sessions {
            rows.push(Row::Session(s.id.clone()));
            for t in &s.tabs {
                rows.push(Row::Tab(s.id.clone(), t.id.clone()));
                rows.push(Row::Detail(s.id.clone(), t.id.clone()));
                if t.blocks.len() > 1 {
                    for b in &t.blocks {
                        rows.push(Row::Block(s.id.clone(), t.id.clone(), b.clone()));
                    }
                }
                if phone {
                    rows.push(Row::Gap(s.id.clone(), t.id.clone()));
                }
            }
            rows.push(Row::AddTab(s.id.clone()));
            rows.push(Row::Blank);
        }
        rows.push(Row::AddSession);
        rows
    }

    /// The row of what is on screen now.
    pub fn current_row(&self) -> usize {
        let rows = self.rows();
        let Some(v) = &self.view else { return rows.iter().position(Row::selectable).unwrap_or(0) };
        rows.iter()
            .position(|r| matches!(r, Row::Block(_, _, b) if *b == v.block))
            .or_else(|| rows.iter().position(|r| matches!(r, Row::Tab(_, t) if *t == v.tab)))
            .unwrap_or(0)
    }

    fn nearest_selectable(&self, sel: usize) -> usize {
        let rows = self.rows();
        let sel = sel.min(rows.len().saturating_sub(1));
        (sel..rows.len()).chain((0..sel).rev()).find(|&i| rows[i].selectable()).unwrap_or(0)
    }

    fn step(&self, sel: usize, delta: i32) -> usize {
        let rows = self.rows();
        let mut i = sel as i32;
        loop {
            i += delta;
            if i < 0 || i >= rows.len() as i32 {
                return sel;
            }
            if rows[i as usize].selectable() {
                return i as usize;
            }
        }
    }

    /// Keep the selection (or what is on screen) inside a list of `height`.
    pub fn fit_list(&mut self, height: usize) {
        let rows = self.rows().len();
        let focus = match self.mode {
            Mode::Switch { sel } => sel,
            _ => self.current_row(),
        };
        let max = rows.saturating_sub(height);
        if focus < self.list_offset {
            self.list_offset = focus.saturating_sub(1);
        } else if focus + 2 > self.list_offset + height {
            self.list_offset = (focus + 3).saturating_sub(height);
        }
        self.list_offset = self.list_offset.min(max);
    }

    pub fn activate(&mut self, row: Row) {
        let sel = self.rows().iter().position(|r| *r == row).unwrap_or(0);
        self.mode = Mode::Normal;
        match row {
            Row::Tab(s, t) | Row::Detail(s, t) | Row::Gap(s, t) => self.open_tab(&s, &t),
            Row::Block(s, t, b) => self.open(&s, &t, &b),
            Row::AddTab(s) => self.new_tab(&s),
            Row::AddSession => self.new_session(),
            Row::Header if self.waiting().0 > 0 => self.attention(),
            Row::Header | Row::Session(_) | Row::Empty | Row::Blank => self.mode = Mode::Switch { sel },
        }
        // On a phone, a failed open lands back on the list.
        if self.view.is_none() && !self.wide() && matches!(self.mode, Mode::Normal) {
            self.mode = Mode::Switch { sel: self.nearest_selectable(sel) };
        }
    }

    /// Leave the list for the tab on screen, if there is one.
    fn back(&mut self) {
        if self.view.is_some() || self.wide() {
            self.mode = Mode::Normal;
        }
    }

    // ---- actions ------------------------------------------------------

    fn here(&self) -> Option<(String, String)> {
        self.view.as_ref().map(|v| (v.session.clone(), v.tab.clone()))
    }

    fn cwd(&self) -> Option<String> {
        let v = self.view.as_ref()?;
        self.model.blocks.get(&v.block).map(|b| b.cwd.clone()).filter(|c| !c.is_empty())
    }

    fn layout(&self) -> Value {
        match self.cwd() {
            Some(cwd) => json!({"block": {"flavor": SHELL, "options": {"cwd": cwd}}}),
            None => json!({"block": {"flavor": SHELL}}),
        }
    }

    /// `focus: false` leaves the Rex app's own tab where it was.
    pub fn new_tab(&mut self, session: &str) {
        let made = self.rex.call("session.new_window", json!({"session_id": session, "focus": false, "layout": self.layout()}));
        match made {
            Ok(r) => {
                self.model.refresh(&self.rex, session);
                let (tab, block) = (r["window_id"].as_str().unwrap_or_default().to_string(), r["block_ids"][0].as_str().unwrap_or_default().to_string());
                self.open(session, &tab, &block);
            }
            Err(e) => self.create_failed(e),
        }
    }

    pub fn new_session(&mut self) {
        match self.rex.call("session.create", json!({"initial_windows": [{"layout": self.layout()}]})) {
            Ok(r) => {
                let session = r["session_id"].as_str().unwrap_or_default().to_string();
                self.model.add(&self.rex, &session);
                let w = &r["initial_windows"][0];
                let (tab, block) = (w["window_id"].as_str().unwrap_or_default().to_string(), w["block_ids"][0].as_str().unwrap_or_default().to_string());
                self.open(&session, &tab, &block);
            }
            Err(e) => self.create_failed(e),
        }
    }

    fn cycle(&mut self, delta: i32) {
        let tabs = self.model.all_tabs();
        if tabs.is_empty() {
            return;
        }
        let here = self.here().and_then(|h| tabs.iter().position(|t| *t == h)).unwrap_or(0) as i32;
        let (s, t) = tabs[(here + delta).rem_euclid(tabs.len() as i32) as usize].clone();
        self.open_tab(&s, &t);
    }

    /// Next tab (after this one, wrapping) that is blocked, done or failed.
    fn attention(&mut self) {
        let tabs = self.model.all_tabs();
        let here = self.here().and_then(|h| tabs.iter().position(|t| *t == h)).unwrap_or(0);
        let wants = |(s, t): &(String, String)| {
            self.model.tab(s, t).and_then(|tab| self.model.tab_status(tab)).is_some_and(|st| st.state >= State::Done)
        };
        let next = (1..=tabs.len()).map(|i| &tabs[(here + i) % tabs.len()]).find(|t| wants(t)).cloned();
        match next {
            Some((s, t)) => self.open_tab(&s, &t),
            None => self.info("Nothing is waiting on you."),
        }
    }

    /// Tabs other than this one waiting on the user: (count, any blocked).
    pub fn waiting(&self) -> (usize, bool) {
        self.waiting_except(self.here())
    }

    /// Every tab waiting on the user, this one included.
    pub fn waiting_all(&self) -> (usize, bool) {
        self.waiting_except(None)
    }

    fn waiting_except(&self, here: Option<(String, String)>) -> (usize, bool) {
        let mut count = 0;
        let mut blocked = false;
        for (s, t) in self.model.all_tabs() {
            if here.as_ref() == Some(&(s.clone(), t.clone())) {
                continue;
            }
            if let Some(st) = self.model.tab(&s, &t).and_then(|tab| self.model.tab_status(tab)) {
                if st.state >= State::Done {
                    count += 1;
                    blocked |= st.state == State::Blocked;
                }
            }
        }
        (count, blocked)
    }

    pub fn run(&mut self, action: Action) {
        self.mode = Mode::Normal;
        match action {
            Action::Switcher => self.mode = Mode::Switch { sel: self.nearest_selectable(self.current_row()) },
            Action::NewTab => match self.here().map(|h| h.0).or_else(|| self.model.sessions.first().map(|s| s.id.clone())) {
                Some(s) => self.new_tab(&s),
                None => self.new_session(),
            },
            Action::NewSession => self.new_session(),
            Action::NextTab => self.cycle(1),
            Action::PrevTab => self.cycle(-1),
            Action::TabN(n) => {
                let tab = self.here().and_then(|(s, _)| Some((s.clone(), self.model.session(&s)?.tabs.get(n)?.id.clone())));
                if let Some((s, t)) = tab {
                    self.open_tab(&s, &t);
                }
            }
            Action::Attention => self.attention(),
            Action::CloseTab => {
                if let Some((s, t)) = self.here() {
                    let name = self.model.tab(&s, &t).map(|tab| self.model.tab_name(tab)).unwrap_or_default();
                    self.mode = Mode::Confirm(Confirm::Close { session: s, tab: t, name });
                }
            }
            Action::RenameTab => {
                if let Some((s, t)) = self.here() {
                    self.mode = Mode::Prompt { kind: Prompt::RenameTab(s, t), buf: String::new() };
                }
            }
            Action::RenameSession => {
                if let Some((s, _)) = self.here() {
                    self.mode = Mode::Prompt { kind: Prompt::RenameSession(s), buf: String::new() };
                }
            }
            Action::Scroll => {
                if let Some(v) = &mut self.view {
                    let page = v.want.1 as i32 / 2;
                    v.term.scroll_display(Scroll::Delta(page));
                    self.mode = Mode::Scroll;
                }
            }
            Action::ToggleRail => {
                self.rail = !self.rail;
                self.resized(self.cols, self.rows);
            }
            Action::Refresh => {
                if let Some(v) = self.view.take() {
                    self.open(&v.session, &v.tab, &v.block);
                }
            }
            Action::Quit => self.quit = true,
            Action::Literal => self.send(b"\x02"),
        }
        self.settle();
    }

    fn close_tab(&mut self, session: &str, tab: &str) {
        if let Err(e) = self.rex.call("session.close_window", json!({"session_id": session, "window_id": tab})) {
            self.error(format!("Couldn't close it: {e}"));
        }
        self.stale.insert(session.to_string());
        self.settle();
    }

    fn apply_prompt(&mut self, kind: Prompt, label: String) {
        let result = match &kind {
            Prompt::RenameTab(s, t) => self.rex.call("session.set_window_label", json!({"session_id": s, "window_id": t, "label": label})),
            Prompt::RenameSession(s) => self.rex.call("session.set_label", json!({"session_id": s, "label": label})),
        };
        if let Err(e) = result {
            self.error(format!("Couldn't rename it: {e}"));
        }
        let (Prompt::RenameTab(s, _) | Prompt::RenameSession(s)) = kind;
        self.stale.insert(s);
    }

    /// Yes / no on whatever is being confirmed or typed.
    fn answer(&mut self, yes: bool) {
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Confirm(Confirm::Close { session, tab, .. }) if yes => self.close_tab(&session, &tab),
            Mode::Confirm(Confirm::Restart { .. }) if yes => self.restart_server(Action::NewTab),
            Mode::Prompt { kind, buf } if yes => self.apply_prompt(kind, buf.trim().to_string()),
            _ => {}
        }
        self.settle();
    }

    // ---- input --------------------------------------------------------

    /// Type into the block.
    fn send(&mut self, bytes: &[u8]) {
        let Some(v) = &mut self.view else { return };
        if v.term.grid().display_offset() > 0 {
            v.term.scroll_display(Scroll::Bottom);
        }
        // Typing here means using it here: take the size back from the Mac.
        if v.sized_by_other {
            v.sized_by_other = false;
            self.rex.post_block(&v.session, &v.block, "resize", json!({"columns": v.want.0, "rows": v.want.1, "mode": "take"}));
        }
        let text = String::from_utf8_lossy(bytes);
        for chunk in text.as_bytes().chunks(256 * 1024) {
            let chunk = String::from_utf8_lossy(chunk);
            self.rex.post_block(&v.session, &v.block, "write", json!({"data": chunk}));
        }
    }

    pub fn input(&mut self, bytes: &[u8]) {
        let mut forward: Vec<u8> = Vec::new();
        for token in input::tokenize(bytes) {
            if let Token::Osc(raw) = token {
                self.terminal_reply(raw);
                continue;
            }
            if let Token::Mouse(m, _) = token {
                self.flush(&mut forward);
                if m.is_left_click() {
                    self.note = None;
                }
                self.mouse(m);
                continue;
            }
            self.note = None;
            if self.lost.is_some() {
                if let Token::Key(k, _) = token {
                    if k.ctrl('c') || k.ctrl('d') || k.key == Key::Char('q') {
                        self.quit = true;
                    }
                }
                continue;
            }
            match (&mut self.mode, &token) {
                (Mode::Normal, Token::Key(k, _)) if k.ctrl('b') => {
                    self.flush(&mut forward);
                    self.mode = Mode::Prefix;
                }
                (Mode::Normal, t) => forward.extend_from_slice(t.bytes()),
                (_, Token::Key(k, raw)) => {
                    let (k, raw) = (*k, raw.to_vec());
                    self.flush(&mut forward);
                    self.key(k, &raw);
                }
                _ => {}
            }
        }
        self.flush(&mut forward);
    }

    fn flush(&mut self, forward: &mut Vec<u8>) {
        if !forward.is_empty() {
            self.send(&std::mem::take(forward));
        }
    }

    /// A key while an rmx mode is up.
    fn key(&mut self, k: KeyPress, raw: &[u8]) {
        match &mut self.mode {
            Mode::Normal => {}
            Mode::Prefix => match prefixed(&k) {
                Some(a) => self.run(a),
                None => self.back_or_list(),
            },
            Mode::Switch { sel } => {
                let sel = *sel;
                let row = self.rows().get(sel).cloned();
                match k.key {
                    Key::Up | Key::Char('k') => self.mode = Mode::Switch { sel: self.step(sel, -1) },
                    Key::Down | Key::Char('j') | Key::Tab => self.mode = Mode::Switch { sel: self.step(sel, 1) },
                    Key::Enter | Key::Char(' ') => {
                        if let Some(row) = row {
                            self.activate(row);
                        }
                    }
                    Key::Char('c' | 't') => {
                        let session = self.rows().iter().take(sel + 1).rev().find_map(|r| match r {
                            Row::Session(s) => Some(s.clone()),
                            _ => None,
                        });
                        self.mode = Mode::Normal;
                        match session {
                            Some(s) => self.new_tab(&s),
                            None => self.new_session(),
                        }
                        self.settle();
                    }
                    Key::Char('S' | 'N' | 'C') => {
                        self.mode = Mode::Normal;
                        self.new_session();
                        self.settle();
                    }
                    Key::Char('x') => {
                        if let Some(Row::Tab(s, t)) = row {
                            let name = self.model.tab(&s, &t).map(|tab| self.model.tab_name(tab)).unwrap_or_default();
                            self.mode = Mode::Confirm(Confirm::Close { session: s, tab: t, name });
                        }
                    }
                    Key::Char('r') => {
                        if let Some(Row::Tab(s, t)) = row {
                            self.mode = Mode::Prompt { kind: Prompt::RenameTab(s, t), buf: String::new() };
                        }
                    }
                    Key::Esc | Key::Char('q') => self.back(),
                    Key::Char('b') if k.mods.ctrl => self.mode = Mode::Prefix,
                    _ => {}
                }
            }
            Mode::Scroll => {
                let Some(v) = &mut self.view else {
                    self.mode = Mode::Normal;
                    return;
                };
                let page = v.want.1 as i32 - 1;
                match k.key {
                    Key::Up | Key::Char('k') => v.term.scroll_display(Scroll::Delta(1)),
                    Key::Down | Key::Char('j') => v.term.scroll_display(Scroll::Delta(-1)),
                    Key::PageUp | Key::Char('u' | 'b') => v.term.scroll_display(Scroll::Delta(page)),
                    Key::PageDown | Key::Char('d' | 'f' | ' ') => v.term.scroll_display(Scroll::Delta(-page)),
                    Key::Home | Key::Char('g') => v.term.scroll_display(Scroll::Top),
                    Key::End | Key::Char('G') => v.term.scroll_display(Scroll::Bottom),
                    Key::Esc | Key::Enter | Key::Char('q') => v.term.scroll_display(Scroll::Bottom),
                    _ => {}
                }
                if v.term.grid().display_offset() == 0 {
                    self.mode = Mode::Normal;
                }
            }
            Mode::Prompt { buf, .. } => match k.key {
                Key::Enter => self.answer(true),
                Key::Esc => self.answer(false),
                Key::Backspace => {
                    buf.pop();
                }
                Key::Char(c) if !k.mods.ctrl && !k.mods.alt => buf.push(c),
                _ if raw.iter().all(|b| *b >= 0x20) => buf.push_str(&String::from_utf8_lossy(raw)),
                _ => {}
            },
            Mode::Confirm(_) => self.answer(matches!(k.key, Key::Char('y' | 'Y') | Key::Enter)),
        }
    }

    /// Close the menu: back to the tab, or the list when there is none.
    fn back_or_list(&mut self) {
        self.mode = Mode::Normal;
        self.settle();
    }

    /// The answer to rmx's own OSC 11 background query, if this is it.
    fn terminal_reply(&mut self, raw: &[u8]) {
        let text = String::from_utf8_lossy(raw);
        let Some(spec) = text.strip_prefix("\x1b]11;rgb:") else { return };
        let spec = spec.trim_end_matches(['\x07', '\\']).trim_end_matches('\x1b');
        let parts: Vec<u8> = spec.split('/').filter_map(|p| u16::from_str_radix(p.get(..2)?, 16).ok().map(|v| v as u8)).collect();
        if let [r, g, b] = parts[..] {
            self.bg = Some(Rgb { r, g, b });
        }
    }

    pub fn tap(&mut self, tap: Tap) {
        match tap {
            Tap::Run(a) => self.run(a),
            Tap::Menu => self.mode = if matches!(self.mode, Mode::Prefix) { Mode::Normal } else { Mode::Prefix },
            Tap::Back => self.back(),
            Tap::Bottom => {
                if let Some(v) = &mut self.view {
                    v.term.scroll_display(Scroll::Bottom);
                }
                self.mode = Mode::Normal;
            }
            Tap::Up | Tap::Down => {
                if let Some(v) = &mut self.view {
                    let page = (v.want.1 as i32 * 2 / 3).max(1);
                    v.term.scroll_display(Scroll::Delta(if tap == Tap::Up { page } else { -page }));
                    if v.term.grid().display_offset() == 0 {
                        self.mode = Mode::Normal;
                    }
                }
            }
            Tap::Yes => self.answer(true),
            Tap::No => self.answer(false),
            Tap::Quit => self.quit = true,
        }
        if matches!(self.mode, Mode::Normal) {
            self.settle();
        }
    }

    fn mouse(&mut self, m: Mouse) {
        let (x, y) = (m.x.saturating_sub(1) as usize, m.y.saturating_sub(1) as usize);
        let footer = self.rows.saturating_sub(self.bar_h());
        let (px, pcols, prows) = self.pane_rect();
        if y >= footer {
            if m.is_left_click() {
                if let Some(t) = crate::ui::footer_hit(self, x) {
                    self.tap(t);
                }
            }
            return;
        }
        if self.lost.is_some() {
            return;
        }
        if let Mode::Prefix = self.mode {
            if m.is_left_click() {
                match crate::ui::menu_hit(self, x, y) {
                    Some(a) => self.run(a),
                    None => self.back_or_list(),
                }
            }
            return;
        }
        if let Mode::Confirm(_) | Mode::Prompt { .. } = self.mode {
            return;
        }
        let in_list = if self.wide() { x < RAIL_W } else { matches!(self.mode, Mode::Switch { .. }) };
        if in_list {
            if let Some(d) = m.wheel() {
                let max = self.rows().len().saturating_sub(footer);
                self.list_offset = (self.list_offset as i32 + d * 2).clamp(0, max as i32) as usize;
            } else if m.is_left_click() {
                if let Some(row) = self.rows().get(y + self.list_offset).cloned() {
                    self.activate(row);
                }
            }
            return;
        }
        if let Mode::Switch { .. } = self.mode {
            if m.is_left_click() {
                self.mode = Mode::Normal;
            }
        }
        let Some(v) = &mut self.view else { return };
        if x < px || x >= px + pcols || y >= prows {
            return;
        }
        let modes = *v.term.mode();
        let shift = v.term.screen_lines().saturating_sub(prows);
        let scrolling = matches!(self.mode, Mode::Scroll);
        if modes.intersects(TermMode::MOUSE_MODE) && !scrolling {
            let wants_motion = modes.contains(TermMode::MOUSE_MOTION) || (modes.contains(TermMode::MOUSE_DRAG) && m.cb & 3 != 3);
            if m.is_motion() && !wants_motion {
                return;
            }
            let bytes = input::encode_mouse(m, (x - px + 1) as u16, (y + shift + 1) as u16, modes.contains(TermMode::SGR_MOUSE));
            self.send(&bytes);
            return;
        }
        let Some(d) = m.wheel() else { return };
        if modes.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let arrow: &[u8] = match (d < 0, modes.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            self.send(&arrow.repeat(3));
            return;
        }
        v.term.scroll_display(Scroll::Delta(-d * 3));
        self.mode = if v.term.grid().display_offset() > 0 { Mode::Scroll } else { Mode::Normal };
    }
}
