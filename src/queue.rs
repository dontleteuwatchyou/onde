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
    /// What plays after the current track. Empty: the whole list, in order.
    pub context: Vec<usize>,
    /// The list chosen, in its own order. Empty: the whole list.
    order: Vec<usize>,
    pub shuffle: bool,
}

/// Small and good enough for a playing order, with no dependency for it.
fn next_random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn shuffled(mut list: Vec<usize>, seed: u64) -> Vec<usize> {
    // Zero would stay zero.
    let mut state = seed | 1;
    for last in (1..list.len()).rev() {
        list.swap(last, (next_random(&mut state) % (last as u64 + 1)) as usize);
    }
    list
}

impl Queue {
    pub fn replace(&mut self, tracks: Vec<Track>) {
        self.tracks = tracks;
        self.current = None;
        self.context.clear();
        self.order.clear();
    }

    /// New contents of the same library: what is playing stays current.
    pub fn refresh(&mut self, tracks: Vec<Track>) {
        let playing = self.current_track().map(|track| track.location.clone());
        let places = |list: &[usize]| -> Vec<_> {
            list.iter()
                .filter_map(|&index| self.tracks.get(index))
                .map(|track| track.location.clone())
                .collect()
        };
        let (context, order) = (places(&self.context), places(&self.order));
        self.tracks = tracks;
        let indexes = |list: &[crate::library::Location]| -> Vec<usize> {
            list.iter()
                .filter_map(|location| self.tracks.iter().position(|t| t.location == *location))
                .collect()
        };
        (self.context, self.order) = (indexes(&context), indexes(&order));
        self.current =
            playing.and_then(|location| self.tracks.iter().position(|t| t.location == location));
    }

    pub fn current_track(&self) -> Option<&Track> {
        self.tracks.get(self.current?)
    }

    /// Starts following a list, or the whole library when it is empty.
    pub fn follow(&mut self, first: usize, order: Vec<usize>, seed: u64) {
        self.order = order;
        self.arrange(Some(first), seed);
    }

    /// The current track stays, and what follows changes.
    pub fn toggle_shuffle(&mut self, seed: u64) {
        self.shuffle = !self.shuffle;
        self.arrange(self.current, seed);
    }

    fn arrange(&mut self, first: Option<usize>, seed: u64) {
        if !self.shuffle {
            self.context = self.order.clone();
            return;
        }
        let mut pool = if self.order.is_empty() {
            (0..self.tracks.len()).collect()
        } else {
            self.order.clone()
        };
        pool.retain(|&index| Some(index) != first);
        let rest = shuffled(pool, seed);
        self.context = first.into_iter().chain(rest).collect();
    }

    /// What to play when nothing was playing yet.
    pub fn first_index(&self, seed: u64) -> Option<usize> {
        if self.tracks.is_empty() {
            None
        } else if self.shuffle {
            Some((seed % self.tracks.len() as u64) as usize)
        } else {
            Some(0)
        }
    }

    /// Where the current track sits in the list being played.
    fn place_in_context(&self) -> Option<usize> {
        let current = self.current?;
        self.context.iter().position(|&index| index == current)
    }

    pub fn next_index(&self) -> Option<usize> {
        if let Some(place) = self.place_in_context() {
            return self.context.get(place + 1).copied();
        }
        let next = self.current.map_or(0, |current| current + 1);
        (next < self.tracks.len()).then_some(next)
    }

    pub fn upcoming_count(&self) -> usize {
        match self.place_in_context() {
            Some(place) => self.context.len() - place - 1,
            None => {
                let next = self.current.map_or(0, |current| current + 1);
                self.tracks.len().saturating_sub(next)
            }
        }
    }

    pub fn upcoming_at(&self, row: usize) -> Option<usize> {
        match self.place_in_context() {
            Some(place) => self.context.get(place + 1 + row).copied(),
            None => {
                let next = self.current.map_or(0, |current| current + 1) + row;
                (next < self.tracks.len()).then_some(next)
            }
        }
    }

    pub fn previous(&self, position: f64) -> Previous {
        if let Some(place) = self.place_in_context() {
            return if place > 0 && position <= RESTART_AFTER_SECONDS {
                Previous::Track(self.context[place - 1])
            } else {
                Previous::Restart
            };
        }
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

    #[test]
    fn shuffling_keeps_every_track_once_and_starts_with_the_chosen_one() {
        let mut list = queue(50, None);
        list.shuffle = true;
        list.follow(7, Vec::new(), 12345);
        assert_eq!(list.context[0], 7);
        assert_ne!(list.context, (0..50).collect::<Vec<_>>());
        let mut sorted = list.context.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..50).collect::<Vec<_>>());
    }

    #[test]
    fn shuffling_a_playlist_stays_inside_it() {
        let mut list = queue(50, None);
        list.shuffle = true;
        list.follow(3, vec![3, 9, 20, 41], 99);
        assert_eq!(list.context[0], 3);
        let mut sorted = list.context.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, [3, 9, 20, 41]);
    }

    #[test]
    fn turning_shuffle_off_comes_back_to_the_list_and_keeps_the_track() {
        let mut list = queue(10, Some(4));
        list.follow(4, vec![4, 5, 6], 1);
        list.toggle_shuffle(777);
        assert_eq!(list.current, Some(4));
        assert_eq!(list.context[0], 4);
        list.toggle_shuffle(777);
        assert_eq!(list.context, [4, 5, 6]);
        assert_eq!(list.next_index(), Some(5));
    }

    #[test]
    fn the_first_track_is_random_only_when_shuffling() {
        let mut list = queue(10, None);
        assert_eq!(list.first_index(13), Some(0));
        list.shuffle = true;
        assert_eq!(list.first_index(13), Some(3));
        assert_eq!(queue(0, None).first_index(13), None);
    }

    #[test]
    fn a_playlist_decides_what_comes_next_and_before() {
        let mut playlist = queue(5, Some(3));
        playlist.context = vec![3, 0, 4];
        assert_eq!(playlist.next_index(), Some(0));
        assert_eq!(playlist.upcoming_count(), 2);
        assert_eq!(playlist.upcoming_at(1), Some(4));
        assert_eq!(playlist.upcoming_at(2), None);
        assert_eq!(playlist.previous(0.0), Previous::Restart);
        playlist.current = Some(4);
        assert_eq!(playlist.next_index(), None, "fin de la playlist");
        assert_eq!(playlist.previous(0.0), Previous::Track(0));
    }

    #[test]
    fn without_a_playlist_the_whole_list_follows() {
        let list = queue(3, Some(0));
        assert_eq!(list.upcoming_count(), 2);
        assert_eq!(list.upcoming_at(0), Some(1));
        assert_eq!(list.upcoming_at(2), None);
    }

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
            added: 0,
        };
        Queue {
            tracks: vec![track; len],
            current,
            ..Queue::default()
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
            added: 0,
        };
        let mut queue = Queue {
            tracks: vec![track("a"), track("b")],
            current: Some(1),
            ..Queue::default()
        };
        queue.refresh(vec![track("new"), track("a"), track("b")]);
        assert_eq!(queue.current, Some(2));
        queue.refresh(vec![track("a")]);
        assert_eq!(queue.current, None);
    }
}
