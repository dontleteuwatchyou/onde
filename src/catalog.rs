//! Albums and artists, worked out from the list of tracks.

use std::collections::HashMap;

use crate::library::Track;

pub struct Album {
    pub key: String,
    pub title: String,
    pub artist: String,
    /// In playing order, as indexes into the track list.
    pub tracks: Vec<usize>,
    pub added: u64,
    pub seconds: f64,
}

pub struct Artist {
    pub name: String,
    pub albums: Vec<usize>,
    pub tracks: usize,
}

#[derive(Default)]
pub struct Catalog {
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    by_key: HashMap<String, usize>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum TrackSort {
    Artist,
    Title,
    Added,
    Duration,
}

/// A track without an album stands alone: a folder of downloads has one
/// picture per track, so they mustn't share one.
pub fn album_key(track: &Track) -> String {
    let album = if track.album.trim().is_empty() {
        format!("\u{1}{}", track.title)
    } else {
        track.album.clone()
    };
    format!("{}\0{album}", track.artist).to_lowercase()
}

impl Catalog {
    pub fn build(tracks: &[Track]) -> Self {
        let mut catalog = Self::default();
        let mut artists: HashMap<String, usize> = HashMap::new();
        for (index, track) in tracks.iter().enumerate() {
            let key = album_key(track);
            let album = *catalog.by_key.entry(key.clone()).or_insert_with(|| {
                catalog.albums.push(Album {
                    key,
                    title: if track.album.trim().is_empty() {
                        track.title.clone()
                    } else {
                        track.album.clone()
                    },
                    artist: track.artist.clone(),
                    tracks: Vec::new(),
                    added: 0,
                    seconds: 0.0,
                });
                catalog.albums.len() - 1
            });
            let entry = &mut catalog.albums[album];
            entry.tracks.push(index);
            entry.added = entry.added.max(track.added);
            entry.seconds += track.duration;

            let artist = *artists
                .entry(track.artist.to_lowercase())
                .or_insert_with(|| {
                    catalog.artists.push(Artist {
                        name: track.artist.clone(),
                        albums: Vec::new(),
                        tracks: 0,
                    });
                    catalog.artists.len() - 1
                });
            let entry = &mut catalog.artists[artist];
            entry.tracks += 1;
            if !entry.albums.contains(&album) {
                entry.albums.push(album);
            }
        }
        catalog
    }

    pub fn album(&self, key: &str) -> Option<&Album> {
        self.albums.get(*self.by_key.get(key)?)
    }

    pub fn album_index(&self, key: &str) -> Option<usize> {
        self.by_key.get(key).copied()
    }

    pub fn artist(&self, name: &str) -> Option<&Artist> {
        let name = name.to_lowercase();
        self.artists
            .iter()
            .find(|artist| artist.name.to_lowercase() == name)
    }

    /// Latest first.
    pub fn recent_albums(&self, count: usize) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.albums.len()).collect();
        order.sort_by_key(|&album| std::cmp::Reverse(self.albums[album].added));
        order.truncate(count);
        order
    }

    pub fn matching_albums(&self, query: &str) -> Vec<usize> {
        let words = words(query);
        (0..self.albums.len())
            .filter(|&index| {
                let album = &self.albums[index];
                contains_all(&words, &[&album.title, &album.artist])
            })
            .collect()
    }

    pub fn matching_artists(&self, query: &str) -> Vec<usize> {
        let words = words(query);
        (0..self.artists.len())
            .filter(|&index| contains_all(&words, &[&self.artists[index].name]))
            .collect()
    }
}

pub fn matching_tracks(tracks: &[Track], query: &str, sort: TrackSort) -> Vec<usize> {
    let words = words(query);
    let mut found: Vec<usize> = (0..tracks.len())
        .filter(|&index| {
            let track = &tracks[index];
            contains_all(&words, &[&track.title, &track.artist, &track.album])
        })
        .collect();
    match sort {
        // The list already comes sorted by artist, then album.
        TrackSort::Artist => {}
        TrackSort::Title => found.sort_by_cached_key(|&i| tracks[i].title.to_lowercase()),
        TrackSort::Added => found.sort_by_key(|&i| std::cmp::Reverse(tracks[i].added)),
        TrackSort::Duration => {
            found.sort_by(|&a, &b| tracks[b].duration.total_cmp(&tracks[a].duration));
        }
    }
    found
}

fn words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

fn contains_all(words: &[String], fields: &[&str]) -> bool {
    if words.is_empty() {
        return true;
    }
    let fields: Vec<String> = fields.iter().map(|field| field.to_lowercase()).collect();
    words
        .iter()
        .all(|word| fields.iter().any(|field| field.contains(word.as_str())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Location;

    fn track(artist: &str, album: &str, title: &str, added: u64, duration: f64) -> Track {
        Track {
            location: Location::Remote(title.into()),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            disc: 0,
            number: 0,
            duration,
            extension: None,
            format: String::new(),
            added,
        }
    }

    fn sample() -> Vec<Track> {
        vec![
            track("Ana", "Mer", "Un", 5, 100.0),
            track("Ana", "Mer", "Deux", 9, 50.0),
            track("Ana", "", "Seul", 2, 300.0),
            track("Bo", "Mer", "Autre", 7, 10.0),
        ]
    }

    #[test]
    fn groups_tracks_by_artist_and_album() {
        let catalog = Catalog::build(&sample());
        assert_eq!(catalog.albums.len(), 3);
        let mer = catalog.album(&album_key(&sample()[0])).unwrap();
        assert_eq!(mer.tracks, vec![0, 1]);
        assert_eq!(mer.added, 9);
        assert_eq!(mer.seconds, 150.0);
    }

    #[test]
    fn a_track_without_album_gets_its_own_cover() {
        let tracks = sample();
        let catalog = Catalog::build(&tracks);
        let single = catalog.album(&album_key(&tracks[2])).unwrap();
        assert_eq!(single.title, "Seul");
        assert_eq!(single.tracks, vec![2]);
        let other = track("Ana", "", "Autre seul", 0, 0.0);
        assert_ne!(album_key(&tracks[2]), album_key(&other));
    }

    #[test]
    fn artists_list_their_albums_once() {
        let catalog = Catalog::build(&sample());
        let ana = catalog.artist("ana").unwrap();
        assert_eq!((ana.tracks, ana.albums.len()), (3, 2));
    }

    #[test]
    fn recent_albums_come_first() {
        let catalog = Catalog::build(&sample());
        let recent = catalog.recent_albums(2);
        assert_eq!(catalog.albums[recent[0]].artist, "Ana");
        assert_eq!(catalog.albums[recent[0]].title, "Mer");
        assert_eq!(catalog.albums[recent[1]].artist, "Bo");
    }

    #[test]
    fn searching_needs_every_word_somewhere() {
        let tracks = sample();
        assert_eq!(
            matching_tracks(&tracks, "ana mer", TrackSort::Artist),
            vec![0, 1]
        );
        assert_eq!(matching_tracks(&tracks, "SEUL", TrackSort::Artist), vec![2]);
        assert!(matching_tracks(&tracks, "bo seul", TrackSort::Artist).is_empty());
        assert_eq!(matching_tracks(&tracks, "", TrackSort::Artist).len(), 4);
    }

    #[test]
    fn sorts_the_tracks() {
        let tracks = sample();
        assert_eq!(
            matching_tracks(&tracks, "", TrackSort::Title),
            vec![3, 1, 2, 0]
        );
        assert_eq!(
            matching_tracks(&tracks, "", TrackSort::Added),
            vec![1, 3, 0, 2]
        );
        assert_eq!(
            matching_tracks(&tracks, "", TrackSort::Duration),
            vec![2, 0, 1, 3]
        );
    }
}
