//! The list of tracks and which one is playing.

use crate::library::Track;

/// Going back this far into a track restarts it instead of changing track.
const RESTART_AFTER_SECONDS: f64 = 3.0;

#[derive(Debug, PartialEq)]
pub enum Previous {
    Restart,
    Track(usize),
}

#[derive(Default)]
pub struct Queue {
    pub tracks: Vec<Track>,
    pub current: Option<usize>,
}

impl Queue {
    pub fn replace(&mut self, tracks: Vec<Track>) {
        self.tracks = tracks;
        self.current = None;
    }

    /// New contents of the same library: what is playing stays current.
    pub fn refresh(&mut self, tracks: Vec<Track>) {
        let playing = self.current_track().map(|track| track.location.clone());
        self.tracks = tracks;
        self.current =
            playing.and_then(|location| self.tracks.iter().position(|t| t.location == location));
    }

    pub fn current_track(&self) -> Option<&Track> {
        self.tracks.get(self.current?)
    }

    pub fn next_index(&self) -> Option<usize> {
        let next = self.current.map_or(0, |current| current + 1);
        (next < self.tracks.len()).then_some(next)
    }

    pub fn previous(&self, position: f64) -> Previous {
        match self.current {
            Some(current) if current > 0 && position <= RESTART_AFTER_SECONDS => {
                Previous::Track(current - 1)
            }
            _ => Previous::Restart,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(len: usize, current: Option<usize>) -> Queue {
        let track = Track {
            location: crate::library::Location::Remote("a".into()),
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            disc: 0,
            number: 0,
            duration: 0.0,
            extension: None,
            format: String::new(),
        };
        Queue {
            tracks: vec![track; len],
            current,
        }
    }

    #[test]
    fn next_starts_at_the_first_track_and_stops_at_the_end() {
        assert_eq!(queue(2, None).next_index(), Some(0));
        assert_eq!(queue(2, Some(0)).next_index(), Some(1));
        assert_eq!(queue(2, Some(1)).next_index(), None);
        assert_eq!(queue(0, None).next_index(), None);
    }

    #[test]
    fn previous_restarts_late_in_a_track_or_on_the_first_one() {
        assert_eq!(queue(3, Some(2)).previous(1.0), Previous::Track(1));
        assert_eq!(queue(3, Some(2)).previous(10.0), Previous::Restart);
        assert_eq!(queue(3, Some(0)).previous(1.0), Previous::Restart);
    }

    #[test]
    fn replacing_tracks_clears_the_selection() {
        let mut q = queue(3, Some(2));
        q.replace(Vec::new());
        assert!(q.current_track().is_none());
    }

    #[test]
    fn refreshing_keeps_the_playing_track_when_it_moves() {
        let track = |id: &str| Track {
            location: crate::library::Location::Remote(id.into()),
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            disc: 0,
            number: 0,
            duration: 0.0,
            extension: None,
            format: String::new(),
        };
        let mut queue = Queue {
            tracks: vec![track("a"), track("b")],
            current: Some(1),
        };
        queue.refresh(vec![track("new"), track("a"), track("b")]);
        assert_eq!(queue.current, Some(2));
        queue.refresh(vec![track("a")]);
        assert_eq!(queue.current, None);
    }
}
