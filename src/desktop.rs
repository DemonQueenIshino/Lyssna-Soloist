//! Public desktop events. Tokens and the configured provider URL stay in Rust.
use crate::{
    api::PlayerAction,
    auth,
    config::{self, Settings},
    lyrics::Lyrics,
    model::{BrowseInfo, Device, Playback, PlayingDetails, Playlist, Profile, Track},
    native::{self, NativeControls, NativeEvent},
    notify::Notifier,
    updater,
    worker::{Command, Event, LyricCommand, Worker},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{Emitter, Manager, State};

/// Drop playback queued during connect if audio took longer than this.
const QUEUED_NATIVE_TTL: Duration = Duration::from_secs(45);
fn send_queued_native(
    queued: &Mutex<Option<(PlayerAction, Instant)>>,
    controls: &NativeControls,
) -> bool {
    queued
        .lock()
        .ok()
        .and_then(|mut queued| queued.take())
        .filter(|(_, at)| at.elapsed() < QUEUED_NATIVE_TTL)
        .is_some_and(|(action, _)| controls.send(action).is_ok())
}
#[derive(Clone, Serialize)]
pub struct Snapshot {
    narrating: bool,
    home: Option<crate::home::HomePage>,
    home_loading: bool,
    home_error: String,
    canvas_url: Option<String>,
    profile: Option<Profile>,
    playlists: Vec<Playlist>,
    playback: Playback,
    settings: Settings,
    native_ready: bool,
    local_device_name: String,
    native_status: String,
    provider_configured: bool,
    provider_builtin: bool,
    provider_kind: Option<String>,
    login_pending: bool,
    login_issue: Option<auth::LoginIssue>,
    can_follow: bool,
    lyrics: Option<Lyrics>,
    lyric_error: String,
    details: PlayingDetails,
    route: String,
    generation: u64,
    tracks: Vec<Track>,
    search_playlists: Vec<Playlist>,
    search_artists: Vec<crate::model::Artist>,
    search_albums: Vec<crate::model::Album>,
    search_top_results: Vec<String>,
    search_users: Vec<crate::model::Profile>,
    search_playlists_loading: bool,
    search_playlists_error: String,
    browse_info: Option<BrowseInfo>,
    loading: bool,
    more: bool,
    update_status: String,
    update_version: Option<String>,
    update_notes: String,
    notice: String,
    observed_at: u64,
}
struct Inner {
    dj_requests: crate::dj::requests::Requests,
    dj_voice: crate::dj::voice::VoiceCapture,
    home_generation: std::sync::atomic::AtomicU64,
    snapshot: Mutex<Snapshot>,
    commands: mpsc::Sender<Command>,
    lyrics: mpsc::Sender<LyricCommand>,
    controls: NativeControls,
    native_id: Mutex<Option<String>>,
    generation: Mutex<u64>,
    last_native: Mutex<Option<Instant>>,
    pending_track: Mutex<Option<(String, Instant)>>,
    /// The Jam this computer sends its playback commands to, when the Jam
    /// asks for them.
    jam_commands: Mutex<Option<String>>,
    /// Click-to-audio timing for the last local selection, by page kind.
    start_timing: Mutex<Option<(String, Instant, &'static str)>>,
    /// Local playback requested before native audio connected.
    queued_native: Mutex<Option<(PlayerAction, Instant)>>,
    login_cancelled: Mutex<Arc<AtomicBool>>,
    stopped: AtomicBool,
    update_commands: mpsc::Sender<updater::Command>,
    available: Mutex<Option<updater::Update>>,
    ready_update: Mutex<Option<updater::ReadyUpdate>>,
}
pub struct Desktop(Arc<Inner>);
fn restore_playback(snapshot: &mut Snapshot, value: crate::resume::LastPlayed) {
    if value.dj_context.is_some()
        && let Some(profile) = snapshot.profile.as_ref()
    {
        crate::resume::remember_dj_context(&profile.id, &value.track.uri, value.dj_context);
    }
    snapshot.playback.item = Some(value.track);
    snapshot.playback.progress_ms = Some(value.position_ms);
    snapshot.playback.is_playing = false;
    snapshot.playback.shuffle_state = snapshot.settings.shuffle;
    snapshot.playback.repeat_state = snapshot.settings.repeat.clone();
    snapshot.playback.device = Device {
        name: native::device_name(),
        volume_percent: Some(snapshot.settings.volume_percent.min(100)),
        ..Device::default()
    };
    snapshot.observed_at = now_ms();
    snapshot.lyrics = None;
    snapshot.canvas_url = None;
    snapshot.details = PlayingDetails::default();
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn error(error: impl std::fmt::Display) -> String {
    error.to_string()
}
fn lock<T>(value: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>> {
    value
        .lock()
        .map_err(|_| anyhow::anyhow!("Desktop state unavailable"))
}

fn targets_local_device(target: Option<&str>, native_id: Option<&str>, device: &Device) -> bool {
    target == Some("native")
        || target.is_some_and(|target| Some(target) == native_id)
        || device.name == native::device_name()
            && (target.is_none() || target == device.id.as_deref())
}

/// DJ narration exists only in this computer's player; every other command,
/// including playing an album or playlist page, follows the chosen device.
fn plays_locally(
    command: &str,
    target: Option<&str>,
    native_id: Option<&str>,
    device: &Device,
) -> bool {
    command == "dj" || targets_local_device(target, native_id, device)
}

fn load_native_catalogue(
    inner: Arc<Inner>,
    app: tauri::AppHandle,
    account: String,
    key: String,
    generation: Option<u64>,
) {
    let controls = inner.controls.scoped(&account).unwrap_or_default();
    thread::spawn(move || {
        let Some((kind, id)) = key
            .split_once(':')
            .filter(|(kind, _)| matches!(*kind, "artist" | "album"))
        else {
            return;
        };
        let cached = crate::library_cache::info(&account, &key);
        let complete = cached.as_ref().is_some_and(|info| {
            if kind == "artist" {
                info.overview.is_some()
            } else {
                info.album_details.is_some()
            }
        });
        let info = if generation.is_none()
            && complete
            && crate::library_cache::info_is_fresh(&account, &key)
        {
            cached
        } else {
            controls.catalogue(kind, id, 0).ok().and_then(|entity| {
                let mut info = crate::catalogue::base_info(&entity, kind, id).ok()?;
                if kind == "artist" {
                    info.overview = crate::artist::from_entity(&entity, id).ok();
                } else {
                    info.album_details = Some(crate::album::parse(&entity));
                }
                Some(crate::library_cache::save_info(&account, info))
            })
        };
        let Some(info) = info else {
            if let Some(generation) = generation
                && let Ok(snapshot) = inner.snapshot.lock()
                && snapshot.generation == generation
                && snapshot.route == key
                && snapshot
                    .profile
                    .as_ref()
                    .is_some_and(|profile| profile.id == account)
            {
                let _ = inner.commands.send(Command::BrowseInfo { key, generation });
            }
            return;
        };
        let album_incomplete = info
            .album_details
            .as_ref()
            .is_some_and(|details| !details.complete_duration || details.streams.is_empty());
        let Ok(mut snapshot) = inner.snapshot.lock() else {
            return;
        };
        if snapshot
            .profile
            .as_ref()
            .is_none_or(|profile| profile.id != account)
        {
            return;
        }
        if let Some(generation) = generation {
            if snapshot.generation != generation || snapshot.route != key {
                return;
            }
            let needs_status = info
                .overview
                .as_ref()
                .is_some_and(|overview| !overview.details_loaded)
                && snapshot
                    .browse_info
                    .as_ref()
                    .and_then(|info| info.overview.as_ref())
                    .is_none();
            snapshot.browse_info = Some(info.clone());
            if kind == "artist" && info.overview.is_some() {
                snapshot.loading = false;
                snapshot.more = false;
            }
            let _ = app.emit(
                "lyssna",
                json!({"type":"browse_info","generation":generation,"info":info}),
            );
            if needs_status {
                let _ = inner.commands.send(Command::ArtistTracks {
                    account,
                    generation,
                    info: Box::new(info),
                });
            }
            if album_incomplete {
                let _ = inner.commands.send(Command::BrowseInfo { key, generation });
            }
        } else {
            let _ = app.emit("lyssna", json!({"type":"info_prefetch","info":info}));
        }
    });
}

impl Desktop {
    fn start(app: tauri::AppHandle) -> Result<Self> {
        let (settings, notice) = match Settings::load() {
            Ok(settings) => (settings, String::new()),
            Err(_) => (
                Settings::default(),
                "Existing settings could not be read. Sign in to continue.".into(),
            ),
        };
        let (wake, wakes) = mpsc::sync_channel(1);
        let notifier = Notifier::new(move || {
            let _ = wake.try_send(());
        });
        crate::lyric_sources::configure(
            &settings.lyrics_source_order,
            &settings.lyrics_sources_disabled,
        );
        crate::lyric_sources::set_prefer_apple(settings.lyrics_prefer_apple);
        crate::lyric_sources::set_musixmatch_words(!settings.lyrics_musixmatch_lines_only);
        crate::lyric_sources::load_musixmatch_token();
        updater::set_channel(&settings.update_channel);
        let scanned = app.clone();
        crate::local_files::on_change(move |count| {
            let _ = scanned.emit("lyssna", json!({"type":"local_files","count":count}));
        });
        crate::local_files::rescan(&settings);
        let worker = Worker::start(notifier.clone());
        let updates = updater::Updater::start(notifier);
        let route = settings.last_view.clone();
        let inner = Arc::new(Inner {
            home_generation: std::sync::atomic::AtomicU64::new(0),
            dj_requests: crate::dj::requests::Requests::default(),
            dj_voice: crate::dj::voice::VoiceCapture::default(),
            snapshot: Mutex::new(Snapshot {
                narrating: false,
                canvas_url: None,
                home: None,
                home_loading: false,
                home_error: String::new(),
                profile: None,
                playlists: Vec::new(),
                playback: Playback {
                    device: Device {
                        volume_percent: Some(settings.volume_percent.min(100)),
                        ..Device::default()
                    },
                    shuffle_state: settings.shuffle,
                    repeat_state: settings.repeat.clone(),
                    ..Playback::default()
                },
                settings: settings.clone(),
                native_ready: false,
                local_device_name: native::device_name(),
                native_status: "Waiting for sign-in".into(),
                provider_configured: crate::provider::configured(),
                provider_builtin: crate::provider::builtin_configured(),
                provider_kind: crate::provider::kind(),
                login_pending: true,
                login_issue: None,
                can_follow: false,
                lyrics: None,
                lyric_error: String::new(),
                details: PlayingDetails::default(),
                route,
                generation: 0,
                tracks: Vec::new(),
                search_playlists: Vec::new(),
                search_artists: Vec::new(),
                search_albums: Vec::new(),
                search_top_results: Vec::new(),
                search_users: Vec::new(),
                search_playlists_loading: false,
                search_playlists_error: String::new(),
                browse_info: None,
                loading: false,
                more: false,
                update_status: "Checking for updates…".into(),
                update_version: None,
                update_notes: String::new(),
                notice,
                observed_at: now_ms(),
            }),
            commands: worker.commands.clone(),
            lyrics: worker.lyrics.clone(),
            controls: worker.native_controls.clone(),
            native_id: Mutex::new(None),
            generation: Mutex::new(0),
            last_native: Mutex::new(None),
            pending_track: Mutex::new(None),
            jam_commands: Mutex::new(None),
            start_timing: Mutex::new(None),
            queued_native: Mutex::new(None),
            login_cancelled: Mutex::new(Arc::new(AtomicBool::new(false))),
            stopped: AtomicBool::new(false),
            update_commands: updates.commands.clone(),
            available: Mutex::new(None),
            ready_update: Mutex::new(None),
        });
        inner.commands.send(Command::NativeLogin {
            restoring: true,
            cancelled: lock(&inner.login_cancelled)?.clone(),
        })?;
        // A Nightly copy looks for newer code and builds it.
        let watched = inner.clone();
        let updated = inner.clone();
        let statuses = app.clone();
        let reports = app.clone();
        crate::nightly::watch(
            move || {
                watched.snapshot.lock().map_or((false, false), |snapshot| {
                    (
                        snapshot.settings.update_channel == "nightly",
                        snapshot.settings.auto_download_updates,
                    )
                })
            },
            move |status| nightly_update_status(&updated, &statuses, status),
            move |report| {
                let _ = reports.emit("lyssna", json!({"type":"nightly","report":report}));
            },
        );
        let pump = inner.clone();
        thread::spawn(move || {
            let emit = |value: Value| {
                let _ = app.emit("lyssna", value);
            };
            while !pump.stopped.load(Ordering::Relaxed) {
                let _ = wakes.recv_timeout(Duration::from_millis(50));
                while let Ok(event) = worker.events.try_recv() {
                    let Ok(mut snapshot) = pump.snapshot.lock() else {
                        return;
                    };
                    match event {
                        Event::TrackFacts(facts) => {
                            if snapshot.profile.is_some() {
                                emit(json!({"type":"track_facts","facts":facts}));
                            }
                        }
                        Event::LastPlayed { account, value } => {
                            if snapshot
                                .profile
                                .as_ref()
                                .is_some_and(|profile| profile.id == account)
                                && !snapshot.playback.is_playing
                                && (snapshot.playback.item.is_none()
                                    || snapshot.playback.device.name == native::device_name())
                                && pump
                                    .last_native
                                    .lock()
                                    .ok()
                                    .is_some_and(|last| last.is_none())
                                && let Some(value) = crate::resume::newer(
                                    crate::resume::load(&account),
                                    Some(*value),
                                )
                            {
                                restore_playback(&mut snapshot, value);
                                emit(
                                    json!({"type":"playback","playback":snapshot.playback,"observed_at":snapshot.observed_at,"changed":true}),
                                );
                                if let Some(track) = snapshot.playback.item.clone() {
                                    let _ = pump.commands.send(Command::FetchLyrics(track.clone()));
                                    let _ = pump.commands.send(Command::PlayingDetails(track));
                                }
                            }
                        }
                        Event::PagePrefetched {
                            account,
                            key,
                            tracks,
                            info,
                        } => {
                            if snapshot
                                .profile
                                .as_ref()
                                .is_some_and(|profile| profile.id == account)
                            {
                                emit(
                                    json!({"type":"page_prefetch","key":key,"more":tracks.len()==50,"tracks":tracks}),
                                );
                                if let Some(info) = info {
                                    emit(json!({"type":"info_prefetch","info":info}));
                                }
                            }
                        }
                        Event::CanvasPrefetched { uri, url } => {
                            emit(json!({"type":"canvas_prefetch","uri":uri,"url":url}));
                        }
                        Event::Canvas { uri, url } => {
                            if snapshot
                                .playback
                                .item
                                .as_ref()
                                .is_some_and(|track| track.uri == uri)
                            {
                                snapshot.canvas_url = url.clone();
                                emit(json!({"type":"canvas","uri":uri,"url":url}));
                            }
                        }
                        Event::ArtistStats {
                            uri,
                            artist_id,
                            listeners,
                            image,
                            header,
                            gallery,
                            verified,
                            biography,
                        } => {
                            // Every artist's About, upcoming ones too: the window
                            // keeps them by artist and preloads the photo, so the
                            // card fills the moment a song starts.
                            emit(json!({
                                "type": "artist_about",
                                "id": artist_id,
                                "about": {
                                    "about_loaded": true,
                                    "gallery_image": gallery,
                                    "header_image": header,
                                    "monthly_listeners": listeners,
                                    "verified": verified,
                                    "biography": biography,
                                    "image": image,
                                },
                            }));
                            if snapshot
                                .playback
                                .item
                                .as_ref()
                                .is_some_and(|track| track.uri == uri)
                            {
                                for artist in &mut snapshot.details.artists {
                                    if artist.id.as_deref() == Some(&artist_id) {
                                        if listeners.is_some() {
                                            artist.monthly_listeners = listeners;
                                        }
                                        if verified.is_some() {
                                            artist.verified = verified;
                                        }
                                        if !biography.is_empty() {
                                            artist.biography = biography.clone();
                                        }
                                        if let Some(url) = &header {
                                            artist.header_image = url.clone();
                                        }
                                        if let Some(url) = &gallery {
                                            artist.gallery_image = url.clone();
                                        }
                                        artist.about_loaded = true;
                                        // Local playback only knows artists whose page
                                        // was opened; fall back to the header photo.
                                        if artist.images.is_empty()
                                            && let Some(url) = &image
                                        {
                                            artist.images = vec![crate::model::Artwork {
                                                url: url.clone(),
                                                width: None,
                                            }];
                                        }
                                    }
                                }
                                emit(
                                    json!({"type":"playing_details","uri":uri,"details":snapshot.details}),
                                );
                            }
                        }
                        Event::TrackInfo { uri, mut track } => {
                            if snapshot
                                .playback
                                .item
                                .as_ref()
                                .is_some_and(|current| current.uri == uri)
                            {
                                track.uri = uri;
                                if track.songwriters.is_empty() {
                                    track.songwriters = snapshot
                                        .playback
                                        .item
                                        .as_ref()
                                        .map(|item| item.songwriters.clone())
                                        .unwrap_or_default();
                                }
                                snapshot.playback.item = Some(*track);
                                emit(
                                    json!({"type":"playback","playback":snapshot.playback,"observed_at":snapshot.observed_at,"changed":false}),
                                );
                            }
                        }
                        Event::Songwriters { uri, names } => {
                            if let Some(item) = snapshot
                                .playback
                                .item
                                .as_mut()
                                .filter(|item| item.uri == uri && item.songwriters.is_empty())
                            {
                                item.songwriters = names;
                                emit(
                                    json!({"type":"playback","playback":snapshot.playback,"observed_at":snapshot.observed_at,"changed":false}),
                                );
                            }
                        }
                        Event::ProfileRefreshed(profile) => {
                            if snapshot.profile.as_ref().map(|old| &old.id) == Some(&profile.id) {
                                let _ = crate::native::save_profile(&profile);
                                snapshot.profile = Some(profile.clone());
                                emit(json!({"type":"profile","profile":profile}));
                            }
                        }
                        Event::Session(profile) => {
                            if !snapshot.login_pending {
                                continue;
                            }
                            if snapshot.profile.as_ref().map(|old| &old.id) != Some(&profile.id) {
                                snapshot.home = None;
                                snapshot.home_loading = false;
                                snapshot.home_error.clear();
                                pump.home_generation.fetch_add(1, Ordering::SeqCst);
                            }
                            snapshot.profile = Some(profile.clone());
                            snapshot.login_issue = None;
                            snapshot.login_pending = false;
                            snapshot.can_follow =
                                pump.controls.connected_or_connecting(&profile.id)
                                    || auth::Tokens::load(&snapshot.settings.client_id)
                                        .ok()
                                        .flatten()
                                        .is_some_and(|tokens| {
                                            tokens
                                                .scopes
                                                .split_whitespace()
                                                .any(|scope| scope == "user-follow-modify")
                                        });
                            emit(
                                json!({"type":"session","profile":profile,"can_follow":snapshot.can_follow}),
                            );
                            if let Some(connection) =
                                pump.controls.connection_event(Some(&profile.id))
                            {
                                match connection {
                                    NativeEvent::Ready(id) => {
                                        snapshot.native_ready = true;
                                        snapshot.native_status = "Ready · this computer".into();
                                        if let Ok(mut current) = pump.native_id.lock() {
                                            *current = Some(id);
                                        }
                                    }
                                    NativeEvent::Authorizing => {
                                        snapshot.native_status =
                                            "Authorize audio playback in your browser…".into()
                                    }
                                    NativeEvent::Error(message) => {
                                        snapshot.native_ready = false;
                                        snapshot.native_status = message;
                                    }
                                    _ => {}
                                }
                                emit(
                                    json!({"type":"native","ready":snapshot.native_ready,"status":snapshot.native_status}),
                                );
                            }
                            if snapshot.home.is_none()
                                && let Some(page) = crate::library_cache::home(&profile.id)
                            {
                                snapshot.home = Some(page.clone());
                                emit(json!({"type":"home","page":page}));
                            }
                            if let Some(value) = crate::resume::load(&profile.id) {
                                restore_playback(&mut snapshot, value);
                                emit(
                                    json!({"type":"playback","playback":snapshot.playback,"observed_at":snapshot.observed_at,"changed":true}),
                                );
                                // Saved lyrics and details show without
                                // waiting for the first playback report.
                                if let Some(track) = snapshot.playback.item.clone() {
                                    let _ = pump.commands.send(Command::FetchLyrics(track.clone()));
                                    let _ = pump.commands.send(Command::PlayingDetails(track));
                                }
                            }
                        }
                        Event::SignedOut => {
                            pump.dj_requests.cancel_all();
                            pump.dj_voice.cancel_all();
                            if let Ok(mut queued) = pump.queued_native.lock() {
                                *queued = None;
                            }
                            snapshot.profile = None;
                            snapshot.login_issue = None;
                            snapshot.login_pending = false;
                            snapshot.playlists.clear();
                            snapshot.home = None;
                            snapshot.home_loading = false;
                            snapshot.home_error.clear();
                            snapshot.playback = Playback::default();
                            snapshot.narrating = false;
                            snapshot.native_ready = false;
                            snapshot.lyrics = None;
                            snapshot.canvas_url = None;
                            snapshot.details = PlayingDetails::default();
                            snapshot.tracks.clear();
                            snapshot.search_playlists.clear();
                            snapshot.search_artists.clear();
                            snapshot.search_albums.clear();
                            snapshot.search_top_results.clear();
                            snapshot.search_users.clear();
                            snapshot.search_playlists_loading = false;
                            snapshot.search_playlists_error.clear();
                            emit(json!({"type":"signed_out"}));
                        }
                        Event::LoginFailed(issue) => {
                            if !snapshot.login_pending {
                                continue;
                            }
                            snapshot.login_issue = Some(issue.clone());
                            snapshot.login_pending = false;
                            emit(json!({"type":"login_failed","issue":issue}));
                        }
                        Event::LoginFinished => {
                            snapshot.login_pending = false;
                            emit(json!({"type":"login_finished"}));
                        }
                        Event::NativeStarting(generation) => {
                            pump.dj_requests.cancel_all();
                            pump.dj_voice.cancel_all();
                            if let Ok(mut current) = pump.generation.lock() {
                                *current = generation;
                            }
                            if let Ok(mut last) = pump.last_native.lock() {
                                *last = None;
                            }
                            snapshot.native_ready = false;
                            snapshot.native_status = "Connecting native audio…".into();
                            if snapshot.playback.device.name == native::device_name() {
                                snapshot.playback.is_playing = false;
                                snapshot.observed_at = now_ms();
                                emit(
                                    json!({"type":"playback","playback":snapshot.playback,"observed_at":snapshot.observed_at,"changed":false}),
                                );
                            }
                            emit(
                                json!({"type":"native","ready":false,"status":snapshot.native_status}),
                            );
                        }
                        Event::Library(playlists) => {
                            snapshot.playlists = playlists;
                            emit(json!({"type":"library","playlists":snapshot.playlists}));
                        }
                        Event::LibraryFolders(folders) => {
                            emit(json!({"type":"library_folders","folders":folders}));
                        }
                        Event::Playback(mut playback, _observed) => {
                            if snapshot.native_ready
                                && playback.device.name == native::device_name()
                            {
                                playback.device.volume_percent =
                                    Some(snapshot.settings.volume_percent);
                            }
                            if snapshot.playback.keep_native_when_empty(
                                &playback,
                                snapshot.native_ready,
                                &native::device_name(),
                            ) {
                                continue;
                            }
                            if let Ok(mut pending) = pump.pending_track.lock()
                                && let Some((uri, requested)) = pending.as_ref()
                            {
                                if playback.item.as_ref().is_none_or(|track| &track.uri != uri)
                                    && requested.elapsed() < Duration::from_secs(8)
                                {
                                    continue;
                                }
                                *pending = None;
                            }
                            let changed = snapshot.playback.item.as_ref().map(|track| &track.uri)
                                != playback.item.as_ref().map(|track| &track.uri);
                            if changed
                                && playback.device.name == native::device_name()
                                && pump
                                    .last_native
                                    .lock()
                                    .ok()
                                    .and_then(|value| *value)
                                    .is_some_and(|native| native.elapsed() < Duration::from_secs(2))
                            {
                                continue;
                            }
                            if !changed
                                && playback.device.name == native::device_name()
                                && pump
                                    .last_native
                                    .lock()
                                    .ok()
                                    .and_then(|value| *value)
                                    .is_some_and(|native| native.elapsed() < Duration::from_secs(2))
                            {
                                let elapsed = if snapshot.playback.is_playing {
                                    now_ms().saturating_sub(snapshot.observed_at)
                                } else {
                                    0
                                };
                                playback.progress_ms = snapshot
                                    .playback
                                    .progress_ms
                                    .map(|position| position.saturating_add(elapsed));
                                playback.is_playing = snapshot.playback.is_playing;
                                playback.shuffle_state = snapshot.playback.shuffle_state;
                                playback.repeat_state = snapshot.playback.repeat_state.clone();
                                playback.device.volume_percent =
                                    snapshot.playback.device.volume_percent;
                                // The Web API can lag a context this player just loaded.
                                if snapshot.playback.context.is_some() {
                                    playback.context = snapshot.playback.context.clone();
                                }
                            }
                            snapshot.playback = *playback;
                            snapshot.observed_at = now_ms();
                            if changed {
                                snapshot.lyrics = None;
                                snapshot.canvas_url = None;
                                snapshot.lyric_error.clear();
                                snapshot.details = PlayingDetails::default();
                                if let Some(track) = snapshot.playback.item.clone() {
                                    let _ = pump.commands.send(Command::FetchLyrics(track.clone()));
                                    let _ = pump.commands.send(Command::PlayingDetails(track));
                                }
                            }
                            emit(
                                json!({"type":"playback","playback":snapshot.playback,"observed_at":snapshot.observed_at,"changed":changed}),
                            );
                        }
                        Event::SearchResults {
                            account,
                            key,
                            generation,
                            results,
                            message,
                        } => {
                            if snapshot.generation != generation
                                || snapshot.route != key
                                || snapshot
                                    .profile
                                    .as_ref()
                                    .is_none_or(|profile| profile.id != account)
                            {
                                continue;
                            }
                            snapshot.tracks = results.tracks.clone();
                            snapshot.search_playlists = results.playlists.clone();
                            snapshot.search_artists = results.artists.clone();
                            snapshot.search_albums = results.albums.clone();
                            snapshot.search_top_results = results.top_results.clone();
                            snapshot.search_users = results.users.clone();
                            snapshot.loading = false;
                            snapshot.more = false;
                            snapshot.search_playlists_loading = false;
                            snapshot.search_playlists_error = message.clone();
                            emit(
                                json!({"type":"search_results","account":account,"key":key,"generation":generation,"results":results,"message":message}),
                            );
                        }
                        Event::Tracks {
                            key,
                            offset,
                            generation,
                            tracks,
                            more,
                        } => {
                            if snapshot.generation != generation || snapshot.route != key {
                                continue;
                            }
                            snapshot.loading = false;
                            snapshot.more = more;
                            if offset == 0
                                && let Some((_, asked)) = ASKED
                                    .lock()
                                    .ok()
                                    .and_then(|mut asked| asked.take())
                                    .filter(|(asked, _)| *asked == generation)
                            {
                                crate::native::diagnostic(&format!(
                                    "browse sent {} {} songs {}ms after asking",
                                    page_kind(&key),
                                    tracks.len(),
                                    asked.elapsed().as_millis()
                                ));
                            }
                            if offset == 0 {
                                snapshot.tracks = tracks.clone();
                            } else if snapshot.tracks.len() == offset {
                                snapshot.tracks.extend(tracks.clone());
                            }
                            emit(
                                json!({"type":"tracks","key":key,"generation":generation,"offset":offset,"tracks":tracks,"more":snapshot.more}),
                            );
                        }
                        Event::TracksFailed {
                            generation,
                            message,
                        } => {
                            if generation == snapshot.generation {
                                snapshot.loading = false;
                                emit(
                                    json!({"type":"tracks_failed","generation":generation,"message":message}),
                                );
                            }
                        }
                        Event::BrowseInfo { generation, info } => {
                            if generation == snapshot.generation {
                                if info
                                    .overview
                                    .as_ref()
                                    .is_some_and(|overview| !overview.details_loaded)
                                {
                                    let _ = pump.commands.send(Command::ArtistTracks {
                                        account: snapshot
                                            .profile
                                            .as_ref()
                                            .map(|profile| profile.id.clone())
                                            .unwrap_or_default(),
                                        generation,
                                        info: info.clone(),
                                    });
                                }
                                snapshot.browse_info = Some(*info.clone());
                                emit(
                                    json!({"type":"browse_info","generation":generation,"info":info}),
                                );
                            }
                        }
                        Event::PlayingDetails { uri, details } => {
                            if snapshot
                                .playback
                                .item
                                .as_ref()
                                .is_some_and(|track| track.uri == uri)
                            {
                                let mut details = *details;
                                // Same song again: keep the About details already
                                // shown, so the card's photo never blinks out.
                                for artist in &mut details.artists {
                                    if let Some(known) =
                                        snapshot.details.artists.iter().find(|known| {
                                            known.about_loaded && known.id == artist.id
                                        })
                                    {
                                        artist.about_loaded = true;
                                        artist.gallery_image = known.gallery_image.clone();
                                        artist.header_image = known.header_image.clone();
                                        artist.monthly_listeners = known.monthly_listeners;
                                        artist.verified = known.verified;
                                        artist.biography = known.biography.clone();
                                        if artist.images.is_empty() {
                                            artist.images = known.images.clone();
                                        }
                                    }
                                }
                                snapshot.details = details.clone();
                                emit(json!({"type":"playing_details","uri":uri,"details":details}));
                            }
                        }
                        Event::PageChanged(key) => {
                            if snapshot.route == key {
                                if key.starts_with("playlist:") {
                                    let _ = pump.commands.send(Command::BrowseInfo {
                                        key: key.clone(),
                                        generation: snapshot.generation,
                                    });
                                }
                                let _ = pump.commands.send(Command::Tracks {
                                    key,
                                    offset: 0,
                                    generation: snapshot.generation,
                                });
                            }
                        }
                        Event::SavedMany(items) => emit(json!({
                            "type": "saved_many",
                            "items": items.into_iter().collect::<std::collections::HashMap<_, _>>()
                        })),
                        Event::Saved { uri, saved } => {
                            emit(json!({"type":"saved","uri":uri,"saved":saved}))
                        }
                        Event::Devices(devices) => {
                            emit(json!({"type":"devices","devices":devices}))
                        }
                        Event::Notice(message) => {
                            emit(json!({"type":"notice","message":message,"error":false}))
                        }
                        Event::Error(message) => {
                            snapshot.notice = message.clone();
                            emit(json!({"type":"notice","message":message,"error":true}));
                        }
                    }
                }
                while let Ok(message) = worker.native_events.try_recv() {
                    if pump
                        .generation
                        .lock()
                        .map_or(true, |generation| *generation != message.generation)
                    {
                        continue;
                    }
                    let Ok(mut snapshot) = pump.snapshot.lock() else {
                        return;
                    };
                    if snapshot.profile.is_none() {
                        continue;
                    }
                    match message.event {
                        NativeEvent::Stopped => {
                            if snapshot.playback.device.name == native::device_name() {
                                snapshot.playback.is_playing = false;
                                snapshot.observed_at = now_ms();
                                emit(
                                    json!({"type":"playback","playback":snapshot.playback,"observed_at":snapshot.observed_at,"changed":false}),
                                );
                            }
                        }
                        NativeEvent::Narration(active) => {
                            snapshot.narrating = active;
                            emit(json!({"type":"narration","active":active}));
                        }
                        NativeEvent::Transport {
                            shuffle,
                            repeat,
                            volume,
                        } => {
                            if snapshot.playback.device.name == native::device_name() {
                                if let Some(shuffle) = shuffle {
                                    snapshot.playback.shuffle_state = shuffle;
                                }
                                if let Some(repeat) = repeat {
                                    snapshot.playback.repeat_state = repeat;
                                }
                                if let Some(volume) = volume {
                                    snapshot.settings.volume_percent = volume;
                                    snapshot.playback.device.volume_percent = Some(volume);
                                }
                                emit(
                                    json!({"type":"playback","playback":snapshot.playback,"observed_at":snapshot.observed_at,"changed":false}),
                                );
                            }
                        }
                        NativeEvent::Track(track) => {
                            let track = *track;
                            if let Ok(mut pending) = pump.pending_track.lock()
                                && let Some((uri, requested)) = pending.as_ref()
                            {
                                if *uri != track.uri && requested.elapsed() < Duration::from_secs(8)
                                {
                                    continue;
                                }
                                *pending = None;
                            }
                            let native_id = pump.native_id.lock().ok().and_then(|id| id.clone());
                            snapshot.observed_at = now_ms();
                            let volume = snapshot.settings.volume_percent;
                            if !snapshot.playback.load_native_track(
                                track.clone(),
                                native_id.as_deref(),
                                &native::device_name(),
                                volume,
                            ) {
                                // The clicked song is already shown, but rows from
                                // search carry no release date; fetch full details.
                                if snapshot.playback.item.as_ref().is_some_and(|item| {
                                    item.uri == track.uri && item.album.release_date.is_empty()
                                }) {
                                    let _ = pump.commands.send(Command::TrackInfo(track.uri));
                                }
                                emit(
                                    json!({"type":"playback","playback":snapshot.playback,"observed_at":snapshot.observed_at,"changed":false,"loading":true}),
                                );
                                continue;
                            }
                            snapshot.observed_at = now_ms();
                            snapshot.lyrics = None;
                            snapshot.canvas_url = None;
                            snapshot.lyric_error.clear();
                            snapshot.details = PlayingDetails::default();
                            let _ = pump.commands.send(Command::TrackInfo(track.uri.clone()));
                            let _ = pump.commands.send(Command::FetchLyrics(track.clone()));
                            let _ = pump.commands.send(Command::PlayingDetails(track));
                            emit(
                                json!({"type":"playback","playback":snapshot.playback,"observed_at":snapshot.observed_at,"changed":true,"loading":true}),
                            );
                        }
                        NativeEvent::Reconnecting(seconds) => {
                            snapshot.narrating = false;
                            snapshot.native_ready = false;
                            snapshot.native_status =
                                format!("Reconnecting native audio in {seconds}s…");
                            if snapshot.playback.device.name == native::device_name() {
                                snapshot.playback.is_playing = false;
                                snapshot.observed_at = now_ms();
                                emit(
                                    json!({"type":"playback","playback":snapshot.playback,"observed_at":snapshot.observed_at,"changed":false}),
                                );
                            }
                            emit(
                                json!({"type":"native","ready":false,"status":snapshot.native_status}),
                            );
                        }
                        NativeEvent::Authorizing => {
                            snapshot.narrating = false;
                            snapshot.native_ready = false;
                            snapshot.native_status =
                                "Authorize audio playback in your browser…".into();
                            emit(
                                json!({"type":"native","ready":false,"status":snapshot.native_status}),
                            );
                        }
                        NativeEvent::Ready(id) => {
                            if !matches!(pump.controls.connection_event(snapshot.profile.as_ref().map(|profile|profile.id.as_str())),Some(NativeEvent::Ready(current))if current==id)
                            {
                                continue;
                            }
                            if let Ok(mut current) = pump.native_id.lock() {
                                *current = Some(id);
                            }
                            snapshot.native_ready = true;
                            snapshot.native_status = "Ready · this computer".into();
                            // Reload playlists from Spotify's own library list.
                            let _ = pump.commands.send(Command::Library);
                            if send_queued_native(&pump.queued_native, &pump.controls)
                                && let Ok(mut last) = pump.last_native.lock()
                            {
                                *last = Some(Instant::now());
                            }
                            if ["playlist:", "album:", "artist:"]
                                .iter()
                                .any(|prefix| snapshot.route.starts_with(prefix))
                                && snapshot.profile.is_some()
                            {
                                if snapshot.route.starts_with("artist:")
                                    || snapshot.route.starts_with("album:")
                                {
                                    load_native_catalogue(
                                        pump.clone(),
                                        app.clone(),
                                        snapshot
                                            .profile
                                            .as_ref()
                                            .map(|profile| profile.id.clone())
                                            .unwrap_or_default(),
                                        snapshot.route.clone(),
                                        Some(snapshot.generation),
                                    );
                                } else {
                                    let _ = pump.commands.send(Command::BrowseInfo {
                                        key: snapshot.route.clone(),
                                        generation: snapshot.generation,
                                    });
                                }
                            }
                            emit(
                                json!({"type":"native","ready":true,"status":snapshot.native_status}),
                            );
                            if let Some(track) = snapshot.playback.item.clone() {
                                // A song restored from last session may lack its date.
                                if track.album.release_date.is_empty() {
                                    let _ =
                                        pump.commands.send(Command::TrackInfo(track.uri.clone()));
                                }
                                let _ = pump.commands.send(Command::FetchLyrics(track.clone()));
                                let _ = pump.commands.send(Command::PlayingDetails(track));
                            }
                        }
                        NativeEvent::Error(message) => {
                            snapshot.narrating = false;
                            if let Ok(mut queued) = pump.queued_native.lock() {
                                *queued = None;
                            }
                            snapshot.native_ready = false;
                            snapshot.native_status = message;
                            if snapshot.playback.device.name == native::device_name() {
                                snapshot.playback.is_playing = false;
                                snapshot.observed_at = now_ms();
                                emit(
                                    json!({"type":"playback","playback":snapshot.playback,"observed_at":snapshot.observed_at,"changed":false}),
                                );
                            }
                            emit(
                                json!({"type":"native","ready":false,"status":snapshot.native_status}),
                            );
                        }
                        NativeEvent::ControlError(message) => {
                            emit(json!({"type":"notice","message":message,"error":true}))
                        }
                        NativeEvent::Position {
                            uri,
                            position_ms,
                            playing,
                            seeked,
                        } => {
                            let id = pump.native_id.lock().ok().and_then(|value| value.clone());
                            if snapshot.playback.apply_native_position(
                                id.as_deref(),
                                &native::device_name(),
                                &uri,
                                position_ms,
                                playing,
                            ) {
                                if playing == Some(true)
                                    && let Ok(mut pending) = pump.pending_track.lock()
                                {
                                    *pending = None;
                                }
                                if playing == Some(true)
                                    && let Ok(mut timing) = pump.start_timing.lock()
                                    && let Some((_, started, kind)) =
                                        timing.take_if(|(wanted, ..)| *wanted == uri)
                                {
                                    record_start_time(kind, started.elapsed());
                                }
                                if let Ok(mut last) = pump.last_native.lock() {
                                    *last = Some(Instant::now());
                                }
                                snapshot.observed_at = now_ms();
                                if let Some(profile) = &snapshot.profile {
                                    let _ = crate::resume::save(
                                        &profile.id,
                                        &snapshot.playback,
                                        snapshot.observed_at,
                                        false,
                                    );
                                }
                                emit(
                                    json!({"type":"native_position","uri":uri,"position_ms":position_ms,"playing":playing,"seeked":seeked,"observed_at":snapshot.observed_at}),
                                );
                            }
                        }
                        NativeEvent::DJContext(active) => {
                            emit(json!({"type": "dj_context", "active": active}));
                        }
                        // Guests joining or leaving, or this computer's
                        // place in the Jam: the app asks for the Jam again.
                        NativeEvent::Jam(following, session) => {
                            let jam = session
                                .and_then(|session| serde_json::from_str(&session).ok())
                                .and_then(|session: serde_json::Value| crate::jam::parse(&session));
                            emit(
                                json!({"type": "jam_changed", "following": following, "jam": jam}),
                            );
                        }
                        // Only this computer's player loads queues, so its
                        // context is the one playing; the track may follow.
                        // Kept for other playlists and the next launch.
                        NativeEvent::Modes(modes) => {
                            snapshot.settings.shuffle = modes.shuffle;
                            snapshot.settings.repeat = modes.repeat.clone();
                            if let Err(error) = snapshot.settings.save() {
                                log::warn!("Cannot save shuffle and repeat: {error:#}");
                            }
                            // Before this computer plays, Spirc reports nothing.
                            let device = &snapshot.playback.device.name;
                            if device.is_empty() || *device == native::device_name() {
                                snapshot.playback.shuffle_state = modes.shuffle;
                                snapshot.playback.repeat_state = modes.repeat;
                                emit(
                                    json!({"type":"playback","playback":snapshot.playback,"observed_at":snapshot.observed_at,"changed":false}),
                                );
                            }
                        }
                        NativeEvent::Context(uri) => {
                            snapshot.playback.context = (!uri.is_empty()).then_some(uri);
                            emit(
                                json!({"type":"playback","playback":snapshot.playback,"observed_at":snapshot.observed_at,"changed":false}),
                            );
                        }
                        NativeEvent::QueueChanged => {
                            if let Some(track) = snapshot.playback.item.clone() {
                                let _ = pump.commands.send(Command::PlayingDetails(track));
                            }
                            if snapshot.route == "queue" {
                                let _ = pump.commands.send(Command::Tracks {
                                    key: "queue".into(),
                                    offset: 0,
                                    generation: snapshot.generation,
                                });
                            }
                        }
                    }
                }
                while let Ok(event) = worker.lyric_events.try_recv() {
                    let Ok(mut snapshot) = pump.snapshot.lock() else {
                        return;
                    };
                    if event.uri.is_empty() {
                        match event.result {
                            Ok(_) => {
                                snapshot.provider_configured = true;
                                snapshot.provider_kind = crate::provider::kind();
                                snapshot.lyric_error.clear();
                                emit(
                                    json!({"type":"provider_configured","kind":snapshot.provider_kind}),
                                );
                                if let Some(track) = snapshot.playback.item.clone() {
                                    let _ = pump.commands.send(Command::FetchLyrics(track));
                                }
                            }
                            Err(message) => {
                                emit(json!({"type":"notice","message":message,"error":true}))
                            }
                        }
                    } else if snapshot
                        .playback
                        .item
                        .as_ref()
                        .is_some_and(|track| track.uri == event.uri)
                    {
                        match event.result {
                            Ok(lyrics) => {
                                snapshot.lyrics = Some(lyrics.clone());
                                snapshot.lyric_error.clear();
                                emit(json!({"type":"lyrics","uri":event.uri,"lyrics":lyrics}));
                            }
                            Err(message) => {
                                snapshot.lyric_error = message.clone();
                                emit(
                                    json!({"type":"lyric_error","uri":event.uri,"message":message}),
                                );
                            }
                        }
                    }
                }
                while let Ok(event) = updates.events.try_recv() {
                    let Ok(mut snapshot) = pump.snapshot.lock() else {
                        return;
                    };
                    match event {
                        updater::Event::Current => {
                            snapshot.update_status = "Up to date".into();
                            snapshot.update_version = None;
                            snapshot.update_notes.clear();
                        }
                        // Nightly's own watcher reports its updates.
                        updater::Event::Nightly => {
                            if let Ok(mut available) = pump.available.lock() {
                                *available = None;
                            }
                            if let Ok(mut ready) = pump.ready_update.lock() {
                                *ready = None;
                            }
                        }
                        // Found before the switch to Nightly.
                        updater::Event::Available(_)
                            if snapshot.settings.update_channel == "nightly" => {}
                        updater::Event::Available(update) => {
                            snapshot.update_version = Some(update.manifest.version.clone());
                            snapshot.update_notes = update.manifest.notes.clone();
                            snapshot.update_status = "Update available".into();
                            if snapshot.settings.auto_download_updates {
                                let _ = pump
                                    .update_commands
                                    .send(updater::Command::Download(update.clone()));
                            }
                            if let Ok(mut available) = pump.available.lock() {
                                *available = Some(update);
                            }
                        }
                        updater::Event::Progress { downloaded, total } => {
                            snapshot.update_status = format!(
                                "Downloading update · {}%",
                                downloaded.saturating_mul(100) / total.max(1)
                            )
                        }
                        updater::Event::Ready(update) => {
                            snapshot.update_status = "Ready to install".into();
                            if let Ok(mut ready) = pump.ready_update.lock() {
                                *ready = Some(update);
                            }
                        }
                        updater::Event::Restarted => {
                            app.exit(0);
                            return;
                        }
                        updater::Event::Error(message) => snapshot.update_status = message,
                    }
                    emit(
                        json!({"type":"update","status":snapshot.update_status,"version":snapshot.update_version,"notes":snapshot.update_notes}),
                    );
                }
            }
        });
        Ok(Self(inner))
    }
    fn stop(&self) {
        self.0.dj_requests.cancel_all();
        self.0.dj_voice.cancel_all();
        if let Ok(mut snapshot) = self.0.snapshot.lock() {
            // A remote speaker's level is not this computer's volume.
            if snapshot.playback.device.name == native::device_name() {
                snapshot.settings.volume_percent = snapshot
                    .playback
                    .device
                    .volume_percent
                    .unwrap_or(snapshot.settings.volume_percent)
                    .min(100);
            }
            let _ = snapshot.settings.save();
        }
        if let Ok(snapshot) = self.0.snapshot.lock()
            && let Some(profile) = &snapshot.profile
            && self
                .0
                .last_native
                .lock()
                .ok()
                .is_some_and(|last| last.is_some())
        {
            let _ = crate::resume::save(&profile.id, &playback_now(&snapshot), now_ms(), true);
        }
        self.0.stopped.store(true, Ordering::Relaxed);
        if let Ok(cancelled) = self.0.login_cancelled.lock() {
            cancelled.store(true, Ordering::Relaxed);
        }
        let _ = self.0.commands.send(Command::Shutdown);
        let _ = self.0.update_commands.send(updater::Command::Shutdown);
    }
}

#[tauri::command]
fn bootstrap(state: State<'_, Desktop>) -> std::result::Result<Snapshot, String> {
    lock(&state.0.snapshot)
        .map(|snapshot| snapshot.clone())
        .map_err(error)
}
#[tauri::command]
fn dj_prepare(account: String, state: State<'_, Desktop>) -> std::result::Result<(), String> {
    let snapshot = lock(&state.0.snapshot).map_err(error)?;
    crate::dj::requests::authorize(
        snapshot.profile.as_ref().map(|p| p.id.as_str()),
        snapshot.native_ready,
        &account,
    )
    .map_err(error)?;
    state.0.controls.prewarm_dj_request(&account).map_err(error)
}
#[tauri::command]
async fn dj_request(
    account: String,
    id: String,
    text: String,
    input: crate::dj::PromptInput,
    state: State<'_, Desktop>,
) -> std::result::Result<(), String> {
    let inner = state.0.clone();
    let ticket = {
        let snapshot = lock(&inner.snapshot).map_err(error)?;
        crate::dj::requests::authorize(
            snapshot.profile.as_ref().map(|p| p.id.as_str()),
            snapshot.native_ready,
            &account,
        )
        .map_err(error)?;
        inner.dj_requests.reserve(&id).map_err(error)?
    };
    let prompt = crate::dj::Prompt::new(&text, input).map_err(error)?;
    tauri::async_runtime::spawn_blocking(move || {
        inner
            .controls
            .request_dj(&account, prompt, ticket.cancelled.clone())
    })
    .await
    .map_err(|_| "DJ worker unavailable".to_owned())?
    .map_err(error)
}
#[tauri::command]
async fn dj_voice(
    account: String,
    id: String,
    dialog: String,
    state: State<'_, Desktop>,
    app: tauri::AppHandle,
) -> std::result::Result<String, String> {
    let (capture, vocabulary) = {
        let snapshot = lock(&state.0.snapshot).map_err(error)?;
        crate::dj::requests::authorize(
            snapshot.profile.as_ref().map(|p| p.id.as_str()),
            snapshot.native_ready,
            &account,
        )
        .map_err(error)?;
        let vocabulary = crate::dj::voice::artist_vocabulary(
            snapshot
                .playback
                .item
                .iter()
                .flat_map(|track| &track.artists)
                .chain(&snapshot.details.artists)
                .chain(snapshot.tracks.iter().flat_map(|track| &track.artists))
                .map(|artist| artist.name.as_str()),
        );
        (
            state.0.dj_voice.reserve(id.clone()).map_err(error)?,
            vocabulary,
        )
    };
    tauri::async_runtime::spawn_blocking(move || {
        capture.transcribe_in_dialog(&vocabulary,&dialog,|progress| {
            let _ = app.emit(
                "lyssna-dj-voice",
                json!({"account":account,"id":id,"level":progress.level,"speech":progress.speech,"stage":progress.stage,"percent":progress.percent}),
            );
        })
    })
    .await
    .map_err(|_| "Voice worker unavailable".to_owned())?
    .map_err(error)
}
#[tauri::command]
async fn dj_release_voice(dialog: String) -> std::result::Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || crate::dj::voice::release_model(&dialog))
        .await
        .map_err(|_| "Voice cleanup unavailable".to_owned())
}
#[tauri::command]
fn dj_cancel(id: String, state: State<'_, Desktop>) {
    state.0.dj_requests.cancel(&id);
    state.0.dj_voice.cancel(&id);
}
#[tauri::command]
fn prefetch_metadata(
    keys: Vec<String>,
    state: State<'_, Desktop>,
    app: tauri::AppHandle,
) -> std::result::Result<(), String> {
    let snapshot = lock(&state.0.snapshot).map_err(error)?;
    let account = snapshot
        .profile
        .as_ref()
        .map(|profile| profile.id.clone())
        .ok_or_else(|| "Sign in first".to_owned())?;
    if !snapshot.native_ready {
        return Ok(());
    }
    let mut seen = std::collections::HashSet::new();
    for key in keys
        .into_iter()
        .take(8)
        .filter(|key| valid_route(key) && (key.starts_with("artist:") || key.starts_with("album:")))
    {
        if seen.insert(key.clone()) {
            load_native_catalogue(state.0.clone(), app.clone(), account.clone(), key, None);
        }
    }
    Ok(())
}
#[tauri::command]
fn sign_in(client_id: String, state: State<'_, Desktop>) -> std::result::Result<(), String> {
    let result = || -> Result<()> {
        ensure!(
            client_id.is_empty() || config::valid_client_id(&client_id),
            "Client ID must contain 32 hexadecimal characters"
        );
        let mut snapshot = lock(&state.0.snapshot)?;
        if !client_id.is_empty() {
            snapshot.settings.client_id = client_id.clone();
        }
        snapshot.settings.save()?;
        snapshot.login_pending = true;
        snapshot.login_issue = None;
        let mut old = lock(&state.0.login_cancelled)?;
        old.store(true, Ordering::Relaxed);
        *old = Arc::new(AtomicBool::new(false));
        let command = if client_id.is_empty() {
            Command::NativeLogin {
                restoring: false,
                cancelled: old.clone(),
            }
        } else {
            Command::Login(client_id, old.clone())
        };
        state.0.commands.send(command)?;
        Ok(())
    };
    result().map_err(error)
}
/// Load a page the pointer is resting on, without navigating, so opening it
/// is instant. Results go to the frontend page cache only.
#[tauri::command]
fn prefetch_page(key: String, state: State<'_, Desktop>) -> std::result::Result<(), String> {
    let result = || -> Result<()> {
        ensure!(valid_route(&key), "Unknown page");
        ensure!(lock(&state.0.snapshot)?.profile.is_some(), "Sign in first");
        state.0.commands.send(Command::PrefetchPage(key))?;
        Ok(())
    };
    result().map_err(error)
}

/// Resolve Canvas videos for tracks the user is about to play (hovered rows).
#[tauri::command]
fn prefetch_canvas(uris: Vec<String>, state: State<'_, Desktop>) {
    let uris: Vec<String> = uris
        .into_iter()
        .filter(|uri| uri.starts_with("spotify:track:") && uri.len() < 64)
        .take(8)
        .collect();
    if !uris.is_empty() {
        let _ = state.0.commands.send(Command::PrefetchCanvas(uris));
    }
}
#[tauri::command]
fn cancel_sign_in(app: tauri::AppHandle, state: State<'_, Desktop>) {
    if let Ok(cancelled) = state.0.login_cancelled.lock() {
        cancelled.store(true, Ordering::Relaxed);
    }
    if let Ok(mut snapshot) = state.0.snapshot.lock() {
        snapshot.login_pending = false;
    }
    let _ = app.emit("lyssna", json!({"type":"login_finished"}));
}
#[tauri::command]
fn retry_sign_in(state: State<'_, Desktop>) -> std::result::Result<(), String> {
    let result = || -> Result<()> {
        let mut snapshot = lock(&state.0.snapshot)?;
        ensure!(
            snapshot.profile.is_none() && !snapshot.login_pending,
            "Sign-in already active"
        );

        let mut cancelled = lock(&state.0.login_cancelled)?;
        cancelled.store(true, Ordering::Release);
        *cancelled = Arc::new(AtomicBool::new(false));
        state.0.commands.send(Command::NativeLogin {
            restoring: true,
            cancelled: cancelled.clone(),
        })?;
        snapshot.login_pending = true;
        snapshot.login_issue = None;
        Ok(())
    };
    result().map_err(error)
}
#[tauri::command]
fn sign_out(state: State<'_, Desktop>) -> std::result::Result<(), String> {
    state.0.dj_requests.cancel_all();
    state.0.dj_voice.cancel_all();
    if let Ok(cancelled) = state.0.login_cancelled.lock() {
        cancelled.store(true, Ordering::Relaxed);
    }
    if let Ok(mut snapshot) = state.0.snapshot.lock() {
        snapshot.profile = None;
        snapshot.login_pending = false;
    }
    state.0.commands.send(Command::Logout).map_err(error)
}
fn valid_route(key: &str) -> bool {
    if ["home", "liked", "local", "queue", "recent", "whats-new"].contains(&key) {
        return true;
    }
    if let Some(query) = key.strip_prefix("search:") {
        return !query.trim().is_empty() && query.len() <= 500;
    }
    // An artist page's shelf by its place on the page.
    if let Some((id, index)) = key
        .strip_prefix("artist-shelf:")
        .and_then(|rest| rest.split_once(':'))
    {
        return id.len() == 22
            && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
            && !index.is_empty()
            && index.len() <= 2
            && index.bytes().all(|byte| byte.is_ascii_digit());
    }
    [
        "album:",
        "artist:",
        "playlist:",
        "liked-artist:",
        "discography:",
    ]
    .into_iter()
    .any(|prefix| {
        key.strip_prefix(prefix)
            .is_some_and(|id| id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()))
    })
}
#[tauri::command]
async fn home(
    timezone: String,
    app: tauri::AppHandle,
    state: State<'_, Desktop>,
) -> std::result::Result<(), String> {
    let inner = state.0.clone();
    let (account, generation) = {
        let mut snapshot = lock(&inner.snapshot).map_err(error)?;
        let account = snapshot
            .profile
            .as_ref()
            .context("Sign in first")
            .map_err(error)?
            .id
            .clone();
        if !snapshot.native_ready {
            // The frontend retries Home when the native-ready event arrives.
            return Ok(());
        }
        if timezone.len() > 100 || timezone.chars().any(char::is_control) {
            return Err("Invalid time zone".into());
        }
        snapshot.home_loading = true;
        snapshot.home_error.clear();
        (
            account,
            inner.home_generation.fetch_add(1, Ordering::SeqCst) + 1,
        )
    };
    let _ = app.emit("lyssna", json!({"type":"home_loading","account":account}));
    let controls = inner.controls.scoped(&account).map_err(error)?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        let page = controls.home(&timezone)?;
        crate::library_cache::save_home(&page);
        Ok::<_, anyhow::Error>(page)
    })
    .await;
    let mut snapshot = lock(&inner.snapshot).map_err(error)?;
    if inner.home_generation.load(Ordering::SeqCst) != generation
        || snapshot.profile.as_ref().map(|profile| &profile.id) != Some(&account)
    {
        return Ok(());
    }
    snapshot.home_loading = false;
    match result {
        Ok(Ok(page)) if page.account == account => {
            snapshot.home = Some(page.clone());
            let _ = app.emit("lyssna", json!({"type":"home","page":page}));
        }
        _ => {
            snapshot.home_error = "Personalized Home could not load. Try refreshing.".into();
            let _ = app.emit(
                "lyssna",
                json!({"type":"home_error","account":account,"message":snapshot.home_error}),
            );
        }
    }
    Ok(())
}
#[tauri::command]
fn browse(
    key: String,
    generation: u64,
    offset: usize,
    state: State<'_, Desktop>,
    app: tauri::AppHandle,
) -> std::result::Result<(), String> {
    let result = || -> Result<()> {
        ensure!(valid_route(&key), "Unknown page");
        ensure!(offset <= 100_000, "Page offset too large");
        let mut snapshot = lock(&state.0.snapshot)?;
        ensure!(snapshot.profile.is_some(), "Sign in first");
        if offset > 0 && (snapshot.loading || offset != snapshot.tracks.len()) {
            return Ok(());
        }
        if offset == 0 {
            if let Ok(mut asked) = ASKED.lock() {
                *asked = Some((generation, Instant::now()));
            }
            snapshot.route = key.clone();
            snapshot.generation = generation;
            snapshot.tracks.clear();
            snapshot.search_playlists.clear();
            snapshot.search_artists.clear();
            snapshot.search_albums.clear();
            snapshot.search_top_results.clear();
            snapshot.search_users.clear();
            snapshot.search_playlists_loading = key.starts_with("search:");
            snapshot.search_playlists_error.clear();
            snapshot.browse_info = snapshot
                .profile
                .as_ref()
                .and_then(|profile| crate::library_cache::info(&profile.id, &key));
            if let Some(info) = snapshot.browse_info.as_ref() {
                let _ = app.emit(
                    "lyssna",
                    json!({"type":"browse_info","generation":generation,"info":info}),
                );
                if info
                    .overview
                    .as_ref()
                    .is_some_and(|overview| !overview.details_loaded)
                {
                    state.0.commands.send(Command::ArtistTracks {
                        account: snapshot
                            .profile
                            .as_ref()
                            .context("Sign in first")?
                            .id
                            .clone(),
                        generation,
                        info: Box::new(info.clone()),
                    })?;
                }
            }
            snapshot.settings.last_view = key.clone();
            snapshot.settings.save()?;
        }
        ensure!(
            snapshot.route == key && snapshot.generation == generation,
            "Page changed"
        );
        snapshot.loading = true;
        // These pages load their own content.
        if key == "home"
            || key == "whats-new"
            || key.starts_with("discography:")
            || key.starts_with("artist-shelf:")
        {
            snapshot.loading = false;
            snapshot.more = false;
            return Ok(());
        }
        if offset == 0
            && (key.starts_with("album:")
                || key.starts_with("artist:")
                || key.starts_with("playlist:"))
        {
            if snapshot.native_ready && (key.starts_with("artist:") || key.starts_with("album:")) {
                let account = snapshot
                    .profile
                    .as_ref()
                    .context("Sign in first")?
                    .id
                    .clone();
                load_native_catalogue(
                    state.0.clone(),
                    app.clone(),
                    account,
                    key.clone(),
                    Some(generation),
                );
            }
            if !snapshot.native_ready || key.starts_with("playlist:") {
                state.0.commands.send(Command::BrowseInfo {
                    key: key.clone(),
                    generation,
                })?;
            }
        }
        state.0.commands.send(Command::Tracks {
            key,
            generation,
            offset,
        })?;
        Ok(())
    };
    result().map_err(error)
}
/// The interface's click-to-screen time for a list, for the local
/// diagnostics log. Only the kind of page is written.
#[tauri::command]
fn page_shown(kind: String, songs: usize, ms: u64) {
    crate::native::diagnostic(&format!(
        "browse shown {} {songs} songs {ms}ms after clicking",
        page_kind(&kind)
    ));
}
/// When the open page asked for its songs, by generation, to log how long
/// they took to come back.
static ASKED: Mutex<Option<(u64, Instant)>> = Mutex::new(None);

/// Page kinds for start timing; never the page's ID or search text.
fn page_kind(route: &str) -> &'static str {
    match route.split_once(':').map_or(route, |(kind, _)| kind) {
        "search" => "search",
        "playlist" => "playlist",
        "album" => "album",
        "artist" => "artist",
        "liked" | "liked-artist" => "liked",
        "local" => "local",
        "recent" => "recent",
        "queue" => "queue",
        "home" => "home",
        _ => "other",
    }
}

/// Append one click-to-audio time to a small local diagnostics log.
fn record_start_time(kind: &str, elapsed: Duration) {
    use std::io::Write;
    let Ok(dir) = crate::config::data_dir().map(|dir| dir.join("diagnostics")) else {
        return;
    };
    let path = dir.join("start-times.log");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > 64 * 1024) {
        let _ = std::fs::remove_file(&path);
    }
    let when = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{when} {kind} {}ms", elapsed.as_millis());
    }
}

/// Most songs a locally played context carries; Spotify caps playlists at
/// 10,000.
const LISTED_CONTEXT_LIMIT: usize = 10_000;

/// Playing a page's context on this computer uses the songs the page already
/// loaded, under that context, instead of waiting for Spotify to resolve it.
fn local_load(action: PlayerAction, tracks: &[Track]) -> Result<PlayerAction> {
    if matches!(&action,PlayerAction::PlayContext{context,..} if context==crate::home::DJ_URI) {
        return Ok(action);
    }
    if let PlayerAction::PlayContext { context, offset } = &action {
        let uris: Vec<_> = tracks
            .iter()
            .filter(|track| (!track.is_local || track.on_disk) && !track.unavailable)
            .map(|track| track.uri.clone())
            .take(LISTED_CONTEXT_LIMIT)
            .collect();
        ensure!(!uris.is_empty(), "Wait for the playlist tracks to load");
        let index = offset.as_ref().map_or(Ok(0), |uri| {
            uris.iter()
                .position(|track| track == uri)
                .context("Selected track is not loaded")
        })?;
        return Ok(PlayerAction::PlayUris {
            uris,
            offset: index,
            context: Some(context.clone()),
        });
    }
    Ok(action)
}

/// The context a `play_uris` request names when its songs are Liked Songs.
const LIKED_SONGS: &str = "liked";

/// Name the collection for Connect without replacing the displayed queue.
/// The interface supplies every playable row in its current sort/filter order.
fn liked_from_collection(action: PlayerAction, context: Option<&str>, user: &str) -> PlayerAction {
    match action {
        PlayerAction::PlayUris { uris, offset, .. } if context == Some(LIKED_SONGS) => {
            PlayerAction::PlayUris {
                uris,
                offset,
                context: Some(format!("spotify:user:{user}:collection")),
            }
        }
        action => action,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlayerRequest {
    command: String,
    uri: Option<String>,
    uris: Option<Vec<String>>,
    context: Option<String>,
    offset: Option<usize>,
    position_ms: Option<u64>,
    volume: Option<u8>,
    enabled: Option<bool>,
    repeat: Option<String>,
    track: Option<Track>,
}
impl PlayerRequest {
    fn action(&self) -> Result<PlayerAction> {
        Ok(match self.command.as_str() {
            "play" => PlayerAction::Play(self.uri.clone()),
            "dj" => PlayerAction::PlayContext {
                context: crate::home::DJ_URI.into(),
                offset: None,
            },
            "play_home" => {
                let context = self.context.clone().context("Playback context missing")?;
                let route = crate::home::route(&context).context("Invalid music context")?;
                if route.starts_with("track:") {
                    PlayerAction::Play(Some(context))
                } else {
                    ensure!(route != "liked", "Open Liked Songs first");
                    // A song picked from a release's list starts there.
                    let offset = self.uri.clone();
                    if let Some(uri) = &offset {
                        native::queue_uri(uri)?;
                    }
                    PlayerAction::PlayContext { context, offset }
                }
            }
            "pause" => PlayerAction::Pause,
            "next" => PlayerAction::Next,
            "skip_to" => {
                let uri = self.uri.clone().context("Track URI missing")?;
                native::queue_uri(&uri)?;
                PlayerAction::SkipTo(uri)
            }
            "previous" => PlayerAction::Previous,
            "play_context" => PlayerAction::PlayContext {
                context: self.context.clone().context("Playback context missing")?,
                offset: self.uri.clone(),
            },
            "play_uris" => {
                let uris = self.uris.clone().context("Track list missing")?;
                ensure!(
                    !uris.is_empty() && uris.len() <= LISTED_CONTEXT_LIMIT,
                    "Choose between 1 and 10000 tracks"
                );
                let offset = self.offset.unwrap_or(0);
                ensure!(offset < uris.len(), "Track offset out of range");
                let context = self.context.as_ref().filter(|value| *value != LIKED_SONGS);
                if let Some(context) = context {
                    ensure!(
                        crate::home::route(context).is_some_and(|route| {
                            route.starts_with("playlist:") || route.starts_with("album:")
                        }),
                        "Invalid listed playback context"
                    );
                }
                PlayerAction::PlayUris {
                    uris,
                    offset,
                    context: context.cloned(),
                }
            }
            "seek" => PlayerAction::Seek(self.position_ms.context("Seek position missing")?),
            "volume" => PlayerAction::Volume(self.volume.context("Volume missing")?.min(100)),
            "shuffle" => PlayerAction::Shuffle(self.enabled.context("Shuffle state missing")?),
            "repeat" => {
                let repeat = self.repeat.clone().context("Repeat state missing")?;
                ensure!(
                    ["off", "context", "track"].contains(&repeat.as_str()),
                    "Invalid repeat state"
                );
                PlayerAction::Repeat(repeat)
            }
            "enqueue" => {
                let uri = self.uri.clone().context("Track URI missing")?;
                native::queue_uri(&uri)?;
                PlayerAction::Enqueue(uri)
            }
            "remove_from_queue" => {
                let uri = self.uri.clone().context("Track URI missing")?;
                native::queue_uri(&uri)?;
                PlayerAction::RemoveFromQueue(uri)
            }
            "clear_queue" => PlayerAction::ClearQueue,
            "transfer" => PlayerAction::Transfer(self.uri.clone().context("Device missing")?),
            _ => anyhow::bail!("Unknown playback command"),
        })
    }
}
#[tauri::command]
fn player(
    app: tauri::AppHandle,
    request: PlayerRequest,
    target: Option<String>,
    state: State<'_, Desktop>,
) -> std::result::Result<(), String> {
    if !matches!(request.command.as_str(), "volume" | "shuffle" | "repeat") {
        state.0.dj_requests.cancel_all();
    }
    let result = || -> Result<()> {
        let mut snapshot = lock(&state.0.snapshot)?;
        ensure!(snapshot.profile.is_some(), "Sign in first");
        let mut action = request.action()?;
        let native_context = matches!(request.command.as_str(), "dj" | "play_home");
        let local = plays_locally(
            &request.command,
            target.as_deref(),
            lock(&state.0.native_id)?.as_deref(),
            &snapshot.playback.device,
        );
        if local && let Some(profile) = snapshot.profile.as_ref() {
            action = liked_from_collection(action, request.context.as_deref(), &profile.id);
        }
        // In a Jam that asks for them, commands go to the Jam, which passes
        // them on to everyone in it, this computer included, as Spotify's
        // apps do. Played here alone, no one else would hear them.
        if request.command != "dj"
            && let Some(jam) = lock(&state.0.jam_commands)?.clone()
            && let Some(command) = crate::native_remote::jam_command(&action)
        {
            drop(snapshot);
            let inner = state.0.clone();
            tauri::async_runtime::spawn_blocking(move || {
                send_to_jam(&app, &inner, &jam, &command, action);
            });
            return Ok(());
        }
        if local
            && matches!(action, PlayerAction::Play(None))
            && !state.0.controls.has_track()
            && snapshot.playback.item.is_none()
        {
            if let Some(saved) = snapshot
                .profile
                .as_ref()
                .and_then(|profile| crate::resume::load(&profile.id))
            {
                restore_playback(&mut snapshot, saved);
            } else if let Some(track) = snapshot
                .tracks
                .iter()
                .find(|track| !track.is_local)
                .cloned()
            {
                snapshot.playback.item = Some(track);
                snapshot.playback.progress_ms = Some(0);
            } else {
                anyhow::bail!("Choose a song to start playback");
            }
        }
        if local
            && !state.0.controls.has_track()
            && let (Some(profile), Some(track)) =
                (snapshot.profile.as_ref(), snapshot.playback.item.as_ref())
            && let Some(restored) = crate::resume::restore_dj_action(
                &profile.id,
                &track.uri,
                snapshot.playback.progress_ms.unwrap_or_default(),
                &action,
            )
        {
            action = restored;
        }
        // A song that played to its end is stopped, not paused, so the
        // player cannot resume it; Play loads it again from the start.
        let finished = snapshot.playback.finished();
        if local
            && matches!(action, PlayerAction::Play(None))
            && (!state.0.controls.has_track() || finished)
            && let Some(track) = snapshot.playback.item.as_ref()
        {
            let queue = snapshot
                .profile
                .as_ref()
                .map(|profile| crate::resume::queue(&profile.id))
                .filter(|queue| !queue.is_empty())
                .unwrap_or_else(|| {
                    snapshot
                        .tracks
                        .iter()
                        .filter(|item| !item.is_local)
                        .take(100)
                        .map(|item| item.uri.clone())
                        .collect()
                });
            action = crate::resume::restore_action(
                &track.uri,
                &queue,
                if finished {
                    0
                } else {
                    snapshot.playback.progress_ms.unwrap_or_default()
                },
            );
            if let Some(profile) = snapshot.profile.as_ref() {
                action = crate::resume::in_context(&profile.id, action, !finished);
            }
        }
        // Skipping to a queued song needs it coming up on this computer.
        // Otherwise, as after a restart, start the saved queue at it, or play
        // the song alone.
        if local
            && let PlayerAction::SkipTo(uri) = &action
            && !(state.0.controls.has_track() && state.0.controls.coming_up(uri))
        {
            let uri = uri.clone();
            let saved = snapshot
                .profile
                .as_ref()
                .map(|profile| crate::resume::queue(&profile.id))
                .unwrap_or_default();
            action = match saved.iter().position(|item| *item == uri) {
                Some(offset) => PlayerAction::PlayUris {
                    uris: saved,
                    offset,
                    context: None,
                },
                None => PlayerAction::Play(Some(uri)),
            };
        }
        // After a restart the player shows the saved song without loading it;
        // skipping starts the saved queue one song on or back.
        if local
            && matches!(action, PlayerAction::Next | PlayerAction::Previous)
            && !state.0.controls.has_track()
        {
            if snapshot.playback.item.is_none()
                && let Some(saved) = snapshot
                    .profile
                    .as_ref()
                    .and_then(|profile| crate::resume::load(&profile.id))
            {
                restore_playback(&mut snapshot, saved);
            }
            let track = snapshot
                .playback
                .item
                .clone()
                .context("Choose a song to start playback")?;
            let queue = snapshot
                .profile
                .as_ref()
                .map(|profile| crate::resume::queue(&profile.id))
                .filter(|queue| queue.contains(&track.uri))
                .unwrap_or_else(|| {
                    snapshot
                        .tracks
                        .iter()
                        .filter(|item| !item.is_local)
                        .take(100)
                        .map(|item| item.uri.clone())
                        .collect()
                });
            action = crate::resume::skip_action(
                &track.uri,
                &queue,
                snapshot.playback.progress_ms.unwrap_or_default(),
                matches!(action, PlayerAction::Next),
            )
            .context(if matches!(action, PlayerAction::Next) {
                "No next song in the saved queue"
            } else {
                "No previous song in the saved queue"
            })?;
            if let Some(profile) = snapshot.profile.as_ref() {
                action = crate::resume::in_context(&profile.id, action, true);
            }
        }
        if local && action.supports_native() {
            if matches!(action, PlayerAction::Enqueue(_)) {
                ensure!(
                    state.0.controls.has_track(),
                    "Play a song on this computer before adding to its queue."
                );
            }
            if matches!(
                action,
                PlayerAction::Next | PlayerAction::SkipTo(_) | PlayerAction::Previous
            ) {
                *lock(&state.0.pending_track)? = None;
            }
            if !native_context {
                action = local_load(action, &snapshot.tracks)?;
            }
            if let Some(NativeEvent::Ready(id)) = state
                .0
                .controls
                .connection_event(snapshot.profile.as_ref().map(|profile| profile.id.as_str()))
            {
                snapshot.native_ready = true;
                snapshot.native_status = "Ready · this computer".into();
                *lock(&state.0.native_id)? = Some(id);
                state.0.controls.send(action.clone()).map_err(|_| {
                    anyhow::anyhow!(
                        "Native audio disconnected. Reconnect native audio in Settings."
                    )
                })?;
                if matches!(action, PlayerAction::Enqueue(_)) {
                    let _ = app.emit(
                        "lyssna",
                        json!({"type":"notice","message":"Added to queue","error":false}),
                    );
                    return Ok(());
                }
                let _ = app.emit(
                    "lyssna",
                    json!({"type":"native","ready":true,"status":snapshot.native_status}),
                );
            } else if matches!(action, PlayerAction::Pause) {
                *lock(&state.0.queued_native)? = None;
                return Ok(());
            } else {
                if matches!(
                    state.0.controls.connection_event(
                        snapshot.profile.as_ref().map(|profile| profile.id.as_str())
                    ),
                    Some(NativeEvent::Error(_))
                ) {
                    state.0.commands.send(Command::EnsureAudio)?;
                }
                ensure!(
                    matches!(
                        action,
                        PlayerAction::Play(_)
                            | PlayerAction::Resume { .. }
                            | PlayerAction::PlayUris { .. }
                            | PlayerAction::PlayContext { .. }
                            | PlayerAction::RestoreDJ { .. }
                    ),
                    "Native audio is connecting. Try again when ready."
                );
                *lock(&state.0.queued_native)? = Some((action.clone(), Instant::now()));
            }
            if let Some(profile) = snapshot.profile.as_ref() {
                crate::resume::remember_queue(&profile.id, &action);
            }
            if let Some(track) = request.track {
                if let Ok(mut pending) = state.0.pending_track.lock() {
                    *pending = Some((track.uri.clone(), Instant::now()));
                }
                if let Ok(mut timing) = state.0.start_timing.lock() {
                    *timing = Some((
                        track.uri.clone(),
                        Instant::now(),
                        page_kind(&snapshot.route),
                    ));
                }
                snapshot.playback.item = Some(track.clone());
                snapshot.playback.progress_ms = Some(0);
                snapshot.playback.is_playing = false;
                snapshot.lyrics = None;
                snapshot.canvas_url = None;
                snapshot.lyric_error.clear();
                snapshot.details = PlayingDetails::default();
                snapshot.observed_at = now_ms();
                let _ = state.0.commands.send(Command::FetchLyrics(track.clone()));
                let _ = state.0.commands.send(Command::PlayingDetails(track));
            }
            snapshot.playback.device.name = native::device_name();
            snapshot.playback.device.id = lock(&state.0.native_id)?.clone();
            snapshot.playback.device.is_active = true;
            if let Ok(mut last) = state.0.last_native.lock() {
                *last = Some(Instant::now());
            }
            return Ok(());
        }
        let device = if local {
            lock(&state.0.native_id)?.clone()
        } else {
            target
        };
        state.0.commands.send(Command::Player { action, device })?;
        Ok(())
    };
    result().map_err(error)
}
/// Sends a playback command to the Jam. Refused, or with the Jam gone, it
/// plays on this computer instead.
fn send_to_jam(
    app: &tauri::AppHandle,
    inner: &Inner,
    jam: &str,
    command: &serde_json::Value,
    action: PlayerAction,
) {
    let endpoint = command["endpoint"].as_str().unwrap_or_default();
    let sent = inner
        .controls
        .run(async |session| crate::jam::redirect(session, jam, command).await);
    crate::native::diagnostic(&format!(
        "jam command {endpoint}: {}",
        match &sent {
            Ok(true) => "sent".to_owned(),
            Ok(false) => "the Jam has ended".to_owned(),
            Err(error) => format!("{error}"),
        }
    ));
    if matches!(sent, Ok(true)) {
        return;
    }
    if matches!(sent, Ok(false))
        && let Ok(mut current) = inner.jam_commands.lock()
        && current.as_deref() == Some(jam)
    {
        *current = None;
    }
    let message = if inner.controls.send(action).is_ok() {
        "The Jam didn't take that, so it played only here."
    } else {
        "The Jam didn't take that. Try again."
    };
    let _ = app.emit(
        "lyssna",
        json!({"type":"notice","message":message,"error":true}),
    );
}
#[tauri::command]
fn retry_audio(state: State<'_, Desktop>) -> std::result::Result<(), String> {
    state.0.commands.send(Command::RetryAudio).map_err(error)
}
#[tauri::command]
async fn test_audio_output(state: State<'_, Desktop>) -> std::result::Result<bool, String> {
    let name = {
        let snapshot = lock(&state.0.snapshot).map_err(error)?;
        snapshot
            .profile
            .as_ref()
            .context("Sign in first")
            .map_err(error)?;
        snapshot.settings.audio_output.clone()
    };
    tauri::async_runtime::spawn_blocking(move || crate::audio_output::test_sound(name))
        .await
        .map_err(error)?
        .map_err(error)?;
    Ok(true)
}
#[tauri::command]
fn refresh(state: State<'_, Desktop>) -> std::result::Result<(), String> {
    if lock(&state.0.snapshot).map_err(error)?.profile.is_none() {
        return Err("Sign in first".into());
    }
    state.0.commands.send(Command::Refresh).map_err(error)
}
#[tauri::command]
fn save_item(
    uri: String,
    saved: bool,
    state: State<'_, Desktop>,
) -> std::result::Result<(), String> {
    if !crate::collection::supports(&uri) {
        return Err("Choose a Spotify song, album or artist".into());
    }
    state
        .0
        .commands
        .send(Command::SetSaved { uri, saved })
        .map_err(error)
}
#[tauri::command]
async fn audio_outputs(state: State<'_, Desktop>) -> std::result::Result<Vec<String>, String> {
    if lock(&state.0.snapshot).map_err(error)?.profile.is_none() {
        return Err("Sign in first".to_owned());
    }
    tauri::async_runtime::spawn_blocking(crate::audio_output::devices)
        .await
        .map_err(|_| "Audio output worker unavailable".to_owned())?
        .map_err(error)
}
#[tauri::command]
fn save_settings(
    settings: Settings,
    state: State<'_, Desktop>,
    app: tauri::AppHandle,
) -> std::result::Result<(), String> {
    let result = || -> Result<()> {
        let mut snapshot = lock(&state.0.snapshot)?;
        let mut settings = settings;
        let output_changed = settings.audio_output != snapshot.settings.audio_output;
        if !["stable", "nightly"].contains(&settings.update_channel.as_str()) {
            settings.update_channel = "stable".into();
        }
        let channel_changed = settings.update_channel != snapshot.settings.update_channel;
        let local_changed = settings.show_local_files != snapshot.settings.show_local_files
            || settings.local_sources != snapshot.settings.local_sources;
        let sources_changed = settings.lyrics_source_order != snapshot.settings.lyrics_source_order
            || settings.lyrics_sources_disabled != snapshot.settings.lyrics_sources_disabled
            || settings.lyrics_prefer_apple != snapshot.settings.lyrics_prefer_apple
            || settings.lyrics_musixmatch_lines_only
                != snapshot.settings.lyrics_musixmatch_lines_only;
        if output_changed {
            ensure!(snapshot.profile.is_some(), "Sign in first");
            ensure!(
                settings
                    .audio_output
                    .as_deref()
                    .is_none_or(crate::audio_output::valid_name),
                "Choose a valid audio output"
            );
        }
        settings.client_id = snapshot.settings.client_id.clone();
        settings.last_view = snapshot.route.clone();
        settings.volume_percent = snapshot.settings.volume_percent;
        // The player keeps these; the settings window may hold old ones.
        settings.shuffle = snapshot.settings.shuffle;
        settings.repeat = snapshot.settings.repeat.clone();
        settings.lyric_size = settings.lyric_size.clamp(18.0, 72.0);
        // 0 came from the earlier slider, where it meant off.
        settings.crossfade_seconds = match settings.crossfade_seconds {
            0 => 5,
            seconds => seconds.min(12),
        };
        settings.ui_zoom_percent = settings.ui_zoom_percent.clamp(75, 200);
        settings.lyric_offset_ms = settings.lyric_offset_ms.clamp(-10_000, 10_000);
        settings.clean_lyrics_options();
        settings.lyrics_source_order =
            crate::lyric_sources::normalize_order(&settings.lyrics_source_order);
        crate::config::clean_song_sorts(&mut settings.song_sorts);
        settings.lyrics_animator.clean();
        settings.local_sources.dedup_by(|a, b| a.path == b.path);
        settings.save()?;
        if local_changed {
            crate::local_files::rescan(&settings);
        }
        // A new channel is checked at once.
        if channel_changed {
            updater::set_channel(&settings.update_channel);
            if updater::DISABLED.is_none() {
                let _ = state.0.update_commands.send(updater::Command::Check);
            }
        }
        crate::lyric_sources::configure(
            &settings.lyrics_source_order,
            &settings.lyrics_sources_disabled,
        );
        crate::lyric_sources::set_prefer_apple(settings.lyrics_prefer_apple);
        crate::lyric_sources::set_musixmatch_words(!settings.lyrics_musixmatch_lines_only);
        // Reload the playing song's lyrics so a source change shows at once;
        // each source keeps its own cache, so this is cheap.
        if sources_changed && let Some(track) = snapshot.playback.item.clone() {
            let _ = state.0.commands.send(Command::FetchLyrics(track));
        }
        if output_changed && let Some(profile) = snapshot.profile.as_ref() {
            state
                .0
                .controls
                .set_audio_output(&profile.id, settings.audio_output.clone());
        }
        if let Some(profile) = snapshot.profile.as_ref() {
            state
                .0
                .controls
                .set_crossfade(&profile.id, settings.active_crossfade_seconds());
        }
        if let Some(tray) = app.tray_by_id("main") {
            let _ = tray.set_visible(settings.keep_playing_on_close);
        }
        // Every window shows the same settings, so a change made in the
        // Cinema or popout window reaches the main one and the reverse.
        let _ = app.emit("lyssna", json!({"type":"settings","settings":&settings}));
        snapshot.settings = settings;
        Ok(())
    };
    result().map_err(error)
}
#[tauri::command]
fn configure_spicy_lyrics(
    key: String,
    consent: bool,
    state: State<'_, Desktop>,
) -> std::result::Result<(), String> {
    state
        .0
        .lyrics
        .send(LyricCommand::ConfigureSpicy { key, consent })
        .map_err(error)
}
#[tauri::command]
fn configure_provider(
    host: String,
    consent: bool,
    state: State<'_, Desktop>,
) -> std::result::Result<(), String> {
    state
        .0
        .lyrics
        .send(LyricCommand::Configure { host, consent })
        .map_err(error)
}
#[tauri::command]
fn retry_lyrics(state: State<'_, Desktop>) -> std::result::Result<(), String> {
    let track = lock(&state.0.snapshot)
        .map_err(error)?
        .playback
        .item
        .clone()
        .ok_or("Play a track first")?;
    state
        .0
        .commands
        .send(Command::FetchLyrics(track))
        .map_err(error)
}
/// Ask every source again for the current song, skipping saved answers,
/// so lyrics uploaded since show without waiting for the cache to expire.
#[tauri::command]
fn refresh_lyrics(
    keep_saved: Option<bool>,
    state: State<'_, Desktop>,
) -> std::result::Result<(), String> {
    let track = lock(&state.0.snapshot)
        .map_err(error)?
        .playback
        .item
        .clone()
        .ok_or("Play a track first")?;
    // The lyrics thread runs commands in order: the purge lands before the
    // fetch. Keeping the saved copy only reloads the song's lyrics.
    if !keep_saved.unwrap_or(false) {
        state
            .0
            .lyrics
            .send(LyricCommand::ForgetCached(track.clone()))
            .map_err(error)?;
    }
    state
        .0
        .commands
        .send(Command::FetchLyrics(track))
        .map_err(error)
}
/// Whether the user's own Musixmatch token is saved. The token itself never
/// leaves the backend.
#[tauri::command]
fn musixmatch_token_saved() -> bool {
    crate::lyric_sources::musixmatch_token_saved()
}
#[tauri::command]
fn save_musixmatch_token(token: String) -> std::result::Result<(), String> {
    crate::lyric_sources::save_musixmatch_token(&token).map_err(error)
}
#[tauri::command]
async fn renew_musixmatch_token() -> std::result::Result<bool, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let client = crate::auth::http_client()?;
        crate::lyric_sources::renew_musixmatch_token(&client).map(|()| true)
    })
    .await
    .map_err(|_| "Musixmatch token worker unavailable".to_owned())?
    .map_err(error)
}
#[tauri::command]
fn clear_lyrics_cache(state: State<'_, Desktop>) -> std::result::Result<(), String> {
    state
        .0
        .lyrics
        .send(LyricCommand::ClearCache)
        .map_err(error)?;
    // The playing song's lyrics are fetched again, after the purge.
    if let Some(track) = lock(&state.0.snapshot)
        .map_err(error)?
        .playback
        .item
        .clone()
    {
        state
            .0
            .commands
            .send(Command::FetchLyrics(track))
            .map_err(error)?;
    }
    Ok(())
}
#[tauri::command]
fn import_lyrics(
    filename: String,
    contents: String,
    mode: Option<crate::lyrics::ImportMode>,
    state: State<'_, Desktop>,
) -> std::result::Result<(), String> {
    if contents.len() > 2_000_000 {
        return Err("Lyric file exceeds 2 MB".into());
    }
    let track = lock(&state.0.snapshot)
        .map_err(error)?
        .playback
        .item
        .clone()
        .ok_or("Play a track first")?;
    state
        .0
        .lyrics
        .send(LyricCommand::ImportText {
            track,
            filename,
            contents,
            mode: mode.unwrap_or_default(),
        })
        .map_err(error)
}
/// Drop any imported lyrics for the playing song and load the provider's.
#[tauri::command]
fn reset_lyrics(state: State<'_, Desktop>) -> std::result::Result<(), String> {
    let track = lock(&state.0.snapshot)
        .map_err(error)?
        .playback
        .item
        .clone()
        .ok_or("Play a track first")?;
    state
        .0
        .lyrics
        .send(LyricCommand::Forget(track.clone()))
        .map_err(error)?;
    state
        .0
        .commands
        .send(Command::FetchLyrics(track))
        .map_err(error)
}
/// Host or join a Spotify Jam from this computer: `current`, `start`,
/// `join` (with a `link`), `end`, `leave`, or `control` (whether guests may change what's playing).
#[tauri::command]
async fn jam(
    action: String,
    id: Option<String>,
    allowed: Option<bool>,
    link: Option<String>,
    state: State<'_, Desktop>,
) -> std::result::Result<Option<crate::jam::Jam>, String> {
    let account = signed_in_account(&state).map_err(error)?;
    let controls = state.0.controls.scoped(&account).map_err(error)?;
    let inner = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // A Jam link from Spotify or Lyssna, short or long.
        let join = match (action.as_str(), link.as_deref()) {
            ("join", Some(link)) => Some(match crate::jam::short_link_code(link) {
                Some(code) => crate::jam::resolve_short_link(code)?,
                None => crate::jam::link_token(link).context("That link isn't a Jam")?,
            }),
            _ => None,
        };
        // Leaving stops this computer following the Jam's song changes at
        // once; joining lets them drive playback again.
        match action.as_str() {
            // A Jam ended here earlier left this computer ignoring Jam
            // transfers; a new one, hosted or joined, follows them again.
            "join" | "start" => controls.follow_jam(true),
            "leave" => controls.follow_jam(false),
            _ => {}
        }
        // Spotify ties a new Jam to the host's active device. Started while
        // Lyssna isn't the one playing, the Jam would follow another
        // device and songs changed here would never reach the guests.
        if action == "start" {
            let local = controls
                .session()
                .map(|session| session.device_id().to_owned());
            let active = controls.devices().ok().and_then(|devices| {
                devices
                    .into_iter()
                    .find(|device| device.is_active)
                    .and_then(|device| device.id)
            });
            if local.is_ok_and(|local| active.as_deref() != Some(local.as_str())) {
                let moved = controls.transfer("native").is_ok();
                crate::native::diagnostic(&format!("jam start: playback moved here={moved}"));
            }
        }
        let result = controls.run(async |session| match action.as_str() {
            "current" => crate::jam::current(session).await,
            "join" => crate::jam::join(session, join.as_deref().unwrap_or_default())
                .await
                .map(Some),
            "start" => crate::jam::start(session).await.map(Some),
            "end" => crate::jam::end(session, id.as_deref().unwrap_or_default())
                .await
                .map(|()| None),
            "leave" => crate::jam::leave(session, id.as_deref().unwrap_or_default())
                .await
                .map(|()| None),
            "control" => crate::jam::let_others_control(session, allowed.unwrap_or(true)).await,
            _ => anyhow::bail!("Unknown Jam action"),
        });
        // Still in the Jam if leaving failed.
        if action == "leave" && result.is_err() {
            controls.follow_jam(true);
        }
        // The host's player plays inside the Jam's session, so the Jam
        // service passes what it plays on to the guests.
        let hosted = match (action.as_str(), &result) {
            ("start" | "current", Ok(Some(jam))) if jam.is_owner => {
                Some(Some(jam.session_id.clone()))
            }
            ("current", Ok(None)) | ("end", Ok(_)) => Some(None),
            _ => None,
        };
        if let Some(session_id) = hosted {
            let hosting = session_id.is_some();
            let sent = controls.send(PlayerAction::HostJam(session_id)).is_ok();
            if action != "current" {
                crate::native::diagnostic(&format!("jam host: hosting={hosting} sent={sent}"));
            }
        }
        // Where play, pause, skips and song picks go while in the Jam.
        let route = match (action.as_str(), &result) {
            ("start" | "join" | "current" | "control", Ok(Some(jam))) => {
                Some((jam.redirects_commands(), Some(jam)))
            }
            ("current", Ok(None)) | ("end" | "leave", Ok(_)) => Some((false, None)),
            _ => None,
        };
        if let Some((redirect, jam)) = route
            && let Ok(mut current) = inner.jam_commands.lock()
        {
            let next = jam.filter(|_| redirect).map(|jam| jam.session_id.clone());
            if *current != next {
                let said = match jam.and_then(|jam| jam.redirect_commands) {
                    Some(true) => "yes",
                    Some(false) => "no",
                    None => "nothing",
                };
                crate::native::diagnostic(&format!(
                    "jam commands: through the Jam={} (Spotify said {said})",
                    next.is_some()
                ));
                *current = next;
            }
        }
        result
    })
    .await
    .map_err(|_| "Jam worker unavailable".to_owned())?
    .map_err(error)
}
/// A song's full credits, for the Credits dialog.
#[tauri::command]
async fn track_credits(
    id: String,
    state: State<'_, Desktop>,
) -> std::result::Result<crate::catalogue::TrackCredits, String> {
    let account = signed_in_account(&state).map_err(error)?;
    let controls = state.0.controls.scoped(&account).map_err(error)?;
    tauri::async_runtime::spawn_blocking(move || {
        controls.run(async |session| crate::catalogue::credits(session, &id).await)
    })
    .await
    .map_err(|_| "Credits worker unavailable".to_owned())?
    .map_err(error)
}
/// Every release of an artist, for the Discography page.
#[tauri::command]
async fn artist_discography(
    id: String,
    state: State<'_, Desktop>,
) -> std::result::Result<Vec<crate::model::Album>, String> {
    let account = signed_in_account(&state).map_err(error)?;
    let controls = state.0.controls.scoped(&account).map_err(error)?;
    tauri::async_runtime::spawn_blocking(move || controls.discography(&id))
        .await
        .map_err(|_| "Library worker unavailable".to_owned())?
        .map_err(error)
}
/// A release's songs and their play counts, for the Discography page.
#[tauri::command]
async fn release_tracks(
    id: String,
    state: State<'_, Desktop>,
) -> std::result::Result<(Vec<Track>, std::collections::HashMap<String, u64>), String> {
    let account = signed_in_account(&state).map_err(error)?;
    let controls = state.0.controls.scoped(&account).map_err(error)?;
    tauri::async_runtime::spawn_blocking(move || -> Result<_> {
        ensure!(
            id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()),
            "Invalid album ID"
        );
        // One request: the album's metadata carries its songs and plays.
        let entity = controls.catalogue("album", &id, 0)?;
        let streams = crate::album::parse(&entity).streams;
        let tracks = album_songs(&entity, &id);
        let total = entity["tracksV2"]["totalCount"]
            .as_u64()
            .unwrap_or_default() as usize;
        // Longer albums page their songs; the song list has them all.
        if tracks.is_empty() || tracks.len() < total {
            return Ok((
                controls.playlist_tracks(&format!("album:{id}"), 0)?,
                streams,
            ));
        }
        Ok((tracks, streams))
    })
    .await
    .map_err(|_| "Library worker unavailable".to_owned())?
    .map_err(error)
}
/// An album's songs from its metadata, each carrying the album.
fn album_songs(entity: &Value, id: &str) -> Vec<Track> {
    let album = crate::catalogue::base_info(entity, "album", id)
        .ok()
        .and_then(|info| info.album)
        .unwrap_or_default();
    entity["tracksV2"]["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| crate::search::native_track(&item["track"]))
        .map(|mut track| {
            track.album = album.clone();
            track
        })
        .collect()
}
fn signed_in_account(state: &State<'_, Desktop>) -> Result<String> {
    Ok(lock(&state.0.snapshot)?
        .profile
        .as_ref()
        .context("Sign in first")?
        .id
        .clone())
}
/// Build the latest Nightly from source; once it is installed Lyssna
/// follows Nightly and restarts into it. True once the build has started.
#[tauri::command]
fn nightly_build(
    state: State<'_, Desktop>,
    app: tauri::AppHandle,
) -> std::result::Result<bool, String> {
    let events = app.clone();
    let inner = state.0.clone();
    crate::nightly::start(
        crate::nightly::Mode::Install,
        move |report| {
            let _ = events.emit("lyssna", json!({"type":"nightly","report":report}));
        },
        move |outcome| {
            if !matches!(outcome, Ok(crate::nightly::Outcome::Installed)) {
                return;
            }
            // The new copy follows Nightly from its first start.
            if let Ok(mut snapshot) = inner.snapshot.lock() {
                snapshot.settings.update_channel = "nightly".into();
                if let Err(error) = snapshot.settings.save() {
                    log::warn!("Cannot save the Nightly channel: {error:#}");
                }
            }
            hand_over_playback(&inner);
            match updater::relaunch() {
                Ok(()) => app.exit(0),
                Err(error) => {
                    log::warn!("Nightly installed; restart Lyssna to use it: {error:#}")
                }
            }
        },
    )
    .map(|()| true)
    .map_err(error)
}
/// Before an update restarts Lyssna: save exactly where the song is, and
/// whether it is playing here, so the new copy carries on from that spot.
fn hand_over_playback(inner: &Inner) {
    let Ok(snapshot) = inner.snapshot.lock() else {
        return;
    };
    let Some(profile) = snapshot.profile.as_ref() else {
        return;
    };
    let playback = playback_now(&snapshot);
    let playing = playback.is_playing
        && playback.device.name == native::device_name()
        && playback.item.as_ref().is_some_and(|track| !track.is_local);
    if let Err(error) = crate::resume::save(&profile.id, &playback, now_ms(), true) {
        log::warn!("Cannot save the song before restarting: {error:#}");
    }
    updater::keep_playing(playing);
}
/// Playback with its position brought up to now: this computer's player
/// reports its position on changes, not continuously.
fn playback_now(snapshot: &Snapshot) -> crate::model::Playback {
    let mut playback = snapshot.playback.clone();
    if playback.is_playing && playback.device.name == native::device_name() {
        playback.progress_ms = Some(
            playback.progress_ms.unwrap_or_default()
                + now_ms().saturating_sub(snapshot.observed_at),
        );
    }
    playback
}
/// Whether this copy was restarted by an update mid-song and should play on,
/// once the song is restored; answers true only once.
#[tauri::command]
fn resume_after_restart(state: State<'_, Desktop>) -> bool {
    lock(&state.0.snapshot).is_ok_and(|snapshot| snapshot.playback.item.is_some())
        && updater::resumes_playing()
}
/// The commits a new Nightly brings, one per line.
fn nightly_notes() -> String {
    let changes = crate::nightly::changes();
    if changes.is_empty() {
        "New changes on main.".into()
    } else {
        changes
    }
}
/// Show what the Nightly watcher found as the update status, so the
/// update prompt opens for a new Nightly.
fn nightly_update_status(inner: &Inner, app: &tauri::AppHandle, status: crate::nightly::Status) {
    use crate::nightly::Status;
    let Ok(mut snapshot) = inner.snapshot.lock() else {
        return;
    };
    let name = |commit: &str| format!("Nightly {}", &commit[..7.min(commit.len())]);
    match status {
        Status::Current => {
            snapshot.update_status = "Up to date".into();
            snapshot.update_version = None;
            snapshot.update_notes.clear();
        }
        Status::Available(commit) => {
            snapshot.update_status = "Update available".into();
            snapshot.update_version = Some(name(&commit));
            snapshot.update_notes = nightly_notes();
        }
        Status::Building => snapshot.update_status = "Building update…".into(),
        Status::Ready(commit) => {
            snapshot.update_status = "Ready to install".into();
            snapshot.update_version = Some(name(&commit));
            snapshot.update_notes = nightly_notes();
        }
        Status::Message(message) => {
            snapshot.update_status = message;
            snapshot.update_version = None;
        }
    }
    let _ = app.emit(
        "lyssna",
        json!({"type":"update","status":snapshot.update_status,"version":snapshot.update_version,"notes":snapshot.update_notes}),
    );
}
/// Look for a newer Nightly now: "current", "available", "building" or
/// "ready", or why it couldn't look.
#[tauri::command]
async fn nightly_check() -> std::result::Result<String, String> {
    use crate::nightly::Status;
    let status = tauri::async_runtime::spawn_blocking(|| crate::nightly::check(false))
        .await
        .map_err(error)?
        .map_err(error)?;
    Ok(match status {
        Status::Current => "current".into(),
        Status::Available(_) => "available".into(),
        Status::Building => "building".into(),
        Status::Ready(_) => "ready".into(),
        Status::Message(message) => message,
    })
}
/// Free the space Nightly's builds take. Off the main thread: a build
/// folder holds many thousands of files.
#[tauri::command]
async fn nightly_clear_cache() -> std::result::Result<u64, String> {
    tauri::async_runtime::spawn_blocking(crate::nightly::clear_cache)
        .await
        .map_err(error)?
        .map_err(|error| format!("{error:#}"))
}
#[tauri::command]
fn nightly_cancel() {
    crate::nightly::cancel();
}
/// The commit this copy was built from, if it is Nightly, and the latest
/// build's progress.
#[tauri::command]
fn nightly_status() -> serde_json::Value {
    json!({"commit": crate::nightly::COMMIT, "report": crate::nightly::last()})
}
/// Open the latest Nightly build's full output.
#[tauri::command]
fn nightly_log() -> std::result::Result<(), String> {
    let path = crate::nightly::last()
        .and_then(|report| report.log)
        .ok_or_else(|| "No Nightly build has run yet".to_owned())?;
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg("-t");
        command
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = std::process::Command::new("notepad");
    command.arg(path).spawn().map(|_| ()).map_err(error)
}
/// Songs in Local Files, for Your Library's row.
#[tauri::command]
fn local_files_count() -> usize {
    crate::local_files::count()
}
/// A folder for Local Files, chosen in the system's folder picker; `None`
/// when the picker was closed.
#[tauri::command]
async fn pick_local_folder(
    window: tauri::WebviewWindow,
) -> std::result::Result<Option<String>, String> {
    // The picker sits on Lyssna's window, so it opens in front of it.
    #[cfg(windows)]
    let owner = window
        .hwnd()
        .map(|hwnd| hwnd.0 as isize)
        .unwrap_or_default();
    #[cfg(not(windows))]
    let owner = {
        let _ = window;
        0
    };
    tauri::async_runtime::spawn_blocking(move || pick_folder(owner))
        .await
        .map_err(|_| "Folder picker unavailable".to_owned())?
        .map(|path| path.map(|path| path.display().to_string()))
        .map_err(error)
}
#[cfg(windows)]
fn pick_folder(owner: isize) -> Result<Option<std::path::PathBuf>> {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    let result = show_folder_picker(owner);
    if initialized {
        unsafe { CoUninitialize() };
    }
    result
}
#[cfg(windows)]
fn show_folder_picker(owner: isize) -> Result<Option<std::path::PathBuf>> {
    use windows::Win32::{
        Foundation::HWND,
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree},
        UI::Shell::{
            FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog,
            SIGDN_FILESYSPATH,
        },
    };
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        dialog.SetOptions(dialog.GetOptions()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)?;
        // Closing the picker is an error from Show; it chose nothing.
        if dialog.Show(Some(HWND(owner as *mut _))).is_err() {
            return Ok(None);
        }
        let name = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
        let path = name.to_string();
        CoTaskMemFree(Some(name.0 as *const _));
        Ok(Some(path?.into()))
    }
}
#[cfg(target_os = "macos")]
fn pick_folder(_owner: isize) -> Result<Option<std::path::PathBuf>> {
    let output = std::process::Command::new("osascript")
        .args([
            "-e",
            "POSIX path of (choose folder with prompt \"Add a source\")",
        ])
        .output()?;
    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((output.status.success() && !path.is_empty()).then(|| path.into()))
}
#[cfg(not(any(windows, target_os = "macos")))]
fn pick_folder(_owner: isize) -> Result<Option<std::path::PathBuf>> {
    anyhow::bail!("Folder picking is not available on this system")
}
/// Songs in Liked Songs, for its header count and total length.
#[tauri::command]
async fn liked_songs_count(state: State<'_, Desktop>) -> std::result::Result<usize, String> {
    let account = lock(&state.0.snapshot)
        .map_err(error)?
        .profile
        .as_ref()
        .context("Sign in first")
        .map_err(error)?
        .id
        .clone();
    let controls = state.0.controls.scoped(&account).map_err(error)?;
    tauri::async_runtime::spawn_blocking(move || controls.liked_count())
        .await
        .map_err(|_| "Library worker unavailable".to_owned())?
        .map_err(error)
}
/// Clipboard text, read natively: the webview refuses clipboard reads
/// unless its document has focus.
#[tauri::command]
async fn read_clipboard() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(|| {
        arboard::Clipboard::new()
            .and_then(|mut clipboard| clipboard.get_text())
            .map_err(|_| "Clipboard has no preset code.".to_owned())
    })
    .await
    .map_err(|error| error.to_string())?
}
/// Exact song count and total length of a playlist or Liked Songs.
#[tauri::command]
async fn list_duration(
    key: String,
    state: State<'_, Desktop>,
) -> std::result::Result<(usize, u64), String> {
    let account = lock(&state.0.snapshot)
        .map_err(error)?
        .profile
        .as_ref()
        .context("Sign in first")
        .map_err(error)?
        .id
        .clone();
    let controls = state.0.controls.scoped(&account).map_err(error)?;
    tauri::async_runtime::spawn_blocking(move || controls.list_duration(&key))
        .await
        .map_err(|_| "Library worker unavailable".to_owned())?
        .map_err(error)
}
/// Songs the user liked by an artist, for the artist page's "You Liked".
#[tauri::command]
async fn artist_liked_count(
    id: String,
    state: State<'_, Desktop>,
) -> std::result::Result<usize, String> {
    let account = lock(&state.0.snapshot)
        .map_err(error)?
        .profile
        .as_ref()
        .context("Sign in first")
        .map_err(error)?
        .id
        .clone();
    let controls = state.0.controls.scoped(&account).map_err(error)?;
    tauri::async_runtime::spawn_blocking(move || controls.liked_by_artist(&id))
        .await
        .map_err(|_| "Library worker unavailable".to_owned())?
        .map_err(error)
}
#[tauri::command]
async fn saved_lyrics(
    state: State<'_, Desktop>,
) -> std::result::Result<Vec<crate::lyrics::SavedLyrics>, String> {
    let account = lock(&state.0.snapshot)
        .map_err(error)?
        .profile
        .as_ref()
        .context("Sign in first")
        .map_err(error)?
        .id
        .clone();
    let controls = state.0.controls.scoped(&account).map_err(error)?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut saved = crate::lyrics::saved_imports()?;
        // Older imports lack album art; look their songs up once and keep it.
        let missing: Vec<String> = saved
            .iter()
            .filter(|item| item.image.is_empty() && item.uri.starts_with("spotify:track:"))
            .map(|item| item.uri.clone())
            .take(50)
            .collect();
        if !missing.is_empty()
            && let Ok(tracks) = controls.resolve(&missing)
        {
            crate::lyrics::complete_saved(&mut saved, &tracks);
        }
        Ok::<_, anyhow::Error>(saved)
    })
    .await
    .map_err(|_| "Saved lyrics are unavailable".to_owned())?
    .map_err(error)
}
/// A saved lyric file and its download name.
#[tauri::command]
fn saved_lyrics_file(key: String) -> std::result::Result<(String, String), String> {
    crate::lyrics::saved_file(&key).map_err(error)
}
#[tauri::command]
fn delete_saved_lyrics(key: String, state: State<'_, Desktop>) -> std::result::Result<(), String> {
    crate::lyrics::delete_saved(&key).map_err(error)?;
    // Reload the playing song's lyrics if its import was the one removed.
    if let Some(track) = lock(&state.0.snapshot)
        .map_err(error)?
        .playback
        .item
        .clone()
    {
        let _ = state.0.commands.send(Command::FetchLyrics(track));
    }
    Ok(())
}
#[tauri::command]
fn add_tracks(
    playlist: Playlist,
    tracks: Vec<Track>,
    state: State<'_, Desktop>,
) -> std::result::Result<(), String> {
    state
        .0
        .commands
        .send(Command::Add { playlist, tracks })
        .map_err(error)
}
#[tauri::command]
fn saved_status(uris: Vec<String>, state: State<'_, Desktop>) -> std::result::Result<(), String> {
    if uris.len() > 500 {
        return Err("Too many library items".into());
    }
    state
        .0
        .commands
        .send(Command::SavedStatus(uris))
        .map_err(error)
}
#[tauri::command]
fn create_folder(name: String, state: State<'_, Desktop>) -> std::result::Result<(), String> {
    if lock(&state.0.snapshot).map_err(error)?.profile.is_none() {
        return Err("Sign in first".into());
    }
    state
        .0
        .commands
        .send(Command::CreateFolder(name))
        .map_err(error)
}
#[tauri::command]
fn move_to_folder(
    playlist: String,
    folder: Option<String>,
    state: State<'_, Desktop>,
) -> std::result::Result<(), String> {
    if lock(&state.0.snapshot).map_err(error)?.profile.is_none() {
        return Err("Sign in first".into());
    }
    state
        .0
        .commands
        .send(Command::MoveToFolder { playlist, folder })
        .map_err(error)
}
#[tauri::command]
fn rename_folder(
    id: String,
    name: String,
    state: State<'_, Desktop>,
) -> std::result::Result<(), String> {
    if lock(&state.0.snapshot).map_err(error)?.profile.is_none() {
        return Err("Sign in first".into());
    }
    state
        .0
        .commands
        .send(Command::RenameFolder { id, name })
        .map_err(error)
}
#[tauri::command]
fn delete_folder(id: String, state: State<'_, Desktop>) -> std::result::Result<(), String> {
    if lock(&state.0.snapshot).map_err(error)?.profile.is_none() {
        return Err("Sign in first".into());
    }
    state
        .0
        .commands
        .send(Command::DeleteFolder(id))
        .map_err(error)
}
#[tauri::command]
fn edit_playlist(
    playlist: Playlist,
    name: String,
    description: String,
    cover: Option<String>,
    state: State<'_, Desktop>,
) -> std::result::Result<(), String> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    if lock(&state.0.snapshot).map_err(error)?.profile.is_none() {
        return Err("Sign in first".into());
    }
    // The window sends the cropped JPEG as base64; 4 MB of image at most.
    let cover = cover
        .map(|data| {
            (data.len() <= 6 * 1024 * 1024)
                .then(|| STANDARD.decode(data).ok())
                .flatten()
                .ok_or("Choose a smaller image")
        })
        .transpose()?;
    state
        .0
        .commands
        .send(Command::EditPlaylist {
            playlist,
            name,
            description,
            cover,
        })
        .map_err(error)
}
#[tauri::command]
fn move_playlist_song(
    playlist: Playlist,
    from: usize,
    uri: String,
    to: usize,
    state: State<'_, Desktop>,
) -> std::result::Result<(), String> {
    if lock(&state.0.snapshot).map_err(error)?.profile.is_none() {
        return Err("Sign in first".into());
    }
    state
        .0
        .commands
        .send(Command::MovePlaylistSong {
            playlist,
            from,
            uri,
            to,
        })
        .map_err(error)
}
#[tauri::command]
fn remove_from_playlist(
    playlist: Playlist,
    index: usize,
    uri: String,
    state: State<'_, Desktop>,
) -> std::result::Result<(), String> {
    if lock(&state.0.snapshot).map_err(error)?.profile.is_none() {
        return Err("Sign in first".into());
    }
    state
        .0
        .commands
        .send(Command::RemoveFromPlaylist {
            playlist,
            index,
            uri,
        })
        .map_err(error)
}
#[tauri::command]
fn remove_playlist(
    id: String,
    name: String,
    state: State<'_, Desktop>,
) -> std::result::Result<(), String> {
    if lock(&state.0.snapshot).map_err(error)?.profile.is_none() {
        return Err("Sign in first".into());
    }
    state
        .0
        .commands
        .send(Command::RemovePlaylist { id, name })
        .map_err(error)
}
#[tauri::command]
fn create_playlist(
    name: String,
    tracks: Vec<Track>,
    state: State<'_, Desktop>,
) -> std::result::Result<(), String> {
    if lock(&state.0.snapshot).map_err(error)?.profile.is_none() {
        return Err("Sign in first".into());
    }
    if tracks.len() > 10_000 {
        return Err("Choose fewer songs".into());
    }
    state
        .0
        .commands
        .send(Command::CreatePlaylist { name, tracks })
        .map_err(error)
}
/// WebView2 switches, the same for every window since they share one
/// browser. wry's defaults, plus: after three GPU process crashes, as a
/// driver reset during a game causes, Chromium falls back to software
/// rendering until restart, dropping lyrics to a few dozen frames a
/// second. Without the limit it brings hardware rendering back each time.
const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --disable-gpu-process-crash-limit";

/// Runs in creator profile windows, which show the full `/{name}` profile
/// page now that the site retires its `/embed/` pages. Hides scrollbars;
/// wheel and touch scrolling still work. The page's banner would cover the
/// top of the sheet, so it is hidden. The site drops its header for a
/// profile opened with `_state={"mp":true}`, as Spicy Lyrics opens it; a
/// profile reached without it, after the site's verification check or a
/// `/uid/` link, is opened again with it.
const PROFILE_WINDOW_SCRIPT: &str = r#"(() => {
  const fonts = document.createElement("style");
  fonts.textContent = PROFILE_FONTS;
  const addFonts = () => (document.head || document.documentElement).appendChild(fonts);
  if (document.documentElement) addFonts();
  else document.addEventListener("DOMContentLoaded", addFonts, { once: true });
  const style = document.createElement("style");
  style.textContent = "*{scrollbar-width:none!important}::-webkit-scrollbar{display:none!important;width:0!important;height:0!important}" +
    "[data-profile-variant]{--profile-banner-h:5rem!important}" +
    "[data-profile-variant]>[aria-hidden=true][class*=profile-banner-h]{display:none!important}" +
    // Spotify's profile header: avatar beside the name, the views one line under it.
    ".profile-hero{min-height:0!important;overflow:visible!important}" +
    ".profile-hero-identity>header>div>div:empty{display:none}" +
    ".profile-hero-identity{grid-template-columns:auto minmax(0,1fr)!important;column-gap:1.75rem!important;row-gap:.875rem!important;align-items:end!important;padding-top:2.5rem!important;padding-bottom:2rem!important}" +
    ".profile-hero-identity>header{display:contents!important}" +
    ".profile-hero-identity .profile-avatar{grid-column:1;grid-row:1/span 2;align-self:center;width:clamp(6rem,17vw,11.5rem)!important;height:clamp(6rem,17vw,11.5rem)!important}" +
    ".profile-hero-identity>header>div{grid-column:2;grid-row:1;gap:.5rem!important}" +
    ".profile-hero-identity>header>div::before{content:'Profile';font-size:.875rem;font-weight:600}" +
    ".profile-name{font-size:clamp(2rem,calc(var(--name-vw,9vw)*.7),5.5rem)!important;max-width:none!important;font-weight:800!important;letter-spacing:-.02em!important}" +
    ".profile-hero-identity>section{grid-column:2;grid-row:2;flex-direction:row!important;flex-wrap:wrap;align-items:center!important;gap:.375rem .875rem!important}" +
    ".profile-hero-identity>section>*{margin:0!important;line-height:1!important}" +
    ".profile-hero-identity>section dd,.profile-hero-identity>section dt{line-height:1!important}" +
    ".profile-hero-identity>section .stats-label{font-size:.6875rem}" +
    ".profile-hero-identity>section dl{display:flex!important;flex-wrap:wrap;align-items:baseline;gap:.625rem!important;text-align:left!important}" +
    ".profile-hero-identity>section dl>div{flex-direction:row-reverse!important;align-items:baseline;gap:.3rem!important}" +
    ".profile-hero-identity>section dl>div+div::after{content:'\\2022';margin-right:.3rem;color:var(--muted-foreground)}" +
    ".profile-hero-identity .profile-figure{font-size:1rem!important;letter-spacing:0!important}" +
    // The counter reserves its widest figure; at this size it only leaves a gap.
    ".profile-hero-identity .profile-figure .invisible{display:none!important}" +
    ".profile-stats-cta{padding:.375rem .875rem!important;font-size:.8125rem!important}" +
    ".profile-aura{left:7rem!important;top:50%!important}" +
    ".profile-spotlight{display:none!important}" +
    // The track list's number column hugs the edge.
    ".profile-setlist>.grid:first-child,.profile-track{padding-left:.25rem!important;column-gap:.75rem!important}" +
    ".profile-setlist>.grid:first-child>:first-child,.profile-track>:first-child{text-align:center!important}" +
    "@media (min-width:640px){.profile-variant{margin-left:4.25rem!important}}" +
    // The tabs and search bar stick a little below the sheet's edge.
    ".profile-dock{top:.75rem!important}";
  const add = () => (document.head || document.documentElement).appendChild(style);
  if (document.documentElement) add();
  else document.addEventListener("DOMContentLoaded", add, { once: true });
  // The site eases wheel scrolling (Lenis) only for a mouse; reporting none
  // keeps the window's own direct scrolling.
  const matchMedia = window.matchMedia.bind(window);
  window.matchMedia = (query) =>
    query.replace(/\s+/g, "") === "(hover:hover)and(pointer:fine)"
      ? matchMedia("not all")
      : matchMedia(query);
  // Listen starts the song on press rather than release. The song comes
  // from the row's React props, as the button's own click handler reads it.
  const listenTrack = (target) => {
    const button = target instanceof Element && target.closest("button");
    if (!button || !/^\s*listen\s*$/i.test(button.getAttribute("aria-label") || button.textContent || "")) return null;
    const key = Object.keys(button).find((name) => name.startsWith("__reactFiber$"));
    for (let fiber = key && button[key], depth = 0; fiber && depth < 12; fiber = fiber.return, depth++) {
      const id = fiber.memoizedProps?.trackId;
      if (typeof id === "string" && /^[A-Za-z0-9]{22}$/.test(id)) return { button, id };
    }
    return null;
  };
  let pressed = null;
  addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    const listen = listenTrack(event.target);
    if (!listen) return;
    pressed = { button: listen.button, at: performance.now() };
    location.href = "spotify:track:" + listen.id;
  }, true);
  addEventListener("click", (event) => {
    const listen = pressed;
    pressed = null;
    if (!listen || performance.now() - listen.at > 2000 || !listen.button.contains(event.target)) return;
    event.preventDefault();
    event.stopImmediatePropagation();
  }, true);
  const routes = new Set(["app", "authcycle", "embed", "goog", "guides", "identity", "legal", "stats", "uid"]);
  const chromeless = () => {
    const name = location.pathname.match(/^\/([^/]+)\/?$/)?.[1];
    const query = new URLSearchParams(location.search);
    if (name && !routes.has(name.toLowerCase()) && query.get("_state") !== '{"mp":true}') {
      query.set("_state", '{"mp":true}');
      location.replace("/" + name + "?" + query);
    }
  };
  for (const method of ["pushState", "replaceState"]) {
    const original = history[method];
    history[method] = function (...args) {
      const result = original.apply(this, args);
      chromeless();
      return result;
    };
  }
  addEventListener("popstate", chromeless);
  chromeless();
})();"#;
/// Rounds a borderless window's corners wider than Windows' own 8 px.
#[cfg(windows)]
fn round_corners(
    window: &tauri::WebviewWindow,
    width: i32,
    height: i32,
    radius: i32,
) -> Result<()> {
    use windows::Win32::{
        Foundation::HWND,
        Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn},
    };
    let hwnd = HWND(window.hwnd()?.0 as _);
    // The window owns the region once set.
    unsafe {
        let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, radius * 2, radius * 2);
        if SetWindowRgn(hwnd, Some(region), true) == 0 {
            anyhow::bail!("Could not round the window's corners");
        }
    }
    Ok(())
}
/// The profile script with Lyssna's Metropolis in place of the site's
/// typefaces. The page is remote, so the faces travel inside the script.
fn profile_window_script() -> &'static str {
    static SCRIPT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let faces: [(u16, &[u8]); 5] = [
            (
                400,
                include_bytes!("../assets/fonts/metropolis-latin-400-normal.woff2"),
            ),
            (
                500,
                include_bytes!("../assets/fonts/metropolis-latin-500-normal.woff2"),
            ),
            (
                600,
                include_bytes!("../assets/fonts/metropolis-latin-600-normal.woff2"),
            ),
            (
                700,
                include_bytes!("../assets/fonts/metropolis-latin-700-normal.woff2"),
            ),
            (
                800,
                include_bytes!("../assets/fonts/metropolis-latin-800-normal.woff2"),
            ),
        ];
        let mut css = String::new();
        for (weight, font) in faces {
            css += &format!(
                "@font-face{{font-family:LyssnaMetropolis;font-weight:{weight};font-display:block;src:url(data:font/woff2;base64,{}) format('woff2')}}",
                STANDARD.encode(font)
            );
        }
        css += "body,body *:not(svg):not(svg *){font-family:LyssnaMetropolis,\"Segoe UI\",sans-serif!important}";
        PROFILE_WINDOW_SCRIPT.replacen(
            "PROFILE_FONTS",
            &serde_json::Value::String(css).to_string(),
            1,
        )
    });
    &SCRIPT
}
/// Cinema Lyrics, as the pixels-spicy-lyrics fork's: the full screen lyrics
/// in a normal window of their own, framed so it moves, resizes and maximizes
/// like any other, and remembered where it was last left. The button toggles
/// it, as the fork's does.
#[tauri::command]
async fn toggle_cinema_lyrics(app: tauri::AppHandle) -> std::result::Result<(), String> {
    const LABEL: &str = "cinema";
    let result = || -> Result<()> {
        if let Some(window) = app.get_webview_window(LABEL) {
            window.close()?;
            return Ok(());
        }
        tauri::WebviewWindowBuilder::new(
            &app,
            LABEL,
            tauri::WebviewUrl::App("index.html?view=cinema".into()),
        )
        .title("Lyssna Cinema")
        .inner_size(1280.0, 720.0)
        .min_inner_size(640.0, 360.0)
        .theme(Some(tauri::Theme::Dark))
        .background_color(tauri::window::Color(0, 0, 0, 0xff))
        .additional_browser_args(BROWSER_ARGS)
        .build()?;
        Ok(())
    };
    result().map_err(error)
}
/// Spicy Lyrics' popup lyrics: the lyrics in their own small window that
/// stays above others, opened again from where it was last closed. The page
/// is Lyssna's own, in its lyrics popout view.
#[tauri::command]
async fn open_lyrics_popout(app: tauri::AppHandle) -> std::result::Result<(), String> {
    const LABEL: &str = "lyrics-popout";
    let result = || -> Result<()> {
        if let Some(window) = app.get_webview_window(LABEL) {
            window.unminimize()?;
            window.set_focus()?;
            return Ok(());
        }
        tauri::WebviewWindowBuilder::new(
            &app,
            LABEL,
            tauri::WebviewUrl::App("index.html?view=lyrics-popout".into()),
        )
        .title("Lyssna Lyrics")
        // Frameless like Spicy Lyrics' popup; the page draws its own close
        // button and drags the window.
        .decorations(false)
        .shadow(true)
        .inner_size(900.0, 640.0)
        .min_inner_size(360.0, 260.0)
        .always_on_top(true)
        .theme(Some(tauri::Theme::Dark))
        .background_color(tauri::window::Color(0x12, 0x12, 0x12, 0xff))
        .additional_browser_args(BROWSER_ARGS)
        .build()?;
        Ok(())
    };
    result().map_err(error)
}
#[tauri::command]
async fn open_creator_profile(
    app: tauri::AppHandle,
    caller: tauri::WebviewWindow,
    username: String,
    user_id: Option<String>,
) -> std::result::Result<(), String> {
    let resolved = match user_id.filter(|id| !id.is_empty()) {
        Some(id) => {
            tauri::async_runtime::spawn_blocking(move || crate::provider::creator_profile(&id))
                .await
                .map_err(anyhow::Error::from)
                .and_then(|found| found)
        }
        None => creator_profile_url(&username),
    };
    let result = || -> Result<()> {
        use sha2::{Digest, Sha256};
        let url = resolved?;
        let digest = Sha256::digest(url.as_str());
        let label = format!(
            "creator-{}",
            digest
                .iter()
                .take(12)
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        if let Some(window) = app.get_webview_window(&label) {
            // A sheet left hidden by a failed open would swallow every
            // later click; show it rather than only focusing it.
            window.show()?;
            window.set_focus()?;
            return Ok(());
        }
        // One profile at a time.
        for (name, window) in app.webview_windows() {
            if name.starts_with("creator-") {
                let _ = window.close();
            }
        }
        let title = format!(
            "{} · Spicy Lyrics",
            username
                .chars()
                .filter(|character| !character.is_control())
                .take(80)
                .collect::<String>()
        );
        // A borderless sheet fixed over the window that asked for it (the
        // main window dims behind it); clicking anywhere else closes it.
        // Owned by the caller so it opens above the always-on-top lyrics
        // windows instead of behind them, where it lost focus and closed.
        let main = caller;
        let listens = app.clone();
        let window =
            tauri::WebviewWindowBuilder::new(&app, label, tauri::WebviewUrl::External(url))
                .title(title)
                .decorations(false)
                .resizable(false)
                .skip_taskbar(true)
                // Its rounded region hides the shadow; the shadow's frame
                // would still draw a light edge.
                .shadow(false)
                .visible(false)
                .parent(&main)?
                .additional_browser_args(BROWSER_ARGS)
                .initialization_script(profile_window_script())
                .on_navigation(move |url| {
                    // Listen plays the song here instead of leaving the page.
                    if let Some(uri) = listen_uri(url) {
                        let _ = listens.emit("lyssna", json!({"type":"play_uri","uri":uri}));
                        return false;
                    }
                    url.scheme() == "https" && url.host_str() == Some("spicylyrics.org")
                })
                .build()?;
        let placed = || -> Result<()> {
            let (origin, size, scale) = (
                main.inner_position()?,
                main.inner_size()?,
                main.scale_factor()?,
            );
            // 1080 px keeps the profile page in its two-column layout.
            let width = (f64::from(size.width) * 0.9).min(1080. * scale);
            let height = (f64::from(size.height) * 0.9).min(800. * scale);
            window.set_size(tauri::PhysicalSize::new(width, height))?;
            #[cfg(windows)]
            round_corners(&window, width as i32, height as i32, (18. * scale) as i32)?;
            window.set_position(tauri::PhysicalPosition::new(
                f64::from(origin.x) + (f64::from(size.width) - width) / 2.,
                f64::from(origin.y) + (f64::from(size.height) - height) / 2.,
            ))?;
            Ok(())
        };
        if let Err(problem) = placed() {
            let _ = window.close();
            return Err(problem);
        }
        let _ = app.emit("lyssna", json!({"type":"creator_profile","open":true}));
        let events = app.clone();
        let popup = window.clone();
        window.on_window_event(move |event| match event {
            tauri::WindowEvent::Focused(false) => {
                let _ = popup.close();
            }
            tauri::WindowEvent::Destroyed => {
                let open = events
                    .webview_windows()
                    .keys()
                    .any(|name| name.starts_with("creator-") && name != popup.label());
                let _ = events.emit("lyssna", json!({"type":"creator_profile","open":open}));
            }
            _ => {}
        });
        window.show()?;
        window.set_focus()?;
        Ok(())
    };
    result().map_err(|problem| {
        crate::native::diagnostic(&format!("creator profile failed: {problem}"));
        error(problem)
    })
}
/// A Spotify user's profile page, read through this account's native session.
#[tauri::command]
async fn user_profile(
    state: State<'_, Desktop>,
    user: String,
) -> std::result::Result<crate::user_profile::UserProfilePage, String> {
    if !crate::user_profile::valid_user(&user) {
        return Err("Invalid Spotify profile".into());
    }
    let account = lock(&state.0.snapshot)
        .map_err(error)?
        .profile
        .as_ref()
        .context("Sign in first")
        .map_err(error)?
        .id
        .clone();
    let controls = state.0.controls.scoped(&account).map_err(error)?;
    tauri::async_runtime::spawn_blocking(move || {
        controls.run(async |session| crate::user_profile::fetch(session, &user).await)
    })
    .await
    .map_err(error)?
    .map_err(|_| "Profile unavailable. Try again shortly.".to_owned())
}
/// Saved albums and followed artists, newest first, for Your Library.
#[tauri::command]
async fn library_items(
    state: State<'_, Desktop>,
) -> std::result::Result<crate::library_items::LibraryItems, String> {
    let account = lock(&state.0.snapshot)
        .map_err(error)?
        .profile
        .as_ref()
        .context("Sign in first")
        .map_err(error)?
        .id
        .clone();
    let controls = state.0.controls.scoped(&account).map_err(error)?;
    tauri::async_runtime::spawn_blocking(move || controls.run(crate::library_items::fetch))
        .await
        .map_err(error)?
        .map_err(|_| "Albums and artists are unavailable. Try again shortly.".to_owned())
}
/// New releases from followed artists, as Spotify's What's New panel.
#[tauri::command]
async fn whats_new(
    state: State<'_, Desktop>,
) -> std::result::Result<Vec<crate::whats_new::Release>, String> {
    let account = lock(&state.0.snapshot)
        .map_err(error)?
        .profile
        .as_ref()
        .context("Sign in first")
        .map_err(error)?
        .id
        .clone();
    let controls = state.0.controls.scoped(&account).map_err(error)?;
    tauri::async_runtime::spawn_blocking(move || controls.run(crate::whats_new::fetch))
        .await
        .map_err(error)?
        .map_err(|_| "What's New is unavailable. Try again shortly.".to_owned())
}
/// Follow or unfollow a Spotify user; returns the confirmed state.
#[tauri::command]
async fn follow_user(
    state: State<'_, Desktop>,
    user: String,
    follow: bool,
) -> std::result::Result<bool, String> {
    if !crate::user_profile::valid_user(&user) {
        return Err("Invalid Spotify profile".into());
    }
    let account = lock(&state.0.snapshot)
        .map_err(error)?
        .profile
        .as_ref()
        .context("Sign in first")
        .map_err(error)?
        .id
        .clone();
    let controls = state.0.controls.scoped(&account).map_err(error)?;
    tauri::async_runtime::spawn_blocking(move || {
        controls
            .run(async |session| crate::user_profile::set_following(session, &user, follow).await)
    })
    .await
    .map_err(error)?
    .map_err(|_| {
        if follow {
            "Could not follow this profile. Try again shortly.".to_owned()
        } else {
            "Could not unfollow this profile. Try again shortly.".to_owned()
        }
    })
}
/// The next page of one search category, for "Load more".
#[tauri::command]
async fn search_page(
    state: State<'_, Desktop>,
    query: String,
    kind: String,
    offset: usize,
) -> std::result::Result<crate::search::SearchPage, String> {
    let account = lock(&state.0.snapshot)
        .map_err(error)?
        .profile
        .as_ref()
        .context("Sign in first")
        .map_err(error)?
        .id
        .clone();
    let controls = state.0.controls.scoped(&account).map_err(error)?;
    tauri::async_runtime::spawn_blocking(move || {
        controls.run(async |session| crate::search::page(session, &query, &kind, offset).await)
    })
    .await
    .map_err(error)?
    .map_err(|_| "More results are unavailable right now. Try again shortly.".to_owned())
}
#[tauri::command]
async fn resolve_album(
    state: State<'_, Desktop>,
    uri: String,
) -> std::result::Result<crate::model::Album, String> {
    let account = lock(&state.0.snapshot)
        .map_err(error)?
        .profile
        .as_ref()
        .context("Sign in first")
        .map_err(error)?
        .id
        .clone();
    let controls = state.0.controls.scoped(&account).map_err(error)?;
    if lock(&state.0.snapshot).map_err(error)?.profile.is_none() {
        return Err("Sign in first".into());
    }
    tauri::async_runtime::spawn_blocking(move || controls.track(&uri).map(|track| track.album))
        .await
        .map_err(error)?
        .map_err(|_| "Album metadata unavailable. Try again shortly.".to_owned())
}
/// The Spotify song a profile's Listen button opens, as a track URI.
fn listen_uri(url: &url::Url) -> Option<String> {
    let id = match url.scheme() {
        "spotify" => url.path().strip_prefix("track:")?,
        "https" if url.host_str() == Some("open.spotify.com") => {
            url.path().strip_prefix("/track/")?.trim_end_matches('/')
        }
        _ => return None,
    };
    (id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        .then(|| format!("spotify:track:{id}"))
}
fn creator_profile_url(username: &str) -> Result<url::Url> {
    let name = username.trim().trim_start_matches('@');
    ensure!(
        !name.is_empty()
            && name.chars().count() <= 80
            && !name
                .chars()
                .any(|character| character.is_control() || character == '/' || character == '\\')
            && !matches!(name, "." | ".."),
        "Invalid creator username"
    );
    let mut url = url::Url::parse("https://spicylyrics.org/")?;
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("Invalid profile path"))?
        .pop_if_empty()
        .push(name);
    url.query_pairs_mut()
        .append_pair("_state", crate::provider::PROFILE_WITHOUT_HEADER);
    Ok(url)
}

/// Spotify and lyrics pages, plus the credited projects' repositories.
fn allowed_link(url: &url::Url) -> bool {
    /// Profiles and repositories named in Settings > Credits and licences.
    const CREDITED: [&str; 8] = [
        "/Stormanzanii",
        "/iPixelGalaxy",
        "/Spikerko",
        "/iPixelGalaxy/spotify-plus",
        "/iPixelGalaxy/spicy-lyrics",
        "/librespot-org/librespot",
        "/Fraktality/spr",
        "/Stormanzanii/Lyssna",
    ];
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && match url.host_str() {
            Some(
                "open.spotify.com"
                | "www.spotify.com"
                | "developer.spotify.com"
                | "developers.spicylyrics.org"
                | "spicylyrics.org"
                | "lyrprep.spicylyrics.org",
            ) => true,
            Some("github.com") => {
                CREDITED.contains(&url.path()) && url.query().is_none() && url.fragment().is_none()
            }
            _ => false,
        }
}
#[tauri::command]
fn open_link(url: String) -> std::result::Result<(), String> {
    let parsed = url::Url::parse(&url).map_err(|_| "Invalid link")?;
    if !allowed_link(&parsed) {
        return Err("Link is not an allowed Spotify or lyrics page".into());
    }
    webbrowser::open(parsed.as_str())
        .map(|_| ())
        .map_err(|_| "Cannot open browser".into())
}
/// The window's restore rectangle from before full screen, when it was
/// maximized; `Some(None)` off Windows. `None` when full screen did not start
/// from a maximized window.
static FULLSCREEN_RESTORE: Mutex<Option<Option<[i32; 4]>>> = Mutex::new(None);

/// Enters or leaves full screen. tao keeps a maximized borderless window
/// inside the work area, so a maximized window has to be restored first,
/// which shrank it to its restore size and grew it again. On Windows the
/// restore rectangle is set to the work area for the switch, so restoring
/// and maximizing do not move the window; it only grows over the taskbar.
/// Opens the WebView's developer tools, only while Developer mode is on.
#[tauri::command]
fn open_devtools(
    state: State<'_, Desktop>,
    window: tauri::WebviewWindow,
) -> std::result::Result<(), String> {
    if !lock(&state.0.snapshot)
        .map_err(error)?
        .settings
        .developer_mode
    {
        return Err("Turn on Developer mode in Settings > Advanced first".into());
    }
    window.open_devtools();
    Ok(())
}
#[tauri::command]
async fn window_fullscreen(
    enabled: bool,
    window: tauri::WebviewWindow,
) -> std::result::Result<(), String> {
    let result = || -> Result<()> {
        if enabled == window.is_fullscreen()? {
            return Ok(());
        }
        if enabled {
            let restore = if window.is_maximized()? {
                #[cfg(windows)]
                let previous = Some(fullscreen_placement::restore_to_work_area(&window)?);
                #[cfg(not(windows))]
                let previous = None;
                window.unmaximize()?;
                Some(previous)
            } else {
                None
            };
            *lock(&FULLSCREEN_RESTORE)? = restore;
            window.set_fullscreen(true)?;
        } else {
            window.set_fullscreen(false)?;
            if let Some(previous) = lock(&FULLSCREEN_RESTORE)?.take() {
                window.maximize()?;
                #[cfg(windows)]
                if let Some(rect) = previous {
                    // Queued behind the maximize, so it lands once the window
                    // is maximized again and only changes where Restore goes.
                    let target = window.clone();
                    window.run_on_main_thread(move || {
                        let _ = fullscreen_placement::set_restore(&target, rect);
                    })?;
                }
                #[cfg(not(windows))]
                let _ = previous;
            }
        }
        Ok(())
    };
    result().map_err(error)
}

#[path = "stream_deck.rs"]
mod stream_deck;

#[cfg(windows)]
mod fullscreen_placement {
    use anyhow::Result;
    use windows::Win32::{
        Foundation::{HWND, RECT},
        Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        },
        UI::WindowsAndMessaging::{GetWindowPlacement, SetWindowPlacement, WINDOWPLACEMENT},
    };

    fn placement(window: &tauri::WebviewWindow) -> Result<(HWND, WINDOWPLACEMENT)> {
        let hwnd = HWND(window.hwnd()?.0 as _);
        let mut placement = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        unsafe { GetWindowPlacement(hwnd, &mut placement)? };
        Ok((hwnd, placement))
    }

    /// Makes the work area the restore rectangle; returns the previous one.
    pub fn restore_to_work_area(window: &tauri::WebviewWindow) -> Result<[i32; 4]> {
        let (hwnd, mut placement) = placement(window)?;
        let previous = placement.rcNormalPosition;
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        unsafe {
            GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info).ok()?
        };
        placement.rcNormalPosition = work_area_in_workspace(info.rcWork, info.rcMonitor);
        unsafe { SetWindowPlacement(hwnd, &placement)? };
        Ok([previous.left, previous.top, previous.right, previous.bottom])
    }

    pub fn set_restore(window: &tauri::WebviewWindow, rect: [i32; 4]) -> Result<()> {
        let (hwnd, mut placement) = placement(window)?;
        let [left, top, right, bottom] = rect;
        placement.rcNormalPosition = RECT {
            left,
            top,
            right,
            bottom,
        };
        unsafe { SetWindowPlacement(hwnd, &placement)? };
        Ok(())
    }

    /// Window placements use workspace coordinates, which are screen
    /// coordinates shifted by the space toolbars take from the monitor's
    /// top-left corner.
    pub(super) fn work_area_in_workspace(work: RECT, monitor: RECT) -> RECT {
        let (dx, dy) = (work.left - monitor.left, work.top - monitor.top);
        RECT {
            left: work.left - dx,
            top: work.top - dy,
            right: work.right - dx,
            bottom: work.bottom - dy,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn work_area_converts_to_workspace_coordinates() {
            let rect = |left, top, right, bottom| RECT {
                left,
                top,
                right,
                bottom,
            };
            // Bottom taskbar: workspace and screen coordinates agree.
            assert_eq!(
                work_area_in_workspace(rect(0, 0, 2560, 1392), rect(0, 0, 2560, 1440)),
                rect(0, 0, 2560, 1392)
            );
            // Top taskbar on a second monitor: the toolbar offset is removed.
            assert_eq!(
                work_area_in_workspace(rect(2560, 48, 4480, 1080), rect(2560, 0, 4480, 1080)),
                rect(2560, 0, 4480, 1032)
            );
        }
    }
}

#[tauri::command]
fn app_update(
    action: String,
    state: State<'_, Desktop>,
    app: tauri::AppHandle,
) -> std::result::Result<(), String> {
    let nightly = lock(&state.0.snapshot)
        .map(|snapshot| snapshot.settings.update_channel == "nightly")
        .map_err(error)?;
    // Nightly updates are built here rather than downloaded.
    if nightly {
        return match action.as_str() {
            "check" | "download" => {
                let build = action == "download";
                thread::spawn(move || {
                    if let Err(error) = crate::nightly::check(build) {
                        log::warn!("Cannot check for Nightly updates: {error:#}");
                    }
                });
                Ok(())
            }
            "install" => crate::nightly::install_ready()
                .and_then(|()| {
                    hand_over_playback(&state.0);
                    updater::relaunch()
                })
                .map(|()| app.exit(0))
                .map_err(error),
            _ => Err("Unknown update action".into()),
        };
    }
    let result = || -> Result<()> {
        if let Some(message) = updater::DISABLED {
            anyhow::bail!(message);
        }
        let command = match action.as_str() {
            "check" => updater::Command::Check,
            "download" => updater::Command::Download(
                lock(&state.0.available)?
                    .clone()
                    .context("No update available")?,
            ),
            "install" => {
                let update = lock(&state.0.ready_update)?
                    .take()
                    .context("Update is not downloaded yet")?;
                hand_over_playback(&state.0);
                updater::Command::Install(update)
            }
            _ => anyhow::bail!("Unknown update action"),
        };
        state.0.update_commands.send(command)?;
        Ok(())
    };
    result().map_err(error)
}
/// Taskbar buttons and the system media session.
struct Media(crate::media::Session);
/// The frontend reports the current item so the taskbar buttons and the
/// system media session match what is playing.
#[tauri::command]
fn media_state(state: crate::media::MediaState, app: tauri::AppHandle, media: State<'_, Media>) {
    media.0.update(&app, state);
}
/// Ask WebView2 to hold little memory (it drops GPU textures, decoded images
/// and other caches, about half its footprint on a 4K screen) or to return
/// to normal. The page keeps rendering either way.
#[cfg(windows)]
fn memory_target(window: &tauri::WebviewWindow, low: bool) {
    let _ = window.with_webview(move |webview| unsafe {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL, ICoreWebView2_19,
        };
        use windows::core::Interface;
        if let Ok(core) = webview.controller().CoreWebView2()
            && let Ok(core) = core.cast::<ICoreWebView2_19>()
        {
            let _ = core.SetMemoryUsageTargetLevel(if low {
                COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
            } else {
                COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
            });
        }
    });
}
#[cfg(not(windows))]
fn memory_target(_window: &tauri::WebviewWindow, _low: bool) {}
/// Hidden windows stop compositing like minimized ones; audio is native and
/// keeps playing.
/// Act on a `lyssna://` link: show Lyssna, and let the interface open
/// what it names once signed in and ready.
fn follow_link(app: &tauri::AppHandle, link: crate::links::Link) {
    if let Some(window) = app.get_webview_window("main") {
        set_window_shown(&window, true);
        let _ = window.set_focus();
    }
    if link != crate::links::Link::Show {
        crate::links::keep(link);
        let _ = app.emit("lyssna", json!({"type":"link"}));
    }
}
/// The `lyssna://` link waiting to be acted on, taken once. A song opens
/// its album with the song selected, so its album is looked up here.
#[tauri::command]
async fn take_link(
    state: State<'_, Desktop>,
) -> std::result::Result<Option<serde_json::Value>, String> {
    let Some(link) = crate::links::take().filter(|link| *link != crate::links::Link::Show) else {
        return Ok(None);
    };
    let crate::links::Link::Song(uri) = link else {
        return Ok(Some(json!(link)));
    };
    let account = lock(&state.0.snapshot)
        .map_err(error)?
        .profile
        .as_ref()
        .context("Sign in first")
        .map_err(error)?
        .id
        .clone();
    let controls = state.0.controls.scoped(&account).map_err(error)?;
    let song = uri.clone();
    let album = tauri::async_runtime::spawn_blocking(move || controls.resolve(&[song]))
        .await
        .map_err(error)?
        .map_err(error)?
        .into_iter()
        .next()
        .and_then(|track| track.album.id)
        .context("Spotify couldn't find that song")
        .map_err(error)?;
    Ok(Some(
        json!({"kind": "open", "value": format!("album:{album}"), "select": uri}),
    ))
}
fn set_window_shown(window: &tauri::WebviewWindow, shown: bool) {
    #[cfg(windows)]
    let _ = window.with_webview(move |webview| unsafe {
        let _ = webview.controller().SetIsVisible(shown);
    });
    memory_target(window, !shown);
    if shown {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    } else {
        let _ = window.hide();
    }
}
#[tauri::command]
async fn autostart() -> std::result::Result<crate::autostart::Launch, String> {
    tauri::async_runtime::spawn_blocking(crate::autostart::get)
        .await
        .map_err(|_| "Startup setting unavailable".to_owned())?
        .map_err(|error| error.to_string())
}
#[tauri::command]
async fn set_autostart(launch: crate::autostart::Launch) -> std::result::Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || crate::autostart::set(launch))
        .await
        .map_err(|_| "Startup setting unavailable".to_owned())?
        .map_err(|error| error.to_string())
}
/// Exit for real; the close button may only hide to the tray.
#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}
/// Notification-area icon, like Spotify's: click to show the window, or use
/// the menu to control playback or quit while the window is hidden.
fn install_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let menu = MenuBuilder::new(app)
        .item(&MenuItemBuilder::with_id("show", "Show Lyssna").build(app)?)
        .separator()
        .item(&MenuItemBuilder::with_id("toggle", "Play/Pause").build(app)?)
        .item(&MenuItemBuilder::with_id("next", "Next").build(app)?)
        .item(&MenuItemBuilder::with_id("previous", "Previous").build(app)?)
        .separator()
        .item(&MenuItemBuilder::with_id("quit", "Quit Lyssna").build(app)?)
        .build()?;
    TrayIconBuilder::with_id("main")
        .icon(tauri::image::Image::new_owned(
            crate::app_icon::icon_rgba(32),
            32,
            32,
        ))
        .tooltip("Lyssna")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    set_window_shown(&window, true);
                }
            }
            "quit" => app.exit(0),
            action @ ("toggle" | "next" | "previous") => {
                let _ = app.emit(
                    "lyssna",
                    json!({"type":"media_control","action":action,"position_ms":null}),
                );
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
                && let Some(window) = tray.app_handle().get_webview_window("main")
            {
                let shown =
                    window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false);
                set_window_shown(&window, !shown);
            }
        })
        .build(app)?;
    Ok(())
}
/// Disk space used by cached songs, in bytes.
#[tauri::command]
async fn audio_cache_size() -> u64 {
    tauri::async_runtime::spawn_blocking(crate::audio_cache::size)
        .await
        .unwrap_or_default()
}
/// Delete cached songs; returns the bytes freed.
#[tauri::command]
async fn clear_audio_cache() -> std::result::Result<u64, String> {
    tauri::async_runtime::spawn_blocking(crate::audio_cache::clear)
        .await
        .map_err(|_| "Cache cleanup was interrupted".to_owned())?
        .map_err(error)
}
/// Drop WebView2 caches once after browsing, while the window stays usable.
#[tauri::command]
fn trim_memory(window: tauri::WebviewWindow) {
    memory_target(&window, true);
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(2));
        if window.is_focused().unwrap_or(true) {
            memory_target(&window, false);
        }
    });
}
/// Asks a running Lyssna to exit instead of hiding to the tray.
pub const QUIT_ARG: &str = "--quit";

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    use tauri_plugin_window_state::StateFlags;
    let app = tauri::Builder::default()
        // Opening Lyssna again, as from the Start menu while it sits in
        // the tray, shows the running window instead of a second copy.
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            // `--quit` from the build script closes the running copy fully,
            // tray included, so the new build can take its place.
            if args.iter().any(|argument| argument == QUIT_ARG) {
                app.exit(0);
                return;
            }
            // A `lyssna://` link opened while Lyssna runs.
            if let Some(link) = crate::links::from_args(&args) {
                follow_link(app, link);
            } else if let Some(window) = app.get_webview_window("main") {
                set_window_shown(&window, true);
            }
        }))
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED)
                .build(),
        )
        // Local Files covers, saved from the songs' own pictures.
        // Read off the interface's thread, so scrolling never waits on disk.
        .register_asynchronous_uri_scheme_protocol(
            crate::local_files::COVER_SCHEME,
            |_, request, responder| {
                let name = request.uri().path().trim_start_matches('/').to_owned();
                tauri::async_runtime::spawn_blocking(move || {
                    responder.respond(match crate::local_files::cover(&name) {
                        Some((bytes, mime)) => tauri::http::Response::builder()
                            .header("Content-Type", mime)
                            .header("Access-Control-Allow-Origin", "*")
                            .header("Cache-Control", "max-age=31536000, immutable")
                            .body(bytes)
                            .unwrap_or_default(),
                        None => tauri::http::Response::builder()
                            .status(404)
                            .body(Vec::new())
                            .unwrap_or_default(),
                    });
                });
            },
        )
        .setup(|app| {
            // Nothing was running to quit: this copy has nothing to do.
            if std::env::args().any(|argument| argument == QUIT_ARG) {
                std::process::exit(0);
            }
            // Lyssna opened by a `lyssna://` link: acted on once ready.
            if let Some(link) = crate::links::from_args(std::env::args()) {
                crate::links::keep(link);
            }
            // Installed copies open `lyssna://` links; a debug build from
            // `target` would take them from the installed one.
            #[cfg(windows)]
            if !cfg!(debug_assertions)
                && let Err(error) = crate::links::register()
            {
                log::warn!("{error:#}");
            }
            #[cfg(target_os = "macos")]
            app.set_menu(tauri::menu::Menu::default(app.handle())?)?;
            // In-app updates replace only the executable; bring the bundle's
            // Info.plist (and so `lyssna://` links) up to date with it.
            #[cfg(target_os = "macos")]
            crate::mac_bundle::refresh();
            if let Some(window) = app.get_webview_window("main") {
                window.set_icon(tauri::image::Image::new_owned(
                    crate::app_icon::icon_rgba(64),
                    64,
                    64,
                ))?;
                // WebView2 keeps rendering while minimized, so tell the page
                // when to suspend or throttle its GPU animations.
                let activity = window.clone();
                #[cfg(windows)]
                let was_minimized = AtomicBool::new(false);
                let was_lean = AtomicBool::new(false);
                window.on_window_event(move |event| {
                    // Signed in, the close button can keep music playing from
                    // the tray. Before sign-in it always quits.
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event
                        && let Some(state) = activity.app_handle().try_state::<Desktop>()
                        && lock(&state.0.snapshot).is_ok_and(|snapshot| {
                            snapshot.settings.keep_playing_on_close && snapshot.profile.is_some()
                        })
                    {
                        api.prevent_close();
                        set_window_shown(&activity, false);
                        return;
                    }
                    // Space Gravity lyrics react to the window being dragged.
                    if let tauri::WindowEvent::Moved(position) = event {
                        let scale = activity.scale_factor().unwrap_or(1.0);
                        let _ = activity.emit(
                            "lyssna",
                            json!({
                                "type": "window_moved",
                                "x": f64::from(position.x) / scale,
                                "y": f64::from(position.y) / scale,
                            }),
                        );
                    }
                    if matches!(
                        event,
                        tauri::WindowEvent::Resized(_) | tauri::WindowEvent::Focused(_)
                    ) {
                        let minimized = activity.is_minimized().unwrap_or(false);
                        let focused = activity.is_focused().unwrap_or(true);
                        // Hiding the WebView2 controller makes Chromium stop
                        // compositing and throttle timers, so a minimized
                        // window uses no GPU. Audio runs natively and continues.
                        #[cfg(windows)]
                        if was_minimized.swap(minimized, Ordering::SeqCst) != minimized {
                            let _ = activity.with_webview(move |webview| unsafe {
                                let _ = webview.controller().SetIsVisible(!minimized);
                            });
                        }
                        // In the background (music playing behind other
                        // windows) the page needs few caches; keep it lean.
                        let lean = minimized || !focused;
                        if was_lean.swap(lean, Ordering::SeqCst) != lean {
                            memory_target(&activity, lean);
                        }
                        let _ = activity.emit(
                            "lyssna",
                            json!({"type":"window_activity","minimized":minimized,"focused":focused}),
                        );
                    }
                });
            }
            app.manage(Desktop::start(app.handle().clone())?);
            // The Stream Deck plugin's local control; the app runs without it.
            if let Err(error) = stream_deck::start(app.handle().clone()) {
                eprintln!("Stream Deck control unavailable: {error}");
            }
            install_tray(app)?;
            // The tray icon shows only while closing keeps music playing.
            let keep_playing = crate::config::Settings::load()
                .is_ok_and(|settings| settings.keep_playing_on_close);
            if let Some(tray) = app.tray_by_id("main") {
                tray.set_visible(keep_playing)?;
            }
            // Opened at sign-in as "Minimized": to the tray when closing keeps
            // music playing, otherwise to the taskbar.
            if std::env::args().any(|argument| argument == crate::autostart::MINIMIZED_ARG)
                && let Some(window) = app.get_webview_window("main")
            {
                if keep_playing {
                    set_window_shown(&window, false);
                } else {
                    window.minimize()?;
                }
            }
            let handle = app.handle().clone();
            let session = crate::media::Session::start(app.handle(), move |action| {
                let position = match action {
                    crate::media::MediaAction::Seek(position) => Some(position),
                    _ => None,
                };
                let _ = handle.emit(
                    "lyssna",
                    json!({"type":"media_control","action":action.name(),"position_ms":position}),
                );
            });
            app.manage(Media(session));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            resume_after_restart,
            take_link,
            window_fullscreen,
            open_lyrics_popout,
            toggle_cinema_lyrics,
            artist_discography,
            jam,
            track_credits,
            release_tracks,
            read_clipboard,
            dj_request,
            dj_prepare,
            dj_voice,
            dj_release_voice,
            dj_cancel,
            home,
            bootstrap,
            sign_in,
            cancel_sign_in,
            retry_sign_in,
            prefetch_canvas,
            prefetch_page,
            trim_memory,
            audio_cache_size,
            clear_audio_cache,
            prefetch_metadata,
            sign_out,
            browse,
            player,
            retry_audio,
            test_audio_output,
            refresh,
            save_item,
            save_settings,
            audio_outputs,
            configure_provider,
            configure_spicy_lyrics,
            retry_lyrics,
            refresh_lyrics,
            quit_app,
            autostart,
            set_autostart,
            whats_new,
            library_items,
            clear_lyrics_cache,
            open_devtools,
            musixmatch_token_saved,
            save_musixmatch_token,
            renew_musixmatch_token,
            import_lyrics,
            reset_lyrics,
            saved_lyrics,
            saved_lyrics_file,
            artist_liked_count,
            list_duration,
            liked_songs_count,
            local_files_count,
            pick_local_folder,
            nightly_build,
            nightly_check,
            nightly_cancel,
            nightly_clear_cache,
            page_shown,
            nightly_status,
            nightly_log,
            delete_saved_lyrics,
            add_tracks,
            create_playlist,
            remove_playlist,
            remove_from_playlist,
            edit_playlist,
            create_folder,
            rename_folder,
            delete_folder,
            move_to_folder,
            move_playlist_song,
            saved_status,
            open_link,
            open_creator_profile,
            resolve_album,
            user_profile,
            follow_user,
            search_page,
            app_update,
            media_state
        ])
        .build(tauri::generate_context!())?;
    app.run(|app, event| {
        // macOS hands `lyssna://` links to the running app.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Opened { urls } = &event
            && let Some(link) = urls.iter().find_map(|url| crate::links::parse(url.as_str()))
        {
            follow_link(app, link);
        }
        if matches!(&event,tauri::RunEvent::WindowEvent{label,event:tauri::WindowEvent::Destroyed,..}if label=="main"){app.exit(0);}
        if matches!(event, tauri::RunEvent::Exit)
            && let Some(state) = app.try_state::<Desktop>()
        {
            state.stop();
        }
    });
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_window_shares_the_browser_switches() {
        // WebView2 refuses a window whose switches differ from the browser
        // already running, and the main window's come from the config.
        let config: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(
            config["app"]["windows"][0]["additionalBrowserArgs"],
            BROWSER_ARGS
        );
        assert!(BROWSER_ARGS.contains("--disable-gpu-process-crash-limit"));
    }
    #[test]
    fn album_songs_come_from_album_metadata_with_the_album() {
        let id = "0123456789abcdefghijkl";
        let entity = json!({
            "uri": format!("spotify:album:{id}"),
            "name": "Vampire Bat",
            "type": "SINGLE",
            "tracksV2": {"totalCount": 1, "items": [{"track": {
                "uri": "spotify:track:abcdefghijklmnopqrstuv",
                "name": "Vampire Bat",
                "duration": {"totalMilliseconds": 206000},
                "contentRating": {"label": "EXPLICIT"},
                "artists": {"items": [{"uri": "spotify:artist:0123456789abcdefghij07", "profile": {"name": "Glass Animals"}}]}
            }}]}
        });
        let songs = album_songs(&entity, id);
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].name, "Vampire Bat");
        assert_eq!(songs[0].duration_ms, 206000);
        assert!(songs[0].explicit);
        assert_eq!(songs[0].artists[0].name, "Glass Animals");
        assert_eq!(songs[0].album.uri, format!("spotify:album:{id}"));
    }
    #[test]
    fn ordered_liked_selection_keeps_requested_queue() {
        let loaded: Vec<_> = ["a", "b", "c"]
            .map(|id| Track {
                uri: format!("spotify:track:{id}"),
                ..Track::default()
            })
            .to_vec();
        for order in [[2, 1, 0], [1, 2, 0]] {
            let requested: Vec<_> = order.map(|index| loaded[index].uri.clone()).to_vec();
            let action = liked_from_collection(
                PlayerAction::PlayUris {
                    uris: requested.clone(),
                    offset: 0,
                    context: None,
                },
                Some("liked"),
                "me",
            );
            let PlayerAction::PlayUris { uris, offset, .. } = action else {
                panic!("Expected the sorted queue");
            };
            assert_eq!(
                &uris[offset..],
                requested,
                "Upcoming songs must follow visible order"
            );
        }
    }
    #[test]
    fn liked_songs_play_from_the_collection_only_when_named() {
        let list = || PlayerAction::PlayUris {
            uris: vec!["spotify:track:a".into(), "spotify:track:b".into()],
            offset: 1,
            context: None,
        };
        match liked_from_collection(list(), Some("liked"), "me") {
            PlayerAction::PlayUris {
                uris,
                offset,
                context,
            } => {
                assert_eq!(context.as_deref(), Some("spotify:user:me:collection"));
                assert_eq!(uris.len(), 2);
                assert_eq!(offset, 1);
                assert_eq!(uris[offset], "spotify:track:b");
            }
            _ => panic!("Liked Songs should play as the collection"),
        }
        assert!(matches!(
            liked_from_collection(list(), None, "me"),
            PlayerAction::PlayUris { context: None, .. }
        ));
    }
    #[test]
    fn listed_playback_preserves_full_queue_and_context() {
        let mut uris: Vec<_> = (0..804)
            .map(|index| format!("spotify:track:{index:022}"))
            .collect();
        // Selecting a later duplicate must keep that occurrence's offset.
        uris[400] = uris[0].clone();
        let context = "spotify:playlist:0123456789abcdefghijkl";
        let request: PlayerRequest = serde_json::from_value(json!({
            "command": "play_uris", "uris": uris, "offset": 400, "context": context
        }))
        .unwrap();
        let PlayerAction::PlayUris {
            uris: actual,
            offset,
            context: actual_context,
        } = local_load(request.action().unwrap(), &[]).unwrap()
        else {
            panic!("Expected the complete displayed queue");
        };
        assert_eq!(actual, uris);
        assert_eq!(offset, 400);
        assert_eq!(actual_context.as_deref(), Some(context));
    }
    #[test]
    fn early_play_requests_start_once_when_native_audio_is_ready() {
        for context in [
            "spotify:track:0123456789abcdefghijkl",
            "spotify:playlist:0123456789abcdefghijkl",
            crate::home::DJ_URI,
        ] {
            let request: PlayerRequest =
                serde_json::from_value(json!({"command":"play_home", "context":context})).unwrap();
            let queued = Mutex::new(Some((request.action().unwrap(), Instant::now())));
            let (controls, mut commands) = NativeControls::test_active("startup-listener");
            assert!(commands.try_recv().is_err());
            assert!(send_queued_native(&queued, &controls));
            match commands.try_recv().unwrap() {
                PlayerAction::Play(Some(uri)) | PlayerAction::PlayContext { context: uri, .. } => {
                    assert_eq!(uri, context)
                }
                _ => panic!("Expected the queued playback request"),
            }
            assert!(!send_queued_native(&queued, &controls));
            assert!(
                commands.try_recv().is_err(),
                "Readiness updates must not replay the request"
            );
        }
    }

    #[test]
    fn expired_or_cancelled_startup_requests_do_not_start_later() {
        let (controls, mut commands) = NativeControls::test_active("startup-listener");
        let queued = Mutex::new(Some((
            PlayerAction::Play(Some("spotify:track:0123456789abcdefghijkl".into())),
            Instant::now() - QUEUED_NATIVE_TTL,
        )));
        assert!(!send_queued_native(&queued, &controls));
        assert!(!send_queued_native(&Mutex::new(None), &controls));
        assert!(commands.try_recv().is_err());
    }

    #[test]
    fn dj_launch_keeps_session_context_without_playlist_tracks() {
        let request: PlayerRequest = serde_json::from_value(json!({"command":"dj"})).unwrap();
        let action = local_load(request.action().unwrap(), &[])
            .expect("DJ must resolve its session rather than a static playlist");
        assert!(
            matches!(action,PlayerAction::PlayContext{context,offset:None} if context==crate::home::DJ_URI)
        );
    }
    #[test]
    fn creator_profiles_use_usernames_and_encode_path_characters() {
        assert_eq!(
            creator_profile_url("@Arashii").unwrap().as_str(),
            "https://spicylyrics.org/Arashii?_state=%7B%22mp%22%3Atrue%7D"
        );
        assert!(creator_profile_url("../other").is_err());
        assert_eq!(
            creator_profile_url("Name#1234").unwrap().as_str(),
            "https://spicylyrics.org/Name%231234?_state=%7B%22mp%22%3Atrue%7D"
        );
    }
    #[test]
    fn listen_plays_only_spotify_tracks() {
        let uri = |link: &str| listen_uri(&url::Url::parse(link).unwrap());
        let track = Some("spotify:track:4uLU6hMCjMI75M1A2tKUQC".to_owned());
        assert_eq!(uri("spotify:track:4uLU6hMCjMI75M1A2tKUQC"), track);
        assert_eq!(
            uri("https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC?si=x"),
            track
        );
        assert_eq!(uri("spotify:album:4uLU6hMCjMI75M1A2tKUQC"), None);
        assert_eq!(uri("spotify:track:short"), None);
        assert_eq!(
            uri("https://example.com/track/4uLU6hMCjMI75M1A2tKUQC"),
            None
        );
    }
    #[test]
    fn creator_profiles_carry_the_app_typeface() {
        let script = profile_window_script();
        assert!(!script.contains("PROFILE_FONTS"));
        assert_eq!(script.matches("@font-face").count(), 5);
        assert!(script.contains("font-family:LyssnaMetropolis"));
    }
    #[test]
    fn local_playlist_selection_uses_the_chosen_loaded_track() {
        let tracks: Vec<_> = (0..150)
            .map(|index| Track {
                uri: format!("spotify:track:{index}"),
                ..Track::default()
            })
            .collect();
        let action = local_load(
            PlayerAction::PlayContext {
                context: "spotify:playlist:other-owner".into(),
                offset: Some(tracks[110].uri.clone()),
            },
            &tracks,
        )
        .unwrap();
        let PlayerAction::PlayUris {
            uris,
            offset,
            context,
        } = action
        else {
            panic!("Local playback must not wait on remote playlist resolution")
        };
        assert_eq!(uris[offset], tracks[110].uri);
        // The whole loaded playlist plays, under its own context.
        assert_eq!(uris.len(), 150);
        assert_eq!(context.as_deref(), Some("spotify:playlist:other-owner"));
        assert!(
            local_load(
                PlayerAction::PlayContext {
                    context: "spotify:playlist:empty".into(),
                    offset: None
                },
                &[]
            )
            .is_err()
        );
    }
    #[test]
    fn local_device_ids_use_native_controls_without_redirecting_remote_devices() {
        let local = Device {
            id: Some("web-api-id".into()),
            name: native::device_name(),
            ..Device::default()
        };
        assert!(targets_local_device(
            Some("native"),
            Some("session-id"),
            &local
        ));
        assert!(targets_local_device(
            Some("session-id"),
            Some("session-id"),
            &local
        ));
        assert!(targets_local_device(
            Some("web-api-id"),
            Some("session-id"),
            &local
        ));
        assert!(targets_local_device(None, Some("session-id"), &local));
        assert!(!targets_local_device(
            Some("another-pc"),
            Some("session-id"),
            &local
        ));
        assert!(!targets_local_device(
            None,
            Some("session-id"),
            &Device::default()
        ));
    }
    #[test]
    fn page_play_follows_the_selected_speaker_and_dj_stays_local() {
        let speaker = Device {
            id: Some("echo".into()),
            name: "Gameroom".into(),
            ..Device::default()
        };
        // The album page's Play button sends play_home; it used to start on
        // this computer while a speaker was selected.
        assert!(!plays_locally(
            "play_home",
            Some("echo"),
            Some("session-id"),
            &speaker
        ));
        assert!(plays_locally(
            "play_home",
            Some("native"),
            Some("session-id"),
            &speaker
        ));
        assert!(plays_locally(
            "dj",
            Some("echo"),
            Some("session-id"),
            &speaker
        ));
    }
    #[test]
    fn links_open_only_spotify_lyrics_and_credited_repositories() {
        let allowed = |url: &str| allowed_link(&url::Url::parse(url).unwrap());
        assert!(allowed("https://open.spotify.com/track/abc"));
        assert!(allowed("https://github.com/iPixelGalaxy/spotify-plus"));
        assert!(allowed("https://github.com/Stormanzanii/Lyssna"));
        assert!(allowed("https://github.com/Spikerko"));
        assert!(!allowed("https://github.com/someone/else"));
        assert!(!allowed(
            "https://github.com/iPixelGalaxy/spotify-plus/releases"
        ));
        assert!(!allowed("http://github.com/iPixelGalaxy/spotify-plus"));
        assert!(!allowed(
            "https://user@github.com/iPixelGalaxy/spotify-plus"
        ));
        assert!(!allowed("https://example.com/"));
    }
    #[test]
    fn routes_reject_arbitrary_urls_and_malformed_ids() {
        assert!(valid_route("search:Petite Meller"));
        assert!(valid_route("recent"));
        assert!(valid_route("album:0123456789abcdefghijkl"));
        assert!(valid_route("liked-artist:0123456789abcdefghijkl"));
        assert!(valid_route("discography:0123456789abcdefghijkl"));
        assert!(!valid_route("discography:short"));
        assert!(valid_route("artist-shelf:0123456789abcdefghijkl:2"));
        assert!(!valid_route("artist-shelf:0123456789abcdefghijkl:x"));
        assert!(!valid_route("liked-artist:short"));
        assert!(!valid_route("https://example.com"));
        assert!(!valid_route("album:../secrets"));
        assert!(!valid_route("search:"));
    }
    #[test]
    fn invalid_player_commands_never_reach_the_audio_worker() {
        let request: PlayerRequest = serde_json::from_value(
            json!({"command":"play_uris","uris":["spotify:track:one"],"offset":5}),
        )
        .unwrap();
        assert!(request.action().is_err());
        let request: PlayerRequest =
            serde_json::from_value(json!({"command":"repeat","repeat":"invalid"})).unwrap();
        assert!(request.action().is_err());
    }
}
