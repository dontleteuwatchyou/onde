//! Keeps the public address of each cover, and hosts the whole library's covers
//! ahead of time, so that Discord gets its image as soon as a track starts.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::online_cover::Picture;

/// One at a time and gently: it is a free service.
const PAUSE: Duration = if cfg!(test) {
    Duration::ZERO
} else {
    Duration::from_secs(1)
};

/// Album key → address. Written out at each change, as hosting can't be undone
/// and shouldn't be done twice.
pub struct Links {
    known: Mutex<HashMap<String, String>>,
    path: Option<PathBuf>,
}

impl Links {
    pub fn load() -> Self {
        let path = dirs::data_dir().map(|dir| dir.join("onde/cover-links.json"));
        let known = path
            .as_ref()
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_default();
        Self {
            known: Mutex::new(known),
            path,
        }
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self {
            known: Mutex::new(HashMap::new()),
            path: None,
        }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.known.lock().unwrap().get(key).cloned()
    }

    pub fn insert(&self, key: String, url: String) {
        let mut known = self.known.lock().unwrap();
        known.insert(key, url);
        self.save(&known);
    }

    pub fn remove(&self, key: &str) {
        let mut known = self.known.lock().unwrap();
        if known.remove(key).is_some() {
            self.save(&known);
        }
    }

    fn save(&self, known: &HashMap<String, String>) {
        let Some(path) = &self.path else {
            return;
        };
        let Ok(content) = serde_json::to_string(known) else {
            return;
        };
        if let Some(folder) = path.parent() {
            let _ = fs::create_dir_all(folder);
        }
        let draft = path.with_extension("tmp");
        if fs::write(&draft, content).is_ok() {
            let _ = fs::rename(&draft, path);
        }
    }
}

/// The work under way. Dropping it stops the work.
pub struct Hosting {
    total: usize,
    done: Arc<AtomicUsize>,
    failed: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl Drop for Hosting {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Hosting {
    pub fn start(
        jobs: Vec<(String, Picture)>,
        links: Arc<Links>,
        upload: fn(&[u8]) -> Option<String>,
    ) -> Self {
        let total = jobs.len();
        let done = Arc::new(AtomicUsize::new(0));
        let failed = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (shared_done, shared_failed, shared_stop) =
            (done.clone(), failed.clone(), stop.clone());
        std::thread::spawn(move || {
            for (key, picture) in jobs {
                if shared_stop.load(Ordering::Relaxed) {
                    return;
                }
                // A track without a picture has nothing to host.
                if let Some(bytes) = picture() {
                    match upload(&bytes) {
                        Some(url) => {
                            links.insert(key, url);
                        }
                        None => {
                            // A transient refusal must not strand every cover
                            // that comes after this one in the library.
                            shared_failed.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    std::thread::sleep(PAUSE);
                }
                shared_done.fetch_add(1, Ordering::Relaxed);
            }
        });
        Self {
            total,
            done,
            failed,
            stop,
        }
    }

    pub fn describe(&self) -> String {
        let done = self.done.load(Ordering::Relaxed);
        let failed = self.failed.load(Ordering::Relaxed);
        if done >= self.total && failed > 0 {
            format!("Terminé : {failed} pochette(s) non hébergée(s)")
        } else if done >= self.total {
            "Toutes hébergées".to_owned()
        } else {
            format!("{done} sur {}", self.total)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_until(check: impl Fn() -> bool) {
        for _ in 0..500 {
            if check() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("trop long");
    }

    fn picture(present: bool) -> Picture {
        Box::new(move || present.then(|| b"image".to_vec()))
    }

    fn hosted(_: &[u8]) -> Option<String> {
        Some("https://hote/a.jpg".into())
    }

    fn refused(_: &[u8]) -> Option<String> {
        None
    }

    #[test]
    fn hosts_what_has_a_picture_and_keeps_the_link() {
        let links = Arc::new(Links::in_memory());
        let jobs = vec![
            ("a".to_owned(), picture(true)),
            ("b".to_owned(), picture(false)),
        ];
        let hosting = Hosting::start(jobs, links.clone(), hosted);
        wait_until(|| hosting.done.load(Ordering::Relaxed) == 2);
        assert_eq!(links.get("a").as_deref(), Some("https://hote/a.jpg"));
        assert_eq!(links.get("b"), None);
        assert_eq!(hosting.describe(), "Toutes hébergées");
    }

    #[test]
    fn gives_up_when_the_host_keeps_refusing() {
        let links = Arc::new(Links::in_memory());
        let jobs = (0..9).map(|n| (n.to_string(), picture(true))).collect();
        let hosting = Hosting::start(jobs, links, refused);
        wait_until(|| hosting.done.load(Ordering::Relaxed) == 9);
        assert_eq!(hosting.failed.load(Ordering::Relaxed), 9);
        assert!(hosting.describe().contains("9 pochette(s) non hébergée(s)"));
    }

    #[test]
    fn links_are_found_again_after_a_restart() {
        let path = std::env::temp_dir().join(format!("onde-links-{}.json", std::process::id()));
        let links = Links {
            known: Mutex::new(HashMap::new()),
            path: Some(path.clone()),
        };
        links.insert("a".into(), "https://hote/a.jpg".into());
        let content = fs::read_to_string(&path).unwrap();
        let read: HashMap<String, String> = serde_json::from_str(&content).unwrap();
        assert_eq!(read["a"], "https://hote/a.jpg");
        links.remove("a");
        assert_eq!(links.get("a"), None);
        let _ = fs::remove_file(path);
    }
}
