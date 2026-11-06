use std::time::{Duration, Instant};

/// Keep the latest user choice while one native transition is outstanding.
#[derive(Default)]
pub(crate) struct PlaybackIntent {
    continuing: bool,
    target: Option<String>,
    desired: Option<bool>,
    waiting: Option<(bool, Instant)>,
    /// Resends while audio already does what was wanted, as when a skip
    /// lands on a different song than the one awaited.
    matched_retries: u8,
}
/// Resends of a choice audio already matches before giving up on it.
const MAX_MATCHED_RETRIES: u8 = 3;
impl PlaybackIntent {
    pub fn request(&mut self, playing: bool) {
        self.matched_retries = 0;
        self.continuing = playing;
        self.desired = Some(playing);
    }
    pub fn loading(&mut self, now: Instant) {
        self.matched_retries = 0;
        self.continuing = true;
        self.target = None;
        self.desired = Some(true);
        self.waiting = Some((true, now));
    }
    pub fn loading_track(&mut self, uri: Option<String>, now: Instant) {
        self.loading(now);
        self.target = uri;
    }
    pub fn acknowledge_track(&mut self, uri: &str, playing: bool) {
        if self.target.as_deref().is_none_or(|target| target == uri) {
            self.acknowledge(playing);
        }
    }
    pub fn track_loading(&mut self, uri: String, now: Instant) {
        if self.target.as_ref().is_some_and(|target| target != &uri)
            || self.desired == Some(false)
            || (!self.continuing && self.desired.is_none())
        {
            return;
        }
        self.loading_track(Some(uri), now);
    }
    pub fn acknowledge(&mut self, playing: bool) {
        if self.desired.is_none_or(|wanted| wanted == playing) {
            self.continuing = playing;
        }
        if self.waiting.is_some_and(|(wanted, _)| wanted == playing) {
            self.waiting = None;
            if self.desired == Some(playing) {
                self.desired = None;
                self.target = None;
            }
        }
    }
    pub fn next(&mut self, playing: bool, now: Instant) -> Option<bool> {
        let retry = self.waiting.is_some();
        if self
            .waiting
            .is_some_and(|(_, sent)| now.saturating_duration_since(sent) < Duration::from_secs(1))
        {
            return None;
        }
        self.waiting = None;
        let wanted = self.desired?;
        if !retry && wanted == playing {
            self.desired = None;
            return None;
        }
        if retry && wanted == playing {
            self.matched_retries += 1;
            if self.matched_retries > MAX_MATCHED_RETRIES {
                self.reset();
                return None;
            }
        }
        self.waiting = Some((wanted, now));
        Some(wanted)
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}
/// Decides when "unavailable" from the player is worth reporting. The player
/// also reports preloads that failed and tracks it skips past by itself, so
/// only the current track counts, and only if nothing plays soon after.
#[derive(Default)]
pub(crate) struct UnavailableWatch {
    current: Option<String>,
    pending: Option<Instant>,
    /// Refused songs in a row when this last reported.
    refused: usize,
}
impl UnavailableWatch {
    const GRACE: Duration = Duration::from_secs(5);
    /// A track started loading or playing.
    pub fn track(&mut self, uri: &str) {
        if self.current.as_deref() != Some(uri) {
            self.pending = None;
        }
        self.current = Some(uri.to_owned());
    }
    /// Audio is playing again, so any skip worked.
    pub fn playing(&mut self, uri: &str) {
        self.current = Some(uri.to_owned());
        self.pending = None;
        self.refused = 0;
    }
    pub fn unavailable(&mut self, uri: &str, now: Instant) {
        if self.current.as_deref() == Some(uri) && self.pending.is_none() {
            self.pending = Some(now);
        }
    }
    /// True once per stop when the current song failed after Spotify
    /// refused keys for `refused` songs in a row: the account is blocked,
    /// not the songs, and the player has stopped.
    pub fn key_refused(&mut self, uri: &str, refused: usize) -> bool {
        if refused < 2 || self.current.as_deref() != Some(uri) || self.refused == refused {
            return false;
        }
        self.refused = refused;
        self.pending = None;
        true
    }
    /// True once, when the current track stayed unplayable for the grace period.
    pub fn due(&mut self, now: Instant) -> bool {
        if self
            .pending
            .is_some_and(|since| now.saturating_duration_since(since) >= Self::GRACE)
        {
            self.pending = None;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod unavailable_tests {
    use super::*;
    #[test]
    fn preload_failures_never_report() {
        let now = Instant::now();
        let mut watch = UnavailableWatch::default();
        watch.track("current");
        watch.unavailable("next", now);
        assert!(!watch.due(now + Duration::from_secs(10)));
    }
    #[test]
    fn automatic_skips_stay_quiet() {
        let now = Instant::now();
        let mut watch = UnavailableWatch::default();
        watch.track("broken");
        watch.unavailable("broken", now);
        watch.track("next");
        watch.playing("next");
        assert!(!watch.due(now + Duration::from_secs(10)));
    }
    #[test]
    fn refused_keys_report_once_per_stop() {
        let mut watch = UnavailableWatch::default();
        watch.track("first");
        watch.unavailable("first", Instant::now());
        assert!(!watch.key_refused("first", 1));
        watch.track("second");
        watch.unavailable("second", Instant::now());
        assert!(!watch.key_refused("next", 2));
        assert!(watch.key_refused("second", 2));
        assert!(!watch.key_refused("second", 2));
        assert!(!watch.due(Instant::now() + Duration::from_secs(10)));
        assert!(watch.key_refused("second", 3));
    }
    #[test]
    fn a_stuck_current_track_reports_once_after_the_grace_period() {
        let now = Instant::now();
        let mut watch = UnavailableWatch::default();
        watch.track("broken");
        watch.unavailable("broken", now);
        assert!(!watch.due(now + Duration::from_secs(4)));
        assert!(watch.due(now + Duration::from_secs(5)));
        assert!(!watch.due(now + Duration::from_secs(6)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_next_retries_a_paused_load_without_another_user_click() {
        let now = Instant::now();
        let mut intent = PlaybackIntent::default();
        intent.acknowledge_track("finished", true);
        intent.track_loading("next".into(), now);
        intent.acknowledge_track("next", false);
        assert_eq!(intent.next(false, now + Duration::from_secs(1)), Some(true));
        intent.acknowledge_track("next", true);
        assert_eq!(intent.next(true, now + Duration::from_secs(2)), None);
    }
    #[test]
    fn automatic_recovery_respects_an_explicit_pause() {
        let now = Instant::now();
        let mut intent = PlaybackIntent::default();
        intent.acknowledge_track("finished", true);
        intent.request(false);
        intent.acknowledge_track("finished", false);
        intent.track_loading("next".into(), now);
        assert_eq!(intent.next(false, now + Duration::from_secs(1)), None);
    }
    #[test]
    fn previous_song_cannot_acknowledge_a_new_load() {
        let now = Instant::now();
        let mut intent = PlaybackIntent::default();
        intent.loading_track(Some("new".into()), now);
        intent.acknowledge_track("old", true);
        assert_eq!(intent.next(true, now + Duration::from_secs(1)), Some(true));
        intent.acknowledge_track("new", true);
        assert_eq!(intent.next(true, now + Duration::from_secs(2)), None);
    }
    #[test]
    fn burst_keeps_only_the_latest_choice_after_the_current_ack() {
        let now = Instant::now();
        let mut intent = PlaybackIntent::default();
        intent.request(false);
        assert_eq!(intent.next(true, now), Some(false));
        for index in 0..100 {
            intent.request(index % 2 == 1);
            assert_eq!(intent.next(true, now), None);
        }
        intent.acknowledge(false);
        assert_eq!(intent.next(false, now), Some(true));
        intent.acknowledge(true);
        assert_eq!(intent.next(true, now), None);
    }
    #[test]
    fn pause_during_track_loading_waits_for_the_track_then_pauses() {
        let now = Instant::now();
        let mut intent = PlaybackIntent::default();
        intent.loading(now);
        intent.request(false);
        assert_eq!(intent.next(false, now), None);
        intent.acknowledge(true);
        assert_eq!(intent.next(true, now), Some(false));
    }
    #[test]
    fn a_skip_past_the_awaited_song_stops_resending_play() {
        let now = Instant::now();
        let mut intent = PlaybackIntent::default();
        intent.loading_track(Some("skipped".into()), now);
        intent.acknowledge_track("landed", true);
        let sent = (1..30)
            .filter_map(|second| intent.next(true, now + Duration::from_secs(second * 2)))
            .count();
        assert_eq!(sent, 3);
    }
    #[test]
    fn late_command_is_followed_by_latest_intent_even_if_state_already_matches() {
        let now = Instant::now();
        let mut intent = PlaybackIntent::default();
        intent.request(false);
        assert_eq!(intent.next(true, now), Some(false));
        intent.request(true);
        assert_eq!(intent.next(true, now + Duration::from_secs(2)), Some(true));
    }
}
