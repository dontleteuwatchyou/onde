//! A folder on this machine.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use super::MusicSource;
use crate::decoder::AudioSource;
use crate::library::{self, Location, Track};
use crate::lyrics::Lyrics;
use crate::lyrics::source::{Origin, load_local};

pub struct LocalFolder(pub PathBuf);

impl MusicSource for LocalFolder {
    fn folder(&self) -> Option<&Path> {
        Some(&self.0)
    }

    fn list_tracks(&self) -> Result<Vec<Track>> {
        Ok(library::scan(&self.0))
    }

    fn audio(&self, track: &Track) -> Result<AudioSource> {
        match &track.location {
            Location::File(path) => Ok(AudioSource::File(path.clone())),
            Location::Remote(_) => bail!("ce morceau n'est pas un fichier local"),
        }
    }

    fn cover(&self, track: &Track) -> Result<Option<Vec<u8>>> {
        Ok(match &track.location {
            Location::File(path) => library::read_cover(path),
            Location::Remote(_) => None,
        })
    }

    fn lyrics(&self, track: &Track) -> Result<Option<(Lyrics, Origin)>> {
        Ok(match &track.location {
            Location::File(path) => load_local(path),
            Location::Remote(_) => None,
        })
    }
}
