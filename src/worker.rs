use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool, mpsc},
    thread,
    time::{Duration, Instant},
};

use crate::notify::Notifier;
use anyhow::{Context, Result};

use crate::{
    api::PlayerAction,
    auth::{self, Tokens},
    lyrics::{self, Lyrics},
    model::{BrowseInfo, Device, Playback, PlayingDetails, Playlist, Profile, Track},
    native::{NativeControls, NativeMessage, NativePlayer},
    spotify::Spotify,
};

pub enum Command {
    LocalSearch {
        query: String,
        limit: usize,
        reply: mpsc::Sender<std::result::Result<Vec<Track>, String>>,
    },
    LocalTrack {
        uri: String,
        reply: mpsc::Sender<std::result::Result<Vec<Track>, String>>,
    },
    PrefetchPage(String),
    Restore(String),
    Login(String, Arc<AtomicBool>),
    LoginResult {
        client_id: String,
        cancelled: Arc<AtomicBool>,
        result: std::result::Result<Box<Tokens>, String>,
    },
    NativeLogin {
        restoring: bool,
        cancelled: Arc<AtomicBool>,
    },
    NativeLoginResult {
        cancelled: Arc<AtomicBool>,
        result:
            std::result::Result<Option<Box<crate::native_login::Authenticated>>, auth::LoginIssue>,
    },
    Logout,
    Refresh,
    Library,
    Tracks {
        key: String,
        offset: usize,
        generation: u64,
    },
    Player {
        action: PlayerAction,
        device: Option<String>,
    },
    Add {
        playlist: Playlist,
        tracks: Vec<Track>,
    },
    /// Create a private playlist holding these songs.
    CreatePlaylist {
        name: String,
        tracks: Vec<Track>,
    },
    /// Remove the song at `index` from an editable playlist.
    RemoveFromPlaylist {
        playlist: Playlist,
        index: usize,
        uri: String,
    },
    /// Create an empty Spotify folder at the top of the library.
    CreateFolder(String),
    /// Move a library playlist into a Spotify folder, or out with `None`.
    MoveToFolder {
        playlist: String,
        folder: Option<String>,
    },
    /// Rename a Spotify folder.
    RenameFolder {
        id: String,
        name: String,
    },
    /// Delete a Spotify folder while keeping its playlists and subfolders.
    DeleteFolder(String),
    /// Rename a playlist you own, set its description and optionally
    /// replace its cover with a JPEG.
    EditPlaylist {
        playlist: Playlist,
        name: String,
        description: String,
        cover: Option<Vec<u8>>,
    },
    /// Move the song at `from` in an editable playlist to position `to`.
    MovePlaylistSong {
        playlist: Playlist,
        from: usize,
        uri: String,
        to: usize,
    },
    /// Delete an owned playlist or unfollow another, as Spotify does.
    RemovePlaylist {
        id: String,
        name: String,
    },
    SaveTrack(String),
    /// Look up saved status for these items and report it.
    SavedStatus(Vec<String>),
    FetchLyrics(Track),
    RetryAudio,
    EnsureAudio,
    BrowseInfo {
        key: String,
        generation: u64,
    },
    PlayingDetails(Track),
    /// Resolve Canvas videos ahead of playback (hovered rows, upcoming queue).
    PrefetchCanvas(Vec<String>),
    ArtistTracks {
        account: String,
        generation: u64,
        info: Box<BrowseInfo>,
    },
    TrackInfo(String),
    SetSaved {
        uri: String,
        saved: bool,
    },
    Shutdown,
}

pub enum Event {
    SearchResults {
        account: String,
        key: String,
        generation: u64,
        results: Box<crate::search::SearchResults>,
        message: String,
    },
    TrackFacts(std::collections::HashMap<String, crate::track_facts::Facts>),
    LastPlayed {
        account: String,
        value: Box<crate::resume::LastPlayed>,
    },
    PagePrefetched {
        account: String,
        key: String,
        tracks: Vec<Track>,
        info: Option<Box<BrowseInfo>>,
    },
    Canvas {
        uri: String,
        url: Option<String>,
    },
    CanvasPrefetched {
        uri: String,
        url: String,
    },
    ArtistStats {
        uri: String,
        artist_id: String,
        listeners: Option<u64>,
        /// Header photo or avatar, used when the artist has no other picture.
        image: Option<String>,
        /// The header photo alone.
        header: Option<String>,
        /// The About section's first photo.
        gallery: Option<String>,
        verified: Option<bool>,
        biography: String,
    },
    TrackInfo {
        uri: String,
        track: Box<Track>,
    },
    Songwriters {
        uri: String,
        names: Vec<String>,
    },
    BrowseInfo {
        generation: u64,
        info: Box<BrowseInfo>,
    },
    PlayingDetails {
        uri: String,
        details: Box<PlayingDetails>,
    },
    Saved {
        uri: String,
        saved: bool,
    },
    /// Songs on this page changed; reload it if it is open.
    PageChanged(String),
    /// Saved status for many items at once.
    SavedMany(Vec<(String, bool)>),
    Session(Profile),
    /// Spotify's current profile, fetched after a restored session unlocked.
    ProfileRefreshed(Profile),
    SignedOut,
    LoginFinished,
    LoginFailed(auth::LoginIssue),
    NativeStarting(u64),
    Library(Vec<Playlist>),
    /// Spotify folders in the library, empty ones included.
    LibraryFolders(Vec<crate::model::LibraryFolder>),
    Tracks {
        key: String,
        offset: usize,
        generation: u64,
        tracks: Vec<Track>,
        /// More songs follow this batch.
        more: bool,
    },
    TracksFailed {
        generation: u64,
        message: String,
    },
    Playback(Box<Playback>, Instant),
    /// Spotify Soloist's own report, sent as it changes.
    SoloistPlayback(Box<Playback>),
    SoloistQueueChanged,
    Devices(Vec<Device>),
    Notice(String),
    Error(String),
}

pub struct Worker {
    pub native_controls: NativeControls,
    pub commands: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
    pub events_in: mpsc::Sender<Event>,
    pub native_events: mpsc::Receiver<NativeMessage>,
    pub lyrics: mpsc::Sender<LyricCommand>,
    pub lyric_events: mpsc::Receiver<LyricEvent>,
}

pub enum LyricCommand {
    ImportText {
        track: Track,
        filename: String,
        contents: String,
        mode: lyrics::ImportMode,
    },
    /// Send lyrics already on disk for the track, if any, before a fetch.
    Cached(Track),
    /// Fetch an upcoming song's lyrics into the cache without showing them.
    Prefetch(Track, String),
    Fetch(Track, String),
    Import(Track, PathBuf),
    /// Drop lyrics imported for a track; a following fetch uses the provider.
    Forget(Track),
    /// Drop a track's saved source answers; a following fetch asks again.
    ForgetCached(Track),
    /// Delete every saved source answer.
    ClearCache,
    Configure {
        host: String,
        consent: bool,
    },
    ConfigureSpicy {
        key: String,
        consent: bool,
    },
}

pub struct LyricEvent {
    pub uri: String,
    pub result: Result<Lyrics, String>,
}

impl Worker {
    pub fn start(context: Notifier) -> Self {
        let native_controls = NativeControls::default();
        let backend_controls = native_controls.clone();
        let (commands, receive_commands) = mpsc::channel();
        let login_results = commands.clone();
        let (send_events, events) = mpsc::channel();
        let events_in = send_events.clone();
        let (send_native, native_events) = mpsc::channel();
        let (lyrics, receive_lyrics) = mpsc::channel();
        let (send_lyrics, lyric_events) = mpsc::channel();
        let (facts, fact_requests) = mpsc::sync_channel::<FactsRequest>(32);
        let fact_events = send_events.clone();
        let fact_context = context.clone();
        thread::spawn(move || {
            let mut catalogue = crate::track_facts::Catalogue::default();
            while let Ok(mut request) = fact_requests.recv() {
                let values =
                    catalogue.enrich(&request.native, &mut request.spotify, request.tracks);
                let _ = fact_events.send(Event::TrackFacts(values));
                fact_context.request_repaint();
            }
        });
        let (prefetch, requests) = mpsc::sync_channel::<PrefetchRequest>(1);
        let prefetch_events = send_events.clone();
        let prefetch_context = context.clone();
        thread::spawn(move || {
            while let Ok(mut request) = requests.recv() {
                let mut info = request
                    .key
                    .split_once(':')
                    .filter(|(kind, _)| matches!(*kind, "album" | "artist"))
                    .and_then(|(kind, id)| {
                        request
                            .native
                            .catalogue(kind, id, 0)
                            .ok()
                            .and_then(|entity| {
                                let mut info =
                                    crate::catalogue::base_info(&entity, kind, id).ok()?;
                                if kind == "artist" {
                                    info.overview = crate::artist::from_entity(&entity, id).ok();
                                } else {
                                    info.album_details = Some(crate::album::parse(&entity));
                                }
                                Some(info)
                            })
                    })
                    .or_else(|| crate::library_cache::info(&request.account, &request.key))
                    .or_else(|| {
                        if request.key.starts_with("artist:") || request.key.starts_with("album:") {
                            request.spotify.browse_info(&request.key).ok()
                        } else {
                            None
                        }
                    });
                if let Some(info) = info.as_mut() {
                    if let Some(id) = request.key.strip_prefix("artist:")
                        && info.overview.is_none()
                    {
                        info.overview = request
                            .native
                            .catalogue("artist", id, 0)
                            .and_then(|entity| crate::artist::from_entity(&entity, id))
                            .or_else(|_| crate::artist::fetch(&request.spotify.client, id))
                            .ok();
                    }
                    if let Some(id) = request.key.strip_prefix("album:")
                        && info.album_details.is_none()
                    {
                        info.album_details = request
                            .native
                            .catalogue("album", id, 0)
                            .map(|entity| crate::album::parse(&entity))
                            .or_else(|_| crate::album::fetch(&request.spotify.client, id))
                            .ok();
                    }
                }
                info = info.map(|info| crate::library_cache::save_info(&request.account, info));
                if let Ok(tracks) =
                    fetch_tracks(&mut request.spotify, &request.native, &request.key, 0)
                {
                    let _ = prefetch_events.send(Event::PagePrefetched {
                        account: request.account,
                        key: request.key,
                        tracks,
                        info: info.map(Box::new),
                    });
                    prefetch_context.request_repaint();
                }
            }
        });
        let lyric_context = context.clone();
        let backend_lyrics = lyrics.clone();
        thread::spawn(move || {
            let client = match auth::http_client() {
                Ok(client) => client,
                Err(error) => {
                    let _ = send_events.send(Event::Error(error.to_string()));
                    context.request_repaint();
                    return;
                }
            };
            let mut backend = Backend {
                facts,
                prefetch,
                spotify: None,
                native: None,
                native_controls: backend_controls,
                native_generation: 0,
                library_refresh: LibraryRefresh::default(),
                client_id: String::new(),
                profile: Profile::default(),
                client,
                events: send_events,
                native_events: send_native,
                lyrics: backend_lyrics,
                context,
            };
            let mut next_poll = Instant::now() + Duration::from_secs(4);
            loop {
                match receive_commands.recv_timeout(Duration::from_millis(250)) {
                    Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Ok(command) => {
                        if let Command::NativeLogin {
                            restoring,
                            cancelled,
                        } = command
                        {
                            let client = backend.client.clone();
                            let reply = login_results.clone();
                            thread::spawn(move || {
                                let result = crate::native_login::authenticate(
                                    &client, restoring, &cancelled,
                                )
                                .map(|login| login.map(Box::new))
                                .map_err(|error| auth::LoginIssue::from_error(&error, restoring));
                                let _ =
                                    reply.send(Command::NativeLoginResult { cancelled, result });
                            });
                            continue;
                        }
                        if let Command::Login(client_id, cancelled) = command {
                            let client = backend.client.clone();
                            let reply = login_results.clone();
                            thread::spawn(move || {
                                let result = auth::login(&client, &client_id, &cancelled)
                                    .map(Box::new)
                                    .map_err(|error| error.to_string());
                                let _ = reply.send(Command::LoginResult {
                                    client_id,
                                    cancelled,
                                    result,
                                });
                            });
                            continue;
                        }
                        if let Command::LoginResult { ref cancelled, .. }
                        | Command::NativeLoginResult { ref cancelled, .. } = command
                            && cancelled.load(std::sync::atomic::Ordering::Relaxed)
                        {
                            continue;
                        }
                        let restoring = matches!(command, Command::Restore(..));
                        let login = matches!(
                            command,
                            Command::LoginResult { .. }
                                | Command::NativeLoginResult { .. }
                                | Command::Restore(..)
                        );
                        match backend.handle(command) {
                            Err(error) if login => {
                                backend.emit(Event::LoginFailed(auth::LoginIssue::from_error(
                                    &error, restoring,
                                )));
                            }
                            Err(error) => backend.emit(Event::Error(error.to_string())),
                            Ok(()) if login => backend.emit(Event::LoginFinished),
                            Ok(()) => {}
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                if backend
                    .spotify
                    .as_ref()
                    .is_some_and(|api| !api.cooling_down())
                    && backend.library_refresh.due(Instant::now())
                    && let Err(error) = backend.refresh_library()
                {
                    backend.emit(Event::Error(error.to_string()));
                }
                if backend
                    .spotify
                    .as_ref()
                    .is_some_and(|api| !api.cooling_down())
                    && Instant::now() >= next_poll
                {
                    if let Err(error) = backend.poll() {
                        backend.emit(Event::Error(error.to_string()));
                    }
                    next_poll = Instant::now()
                        + Duration::from_secs(
                            if backend.spotify.as_ref().is_some_and(Spotify::is_native) {
                                2
                            } else if backend.native.is_some() {
                                60
                            } else {
                                12
                            },
                        );
                }
            }
        });
        // Spotify's own lyrics come through the native session.
        let lyric_native = native_controls.clone();
        thread::spawn(move || {
            let Ok(client) = auth::http_client() else {
                return;
            };
            // The song whose saved lyrics are showing: a failed refresh
            // must not replace them with an error.
            let mut showing_cached = None;
            // Commands read ahead while checking for a newer song.
            let mut pending = std::collections::VecDeque::new();
            while let Some(command) = pending.pop_front().or_else(|| receive_lyrics.recv().ok()) {
                let (track, result) = match command {
                    LyricCommand::Cached(track) => {
                        showing_cached = None;
                        let Some(found) = lyrics::cached(&client, &track, &lyric_native) else {
                            continue;
                        };
                        showing_cached = Some(track.uri.clone());
                        (track, Ok(found))
                    }
                    LyricCommand::ImportText {
                        track,
                        filename,
                        contents,
                        mode,
                    } => {
                        let result = lyrics::import_with_mode(&filename, &contents, &track, mode);
                        (track, result)
                    }
                    // Its own thread: an upcoming song never delays the
                    // current one's lyrics.
                    LyricCommand::Prefetch(track, token) => {
                        let (client, native) = (client.clone(), lyric_native.clone());
                        thread::spawn(move || {
                            let _ = lyrics::fetch(&client, &track, &token, &native);
                        });
                        continue;
                    }
                    LyricCommand::Fetch(track, token) => {
                        // Skipping through songs queues a fetch for each; only
                        // the newest song's lyrics are worth waiting for.
                        pending.extend(receive_lyrics.try_iter());
                        if pending
                            .iter()
                            .any(|command| matches!(command, LyricCommand::Fetch(..)))
                        {
                            continue;
                        }
                        let result = lyrics::fetch(&client, &track, &token, &lyric_native);
                        if result.is_err() && showing_cached.as_deref() == Some(&track.uri) {
                            continue;
                        }
                        (track, result)
                    }
                    LyricCommand::Import(track, path) => {
                        let result = lyrics::import_file(&path, &track);
                        (track, result)
                    }
                    LyricCommand::Forget(track) => {
                        let _ = lyrics::forget_local(&track);
                        continue;
                    }
                    LyricCommand::ForgetCached(track) => {
                        let _ = lyrics::forget_cached(&track);
                        continue;
                    }
                    LyricCommand::ClearCache => {
                        let _ = lyrics::clear_cache();
                        continue;
                    }
                    LyricCommand::Configure { host, consent } => {
                        let result = crate::provider::configure(&host, consent)
                            .map(|_| Lyrics::default())
                            .map_err(|error| error.to_string());
                        let _ = send_lyrics.send(LyricEvent {
                            uri: String::new(),
                            result,
                        });
                        lyric_context.request_repaint();
                        continue;
                    }
                    LyricCommand::ConfigureSpicy { key, consent } => {
                        let result = crate::provider::configure_spicy(&key, consent)
                            .map(|_| Lyrics::default())
                            .map_err(|error| error.to_string());
                        let _ = send_lyrics.send(LyricEvent {
                            uri: String::new(),
                            result,
                        });
                        lyric_context.request_repaint();
                        continue;
                    }
                };
                let _ = send_lyrics.send(LyricEvent {
                    uri: track.uri,
                    result: result.map_err(|error| error.to_string()),
                });
                lyric_context.request_repaint();
            }
        });
        Self {
            native_controls,
            commands,
            events,
            events_in,
            native_events,
            lyrics,
            lyric_events,
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
    }
}

struct Backend {
    library_refresh: LibraryRefresh,
    facts: mpsc::SyncSender<FactsRequest>,
    prefetch: mpsc::SyncSender<PrefetchRequest>,
    native_controls: NativeControls,
    spotify: Option<Spotify>,
    native: Option<NativePlayer>,
    native_generation: u64,
    client_id: String,
    profile: Profile,
    client: reqwest::blocking::Client,
    events: mpsc::Sender<Event>,
    native_events: mpsc::Sender<NativeMessage>,
    lyrics: mpsc::Sender<LyricCommand>,
    context: Notifier,
}

#[derive(Default)]
struct LibraryRefresh {
    next: Option<Instant>,
}
impl LibraryRefresh {
    fn due(&self, now: Instant) -> bool {
        self.next.is_some_and(|next| now >= next)
    }
    fn fetch<T>(&mut self, now: Instant, fetch: impl FnOnce() -> Result<T>) -> Result<T> {
        let result = fetch();
        self.next = result.as_ref().err().map(|_| now + Duration::from_secs(30));
        result
    }
}

/// Owner names the native lookup missed and generated mosaics the rootlist
/// omits come from the last full library read.
fn fill_library_details(
    mut playlists: Vec<Playlist>,
    previous: Option<&[Playlist]>,
    profile: &Profile,
) -> Vec<Playlist> {
    let previous: HashMap<&str, &Playlist> = previous
        .unwrap_or_default()
        .iter()
        .map(|playlist| (playlist.id.as_str(), playlist))
        .collect();
    for playlist in &mut playlists {
        let earlier = previous.get(playlist.id.as_str());
        if playlist.images.is_empty()
            && let Some(earlier) = earlier
        {
            playlist.images = earlier.images.clone();
        }
        if playlist.owner.id == profile.id {
            playlist.owner.display_name = profile.display_name.clone();
        } else if playlist.owner.display_name.is_none()
            && let Some(earlier) = earlier.filter(|earlier| earlier.owner.id == playlist.owner.id)
        {
            playlist.owner.display_name = earlier.owner.display_name.clone();
        }
    }
    playlists
}

fn profile_for_session(
    remote: Result<Profile>,
    cached: Option<Profile>,
    restoring: bool,
) -> Result<Profile> {
    match remote {
        Ok(profile) => Ok(profile),
        Err(error) => {
            if restoring
                && error
                    .downcast_ref::<crate::api_backoff::CoolingDown>()
                    .is_some()
                && let Some(profile) = cached
            {
                return Ok(profile);
            }
            Err(error)
        }
    }
}
impl Drop for Backend {
    fn drop(&mut self) {
        if let Some(spotify) = &self.spotify {
            spotify.cancel_background();
        }
    }
}
impl Backend {
    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
        self.context.request_repaint();
    }

    fn establish(&mut self, tokens: Tokens, client_id: String, restoring: bool) -> Result<()> {
        // Audio login is slow and needs only the account, so start it from the
        // stored profile while the Web API session is still being verified.
        let early_audio = restoring
            .then(|| tokens.profile.clone())
            .flatten()
            .map(|profile| {
                self.profile = profile;
                let _ = self.start_audio();
                self.profile.id.clone()
            });
        let result = self.establish_session(tokens, client_id, restoring, early_audio.as_deref());
        if result.is_err() && early_audio.is_some() {
            self.native.take();
        }
        result
    }

    fn establish_session(
        &mut self,
        mut tokens: Tokens,
        client_id: String,
        restoring: bool,
        early_audio: Option<&str>,
    ) -> Result<()> {
        tokens.refresh_if_needed(&self.client, &client_id)?;
        let cached = tokens.profile.clone();
        let mut spotify = Spotify::new(self.client.clone(), client_id.clone(), tokens);
        self.profile = profile_for_session(spotify.profile(), cached, restoring)?;
        spotify.save_profile(self.profile.clone())?;
        self.client_id = client_id;
        if let Some(previous) = self.spotify.as_ref() {
            previous.cancel_background();
        }
        self.spotify = Some(spotify);
        self.emit(Event::Session(self.profile.clone()));
        if let Some(playlists) = crate::library_cache::load(&self.profile.id) {
            self.emit(Event::Library(playlists));
        }
        if early_audio != Some(self.profile.id.as_str())
            && let Err(error) = self.start_audio()
        {
            self.emit(Event::Error(error.to_string()));
        }
        if let Err(error) = self.refresh_library() {
            self.emit(Event::Error(error.to_string()));
        }
        if let Err(error) = self.poll() {
            self.emit(Event::Error(error.to_string()));
        }
        if let Ok(Some(value)) = self.api()?.last_played() {
            self.emit(Event::LastPlayed {
                account: self.profile.id.clone(),
                value: Box::new(value),
            });
        }
        Ok(())
    }

    fn establish_native(&mut self, mut login: crate::native_login::Authenticated) -> Result<()> {
        let profile = login.profile.clone();
        if let Some(refresh) = login.profile_refresh.take() {
            let events = self.events.clone();
            let context = self.context.clone();
            thread::spawn(move || {
                let refresh = refresh.into_inner();
                if let Ok(profile) = refresh.map_err(|_| ()).and_then(|refresh| {
                    refresh
                        .recv_timeout(Duration::from_secs(15))
                        .map_err(|_| ())
                }) {
                    let _ = events.send(Event::ProfileRefreshed(profile));
                    context.request_repaint();
                }
            });
        }
        if let Some(previous) = self.spotify.take() {
            previous.cancel_background();
        }
        // Retain only the optional app identifier so sign-out can remove its old token.
        self.client_id = crate::config::Settings::load()
            .map(|settings| settings.client_id)
            .unwrap_or_default();
        self.profile = profile;
        self.start_audio_with_login(Some(login))?;
        self.spotify = Some(Spotify::native(
            self.client.clone(),
            self.native_controls.clone(),
            self.profile.clone(),
        )?);
        self.emit(Event::Session(self.profile.clone()));
        if let Some(playlists) = crate::library_cache::load(&self.profile.id) {
            self.emit(Event::Library(playlists));
        }
        self.library_refresh.next = Some(Instant::now() + Duration::from_secs(2));
        Ok(())
    }

    fn scoped_native(&self) -> NativeControls {
        self.native_controls
            .scoped(&self.profile.id)
            .unwrap_or_default()
    }

    fn api(&mut self) -> Result<&mut Spotify> {
        self.spotify.as_mut().context("Sign in to Spotify first")
    }

    fn refresh_library(&mut self) -> Result<()> {
        // Spotify's own rootlist has no 50-playlist cap and skips the Web API.
        if matches!(
            self.native_controls
                .connection_event(Some(&self.profile.id)),
            Some(crate::native::NativeEvent::Ready(_))
        ) && let Ok((playlists, folders)) = self.native_controls.library()
        {
            self.emit(Event::LibraryFolders(folders));
            let playlists = fill_library_details(
                playlists,
                crate::library_cache::load(&self.profile.id).as_deref(),
                &self.profile,
            );
            let restore_history = self.library_refresh.next.is_some()
                && self.spotify.as_ref().is_some_and(Spotify::is_native);
            self.library_refresh.next = None;
            crate::library_cache::save(&self.profile.id, &playlists);
            self.emit(Event::Library(playlists));
            if restore_history
                && !self.native_controls.has_track()
                && let Ok(Some(value)) = self.api()?.last_played()
            {
                self.emit(Event::LastPlayed {
                    account: self.profile.id.clone(),
                    value: Box::new(value),
                });
            }
            return Ok(());
        }
        let api = self.spotify.as_mut().context("Sign in to Spotify first")?;
        if api.cooling_down()
            && let Some(playlists) = crate::library_cache::load(&self.profile.id)
        {
            self.library_refresh.next = Some(Instant::now() + Duration::from_secs(30));
            self.emit(Event::Library(playlists));
            return Ok(());
        }
        let playlists = self
            .library_refresh
            .fetch(Instant::now(), || api.playlists())?;
        crate::library_cache::save(&self.profile.id, &playlists);
        self.emit(Event::Library(playlists));
        Ok(())
    }

    fn start_audio(&mut self) -> Result<()> {
        self.start_audio_with_login(None)
    }

    fn start_audio_with_login(
        &mut self,
        login: Option<crate::native_login::Authenticated>,
    ) -> Result<()> {
        self.native.take();
        self.native_generation = self.native_generation.wrapping_add(1);
        self.emit(Event::NativeStarting(self.native_generation));
        self.native = Some(match login {
            Some(login) => NativePlayer::start_authenticated(
                login,
                self.native_generation,
                self.native_events.clone(),
                self.context.clone(),
                self.native_controls.clone(),
            ),
            None => NativePlayer::start(
                self.profile.id.clone(),
                self.native_generation,
                self.native_events.clone(),
                self.context.clone(),
                self.native_controls.clone(),
            ),
        });
        if self.spotify.as_ref().is_some_and(Spotify::is_native) {
            if let Some(previous) = self.spotify.take() {
                previous.cancel_background();
            }
            self.spotify = Some(Spotify::native(
                self.client.clone(),
                self.native_controls.clone(),
                self.profile.clone(),
            )?);
        }
        Ok(())
    }

    /// Send a player command, resolving the device through Spotify's device
    /// list. This computer's player can take a few seconds to appear there
    /// after it connects, so look again briefly before giving up.
    fn player(&mut self, action: PlayerAction, device: Option<String>) -> Result<()> {
        let own = crate::native::device_name();
        let requested = match &action {
            PlayerAction::Transfer(id) => Some(id.clone()),
            _ => device,
        };
        if let PlayerAction::Transfer(id) = &action
            && matches!(
                self.native_controls
                    .connection_event(Some(&self.profile.id)),
                Some(crate::native::NativeEvent::Ready(_))
            )
        {
            let transferred = self.native_controls.transfer(id);
            // The device picker follows the active device: after a failure
            // it must return to the current one, and after a success it must
            // not see the old one still active and send controls back.
            let devices = match transferred {
                Ok(()) => self
                    .native_controls
                    .devices_after_transfer(id, Duration::from_secs(8)),
                Err(_) => self.native_controls.devices(),
            };
            if let Ok(devices) = devices {
                let _ = self.events.send(Event::Devices(devices));
            }
            return transferred;
        }
        if action.supports_native()
            && let Some(crate::native::NativeEvent::Ready(id)) = self
                .native_controls
                .connection_event(Some(&self.profile.id))
            && requested
                .as_deref()
                .is_none_or(|requested| requested == id || requested == "native")
        {
            let queued = matches!(action, PlayerAction::Enqueue(_));
            return self.native_controls.send(action).map_err(|_| {
                anyhow::anyhow!(if queued {
                    "Play a song on this computer before adding to its queue."
                } else {
                    "Native audio disconnected. Press Play to reconnect."
                })
            });
        }
        for attempt in 0..4 {
            let devices = self.api()?.devices()?;
            let _ = self.events.send(Event::Devices(devices.clone()));
            if let Some(id) = crate::model::resolve_device(&devices, requested.as_deref(), &own) {
                return match action {
                    PlayerAction::Transfer(_) => {
                        self.api()?.player_action(PlayerAction::Transfer(id), None)
                    }
                    action => self.api()?.player_action(action, Some(&id)),
                };
            }
            if self.native.is_none() || attempt == 3 {
                break;
            }
            thread::sleep(Duration::from_millis(1200));
        }
        anyhow::bail!(if self.native.is_some() {
            "This computer is not ready for playback yet. Wait a moment, or reconnect native audio in Settings."
        } else {
            "No playback device. Reconnect native audio in Settings, or open Spotify on another device."
        })
    }

    fn poll(&mut self) -> Result<()> {
        if self.native_connecting()
            || self.native_controls.has_track()
            || self.api()?.cooling_down()
        {
            return Ok(());
        }
        let observed = Instant::now();
        let playback = self.api()?.playback()?;
        self.emit(Event::Playback(Box::new(playback), observed));
        Ok(())
    }

    fn handle(&mut self, command: Command) -> Result<()> {
        match command {
            Command::PrefetchPage(key) => {
                let request = PrefetchRequest {
                    spotify: self.api()?.background(),
                    native: self.scoped_native(),
                    account: self.profile.id.clone(),
                    key,
                };
                let _ = self.prefetch.try_send(request);
            }
            Command::NativeLogin { .. } => {
                unreachable!("Native login runs outside the command worker")
            }
            Command::NativeLoginResult { cancelled, result } => {
                if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    return Ok(());
                }
                match result {
                    Ok(Some(login)) => {
                        crate::native::save_login(&login.profile, &login.credentials, &cancelled)?;
                        self.establish_native(*login)?;
                    }
                    Ok(None) => self.emit(Event::SignedOut),
                    Err(issue) => self.emit(Event::LoginFailed(issue)),
                }
            }
            Command::Restore(client_id) => {
                self.client_id = client_id.clone();
                if let Some(tokens) = Tokens::load(&client_id)? {
                    self.establish(tokens, client_id, true)?;
                } else {
                    self.emit(Event::SignedOut);
                }
            }
            Command::Login(client_id, cancelled) => {
                let _ = (client_id, cancelled);
                unreachable!("Login must run outside the API worker");
            }
            Command::LoginResult {
                client_id,
                cancelled,
                result,
            } => {
                if !cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                    self.establish(*result.map_err(anyhow::Error::msg)?, client_id, false)?;
                }
            }
            Command::Logout => {
                self.library_refresh = LibraryRefresh::default();
                self.native.take();
                let audio_deleted = crate::native::forget_credentials(&self.profile.id);
                if let Some(previous) = self.spotify.take() {
                    previous.cancel_background();
                }
                let deleted = Tokens::delete(&self.client_id);
                self.emit(Event::SignedOut);
                audio_deleted?;
                deleted?;
            }
            Command::Refresh if self.native_connecting() => {}
            Command::Refresh => match self.native_controls.devices() {
                Ok(devices) => self.emit(Event::Devices(devices)),
                Err(_) if self.api()?.cooling_down() => {}
                Err(_) => {
                    self.poll()?;
                    let devices = self.api()?.devices()?;
                    self.emit(Event::Devices(devices));
                }
            },
            Command::TrackInfo(uri) => {
                if let Ok(track) = self
                    .native_controls
                    .track(&uri)
                    .or_else(|_| self.api()?.track(&uri))
                {
                    self.emit(Event::TrackInfo {
                        uri,
                        track: Box::new(track),
                    });
                }
            }
            Command::Library => {
                self.refresh_library()?;
            }
            Command::Tracks {
                key,
                offset,
                generation,
            } => {
                if offset == 0
                    && let Some(query) = key.strip_prefix("search:")
                {
                    let spotify = self.api()?;
                    // Refresh on the background worker before sharing a read-only snapshot.
                    let _ = spotify.refresh_credentials();
                    let mut spotify = spotify.background();
                    let native = self.scoped_native();
                    let events = self.events.clone();
                    let context = self.context.clone();
                    let account = self.profile.id.clone();
                    let query = query.to_owned();
                    let key = key.clone();
                    thread::spawn(move || {
                        let (results, message) = match native.search_catalogue(&query) {
                            Ok(results) => (results, String::new()),
                            Err(_) => match spotify.search_catalogue(&query) {
                                Ok(results) => (
                                    results,
                                    "Some Spotify-curated playlists may be unavailable.".into(),
                                ),
                                Err(_) => (
                                    crate::search::SearchResults::default(),
                                    "Could not load search results. Try searching again.".into(),
                                ),
                            },
                        };
                        let _ = events.send(Event::SearchResults {
                            account,
                            key,
                            generation,
                            results: Box::new(results),
                            message,
                        });
                        context.request_repaint();
                    });
                    return Ok(());
                }
                // Local Files are read from this computer, all at once.
                if key == "local" {
                    self.emit(Event::Tracks {
                        tracks: if offset == 0 {
                            crate::local_files::tracks()
                        } else {
                            Vec::new()
                        },
                        more: false,
                        key,
                        offset,
                        generation,
                    });
                    return Ok(());
                }
                // A playlist, Liked Songs or an album shows every song at once,
                // like Spotify: the whole list loads in one read on a
                // background thread, so it never arrives a page at a time.
                if loads_whole(&key) {
                    let spotify = self.api()?;
                    let _ = spotify.refresh_credentials();
                    let mut spotify = spotify.background();
                    let facts_spotify = spotify.background();
                    let native = self.scoped_native();
                    let native_ready = self.native_ready();
                    let events = self.events.clone();
                    let context = self.context.clone();
                    let facts = self.facts.clone();
                    thread::spawn(move || {
                        let mut tracks = Vec::new();
                        let mut more = true;
                        let mut failure = None;
                        // The copy saved last time shows at once; the fresh
                        // list below replaces it only if it changed.
                        let saved = (offset == 0).then(|| spotify.account()).flatten().and_then(
                            |account| {
                                crate::library_cache::whole(
                                    account,
                                    &key,
                                    crate::native_browse::READ_LIMIT - 1,
                                )
                            },
                        );
                        if let Some(saved) = &saved {
                            let _ = events.send(Event::Tracks {
                                key: key.clone(),
                                offset,
                                generation,
                                tracks: saved.clone(),
                                more: false,
                            });
                            context.request_repaint();
                        }
                        let mut whole = read_all(&native, &key, offset);
                        // A page restored at launch can ask before Spotify's
                        // connection is up; it is moments away, and far
                        // quicker than paging the Web API.
                        for _ in 0..40 {
                            let connecting = matches!(&whole, Some(Err(error))
                                if error.to_string().starts_with("Connect native audio"));
                            if !connecting {
                                break;
                            }
                            thread::sleep(Duration::from_millis(250));
                            whole = read_all(&native, &key, offset);
                        }
                        if let Some(Err(error)) = &whole {
                            crate::native::diagnostic(&format!(
                                "browse whole read failed, paging instead: {error:#}"
                            ));
                        }
                        let read_natively = match whole {
                            Some(Ok(batch)) => {
                                more = batch.len() == crate::native_browse::READ_LIMIT;
                                if offset == 0
                                    && !more
                                    && let Some(account) = spotify.account()
                                {
                                    crate::library_cache::save_whole(account, &key, &batch);
                                }
                                tracks = batch;
                                true
                            }
                            _ => false,
                        };
                        // Page by page when the native read is unavailable.
                        while !read_natively && tracks.len() < REST_BATCH {
                            match fetch_tracks(&mut spotify, &native, &key, offset + tracks.len()) {
                                Ok(page) => {
                                    let last = page.len() < 50;
                                    tracks.extend(page);
                                    if last {
                                        more = false;
                                        break;
                                    }
                                }
                                Err(error) => {
                                    failure = Some(error.to_string());
                                    break;
                                }
                            }
                        }
                        let uris: Vec<String> =
                            tracks.iter().map(|track| track.uri.clone()).collect();
                        let facts_tracks = if key.starts_with("album:") {
                            Vec::new()
                        } else {
                            tracks.clone()
                        };
                        if tracks.is_empty()
                            && let Some(message) = failure
                        {
                            let _ = events.send(Event::TracksFailed {
                                generation,
                                message,
                            });
                        } else if saved.as_ref().is_none_or(|saved| {
                            more || saved.len() != tracks.len()
                                || saved.iter().zip(&tracks).any(|(old, new)| {
                                    old.uri != new.uri || old.added_at != new.added_at
                                })
                        }) {
                            let _ = events.send(Event::Tracks {
                                key,
                                offset,
                                generation,
                                tracks,
                                more,
                            });
                        }
                        context.request_repaint();
                        // Facts for the whole batch, in chunks so filters and
                        // sorts fill in steadily. This thread can wait for the
                        // facts worker, so none are dropped.
                        for chunk in facts_tracks.chunks(200) {
                            let _ = facts.send(FactsRequest {
                                spotify: facts_spotify.background(),
                                native: native.clone(),
                                tracks: chunk.to_vec(),
                            });
                        }
                        if native_ready {
                            let mut seen = std::collections::HashSet::new();
                            let uris: Vec<String> = uris
                                .into_iter()
                                .filter(|uri| {
                                    crate::collection::supports(uri) && seen.insert(uri.clone())
                                })
                                .collect();
                            for chunk in uris.chunks(500) {
                                let Ok(saved) = native.saved_items(chunk) else {
                                    break;
                                };
                                let _ = events.send(Event::SavedMany(
                                    chunk.iter().cloned().zip(saved).collect(),
                                ));
                            }
                            context.request_repaint();
                        }
                    });
                    return Ok(());
                }
                let result = self.fetch_tracks(&key, offset);
                match result {
                    Ok(tracks) => {
                        self.publish_saved(tracks.iter().map(|track| track.uri.clone()).collect());
                        if (key == "liked"
                            || key.starts_with("liked-artist:")
                            || key.starts_with("playlist:"))
                            && let Some(spotify) = self.spotify.as_ref()
                        {
                            let _ = self.facts.try_send(FactsRequest {
                                spotify: spotify.background(),
                                native: self.scoped_native(),
                                tracks: tracks.clone(),
                            });
                        }
                        self.emit(Event::Tracks {
                            more: tracks.len() == 50,
                            key,
                            offset,
                            generation,
                            tracks,
                        });
                    }
                    Err(error) => self.emit(Event::TracksFailed {
                        generation,
                        message: error.to_string(),
                    }),
                }
            }
            Command::LocalSearch {
                query,
                limit,
                reply,
            } => {
                let result = self
                    .api()
                    .and_then(|api| api.search(&query))
                    .map(|tracks| tracks.into_iter().take(limit).collect())
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
            }
            Command::LocalTrack { uri, reply } => {
                let result = self
                    .api()
                    .and_then(|api| api.track(&uri))
                    .map(|track| vec![track])
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
            }
            Command::Player { action, device } => {
                let queued = matches!(action, PlayerAction::Enqueue(_));
                self.player(action, device)?;
                if queued {
                    self.emit(Event::Notice("Added to queue".into()));
                }
                let _ = self.poll();
            }
            Command::Add { playlist, tracks } => {
                anyhow::ensure!(
                    playlist.editable_by(&self.profile.id),
                    "This playlist cannot be edited by your account"
                );
                let native = if self.native_ready() {
                    self.native_controls.add_to_playlist(&playlist.id, &tracks)
                } else {
                    Err(anyhow::anyhow!("Native audio is connecting"))
                };
                let count = match native {
                    Ok(crate::native_browse::Added::Count(count)) => count,
                    // Nothing was written natively, so the Web API can retry.
                    Ok(crate::native_browse::Added::NotWritten(_)) | Err(_) => {
                        self.api()?.add_to_playlist(&playlist.id, &tracks)?
                    }
                };
                self.emit(Event::Notice(format!(
                    "Added {count} tracks to {}. Existing tracks skipped.",
                    playlist.name
                )));
                if count > 0 {
                    self.emit(Event::PageChanged(format!("playlist:{}", playlist.id)));
                }
            }
            Command::RemoveFromPlaylist {
                playlist,
                index,
                uri,
            } => {
                anyhow::ensure!(
                    playlist.editable_by(&self.profile.id),
                    "This playlist cannot be edited by your account"
                );
                anyhow::ensure!(
                    self.native_ready(),
                    "Wait for native audio to connect, then remove the song."
                );
                let removed = self
                    .native_controls
                    .remove_from_playlist(&playlist.id, index, &uri);
                if removed.is_err() {
                    // The window already hid the song; reload to restore it.
                    self.emit(Event::PageChanged(format!("playlist:{}", playlist.id)));
                }
                removed?;
                self.emit(Event::Notice(format!("Removed from {}", playlist.name)));
            }
            Command::CreateFolder(name) => {
                anyhow::ensure!(
                    self.native_ready(),
                    "Wait for native audio to connect, then create the folder."
                );
                self.native_controls.create_folder(&name)?;
                self.refresh_library()?;
                self.emit(Event::Notice(format!("Created {}", name.trim())));
            }
            Command::MoveToFolder { playlist, folder } => {
                anyhow::ensure!(
                    self.native_ready(),
                    "Wait for native audio to connect, then move the playlist."
                );
                self.native_controls
                    .move_to_folder(&playlist, folder.as_deref())?;
                self.refresh_library()?;
                self.emit(Event::Notice(
                    if folder.is_some() {
                        "Moved to folder"
                    } else {
                        "Moved out of folder"
                    }
                    .into(),
                ));
            }
            Command::RenameFolder { id, name } => {
                anyhow::ensure!(
                    self.native_ready(),
                    "Wait for native audio to connect, then rename the folder."
                );
                self.native_controls.rename_folder(&id, &name)?;
                self.refresh_library()?;
                self.emit(Event::Notice(format!("Renamed to {}", name.trim())));
            }
            Command::DeleteFolder(id) => {
                anyhow::ensure!(
                    self.native_ready(),
                    "Wait for native audio to connect, then delete the folder."
                );
                self.native_controls.delete_folder(&id)?;
                self.refresh_library()?;
                self.emit(Event::Notice(
                    "Folder deleted. Playlists kept in Your Library.".into(),
                ));
            }
            Command::EditPlaylist {
                playlist,
                name,
                description,
                cover,
            } => {
                anyhow::ensure!(
                    playlist.owner.id == self.profile.id,
                    "Only the owner can edit this playlist's details"
                );
                anyhow::ensure!(
                    self.native_ready(),
                    "Wait for native audio to connect, then edit the playlist."
                );
                if let Some(cover) = cover {
                    self.native_controls
                        .set_playlist_cover(&playlist.id, &cover)?;
                }
                self.native_controls
                    .edit_playlist(&playlist.id, &name, &description)?;
                self.refresh_library()?;
                self.emit(Event::PageChanged(format!("playlist:{}", playlist.id)));
                self.emit(Event::Notice(format!("Saved {}", name.trim())));
            }
            Command::MovePlaylistSong {
                playlist,
                from,
                uri,
                to,
            } => {
                anyhow::ensure!(
                    playlist.editable_by(&self.profile.id),
                    "This playlist cannot be edited by your account"
                );
                anyhow::ensure!(
                    self.native_ready(),
                    "Wait for native audio to connect, then move the song."
                );
                let moved = self
                    .native_controls
                    .move_playlist_song(&playlist.id, from, &uri, to);
                if moved.is_err() {
                    // The window already moved the row; reload to restore it.
                    self.emit(Event::PageChanged(format!("playlist:{}", playlist.id)));
                }
                moved?;
            }
            Command::CreatePlaylist { name, tracks } => {
                anyhow::ensure!(
                    self.native_ready(),
                    "Wait for native audio to connect, then create the playlist."
                );
                let id = self.native_controls.create_playlist(&name)?;
                let added = match self.native_controls.add_to_playlist(&id, &tracks) {
                    Ok(crate::native_browse::Added::Count(count)) => count,
                    Ok(crate::native_browse::Added::NotWritten(error)) | Err(error) => {
                        self.refresh_library()?;
                        return Err(
                            error.context(format!("Created {name} but could not add songs"))
                        );
                    }
                };
                self.refresh_library()?;
                self.emit(Event::Notice(if added == 0 {
                    format!("Created {name}")
                } else {
                    format!("Added to {name}")
                }));
            }
            Command::RemovePlaylist { id, name } => {
                anyhow::ensure!(
                    self.native_ready(),
                    "Wait for native audio to connect, then remove the playlist."
                );
                self.native_controls.remove_playlist(&id)?;
                self.refresh_library()?;
                self.emit(Event::Notice(format!("Removed {name} from Your Library")));
            }
            Command::SavedStatus(uris) => self.publish_saved(uris),
            Command::SaveTrack(uri) => {
                if !self.native_ready() || self.native_controls.set_saved(&uri, true).is_err() {
                    self.api()?.save_track(&uri)?;
                }
                self.emit(Event::Saved { uri, saved: true });
                self.emit(Event::PageChanged("liked".into()));
                self.emit(Event::Notice("Saved to Liked Songs".into()));
            }
            Command::FetchLyrics(track) => {
                // Saved lyrics show before the token request and the sources.
                let _ = self.lyrics.send(LyricCommand::Cached(track.clone()));
                let api = self.api()?;
                let token = if crate::provider::requires_spotify_token() {
                    api.provider_token()?
                } else {
                    String::new()
                };
                let _ = self.lyrics.send(LyricCommand::Fetch(track, token));
            }
            Command::RetryAudio => self.start_audio()?,
            Command::EnsureAudio => {
                if !self
                    .native_controls
                    .connected_or_connecting(&self.profile.id)
                {
                    self.start_audio()?;
                }
            }
            Command::BrowseInfo { key, generation } => {
                let account = self.profile.id.clone();
                let mut info =
                    crate::library_cache::info(&account, &key).unwrap_or_else(|| BrowseInfo {
                        key: key.clone(),
                        ..BrowseInfo::default()
                    });
                self.emit(Event::BrowseInfo {
                    generation,
                    info: Box::new(info.clone()),
                });
                if let Some(id) = key.strip_prefix("artist:") {
                    let mut api = self.api()?.background();
                    let id = id.to_owned();
                    let native = self.scoped_native();
                    let client = self.client.clone();
                    let events = self.events.clone();
                    let context = self.context.clone();
                    thread::spawn(move || {
                        if let Ok(overview) = native
                            .catalogue("artist", &id, 0)
                            .and_then(|entity| {
                                if let Ok(base) =
                                    crate::catalogue::base_info(&entity, "artist", &id)
                                {
                                    info = base;
                                    publish_browse_info(
                                        &events,
                                        &context,
                                        &account,
                                        generation,
                                        info.clone(),
                                    );
                                }
                                crate::artist::from_entity(&entity, &id)
                            })
                            .or_else(|_| {
                                if info.title.is_empty()
                                    && let Ok(base) = api.browse_info(&info.key)
                                {
                                    info = base;
                                    publish_browse_info(
                                        &events,
                                        &context,
                                        &account,
                                        generation,
                                        info.clone(),
                                    );
                                }
                                crate::artist::fetch(&client, &id)
                            })
                        {
                            let mut info = info;
                            info.overview = Some(overview);
                            publish_browse_info(&events, &context, &account, generation, info);
                        }
                    });
                } else if let Some(id) = key.strip_prefix("playlist:") {
                    let id = id.to_owned();
                    let native = self.scoped_native();
                    let events = self.events.clone();
                    let context = self.context.clone();
                    thread::spawn(move || {
                        if let Ok(info) = native.playlist_info(&id) {
                            publish_browse_info(&events, &context, &account, generation, info);
                        }
                    });
                } else if let Some(id) = key.strip_prefix("album:") {
                    let mut api = self.api()?.background();
                    let id = id.to_owned();
                    let native = self.scoped_native();
                    let client = self.client.clone();
                    let events = self.events.clone();
                    let context = self.context.clone();
                    thread::spawn(move || {
                        let desktop = native.catalogue("album", &id, 0).map(|entity| {
                            if let Ok(base) = crate::catalogue::base_info(&entity, "album", &id) {
                                info = base;
                            }
                            let mut details = crate::album::parse(&entity);
                            let total = entity["tracksV2"]["totalCount"]
                                .as_u64()
                                .unwrap_or_default()
                                as usize;
                            let mut offset =
                                entity["tracksV2"]["items"].as_array().map_or(0, Vec::len);
                            let mut preview = info.clone();
                            preview.album_details = Some(details.clone());
                            publish_browse_info(&events, &context, &account, generation, preview);
                            while offset > 0 && offset < total && offset < 5000 {
                                let Ok(page) = native.catalogue("album", &id, offset) else {
                                    break;
                                };
                                let count =
                                    page["tracksV2"]["items"].as_array().map_or(0, Vec::len);
                                if count == 0 {
                                    break;
                                }
                                let next = crate::album::parse(&page);
                                let before = details.streams.len();
                                details.streams.extend(next.streams);
                                if before == details.streams.len() {
                                    break;
                                }
                                details.duration_ms += next.duration_ms;
                                offset += count;
                            }
                            details.complete_duration = offset >= total;
                            details
                        });
                        if let Ok(mut details) = desktop.or_else(|_| {
                            if info.title.is_empty()
                                && let Ok(base) = api.browse_info(&info.key)
                            {
                                info = base;
                                publish_browse_info(
                                    &events,
                                    &context,
                                    &account,
                                    generation,
                                    info.clone(),
                                );
                            }
                            crate::album::fetch(&client, &id)
                        }) {
                            let mut preview = info.clone();
                            preview.album_details = Some(details.clone());
                            publish_browse_info(&events, &context, &account, generation, preview);
                            if let Some(artist_id) = info
                                .album
                                .as_ref()
                                .and_then(|album| album.artists.first())
                                .and_then(|artist| artist.id.as_deref())
                                && let Ok(artist) = crate::artist::fetch(&client, artist_id)
                            {
                                for popular in artist.popular {
                                    if let Some(streams) = popular.streams {
                                        details.streams.entry(popular.track.uri).or_insert(streams);
                                    }
                                }
                            }
                            let mut info = info;
                            info.album_details = Some(details);
                            publish_browse_info(&events, &context, &account, generation, info);
                        }
                    });
                }
            }
            Command::ArtistTracks {
                account,
                generation,
                mut info,
            } => {
                if self.profile.id != account {
                    return Ok(());
                }
                if let Some(overview) = info.overview.as_mut() {
                    let mut statuses = Vec::new();
                    {
                        // The artist too, so Follow reads Following.
                        let uris: Vec<_> = overview
                            .popular
                            .iter()
                            .map(|item| item.track.uri.clone())
                            .chain(
                                info.artist
                                    .as_ref()
                                    .and_then(|artist| artist.id.as_ref())
                                    .map(|id| format!("spotify:artist:{id}")),
                            )
                            .collect();
                        let native = self.native_controls.saved_items(&uris);
                        if !uris.is_empty()
                            && let Ok(saved) = native.or_else(|_| self.api()?.saved_items(&uris))
                        {
                            statuses = uris.into_iter().zip(saved).collect();
                        }
                    }
                    for (uri, saved) in statuses {
                        self.emit(Event::Saved { uri, saved });
                    }
                    overview.details_loaded = true;
                }
                self.emit(Event::BrowseInfo { generation, info });
            }
            Command::PrefetchCanvas(uris) => {
                let native = self.scoped_native();
                let events = self.events.clone();
                thread::spawn(move || {
                    for uri in uris.into_iter().take(8) {
                        if let Some(url) = canvas_url(&native, &uri) {
                            let _ = events.send(Event::CanvasPrefetched { uri, url });
                        }
                    }
                });
            }
            Command::PlayingDetails(track) => {
                // The artist card's About details first, before the queue and
                // metadata calls below can hold them up.
                if let Some(artist_id) = track.artists.first().and_then(|artist| artist.id.clone())
                {
                    spawn_artist_about(
                        self.scoped_native(),
                        self.client.clone(),
                        self.events.clone(),
                        self.context.clone(),
                        track.uri.clone(),
                        artist_id,
                    );
                }
                self.publish_saved(vec![track.uri.clone()]);
                let canvas_track = track.uri.clone();
                let native = self.scoped_native();
                let events = self.events.clone();
                let context = self.context.clone();
                thread::spawn(move || {
                    let url = canvas_url(&native, &canvas_track);
                    let _ = events.send(Event::Canvas {
                        uri: canvas_track,
                        url,
                    });
                    context.request_repaint();
                });
                // Most metadata names no composer; Spotify's credits list does.
                if track.songwriters.is_empty()
                    && let Some(id) = track.uri.strip_prefix("spotify:track:")
                {
                    let id = id.to_owned();
                    let uri = track.uri.clone();
                    let native = self.scoped_native();
                    let events = self.events.clone();
                    thread::spawn(move || {
                        if let Ok(names) = native.songwriters(&id)
                            && !names.is_empty()
                        {
                            let _ = events.send(Event::Songwriters { uri, names });
                        }
                    });
                }
                let local = self.native_controls.has_track();
                let artists = if local {
                    track
                        .artists
                        .iter()
                        .map(|artist| {
                            artist
                                .id
                                .as_ref()
                                .and_then(|id| {
                                    crate::library_cache::info(
                                        &self.profile.id,
                                        &format!("artist:{id}"),
                                    )
                                })
                                .and_then(|info| info.artist)
                                .unwrap_or_else(|| artist.clone())
                        })
                        .collect()
                } else {
                    // Browsed artists come from the cache; only others ask
                    // the Web API.
                    let mut artists = Vec::new();
                    for artist in &track.artists {
                        let Some(id) = &artist.id else { continue };
                        if let Some(cached) =
                            crate::library_cache::info(&self.profile.id, &format!("artist:{id}"))
                                .and_then(|info| info.artist)
                        {
                            artists.push(cached);
                        } else if let Ok(artist) = self.api()?.artist(id) {
                            artists.push(artist);
                        }
                    }
                    artists
                };
                // Spotify's Web API does not say which songs were queued.
                let (queue, queued) = if let Some(soloist) = crate::soloist_link::queue() {
                    soloist
                } else if local {
                    self.native_controls.queue_details().unwrap_or_default()
                } else {
                    (self.api()?.queue().unwrap_or_default(), 0)
                };
                crate::native::diagnostic(&format!(
                    "details queue {} songs, {queued} queued",
                    queue.len()
                ));
                // Shown first: warming the next songs below waits on the network.
                let upcoming: Vec<Track> = queue.iter().take(3).cloned().collect();
                self.emit(Event::PlayingDetails {
                    uri: track.uri.clone(),
                    details: Box::new(PlayingDetails {
                        artists,
                        queue,
                        queued,
                    }),
                });
                // Warm the next songs' lyrics and artist cards so skipping
                // shows them at once, as Canvas below.
                let next: Vec<Track> = upcoming.iter().take(2).cloned().collect();
                if !next.is_empty() {
                    let token = if crate::provider::requires_spotify_token() {
                        self.api()?.provider_token().unwrap_or_default()
                    } else {
                        String::new()
                    };
                    let artists: Vec<String> = next
                        .iter()
                        .filter_map(|track| track.artists.first()?.id.clone())
                        .collect();
                    for track in next {
                        let _ = self
                            .lyrics
                            .send(LyricCommand::Prefetch(track, token.clone()));
                    }
                    // Through Spotify's catalogue, as the card loads it, so the
                    // About photo is ready (and preloaded) before the skip.
                    for id in artists {
                        spawn_artist_about(
                            self.scoped_native(),
                            self.client.clone(),
                            self.events.clone(),
                            self.context.clone(),
                            String::new(),
                            id,
                        );
                    }
                }
                {
                    // Warm the next tracks' Canvas so skipping shows video at once.
                    let upcoming: Vec<String> =
                        upcoming.into_iter().map(|track| track.uri).collect();
                    let native = self.scoped_native();
                    let events = self.events.clone();
                    thread::spawn(move || {
                        for uri in upcoming {
                            if let Some(url) = canvas_url(&native, &uri) {
                                let _ = events.send(Event::CanvasPrefetched { uri, url });
                            }
                        }
                    });
                }
            }
            Command::SetSaved { uri, saved } => {
                if (!self.native_ready() || self.native_controls.set_saved(&uri, saved).is_err())
                    && let Err(error) = self.api().and_then(|api| api.set_saved_item(&uri, saved))
                {
                    // The window already applied the change; undo it there.
                    self.emit(Event::Saved {
                        uri: uri.clone(),
                        saved: !saved,
                    });
                    self.emit(Event::PageChanged("liked".into()));
                    return Err(error);
                }
                // The window drops unliked songs itself; new likes need a reload.
                if saved && uri.starts_with("spotify:track:") {
                    self.emit(Event::PageChanged("liked".into()));
                }
                self.emit(Event::Saved { uri, saved });
            }
            Command::Shutdown => {}
        }
        Ok(())
    }

    fn native_connecting(&self) -> bool {
        self.spotify.as_ref().is_some_and(Spotify::is_native)
            && !self.native_ready()
            && self
                .native_controls
                .connected_or_connecting(&self.profile.id)
    }

    fn native_ready(&self) -> bool {
        matches!(
            self.native_controls
                .connection_event(Some(&self.profile.id)),
            Some(crate::native::NativeEvent::Ready(_))
        )
    }

    /// Saved status from the native library mirror, off the worker thread.
    fn publish_saved(&self, uris: Vec<String>) {
        let mut seen = std::collections::HashSet::new();
        let uris: Vec<String> = uris
            .into_iter()
            .filter(|uri| crate::collection::supports(uri) && seen.insert(uri.clone()))
            .collect();
        if uris.is_empty() || !self.native_ready() {
            return;
        }
        let native = self.scoped_native();
        let events = self.events.clone();
        let context = self.context.clone();
        thread::spawn(move || {
            for chunk in uris.chunks(500) {
                let Ok(saved) = native.saved_items(chunk) else {
                    return;
                };
                let _ = events.send(Event::SavedMany(chunk.iter().cloned().zip(saved).collect()));
            }
            context.request_repaint();
        });
    }

    fn fetch_tracks(&mut self, key: &str, offset: usize) -> Result<Vec<Track>> {
        let native = self.scoped_native();
        fetch_tracks(self.api()?, &native, key, offset)
    }
}
/// Most songs one background batch loads page by page, when the native
/// read is unavailable.
const REST_BATCH: usize = 2_000;
/// A playlist, Liked Songs, an artist's liked songs or an album: lists
/// that show every song at once.
fn loads_whole(key: &str) -> bool {
    key == "liked"
        || ["playlist:", "liked-artist:", "album:"]
            .iter()
            .any(|prefix| key.starts_with(prefix))
}
/// Such a list in one native read; None for anything else.
fn read_all(native: &NativeControls, key: &str, offset: usize) -> Option<Result<Vec<Track>>> {
    let id = key.strip_prefix("playlist:").unwrap_or(key);
    loads_whole(key)
        .then(|| native.playlist_tracks_range(id, offset, crate::native_browse::READ_LIMIT))
}
struct PrefetchRequest {
    spotify: Spotify,
    native: NativeControls,
    account: String,
    key: String,
}
fn publish_browse_info(
    events: &mpsc::Sender<Event>,
    context: &Notifier,
    account: &str,
    generation: u64,
    info: BrowseInfo,
) {
    let info = crate::library_cache::save_info(account, info);
    let _ = events.send(Event::BrowseInfo {
        generation,
        info: Box::new(info),
    });
    context.request_repaint();
}
struct FactsRequest {
    spotify: Spotify,
    native: NativeControls,
    tracks: Vec<Track>,
}
fn fetch_tracks(
    api: &mut Spotify,
    native: &NativeControls,
    key: &str,
    offset: usize,
) -> Result<Vec<Track>> {
    let account = api.account().map(str::to_owned);
    let result = fetch_live_tracks(api, native, key, offset);
    match result {
        Ok(tracks) => {
            if let Some(account) = account {
                crate::library_cache::save_tracks(&account, key, offset, &tracks);
            }
            Ok(tracks)
        }
        Err(error) => {
            if let Some(account) = account
                && let Some(tracks) = crate::library_cache::tracks(&account, key, offset)
            {
                return Ok(tracks);
            }
            Err(error)
        }
    }
}
fn fetch_live_tracks(
    api: &mut Spotify,
    native: &NativeControls,
    key: &str,
    offset: usize,
) -> Result<Vec<Track>> {
    if let Some(query) = key.strip_prefix("search:") {
        native.playlist_tracks(key, offset).or_else(|_| {
            if offset == 0 {
                api.search(query)
            } else {
                Ok(Vec::new())
            }
        })
    } else if let Some(id) = key.strip_prefix("playlist:") {
        match native.playlist_tracks(id, offset) {
            Ok(tracks) => Ok(tracks),
            Err(_) => api.playlist_tracks(id, offset),
        }
    } else if let Some(id) = key.strip_prefix("album:") {
        native
            .playlist_tracks(key, offset)
            .or_else(|_| api.album_tracks(id, offset))
    } else if key.starts_with("liked-artist:") {
        native.playlist_tracks(key, offset)
    } else if key.starts_with("artist:") {
        Ok(Vec::new())
    } else {
        match key {
            "liked" => native
                .playlist_tracks("liked", offset)
                .or_else(|_| api.saved_tracks(offset)),
            "queue" if let Some((queue, _)) = crate::soloist_link::queue() => Ok(queue),
            "queue" if native.has_track() => native.queue(),
            "queue" => api.queue(),
            // Spotify keeps only the latest 50 plays, so there is no second page.
            _ if offset > 0 => Ok(Vec::new()),
            _ => native.recent().or_else(|_| api.recent()),
        }
    }
}

/// Canvas video URL for a track, from the cache or the Spotify Connect
/// metadata service.
/// Loads an artist's About details for Now playing's card: the saved copy
/// at once when it has the About photo, then Spotify's catalogue, saved for
/// next time. `uri` is the song it is for, empty when warming an upcoming one.
fn spawn_artist_about(
    native: NativeControls,
    client: reqwest::blocking::Client,
    events: mpsc::Sender<Event>,
    context: Notifier,
    uri: String,
    artist_id: String,
) {
    thread::spawn(move || {
        let send = |overview: crate::artist::ArtistOverview| {
            let _ = events.send(Event::ArtistStats {
                uri: uri.clone(),
                artist_id: artist_id.clone(),
                listeners: overview.monthly_listeners,
                header: overview.header_image.clone(),
                gallery: overview.gallery_image.clone(),
                image: overview.header_image.or(overview.avatar_image),
                verified: overview.verified,
                biography: overview.biography,
            });
            context.request_repaint();
        };
        // Loaded from the catalogue lately: the saved copy is that answer.
        static RECENT: std::sync::Mutex<Option<HashMap<String, Instant>>> =
            std::sync::Mutex::new(None);
        let recent = RECENT.lock().ok().is_some_and(|mut recent| {
            let recent = recent.get_or_insert_with(HashMap::new);
            recent.retain(|_, when| when.elapsed() < Duration::from_secs(600));
            recent.contains_key(&artist_id)
        });
        if recent {
            send(crate::artist::saved(&artist_id).unwrap_or_default());
            return;
        }
        // An older copy without the About photo would flash the avatar.
        if let Some(saved) =
            crate::artist::saved(&artist_id).filter(|saved| saved.gallery_image.is_some())
        {
            send(saved);
        }
        // The public web page has no About photos, so the catalogue comes first.
        let native = native
            .catalogue("artist", &artist_id, 0)
            .and_then(|entity| crate::artist::from_entity(&entity, &artist_id));
        if let Ok(overview) = &native {
            crate::artist::save(&artist_id, overview);
            // Marked only once saved, so a parallel request never reads an
            // unsaved copy.
            if let Ok(mut recent) = RECENT.lock() {
                recent
                    .get_or_insert_with(HashMap::new)
                    .insert(artist_id.clone(), Instant::now());
            }
        }
        // Sent even when nothing loaded, so the card stops waiting.
        send(
            native
                .or_else(|_| crate::artist::fetch(&client, &artist_id))
                .unwrap_or_default(),
        );
    });
}

fn canvas_url(native: &NativeControls, uri: &str) -> Option<String> {
    if let Some(url) = crate::canvas::cached(uri) {
        return url;
    }
    let bytes = native
        .metadata(
            uri,
            librespot_protocol::extension_kind::ExtensionKind::CANVAS_V1,
        )
        .ok()?;
    let url = crate::canvas::video_url(&bytes, uri).ok().flatten();
    crate::canvas::remember(uri, url.clone());
    url
}

#[cfg(test)]
mod session_tests {
    use super::*;
    fn empty_backend() -> (Backend, mpsc::Receiver<Event>) {
        let (events, receive) = mpsc::channel();
        (
            Backend {
                library_refresh: Default::default(),
                facts: mpsc::sync_channel(1).0,
                prefetch: mpsc::sync_channel(1).0,
                native_controls: Default::default(),
                spotify: None,
                native: None,
                native_generation: 0,
                client_id: String::new(),
                profile: Default::default(),
                client: reqwest::blocking::Client::new(),
                events,
                native_events: mpsc::channel().0,
                lyrics: mpsc::channel().0,
                context: Default::default(),
            },
            receive,
        )
    }

    #[test]
    fn automatic_refresh_during_native_startup_does_not_report_a_playback_error() {
        let (mut backend, events) = empty_backend();
        backend.profile.id = "startup-listener".into();
        let (controls, _commands) = NativeControls::test_connecting(&backend.profile.id);
        backend.native_controls = controls.clone();
        backend.spotify = Some(
            Spotify::native(backend.client.clone(), controls, backend.profile.clone()).unwrap(),
        );
        assert!(
            backend
                .native_controls
                .connected_or_connecting(&backend.profile.id)
        );
        assert!(
            backend.poll().is_ok(),
            "Periodic startup polling must not report a connection error"
        );
        let result = backend.handle(Command::Refresh);
        assert!(
            result.is_ok(),
            "Normal startup refresh must not create an error toast: {}",
            result
                .err()
                .map(|error| error.to_string())
                .unwrap_or_default()
        );
        assert!(events.try_recv().is_err());
        assert!(
            backend.native.is_none(),
            "Refresh must not restart the player"
        );
        let (controls, _commands) = NativeControls::test_active(&backend.profile.id);
        backend.native_controls = controls;
        backend.handle(Command::Refresh).unwrap();
        assert!(matches!(events.try_recv(), Ok(Event::Devices(_))));
    }

    #[test]
    fn cancelled_native_login_cannot_unlock_or_persist_credentials() {
        let (mut backend, events) = empty_backend();
        backend
            .handle(Command::NativeLoginResult {
                cancelled: Arc::new(AtomicBool::new(true)),
                result: Ok(Some(Box::new(crate::native_login::Authenticated::test(
                    Profile {
                        id: "cancelled-test-account".into(),
                        ..Default::default()
                    },
                    librespot_core::authentication::Credentials::with_access_token("test"),
                )))),
            })
            .unwrap();
        assert!(backend.spotify.is_none());
        assert!(backend.native.is_none());
        assert!(backend.profile.id.is_empty());
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn missing_or_rejected_native_login_keeps_client_locked() {
        let (mut backend, events) = empty_backend();
        backend
            .handle(Command::NativeLoginResult {
                cancelled: Arc::new(AtomicBool::new(false)),
                result: Ok(None),
            })
            .unwrap();
        assert!(matches!(events.try_recv(), Ok(Event::SignedOut)));
        backend
            .handle(Command::NativeLoginResult {
                cancelled: Arc::new(AtomicBool::new(false)),
                result: Err(auth::LoginIssue {
                    message: "Spotify unavailable".into(),
                    retry_saved: true,
                }),
            })
            .unwrap();
        assert!(matches!(events.try_recv(), Ok(Event::LoginFailed(_))));
        assert!(backend.spotify.is_none());
        assert!(backend.native.is_none());
    }

    #[test]
    fn local_song_details_do_not_spend_web_api_quota() {
        let client = auth::http_client().unwrap();
        let mut spotify = Spotify::new(
            client.clone(),
            "test-local-song-details".into(),
            Tokens {
                access_token: String::new(),
                refresh_token: String::new(),
                expires_at: u64::MAX,
                scopes: String::new(),
                profile: None,
            },
        );
        let attempts = spotify.count_network_attempts();
        let (native_controls, mut native_commands) =
            NativeControls::test_active("test-local-song-details");
        let mut backend = Backend {
            library_refresh: Default::default(),
            facts: mpsc::sync_channel(1).0,
            prefetch: mpsc::sync_channel(1).0,
            native_controls,
            spotify: Some(spotify),
            native: None,
            native_generation: 0,
            client_id: "test-local-song-details".into(),
            profile: Profile {
                id: "test-local-song-details".into(),
                ..Default::default()
            },
            client,
            events: mpsc::channel().0,
            native_events: mpsc::channel().0,
            lyrics: mpsc::channel().0,
            context: Default::default(),
        };
        for index in 0..3 {
            backend
                .handle(Command::PlayingDetails(Track {
                    uri: format!("spotify:track:{index:022}"),
                    artists: vec![crate::model::Artist {
                        id: Some("invalid-test-artist".into()),
                        name: "Test artist".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }))
                .unwrap();
        }
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "Local song changes must use native queue data rather than polling the Web API"
        );
        backend.poll().unwrap();
        backend.handle(Command::Refresh).unwrap();
        assert!(
            backend
                .player(PlayerAction::Transfer("test-device".into()), None)
                .is_err()
        );
        backend
            .player(
                PlayerAction::Enqueue("spotify:track:0000000000000000000001".into()),
                Some("test-device".into()),
            )
            .unwrap();
        assert!(matches!(
            native_commands.try_recv(),
            Ok(PlayerAction::Enqueue(_))
        ));
        assert_eq!(attempts.load(std::sync::atomic::Ordering::Relaxed), 0);
        backend.native_controls = NativeControls::default();
        backend
            .handle(Command::PlayingDetails(Track::default()))
            .unwrap();
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "Remote playback retains its Web API queue lookup"
        );
    }
    #[test]
    fn a_failed_startup_library_fetch_is_retried_without_another_login() {
        let mut refresh = LibraryRefresh::default();
        let now = Instant::now();
        let failure =
            refresh.fetch::<Vec<Playlist>>(now, || anyhow::bail!("Temporary catalogue failure"));
        assert!(failure.is_err());
        assert!(!refresh.due(now + Duration::from_secs(1)));
        assert!(
            refresh.due(now + Duration::from_secs(30)),
            "Missing library must schedule recovery"
        );
        let playlists = refresh
            .fetch(now + Duration::from_secs(30), || {
                Ok((0..36)
                    .map(|index| Playlist {
                        id: format!("playlist-{index}"),
                        ..Default::default()
                    })
                    .collect::<Vec<_>>())
            })
            .unwrap();
        assert_eq!(playlists.len(), 36);
        assert!(!refresh.due(now + Duration::from_secs(60)));
    }
    #[test]
    fn native_library_keeps_owner_names_and_artwork_from_the_last_read() {
        let owned = |id: &str, owner: &str, name: Option<&str>, art: bool| Playlist {
            id: id.into(),
            owner: crate::model::Owner {
                id: owner.into(),
                display_name: name.map(Into::into),
            },
            images: if art {
                vec![crate::model::Artwork {
                    url: "https://mosaic.scdn.co/cover".into(),
                    width: None,
                }]
            } else {
                Vec::new()
            },
            ..Default::default()
        };
        let profile = Profile {
            id: "me".into(),
            display_name: Some("Me".into()),
            ..Profile::default()
        };
        let previous = [
            owned("a", "friend", Some("Friend"), true),
            owned("d", "renamed", Some("Old"), true),
        ];
        let filled = fill_library_details(
            vec![
                owned("a", "friend", None, false),
                owned("b", "me", None, false),
                owned("c", "other", None, false),
                owned("d", "renamed", Some("New"), true),
            ],
            Some(&previous),
            &profile,
        );
        assert_eq!(filled[0].owner.display_name.as_deref(), Some("Friend"));
        assert_eq!(filled[0].images.len(), 1);
        assert_eq!(filled[1].owner.display_name.as_deref(), Some("Me"));
        assert_eq!(filled[2].owner.display_name, None);
        assert_eq!(filled[3].owner.display_name.as_deref(), Some("New"));
    }
    #[test]
    fn cached_profiles_only_restore_the_same_session_during_cooldown() {
        let cached = Profile {
            id: "existing".into(),
            ..Profile::default()
        };
        assert_eq!(
            profile_for_session(
                Err(crate::api_backoff::CoolingDown(60).into()),
                Some(cached.clone()),
                true
            )
            .unwrap()
            .id,
            "existing"
        );
        assert!(
            profile_for_session(
                Err(crate::api_backoff::CoolingDown(60).into()),
                Some(cached.clone()),
                false
            )
            .is_err()
        );
        assert!(
            profile_for_session(Err(anyhow::anyhow!("unauthorized")), Some(cached), true).is_err()
        );
    }
}
