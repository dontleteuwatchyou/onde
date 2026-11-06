//! Optional, explicitly started Telegram bridge. JSON events contain no login secrets.
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

#[derive(Clone, serde::Deserialize)]
pub struct MenuButton {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub details: String,
}

#[derive(Default, Clone)]
pub struct State {
    pub connected: bool,
    pub busy: bool,
    pub status: String,
    pub qr: Vec<Vec<bool>>,
    pub files: Vec<String>,
    pub candidates: Vec<i64>,
    pub buttons: Vec<MenuButton>,
}

pub struct Telegram {
    child: Option<Child>,
    input: ChildStdin,
    pub state: Arc<Mutex<State>>,
}

impl Telegram {
    pub fn start() -> Result<Self> {
        let root = dirs::config_dir()
            .context("Dossier de configuration absent")?
            .join("onde/telegram");
        let python = std::env::var_os("ONDE_TELEGRAM_PYTHON").unwrap_or_else(|| "python3".into());
        let mut child = Command::new(python)
            .args([
                "-u",
                "-c",
                include_str!("../integrations/telegram/bridge.py"),
            ])
            .arg(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("Démarrage Python Telegram impossible")?;
        let input = child.stdin.take().context("Entrée Telegram absente")?;
        let output = child.stdout.take().context("Sortie Telegram absente")?;
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                let Ok(event) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let mut state = shared.lock().unwrap();
                if let Some(value) = event["connected"].as_bool() {
                    state.connected = value;
                }
                if let Some(value) = event["busy"].as_bool() {
                    state.busy = value;
                }
                if let Some(value) = event["status"].as_str() {
                    state.status = value.to_owned();
                }
                if let Some(value) = event.get("buttons") {
                    state.buttons = serde_json::from_value(value.clone()).unwrap_or_default();
                }
                if let Some(value) = event.get("qr") {
                    state.qr = serde_json::from_value(value.clone()).unwrap_or_default();
                }
                if event["clear_candidates"].as_bool() == Some(true) {
                    state.candidates.clear();
                }
                if let Some(value) = event["candidate"].as_i64() {
                    if !state.candidates.contains(&value) {
                        state.candidates.push(value);
                    }
                }
                if let Some(value) = event["file"].as_str() {
                    state.files.push(value.to_owned());
                }
            }
            let mut state = shared.lock().unwrap();
            state.connected = false;
            state.busy = false;
            state.qr.clear();
            state.buttons.clear();
            state.status.push_str(" · Service arrêté.");
        });
        Ok(Self {
            child: Some(child),
            input,
            state,
        })
    }

    pub fn send(&mut self, command: Value) -> Result<()> {
        serde_json::to_writer(&mut self.input, &command)?;
        self.input.write_all(b"\n")?;
        self.input.flush()?;
        Ok(())
    }
}

impl Drop for Telegram {
    fn drop(&mut self) {
        let _ = self.send(json!({"action": "quit"}));
        // Reap without blocking the UI. EOF is also handled by the helper.
        if let Some(mut child) = self.child.take() {
            std::thread::spawn(move || {
                for _ in 0..50 {
                    if child.try_wait().ok().flatten().is_some() {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                let _ = child.kill();
                let _ = child.wait();
            });
        }
    }
}
