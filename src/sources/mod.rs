//! Where music comes from. Adding a source means implementing `MusicSource`.

pub mod local;
pub mod subsonic;

use std::path::Path;

use anyhow::Result;

use crate::decoder::AudioSource;
use crate::library::Track;
use crate::lyrics::Lyrics;
use crate::lyrics::source::Origin;

/// Methods may block on the network: call them off the UI thread,
/// except `audio`, which must return immediately.
pub trait MusicSource: Send + Sync {
    /// The folder on this machine it reads, if any.
    fn folder(&self) -> Option<&Path> {
        None
    }

    fn list_tracks(&self) -> Result<Vec<Track>>;
    fn audio(&self, track: &Track) -> Result<AudioSource>;
    /// Lyrics the source itself holds, tried before searching online.
    fn lyrics(&self, track: &Track) -> Result<Option<(Lyrics, Origin)>>;
    /// The track's image, as encoded in the file.
    fn cover(&self, track: &Track) -> Result<Option<Vec<u8>>>;
    /// Asks the source to notice new files, and waits until it has.
    fn rescan(&self) -> Result<()> {
        Ok(())
    }
}
