//! Rex forgets every session when its server restarts (an app update, a
//! crash). `rmx --snapshot` writes down what is open and `rmx --restore`
//! rebuilds it, typing `ck --resume <name>` into each tab that was running ck.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use serde_json::{json, Value};

use crate::app::SHELL;
use crate::model::{Block, Model};
use crate::rex::Control;

fn path() -> PathBuf {
    crate::home().join(".local/state/rmx/snapshot.json")
}

fn read() -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path()).ok()?).ok()
}

/// The ck session a block runs, by the name ck reports over OSC 7501.
fn ck_session(b: &Block) -> Option<&str> {
    if b.process != "ck" {
        return None;
    }
    b.status.values().find(|s| s.app == "ck" && !s.title.is_empty()).map(|s| s.title.as_str())
}

fn count(snap: &Value) -> usize {
    snap["sessions"].as_array().into_iter().flatten().map(|s| s["tabs"].as_array().map_or(0, Vec::len)).sum()
}

pub enum Job {
    Snapshot,
    Restore,
    /// Snapshot every 30 s, for launchd to keep alive.
    Watch,
}

/// Only a restore may start Rex's server; snapshots run unattended.
pub fn run(job: Job, socket: &Path, manage_server: bool) -> Result<String, String> {
    let restore = matches!(job, Job::Restore);
    if matches!(job, Job::Watch) {
        loop {
            let _ = once(false, socket, false);
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
    }
    once(restore, socket, manage_server && restore)
}

fn once(restore: bool, socket: &Path, start_server: bool) -> Result<String, String> {
    let (tx, _events) = mpsc::channel();
    let rex = crate::connect(socket, start_server, 0, tx)?;
    let done = work(&rex, restore);
    rex.close();
    done
}

fn work(rex: &Control, restore: bool) -> Result<String, String> {
    let mut model = Model::default();
    let list = rex.call("session.list", json!({}))?;
    for s in list["sessions"].as_array().into_iter().flatten().filter_map(|s| s["session_id"].as_str()) {
        model.add(rex, s);
    }
    if restore {
        restore_all(rex, &model)
    } else {
        take(rex, &model)
    }
}

fn take(rex: &Control, model: &Model) -> Result<String, String> {
    // One entry per block: a split tab comes back as separate tabs.
    let sessions: Vec<Value> = model
        .sessions
        .iter()
        .filter_map(|s| {
            let tabs: Vec<Value> = s
                .tabs
                .iter()
                .flat_map(|t| {
                    let blocks = t.blocks.iter().filter_map(|b| model.blocks.get(b));
                    blocks.map(move |b| json!({"label": t.label, "cwd": b.cwd, "ck": ck_session(b)}))
                })
                .collect();
            (!tabs.is_empty()).then(|| json!({"label": s.label, "tabs": tabs}))
        })
        .collect();
    let instance = rex.call("server.status", json!({}))?["instance_id"].clone();
    let taken = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let snap = json!({"taken": taken, "instance": instance, "sessions": sessions});
    // A restarted server comes back nearly empty. Until the new one is as
    // big as the old picture, the old picture is what `--restore` needs.
    if let Some(old) = read().filter(|o| o["instance"] != snap["instance"]) {
        if count(&snap) < count(&old) {
            return Ok(format!("kept the snapshot from before the restart ({} tabs)", count(&old)));
        }
    }
    let file = path();
    std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
    let tmp = file.with_extension("tmp");
    std::fs::write(&tmp, snap.to_string()).and_then(|_| std::fs::rename(&tmp, &file)).map_err(|e| e.to_string())?;
    Ok(format!("snapshot: {} windows, {} tabs", snap["sessions"].as_array().map_or(0, Vec::len), count(&snap)))
}

fn restore_all(rex: &Control, model: &Model) -> Result<String, String> {
    let snap = read().ok_or("no snapshot yet; `rmx --snapshot` takes one")?;
    // Re-running is safe: a window that is already open, or a ck session
    // that is already running, is left alone.
    let open: HashSet<&str> = model.sessions.iter().map(|s| s.label.as_str()).collect();
    let running: HashSet<&str> = model.blocks.values().filter_map(ck_session).collect();
    let (mut windows, mut tabs) = (0, 0);
    for s in snap["sessions"].as_array().into_iter().flatten() {
        let label = s["label"].as_str().unwrap_or_default();
        if open.contains(label) {
            continue;
        }
        let wanted = s["tabs"].as_array().into_iter().flatten().filter(|t| t["ck"].as_str().is_none_or(|n| !running.contains(n)));
        let mut session: Option<String> = None;
        for t in wanted {
            let mut block = json!({"flavor": SHELL});
            if let Some(cwd) = t["cwd"].as_str().filter(|c| !c.is_empty()) {
                block["options"] = json!({"cwd": cwd});
            }
            let mut window = json!({"layout": {"block": block}});
            if let Some(l) = t["label"].as_str() {
                window["window_label"] = json!(l);
            }
            let (sid, bid) = match &session {
                None => {
                    let r = rex.call("session.create", json!({"label": label, "initial_windows": [window]}))?;
                    (r["session_id"].clone(), r["initial_windows"][0]["block_ids"][0].clone())
                }
                Some(id) => {
                    window["session_id"] = json!(id);
                    window["focus"] = json!(false);
                    let r = rex.call("session.new_window", window)?;
                    (json!(id), r["block_ids"][0].clone())
                }
            };
            let sid = sid.as_str().ok_or("rex made no session")?.to_string();
            let bid = bid.as_str().ok_or("rex made no block")?;
            if session.is_none() {
                rex.call("session.attach", json!({"session_id": sid}))?;
            }
            if let Some(name) = t["ck"].as_str() {
                let cmd = format!("ck --resume '{}'\n", name.replace('\'', r"'\''"));
                rex.block(&sid, bid, "write", json!({"data": cmd}))?;
            }
            windows += session.is_none() as usize;
            tabs += 1;
            session = Some(sid);
        }
    }
    Ok(format!("restored {windows} windows, {tabs} tabs"))
}
