//! What the Rex server holds, as rmx shows it: sessions, their tabs (Rex
//! windows) and each tab's blocks, plus what every block reports about itself.
use std::collections::{BTreeMap, HashMap};

use serde_json::{json, Value};

use crate::rex::Control;

pub struct Session {
    pub id: String,
    pub label: String,
    pub tabs: Vec<Tab>,
}

pub struct Tab {
    pub id: String,
    /// None while Rex's generated "Window xxxxxxxx" label is in place.
    pub label: Option<String>,
    /// Layout order.
    pub blocks: Vec<String>,
    pub focused: Option<String>,
}

#[derive(Default)]
pub struct Block {
    pub title: String,
    pub process: String,
    pub cwd: String,
    /// OSC 7501 records by id ("" is the root record).
    pub status: BTreeMap<String, Status>,
}

#[derive(Clone)]
pub struct Status {
    pub state: State,
    pub app: String,
    pub msg: String,
    /// What the program calls this: ck reports its session name.
    pub title: String,
}

/// Ordered by how much it wants the user.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum State {
    Idle,
    Working,
    Done,
    Error,
    Blocked,
}

impl Status {
    fn parse(v: &Value) -> Option<Status> {
        let state = match v["state"].as_str()? {
            "idle" => State::Idle,
            "working" => State::Working,
            "done" => State::Done,
            "error" => State::Error,
            "blocked" => State::Blocked,
            _ => return None,
        };
        let app = v["effective_app"].as_str().or(v["app"].as_str()).unwrap_or_default().to_string();
        let text = |k: &str| v[k].as_str().unwrap_or_default().to_string();
        Some(Status { state, app, msg: text("msg"), title: text("title") })
    }
}

#[derive(Default)]
pub struct Model {
    pub sessions: Vec<Session>,
    pub blocks: HashMap<String, Block>,
}

impl Model {
    /// Attach every session and read all of it.
    pub fn load(rex: &Control) -> Result<Model, String> {
        let all = rex.call("session.attach_all", json!({"event_scope": "all"}))?;
        let ids: Vec<&str> = all["sessions"].as_array().into_iter().flatten().filter_map(|s| s["session_id"].as_str()).collect();
        let boot = rex.call("session.bootstrap", json!({"event_scope": "all", "attach_session_ids": ids}))?;
        let mut model = Model::default();
        for s in boot["attached_sessions"].as_array().into_iter().flatten() {
            model.put_view(rex, &s["view"]);
        }
        Ok(model)
    }

    /// Re-read one session after it changed shape; a gone session drops out.
    pub fn refresh(&mut self, rex: &Control, session: &str) {
        match rex.call("session.view", json!({"session_id": session})) {
            Ok(view) => self.put_view(rex, &view),
            Err(_) => self.remove(session),
        }
    }

    pub fn add(&mut self, rex: &Control, session: &str) {
        if rex.call("session.attach", json!({"session_id": session})).is_ok() {
            self.refresh(rex, session);
        }
    }

    pub fn remove(&mut self, session: &str) {
        self.sessions.retain(|s| s.id != session);
        self.prune();
    }

    fn put_view(&mut self, rex: &Control, view: &Value) {
        let Some(id) = view["session_id"].as_str() else { return };
        let tabs: Vec<Tab> = view["windows"].as_array().into_iter().flatten().map(parse_tab).collect();
        for b in tabs.iter().flat_map(|t| &t.blocks) {
            if !self.blocks.contains_key(b) {
                let info = read_block(rex, id, b);
                self.blocks.insert(b.clone(), info);
            }
        }
        let session = Session { id: id.to_string(), label: view["label"].as_str().unwrap_or_default().to_string(), tabs };
        match self.sessions.iter_mut().find(|s| s.id == id) {
            Some(s) => *s = session,
            None => self.sessions.push(session),
        }
        self.prune();
    }

    fn prune(&mut self) {
        let live: std::collections::HashSet<&String> = self.sessions.iter().flat_map(|s| &s.tabs).flat_map(|t| &t.blocks).collect();
        self.blocks.retain(|b, _| live.contains(b));
    }

    /// Fold a block event in. Returns true when something visible changed.
    pub fn block_event(&mut self, block: &str, name: &str, p: &Value) -> bool {
        let Some(b) = self.blocks.get_mut(block) else { return false };
        match name {
            "title_changed" => b.title = p["title"].as_str().unwrap_or_default().to_string(),
            "process_changed" => set_process(b, &p["foreground"]),
            "program_status_changed" => {
                let id = p["id"].as_str().unwrap_or_default().to_string();
                match Status::parse(&p["record"]) {
                    Some(s) => b.status.insert(id, s),
                    None => b.status.remove(&id),
                };
            }
            "program_status_removed" => {
                for id in p["ids"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    b.status.remove(id);
                }
            }
            _ => return false,
        }
        true
    }

    pub fn session(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn tab(&self, session: &str, tab: &str) -> Option<&Tab> {
        self.session(session)?.tabs.iter().find(|t| t.id == tab)
    }

    /// Every (session, tab) in rail order.
    pub fn all_tabs(&self) -> Vec<(String, String)> {
        self.sessions.iter().flat_map(|s| s.tabs.iter().map(|t| (s.id.clone(), t.id.clone()))).collect()
    }

    /// The block a tab shows: Rex's focus inside it, else its first.
    pub fn tab_block(tab: &Tab) -> Option<String> {
        tab.focused.clone().filter(|f| tab.blocks.contains(f)).or_else(|| tab.blocks.first().cloned())
    }

    pub fn tab_name(&self, tab: &Tab) -> String {
        if let Some(label) = &tab.label {
            return label.clone();
        }
        let b = Model::tab_block(tab).and_then(|b| self.blocks.get(&b));
        match b {
            // A shell's title is "user@host:dir" noise; its folder says more.
            Some(b) if SHELLS.contains(&b.process.as_str()) && !b.cwd.is_empty() => folder(&b.cwd),
            Some(b) if !b.title.trim().is_empty() => b.title.trim().to_string(),
            Some(b) if !b.process.is_empty() => b.process.clone(),
            _ => "shell".into(),
        }
    }

    /// The status that most wants the user, across the tab's blocks.
    pub fn tab_status(&self, tab: &Tab) -> Option<&Status> {
        tab.blocks.iter().filter_map(|b| self.blocks.get(b)).flat_map(|b| b.status.values()).max_by_key(|s| s.state)
    }
}

const SHELLS: [&str; 9] = ["zsh", "bash", "fish", "sh", "dash", "ksh", "tcsh", "nu", "xonsh"];

fn folder(cwd: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if cwd == home {
        return "~".into();
    }
    cwd.rsplit('/').find(|p| !p.is_empty()).unwrap_or("/").to_string()
}

fn parse_tab(w: &Value) -> Tab {
    let id = w["window_id"].as_str().unwrap_or_default().to_string();
    let label = w["label"].as_str().unwrap_or_default().to_string();
    let generated = format!("Window {}", &id[id.len().saturating_sub(8)..]);
    let mut blocks = Vec::new();
    for layer in w["layers"].as_array().into_iter().flatten() {
        collect_blocks(&layer["layout"], &mut blocks);
    }
    Tab {
        label: (!label.is_empty() && label != generated).then_some(label),
        focused: w["focused_block_id"].as_str().map(str::to_string),
        id,
        blocks,
    }
}

/// A layout node is a block or a split of two nodes.
fn collect_blocks(node: &Value, out: &mut Vec<String>) {
    if let Some(b) = node["block_id"].as_str() {
        out.push(b.to_string());
    } else if node["split"].is_object() {
        collect_blocks(&node["split"]["before"], out);
        collect_blocks(&node["split"]["after"], out);
    }
}

fn read_block(rex: &Control, session: &str, block: &str) -> Block {
    let mut b = Block::default();
    if let Ok(t) = rex.block(session, block, "title", json!({})) {
        b.title = t["title"].as_str().unwrap_or_default().to_string();
    }
    if let Ok(p) = rex.block(session, block, "process", json!({})) {
        set_process(&mut b, &p["foreground"]);
    }
    if let Ok(s) = rex.block(session, block, "program_status", json!({})) {
        for r in s["records"].as_array().into_iter().flatten() {
            if let Some(status) = Status::parse(r) {
                b.status.insert(r["id"].as_str().unwrap_or_default().to_string(), status);
            }
        }
    }
    b
}

fn set_process(b: &mut Block, fg: &Value) {
    b.process = fg["name"].as_str().unwrap_or_default().to_string();
    b.cwd = fg["cwd"].as_str().unwrap_or_default().to_string();
}
