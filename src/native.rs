use crate::credential as keyring;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use crate::api::PlayerAction;
use anyhow::{Context, Result};
use librespot_connect::{ConnectConfig, LoadRequest, LoadRequestOptions, PlayingTrack, Spirc};
use librespot_core::error::ErrorKind;
use librespot_core::{Session, authentication::Credentials, config::SessionConfig};
use librespot_playback::{
    config::PlayerConfig,
    mixer::{Mixer, MixerConfig, softmixer::SoftMixer},
    player::{Player, PlayerEvent},
};
use librespot_protocol::authentication::AuthenticationType;
use tokio::sync::mpsc as async_channel;
use tokio::sync::oneshot;

const CREDENTIAL_SERVICE: &str = "Lyssna/SpotifyAudio";
static CREDENTIAL_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod scoped_tests {
    use super::*;

    #[test]
    fn alternative_audio_keeps_the_requested_track_for_progress_events() {
        use librespot_core::SpotifyUri;
        use librespot_metadata::audio::item::{AudioItem, UniqueFields};
        let requested = SpotifyUri::from_uri("spotify:track:0123456789abcdefghijkl").unwrap();
        for uri in [
            requested.to_uri(),
            "spotify:track:1123456789abcdefghijkl".into(),
        ] {
            let audio_id = SpotifyUri::from_uri(&uri).unwrap();
            let event = PlayerEvent::TrackChanged {
                track_id: requested.clone(),
                audio_item: Box::new(AudioItem {
                    track_id: audio_id,
                    uri,
                    files: Default::default(),
                    name: "Playable version".into(),
                    covers: Vec::new(),
                    language: Vec::new(),
                    duration_ms: 82_000,
                    is_explicit: false,
                    availability: Ok(()),
                    alternatives: None,
                    unique_fields: UniqueFields::Track {
                        artists: Default::default(),
                        album: "Album".into(),
                        album_artists: Vec::new(),
                        popularity: 0,
                        number: 1,
                        disc_number: 1,
                    },
                }),
            };
            let track = public_track_change(&event).unwrap();
            let mut playback = crate::model::Playback::default();
            playback.load_native_track(track, Some("native-session"), "Lyssna", 100);
            assert!(
                playback.apply_native_position(
                    Some("native-session"),
                    "Lyssna",
                    &requested.to_uri(),
                    1500,
                    Some(true)
                ),
                "Playing events must update an available alternative, rather than leave it loading"
            );
            assert!(playback.is_playing);
            assert_eq!(playback.progress_ms, Some(1500));
            assert_eq!(
                playback.item.unwrap().id.as_deref(),
                requested.to_uri().rsplit(':').next()
            );
        }
    }

    #[test]
    fn scoped_background_connection_cannot_follow_an_account_switch() {
        let (controls, _) = NativeControls::test_active("alice");
        let scoped = controls.scoped("alice").unwrap();
        let (replacement, _) = NativeControls::test_active("bob");
        *controls.0.lock().unwrap() = replacement.0.lock().unwrap().take();
        assert!(scoped.connection_event(Some("bob")).is_none());
        assert!(scoped.scoped("alice").is_ok());
        assert!(scoped.scoped("bob").is_err());
        assert!(controls.scoped("alice").is_err());
    }
}

#[derive(Clone)]
pub enum NativeEvent {
    Narration(bool),
    /// Waiting for the one-time browser authorization for audio.
    Authorizing,
    Reconnecting(u64),
    Ready(String),
    Stopped,
    QueueChanged,
    /// Spotify changed the Jam this account is in; whether this computer
    /// still plays along with it.
    Jam(bool, Option<String>),
    /// Whether the current context is a DJ session.
    DJContext(bool),
    /// The playlist, album, artist or collection URI now playing.
    Context(String),
    /// Shuffle or repeat changed outside DJ, to keep for later.
    Modes(PlayModes),
    Track(Box<crate::model::Track>),
    Transport {
        shuffle: Option<bool>,
        repeat: Option<String>,
        volume: Option<u8>,
    },
    Position {
        uri: String,
        position_ms: u64,
        playing: Option<bool>,
        seeked: bool,
    },
    Error(String),
    ControlError(String),
}

#[derive(Clone)]
struct ControlConnection {
    devices: Arc<Mutex<librespot_connect::ConnectedDevices>>,
    audio_buffer: crate::audio::AudioBuffer,
    account: String,
    status: Arc<Mutex<Option<NativeEvent>>>,
    session: Arc<Mutex<Option<Session>>>,
    dj_requests: crate::dj::transport::Requests,
    volume: Arc<AtomicU32>,
    sender: async_channel::UnboundedSender<PlayerAction>,
    ready: Arc<AtomicBool>,
    loaded: Arc<AtomicBool>,
    queue_snapshot: Arc<Mutex<Option<Vec<QueueEntry>>>>,
    playlists: Option<async_channel::UnboundedSender<PlaylistRead>>,
    metadata: Option<async_channel::UnboundedSender<MetadataRead>>,
}
struct MetadataRead {
    offset: usize,
    uri: String,
    kind: librespot_protocol::extension_kind::ExtensionKind,
    reply: mpsc::Sender<std::result::Result<Vec<u8>, String>>,
}
struct PlaylistRead {
    id: String,
    offset: usize,
    limit: usize,
    reply: mpsc::Sender<std::result::Result<Vec<crate::model::Track>, String>>,
}
struct PlaybackServices<'a> {
    devices: &'a Mutex<librespot_connect::ConnectedDevices>,
    audio_buffer: &'a crate::audio::AudioBuffer,
    commands: &'a mut async_channel::UnboundedReceiver<PlayerAction>,
    playlists: &'a mut async_channel::UnboundedReceiver<PlaylistRead>,
    metadata: &'a mut async_channel::UnboundedReceiver<MetadataRead>,
    latest_volume: &'a AtomicU32,
    shared_session: &'a Mutex<Option<Session>>,
    loaded: &'a AtomicBool,
    queue_snapshot: &'a Mutex<Option<Vec<QueueEntry>>>,
}

/// An upcoming song, and whether the listener queued it rather than it
/// coming from the playing context.
#[derive(Clone)]
struct QueueEntry {
    uri: String,
    added: bool,
}

/// Local transport commands never wait for the browsing/Web API worker.
#[derive(Clone, Default)]
pub struct NativeControls(Arc<Mutex<Option<ControlConnection>>>);

impl NativeControls {
    /// Capture this account's connection so background work cannot switch accounts.
    pub(crate) fn scoped(&self, account: &str) -> Result<Self> {
        let connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Native session unavailable"))?;
        let connection = connection
            .as_ref()
            .filter(|value| value.account == account)
            .context("Spotify account changed")?
            .clone();
        Ok(Self(Arc::new(Mutex::new(Some(connection)))))
    }

    pub(crate) fn provider_token(&self) -> Result<String> {
        self.run(async |session| {
            let token = session
                .token_provider()
                .get_token("streaming")
                .await
                .map_err(|_| {
                    anyhow::anyhow!("Could not authorize the configured lyrics provider")
                })?;
            Ok(token.access_token)
        })
    }
    pub fn set_audio_output(&self, account: &str, name: Option<String>) {
        if let Ok(connection) = self.0.lock()
            && let Some(connection) = connection
                .as_ref()
                .filter(|connection| connection.account == account)
        {
            connection.audio_buffer.output.request(name);
        }
    }
    pub fn set_crossfade(&self, account: &str, seconds: u8) {
        if let Ok(connection) = self.0.lock()
            && let Some(connection) = connection
                .as_ref()
                .filter(|connection| connection.account == account)
        {
            connection.audio_buffer.set_crossfade(seconds);
        }
    }
    #[cfg(test)]
    pub(crate) fn test_active(
        account: &str,
    ) -> (Self, async_channel::UnboundedReceiver<PlayerAction>) {
        let (sender, receiver) = async_channel::unbounded_channel();
        let controls = Self(Arc::new(Mutex::new(Some(ControlConnection {
            devices: Default::default(),
            audio_buffer: Default::default(),
            account: account.into(),
            status: Arc::new(Mutex::new(Some(NativeEvent::Ready("test-device".into())))),
            session: Default::default(),
            dj_requests: Default::default(),
            volume: Arc::new(AtomicU32::new(100)),
            sender,
            ready: Arc::new(AtomicBool::new(true)),
            loaded: Arc::new(AtomicBool::new(true)),
            playlists: None,
            metadata: None,
            queue_snapshot: Arc::new(Mutex::new(Some(Vec::new()))),
        }))));
        (controls, receiver)
    }
    #[cfg(test)]
    pub(crate) fn test_connecting(
        account: &str,
    ) -> (Self, async_channel::UnboundedReceiver<PlayerAction>) {
        let (controls, receiver) = Self::test_active(account);
        {
            let connection = controls.0.lock().unwrap();
            let connection = connection.as_ref().unwrap();
            connection.ready.store(false, Ordering::Release);
            connection.loaded.store(false, Ordering::Release);
            *connection.status.lock().unwrap() = None;
        }
        (controls, receiver)
    }

    pub fn connected_or_connecting(&self, account: &str) -> bool {
        self.0.lock().is_ok_and(|connection| {
            connection.as_ref().is_some_and(|connection| {
                connection.account == account
                    && !connection.sender.is_closed()
                    && (connection.ready.load(Ordering::Acquire)
                        || connection.status.lock().is_ok_and(|status| {
                            !matches!(status.as_ref(), Some(NativeEvent::Error(_)))
                        }))
            })
        })
    }
    pub fn prewarm_dj_request(&self, account: &str) -> Result<()> {
        let (session, requests) = {
            let connection = self
                .0
                .lock()
                .map_err(|_| anyhow::anyhow!("Native audio unavailable"))?;
            let connection = connection
                .as_ref()
                .filter(|c| c.account == account && c.ready.load(Ordering::Acquire))
                .context("Connect native audio before asking DJ")?;
            let session = connection
                .session
                .lock()
                .map_err(|_| anyhow::anyhow!("Native session unavailable"))?
                .clone()
                .context("Native session unavailable")?;
            (session, connection.dj_requests.clone())
        };
        session.spawn({
            let session = session.clone();
            async move {
                let _ = requests.warm_up(&session).await;
            }
        });
        Ok(())
    }
    pub fn request_dj(
        &self,
        account: &str,
        prompt: crate::dj::Prompt,
        cancelled: Arc<AtomicBool>,
    ) -> Result<()> {
        let (session, requests) = {
            let connection = self
                .0
                .lock()
                .map_err(|_| anyhow::anyhow!("Native audio unavailable"))?;
            let connection = connection
                .as_ref()
                .filter(|c| c.account == account && c.ready.load(Ordering::Acquire))
                .context("Connect native audio before asking DJ")?;
            let session = connection
                .session
                .lock()
                .map_err(|_| anyhow::anyhow!("Native session unavailable"))?
                .clone()
                .context("Native session unavailable")?;
            (session, connection.dj_requests.clone())
        };
        let (ready, result) = mpsc::sync_channel(1);
        let prepare_session = session.clone();
        let prepare_cancelled = cancelled.clone();
        session.spawn(async move {
            let prepared = tokio::time::timeout(
                Duration::from_secs(65),
                requests.prepare(&prepare_session, prompt, &prepare_cancelled),
            )
            .await
            .context("DJ request timed out")
            .and_then(|result| result);
            let _ = ready.send(prepared);
        });
        let context = result
            .recv_timeout(Duration::from_secs(66))
            .context("DJ request worker unavailable")??;
        let connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Native audio unavailable"))?;
        let connection = connection
            .as_ref()
            .filter(|c| c.account == account && c.ready.load(Ordering::Acquire))
            .context("Spotify account changed during DJ request")?;
        anyhow::ensure!(!cancelled.load(Ordering::Acquire), "DJ request cancelled");
        connection.audio_buffer.interrupt(None);
        let start = context.start;
        connection
            .sender
            .send(PlayerAction::PreparedDJ {
                context: Box::new(context.context),
                provider: context.provider,
                cancelled,
            })
            .map_err(|_| anyhow::anyhow!("Native audio disconnected"))?;
        connection.loaded.store(true, Ordering::Release);
        if let Some(start) = start {
            start.remember();
        }
        Ok(())
    }
    pub fn connection_event(&self, account: Option<&str>) -> Option<NativeEvent> {
        let account = account?;
        let connection = self.0.lock().ok()?;
        let connection = connection
            .as_ref()
            .filter(|connection| connection.account == account)?;
        if connection.sender.is_closed() {
            return Some(NativeEvent::Error(
                "Native audio disconnected. Press Play to reconnect.".into(),
            ));
        }
        let event = connection.status.lock().ok()?.clone()?;
        if matches!(event, NativeEvent::Ready(_)) && !connection.ready.load(Ordering::Acquire) {
            Some(NativeEvent::Error(
                "Native audio disconnected. Press Play to reconnect.".into(),
            ))
        } else {
            Some(event)
        }
    }
    pub fn has_track(&self) -> bool {
        self.0.lock().is_ok_and(|connection| {
            connection.as_ref().is_some_and(|connection| {
                connection.ready.load(Ordering::Acquire)
                    && connection.loaded.load(Ordering::Acquire)
            })
        })
    }
    pub(crate) fn session(&self) -> Result<Session> {
        let session = {
            let connection = self
                .0
                .lock()
                .map_err(|_| anyhow::anyhow!("Native audio unavailable"))?;
            let connection = connection
                .as_ref()
                .filter(|connection| connection.ready.load(Ordering::Acquire))
                .context("Native audio is connecting")?;
            connection
                .session
                .lock()
                .map_err(|_| anyhow::anyhow!("Native session unavailable"))?
                .clone()
                .context("Native session unavailable")?
        };
        Ok(session)
    }
    pub fn playlist_info(&self, id: &str) -> Result<crate::model::BrowseInfo> {
        let session = self.session()?;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(crate::native_browse::playlist_info(&session, id))
    }
    pub fn liked_by_artist(&self, artist: &str) -> Result<usize> {
        self.run(async |session| crate::native_browse::liked_by_artist(session, artist).await)
    }
    pub fn liked_count(&self) -> Result<usize> {
        self.run(crate::native_browse::liked_count)
    }
    pub fn list_duration(&self, key: &str) -> Result<(usize, u64)> {
        self.run(async |session| crate::native_browse::list_duration(session, key).await)
    }
    pub fn library(
        &self,
    ) -> Result<(
        Vec<crate::model::Playlist>,
        Vec<crate::model::LibraryFolder>,
    )> {
        self.run(crate::native_browse::library_with_folders)
    }
    pub fn move_to_folder(&self, playlist: &str, folder: Option<&str>) -> Result<()> {
        self.run(async |session| {
            crate::native_browse::move_to_folder(session, playlist, folder).await
        })
    }
    pub fn rename_folder(&self, id: &str, name: &str) -> Result<()> {
        self.run(async |session| crate::native_browse::rename_folder(session, id, name).await)
    }
    pub fn delete_folder(&self, id: &str) -> Result<()> {
        self.run(async |session| crate::native_browse::delete_folder(session, id).await)
    }
    pub fn create_folder(&self, name: &str) -> Result<String> {
        self.run(async |session| crate::native_browse::create_folder(session, name).await)
    }
    pub fn saved_items(&self, uris: &[String]) -> Result<Vec<bool>> {
        self.run(async |session| crate::collection::contains(session, uris).await)
    }
    pub fn set_saved(&self, uri: &str, saved: bool) -> Result<()> {
        self.run(async |session| crate::collection::write(session, uri, saved).await)
    }
    pub fn recent(&self) -> Result<Vec<crate::model::Track>> {
        self.run(crate::native_browse::recent)
    }
    pub fn add_to_playlist(
        &self,
        playlist_id: &str,
        tracks: &[crate::model::Track],
    ) -> Result<crate::native_browse::Added> {
        self.run(async |session| {
            crate::native_browse::add_to_playlist(session, playlist_id, tracks).await
        })
    }
    pub fn create_playlist(&self, name: &str) -> Result<String> {
        self.run(async |session| crate::native_browse::create_playlist(session, name).await)
    }
    pub fn edit_playlist(&self, id: &str, name: &str, description: &str) -> Result<()> {
        self.run(async |session| {
            crate::native_browse::edit_details(session, id, name, description).await
        })
    }
    pub fn set_playlist_cover(&self, id: &str, jpeg: &[u8]) -> Result<String> {
        self.run(async |session| crate::native_browse::set_cover(session, id, jpeg).await)
    }
    pub fn move_playlist_song(&self, id: &str, from: usize, uri: &str, to: usize) -> Result<()> {
        self.run(async |session| crate::native_browse::move_song(session, id, from, uri, to).await)
    }
    pub fn remove_from_playlist(&self, id: &str, index: usize, uri: &str) -> Result<()> {
        self.run(async |session| {
            crate::native_browse::remove_from_playlist(session, id, index, uri).await
        })
    }
    pub fn remove_playlist(&self, id: &str) -> Result<()> {
        self.run(async |session| crate::native_browse::remove_from_library(session, id).await)
    }
    pub(crate) fn run<T>(&self, task: impl AsyncFnOnce(&Session) -> Result<T>) -> Result<T> {
        let session = self.session()?;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(task(&session))
    }
    /// Whether `uri` is coming up in this computer's queue.
    pub fn coming_up(&self, uri: &str) -> bool {
        self.0.lock().is_ok_and(|connection| {
            connection.as_ref().is_some_and(|connection| {
                connection.queue_snapshot.lock().is_ok_and(|queue| {
                    queue
                        .as_ref()
                        .is_some_and(|queue| queue.iter().any(|item| item.uri == uri))
                })
            })
        })
    }
    pub fn queue(&self) -> Result<Vec<crate::model::Track>> {
        self.queue_details().map(|(tracks, _)| tracks)
    }
    /// The upcoming songs, and how many at the front the listener queued.
    pub fn queue_details(&self) -> Result<(Vec<crate::model::Track>, usize)> {
        let entries = {
            let connection = self
                .0
                .lock()
                .map_err(|_| anyhow::anyhow!("Native queue unavailable"))?;
            let connection = connection
                .as_ref()
                .filter(|connection| connection.ready.load(Ordering::Acquire))
                .context("Native audio is connecting")?;
            connection
                .queue_snapshot
                .lock()
                .map_err(|_| anyhow::anyhow!("Native queue unavailable"))?
                .clone()
                .context("Native queue is not ready")?
        };
        if entries.is_empty() {
            return Ok((Vec::new(), 0));
        }
        let uris: Vec<String> = entries.iter().map(|entry| entry.uri.clone()).collect();
        let session = self.session()?;
        let tracks = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(crate::native_browse::resolve_tracks(&session, &uris))?;
        let added = added_count(&entries, &tracks);
        Ok((tracks, added))
    }
    /// Song details for up to 50 song URIs.
    pub fn resolve(&self, uris: &[String]) -> Result<Vec<crate::model::Track>> {
        anyhow::ensure!(uris.len() <= 50, "Too many songs");
        let session = self.session()?;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(crate::native_browse::resolve_tracks(&session, uris))
    }
    pub fn devices(&self) -> Result<Vec<crate::model::Device>> {
        let connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Native devices unavailable"))?;
        let connection = connection
            .as_ref()
            .filter(|connection| connection.ready.load(Ordering::Acquire))
            .context("Native audio is connecting")?;
        let devices = connection
            .devices
            .lock()
            .map_err(|_| anyhow::anyhow!("Native devices unavailable"))?
            .snapshot();
        Ok(devices
            .into_iter()
            .map(|device| crate::model::Device {
                id: Some(device.id),
                name: device.name,
                is_active: device.is_active,
                volume_percent: device.volume_percent,
                is_offline: device.is_offline,
            })
            .collect())
    }
    pub(crate) fn connect_cluster(&self) -> Result<Option<librespot_protocol::connect::Cluster>> {
        let connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Native devices unavailable"))?;
        let connection = connection.as_ref().context("Native session unavailable")?;
        let cluster = connection
            .devices
            .lock()
            .map_err(|_| anyhow::anyhow!("Native devices unavailable"))?
            .cluster();
        Ok(cluster)
    }
    /// How far this computer's clock runs ahead of Spotify's, if measured.
    /// Whether this computer's player follows Jam transfers: off after
    /// leaving a Jam, so late ones from it can't change the song; on before
    /// joining one.
    pub(crate) fn follow_jam(&self, follow: bool) {
        let Ok(connection) = self.0.lock() else {
            return;
        };
        let Some(devices) = connection
            .as_ref()
            .and_then(|connection| connection.devices.lock().ok())
        else {
            return;
        };
        if follow {
            devices.join_jam();
        } else {
            devices.leave_jam();
        }
    }
    pub(crate) fn server_clock_offset_ms(&self) -> Option<i64> {
        let connection = self.0.lock().ok()?;
        connection
            .as_ref()?
            .devices
            .lock()
            .ok()?
            .server_clock_offset_ms()
    }
    pub fn transfer(&self, requested: &str) -> Result<()> {
        let session = self.session()?;
        let devices = self.devices()?;
        let target = transfer_target(requested, session.device_id(), &devices)?;
        let device = devices
            .iter()
            .find(|device| device.id.as_deref() == Some(target.as_str()));
        tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
            tokio::time::timeout(Duration::from_secs(15), session.spclient().transfer(session.device_id(), &target, None))
                .await.map_err(|_| anyhow::anyhow!("Speaker connection timed out. Check its power and Spotify connection, then retry."))?
                .map_err(|error| anyhow::anyhow!(transfer_failure(error.kind, device)))?;
            Ok(())
        })
    }
    /// The device list once Spotify reports `requested` active, or the
    /// latest list after `timeout`. Spotify accepts a transfer a few seconds
    /// before its cluster names the new device, and the picker follows the
    /// active device, so an earlier list would send controls back here.
    pub fn devices_after_transfer(
        &self,
        requested: &str,
        timeout: Duration,
    ) -> Result<Vec<crate::model::Device>> {
        let local = self.session()?.device_id().to_owned();
        let target = if requested == "native" {
            local.as_str()
        } else {
            requested
        };
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let devices = self.devices()?;
            if transfer_settled(target, &devices) || std::time::Instant::now() >= deadline {
                return Ok(devices);
            }
            thread::sleep(Duration::from_millis(200));
        }
    }
    pub fn home(&self, timezone: &str) -> Result<crate::home::HomePage> {
        let session = self.session()?;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(crate::home::fetch(&session, timezone))
    }
    pub fn search_playlists(&self, query: &str) -> Result<Vec<crate::model::Playlist>> {
        let session = self.session()?;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(crate::search::playlists(&session, query))
    }
    pub fn search_catalogue(&self, query: &str) -> Result<crate::search::SearchResults> {
        let session = self.session()?;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(crate::search::catalogue(&session, query))
    }
    pub fn songwriters(&self, track_id: &str) -> Result<Vec<String>> {
        let session = self.session()?;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(crate::catalogue::songwriters(&session, track_id))
    }
    /// Every release of an artist, newest first.
    pub fn discography(&self, id: &str) -> Result<Vec<crate::model::Album>> {
        let session = self.session()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let mut releases = Vec::new();
        let mut offset = 0;
        loop {
            let page = runtime.block_on(crate::catalogue::discography(&session, id, offset))?;
            let count = page["items"].as_array().map_or(0, Vec::len);
            releases.extend(crate::album::releases(&page));
            offset += count;
            let total = page["totalCount"].as_u64().unwrap_or_default() as usize;
            if count == 0 || offset >= total || offset >= 1000 {
                break;
            }
        }
        Ok(releases)
    }
    pub fn catalogue(&self, kind: &str, id: &str, offset: usize) -> Result<serde_json::Value> {
        use librespot_protocol::extension_kind::ExtensionKind;
        anyhow::ensure!(
            id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()) && offset <= 5000,
            "Invalid catalogue request"
        );
        let extension = match kind {
            "artist" => ExtensionKind::ARTIST_V4,
            "album" => ExtensionKind::ALBUM_V4,
            _ => anyhow::bail!("Unsupported catalogue query"),
        };
        serde_json::from_slice(&self.request_metadata(
            &format!("spotify:{kind}:{id}"),
            extension,
            offset,
        )?)
        .map_err(anyhow::Error::from)
    }
    /// Spotify's own lyrics for a track (color-lyrics JSON).
    pub fn lyrics(&self, track_id: &str) -> Result<Vec<u8>> {
        anyhow::ensure!(
            track_id.len() == 22 && track_id.bytes().all(|byte| byte.is_ascii_alphanumeric()),
            "Invalid track ID"
        );
        let session = self.session()?;
        let id = librespot_core::SpotifyId::from_base62(track_id)?;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async {
                tokio::time::timeout(Duration::from_secs(10), session.spclient().get_lyrics(&id))
                    .await
                    .map_err(|_| anyhow::anyhow!("Spotify lyrics timed out"))?
                    .map(|body| body.to_vec())
                    .map_err(|_| anyhow::anyhow!("Spotify has no lyrics for this song"))
            })
    }
    pub fn metadata(
        &self,
        uri: &str,
        kind: librespot_protocol::extension_kind::ExtensionKind,
    ) -> Result<Vec<u8>> {
        self.request_metadata(uri, kind, 0)
    }
    pub fn track(&self, uri: &str) -> Result<crate::model::Track> {
        use protobuf::Message;
        let bytes = self.metadata(
            uri,
            librespot_protocol::extension_kind::ExtensionKind::TRACK_V4,
        )?;
        let message = librespot_protocol::metadata::Track::parse_from_bytes(&bytes)?;
        let track = librespot_metadata::Track::try_from(&message)?;
        metadata_track(&track, None).context("Track metadata unavailable")
    }
    fn request_metadata(
        &self,
        uri: &str,
        kind: librespot_protocol::extension_kind::ExtensionKind,
        offset: usize,
    ) -> Result<Vec<u8>> {
        use librespot_protocol::extension_kind::ExtensionKind;
        anyhow::ensure!(
            matches!(
                kind,
                ExtensionKind::CANVAS_V1
                    | ExtensionKind::TRACK_V4
                    | ExtensionKind::CANVAZ
                    | ExtensionKind::STREAM_COUNT
                    | ExtensionKind::ARTIST_V4
                    | ExtensionKind::ALBUM_V4
            ),
            "Unsupported native metadata kind"
        );
        let parsed = librespot_core::SpotifyUri::from_uri(uri)?;
        anyhow::ensure!(
            matches!(
                parsed,
                librespot_core::SpotifyUri::Track { .. }
                    | librespot_core::SpotifyUri::Artist { .. }
                    | librespot_core::SpotifyUri::Album { .. }
            ),
            "Track metadata required"
        );
        let (reply, receive) = mpsc::channel();
        {
            let connection = self
                .0
                .lock()
                .map_err(|_| anyhow::anyhow!("Native metadata unavailable"))?;
            let sender = connection
                .as_ref()
                .filter(|connection| connection.ready.load(Ordering::Acquire))
                .and_then(|connection| connection.metadata.as_ref())
                .context("Connect native audio for metadata")?;
            sender
                .send(MetadataRead {
                    offset,
                    uri: uri.into(),
                    kind,
                    reply,
                })
                .map_err(|_| anyhow::anyhow!("Native metadata unavailable"))?;
        }
        receive
            .recv_timeout(Duration::from_secs(10))
            .context("Native metadata request timed out")?
            .map_err(anyhow::Error::msg)
    }
    pub fn playlist_tracks(&self, id: &str, offset: usize) -> Result<Vec<crate::model::Track>> {
        self.playlist_tracks_range(id, offset, 50)
    }
    /// Up to `limit` songs from `offset`, in one read.
    pub fn playlist_tracks_range(
        &self,
        id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<crate::model::Track>> {
        anyhow::ensure!(
            (id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()))
                || id == "liked"
                || id
                    .strip_prefix("search:")
                    .is_some_and(|query| !query.trim().is_empty()
                        && query.len() <= 600
                        && !query.chars().any(char::is_control))
                || ["album:", "liked-artist:"].into_iter().any(|prefix| id
                    .strip_prefix(prefix)
                    .is_some_and(
                        |id| id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
                    )),
            "Invalid browsing request"
        );
        let (reply, receive) = mpsc::channel();
        {
            let connection = self
                .0
                .lock()
                .map_err(|_| anyhow::anyhow!("Native Spotify connection unavailable"))?;
            let sender = connection
                .as_ref()
                .filter(|connection| connection.ready.load(Ordering::Acquire))
                .and_then(|connection| connection.playlists.as_ref())
                .context("Connect native audio to read this playlist")?;
            sender
                .send(PlaylistRead {
                    id: id.into(),
                    offset,
                    limit,
                    reply,
                })
                .map_err(|_| anyhow::anyhow!("Native Spotify connection unavailable"))?;
        }
        receive
            .recv_timeout(Duration::from_secs(25))
            .context("Spotify playlist request timed out")?
            .map_err(anyhow::Error::msg)
    }
    pub fn send(&self, action: PlayerAction) -> std::result::Result<(), PlayerAction> {
        if let PlayerAction::Enqueue(uri) = &action
            && queue_uri(uri).is_err()
        {
            return Err(action);
        }
        if !action.supports_native() {
            return Err(action);
        }
        let Ok(connection) = self.0.lock() else {
            return Err(action);
        };
        let Some(connection) = connection
            .as_ref()
            .filter(|connection| connection.ready.load(Ordering::Acquire))
        else {
            return Err(action);
        };
        if matches!(action, PlayerAction::Enqueue(_)) && !connection.loaded.load(Ordering::Acquire)
        {
            return Err(action);
        }
        if let PlayerAction::Volume(value) = action {
            connection
                .volume
                .store(u32::from(value.min(100)), Ordering::Release);
            return Ok(());
        }
        let loading = matches!(
            action,
            PlayerAction::Play(Some(_))
                | PlayerAction::PreparedDJ { .. }
                | PlayerAction::PlayUris { .. }
                | PlayerAction::Resume { .. }
                | PlayerAction::PlayContext { .. }
                | PlayerAction::RestoreDJ { .. }
        );
        if loading {
            connection.loaded.store(true, Ordering::Release);
        }
        if loading
            || matches!(
                action,
                PlayerAction::Next | PlayerAction::SkipTo(_) | PlayerAction::Previous
            )
        {
            connection.audio_buffer.interrupt(selected_track(&action));
        }
        let result = connection.sender.send(action).map_err(|error| error.0);
        if loading && result.is_err() {
            connection.loaded.store(false, Ordering::Release);
        }
        result
    }
}

fn selected_track(action: &PlayerAction) -> Option<String> {
    match action {
        PlayerAction::Play(Some(uri)) => Some(uri.clone()),
        PlayerAction::PlayContext { offset, .. } => offset.clone(),
        PlayerAction::RestoreDJ {
            track,
            relative_offset: 0,
            ..
        } => Some(track.clone()),
        PlayerAction::PlayUris { uris, offset, .. } | PlayerAction::Resume { uris, offset, .. } => {
            uris.get(*offset).cloned()
        }
        _ => None,
    }
}

fn preload_selection(action: &PlayerAction) -> Option<(librespot_core::SpotifyUri, u32)> {
    let (uri, position) = match action {
        PlayerAction::Play(Some(uri)) => (uri.as_str(), 0),
        PlayerAction::PlayUris { uris, offset, .. } => (uris.get(*offset)?.as_str(), 0),
        PlayerAction::Resume {
            uris,
            offset,
            position_ms,
            ..
        } => (
            uris.get(*offset)?.as_str(),
            (*position_ms).min(u64::from(u32::MAX)) as u32,
        ),
        _ => return None,
    };
    Some((librespot_core::SpotifyUri::from_uri(uri).ok()?, position))
}

/// How long a selection may take to start loading before Play gives up on it.
const LOAD_TIMEOUT: Duration = Duration::from_secs(20);

/// Transport commands for the local player diagnostics log.
fn command_kind(action: &PlayerAction) -> Option<&'static str> {
    Some(match action {
        PlayerAction::Play(None) => "play",
        PlayerAction::Play(Some(_)) => "play-track",
        PlayerAction::Pause => "pause",
        PlayerAction::Next => "next",
        PlayerAction::SkipTo(_) => "skip-to",
        PlayerAction::Previous => "previous",
        PlayerAction::Seek(_) => "seek",
        PlayerAction::PlayUris { .. } => "play-list",
        PlayerAction::PlayContext { .. } => "play-context",
        PlayerAction::Resume { .. } => "resume",
        PlayerAction::RestoreDJ { .. } => "restore-dj",
        PlayerAction::PreparedDJ { .. } => "dj",
        _ => return None,
    })
}

/// Player events for the local diagnostics log: the kind and track only.
/// Position updates are left out; they arrive many times a second.
fn event_summary(event: &PlayerEvent) -> Option<(&'static str, String)> {
    Some(match event {
        PlayerEvent::Stopped { track_id, .. } => ("stopped", track_id.to_uri()),
        PlayerEvent::Loading { track_id, .. } => ("loading", track_id.to_uri()),
        PlayerEvent::Preloading { track_id } => ("preloaded", track_id.to_uri()),
        PlayerEvent::Playing { track_id, .. } => ("playing", track_id.to_uri()),
        PlayerEvent::Paused { track_id, .. } => ("paused", track_id.to_uri()),
        PlayerEvent::TimeToPreloadNextTrack { track_id, .. } => ("preload-due", track_id.to_uri()),
        PlayerEvent::EndOfTrack { track_id, .. } => ("end-of-track", track_id.to_uri()),
        PlayerEvent::Unavailable { track_id, .. } => ("unavailable", track_id.to_uri()),
        PlayerEvent::Seeked { track_id, .. } => ("seeked", track_id.to_uri()),
        PlayerEvent::TrackChanged { audio_item, .. } => {
            ("track-changed", audio_item.track_id.to_uri())
        }
        _ => return None,
    })
}

/// Append one line to a small local log of transport commands and player
/// events, for diagnosing stalls. Holds no account data or credentials.
pub(crate) fn diagnostic(line: &str) {
    use std::io::Write;
    let Ok(dir) = crate::config::data_dir().map(|dir| dir.join("diagnostics")) else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("player-events.log");
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > 256 * 1024) {
        let _ = std::fs::remove_file(&path);
    }
    let when = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{when} {line}");
    }
}

/// Sends librespot's warnings and errors to the diagnostics log, so a load
/// Spotify Connect refused leaves a reason behind. Lines that mention a
/// token are dropped whole.
struct LibrespotLog;
impl log::Log for LibrespotLog {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Warn && metadata.target().starts_with("librespot")
    }
    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let message = record.args().to_string();
        if message.to_lowercase().contains("token") {
            return;
        }
        let message: String = message.chars().take(300).collect();
        diagnostic(&format!(
            "librespot {} {}: {message}",
            record.level(),
            record.target()
        ));
    }
    fn flush(&self) {}
}
fn log_librespot() {
    static LOG: LibrespotLog = LibrespotLog;
    if log::set_logger(&LOG).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    }
}

/// How many of the resolved songs, from the front, the listener queued.
/// Songs Spotify could not describe are left out of `tracks`, so they are
/// skipped here too.
fn added_count(entries: &[QueueEntry], tracks: &[crate::model::Track]) -> usize {
    let mut entries = entries.iter();
    let mut added = 0;
    for track in tracks {
        let Some(entry) = entries.find(|entry| entry.uri == track.uri) else {
            break;
        };
        if !entry.added {
            break;
        }
        added += 1;
    }
    added
}

fn is_dj_context(uri: &str) -> bool {
    uri == crate::home::DJ_URI
        || uri.starts_with("spotify:lexicon:")
        || uri == librespot_connect::REQUESTED_DJ_CONTEXT
}

fn remember_player_queue(account: &str, event: PlayerEvent) {
    let PlayerEvent::SetQueue {
        context_uri,
        session_url,
        track_uid,
        current_track: Some(track),
        next_tracks,
        prev_tracks,
    } = event
    else {
        return;
    };
    // The live order, shuffled or not, so a restart resumes and skips
    // through the songs the queue showed, not the start of a long list.
    let queue = prev_tracks
        .iter()
        .rev()
        .take(33)
        .rev()
        .chain(std::iter::once(&track))
        .chain(next_tracks.iter().take(66))
        .filter(|track| track.uri.starts_with("spotify:track:"))
        .map(|track| track.uri.clone())
        .collect();
    crate::resume::remember_native_queue(account, queue);
    crate::resume::remember_native_context(
        account,
        (!is_dj_context(&context_uri)).then(|| context_uri.clone()),
        next_tracks
            .iter()
            .filter(|track| track.provider == "queue")
            .map(|track| track.uri.clone())
            .collect(),
    );
    let context = if is_dj_context(&context_uri) {
        (context_uri != librespot_connect::REQUESTED_DJ_CONTEXT).then_some(
            crate::resume::DJContext {
                uri: context_uri,
                session_url,
                track_uid,
            },
        )
    } else {
        None
    };
    crate::resume::remember_dj_context(account, &track.uri, context);
}

#[cfg(test)]
mod dj_queue_tests {
    use super::*;
    #[test]
    fn skipping_a_song_on_repeat_one_repeats_the_playlist() {
        let mut modes = PlayModes::new(true, "track");
        assert!(modes.skipped());
        assert_eq!(modes, PlayModes::new(true, "context"));
        assert!(!modes.skipped());
        let mut off = PlayModes::new(false, "off");
        assert!(!off.skipped());
        assert_eq!(PlayModes::new(false, "loop").repeat, "off");
    }
    #[test]
    fn shuffled_playlist_queue_is_saved_in_its_live_order() {
        use librespot_playback::player::QueueTrack;
        let account = "test-native-live-order";
        let track = |id: char| QueueTrack {
            uri: format!("spotify:track:{}", id.to_string().repeat(22)),
            provider: "context".into(),
        };
        remember_player_queue(
            account,
            PlayerEvent::SetQueue {
                context_uri: "spotify:user:me:collection".into(),
                session_url: None,
                track_uid: None,
                current_track: Some(track('c')),
                prev_tracks: vec![track('a')],
                next_tracks: vec![track('z'), track('m')],
            },
        );
        let queue = crate::resume::queue(account);
        assert_eq!(queue.len(), 4);
        assert_eq!(queue[1], track('c').uri);
        assert!(matches!(
            crate::resume::skip_action(&queue[1], &queue, 0, true),
            Some(PlayerAction::Resume { offset: 2, .. })
        ));
    }

    #[test]
    fn queued_songs_are_counted_from_the_front_past_unresolved_ones() {
        let entry = |uri: &str, added| QueueEntry {
            uri: uri.into(),
            added,
        };
        let track = |uri: &str| crate::model::Track {
            uri: uri.into(),
            ..Default::default()
        };
        let entries = [
            entry("a", true),
            entry("gone", true),
            entry("b", true),
            entry("c", false),
            entry("d", true),
        ];
        // "gone" could not be resolved; "d" follows the playlist's songs.
        let tracks = [track("a"), track("b"), track("c"), track("d")];
        assert_eq!(added_count(&entries, &tracks), 2);
        assert_eq!(added_count(&entries[3..], &tracks[2..]), 0);
    }

    #[test]
    fn player_queue_remembers_dj_origin_but_manual_play_clears_it() {
        use librespot_playback::player::QueueTrack;
        let account = "test-native-dj-origin";
        let current = "spotify:track:abcdefghijklmnopqrstuv";
        let event = |context_uri: String, session_url: Option<String>| PlayerEvent::SetQueue {
            context_uri,
            session_url,
            track_uid: Some("set2-track3".into()),
            current_track: Some(QueueTrack {
                uri: current.into(),
                provider: "context".into(),
            }),
            prev_tracks: Vec::new(),
            next_tracks: vec![QueueTrack {
                uri: "spotify:track:bcdefghijklmnopqrstuvw".into(),
                provider: "context".into(),
            }],
        };
        remember_player_queue(
            account,
            event(
                crate::home::DJ_URI.into(),
                Some("hm://lexicon-session-provider/context-resolve/v2/session?id=test".into()),
            ),
        );
        assert_eq!(crate::resume::queue(account).len(), 2);
        assert!(
            matches!(crate::resume::restore_dj_action(account,current,77000,&PlayerAction::Next),Some(PlayerAction::RestoreDJ{relative_offset:1,track_uid:Some(uid),..}) if uid=="set2-track3")
        );
        remember_player_queue(account, event("spotify:album:manual".into(), None));
        assert!(
            crate::resume::restore_dj_action(account, current, 77000, &PlayerAction::Next)
                .is_none()
        );
        // Native request pages cannot be reopened through the legacy DJ URL.
        remember_player_queue(
            account,
            event(librespot_connect::REQUESTED_DJ_CONTEXT.into(), None),
        );
        assert!(
            crate::resume::restore_dj_action(account, current, 77000, &PlayerAction::Play(None))
                .is_none()
        );
    }
}

/// A context holding `uris` as one page, as Spotify would resolve it.
fn listed_context(uri: &str, uris: &[String]) -> librespot_protocol::context::Context {
    use librespot_protocol::{
        context::Context, context_page::ContextPage, context_track::ContextTrack,
    };
    Context {
        uri: Some(uri.into()),
        pages: vec![ContextPage {
            tracks: uris
                .iter()
                .map(|uri| ContextTrack {
                    uri: Some(uri.clone()),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// DJ narration and music keep their supplied order.
fn is_ordered(action: &PlayerAction) -> bool {
    matches!(
        action,
        PlayerAction::PreparedDJ { .. } | PlayerAction::RestoreDJ { .. }
    ) || matches!(action, PlayerAction::PlayContext { context, .. } if is_dj_context(context))
}

fn load_request(action: &PlayerAction, modes: &PlayModes) -> Option<LoadRequest> {
    // DJ starts in order with repeat off; everything else with the chosen
    // shuffle and repeat.
    let context_options = Some(if is_ordered(action) {
        librespot_connect::LoadContextOptions::Options(Default::default())
    } else {
        modes.options()
    });
    let options = |playing_track| LoadRequestOptions {
        start_playing: true,
        playing_track,
        context_options: context_options.clone(),
        ..LoadRequestOptions::default()
    };
    Some(match action {
        PlayerAction::PreparedDJ {
            context, provider, ..
        } => LoadRequest::from_context_uri(
            crate::home::DJ_URI.into(),
            LoadRequestOptions {
                prepared_context: Some(context.clone()),
                page_provider: Some(provider.clone()),
                ..options(Some(PlayingTrack::Index(0)))
            },
        ),
        PlayerAction::RestoreDJ {
            context,
            session_url,
            track,
            track_uid,
            position_ms,
            relative_offset,
        } => LoadRequest::from_context_uri(
            context.clone(),
            LoadRequestOptions {
                start_playing: true,
                seek_to: (*position_ms).min(u64::from(u32::MAX)) as u32,
                playing_track: Some(track_uid.clone().filter(|uid| !uid.is_empty()).map_or_else(
                    || PlayingTrack::Uri(track.clone()),
                    |uid| PlayingTrack::UidOrUri {
                        uid,
                        uri: track.clone(),
                    },
                )),
                session_url: session_url.clone(),
                relative_offset: *relative_offset,
                ..options(None)
            },
        ),
        PlayerAction::Play(Some(uri)) => {
            LoadRequest::from_tracks(vec![uri.clone()], options(Some(PlayingTrack::Index(0))))
        }
        PlayerAction::PlayContext { context, offset } => LoadRequest::from_context_uri(
            context.clone(),
            options(offset.clone().map(PlayingTrack::Uri)),
        ),
        // Songs already loaded from a playlist, album or Liked Songs play
        // under that context straight away: Spotify Connect and the library
        // see what is playing, and nothing waits on resolving the context.
        PlayerAction::PlayUris {
            uris,
            offset,
            context: Some(context),
        } => LoadRequest::from_context_uri(
            context.clone(),
            LoadRequestOptions {
                prepared_context: Some(Box::new(listed_context(context, uris))),
                ..options(Some(PlayingTrack::Index(
                    (*offset).min(u32::MAX as usize) as u32
                )))
            },
        ),
        PlayerAction::PlayUris { uris, offset, .. } => LoadRequest::from_tracks(
            uris.clone(),
            options(Some(PlayingTrack::Index(
                (*offset).min(u32::MAX as usize) as u32
            ))),
        ),
        // A list played in its own order, such as a Random sort, resumes in
        // that order under its playlist.
        PlayerAction::Resume {
            uris,
            offset,
            position_ms,
            context: Some(context),
            listed: true,
            ..
        } if uris.get(*offset).is_some() => LoadRequest::from_context_uri(
            context.clone(),
            LoadRequestOptions {
                seek_to: (*position_ms).min(u64::from(u32::MAX)) as u32,
                prepared_context: Some(Box::new(listed_context(context, uris))),
                ..options(Some(PlayingTrack::Index(
                    (*offset).min(u32::MAX as usize) as u32
                )))
            },
        ),
        // A restored song carries on through the playlist it came from, as
        // Spotify's own app restores it.
        PlayerAction::Resume {
            uris,
            offset,
            position_ms,
            context: Some(context),
            ..
        } if uris.get(*offset).is_some() => LoadRequest::from_context_uri(
            context.clone(),
            LoadRequestOptions {
                seek_to: (*position_ms).min(u64::from(u32::MAX)) as u32,
                ..options(uris.get(*offset).cloned().map(PlayingTrack::Uri))
            },
        ),
        PlayerAction::Resume {
            uris,
            offset,
            position_ms,
            ..
        } => LoadRequest::from_tracks(
            uris.clone(),
            LoadRequestOptions {
                seek_to: (*position_ms).min(u64::from(u32::MAX)) as u32,
                ..options(Some(PlayingTrack::Index(
                    (*offset).min(u32::MAX as usize) as u32
                )))
            },
        ),
        _ => return None,
    })
}

struct PreparedResume {
    uri: librespot_core::SpotifyUri,
    position_ms: u32,
    narration: Option<librespot_playback::narration::Narration>,
}

fn saved_context_track<'a>(
    saved: &crate::resume::LastPlayed,
    context: &'a librespot_protocol::context::Context,
) -> Option<&'a librespot_protocol::context_track::ContextTrack> {
    let tracks = || context.pages.iter().flat_map(|page| &page.tracks);
    let uid = saved.dj_context.as_ref()?.track_uid.as_deref();
    tracks()
        .find(|track| track.uri.as_ref() == Some(&saved.track.uri) && track.uid.as_deref() == uid)
        .or_else(|| tracks().find(|track| track.uri.as_ref() == Some(&saved.track.uri)))
}

async fn prepare_resume(
    session: &Session,
    saved: Option<crate::resume::LastPlayed>,
) -> Option<PreparedResume> {
    let saved = saved?;
    let uri = librespot_core::SpotifyUri::from_uri(&saved.track.uri).ok()?;
    let narration = if let Some(context) = &saved.dj_context {
        let context = session
            .spclient()
            .get_context_url(context.session_url.as_deref()?)
            .await
            .ok()?;
        let track = saved_context_track(&saved, &context)?;
        let narration: librespot_playback::narration::Narration = track
            .metadata
            .iter()
            .filter(|(key, _)| key.starts_with("narration."))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        (!narration.is_empty()).then_some(narration)
    } else {
        None
    };
    Some(PreparedResume {
        uri,
        position_ms: saved.position_ms.min(u64::from(u32::MAX)) as u32,
        narration,
    })
}

/// Shuffle and repeat as the listener last chose them. Every playlist,
/// album or song list starts with them, as in Spotify, and they outlast a
/// restart; DJ keeps its own order and doesn't change them.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayModes {
    pub shuffle: bool,
    /// "off", "context" or "track".
    pub repeat: String,
}

impl PlayModes {
    pub fn new(shuffle: bool, repeat: &str) -> Self {
        let repeat = if ["context", "track"].contains(&repeat) {
            repeat
        } else {
            "off"
        };
        Self {
            shuffle,
            repeat: repeat.into(),
        }
    }

    fn saved() -> Self {
        crate::config::Settings::load()
            .map(|settings| Self::new(settings.shuffle, &settings.repeat))
            .unwrap_or_else(|_| Self::new(false, "off"))
    }

    fn options(&self) -> librespot_connect::LoadContextOptions {
        librespot_connect::LoadContextOptions::Options(librespot_connect::Options {
            shuffle: self.shuffle,
            repeat: self.repeat == "context",
            repeat_track: self.repeat == "track",
        })
    }

    /// Skipping a song on repeat one moves on to repeating the playlist,
    /// as Spotify does. Whether it changed.
    fn skipped(&mut self) -> bool {
        let changed = self.repeat == "track";
        if changed {
            self.repeat = "context".into();
        }
        changed
    }
}

fn apply_control(
    spirc: &Spirc,
    action: PlayerAction,
    active: &mut bool,
    modes: &mut PlayModes,
    ordered: &mut bool,
) -> Result<()> {
    if let Some(request) = load_request(&action, modes) {
        spirc.activate()?;
        *active = true;
        *ordered = is_ordered(&action);
        spirc.load(request)?;
        // Spirc handles commands in order, so the restored queue follows the
        // loaded song.
        if let PlayerAction::Resume { queued, .. } = &action {
            for uri in queued {
                if let Ok(uri) = queue_uri(uri) {
                    spirc.add_to_queue(uri)?;
                }
            }
        }
        return Ok(());
    }
    if !*active && matches!(action, PlayerAction::Play(None)) {
        spirc.activate()?;
        *active = true;
    }
    match action {
        PlayerAction::Play(None) => spirc.play()?,
        PlayerAction::Pause => spirc.pause()?,
        PlayerAction::Next | PlayerAction::SkipTo(_) => {
            if *active && !*ordered && modes.skipped() {
                spirc.repeat_track(false)?;
                spirc.repeat(true)?;
            }
            match action {
                PlayerAction::SkipTo(uri) => spirc.next_to(uri)?,
                _ => spirc.next()?,
            }
        }
        PlayerAction::Previous => spirc.prev()?,
        PlayerAction::Seek(position) => {
            spirc.set_position_ms(position.min(u32::MAX as u64) as u32)?
        }
        PlayerAction::Volume(volume) => {
            spirc.set_volume((u32::from(volume.min(100)) * u32::from(u16::MAX) / 100) as u16)?
        }
        // Spirc ignores these until this computer plays; the next load
        // starts with them either way.
        PlayerAction::Shuffle(enabled) => {
            modes.shuffle = enabled;
            if *active {
                spirc.shuffle(enabled)?;
            }
        }
        PlayerAction::Repeat(state) => {
            *modes = PlayModes::new(modes.shuffle, &state);
            if *active {
                spirc.repeat(modes.repeat == "context")?;
                spirc.repeat_track(modes.repeat == "track")?;
            }
        }
        PlayerAction::Enqueue(uri) => spirc.add_to_queue(queue_uri(&uri)?)?,
        PlayerAction::RemoveFromQueue(uri) => spirc.remove_from_queue(queue_uri(&uri)?.to_uri())?,
        PlayerAction::ClearQueue => spirc.clear_queue()?,
        PlayerAction::HostJam(session_id) => spirc.host_jam(session_id)?,
        PlayerAction::Transfer(_) => {
            anyhow::bail!("This command requires Spotify's Web API")
        }
        _ => unreachable!("Load requests handled before transport commands"),
    }
    Ok(())
}

pub(crate) fn queue_uri(uri: &str) -> Result<librespot_core::SpotifyUri> {
    let parsed = librespot_core::SpotifyUri::from_uri(uri)
        .map_err(|_| anyhow::anyhow!("Choose a Spotify track or episode to add to queue"))?;
    anyhow::ensure!(
        matches!(
            parsed,
            librespot_core::SpotifyUri::Track { .. }
                | librespot_core::SpotifyUri::Episode { .. }
                | librespot_core::SpotifyUri::Local { .. }
        ),
        "Choose a Spotify track or episode to add to queue"
    );
    Ok(parsed)
}

fn transfer_target(
    requested: &str,
    local: &str,
    devices: &[crate::model::Device],
) -> Result<String> {
    let target = if requested == "native" {
        local
    } else {
        requested
    };
    anyhow::ensure!(
        !target.is_empty()
            && target.len() <= 128
            && target
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "Invalid Spotify Connect destination"
    );
    if target == local {
        return Ok(target.into());
    }
    listed_device(target, devices)
        .and_then(|device| device.id.clone())
        .context("Refresh devices before connecting to this speaker")
}

/// Whether Spotify's device list names the transfer destination active.
fn transfer_settled(target: &str, devices: &[crate::model::Device]) -> bool {
    listed_device(target, devices).is_some_and(|device| device.is_active)
}

/// The listed device for an identifier the UI may have kept from an earlier
/// device list. Alexa speakers switch between alias and bare identifiers as
/// Spotify's listings arrive; both reach the same speaker.
pub(crate) fn listed_device<'a>(
    requested: &str,
    devices: &'a [crate::model::Device],
) -> Option<&'a crate::model::Device> {
    devices
        .iter()
        .find(|device| device.id.as_deref() == Some(requested))
        .or_else(|| {
            let speaker = librespot_connect::speaker_id(requested);
            devices.iter().find(|device| {
                device
                    .id
                    .as_deref()
                    .is_some_and(|id| librespot_connect::speaker_id(id) == speaker)
            })
        })
}

/// Spotify answers a transfer to a speaker it cannot reach with HTTP 410,
/// which librespot reports as `NotFound`. Standby Alexa speakers do this
/// until they reconnect to Spotify from the speaker side.
fn transfer_failure(
    kind: librespot_core::error::ErrorKind,
    device: Option<&crate::model::Device>,
) -> String {
    let name = device
        .map(|device| device.name.as_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("The speaker");
    match (kind, device.is_some_and(|device| device.is_offline)) {
        (librespot_core::error::ErrorKind::NotFound, true) => format!(
            "{name} is on standby in Spotify Connect, so Spotify could not reach it. Start Spotify on the speaker itself (for Alexa, ask it to play Spotify), then retry."
        ),
        (librespot_core::error::ErrorKind::NotFound, false) => format!(
            "Spotify could not reach {name}. Refresh devices and retry; if it still fails, start Spotify on the speaker itself."
        ),
        (kind, _) => format!(
            "Could not transfer playback ({kind}). Check the speaker's power and Spotify connection, then retry."
        ),
    }
}

fn public_track_change(event: &PlayerEvent) -> Option<crate::model::Track> {
    let PlayerEvent::TrackChanged {
        track_id,
        audio_item,
    } = event
    else {
        return None;
    };
    // Local files carry their details from the folder scan, cover included.
    if matches!(track_id, librespot_core::SpotifyUri::Local { .. }) {
        let uri = track_id.to_uri();
        return crate::local_files::track(&uri).or_else(|| {
            Some(crate::model::Track {
                name: audio_item.name.clone(),
                duration_ms: audio_item.duration_ms.into(),
                is_local: true,
                uri,
                ..crate::model::Track::default()
            })
        });
    }
    let mut track = public_track(audio_item)?;
    // Metadata may describe an available alternative. Transport and queue
    // events still use the originally requested track, so keep that identity.
    track.uri = track_id.to_uri();
    track.id = track.uri.rsplit(':').next().map(str::to_owned);
    Some(track)
}

fn public_track(item: &librespot_metadata::audio::item::AudioItem) -> Option<crate::model::Track> {
    use crate::model::{Album, Artist, Artwork, Track};
    use librespot_metadata::{artist::ArtistRole, audio::item::UniqueFields};
    let UniqueFields::Track { artists, album, .. } = &item.unique_fields else {
        return None;
    };
    let uri = item.track_id.to_uri();
    let mut images: Vec<Artwork> = item
        .covers
        .iter()
        .map(|image| Artwork {
            url: image.url.clone(),
            width: u32::try_from(image.width).ok(),
        })
        .collect();
    images.sort_by_key(|image| std::cmp::Reverse(image.width));
    Some(Track {
        explicit: item.is_explicit,
        id: uri.rsplit(':').next().map(str::to_owned),
        uri,
        name: item.name.clone(),
        duration_ms: item.duration_ms.into(),
        songwriters: artists
            .iter()
            .filter(|artist| artist.role == ArtistRole::ARTIST_ROLE_COMPOSER)
            .map(|artist| artist.name.clone())
            .collect(),
        artists: artists
            .iter()
            .filter(|artist| {
                !matches!(
                    artist.role,
                    ArtistRole::ARTIST_ROLE_COMPOSER | ArtistRole::ARTIST_ROLE_CONDUCTOR
                )
            })
            .map(|artist| {
                let uri = artist.id.to_uri();
                Artist {
                    id: uri.rsplit(':').next().map(str::to_owned),
                    uri,
                    name: artist.name.clone(),
                    ..Artist::default()
                }
            })
            .collect(),
        album: Album {
            name: album.clone(),
            images,
            ..Album::default()
        },
        ..Track::default()
    })
}

pub struct NativeMessage {
    pub generation: u64,
    pub event: NativeEvent,
}

/// Spotify Connect name of this computer's player, unique per PC so the Web
/// API device list can be matched even when its IDs differ from librespot's.
/// Audible position when playback starts. A resume reuses the position
/// recorded at pause; otherwise subtract queued audio, as position updates do,
/// so lyrics don't start ahead of the sound and then pull back.
fn playing_position(decoded_ms: u32, queued_ms: u64, resumed: Option<u64>) -> u64 {
    resumed.unwrap_or_else(|| u64::from(decoded_ms).saturating_sub(queued_ms))
}

/// The Spotify Connect name: the computer first, as Spotify's own client
/// names itself, then the app, e.g. "ARASHII-PC (Lyssna)".
pub fn device_name() -> String {
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    named_device(&host)
}

fn named_device(host: &str) -> String {
    match host.trim() {
        "" => "Lyssna".into(),
        host => format!("{host} (Lyssna)"),
    }
}

// Reusable credentials are protected by the platform credential backend.
// macOS stores only authenticated ciphertext on disk; credentials never enter logs.
pub(crate) fn load_credentials(account: &str) -> Result<Option<Credentials>> {
    let _guard = CREDENTIAL_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Audio credential store unavailable"))?;
    let entry = keyring::Entry::new(CREDENTIAL_SERVICE, account)?;
    match entry.get_password() {
        Ok(value) => {
            if let Ok(credentials) = serde_json::from_str::<Credentials>(&value)
                && valid_credentials(&credentials, account)
            {
                return Ok(Some(credentials));
            }
            entry
                .delete_credential()
                .context("Cannot remove invalid saved audio login")?;
            Ok(None)
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => anyhow::bail!("Cannot read saved audio login from the secure credential store"),
    }
}

fn valid_credentials(credentials: &Credentials, account: &str) -> bool {
    !account.is_empty()
        && credentials.username.as_deref() == Some(account)
        && credentials.auth_type == AuthenticationType::AUTHENTICATION_STORED_SPOTIFY_CREDENTIALS
        && !credentials.auth_data.is_empty()
}

fn save_credentials(
    account: &str,
    credentials: &Credentials,
    cancelled: &AtomicBool,
) -> Result<()> {
    let _guard = CREDENTIAL_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Audio credential store unavailable"))?;
    if cancelled.load(Ordering::Relaxed) {
        return Ok(());
    }
    anyhow::ensure!(
        valid_credentials(credentials, account),
        "Audio sign-in belongs to a different Spotify account. Reconnect and select the account used in Lyssna."
    );
    keyring::Entry::new(CREDENTIAL_SERVICE, account)?
        .set_password(&serde_json::to_string(credentials)?)
        .context("Cannot save audio login to the secure credential store")
}

fn forget_audio_credentials(account: &str) -> Result<()> {
    let _guard = CREDENTIAL_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Audio credential store unavailable"))?;
    match keyring::Entry::new(CREDENTIAL_SERVICE, account)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => anyhow::bail!("Cannot remove rejected audio login"),
    }
}

pub fn forget_credentials(account: &str) -> Result<()> {
    let _guard = CREDENTIAL_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Audio credential store unavailable"))?;
    if account.is_empty() {
        return Ok(());
    }
    let active = keyring::Entry::new("Lyssna/Account", "active")?;
    if active
        .get_password()
        .ok()
        .and_then(|value| serde_json::from_str::<crate::model::Profile>(&value).ok())
        .is_some_and(|profile| profile.id == account)
    {
        active
            .delete_credential()
            .context("Cannot remove saved Spotify account")?;
    }
    match keyring::Entry::new(CREDENTIAL_SERVICE, account)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => anyhow::bail!("Cannot remove saved audio login from the secure credential store"),
    }
}

pub(crate) fn saved_profile() -> Result<Option<crate::model::Profile>> {
    let _guard = CREDENTIAL_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Spotify credential store unavailable"))?;
    match keyring::Entry::new("Lyssna/Account", "active")?.get_password() {
        Ok(value) => Ok(Some(
            serde_json::from_str(&value)
                .context("Saved Spotify account is invalid; sign in again")?,
        )),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => anyhow::bail!("Cannot read saved Spotify account from the credential store"),
    }
}

/// Updates the saved account profile (name and picture) for later restores.
pub(crate) fn save_profile(profile: &crate::model::Profile) -> Result<()> {
    let _guard = CREDENTIAL_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Spotify credential store unavailable"))?;
    keyring::Entry::new("Lyssna/Account", "active")?
        .set_password(&serde_json::to_string(profile)?)
        .context("Cannot save Spotify account")
}

pub(crate) fn save_login(
    profile: &crate::model::Profile,
    credentials: &Credentials,
    cancelled: &AtomicBool,
) -> Result<()> {
    let _guard = CREDENTIAL_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Spotify credential store unavailable"))?;
    anyhow::ensure!(!cancelled.load(Ordering::Acquire), "Sign-in cancelled");
    anyhow::ensure!(
        valid_credentials(credentials, &profile.id),
        "Spotify account does not match saved credentials"
    );
    keyring::Entry::new(CREDENTIAL_SERVICE, &profile.id)?
        .set_password(&serde_json::to_string(credentials)?)
        .context("Cannot save Spotify login")?;
    keyring::Entry::new("Lyssna/Account", "active")?
        .set_password(&serde_json::to_string(profile)?)
        .context("Cannot save Spotify account")?;
    Ok(())
}

enum Outcome {
    Stopped,
    Failed(Failure),
}

pub(crate) fn sign_in_error(error: librespot_core::Error) -> anyhow::Error {
    let failure = Failure::from_connection(error);
    if failure.rejected_credentials {
        crate::auth::SignInRequired::NativeRejected.into()
    } else {
        anyhow::anyhow!(failure.reason)
    }
}

struct Failure {
    reason: String,
    rejected_credentials: bool,
    retryable: bool,
}

impl Failure {
    fn other(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            rejected_credentials: false,
            retryable: false,
        }
    }
    fn transient(reason: impl Into<String>) -> Self {
        Self {
            retryable: true,
            ..Self::other(reason)
        }
    }

    fn from_connection(error: librespot_core::Error) -> Self {
        // The AP error type is private in librespot 0.8. Match only its exact
        // credential-rejection messages; other permission errors are not login expiry.
        let message = error.error.to_string();
        let rejected_credentials = error.kind == ErrorKind::Unauthenticated
            || (error.kind == ErrorKind::PermissionDenied
                && matches!(
                    message.as_str(),
                    "Login failed with reason: Bad credentials"
                        | "Login failed with reason: Could not validate credentials"
                ));
        let reason = if rejected_credentials {
            "Spotify rejected the audio login".into()
        } else if message == "Login failed with reason: Premium account required" {
            "Spotify reports this audio account requires Premium".into()
        } else {
            format!("Spotify audio connection failed: {}", error.kind)
        };
        Self {
            reason,
            rejected_credentials,
            retryable: !rejected_credentials
                && message != "Login failed with reason: Premium account required"
                && matches!(
                    error.kind,
                    ErrorKind::Unavailable
                        | ErrorKind::DeadlineExceeded
                        | ErrorKind::Aborted
                        | ErrorKind::Unknown
                        | ErrorKind::Internal
                        | ErrorKind::ResourceExhausted
                ),
        }
    }
}

fn connect_with_credentials(
    stored: Option<Credentials>,
    cancelled: &AtomicBool,
    mut authorize: impl FnMut() -> Result<Credentials>,
    mut connect: impl FnMut(Credentials) -> Outcome,
    mut forget: impl FnMut() -> Result<()>,
) -> Outcome {
    if cancelled.load(Ordering::Relaxed) {
        return Outcome::Stopped;
    }
    if let Some(credentials) = stored {
        match connect(credentials) {
            Outcome::Failed(failure) if failure.rejected_credentials => {
                if cancelled.load(Ordering::Relaxed) {
                    return Outcome::Stopped;
                }
                if let Err(error) = forget() {
                    return Outcome::Failed(Failure::other(error.to_string()));
                }
            }
            outcome => return outcome,
        }
    }
    if cancelled.load(Ordering::Relaxed) {
        return Outcome::Stopped;
    }
    let credentials = match authorize() {
        Ok(credentials) => credentials,
        Err(_) if cancelled.load(Ordering::Relaxed) => return Outcome::Stopped,
        Err(error) => {
            return Outcome::Failed(Failure::other(format!(
                "Audio authorization failed: {error}"
            )));
        }
    };
    if cancelled.load(Ordering::Relaxed) {
        return Outcome::Stopped;
    }
    connect(credentials)
}

pub struct NativePlayer {
    stop: Option<oneshot::Sender<()>>,
    cancelled: Arc<AtomicBool>,
    ready: Arc<AtomicBool>,
}

impl NativePlayer {
    pub fn start(
        account: String,
        generation: u64,
        events: mpsc::Sender<NativeMessage>,
        repaint: crate::notify::Notifier,
        controls: NativeControls,
    ) -> Self {
        Self::start_with_login(account, generation, events, repaint, controls, None)
    }

    pub(crate) fn start_authenticated(
        login: crate::native_login::Authenticated,
        generation: u64,
        events: mpsc::Sender<NativeMessage>,
        repaint: crate::notify::Notifier,
        controls: NativeControls,
    ) -> Self {
        Self::start_with_login(
            login.profile.id.clone(),
            generation,
            events,
            repaint,
            controls,
            Some(login),
        )
    }

    fn start_with_login(
        account: String,
        generation: u64,
        events: mpsc::Sender<NativeMessage>,
        repaint: crate::notify::Notifier,
        controls: NativeControls,
        login: Option<crate::native_login::Authenticated>,
    ) -> Self {
        log_librespot();
        let (stop, mut stopped) = oneshot::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        let ready = Arc::new(AtomicBool::new(false));
        let thread_ready = ready.clone();
        let loaded = Arc::new(AtomicBool::new(false));
        let queue_snapshot = Arc::new(Mutex::new(None));
        let audio_buffer = crate::audio::AudioBuffer::default();
        let connection_status = Arc::new(Mutex::new(None));
        let (sender, mut commands) = async_channel::unbounded_channel();
        let (playlist_sender, mut playlists) = async_channel::unbounded_channel();
        let (metadata_sender, mut metadata) = async_channel::unbounded_channel();
        let shared_session = Arc::new(Mutex::new(None));
        let devices = Arc::new(Mutex::new(librespot_connect::ConnectedDevices::default()));
        let volume = crate::config::Settings::load()
            .map(|settings| settings.volume_percent.min(100))
            .unwrap_or(100);
        let latest_volume = Arc::new(AtomicU32::new(u32::from(volume)));
        if let Ok(settings) = crate::config::Settings::load() {
            audio_buffer.set_crossfade(settings.active_crossfade_seconds());
            audio_buffer.output.request(settings.audio_output);
        }
        if let Ok(mut connection) = controls.0.lock() {
            *connection = Some(ControlConnection {
                devices: devices.clone(),
                audio_buffer: audio_buffer.clone(),
                account: account.clone(),
                status: connection_status.clone(),
                session: shared_session.clone(),
                dj_requests: Default::default(),
                volume: latest_volume.clone(),
                sender,
                ready: ready.clone(),
                loaded: loaded.clone(),
                queue_snapshot: queue_snapshot.clone(),
                playlists: Some(playlist_sender),
                metadata: Some(metadata_sender),
            });
        }
        thread::spawn(move || {
            let emit = |event| {
                if matches!(
                    &event,
                    NativeEvent::Ready(_)
                        | NativeEvent::Authorizing
                        | NativeEvent::Reconnecting(_)
                        | NativeEvent::Error(_)
                ) && let Ok(mut status) = connection_status.lock()
                {
                    *status = Some(event.clone());
                }
                let _ = events.send(NativeMessage { generation, event });
                repaint.request_repaint();
            };
            let (runtime, mut authenticated, mut last_credentials) = if let Some(login) = login {
                (
                    login.runtime,
                    Some(login.connection),
                    Some(login.credentials),
                )
            } else {
                let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .worker_threads(2)
                    .build()
                else {
                    emit(NativeEvent::Error(
                        "Cannot start native audio runtime".into(),
                    ));
                    return;
                };
                (runtime, None, None)
            };
            let mut backoff = crate::reconnect::ReconnectBackoff::default();
            loop {
                if cancel.load(Ordering::Acquire) {
                    break;
                }
                let stored = if authenticated.is_some() {
                    last_credentials.clone()
                } else {
                    match load_credentials(&account) {
                        Ok(credentials) => credentials,
                        Err(error) => {
                            emit(NativeEvent::Error(error.to_string()));
                            return;
                        }
                    }
                    .or_else(|| last_credentials.clone())
                };
                let started = std::time::Instant::now();
                let outcome = connect_with_credentials(
                    stored,
                    &cancel,
                    || {
                        emit(NativeEvent::Authorizing);
                        crate::auth::http_client()
                            .and_then(|client| crate::auth::authorize_audio(&client, &cancel))
                            .map(Credentials::with_access_token)
                    },
                    |credentials| {
                        last_credentials = Some(credentials.clone());
                        let connected = authenticated.take();
                        runtime.block_on(run(
                            ConnectionLogin {
                                credentials,
                                session: connected.as_ref().map(|connection| connection.0.clone()),
                            },
                            &account,
                            &cancel,
                            &mut stopped,
                            PlaybackServices {
                                devices: &devices,
                                audio_buffer: &audio_buffer,
                                commands: &mut commands,
                                playlists: &mut playlists,
                                metadata: &mut metadata,
                                latest_volume: &latest_volume,
                                shared_session: &shared_session,
                                loaded: &loaded,
                                queue_snapshot: &queue_snapshot,
                            },
                            &thread_ready,
                            &emit,
                        ))
                    },
                    || forget_audio_credentials(&account),
                );
                thread_ready.store(false, Ordering::Release);
                loaded.store(false, Ordering::Release);
                if let Ok(mut queue) = queue_snapshot.lock() {
                    *queue = None;
                }
                audio_buffer.interrupt(None);
                audio_buffer.output.retire_stream();
                if let Ok(mut session) = shared_session.lock() {
                    *session = None;
                }
                if let Outcome::Failed(failure) = outcome
                    && !cancel.load(Ordering::Relaxed)
                {
                    if failure.retryable {
                        let delay = backoff.next(started.elapsed());
                        emit(NativeEvent::Reconnecting(delay.as_secs()));
                        if runtime.block_on(crate::reconnect::wait(delay, &mut stopped)) {
                            continue;
                        }
                        break;
                    }
                    emit(NativeEvent::Error(format!(
                        "{}. Reconnect native audio in Settings.",
                        failure.reason
                    )));
                }
                break;
            }
        });
        Self {
            stop: Some(stop),
            cancelled,
            ready,
        }
    }
}

struct ConnectionLogin {
    credentials: Credentials,
    session: Option<Session>,
}

async fn run(
    login: ConnectionLogin,
    account: &str,
    cancelled: &AtomicBool,
    stopped: &mut oneshot::Receiver<()>,
    channels: PlaybackServices<'_>,
    ready: &AtomicBool,
    emit: &impl Fn(NativeEvent),
) -> Outcome {
    let PlaybackServices {
        devices,
        audio_buffer,
        commands,
        playlists,
        metadata,
        latest_volume,
        shared_session,
        loaded,
        queue_snapshot,
    } = channels;
    loaded.store(false, Ordering::Release);
    let (session, credentials) = match login.session {
        Some(session) => (session, None),
        None => (
            Session::new(
                SessionConfig {
                    client_id: crate::auth::AUDIO_CLIENT_ID.into(),
                    ..SessionConfig::default()
                },
                crate::audio_cache::cache().ok(),
            ),
            Some(login.credentials),
        ),
    };
    let mixer = match SoftMixer::open(MixerConfig::default()) {
        Ok(mixer) => Arc::new(mixer),
        Err(_) => return Outcome::Failed(Failure::other("Cannot initialize audio mixer")),
    };
    let flush_audio = Arc::new(AtomicBool::new(false));
    let sink_flush = flush_audio.clone();
    let (seek_ready, mut audible_seeks) = async_channel::unbounded_channel();
    let sink_buffer = audio_buffer.clone();
    let sink_mixer = mixer.clone();
    let player = Player::new(
        PlayerConfig {
            position_update_interval: Some(Duration::from_millis(250)),
            ..PlayerConfig::default()
        },
        session.clone(),
        Box::new(librespot_playback::mixer::NoOpVolume),
        move || {
            let mut sink = crate::audio::AudioSink::new(
                sink_flush.clone(),
                seek_ready.clone(),
                sink_buffer.clone(),
            )
            .with_mixer(sink_mixer.clone());
            let _ = sink.prepare_output();
            Box::new(sink)
        },
    );
    let mut player_events = player.get_player_event_channel();
    let local_player = player.clone();
    let config = ConnectConfig {
        resume_on_startup: false,
        emit_set_queue_events: true,
        name: device_name(),
        // A desktop client is a computer to Spotify, as the official app is.
        // Discord's listen-along only plays on computers.
        device_type: librespot_core::config::DeviceType::Computer,
        initial_volume: crate::config::Settings::load()
            .ok()
            .map(|settings| {
                (u32::from(settings.volume_percent.min(100)) * u32::from(u16::MAX) / 100) as u16
            })
            .unwrap_or(u16::MAX),
        ..ConnectConfig::default()
    };
    let connection = tokio::select! {
        _ = &mut *stopped => { session.shutdown(); return Outcome::Stopped; }
        result = tokio::time::timeout(Duration::from_secs(40), Spirc::new_with_session(config, session.clone(), credentials, player, mixer.clone())) => result,
    };
    let (spirc, task) = match connection {
        Ok(Ok(connection)) => connection,
        failure => {
            session.shutdown();
            return Outcome::Failed(match failure {
                Ok(Err(error)) => Failure::from_connection(error),
                _ => Failure::transient("Audio connection timed out after 40 seconds"),
            });
        }
    };
    if let Ok(mut devices) = devices.lock() {
        *devices = spirc.devices();
    }
    if cancelled.load(Ordering::Relaxed) {
        session.shutdown();
        return Outcome::Stopped;
    }
    let reusable = Credentials {
        username: Some(session.username()),
        auth_type: AuthenticationType::AUTHENTICATION_STORED_SPOTIFY_CREDENTIALS,
        auth_data: session.auth_data(),
    };
    if reusable.username.as_deref() != Some(account) {
        let _ = spirc.shutdown();
        session.shutdown();
        return Outcome::Failed(Failure::other(
            "Audio sign-in belongs to a different Spotify account. Reconnect and select the account used in Lyssna",
        ));
    }
    if !reusable.auth_data.is_empty()
        && let Err(error) = save_credentials(account, &reusable, cancelled)
    {
        emit(NativeEvent::Error(format!("{error:#}")));
    }
    if cancelled.load(Ordering::Relaxed) {
        session.shutdown();
        return Outcome::Stopped;
    }
    // Creating Spirc authenticates the session; its task still has to register
    // the Connect sender with Spotify. Remote commands before that acknowledgement
    // can be rejected with PermissionDenied. Drive the task while waiting.
    tokio::pin!(task);
    let registration = async {
        loop {
            if cancelled.load(Ordering::Acquire) {
                return false;
            }
            if spirc
                .devices()
                .snapshot()
                .iter()
                .any(|device| device.id == session.device_id())
            {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    let registered = tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(20), registration) => result.ok(),
        _ = &mut *stopped => Some(false),
        _ = &mut task => None,
    };
    if registered != Some(true) {
        session.shutdown();
        return if registered == Some(false) {
            Outcome::Stopped
        } else {
            Outcome::Failed(Failure::transient(
                "Spotify Connect registration did not complete",
            ))
        };
    }
    if let Ok(mut shared) = shared_session.lock() {
        *shared = Some(session.clone());
    }
    ready.store(true, Ordering::Release);
    emit(NativeEvent::Ready(session.device_id().to_owned()));
    let _ = spirc.prewarm_dj();
    let mut active = false;
    let mut modes = PlayModes::saved();
    let mut ordered = false;
    let mut audio_playing = false;
    let mut intent = crate::playback_intent::PlaybackIntent::default();
    // While following a Jam its host decides play and pause; songs it loads
    // must not be resumed as if the listener had picked them.
    let mut jam_following = false;
    let mut unavailable = crate::playback_intent::UnavailableWatch::default();
    let mut intent_tick = tokio::time::interval(Duration::from_millis(20));
    let mut dj_prewarm = tokio::time::interval(Duration::from_secs(300));
    dj_prewarm.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut volume_sync: Option<(u8, std::time::Instant)> = None;
    let mut volume_guard: Option<(u8, std::time::Instant)> = None;
    let mut pending_seek: Option<(String, u64)> = None;
    let mut paused_position: Option<(String, u64)> = None;
    let mut narrating = false;
    let mut resume_preparation = Box::pin(tokio::time::timeout(
        Duration::from_secs(8),
        prepare_resume(&session, crate::resume::load(account)),
    ));
    let mut resume_prepared = false;
    let mut connection_lost = false;
    // A load Spotify Connect never starts would leave Play doing nothing.
    let mut load_due: Option<std::time::Instant> = None;
    loop {
        tokio::select! {
            prepared = &mut resume_preparation, if !resume_prepared => {
                resume_prepared = true;
                // The native command loop owns the player. Late preparation cannot
                // replace a selection or narration after the user starts playback.
                if !active && !loaded.load(Ordering::Acquire)
                    && let Ok(Some(prepared)) = prepared
                {
                    local_player.set_narration(prepared.uri.clone(), prepared.narration);
                    local_player.preload_at(prepared.uri, prepared.position_ms);
                }
            }
            Some(request)=metadata.recv()=>{let session=session.clone();tokio::spawn(async move{let result=async {let uri=librespot_core::SpotifyUri::from_uri(&request.uri)?;let bytes=if matches!(request.kind,librespot_protocol::extension_kind::ExtensionKind::ARTIST_V4|librespot_protocol::extension_kind::ExtensionKind::ALBUM_V4){let kind=if request.kind==librespot_protocol::extension_kind::ExtensionKind::ARTIST_V4{"artist"}else{"album"};let id=request.uri.rsplit(":").next().context("Missing catalogue ID")?;let data=crate::catalogue::page(&session,kind,id,request.offset).await?;serde_json::to_vec(&data)?.into()}else if request.kind==librespot_protocol::extension_kind::ExtensionKind::CANVAS_V1{let text=uri.to_uri();let mut body=vec![10,(text.len()+2)as u8,10,text.len()as u8];body.extend_from_slice(text.as_bytes());session.spclient().request(&reqwest::Method::POST,"/canvaz-cache/v0/canvases",None,Some(&body)).await?}else{session.spclient().get_metadata(request.kind,&uri).await?};anyhow::ensure!(bytes.len()<=1_000_000,"Metadata response too large");Ok::<_,anyhow::Error>(bytes.to_vec())};let result=tokio::time::timeout(Duration::from_secs(8),result).await.map_err(|_|"Metadata request timed out".to_owned()).and_then(|value|value.map_err(|error|error.to_string()));let _=request.reply.send(result);});}
            _=intent_tick.tick()=>{},
            _=dj_prewarm.tick(),if audio_playing=>{let _=spirc.prewarm_dj();},
            Some(())=audible_seeks.recv()=>{if let Some((uri,position_ms))=pending_seek.take(){emit(NativeEvent::Position{uri,position_ms,playing:Some(audio_playing),seeked:true});}}
            Some(request)=playlists.recv()=>{
                let session=session.clone();
                tokio::spawn(async move{
                    let result=tokio::time::timeout(Duration::from_secs(22),crate::native_browse::read_range(&session,&request.id,request.offset,request.limit)).await.map_err(|_|"Spotify browsing request timed out".to_owned()).and_then(|result|result.map_err(|error|error.to_string()));
                    let _=request.reply.send(result);
                });
            }
            _ = &mut *stopped => {
                let _ = spirc.shutdown();
                let _ = tokio::time::timeout(Duration::from_secs(2), &mut task).await;
                break;
            }
            _ = &mut task => { connection_lost = true; break; }
            Some(action) = commands.recv() => {
                if let Some(kind) = command_kind(&action) {
                    diagnostic(&format!("command {kind}"));
                }
                if let PlayerAction::PreparedDJ{cancelled,..}=&action && cancelled.load(Ordering::Acquire){continue;}
                if let Some((uri, position)) = preload_selection(&action) {
                    local_player.preload_at(uri, position);
                }
                if matches!(action,PlayerAction::Play(None)|PlayerAction::Pause){
                    if matches!(action,PlayerAction::Play(None))&&!active{
                        if spirc.activate().is_err(){loaded.store(false,Ordering::Release);emit(NativeEvent::ControlError("Native playback command failed. Reconnect native audio in Settings.".into()));continue;}
                        active=true;
                    }
                    intent.request(matches!(action,PlayerAction::Play(None)));
                }else{
                if matches!(action,PlayerAction::Play(Some(_))|PlayerAction::PreparedDJ{..}|PlayerAction::PlayUris{..}|PlayerAction::Resume{..}|PlayerAction::PlayContext{..}|PlayerAction::RestoreDJ{..}){paused_position=None;intent.loading_track(selected_track(&action),std::time::Instant::now());load_due=Some(std::time::Instant::now()+LOAD_TIMEOUT);}
                if let PlayerAction::Volume(volume)=&action{mixer.set_volume((u32::from((*volume).min(100))*u32::from(u16::MAX)/100)as u16);emit(NativeEvent::Transport{shuffle:None,repeat:None,volume:Some((*volume).min(100))});}
                let before=modes.clone();
                if apply_control(&spirc, action, &mut active, &mut modes, &mut ordered).is_err() {loaded.store(false,Ordering::Release);intent.reset(); emit(NativeEvent::ControlError("Native playback command failed. Reconnect native audio in Settings.".into())); }
                if modes!=before{emit(NativeEvent::Modes(modes.clone()));}}
            }
            event = player_events.recv() => {
                if let Some((kind, uri)) = event.as_ref().and_then(event_summary) {
                    diagnostic(&format!("event {kind} {uri}"));
                }
                match event {
                    Some(event @ PlayerEvent::SetQueue{..})=>{
                        if let PlayerEvent::SetQueue { next_tracks, .. } = &event
                            && let Ok(mut queue) = queue_snapshot.lock()
                        {
                            *queue = Some(next_tracks.iter().take(50).map(|track| QueueEntry { uri: track.uri.clone(), added: track.provider == "queue" }).collect());
                        }
                        if let PlayerEvent::SetQueue { context_uri, .. } = &event {
                            ordered = is_dj_context(context_uri);
                            emit(NativeEvent::DJContext(ordered));
                            emit(NativeEvent::Context(context_uri.clone()));
                        }
                        remember_player_queue(account,event);
                        emit(NativeEvent::QueueChanged);
                    },
                    Some(PlayerEvent::Loading{track_id,..})=>{load_due=None;unavailable.track(&track_id.to_uri());loaded.store(true,Ordering::Release);if !jam_following{intent.track_loading(track_id.to_uri(),std::time::Instant::now());}},
                    Some(PlayerEvent::ShuffleChanged{shuffle})=>{
                        emit(NativeEvent::Transport{shuffle:Some(shuffle),repeat:None,volume:None});
                        // Another Spotify app changed it, or a load applied it.
                        if !ordered&&modes.shuffle!=shuffle{modes.shuffle=shuffle;emit(NativeEvent::Modes(modes.clone()));}
                    },
                    Some(PlayerEvent::RepeatChanged{context,track})=>{
                        let repeat=if track{"track"}else if context{"context"}else{"off"};
                        emit(NativeEvent::Transport{shuffle:None,repeat:Some(repeat.into()),volume:None});
                        if !ordered&&modes.repeat!=repeat{modes.repeat=repeat.into();emit(NativeEvent::Modes(modes.clone()));}
                    },
                    Some(PlayerEvent::VolumeChanged{volume})=>{let value=((u32::from(volume)*100+u32::from(u16::MAX)/2)/u32::from(u16::MAX))as u8;if let Some((wanted,until))=volume_guard&&std::time::Instant::now()<until&&wanted!=value{mixer.set_volume((u32::from(wanted)*u32::from(u16::MAX)/100)as u16);}else{emit(NativeEvent::Transport{shuffle:None,repeat:None,volume:Some(value)});}},
                    Some(PlayerEvent::JamChanged { following, session }) => {diagnostic(&format!("event jam following={following}"));if following&&!jam_following{intent.reset();}jam_following=following;emit(NativeEvent::Jam(following, session));},
                    Some(PlayerEvent::SessionConnected { .. }) => active = true,
                    Some(PlayerEvent::SessionDisconnected { .. }) => {active=false;paused_position=None;loaded.store(false,Ordering::Release);},
                    Some(PlayerEvent::Stopped{..})=>{loaded.store(false,Ordering::Release);audio_playing=false;paused_position=None;intent.reset();emit(NativeEvent::Stopped);},
                    Some(event @ PlayerEvent::TrackChanged { .. }) => {load_due=None;loaded.store(true,Ordering::Release);pending_seek=None;paused_position=None;if let Some(track)=public_track_change(&event) {if !jam_following{intent.track_loading(track.uri.clone(),std::time::Instant::now());} emit(NativeEvent::Track(Box::new(track))); } },
                    Some(PlayerEvent::Playing { track_id, position_ms, .. }) => {
                        let uri = track_id.to_uri();
                        unavailable.playing(&uri);
                        let resumed = paused_position.take().filter(|(track, _)| track == &uri).map(|(_, position)| position);
                        let position_ms = playing_position(position_ms, audio_buffer.latency_ms(), resumed);
                        audio_playing = true;
                        intent.acknowledge_track(&uri, true);
                        emit(NativeEvent::Position { uri, position_ms, playing: Some(true), seeked: false });
                    },
                    Some(PlayerEvent::Paused { track_id, position_ms, .. }) => {
                        let uri = track_id.to_uri();
                        let seeked = pending_seek.take().is_some();
                        let position_ms = u64::from(position_ms).saturating_sub(
                            if seeked { 0 } else { audio_buffer.latency_ms() }
                        );
                        paused_position = Some((uri.clone(), position_ms));
                        audio_playing = false;
                        intent.acknowledge_track(&uri, false);
                        emit(NativeEvent::Position { uri, position_ms, playing: Some(false), seeked });
                    },
                    Some(PlayerEvent::Seeked { track_id, position_ms, .. }) => {let uri=track_id.to_uri();flush_audio.store(true,Ordering::Release);if audio_playing{pending_seek=Some((uri,position_ms.into()));}else{paused_position=Some((uri.clone(),position_ms.into()));emit(NativeEvent::Position { uri, position_ms: position_ms.into(), playing: Some(false), seeked: true });}},
                    Some(PlayerEvent::PositionChanged { track_id, position_ms, .. } | PlayerEvent::PositionCorrection { track_id, position_ms, .. }) => {if pending_seek.is_none(){emit(NativeEvent::Position { uri: track_id.to_uri(), position_ms: u64::from(position_ms).saturating_sub(audio_buffer.latency_ms()), playing: None, seeked: false });}},
                    // Spotify skips unplayable tracks itself and also reports failed
                    // preloads; only a current track that stays stuck is an error.
                    Some(PlayerEvent::Unavailable { track_id, .. }) => unavailable.unavailable(&track_id.to_uri(), std::time::Instant::now()),
                    None => { connection_lost = true; break; },
                    _ => {}
                }
            }
        }
        let now = std::time::Instant::now();
        if load_due.is_some_and(|due| now >= due) {
            load_due = None;
            diagnostic("load never started");
            loaded.store(false, Ordering::Release);
            intent.reset();
            emit(NativeEvent::ControlError(
                "Spotify didn't start that. Press Play to try again.".into(),
            ));
        }
        let audible_narration = audio_buffer.narrating();
        if audible_narration != narrating {
            narrating = audible_narration;
            emit(NativeEvent::Narration(narrating));
        }
        let value = latest_volume.swap(u32::MAX, Ordering::AcqRel);
        if value <= 100 {
            let volume = value as u8;
            mixer.set_volume((value * u32::from(u16::MAX) / 100) as u16);
            volume_sync = Some((volume, now));
            volume_guard = Some((volume, now + Duration::from_secs(3)));
            emit(NativeEvent::Transport {
                shuffle: None,
                repeat: None,
                volume: Some(volume),
            });
        }
        if let Some((volume, changed)) = volume_sync
            && now.duration_since(changed) >= Duration::from_millis(180)
        {
            volume_sync = None;
            let _ = spirc.set_volume((u32::from(volume) * u32::from(u16::MAX) / 100) as u16);
        }
        if let Some(issue) = audio_buffer.output.take_issue() {
            match issue {
                crate::audio_output::OutputIssue::UsingDefault => emit(NativeEvent::ControlError(
                    "Selected audio output is unavailable. Using the system default.".into(),
                )),
                crate::audio_output::OutputIssue::Unavailable => {
                    intent.reset();
                    local_player.pause();
                    emit(NativeEvent::ControlError("No audio output is available. Connect speakers or headphones, then press Play.".into()));
                }
            }
        }
        if unavailable.due(now) {
            loaded.store(false, Ordering::Release);
            intent.reset();
            emit(NativeEvent::ControlError(
                "This track is unavailable for your account. Choose another song.".into(),
            ));
        }
        if let Some(playing) = intent.next(audio_playing, std::time::Instant::now()) {
            // Local transport must not queue behind Spotify Connect HTTP notifications.
            if playing {
                local_player.play();
            } else {
                local_player.pause();
            }
        }
    }
    session.shutdown();
    if connection_lost {
        Outcome::Failed(Failure::transient("Native audio disconnected"))
    } else {
        Outcome::Stopped
    }
}

impl Drop for NativePlayer {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.ready.store(false, Ordering::Release);
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

/// Whether librespot would play `item` for `user`, by the rules of its own
/// availability check: released, offered in the listener's country for
/// their catalogue, with audio files; otherwise an alternative recording.
/// Without the listener's details, country restrictions are not checked.
pub(crate) fn playable(
    item: &librespot_metadata::Track,
    user: Option<&librespot_core::session::UserData>,
) -> bool {
    use librespot_core::date::Date;
    let now = Date::now_utc();
    let released = now >= item.earliest_live_timestamp
        && (item.availability.is_empty()
            || item.availability.iter().any(|window| now >= window.start));
    let offered = user.is_none_or(|user| {
        let catalogue = user
            .attributes
            .get("catalogue")
            .map_or("premium", String::as_str);
        // The first restriction for this catalogue decides, as in librespot.
        item.restrictions
            .iter()
            .find(|restriction| {
                restriction
                    .catalogue_strs
                    .iter()
                    .any(|name| name == catalogue)
                    && (restriction.countries_allowed.is_some()
                        || restriction.countries_forbidden.is_some())
            })
            .is_none_or(|restriction| match &restriction.countries_allowed {
                Some(allowed) => allowed.contains(&user.country),
                None => !restriction
                    .countries_forbidden
                    .as_ref()
                    .is_some_and(|forbidden| forbidden.contains(&user.country)),
            })
    });
    (released && offered && !item.files.is_empty()) || !item.alternatives.is_empty()
}

pub(crate) fn metadata_track(
    item: &librespot_metadata::Track,
    user: Option<&librespot_core::session::UserData>,
) -> Option<crate::model::Track> {
    use crate::model::{Album, Artist, Artwork, Track};
    let artists = |items: &librespot_metadata::artist::Artists| {
        items
            .iter()
            .map(|artist| {
                let uri = artist.id.to_uri();
                Artist {
                    id: uri.rsplit(':').next().map(str::to_owned),
                    uri,
                    name: artist.name.clone(),
                    ..Artist::default()
                }
            })
            .collect()
    };
    let uri = item.id.to_uri();
    let album_uri = item.album.id.to_uri();
    let mut images: Vec<_> = item
        .album
        .cover_group
        .iter()
        .chain(item.album.covers.iter())
        .map(|image| Artwork {
            url: format!("https://i.scdn.co/image/{}", image.id),
            width: u32::try_from(image.width).ok(),
        })
        .collect();
    images.sort_by_key(|image| std::cmp::Reverse(image.width));
    images.dedup_by(|a, b| a.url == b.url);
    Some(Track {
        unavailable: !playable(item, user),
        explicit: item.is_explicit,
        id: uri.rsplit(':').next().map(str::to_owned),
        uri,
        name: item.name.clone(),
        duration_ms: item.duration.max(0) as u64,
        artists: artists(&item.artists),
        songwriters: item
            .artists_with_role
            .iter()
            .filter(|artist| {
                artist.role == librespot_metadata::artist::ArtistRole::ARTIST_ROLE_COMPOSER
            })
            .map(|artist| artist.name.clone())
            .collect(),
        album: Album {
            id: album_uri.rsplit(':').next().map(str::to_owned),
            uri: album_uri,
            name: item.album.name.clone(),
            artists: artists(&item.album.artists),
            images,
            release_date: item.album.date.0.date().to_string(),
            album_type: item.album.type_str.clone(),
            ..Album::default()
        },
        ..Track::default()
    })
}

/// The account of the saved native sign-in, for ignored live tests.
#[cfg(test)]
pub(crate) fn live_account() -> String {
    saved_profile()
        .expect("Cannot read saved native sign-in")
        .expect("Saved native sign-in required")
        .id
}

/// Runs `check` on a fresh native session from the saved sign-in. For
/// ignored live tests only; never starts playback.
#[cfg(test)]
pub(crate) fn with_live_session<T>(check: impl AsyncFnOnce(&Session) -> T) -> T {
    // Same restore path as the app: the saved native sign-in in the OS
    // credential store, without the old developer tokens.
    let client = crate::auth::http_client().expect("HTTP client unavailable");
    let login = crate::native_login::authenticate(&client, true, &AtomicBool::new(false))
        .expect("Native sign-in failed")
        .expect("Saved native sign-in required");
    let session = login.connection.0.clone();
    login.runtime.block_on(check(&session))
}

#[cfg(test)]
mod tests {
    #[test]
    fn connect_name_puts_the_computer_first() {
        assert_eq!(super::named_device("ARASHII-PC"), "ARASHII-PC (Lyssna)");
        assert_eq!(super::named_device("  "), "Lyssna");
    }
    #[test]
    #[ignore = "Saved sign-in; register an idle Connect device without starting playback"]
    fn live_native_ready_requires_connect_registration() {
        use super::*;
        let account = live_account();
        let controls = NativeControls::default();
        let (events, receive) = mpsc::channel();
        let player = NativePlayer::start(account, 1, events, Default::default(), controls.clone());
        let deadline = std::time::Instant::now() + Duration::from_secs(45);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "Native readiness timed out"
            );
            if let Ok(NativeMessage {
                event: NativeEvent::Ready(id),
                ..
            }) = receive.recv_timeout(Duration::from_millis(20))
            {
                assert!(
                    controls.connect_cluster().unwrap().is_some(),
                    "Native Ready arrived before Spotify registered the Connect sender"
                );
                assert!(
                    controls
                        .devices()
                        .unwrap()
                        .iter()
                        .any(|device| device.id.as_deref() == Some(&id)),
                    "Ready sender missing from Spotify device list"
                );
                break;
            }
        }
        drop(player);
    }
    #[test]
    fn playing_position_matches_audible_audio() {
        assert_eq!(super::playing_position(10_000, 400, None), 9_600);
        assert_eq!(super::playing_position(200, 400, None), 0);
        assert_eq!(super::playing_position(10_000, 400, Some(8_000)), 8_000);
    }
    use super::*;
    use std::cell::Cell;
    /// Loads with shuffle and repeat off.
    fn load_request(action: &PlayerAction) -> Option<LoadRequest> {
        super::load_request(action, &PlayModes::new(false, "off"))
    }
    #[test]
    fn normal_loads_use_the_chosen_modes_but_dj_keeps_narration_order() {
        let modes = PlayModes::new(true, "track");
        for action in [
            PlayerAction::PlayUris {
                uris: vec!["spotify:track:song".into()],
                offset: 0,
                context: None,
            },
            PlayerAction::PlayContext {
                context: "spotify:playlist:normal".into(),
                offset: None,
            },
        ] {
            assert!(
                matches!(super::load_request(&action, &modes).unwrap().context_options,
                Some(librespot_connect::LoadContextOptions::Options(ref options))
                    if options.shuffle && options.repeat_track && !options.repeat)
            );
        }
        for action in [
            PlayerAction::PlayContext {
                context: crate::home::DJ_URI.into(),
                offset: None,
            },
            PlayerAction::RestoreDJ {
                context: crate::home::DJ_URI.into(),
                session_url: None,
                track: "spotify:track:song".into(),
                track_uid: None,
                position_ms: 0,
                relative_offset: 0,
            },
        ] {
            assert!(
                matches!(super::load_request(&action, &modes).unwrap().context_options,
                Some(librespot_connect::LoadContextOptions::Options(ref options))
                    if !options.shuffle && !options.repeat_track)
            );
        }
    }
    #[test]
    fn saved_queue_load_starts_at_saved_position_without_a_followup_seek() {
        let queue: Vec<String> = vec!["spotify:track:first".into(), "spotify:track:second".into()];
        let action = crate::resume::restore_action(&queue[1], &queue, 77000);
        let request = load_request(&action).unwrap();
        assert!(request.start_playing);
        assert_eq!(request.seek_to, 77000);
        assert!(matches!(
            request.playing_track,
            Some(PlayingTrack::Index(1))
        ));
        let request = load_request(&crate::resume::restore_action(
            "spotify:track:other",
            &queue,
            u64::MAX,
        ))
        .unwrap();
        assert_eq!(request.seek_to, u32::MAX);
        assert!(matches!(
            request.playing_track,
            Some(PlayingTrack::Index(0))
        ));
        assert!(load_request(&PlayerAction::Pause).is_none());
        // With its playlist saved, the song loads from the playlist itself.
        let request = load_request(&PlayerAction::Resume {
            uris: queue.clone(),
            offset: 1,
            position_ms: 77000,
            context: Some("spotify:playlist:0123456789abcdefghijkl".into()),
            queued: Vec::new(),
            listed: false,
        })
        .unwrap();
        assert_eq!(request.seek_to, 77000);
        assert!(matches!(
            &request.playing_track,
            Some(PlayingTrack::Uri(uri)) if *uri == queue[1]
        ));
        // A list played in its own order resumes in that order.
        let request = load_request(&PlayerAction::Resume {
            uris: queue.clone(),
            offset: 1,
            position_ms: 77000,
            context: Some("spotify:playlist:0123456789abcdefghijkl".into()),
            queued: Vec::new(),
            listed: true,
        })
        .unwrap();
        assert_eq!(request.seek_to, 77000);
        assert!(matches!(
            request.playing_track,
            Some(PlayingTrack::Index(1))
        ));
        let tracks: Vec<_> = request.prepared_context.as_ref().unwrap().pages[0]
            .tracks
            .iter()
            .filter_map(|track| track.uri.clone())
            .collect();
        assert_eq!(tracks, queue);
    }
    #[test]
    fn listed_songs_load_under_their_context_without_resolving_it() {
        let uris: Vec<String> = vec!["spotify:track:a".into(), "spotify:track:b".into()];
        let request = load_request(&PlayerAction::PlayUris {
            uris: uris.clone(),
            offset: 1,
            context: Some("spotify:playlist:abc".into()),
        })
        .unwrap();
        assert!(request.start_playing);
        assert!(matches!(
            request.playing_track,
            Some(PlayingTrack::Index(1))
        ));
        let context = request.prepared_context.as_deref().unwrap();
        assert_eq!(context.uri.as_deref(), Some("spotify:playlist:abc"));
        assert_eq!(
            context.pages[0]
                .tracks
                .iter()
                .map(|track| track.uri.clone().unwrap())
                .collect::<Vec<_>>(),
            uris
        );
        let plain = load_request(&PlayerAction::PlayUris {
            uris,
            offset: 0,
            context: None,
        })
        .unwrap();
        assert!(plain.prepared_context.is_none());
    }
    #[test]
    fn resume_preparation_cannot_use_another_tracks_narration_from_a_stale_uid() {
        use librespot_protocol::{
            context::Context, context_page::ContextPage, context_track::ContextTrack,
        };
        let mut saved = crate::resume::LastPlayed {
            track: crate::model::Track {
                uri: "spotify:track:saved".into(),
                ..Default::default()
            },
            position_ms: 77000,
            played_at: 1,
            queue: vec![],
            dj_context: Some(crate::resume::DJContext {
                uri: crate::home::DJ_URI.into(),
                session_url: None,
                track_uid: Some("old-uid".into()),
            }),
            context: None,
            queued: Vec::new(),
        };
        let context = Context {
            pages: vec![ContextPage {
                tracks: vec![
                    ContextTrack {
                        uri: Some("spotify:track:other".into()),
                        uid: Some("old-uid".into()),
                        ..Default::default()
                    },
                    ContextTrack {
                        uri: Some(saved.track.uri.clone()),
                        uid: Some("new-uid".into()),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            saved_context_track(&saved, &context)
                .unwrap()
                .uid
                .as_deref(),
            Some("new-uid")
        );
        saved.track.uri = "spotify:track:missing".into();
        assert!(saved_context_track(&saved, &context).is_none());
    }
    #[test]
    fn mono_dj_mp3_decodes_into_both_output_channels() {
        use librespot_playback::decoder::{AudioDecoder, AudioPacket, SymphoniaDecoder};
        let mut decoder = SymphoniaDecoder::new(
            std::io::Cursor::new(&include_bytes!("../tests/fixtures/dj-mono.mp3")[..]),
            Default::default(),
        )
        .unwrap();
        let mut frames = 0;
        let mut audible = false;
        while let Some((_, AudioPacket::Samples(samples))) = decoder.next_packet().unwrap() {
            let (stereo, remainder) = samples.as_chunks::<2>();
            assert!(remainder.is_empty());
            for frame in stereo {
                assert_eq!(frame[0], frame[1]);
                audible |= frame[0].abs() > 0.0001;
                frames += 1;
            }
        }
        assert!(frames > 0);
        assert!(audible);
    }
    #[test]
    fn album_resolution_reads_track_metadata_through_the_native_channel() {
        use librespot_protocol::{
            extension_kind::ExtensionKind,
            metadata::{Album as WireAlbum, Track as WireTrack},
        };
        use protobuf::Message;
        let wire = WireTrack {
            gid: Some(vec![1; 16]),
            name: Some("Song".into()),
            duration: Some(180000),
            album: protobuf::MessageField::some(WireAlbum {
                gid: Some(vec![2; 16]),
                name: Some("Album".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let uri = librespot_core::SpotifyId::from_raw(&[1; 16])
            .unwrap()
            .to_base62();
        let controls = NativeControls::default();
        let (sender, _commands) = async_channel::unbounded_channel();
        let (metadata, mut requests) = async_channel::unbounded_channel::<MetadataRead>();
        *controls.0.lock().unwrap() = Some(ControlConnection {
            devices: Default::default(),
            audio_buffer: Default::default(),
            dj_requests: Default::default(),
            account: "listener".into(),
            status: Arc::default(),
            session: Arc::default(),
            volume: Arc::new(AtomicU32::new(u32::MAX)),
            sender,
            ready: Arc::new(AtomicBool::new(true)),
            loaded: Arc::new(AtomicBool::new(false)),
            playlists: None,
            metadata: Some(metadata),
            queue_snapshot: Default::default(),
        });
        let service = thread::spawn(move || {
            if let Some(request) = requests.blocking_recv() {
                assert_eq!(request.kind, ExtensionKind::TRACK_V4);
                request
                    .reply
                    .send(Ok(wire.write_to_bytes().unwrap()))
                    .unwrap();
            }
        });
        let result = controls.track(&format!("spotify:track:{uri}"));
        drop(controls);
        service.join().unwrap();
        let track = result.expect("Album resolution must reach native track metadata");
        assert_eq!(track.album.name, "Album");
        assert!(track.album.id.is_some());
    }
    #[test]
    fn extended_track_metadata_keeps_album_links() {
        use librespot_protocol::metadata::{Album as WireAlbum, Date, Track as WireTrack};
        let wire = WireTrack {
            gid: Some(vec![1; 16]),
            name: Some("Song".into()),
            duration: Some(180000),
            album: protobuf::MessageField::some(WireAlbum {
                gid: Some(vec![2; 16]),
                name: Some("Album".into()),
                date: protobuf::MessageField::some(Date {
                    year: Some(2025),
                    month: Some(1),
                    day: Some(1),
                    ..Date::default()
                }),
                ..WireAlbum::default()
            }),
            ..WireTrack::default()
        };
        let track =
            metadata_track(&librespot_metadata::Track::try_from(&wire).unwrap(), None).unwrap();
        assert!(track.album.id.is_some());
        assert!(track.album.uri.starts_with("spotify:album:"));
        assert_eq!(track.album.name, "Album");
    }

    #[test]
    fn direct_selection_preloads_the_requested_position_without_starting_dj() {
        let uri = "spotify:track:0000000000000000000001".to_owned();
        let action = PlayerAction::Resume {
            uris: vec![uri.clone()],
            offset: 0,
            position_ms: 77000,
            context: None,
            queued: Vec::new(),
            listed: false,
        };
        let (actual, position) = preload_selection(&action).unwrap();
        assert_eq!(actual.to_uri(), uri);
        assert_eq!(position, 77000);
        assert!(
            preload_selection(&PlayerAction::PlayContext {
                context: crate::home::DJ_URI.into(),
                offset: None
            })
            .is_none()
        );
        assert!(preload_selection(&PlayerAction::Next).is_none());
    }

    #[test]
    #[ignore = "Uses saved native sign-in for read-only playlist search; never starts playback"]
    fn live_playlist_search_without_playback() {
        let account = live_account();
        let credentials = load_credentials(&account)
            .expect("Cannot read saved native sign-in")
            .expect("Saved native sign-in required");
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let session = Session::new(
                    SessionConfig {
                        client_id: crate::auth::AUDIO_CLIENT_ID.into(),
                        ..SessionConfig::default()
                    },
                    None,
                );
                tokio::time::timeout(Duration::from_secs(30), session.connect(credentials, false))
                    .await
                    .expect("Native connection timed out")
                    .expect("Native connection failed");
                let results = crate::search::playlists(&session, "workout").await;
                let catalogue = crate::search::catalogue(&session, "arianna").await;
                session.shutdown();
                let results = results.expect("Native playlist search failed");
                assert!(!results.is_empty(), "Expected workout playlists");
                assert!(
                    results
                        .iter()
                        .any(|playlist| playlist.owner.id == "spotify"),
                    "Expected Spotify-curated playlists"
                );
                assert!(
                    results
                        .iter()
                        .any(|playlist| !playlist.owner.id.is_empty()
                            && playlist.owner.id != "spotify"),
                    "Expected public user playlists"
                );
                eprintln!(
                    "Native search returned {} playlists, including Spotify and public creators",
                    results.len()
                );
                let catalogue = catalogue.expect("Native music search failed");
                assert!(!catalogue.tracks.is_empty(), "Expected song results");
                assert!(!catalogue.artists.is_empty(), "Expected artist results");
                assert!(!catalogue.albums.is_empty(), "Expected album results");
                assert!(!catalogue.playlists.is_empty(), "Expected playlist results");
                assert!(
                    !catalogue.top_results.is_empty(),
                    "Expected Spotify result ranking"
                );
                assert!(
                    catalogue.tracks.iter().all(|track| track.duration_ms > 0),
                    "Expected track durations"
                );
                eprintln!(
                    "Music search returned {} songs, {} artists, {} albums and {} playlists",
                    catalogue.tracks.len(),
                    catalogue.artists.len(),
                    catalogue.albums.len(),
                    catalogue.playlists.len()
                );
            });
    }

    #[test]
    #[ignore = "Measures saved native sign-in to playback readiness; does not play audio"]
    fn live_login_to_playback_readiness() {
        use std::time::Instant;
        let client = crate::auth::http_client().unwrap();
        let mut after_login = Vec::new();
        for sample in 0..3 {
            let started = Instant::now();
            let login = crate::native_login::authenticate(&client, true, &AtomicBool::new(false))
                .unwrap()
                .expect("Saved native login required");
            let authenticated_device = login.connection.0.device_id().to_owned();
            let signed_in = Instant::now();
            let controls = NativeControls::default();
            let (events, receive) = mpsc::channel();
            let player =
                NativePlayer::start_authenticated(login, 1, events, Default::default(), controls);
            let deadline = Instant::now() + Duration::from_secs(45);
            loop {
                assert!(Instant::now() < deadline, "Native readiness timed out");
                match receive.recv_timeout(Duration::from_millis(20)) {
                    Ok(NativeMessage {
                        event: NativeEvent::Ready(device),
                        ..
                    }) => {
                        assert_eq!(
                            device, authenticated_device,
                            "Playback must reuse the verified sign-in session"
                        );
                        break;
                    }
                    Ok(NativeMessage {
                        event: NativeEvent::Error(_) | NativeEvent::Authorizing,
                        ..
                    }) => panic!("Saved login did not become ready"),
                    _ => {}
                }
            }
            let delay = signed_in.elapsed().as_millis();
            println!(
                "sample {}: login={} ms, after-login readiness={} ms, total={} ms",
                sample + 1,
                signed_in.duration_since(started).as_millis(),
                delay,
                started.elapsed().as_millis()
            );
            after_login.push(delay);
            drop(player);
        }
        after_login.sort_unstable();
        println!("median after-login readiness={} ms", after_login[1]);
        assert!(
            after_login[1] < 500,
            "Post-login playback setup must reuse authenticated session"
        );
    }

    #[test]
    #[ignore = "Measures sign-in to first decoded audio using the saved song; playback is muted"]
    fn live_login_to_first_audio() {
        use std::time::Instant;
        let began = Instant::now();
        let login = crate::native_login::authenticate(
            &crate::auth::http_client().unwrap(),
            true,
            &AtomicBool::new(false),
        )
        .unwrap()
        .expect("Saved native login required");
        let saved = crate::resume::load(&login.profile.id).expect("Saved song required");
        let signed_in = Instant::now();
        let controls = NativeControls::default();
        let (events, receive) = mpsc::channel();
        let player = NativePlayer::start_authenticated(
            login,
            1,
            events,
            Default::default(),
            controls.clone(),
        );
        let buffer = controls
            .0
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .audio_buffer
            .clone();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            assert!(Instant::now() < deadline, "Native readiness timed out");
            match receive.recv_timeout(Duration::from_millis(10)) {
                Ok(NativeMessage {
                    event: NativeEvent::Ready(_),
                    ..
                }) => break,
                Ok(NativeMessage {
                    event: NativeEvent::Error(_) | NativeEvent::Authorizing,
                    ..
                }) => panic!("Saved login failed"),
                _ => {}
            }
        }
        assert!(controls.send(PlayerAction::Volume(0)).is_ok());
        let requested = Instant::now();
        assert!(
            controls
                .send(PlayerAction::Play(Some(saved.track.uri)))
                .is_ok()
        );
        let first_output = loop {
            if let Some(started) = buffer.output_started() {
                break started;
            }
            assert!(Instant::now() < deadline, "First audio timed out");
            if let Ok(NativeMessage {
                event: NativeEvent::Error(_) | NativeEvent::ControlError(_),
                ..
            }) = receive.recv_timeout(Duration::from_millis(10))
            {
                panic!("First song failed");
            }
        };
        assert!(controls.send(PlayerAction::Pause).is_ok());
        println!(
            "muted first audio: login={} ms, after-login={} ms, selection={} ms, total={} ms",
            signed_in.duration_since(began).as_millis(),
            first_output.duration_since(signed_in).as_millis(),
            first_output.duration_since(requested).as_millis(),
            first_output.duration_since(began).as_millis()
        );
        drop(player);
    }

    #[test]
    #[ignore = "Measures live Spotify starts using saved sign-in and the default audio output"]
    fn live_track_start_latency() {
        measure_live_track_starts(false, 500);
    }

    #[test]
    #[ignore = "Measures live Spotify skips using saved sign-in and the default audio output"]
    fn live_skip_start_latency() {
        measure_live_track_starts(true, 150);
    }

    fn measure_live_track_starts(use_skips: bool, median_budget_ms: u128) {
        use std::time::Instant;
        let account = live_account();
        let saved = crate::resume::load(&account).expect("Saved listening queue required");
        let mut uris = Vec::new();
        for uri in saved.queue.iter().chain(std::iter::once(&saved.track.uri)) {
            if uri.starts_with("spotify:track:") && !uris.contains(uri) {
                uris.push(uri.clone());
            }
            if uris.len() == 3 {
                break;
            }
        }
        assert!(uris.len() >= 2, "Choose a playlist before measuring starts");
        let controls = NativeControls::default();
        let (events, receive) = mpsc::channel();
        let player = NativePlayer::start(account, 1, events, Default::default(), controls.clone());
        let deadline = Instant::now() + Duration::from_secs(50);
        loop {
            assert!(Instant::now() < deadline, "Native connection timed out");
            match receive.recv_timeout(Duration::from_millis(100)) {
                Ok(NativeMessage {
                    event: NativeEvent::Ready(_),
                    ..
                }) => break,
                Ok(NativeMessage {
                    event: NativeEvent::Error(_),
                    ..
                }) => panic!("Native connection failed"),
                _ => {}
            }
        }
        let buffer = controls
            .0
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .audio_buffer
            .clone();
        let mut timings = Vec::new();
        for index in 0..6 {
            while receive.try_recv().is_ok() {}
            let requested = Instant::now();
            let action = if use_skips && index % uris.len() != 0 {
                PlayerAction::Next
            } else {
                PlayerAction::PlayUris {
                    uris: uris.clone(),
                    offset: index % uris.len(),
                    context: None,
                }
            };
            assert!(controls.send(action).is_ok());
            loop {
                if let Some(started) = buffer.output_started() {
                    let elapsed = started.saturating_duration_since(requested).as_millis();
                    println!("selection {} first output: {} ms", index + 1, elapsed);
                    if index > 0 && (!use_skips || index % uris.len() != 0) {
                        timings.push(elapsed);
                    }
                    break;
                }
                assert!(
                    requested.elapsed() < Duration::from_secs(15),
                    "Track start timed out"
                );
                if let Ok(NativeMessage {
                    event: NativeEvent::Error(_) | NativeEvent::ControlError(_),
                    ..
                }) = receive.recv_timeout(Duration::from_millis(10))
                {
                    panic!("Native playback failed");
                }
            }
            thread::sleep(Duration::from_millis(400));
        }
        let _ = controls.send(PlayerAction::Pause);
        drop(player);
        timings.sort_unstable();
        let median = timings[timings.len() / 2];
        println!("warm median first output: {} ms", median);
        assert!(
            median < median_budget_ms,
            "Warm track starts exceed the latency target"
        );
    }

    #[test]
    fn track_selection_and_skips_stop_old_audio_before_connect_responds() {
        use librespot_playback::{audio_backend::Sink, convert::Converter, decoder::AudioPacket};

        for action in [
            PlayerAction::Play(Some("spotify:track:0123456789abcdefghijkl".into())),
            PlayerAction::Next,
            PlayerAction::Previous,
        ] {
            let buffer = crate::audio::AudioBuffer::default();
            let (seek_ready, _) = async_channel::unbounded_channel();
            let mut audio = crate::audio::AudioSink::new(
                Arc::new(AtomicBool::new(false)),
                seek_ready,
                buffer.clone(),
            );
            let mut output = audio.test_output();
            audio
                .write(
                    AudioPacket::Samples(vec![0.25; 22_050]),
                    &mut Converter::new(None),
                )
                .unwrap();
            assert_eq!(output.next(), Some(0.25));
            let controls = NativeControls::default();
            let (sender, _receiver) = async_channel::unbounded_channel();
            *controls.0.lock().unwrap() = Some(ControlConnection {
                devices: Default::default(),
                audio_buffer: buffer.clone(),
                dj_requests: Default::default(),
                account: "listener".into(),
                status: Arc::new(Mutex::new(Some(NativeEvent::Ready("device".into())))),
                session: Arc::default(),
                volume: Arc::new(AtomicU32::new(u32::MAX)),
                sender,
                ready: Arc::new(AtomicBool::new(true)),
                loaded: Arc::new(AtomicBool::new(true)),
                playlists: None,
                metadata: None,
                queue_snapshot: Default::default(),
            });
            assert!(controls.send(action).is_ok());
            assert!(
                (0..1_764).all(|_| output.next() != Some(0.25)),
                "Previous song remains audible after the UI selects another track"
            );
        }
    }

    #[test]
    fn transient_connection_failures_retry_but_authentication_failures_do_not() {
        for kind in [
            ErrorKind::Unavailable,
            ErrorKind::DeadlineExceeded,
            ErrorKind::Aborted,
        ] {
            assert!(
                Failure::from_connection(librespot_core::Error::new(kind, "test failure"))
                    .retryable
            );
        }
        for kind in [
            ErrorKind::Unauthenticated,
            ErrorKind::PermissionDenied,
            ErrorKind::InvalidArgument,
        ] {
            assert!(
                !Failure::from_connection(librespot_core::Error::new(kind, "test failure"))
                    .retryable
            );
        }
    }

    #[test]
    #[ignore = "Uses saved sign-in and briefly plays and pauses the selected song"]
    fn live_selected_song_resumes_without_reconnecting() {
        let account = live_account();
        let saved = crate::resume::load(&account).expect("Saved selection required");
        let controls = NativeControls::default();
        let (events, receive) = mpsc::channel();
        let player = NativePlayer::start(account, 1, events, Default::default(), controls.clone());
        let wait = |wanted: Option<bool>| {
            let started = std::time::Instant::now();
            loop {
                assert!(
                    started.elapsed() < Duration::from_secs(40),
                    "Transport timed out"
                );
                match receive.recv_timeout(Duration::from_millis(100)) {
                    Ok(NativeMessage {
                        event: NativeEvent::Ready(_),
                        ..
                    }) if wanted.is_none() => break,
                    Ok(NativeMessage {
                        event:
                            NativeEvent::Position {
                                playing: Some(playing),
                                ..
                            },
                        ..
                    }) if Some(playing) == wanted => break,
                    Ok(NativeMessage {
                        event:
                            NativeEvent::Reconnecting(_)
                            | NativeEvent::Authorizing
                            | NativeEvent::Error(_),
                        ..
                    }) => panic!("Selected song unexpectedly reconnected"),
                    _ => {}
                }
            }
            started.elapsed()
        };
        wait(None);
        let mut uris = vec![saved.track.uri];
        for uri in saved.queue {
            if uri.starts_with("spotify:track:") && !uris.contains(&uri) {
                uris.push(uri);
            }
            if uris.len() == 3 {
                break;
            }
        }
        let expected_queue = uris[1..].to_vec();
        assert!(
            controls
                .send(PlayerAction::PlayUris {
                    uris,
                    offset: 0,
                    context: None,
                })
                .is_ok()
        );
        wait(Some(true));
        let queue = controls.queue().expect("Native queue unavailable");
        assert!(
            queue
                .iter()
                .map(|track| track.uri.clone())
                .collect::<Vec<_>>()
                == expected_queue,
            "Native queue differs from the requested upcoming tracks"
        );
        assert!(
            queue.iter().all(|track| !track.is_local),
            "Native queue metadata failed to resolve"
        );
        println!("native queue: {} tracks in the expected order", queue.len());
        assert!(controls.send(PlayerAction::Pause).is_ok());
        wait(Some(false));
        assert!(controls.send(PlayerAction::Play(None)).is_ok());
        let elapsed = wait(Some(true));
        println!(
            "selected-song resume acknowledgment: {} ms",
            elapsed.as_millis()
        );
        assert!(elapsed < Duration::from_millis(250));
        let _ = controls.send(PlayerAction::Pause);
        drop(player);
    }

    #[test]
    #[ignore = "Uses saved native sign-in and briefly plays the selected song muted to verify native queue writes"]
    fn live_native_queue_write_and_device_discovery() {
        use std::time::Instant;
        let settings = crate::config::Settings::load().unwrap();
        let account = live_account();
        assert!(
            load_credentials(&account).unwrap().is_some(),
            "Saved native sign-in required"
        );
        let uri = crate::resume::load(&account)
            .expect("Saved selection required")
            .track
            .uri;
        let controls = NativeControls::default();
        let (events, receive) = mpsc::channel();
        let player = NativePlayer::start(account, 1, events, Default::default(), controls.clone());
        let deadline = Instant::now() + Duration::from_secs(80);
        loop {
            assert!(
                Instant::now() < deadline,
                "Native device discovery timed out"
            );
            if let Ok(devices) = controls.devices()
                && devices.iter().any(|device| device.name == device_name())
            {
                println!(
                    "Native discovery: {} available playback destinations",
                    devices.len()
                );
                break;
            }
            if let Ok(NativeMessage {
                event: NativeEvent::Authorizing | NativeEvent::Error(_),
                ..
            }) = receive.recv_timeout(Duration::from_millis(100))
            {
                panic!("Native discovery unexpectedly required fresh sign-in");
            }
        }
        assert!(controls.send(PlayerAction::Volume(0)).is_ok());
        assert!(controls.send(PlayerAction::Play(Some(uri.clone()))).is_ok());
        loop {
            assert!(
                Instant::now() < deadline,
                "Native playback did not become active"
            );
            if let Ok(NativeMessage {
                event:
                    NativeEvent::Position {
                        playing: Some(true),
                        ..
                    },
                ..
            }) = receive.recv_timeout(Duration::from_millis(100))
            {
                break;
            }
        }
        assert!(controls.send(PlayerAction::Enqueue(uri.clone())).is_ok());
        loop {
            assert!(
                Instant::now() < deadline,
                "Native enqueue did not update the queue"
            );
            if let Ok(NativeMessage {
                event: NativeEvent::QueueChanged,
                ..
            }) = receive.recv_timeout(Duration::from_millis(100))
            {
                let queued = controls
                    .0
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .queue_snapshot
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_default();
                if queued.iter().any(|entry| entry.uri == uri) {
                    assert!(
                        controls
                            .queue()
                            .unwrap()
                            .iter()
                            .any(|track| track.uri == uri)
                    );
                    println!(
                        "Native enqueue acknowledged; queue metadata resolved without Web API"
                    );
                    break;
                }
            }
        }
        if let Ok(name) = std::env::var("LYSSNA_TEST_CONNECT_TARGET") {
            let devices = controls.devices().unwrap();
            let matching: Vec<_> = devices
                .iter()
                .filter(|device| device.name.to_lowercase().contains(&name.to_lowercase()))
                .collect();
            assert_eq!(matching.len(), 1, "Choose one unambiguous speaker name");
            let target = matching[0].id.as_ref().unwrap().clone();
            controls.transfer(&target).unwrap();
            println!("Remote transfer request accepted; confirm speaker audio separately");
            let hold = std::env::var("LYSSNA_TEST_CONNECT_HOLD_SECONDS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0)
                .min(45);
            thread::sleep(Duration::from_secs(hold));
            while receive.try_recv().is_ok() {}
            controls.transfer("native").unwrap();
            assert!(
                controls
                    .send(PlayerAction::Volume(settings.volume_percent))
                    .is_ok()
            );
            let until = std::time::Instant::now() + Duration::from_secs(20);
            loop {
                assert!(
                    std::time::Instant::now() < until,
                    "Local playback did not resume after returning"
                );
                if let Ok(NativeMessage {
                    event:
                        NativeEvent::Position {
                            playing: Some(true),
                            ..
                        },
                    ..
                }) = receive.recv_timeout(Duration::from_millis(100))
                {
                    break;
                }
            }
            println!("Local playback reported after native return transfer");
            thread::sleep(Duration::from_secs(5));
            let _ = controls.send(PlayerAction::Pause);
        }
        drop(player);
    }

    #[test]
    #[ignore = "Uses saved sign-in and deliberately disconnects one native session"]
    fn live_reconnect_recovers_without_another_browser_login() {
        let account = live_account();
        assert!(
            load_credentials(&account).ok().flatten().is_some(),
            "Saved audio sign-in required"
        );
        let controls = NativeControls::default();
        let (events, receive) = mpsc::channel();
        let player = NativePlayer::start(
            account.clone(),
            1,
            events,
            Default::default(),
            controls.clone(),
        );
        let started = std::time::Instant::now();
        let mut disconnected = false;
        let mut retry_seen = false;
        loop {
            assert!(
                started.elapsed() < Duration::from_secs(50),
                "Recovery timed out"
            );
            match receive.recv_timeout(Duration::from_millis(100)) {
                Ok(NativeMessage {
                    event: NativeEvent::Ready(_),
                    ..
                }) if !disconnected => {
                    controls.session().unwrap().shutdown();
                    disconnected = true;
                }
                Ok(NativeMessage {
                    event: NativeEvent::Ready(_),
                    ..
                }) => {
                    assert!(
                        retry_seen,
                        "A new connection must follow the recovery state"
                    );
                    assert!(controls.connected_or_connecting(&account));
                    break;
                }
                Ok(NativeMessage {
                    event: NativeEvent::Reconnecting(_),
                    ..
                }) => retry_seen = true,
                Ok(NativeMessage {
                    event: NativeEvent::Authorizing,
                    ..
                }) => panic!("Recovery opened browser authorization"),
                Ok(NativeMessage {
                    event: NativeEvent::Error(_),
                    ..
                }) => panic!("Recovery failed"),
                _ => {}
            }
        }
        drop(player);
    }

    #[test]
    fn transport_commands_do_not_claim_a_track_is_loaded() {
        let controls = NativeControls::default();
        let (sender, mut receiver) = async_channel::unbounded_channel();
        let ready = Arc::new(AtomicBool::new(true));
        let loaded = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(Some(NativeEvent::Ready("device".into()))));
        *controls.0.lock().unwrap() = Some(ControlConnection {
            devices: Default::default(),
            audio_buffer: Default::default(),
            dj_requests: Default::default(),
            account: "listener".into(),
            status: status.clone(),
            session: Arc::default(),
            volume: Arc::new(AtomicU32::new(u32::MAX)),
            sender,
            ready: ready.clone(),
            loaded: loaded.clone(),
            playlists: None,
            metadata: None,
            queue_snapshot: Default::default(),
        });
        assert!(controls.send(PlayerAction::Pause).is_ok());
        assert!(controls.connected_or_connecting("listener"));
        assert!(!controls.connected_or_connecting("another-account"));
        assert!(controls.connection_event(None).is_none());
        assert!(controls.connection_event(Some("another-account")).is_none());
        assert!(
            matches!(controls.connection_event(Some("listener")),Some(NativeEvent::Ready(device))if device=="device")
        );
        assert!(!controls.has_track());
        assert!(matches!(receiver.try_recv().unwrap(), PlayerAction::Pause));
        assert!(controls.send(PlayerAction::Volume(50)).is_ok());
        assert!(!controls.has_track());
        assert!(controls.send(PlayerAction::Next).is_ok());
        assert!(matches!(receiver.try_recv().unwrap(), PlayerAction::Next));
        assert!(!controls.has_track());
        assert!(
            controls
                .send(PlayerAction::Play(Some(
                    "spotify:track:0123456789abcdefghijkl".into()
                )))
                .is_ok()
        );
        assert!(matches!(
            receiver.try_recv().unwrap(),
            PlayerAction::Play(Some(_))
        ));
        assert!(controls.has_track());
        loaded.store(false, Ordering::Release);
        assert!(!controls.has_track());
        ready.store(false, Ordering::Release);
        assert!(matches!(
            controls.connection_event(Some("listener")),
            Some(NativeEvent::Error(_))
        ));
        *status.lock().unwrap() = Some(NativeEvent::Authorizing);
        assert!(controls.connected_or_connecting("listener"));
        assert!(matches!(
            controls.connection_event(Some("listener")),
            Some(NativeEvent::Authorizing)
        ));
        *status.lock().unwrap() = None;
        assert!(controls.connected_or_connecting("listener"));
        receiver.close();
        assert!(!controls.connected_or_connecting("listener"));
        assert!(matches!(
            controls.connection_event(Some("listener")),
            Some(NativeEvent::Error(_))
        ));
        assert!(matches!(
            controls.send(PlayerAction::Pause),
            Err(PlayerAction::Pause)
        ));
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn local_enqueue_uses_native_transport_without_interrupting_playback() {
        let (controls, mut receiver) = NativeControls::test_active("test-native-enqueue");
        let uri = "spotify:track:0000000000000000000001";
        assert!(controls.send(PlayerAction::Enqueue(uri.into())).is_ok());
        assert!(matches!(receiver.try_recv(), Ok(PlayerAction::Enqueue(queued)) if queued == uri));
        assert!(controls.has_track());
    }

    #[test]
    fn transfers_accept_only_known_destinations_and_the_local_alias() {
        let devices = [crate::model::Device {
            id: Some("registered-speaker".into()),
            ..Default::default()
        }];
        assert_eq!(
            transfer_target("native", "local-player", &devices).unwrap(),
            "local-player"
        );
        assert_eq!(
            transfer_target("registered-speaker", "local-player", &devices).unwrap(),
            "registered-speaker"
        );
        assert!(transfer_target("unknown-speaker", "local-player", &devices).is_err());
        assert!(transfer_target("../other-account", "local-player", &devices).is_err());
    }

    #[test]
    fn alexa_alias_and_bare_ids_reach_the_listed_speaker() {
        let listed = |id: &str| crate::model::Device {
            id: Some(id.into()),
            name: "Gameroom".into(),
            ..Default::default()
        };
        // The picker kept the alias ID; the latest list has the bare one.
        let bare = [listed("6402d6ca-echo")];
        assert_eq!(
            transfer_target("6402d6ca-echo_amzn_1", "local-player", &bare).unwrap(),
            "6402d6ca-echo"
        );
        // And the reverse after a reconnect lists aliases again.
        let alias = [listed("6402d6ca-echo_amzn_1")];
        assert_eq!(
            listed_device("6402d6ca-echo", &alias).and_then(|device| device.id.as_deref()),
            Some("6402d6ca-echo_amzn_1")
        );
        assert!(listed_device("other-echo_amzn_1", &bare).is_none());
        assert!(transfer_target("other-echo", "local-player", &bare).is_err());
    }

    #[test]
    fn transfers_settle_only_once_spotify_names_the_destination_active() {
        let device = |id: &str, active: bool| crate::model::Device {
            id: Some(id.into()),
            is_active: active,
            ..Default::default()
        };
        // Live: Spotify accepted the transfer about 2.4 s before its device
        // list stopped naming this computer active.
        let accepted = [device("local-player", true), device("echo", false)];
        assert!(!transfer_settled("echo", &accepted));
        let moved = [device("local-player", false), device("echo", true)];
        assert!(transfer_settled("echo", &moved));
        assert!(transfer_settled("echo_amzn_1", &moved));
        assert!(!transfer_settled("local-player", &moved));
    }

    #[test]
    fn standby_speakers_stay_selectable_and_explain_spotify_rejections() {
        use librespot_core::error::ErrorKind;
        let echo = crate::model::Device {
            id: Some("echo".into()),
            name: "Kitchen Echo".into(),
            is_offline: true,
            ..Default::default()
        };
        // Spotify may have woken the speaker since the last cluster update.
        assert_eq!(
            transfer_target("echo", "local-player", std::slice::from_ref(&echo)).unwrap(),
            "echo"
        );
        // HTTP 410 from connect-state arrives as NotFound ("Requested entity
        // was not found"), which used to read as a power problem.
        let standby = transfer_failure(ErrorKind::NotFound, Some(&echo));
        assert!(
            standby.starts_with("Kitchen Echo is on standby"),
            "{standby}"
        );
        assert!(!standby.contains("Requested entity"));
        let online = crate::model::Device {
            is_offline: false,
            ..echo
        };
        assert!(
            transfer_failure(ErrorKind::NotFound, Some(&online))
                .starts_with("Spotify could not reach Kitchen Echo")
        );
        assert!(
            transfer_failure(ErrorKind::NotFound, None)
                .starts_with("Spotify could not reach The speaker")
        );
        assert!(
            transfer_failure(ErrorKind::Unavailable, Some(&online))
                .contains("(Service unavailable)")
        );
    }

    #[test]
    fn an_inactive_player_cannot_silently_accept_queue_additions() {
        let (controls, mut receiver) = NativeControls::test_active("test-native-enqueue");
        controls
            .0
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .loaded
            .store(false, Ordering::Release);
        assert!(
            controls
                .send(PlayerAction::Enqueue(
                    "spotify:track:0000000000000000000001".into()
                ))
                .is_err()
        );
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn remote_only_commands_keep_their_web_api_route() {
        let (controls, _receiver) = NativeControls::test_active("test-native-enqueue");
        assert!(matches!(
            controls.send(PlayerAction::Transfer("remote".into())),
            Err(PlayerAction::Transfer(_))
        ));
    }

    fn stored_credentials() -> Credentials {
        Credentials {
            username: Some("account".into()),
            auth_type: AuthenticationType::AUTHENTICATION_STORED_SPOTIFY_CREDENTIALS,
            auth_data: b"test-reusable-credential".to_vec(),
        }
    }

    #[test]
    fn temporary_audio_failure_keeps_saved_login_and_does_not_open_browser() {
        let browser = Cell::new(0);
        let deletions = Cell::new(0);
        let outcome = connect_with_credentials(
            Some(stored_credentials()),
            &AtomicBool::new(false),
            || {
                browser.set(browser.get() + 1);
                anyhow::bail!("unexpected browser login");
            },
            |_| Outcome::Failed(Failure::other("connection timed out")),
            || {
                deletions.set(deletions.get() + 1);
                Ok(())
            },
        );
        assert!(matches!(outcome, Outcome::Failed(_)));
        assert_eq!(browser.get(), 0);
        assert_eq!(deletions.get(), 0);
    }

    #[test]
    fn rejected_saved_login_authorizes_again_only_once() {
        let attempts = Cell::new(0);
        let browser = Cell::new(0);
        let deletions = Cell::new(0);
        let outcome = connect_with_credentials(
            Some(stored_credentials()),
            &AtomicBool::new(false),
            || {
                browser.set(browser.get() + 1);
                Ok(Credentials::with_access_token("test-access-token"))
            },
            |credentials| {
                attempts.set(attempts.get() + 1);
                if attempts.get() == 1 {
                    assert!(valid_credentials(&credentials, "account"));
                } else {
                    assert!(
                        credentials.auth_type == AuthenticationType::AUTHENTICATION_SPOTIFY_TOKEN
                    );
                }
                Outcome::Failed(Failure {
                    reason: "credentials rejected".into(),
                    rejected_credentials: true,
                    retryable: false,
                })
            },
            || {
                deletions.set(deletions.get() + 1);
                Ok(())
            },
        );
        assert!(matches!(outcome, Outcome::Failed(_)));
        assert_eq!(attempts.get(), 2);
        assert_eq!(browser.get(), 1);
        assert_eq!(deletions.get(), 1);
    }

    #[test]
    fn saved_login_connects_without_browser_and_cancellation_skips_work() {
        let attempts = Cell::new(0);
        let outcome = connect_with_credentials(
            Some(stored_credentials()),
            &AtomicBool::new(false),
            || panic!("saved login must not open browser"),
            |_| {
                attempts.set(attempts.get() + 1);
                Outcome::Stopped
            },
            || panic!("working login must not be deleted"),
        );
        assert!(matches!(outcome, Outcome::Stopped));
        assert_eq!(attempts.get(), 1);
        let outcome = connect_with_credentials(
            None,
            &AtomicBool::new(true),
            || panic!("cancelled login must not open browser"),
            |_| panic!("cancelled login must not connect"),
            || panic!("cancelled login must not delete credentials"),
        );
        assert!(matches!(outcome, Outcome::Stopped));
    }

    #[test]
    fn reusable_credentials_roundtrip_and_are_bound_to_the_account() {
        let value = serde_json::to_string(&stored_credentials()).unwrap();
        let credentials: Credentials = serde_json::from_str(&value).unwrap();
        assert!(valid_credentials(&credentials, "account"));
        assert!(!valid_credentials(&credentials, "different-account"));
        assert!(!valid_credentials(
            &Credentials::with_access_token("test-token"),
            "account"
        ));
    }

    #[test]
    fn only_credential_rejections_trigger_browser_reauthorization() {
        let rejected = Failure::from_connection(librespot_core::Error::permission_denied(
            "Login failed with reason: Bad credentials",
        ));
        assert!(rejected.rejected_credentials);
        let network =
            Failure::from_connection(librespot_core::Error::unavailable("network failure"));
        assert!(!network.rejected_credentials);
        let premium = Failure::from_connection(librespot_core::Error::permission_denied(
            "Login failed with reason: Premium account required",
        ));
        assert!(!premium.rejected_credentials);
        let unrelated = Failure::from_connection(librespot_core::Error::permission_denied(
            "different permission failure",
        ));
        assert!(!unrelated.rejected_credentials);
    }
}
