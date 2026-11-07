//! Liked tracks and playlists. They hold identifiers rather than positions,
//! so they survive a new scan of the library.

use serde::{Deserialize, Serialize};

use crate::library::{Location, Track};

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Debug)]
pub struct Playlist {
    pub name: String,
    pub tracks: Vec<String>,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Debug)]
#[serde(default)]
pub struct Collections {
    /// Latest first.
    pub liked: Vec<String>,
    pub playlists: Vec<Playlist>,
}

pub fn track_id(track: &Track) -> String {
    match &track.location {
        Location::File(path) => format!("file:{}", path.display()),
        Location::Remote(id) => format!("remote:{id}"),
    }
}

impl Collections {
    pub fn is_liked(&self, id: &str) -> bool {
        self.liked.iter().any(|known| known == id)
    }

    /// Returns whether the track is liked now.
    pub fn toggle_liked(&mut self, id: &str) -> bool {
        if self.is_liked(id) {
            self.liked.retain(|known| known != id);
            false
        } else {
            self.liked.insert(0, id.to_owned());
            true
        }
    }

    /// Returns its position, or nothing when the name is blank.
    pub fn create(&mut self, name: &str) -> Option<usize> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        self.playlists.push(Playlist {
            name: name.to_owned(),
            tracks: Vec::new(),
        });
        Some(self.playlists.len() - 1)
    }

    /// A track only goes once in a playlist. Returns whether it was added.
    pub fn add(&mut self, playlist: usize, id: &str) -> bool {
        let Some(playlist) = self.playlists.get_mut(playlist) else {
            return false;
        };
        if playlist.tracks.iter().any(|known| known == id) {
            return false;
        }
        playlist.tracks.push(id.to_owned());
        true
    }

    pub fn remove(&mut self, playlist: usize, id: &str) {
        if let Some(playlist) = self.playlists.get_mut(playlist) {
            playlist.tracks.retain(|known| known != id);
        }
    }

    pub fn delete(&mut self, playlist: usize) {
        if playlist < self.playlists.len() {
            self.playlists.remove(playlist);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liking_twice_undoes_it_and_the_latest_comes_first() {
        let mut collections = Collections::default();
        assert!(collections.toggle_liked("a"));
        assert!(collections.toggle_liked("b"));
        assert_eq!(collections.liked, ["b", "a"]);
        assert!(!collections.toggle_liked("b"));
        assert_eq!(collections.liked, ["a"]);
    }

    #[test]
    fn a_playlist_needs_a_name_and_keeps_each_track_once() {
        let mut collections = Collections::default();
        assert_eq!(collections.create("   "), None);
        let index = collections.create("  Soir ").unwrap();
        assert_eq!(collections.playlists[index].name, "Soir");
        assert!(collections.add(index, "a"));
        assert!(!collections.add(index, "a"));
        assert!(!collections.add(9, "a"));
        collections.remove(index, "a");
        assert!(collections.playlists[index].tracks.is_empty());
        collections.delete(index);
        collections.delete(9);
        assert!(collections.playlists.is_empty());
    }

    #[test]
    fn local_and_remote_tracks_have_distinct_identifiers() {
        let track = |location| Track {
            location,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            disc: 0,
            number: 0,
            duration: 0.0,
            extension: None,
            format: String::new(),
            added: 0,
        };
        assert_eq!(track_id(&track(Location::File("/m/a".into()))), "file:/m/a");
        assert_eq!(
            track_id(&track(Location::Remote("/m/a".into()))),
            "remote:/m/a"
        );
    }
}
