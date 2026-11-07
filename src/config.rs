//! Saved settings, readable only by this user.

use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::sources::subsonic::Credentials;

#[derive(Default, Serialize, Deserialize, PartialEq, Debug)]
#[serde(default)]
pub struct Config {
    pub navidrome: Option<Credentials>,
    /// Where the user keeps their music, and where downloads go.
    #[serde(alias = "download_folder")]
    pub music_folder: Option<PathBuf>,
    pub theme: Option<String>,
    pub visual: Option<String>,
    /// Albums listened to, latest first.
    pub recent_albums: Vec<String>,
    pub collections: crate::collections::Collections,
    pub discord_enabled: bool,
    /// Of an application the user made on Discord's developer site.
    pub discord_client_id: Option<String>,
    /// Puts every cover of the library on a public host, ahead of time.
    pub host_covers: bool,
    pub shuffle: bool,
    /// Never asks GitHub for a new version on its own.
    pub no_update_check: bool,
}

fn path() -> Result<PathBuf> {
    dirs::config_dir()
        .map(|dir| dir.join("onde/config.json"))
        .ok_or_else(|| anyhow!("dossier de configuration introuvable"))
}

pub fn load() -> Config {
    path().map_or_else(|_| Config::default(), |path| load_from(&path))
}

pub fn save(config: &Config) -> Result<PathBuf> {
    let path = path()?;
    save_to(config, &path)?;
    Ok(path)
}

fn load_from(path: &Path) -> Config {
    fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn save_to(config: &Config, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("création du dossier de configuration")?;
    }
    // Write beside it then rename, so a crash can't leave a truncated file.
    let temporary = path.with_extension("tmp");
    let _ = fs::remove_file(&temporary);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    // Under Windows, the user's profile folder is already private.
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temporary)
        .context("écriture de la configuration")?;
    file.write_all(serde_json::to_string_pretty(config)?.as_bytes())?;
    file.sync_all()?;
    fs::rename(&temporary, path).context("écriture de la configuration")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_former_name_of_the_music_folder() {
        let config: Config = serde_json::from_str(r#"{"download_folder":"/m"}"#).unwrap();
        assert_eq!(config.music_folder, Some("/m".into()));
    }
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn config() -> Config {
        Config {
            navidrome: Some(Credentials {
                url: "http://a.test".into(),
                user: "u".into(),
                salt: "s".into(),
                token: "t".into(),
            }),
            music_folder: Some("/musique".into()),
            ..Config::default()
        }
    }

    #[test]
    fn saves_privately_and_loads_back() {
        let path = PathBuf::from("target/config-test/roundtrip/config.json");
        let _ = fs::remove_file(&path);
        save_to(&config(), &path).unwrap();
        assert_eq!(load_from(&path), config());
        #[cfg(unix)]
        {
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_missing_or_broken_file_gives_defaults() {
        assert_eq!(
            load_from(Path::new("target/config-test/absent.json")),
            Config::default()
        );
        let path = PathBuf::from("target/config-test/broken.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{pas du json").unwrap();
        assert_eq!(load_from(&path), Config::default());
    }
}
