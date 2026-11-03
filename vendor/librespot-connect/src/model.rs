use crate::{
    core::dealer::protocol::SkipTo, protocol::context_player_options::ContextPlayerOptionOverrides,
};

use std::ops::Deref;

/// Internal queue identity: requested DJ queues resume as music, not legacy DJ sessions.
pub const REQUESTED_DJ_CONTEXT: &str = "lyssna:requested-dj";

/// Context metadata marking a play command's pages as the order to play,
/// such as a Random sort. Spotify's apps ignore it and play the playlist.
pub const LISTED_CONTEXT: &str = "lyssna.listed";

/// Asynchronous native context page result.
pub type ContextPageFuture<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<crate::protocol::context_page::ContextPage, crate::core::Error>,
            > + Send
            + 'a,
    >,
>;
/// Native, account-bound paging for contexts the legacy context API cannot resolve.
pub trait ContextPageProvider: std::fmt::Debug + Send + Sync {
    /// Whether a page marker belongs to this provider.
    fn owns(&self, uri: &str) -> bool;
    /// Fetch an owned page, preserving mutation results across repeated reads.
    fn page(&self, uri: String) -> ContextPageFuture<'_>;
}

/// Request for loading playback
#[derive(Debug, Clone)]
pub struct LoadRequest {
    pub(super) context: PlayContext,
    pub(super) options: LoadRequestOptions,
}

impl Deref for LoadRequest {
    type Target = LoadRequestOptions;

    fn deref(&self) -> &Self::Target {
        &self.options
    }
}

#[derive(Debug, Clone)]
pub(super) enum PlayContext {
    Uri(String),
    Tracks(Vec<String>),
}

/// The parameters for creating a load request
#[derive(Debug, Default, Clone)]
pub struct LoadRequestOptions {
    /// Optional native source for subsequent context pages.
    pub page_provider: Option<std::sync::Arc<dyn ContextPageProvider>>,
    /// A freshly prepared context supplied by the native client.
    pub prepared_context: Option<Box<crate::protocol::context::Context>>,
    /// Reopen an existing DJ session instead of generating another session.
    pub session_url: Option<String>,
    /// Select a track relative to `playing_track` after resolving all required pages.
    pub relative_offset: i32,
    /// Whether the given tracks should immediately start playing, or just be initially loaded.
    pub start_playing: bool,
    /// Start the playback at a specific point of the track.
    ///
    /// The provided value is used as milliseconds. Providing a value greater
    /// than the track duration will start the track at the beginning.
    pub seek_to: u32,
    /// Options that decide how the context starts playing
    pub context_options: Option<LoadContextOptions>,
    /// Decides the starting position in the given context.
    ///
    /// If the provided item doesn't exist or is out of range,
    /// the playback starts at the beginning of the context.
    ///
    /// If `None` is provided and `shuffle` is `true`, a random track is played, otherwise the first
    pub playing_track: Option<PlayingTrack>,
}

/// The options which decide how the playback is started
///
/// Separated into an `enum` to exclude the other variants from being used
/// simultaneously, as they are not compatible.
#[derive(Debug, Clone)]
pub enum LoadContextOptions {
    /// Starts the context with options
    Options(Options),
    /// Starts the playback as the autoplay variant of the context
    ///
    /// This is the same as finishing a context and
    /// automatically continuing playback of similar tracks
    Autoplay,
}

/// The available options that indicate how to start the context
#[derive(Debug, Default, Clone)]
pub struct Options {
    /// Start the context in shuffle mode
    pub shuffle: bool,
    /// Start the context in repeat mode
    pub repeat: bool,
    /// Start the context, repeating the first track until skipped or manually disabled
    pub repeat_track: bool,
}

impl From<ContextPlayerOptionOverrides> for Options {
    fn from(value: ContextPlayerOptionOverrides) -> Self {
        Self {
            shuffle: value.shuffling_context.unwrap_or_default(),
            repeat: value.repeating_context.unwrap_or_default(),
            repeat_track: value.repeating_track.unwrap_or_default(),
        }
    }
}

impl LoadRequest {
    /// Create a load request from a `context_uri`
    ///
    /// For supported `context_uri` see [`SpClient::get_context`](librespot_core::spclient::SpClient::get_context)
    ///
    /// Equivalent to using [`/me/player/play`](https://developer.spotify.com/documentation/web-api/reference/start-a-users-playback)
    /// and providing `context_uri`
    pub fn from_context_uri(context_uri: String, options: LoadRequestOptions) -> Self {
        Self {
            context: PlayContext::Uri(context_uri),
            options,
        }
    }

    /// Create a load request from a set of `tracks`
    ///
    /// Equivalent to using [`/me/player/play`](https://developer.spotify.com/documentation/web-api/reference/start-a-users-playback)
    /// and providing `uris`
    pub fn from_tracks(tracks: Vec<String>, options: LoadRequestOptions) -> Self {
        Self {
            context: PlayContext::Tracks(tracks),
            options,
        }
    }
}

/// An item that represent a track to play
#[derive(Debug, Clone)]
pub enum PlayingTrack {
    /// Prefer the persisted occurrence UID, falling back to its URI if Spotify regenerated UIDs.
    UidOrUri {
        /// The saved occurrence in its original context.
        uid: String,
        /// The catalogue URI to find if the occurrence was regenerated.
        uri: String,
    },
    /// Represent the track at a given index.
    Index(u32),
    /// Represent the uri of a track.
    Uri(String),
    #[doc(hidden)]
    /// Represent an internal identifier from spotify.
    ///
    /// The internal identifier is not the id contained in the uri. And rather
    /// an unrelated id probably unique in spotify's internal database. But that's
    /// just speculation.
    ///
    /// This identifier is not available by any public api. It's used for varies in
    /// any spotify client, like sorting, displaying which track is currently played
    /// and skipping to a track. Mobile uses it pretty intensively but also web and
    /// desktop seem to make use of it.
    Uid(String),
}

impl TryFrom<SkipTo> for PlayingTrack {
    type Error = ();

    fn try_from(value: SkipTo) -> Result<Self, Self::Error> {
        // order of checks is important, as the index can be 0, but still has an uid or uri provided,
        // so we only use the index as last resort
        if let Some(uri) = value.track_uri {
            Ok(PlayingTrack::Uri(uri))
        } else if let Some(uid) = value.track_uid {
            Ok(PlayingTrack::Uid(uid))
        } else if let Some(index) = value.track_index {
            Ok(PlayingTrack::Index(index))
        } else {
            Err(())
        }
    }
}

#[derive(Debug)]
pub(super) enum SpircPlayStatus {
    Stopped,
    LoadingPlay {
        position_ms: u32,
    },
    LoadingPause {
        position_ms: u32,
    },
    Playing {
        nominal_start_time: i64,
        preloading_of_next_track_triggered: bool,
    },
    Paused {
        position_ms: u32,
        preloading_of_next_track_triggered: bool,
    },
}
impl SpircPlayStatus {
    /// Apply an acknowledged audio-player start, including direct local resume.
    pub(super) fn player_started(
        &mut self,
        position_ms: u32,
        now_ms: i64,
        playing_event: bool,
    ) -> bool {
        let start = now_ms - i64::from(position_ms);
        match self {
            Self::Playing {
                nominal_start_time, ..
            } => {
                if (*nominal_start_time - start).abs() <= 100 {
                    return false;
                }
                *nominal_start_time = start;
            }
            Self::LoadingPlay { .. } | Self::LoadingPause { .. } => {
                *self = Self::Playing {
                    nominal_start_time: start,
                    preloading_of_next_track_triggered: false,
                };
            }
            Self::Paused {
                preloading_of_next_track_triggered,
                ..
            } if playing_event => {
                *self = Self::Playing {
                    nominal_start_time: start,
                    preloading_of_next_track_triggered: *preloading_of_next_track_triggered,
                };
            }
            _ => return false,
        }
        true
    }
}
#[cfg(test)]
mod local_transport_tests {
    use super::*;
    #[test]
    fn local_resume_publishes_playing_after_pause() {
        let mut status = SpircPlayStatus::Paused {
            position_ms: 1200,
            preloading_of_next_track_triggered: false,
        };
        assert!(status.player_started(1200, 5000, true));
        assert!(matches!(
            status,
            SpircPlayStatus::Playing {
                nominal_start_time: 3800,
                ..
            }
        ));
    }
    #[test]
    fn stopped_sessions_ignore_late_audio_starts() {
        let mut status = SpircPlayStatus::Stopped;
        assert!(!status.player_started(0, 5000, true));
    }
    #[test]
    fn clock_correction_does_not_unpause_playback() {
        let mut status = SpircPlayStatus::Paused {
            position_ms: 1200,
            preloading_of_next_track_triggered: false,
        };
        assert!(!status.player_started(1300, 5000, false));
        assert!(matches!(status, SpircPlayStatus::Paused { .. }));
    }
}
