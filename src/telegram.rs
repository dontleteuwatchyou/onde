//! Telegram bridge: a Python child living as long as the app. JSON events contain no login secrets.
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
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
    pub buttons: Vec<MenuButton>,
}

pub struct Telegram {
    child: Option<Child>,
    input: ChildStdin,
    pub state: Arc<Mutex<State>>,
}

fn root() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("Dossier de configuration absent")?
        .join("onde/telegram"))
}

fn python() -> std::ffi::OsString {
    if let Some(python) = std::env::var_os("ONDE_TELEGRAM_PYTHON") {
        return python;
    }
    match dirs::data_dir().map(|data| data.join("onde/telegram-venv/bin/python")) {
        Some(venv) if venv.exists() => venv.into(),
        _ => "python3".into(),
    }
}

/// API identifiers from `api.env`, so the app also works when not launched from a
/// configured shell. The real environment wins.
fn api_env(root: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(root.join("api.env"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.trim().split_once('='))
        .filter(|(key, _)| {
            matches!(*key, "ONDE_TELEGRAM_API_ID" | "ONDE_TELEGRAM_API_HASH")
                && std::env::var_os(key).is_none()
        })
        .map(|(key, value)| (key.to_owned(), value.trim().trim_matches(['\'', '"']).to_owned()))
        .collect()
}

impl Telegram {
    pub fn has_session() -> bool {
        root().is_ok_and(|root| root.join("account.session").exists())
    }

    pub fn start() -> Result<Self> {
        let root = root()?;
        let mut child = Command::new(python())
            .envs(api_env(&root))
            .args([
                "-u",
                "-c",
                include_str!("../integrations/telegram/bridge.py"),
            ])
            .arg(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Démarrage Python Telegram impossible")?;
        let input = child.stdin.take().context("Entrée Telegram absente")?;
        let output = child.stdout.take().context("Sortie Telegram absente")?;
        let errors = child.stderr.take().context("Erreurs Telegram absentes")?;
        // Kept in memory only: the last line is what explains an unexpected stop.
        let last_error = std::thread::spawn(move || {
            BufReader::new(errors)
                .lines()
                .map_while(Result::ok)
                .filter(|line| !line.trim().is_empty())
                .last()
                .map(|line| line.trim().chars().take(300).collect::<String>())
        });
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
                if let Some(value) = event["file"].as_str() {
                    state.files.push(value.to_owned());
                }
            }
            let stopped = match last_error.join().ok().flatten() {
                Some(reason) => format!("Service arrêté : {reason}"),
                None => "Service arrêté.".to_owned(),
            };
            let mut state = shared.lock().unwrap();
            state.connected = false;
            state.busy = false;
            state.qr.clear();
            state.buttons.clear();
            if !state.status.is_empty() {
                state.status.push_str(" · ");
            }
            state.status.push_str(&stopped);
        });
        Ok(Self {
            child: Some(child),
            input,
            state,
        })
    }

    pub fn is_alive(&mut self) -> bool {
        self.child
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
    }

    pub fn send(&mut self, command: Value) -> Result<()> {
        let mut line = serde_json::to_vec(&command)?;
        line.push(b'\n');
        self.input
            .write_all(&line)
            .and_then(|()| self.input.flush())
            .context("Service Telegram arrêté : cliquer sur Connecter pour le relancer")
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
