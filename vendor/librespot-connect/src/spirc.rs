use crate::{
    LoadContextOptions, LoadRequestOptions, PlayContext,
    context_resolver::{ContextAction, ContextResolver, LEXICON_CONTEXT_URL, ResolveContext},
    core::{
        Error, Session, SpotifyUri,
        authentication::Credentials,
        dealer::{
            manager::{BoxedStream, BoxedStreamResult, Reply, RequestReply},
            protocol::{Command, FallbackWrapper, Message, Request, describe_unknown},
        },
        session::UserAttributes,
        spclient::TransferRequest,
    },
    jam_follow::{self, HostPlayback, JamPhase, LocalPlayback, Plan},
    model::{LoadRequest, PlayingTrack, SpircPlayStatus},
    playback::{
        mixer::Mixer,
        player::{Player, PlayerEvent, PlayerEventChannel, QueueTrack},
    },
    protocol::{
        connect::{Cluster, ClusterUpdate, LogoutCommand, SetVolumeCommand},
        context::Context,
        explicit_content_pubsub::UserAttributesUpdate,
        player::{PlayOrigin, ProvidedTrack},
        playlist4_external::PlaylistModificationInfo,
        social_connect_v2::{Session as JamSession, SessionUpdate, SessionUpdateReason},
        transfer_state::TransferState,
        user_attributes::UserAttributesMutation,
    },
    state::{
        context::{ContextType, ResetContext},
        provider::IsProvider,
        {ConnectConfig, ConnectState},
    },
};
use futures_util::StreamExt;
use librespot_protocol::context_page::ContextPage;
use protobuf::MessageField;
use std::{
    future::Future,
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use tokio::{sync::mpsc, time::sleep};

#[derive(Debug, Error)]
enum SpircError {
    #[error("response payload empty")]
    NoData,
    #[error("{0} had no uri")]
    NoUri(&'static str),
    #[error("message pushed for another URI")]
    InvalidUri(String),
    #[error("failed to put connect state for new device")]
    FailedDealerSetup,
    #[error("unknown endpoint: {0}")]
    UnknownEndpoint(String),
}

impl From<SpircError> for Error {
    fn from(err: SpircError) -> Self {
        use SpircError::*;
        match err {
            NoData | NoUri(_) => Error::unavailable(err),
            InvalidUri(_) | FailedDealerSetup => Error::aborted(err),
            UnknownEndpoint(_) => Error::unimplemented(err),
        }
    }
}

struct SpircTask {
    devices: ConnectedDevices,
    narrated: crate::narrated::Narrated,
    state_updates: crate::state_updates::StateUpdates,
    resume_on_startup: bool,
    player: Arc<Player>,
    mixer: Arc<dyn Mixer>,

    /// the state management object
    connect_state: ConnectState,
    connect_established: bool,

    play_request_id: Option<u64>,
    play_status: SpircPlayStatus,

    connection_id_update: BoxedStreamResult<String>,
    connect_state_update: BoxedStreamResult<ClusterUpdate>,
    connect_state_volume_update: BoxedStreamResult<SetVolumeCommand>,
    connect_state_logout_request: BoxedStreamResult<LogoutCommand>,
    playlist_update: BoxedStreamResult<PlaylistModificationInfo>,
    session_update: BoxedStreamResult<FallbackWrapper<SessionUpdate>>,
    connect_state_command: BoxedStream<RequestReply>,
    user_attributes_update: BoxedStreamResult<UserAttributesUpdate>,
    user_attributes_mutation: BoxedStreamResult<UserAttributesMutation>,

    commands: Option<mpsc::UnboundedReceiver<SpircCommand>>,
    player_events: Option<PlayerEventChannel>,

    context_resolver: ContextResolver,

    emit_set_queue_events: bool,

    shutdown: bool,
    session: Session,

    /// is set when transferring, and used after resolving the contexts to finish the transfer
    pub transfer_state: Option<TransferState>,

    /// The Jam queue last lined up, so resends with the same queue skip it.
    jam_queue_hash: Option<u64>,
    /// The Jam this device hosts, whose session its state carries.
    jam_host: Option<String>,
    /// The Jam diagnostics last logged: the active device's state, then this one's.
    jam_states_logged: (String, String),
    /// The Jam the last session update named.
    jam_session_seen: Option<String>,
    /// Whether the Jam this device hosts has guests.
    jam_has_guests: bool,
    /// The active device's capabilities as last logged.
    jam_device_logged: String,
    /// The active device last written to the diagnostics log.
    active_device_logged: String,

    /// when set to true, it will update the volume after [VOLUME_UPDATE_DELAY],
    /// when no other future resolves, otherwise resets the delay
    update_volume: bool,

    /// when set to true, it will update the volume after [UPDATE_STATE_DELAY],
    /// when no other future resolves, otherwise resets the delay
    update_state: bool,

    spirc_id: usize,
}

static SPIRC_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug, PartialEq, Eq)]
/// Playback destination advertised by the signed-in account's Connect cluster.
pub struct ConnectedDevice {
    /// Spotify reports this destination on standby; transfers to it fail
    /// until the speaker reconnects to Spotify itself.
    pub is_offline: bool,
    /// Spotify Connect device identifier.
    pub id: String,
    /// User-visible device name.
    pub name: String,
    /// Whether this destination owns current playback.
    pub is_active: bool,
    /// Adjustable volume, or None for fixed-volume destinations.
    pub volume_percent: Option<u8>,
}

/// The device a Connect identifier belongs to. The put-state response lists
/// Alexa speakers once per alias, as `<device>_amzn_<alias>`, while dealer
/// cluster updates list the bare `<device>`; both name the same speaker.
pub fn speaker_id(id: &str) -> &str {
    id.rsplit_once('_')
        .filter(|(_, alias)| !alias.is_empty() && alias.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|(rest, _)| rest.rsplit_once('_'))
        .filter(|(device, vendor)| {
            !device.is_empty()
                && !vendor.is_empty()
                && vendor.bytes().all(|b| b.is_ascii_alphabetic())
        })
        .map_or(id, |(device, _)| device)
}

/// True for names that are really identifiers, such as the UUIDs some
/// speakers and Cast groups register with.
fn looks_like_id(name: &str) -> bool {
    let name = name.trim();
    let hex = name.chars().filter(char::is_ascii_hexdigit).count();
    name.is_empty()
        || (name.len() >= 16
            && hex * 10 >= name.len() * 8
            && name.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
}

/// A readable name for a Connect device. Some devices advertise an ID as
/// their name; fall back to their alias, audio output, brand and model, or
/// kind of device.
pub fn device_display_name(device: &librespot_protocol::connect::DeviceInfo) -> String {
    use librespot_protocol::devices::DeviceType;
    if !looks_like_id(&device.name) {
        return device.name.trim().to_owned();
    }
    let usable = |name: &str| (!looks_like_id(name)).then(|| name.trim().to_owned());
    let mut aliases: Vec<_> = device.device_aliases.values().collect();
    aliases.sort_by_key(|alias| (alias.id != device.selected_alias_id, alias.id));
    if let Some(name) = aliases.iter().find_map(|alias| usable(&alias.display_name)) {
        return name;
    }
    if let Some(name) = device
        .audio_output_device_info
        .as_ref()
        .and_then(|info| info.device_name.as_deref())
        .and_then(usable)
    {
        return name;
    }
    if let Some(name) = usable(&format!("{} {}", device.brand.trim(), device.model.trim())) {
        return name;
    }
    let kind = match device.device_type.enum_value() {
        Ok(DeviceType::SPEAKER) => "Speaker",
        Ok(DeviceType::TV) => "TV",
        Ok(DeviceType::AVR) => "AV receiver",
        Ok(DeviceType::STB) => "Set-top box",
        Ok(DeviceType::AUDIO_DONGLE) => "Audio dongle",
        Ok(DeviceType::GAME_CONSOLE) => "Game console",
        Ok(DeviceType::CAST_VIDEO) => "Cast device",
        Ok(DeviceType::CAST_AUDIO) => "Cast speaker",
        Ok(DeviceType::AUTOMOBILE | DeviceType::CAR_THING) => "Car",
        Ok(DeviceType::SMARTWATCH) => "Watch",
        Ok(DeviceType::COMPUTER | DeviceType::CHROMEBOOK) => "Computer",
        Ok(DeviceType::SMARTPHONE) => "Phone",
        Ok(DeviceType::TABLET) => "Tablet",
        _ => "",
    };
    if !kind.is_empty() {
        return if device.is_group {
            format!("{kind} group")
        } else {
            kind.to_owned()
        };
    }
    device.name.trim().to_owned()
}

#[cfg(test)]
mod display_name_tests {
    use super::{device_display_name, looks_like_id};
    use librespot_protocol::{connect::DeviceInfo, devices::DeviceAlias};

    #[test]
    fn readable_names_are_kept_and_ids_are_replaced() {
        assert!(looks_like_id("2c5fe5dc-8c99-4b20-9eff-546a4fd16960"));
        assert!(!looks_like_id("Kitchen speaker"));
        assert!(!looks_like_id("Cafe"));
        let mut device = DeviceInfo::new();
        device.name = "Gameroom".into();
        assert_eq!(device_display_name(&device), "Gameroom");

        device.name = "75cb14aa-b9cc-4b8d-7a47-60ac5c1a7d00".into();
        device.brand = "Sonos".into();
        device.model = "One".into();
        assert_eq!(device_display_name(&device), "Sonos One");

        let mut alias = DeviceAlias::new();
        alias.id = 2;
        alias.display_name = "Living room".into();
        device.device_aliases.insert("2".into(), alias);
        device.selected_alias_id = 2;
        assert_eq!(device_display_name(&device), "Living room");
    }
}

/// Account-local device discovery supplied by the existing Connect subscription.
#[derive(Clone, Default)]
pub struct ConnectedDevices(
    Arc<Mutex<Vec<ConnectedDevice>>>,
    Arc<Mutex<Option<Cluster>>>,
    Arc<Mutex<Option<i64>>>,
    Arc<AtomicU8>,
);

impl ConnectedDevices {
    /// Latest account-local Connect state, supplied by the existing subscription.
    pub fn cluster(&self) -> Option<Cluster> {
        self.1.lock().ok().and_then(|cluster| cluster.clone())
    }
    /// Return the latest complete device list without making a network request.
    pub fn snapshot(&self) -> Vec<ConnectedDevice> {
        self.0
            .lock()
            .map(|devices| devices.clone())
            .unwrap_or_default()
    }

    /// Milliseconds this computer's clock runs ahead of Spotify's, measured
    /// when the latest cluster arrived. Remote positions are stamped with
    /// Spotify's clock, so extrapolating them with a skewed local clock
    /// shows progress running ahead while playing and jumping back on pause.
    pub fn server_clock_offset_ms(&self) -> Option<i64> {
        self.2.lock().ok().and_then(|offset| *offset)
    }

    /// Whether this device follows the Jam it is in.
    pub fn jam_phase(&self) -> JamPhase {
        JamPhase::from_u8(self.3.load(Ordering::Acquire))
    }

    /// Stop following a Jam this device left, keeping the current song.
    pub fn leave_jam(&self) {
        self.set_jam_phase(JamPhase::Left);
    }

    /// Follow the next Jam this device joins.
    pub fn join_jam(&self) {
        if self.jam_phase() == JamPhase::Left {
            self.set_jam_phase(JamPhase::Idle);
        }
    }

    /// Returns whether the phase changed.
    fn set_jam_phase(&self, phase: JamPhase) -> bool {
        self.3.swap(phase.to_u8(), Ordering::AcqRel) != phase.to_u8()
    }

    fn update(&self, cluster: &Cluster) {
        let received = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |now| now.as_millis().min(i64::MAX as u128) as i64);
        self.update_at(cluster, received);
    }

    fn update_at(&self, cluster: &Cluster, received_ms: i64) {
        if cluster.server_timestamp_ms > 0
            && let Ok(mut offset) = self.2.lock()
        {
            *offset = Some(received_ms.saturating_sub(cluster.server_timestamp_ms));
        }
        if let Ok(mut current) = self.1.lock() {
            *current = Some(cluster.clone());
        }
        let mut devices: Vec<_> = cluster
            .device
            .iter()
            .filter(|(id, device)| !id.is_empty() && device.can_play)
            .map(|(id, device)| (id, device, device_display_name(device)))
            .filter(|(_, _, name)| !name.is_empty())
            .map(|(id, device, name)| ConnectedDevice {
                is_offline: device.is_offline,
                id: id.clone(),
                name,
                is_active: id == &cluster.active_device_id,
                volume_percent: (!device.capabilities.disable_volume)
                    .then_some((device.volume.min(65535) * 100 / 65535) as u8),
            })
            .collect();
        if let Ok(current) = self.0.lock() {
            let remembered: Vec<_> = current
                .iter()
                // A remembered alias ID stops working once Spotify lists the
                // speaker under another form, so keep only one entry per speaker.
                .filter(|device| {
                    device.is_offline
                        && !devices
                            .iter()
                            .any(|next| speaker_id(&next.id) == speaker_id(&device.id))
                })
                .take(256)
                .cloned()
                .map(|mut device| {
                    device.is_active = device.id == cluster.active_device_id;
                    device
                })
                .collect();
            devices.extend(remembered);
        }
        devices.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
        if let Ok(mut current) = self.0.lock() {
            *current = devices;
        }
    }
}

#[cfg(test)]
mod connected_device_tests {
    use super::*;
    use crate::protocol::connect::DeviceInfo;

    #[test]
    fn device_updates_keep_standby_speakers_and_filter_non_players() {
        let devices = ConnectedDevices::default();
        let mut cluster = Cluster {
            active_device_id: "headphones".into(),
            ..Default::default()
        };
        let active = DeviceInfo {
            name: "Headphones".into(),
            can_play: true,
            volume: 65535,
            ..Default::default()
        };
        cluster.device.insert("headphones".into(), active.clone());
        cluster.device.insert(
            "controller".into(),
            DeviceInfo {
                name: "Controller".into(),
                ..Default::default()
            },
        );
        cluster.device.insert(
            "offline".into(),
            DeviceInfo {
                is_offline: true,
                ..active.clone()
            },
        );
        devices.update(&cluster);
        assert_eq!(devices.cluster().unwrap().active_device_id, "headphones");
        let snapshot = devices.snapshot();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].id, "headphones");
        assert!(snapshot[0].is_active);
        assert_eq!(snapshot[0].volume_percent, Some(100));
        assert_eq!(snapshot[1].id, "offline");
        assert!(!snapshot[1].is_active);
        cluster.device.clear();
        devices.update(&cluster);
        assert_eq!(
            devices.snapshot().len(),
            1,
            "Live updates may omit registered standby speakers"
        );
        assert_eq!(devices.snapshot()[0].id, "offline");
        assert!(ConnectedDevices::default().snapshot().is_empty());
    }

    #[test]
    fn alias_ids_from_put_state_do_not_outlive_the_dealer_listing() {
        use crate::protocol::devices::DeviceAlias;
        let echo = DeviceInfo {
            name: "Kitchen Echo".into(),
            can_play: true,
            is_offline: true,
            ..Default::default()
        };
        let devices = ConnectedDevices::default();
        // Shape of the put-state response: one entry per Alexa alias.
        let mut put_state = Cluster::default();
        put_state
            .device
            .insert("2c5fe5dc-echo_amzn_1".into(), echo.clone());
        devices.update(&put_state);
        assert_eq!(devices.snapshot()[0].id, "2c5fe5dc-echo_amzn_1");
        assert!(devices.snapshot()[0].is_offline);
        // Shape of a dealer cluster update: the bare device with its aliases.
        let mut dealer = Cluster::default();
        let mut bare = DeviceInfo {
            name: "2c5fe5dc-0000-4000-8000-000000000000".into(),
            ..echo
        };
        bare.device_aliases.insert(
            "".into(),
            DeviceAlias {
                id: 1,
                display_name: "Kitchen Echo".into(),
                ..Default::default()
            },
        );
        dealer.device.insert("2c5fe5dc-echo".into(), bare);
        devices.update(&dealer);
        let snapshot = devices.snapshot();
        assert_eq!(
            snapshot
                .iter()
                .map(|device| device.id.as_str())
                .collect::<Vec<_>>(),
            ["2c5fe5dc-echo"],
            "A stale alias ID must not stay listed as a second speaker"
        );
        assert_eq!(snapshot[0].name, "Kitchen Echo");
        assert!(snapshot[0].is_offline);
        // A reconnect lists aliases again; the bare form is not kept beside them.
        devices.update(&put_state);
        assert_eq!(devices.snapshot().len(), 1);
    }

    #[test]
    fn cluster_receipt_measures_local_clock_skew() {
        let devices = ConnectedDevices::default();
        assert_eq!(devices.server_clock_offset_ms(), None);
        devices.update_at(
            &Cluster {
                server_timestamp_ms: 1_000_000,
                ..Default::default()
            },
            1_002_400,
        );
        assert_eq!(devices.server_clock_offset_ms(), Some(2_400));
        // Clusters without a server timestamp keep the last measurement.
        devices.update_at(&Cluster::default(), 5_000_000);
        assert_eq!(devices.server_clock_offset_ms(), Some(2_400));
    }

    #[test]
    fn speaker_ids_strip_only_vendor_alias_suffixes() {
        assert_eq!(speaker_id("2c5fe5dc_amzn_1"), "2c5fe5dc");
        assert_eq!(speaker_id("2c5fe5dc_amzn_12"), "2c5fe5dc");
        assert_eq!(speaker_id("2c5fe5dc"), "2c5fe5dc");
        assert_eq!(speaker_id("cast_group_x"), "cast_group_x");
        assert_eq!(speaker_id("_amzn_1"), "_amzn_1");
        assert_eq!(speaker_id("device_2"), "device_2");
    }
}

#[derive(Debug)]
enum SpircCommand {
    Play,
    PlayPause,
    Pause,
    Prev,
    Next,
    VolumeUp,
    VolumeDown,
    Shutdown,
    Shuffle(bool),
    Repeat(bool),
    RepeatTrack(bool),
    Disconnect { pause: bool },
    SetPosition(u32),
    SetVolume(u16),
    Activate,
    PrewarmDJ,
    Transfer(Option<TransferRequest>),
    Load(LoadRequest),
    AddToQueue(SpotifyUri),
    RemoveFromQueue(String),
    ClearQueue,
    NextTo(String),
    HostJam(Option<String>),
}

const CONTEXT_FETCH_THRESHOLD: usize = 2;

// delay to update volume after a certain amount of time, instead on each update request
const VOLUME_UPDATE_DELAY: Duration = Duration::from_millis(500);
// to reduce updates to remote, we group some request by waiting for a set amount of time
const UPDATE_STATE_DELAY: Duration = Duration::from_millis(200);
/// The context metadata entry naming the Jam a state belongs to.
const JAM_SESSION_KEY: &str = "jam-session-id";
/// Marks a requested DJ queue; the pre-rename `spotiflex.dj_request` key is still accepted.
/// A device ID cut short for the diagnostics log.
fn short_device(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn is_dj_request(metadata: &std::collections::HashMap<String, String>) -> bool {
    metadata.contains_key("lyssna.dj_request") || metadata.contains_key("spotiflex.dj_request")
}
fn requested_context_index(context: &Context, track: &PlayingTrack) -> Option<usize> {
    let mut tracks = context.pages.iter().flat_map(|page| &page.tracks);
    match track {
        PlayingTrack::Index(index) => Some(*index as usize),
        PlayingTrack::Uri(uri) => tracks.position(|track| track.uri.as_ref() == Some(uri)),
        PlayingTrack::Uid(uid) => tracks.position(|track| track.uid.as_ref() == Some(uid)),
        PlayingTrack::UidOrUri { uid, uri } => context
            .pages
            .iter()
            .flat_map(|page| &page.tracks)
            .position(|track| track.uid.as_ref() == Some(uid))
            .or_else(|| {
                context
                    .pages
                    .iter()
                    .flat_map(|page| &page.tracks)
                    .position(|track| track.uri.as_ref() == Some(uri))
            }),
    }
}
/// Whether the requested track, moved on by `offset`, lies past the pages
/// loaded so far. A track picked deep in a long playlist from another client
/// is not in the first page Spotify resolves.
fn requested_track_unloaded(context: &Context, track: &PlayingTrack, offset: usize) -> bool {
    let length = context
        .pages
        .iter()
        .map(|page| page.tracks.len())
        .sum::<usize>();
    requested_context_index(context, track)
        .is_none_or(|index| index.saturating_add(offset) >= length)
}
/// Pages fetched at most to find a requested track: 10,000 tracks.
const MAX_REQUESTED_TRACK_PAGES: usize = 100;
/// A Jam session in the JSON shape of Spotify's social-connect REST replies.
fn jam_session_json(session: &JamSession) -> String {
    serde_json::json!({
        "timestamp": session.timestamp,
        "session_id": session.session_id,
        "join_session_token": session.join_session_token,
        "join_session_uri": session.join_session_uri,
        "session_owner_id": session.session_owner_id,
        "is_session_owner": session.is_session_owner,
        "session_members": session
            .session_members
            .iter()
            .map(|member| serde_json::json!({
                "id": member.id,
                "username": member.username,
                "display_name": member.display_name,
                "image_url": member.image_url,
            }))
            .collect::<Vec<_>>(),
    })
    .to_string()
}

fn automatic_restore_allowed(enabled: bool, cluster: &Cluster, session_id: &str) -> bool {
    enabled
        && cluster.active_device_id.is_empty()
        && (cluster.player_state.session_id.is_empty()
            || cluster.player_state.session_id == session_id)
        && !cluster.transfer_data.is_empty()
}

/// The spotify connect handle
pub struct Spirc {
    devices: ConnectedDevices,
    commands: mpsc::UnboundedSender<SpircCommand>,
}

impl Spirc {
    /// Obtain this connection's independently readable device list.
    pub fn devices(&self) -> ConnectedDevices {
        self.devices.clone()
    }
    /// Initializes a new spotify connect device
    ///
    /// The returned tuple consists out of a handle to the [`Spirc`] that
    /// can control the local connect device when active. And a [`Future`]
    /// which represents the [`Spirc`] event loop that processes the whole
    /// connect device logic.
    pub async fn new(
        config: ConnectConfig,
        session: Session,
        credentials: Credentials,
        player: Arc<Player>,
        mixer: Arc<dyn Mixer>,
    ) -> Result<(Spirc, impl Future<Output = ()>), Error> {
        Self::new_with_session(config, session, Some(credentials), player, mixer).await
    }

    /// Reuse an authenticated session when `credentials` is `None`.
    /// The caller must keep the session's Tokio runtime alive until playback ends.
    pub async fn new_with_session(
        config: ConnectConfig,
        session: Session,
        credentials: Option<Credentials>,
        player: Arc<Player>,
        mixer: Arc<dyn Mixer>,
    ) -> Result<(Spirc, impl Future<Output = ()>), Error> {
        if credentials.is_none() {
            if session.is_invalid() {
                return Err(Error::unavailable(
                    "Spotify session closed before playback startup",
                ));
            }
            if session.username().is_empty() || session.auth_data().is_empty() {
                return Err(Error::unauthenticated(
                    "An authenticated Spotify session is required",
                ));
            }
        }
        fn extract_connection_id(msg: Message) -> Result<String, Error> {
            let connection_id = msg
                .headers
                .get("Spotify-Connection-Id")
                .ok_or_else(|| SpircError::InvalidUri(msg.uri.clone()))?;
            Ok(connection_id.to_owned())
        }

        let spirc_id = SPIRC_COUNTER.fetch_add(1, Ordering::AcqRel);
        debug!("new Spirc[{spirc_id}]");

        let emit_set_queue_events = config.emit_set_queue_events;
        let resume_on_startup = config.resume_on_startup;
        let connect_state = ConnectState::new(config, &session);

        let connection_id_update = session
            .dealer()
            .listen_for("hm://pusher/v1/connections/", extract_connection_id)?;

        let connect_state_update = session
            .dealer()
            .listen_for("hm://connect-state/v1/cluster", Message::from_raw)?;

        let connect_state_volume_update = session
            .dealer()
            .listen_for("hm://connect-state/v1/connect/volume", Message::from_raw)?;

        let connect_state_logout_request = session
            .dealer()
            .listen_for("hm://connect-state/v1/connect/logout", Message::from_raw)?;

        let playlist_update = session
            .dealer()
            .listen_for("hm://playlist/v2/playlist/", Message::from_raw)?;

        let session_update = session
            .dealer()
            .listen_for("social-connect/v2/session_update", Message::try_from_json)?;

        let user_attributes_update = session
            .dealer()
            .listen_for("spotify:user:attributes:update", Message::from_raw)?;

        // can be trigger by toggling autoplay in a desktop client
        let user_attributes_mutation = session
            .dealer()
            .listen_for("spotify:user:attributes:mutated", Message::from_raw)?;

        let connect_state_command = session
            .dealer()
            .handle_for("hm://connect-state/v1/player/command")?;

        // pre-acquire client_token, preventing multiple request while running
        let _ = session.spclient().client_token().await?;

        // Connect *after* all message listeners are registered
        if let Some(credentials) = credentials {
            session.connect(credentials, true).await?;
        }

        // pre-acquire access_token (we need to be authenticated to retrieve a token)
        let _ = session.login5().auth_token().await?;

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();

        let player_events = player.get_player_event_channel();

        let mut task = SpircTask {
            devices: ConnectedDevices::default(),
            state_updates: Default::default(),
            narrated: Default::default(),
            resume_on_startup,
            player,
            mixer,

            connect_state,
            connect_established: false,

            play_request_id: None,
            play_status: SpircPlayStatus::Stopped,

            connection_id_update,
            connect_state_update,
            connect_state_volume_update,
            connect_state_logout_request,
            playlist_update,
            session_update,
            connect_state_command,
            user_attributes_update,
            user_attributes_mutation,
            commands: Some(cmd_rx),
            player_events: Some(player_events),

            context_resolver: ContextResolver::new(session.clone()),

            emit_set_queue_events,

            shutdown: false,
            session,

            transfer_state: None,
            jam_queue_hash: None,
            jam_host: None,
            jam_states_logged: Default::default(),
            jam_session_seen: None,
            jam_has_guests: false,
            jam_device_logged: String::new(),
            active_device_logged: String::new(),
            update_volume: false,
            update_state: false,

            spirc_id,
        };

        let spirc = Spirc {
            commands: cmd_tx,
            devices: task.devices.clone(),
        };

        let initial_volume = task.connect_state.device_info().volume;
        task.connect_state.set_volume(0);

        match initial_volume.try_into() {
            Ok(volume) => {
                task.set_volume(volume);
                // we don't want to update the volume initially,
                // we just want to set the mixer to the correct volume
                task.update_volume = false;
            }
            Err(why) => error!("failed to update initial volume: {why}"),
        };

        Ok((spirc, task.run()))
    }

    /// Safely shutdowns the spirc.
    ///
    /// This pauses the playback, disconnects the connect device and
    /// bring the future initially returned to an end.
    pub fn shutdown(&self) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::Shutdown)?)
    }

    /// Resumes the playback
    ///
    /// Does nothing if we are not the active device, or it isn't paused.
    pub fn play(&self) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::Play)?)
    }

    /// Resumes or pauses the playback
    ///
    /// Does nothing if we are not the active device.
    pub fn play_pause(&self) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::PlayPause)?)
    }

    /// Pauses the playback
    ///
    /// Does nothing if we are not the active device, or if it isn't playing.
    pub fn pause(&self) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::Pause)?)
    }

    /// Seeks to the beginning or skips to the previous track.
    ///
    /// Seeks to the beginning when the current track position
    /// is greater than 3 seconds.
    ///
    /// Does nothing if we are not the active device.
    pub fn prev(&self) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::Prev)?)
    }

    /// Skips to the next track.
    ///
    /// Does nothing if we are not the active device.
    pub fn next(&self) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::Next)?)
    }

    /// Increases the volume by configured steps of [ConnectConfig].
    ///
    /// Does nothing if we are not the active device.
    pub fn volume_up(&self) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::VolumeUp)?)
    }

    /// Decreases the volume by configured steps of [ConnectConfig].
    ///
    /// Does nothing if we are not the active device.
    pub fn volume_down(&self) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::VolumeDown)?)
    }

    /// Shuffles the playback according to the value.
    ///
    /// If true shuffles/reshuffles the playback. Otherwise, does
    /// nothing (if not shuffled) or unshuffles the playback while
    /// resuming at the position of the current track.
    ///
    /// Does nothing if we are not the active device.
    pub fn shuffle(&self, shuffle: bool) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::Shuffle(shuffle))?)
    }

    /// Repeats the playback context according to the value.
    ///
    /// Does nothing if we are not the active device.
    pub fn repeat(&self, repeat: bool) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::Repeat(repeat))?)
    }

    /// Repeats the current track if true.
    ///
    /// Does nothing if we are not the active device.
    ///
    /// Skipping to the next track disables the repeating.
    pub fn repeat_track(&self, repeat: bool) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::RepeatTrack(repeat))?)
    }

    /// Update the volume to the given value.
    ///
    /// Does nothing if we are not the active device.
    pub fn set_volume(&self, volume: u16) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::SetVolume(volume))?)
    }

    /// Updates the position to the given value.
    ///
    /// Does nothing if we are not the active device.
    ///
    /// If value is greater than the track duration,
    /// the update is ignored.
    pub fn set_position_ms(&self, position_ms: u32) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::SetPosition(position_ms))?)
    }

    /// Load a new context and replace the current.
    ///
    /// Does nothing if we are not the active device.
    ///
    /// Does not overwrite the queue.
    pub fn load(&self, command: LoadRequest) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::Load(command))?)
    }

    /// Adds a track, episode, album or playlist to the queue.
    ///
    /// Does nothing if we are not the active device.
    ///
    /// For albums and playlists, all tracks/episodes are resolved and added to the queue.
    pub fn add_to_queue(&self, uri: SpotifyUri) -> Result<(), Error> {
        if !matches!(
            uri,
            SpotifyUri::Track { .. }
                | SpotifyUri::Episode { .. }
                | SpotifyUri::Local { .. }
                | SpotifyUri::Album { .. }
                | SpotifyUri::Playlist { .. }
        ) {
            return Err(Error::invalid_argument("uri"));
        }
        Ok(self.commands.send(SpircCommand::AddToQueue(uri))?)
    }

    /// Removes all queued tracks from the next tracks.
    ///
    /// Does nothing if we are not the active device.
    ///
    /// Only tracks added to the queue (via [Spirc::add_to_queue] or a connect client)
    /// are removed. The current track, even if it was queued, and the tracks of the
    /// context are kept; the next tracks are filled up from the context again.
    pub fn clear_queue(&self) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::ClearQueue)?)
    }

    /// Removes the first queued track with this URI from the next tracks.
    /// Songs from the context are never removed.
    ///
    /// Does nothing if we are not the active device.
    pub fn remove_from_queue(&self, uri: String) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::RemoveFromQueue(uri))?)
    }

    /// Skips forward to an upcoming track, keeping the context and the queue
    /// after it. Does nothing when the track is not coming up.
    ///
    /// Does nothing if we are not the active device.
    pub fn next_to(&self, uri: String) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::NextTo(uri))?)
    }

    /// Disconnects the current device and pauses the playback according the value.
    ///
    /// Does nothing if we are not the active device.
    pub fn disconnect(&self, pause: bool) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::Disconnect { pause })?)
    }

    /// Host the Jam with this session ID from this device, or stop with `None`.
    pub fn host_jam(&self, session_id: Option<String>) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::HostJam(session_id))?)
    }

    /// Acquires the control as active connect device.
    ///
    /// Does not [Spirc::transfer] the playback. Does nothing if we are not the active device.
    pub fn activate(&self) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::Activate)?)
    }
    /// Prepare one unheard DJ set and its speech without activating this device.
    pub fn prewarm_dj(&self) -> Result<(), Error> {
        Ok(self.commands.send(SpircCommand::PrewarmDJ)?)
    }

    /// Acquires the control as active connect device over the transfer flow.
    ///
    /// Does nothing if we are not the active device.
    pub fn transfer(&self, transfer_request: Option<TransferRequest>) -> Result<(), Error> {
        Ok(self
            .commands
            .send(SpircCommand::Transfer(transfer_request))?)
    }
}

impl SpircTask {
    async fn run(mut self) {
        // simplify unwrapping of received item or parsed result
        macro_rules! unwrap {
            ( $next:expr, |$some:ident| $use_some:expr ) => {
                match $next {
                    Some($some) => $use_some,
                    None => {
                        error!("{} selected, but none received", stringify!($next));
                        break;
                    }
                }
            };
            ( $next:expr, match |$ok:ident| $use_ok:expr ) => {
                unwrap!($next, |$ok| match $ok {
                    Ok($ok) => $use_ok,
                    Err(why) => error!("could not parse {}: {}", stringify!($ok), why),
                })
            };
        }

        if let Err(why) = self.session.dealer().start().await {
            error!("starting dealer failed: {why}");
            return;
        }

        while !self.session.is_invalid() && !self.shutdown {
            let commands = self.commands.as_mut();
            let player_events = self.player_events.as_mut();

            // when state and volume update have a higher priority than context resolving
            // because of that the context resolving has to wait, so that the other tasks can finish
            let allow_context_resolving = !self.update_state && !self.update_volume;

            tokio::select! {
                result = self.state_updates.next() => {
                    if let Err(why) = result {
                        error!("state upload failed: {why}");
                    }
                },
                // startup of the dealer requires a connection_id, which is retrieved at the very beginning
                connection_id_update = self.connection_id_update.next() => unwrap! {
                    connection_id_update,
                    match |connection_id| if let Err(why) = self.handle_connection_id_update(connection_id).await {
                        error!("failed handling connection id update: {why}");
                        break;
                    }
                },
                // main dealer update of any remote device updates
                cluster_update = self.connect_state_update.next() => unwrap! {
                    cluster_update,
                    match |cluster_update| if let Err(e) = self.handle_cluster_update(cluster_update).await {
                        error!("could not dispatch connect state update: {e}");
                    }
                },
                // main dealer request handling (dealer expects an answer)
                request = self.connect_state_command.next() => unwrap! {
                    request,
                    |request| if let Err(e) = self.handle_connect_state_request(request).await {
                        error!("couldn't handle connect state command: {e}");
                    }
                },
                // volume request handling is send separately (it's more like a fire forget)
                volume_update = self.connect_state_volume_update.next() => unwrap! {
                    volume_update,
                    match |volume_update| match volume_update.volume.try_into() {
                        Ok(volume) => {
                            log::warn!(
                                target: "librespot_connect::devices",
                                "remote volume {}%, we are active: {}",
                                u32::from(volume) * 100 / u32::from(u16::MAX),
                                self.connect_state.is_active()
                            );
                            self.set_volume(volume)
                        }
                        Err(why) => error!("can't update volume, failed to parse i32 to u16: {why}")
                    }
                },
                logout_request = self.connect_state_logout_request.next() => unwrap! {
                    logout_request,
                    |logout_request| {
                        error!("received logout request, currently not supported: {logout_request:#?}");
                        // todo: call logout handling
                    }
                },
                playlist_update = self.playlist_update.next() => unwrap! {
                    playlist_update,
                    match |playlist_update| if let Err(why) = self.handle_playlist_modification(playlist_update) {
                        error!("failed to handle playlist modification: {why}")
                    }
                },
                user_attributes_update = self.user_attributes_update.next() => unwrap! {
                    user_attributes_update,
                    match |attributes| self.handle_user_attributes_update(attributes)
                },
                user_attributes_mutation = self.user_attributes_mutation.next() => unwrap! {
                    user_attributes_mutation,
                    match |attributes| self.handle_user_attributes_mutation(attributes)
                },
                session_update = self.session_update.next() => unwrap! {
                    session_update,
                    match |session_update| self.handle_session_update(session_update)
                },
                cmd = async { commands?.recv().await }, if commands.is_some() && self.connect_established => if let Some(cmd) = cmd {
                    if let Err(e) = self.handle_command(cmd).await {
                        debug!("could not dispatch command: {e}");
                    }
                },
                event = async { player_events?.recv().await }, if player_events.is_some() => if let Some(event) = event {
                    if let Err(e) = self.handle_player_event(event) {
                        error!("could not dispatch player event: {e}");
                    }
                },
                _ = async { sleep(UPDATE_STATE_DELAY).await }, if self.update_state => {
                    self.update_state = false;

                    if let Err(why) = self.notify().await {
                        error!("state update: {why}")
                    }
                },
                _ = async { sleep(VOLUME_UPDATE_DELAY).await }, if self.update_volume => {
                    self.update_volume = false;

                    info!("delayed volume update for all devices: volume is now {}", self.connect_state.device_info().volume);
                    if let Err(why) = self.connect_state.notify_volume_changed(&self.session).await {
                        error!("error updating connect state for volume update: {why}")
                    }

                    // for some reason the web-player does need two separate updates, so that the
                    // position of the current track is retained, other clients also send a state
                    // update before they send the volume update
                    if let Err(why) = self.notify().await {
                        error!("error updating connect state for volume update: {why}")
                    }
                },
                // context resolver handling, the idea/reason behind it the following:
                //
                // when we request a context that has multiple pages (for example an artist)
                // resolving all pages at once can take around ~1-30sec, when we resolve
                // everything at once that would block our main loop for that time
                //
                // to circumvent this behavior, we request each context separately here and
                // finish after we received our last item of a type
                next_context = async {
                    self.context_resolver.get_next_context(|| {
                        // Sending local file URIs to this endpoint results in a Bad Request status.
                        // It's likely appropriate to filter them out anyway; Spotify's backend
                        // has no knowledge about these tracks and so can't do anything with them.
                        self.connect_state.recent_track_uris()
                            .into_iter()
                            .filter(|t| !t.starts_with("spotify:local"))
                            .collect::<Vec<_>>()
                    }).await
                }, if allow_context_resolving && self.context_resolver.has_next() => {
                    let update_state = self.handle_next_context(next_context);
                    if update_state {
                        if let Err(why) = self.notify().await {
                            error!("update after context resolving failed: {why}")
                        }
                    }
                },
                else => break
            }
        }

        if !self.shutdown && self.connect_state.is_active() {
            warn!("unexpected shutdown");
            if let Err(why) = self.handle_disconnect().await {
                error!("error during disconnecting: {why}")
            }
        }

        // this should clear the active session id, leaving an empty state
        if let Err(why) = self.session.spclient().delete_connect_state_request().await {
            error!("error during connect state deletion: {why}")
        };

        self.session.dealer().close().await;
    }

    fn handle_next_context(&mut self, next_context: Result<Context, Error>) -> bool {
        let next_context = match next_context {
            Err(why) => {
                self.context_resolver.mark_next_unavailable(&why);
                self.context_resolver.remove_used_and_invalid();
                error!("{why}");
                return false;
            }
            Ok(ctx) => ctx,
        };

        debug!("handling next context {:?}", next_context.uri);

        match self
            .context_resolver
            .apply_next_context(&mut self.connect_state, next_context)
        {
            Ok(remaining) => {
                if let Some(remaining) = remaining {
                    self.context_resolver.add_list(remaining)
                }
            }
            Err(why) => {
                error!("{why}")
            }
        }

        let update_state = if self
            .context_resolver
            .try_finish(&mut self.connect_state, &mut self.transfer_state)
        {
            self.add_autoplay_resolving_when_required();
            true
        } else {
            false
        };

        // Fire set queue event if context was successfully loaded
        if update_state {
            self.emit_set_queue_event();
            self.refresh_preload();
        }

        self.context_resolver.remove_used_and_invalid();
        update_state
    }

    /// Emit set queue event via PlayerEvent
    fn emit_set_queue_event(&self) {
        if !self.emit_set_queue_events {
            return;
        }

        let state_player = self.connect_state.player();

        let current_track = state_player.track.as_ref().map(|t| QueueTrack {
            uri: t.uri.clone(),
            provider: t.provider.clone(),
        });

        let next_tracks: Vec<_> = state_player
            .next_tracks
            .iter()
            .map(|t| QueueTrack {
                uri: t.uri.clone(),
                provider: t.provider.clone(),
            })
            .collect();

        let prev_tracks: Vec<_> = state_player
            .prev_tracks
            .iter()
            .map(|t| QueueTrack {
                uri: t.uri.clone(),
                provider: t.provider.clone(),
            })
            .collect();

        let context_uri = if is_dj_request(&state_player.context_metadata) {
            crate::REQUESTED_DJ_CONTEXT.to_owned()
        } else {
            self.connect_state.context_uri().clone()
        };

        self.player.emit_set_queue_event_with_session(
            context_uri,
            current_track,
            next_tracks,
            prev_tracks,
            state_player
                .context_metadata
                .get(LEXICON_CONTEXT_URL)
                .cloned(),
            state_player.track.as_ref().map(|track| track.uid.clone()),
        );
    }

    // todo: is the time_delta still necessary?
    fn now_ms(&self) -> i64 {
        let dur = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|err| err.duration());

        dur.as_millis() as i64 + 1000 * self.session.time_delta()
    }

    async fn handle_command(&mut self, cmd: SpircCommand) -> Result<(), Error> {
        trace!("Received SpircCommand::{cmd:?}");
        match cmd {
            SpircCommand::PrewarmDJ => {
                self.context_resolver.prewarm_dj();
                return Ok(());
            }
            SpircCommand::Shutdown => {
                trace!("Received SpircCommand::Shutdown");
                self.handle_pause();
                self.handle_disconnect().await?;
                self.shutdown = true;
                if let Some(rx) = self.commands.as_mut() {
                    rx.close()
                }
            }
            SpircCommand::Transfer(request) if !self.connect_state.is_active() => {
                let device_id = self.session.device_id();
                self.session
                    .spclient()
                    .transfer(device_id, device_id, request.as_ref())
                    .await?;
                return Ok(());
            }
            // Spotify's own host plays inside its Jam's session, which is
            // how the Jam service knows what the host plays.
            // The app repeats it with every Jam check: only a change counts.
            SpircCommand::HostJam(session_id) if session_id == self.jam_host => return Ok(()),
            SpircCommand::HostJam(session_id) => {
                self.jam_host = session_id;
                if self.connect_state.is_active() {
                    return self.notify().await;
                }
                return Ok(());
            }
            SpircCommand::Activate if !self.connect_state.is_active() => {
                trace!("Received SpircCommand::{cmd:?}");
                self.handle_activate(true);
                return self.notify().await;
            }
            SpircCommand::Transfer(..) | SpircCommand::Activate => {
                warn!("SpircCommand::{cmd:?} will be ignored while already active")
            }
            _ if !self.connect_state.is_active() => {
                warn!("SpircCommand::{cmd:?} will be ignored while Not Active")
            }
            SpircCommand::Disconnect { pause } => {
                if pause {
                    self.handle_pause()
                }
                return self.handle_disconnect().await;
            }
            SpircCommand::Play => self.handle_play(),
            SpircCommand::PlayPause => self.handle_play_pause(),
            SpircCommand::Pause => self.handle_pause(),
            SpircCommand::Prev => self.handle_prev(true)?,
            SpircCommand::Next => self.handle_next(None, true)?,
            // Skipping toward a track that is not coming up would never stop.
            SpircCommand::NextTo(uri) if self.connect_state.has_next_track(&uri) => {
                self.handle_next(Some(uri), true)?
            }
            SpircCommand::NextTo(uri) => warn!("{uri} is not coming up; not skipping"),
            SpircCommand::VolumeUp => self.handle_volume_up(),
            SpircCommand::VolumeDown => self.handle_volume_down(),
            SpircCommand::Shuffle(shuffle) => self.handle_shuffle(shuffle)?,
            SpircCommand::Repeat(repeat) => self.handle_repeat_context(repeat)?,
            SpircCommand::RepeatTrack(repeat) => self.handle_repeat_track(repeat),
            SpircCommand::SetPosition(position) => self.handle_seek(position),
            SpircCommand::SetVolume(volume) => self.set_volume(volume),
            SpircCommand::Load(command) => self.handle_load(command, None, None).await?,
            SpircCommand::AddToQueue(uri) => self.handle_add_to_queue(uri).await,
            SpircCommand::ClearQueue => {
                self.connect_state.clear_queue()?;
                self.emit_set_queue_event();
                self.refresh_preload();
            }
            SpircCommand::RemoveFromQueue(uri) => {
                if self.connect_state.remove_from_queue(&uri)? {
                    self.emit_set_queue_event();
                    self.refresh_preload();
                }
            }
        };

        self.notify().await
    }

    fn handle_player_event(&mut self, event: PlayerEvent) -> Result<(), Error> {
        let playing_event = matches!(&event, PlayerEvent::Playing { .. });
        if let PlayerEvent::TrackChanged { audio_item, .. } = event {
            self.connect_state.update_duration(audio_item.duration_ms);
            self.update_state = true;
            if self
                .connect_state
                .current_track(|track| track.uri == audio_item.track_id.to_uri())
            {
                self.handle_preload_next_track();
            }
            return Ok(());
        }

        // update play_request_id
        if let PlayerEvent::PlayRequestIdChanged { play_request_id } = event {
            self.play_request_id = Some(play_request_id);
            return Ok(());
        }

        let is_current_track = matches!(
            (event.get_play_request_id(), self.play_request_id),
            (Some(event_id), Some(current_id)) if event_id == current_id
        );

        // we only process events if the play_request_id matches. If it doesn't, it is
        // an event that belongs to a previous track and only arrives now due to a race
        // condition. In this case we have updated the state already and don't want to
        // mess with it.
        if !is_current_track {
            return Ok(());
        }

        match event {
            PlayerEvent::EndOfTrack { .. } => {
                let next_track = self
                    .connect_state
                    .repeat_track()
                    .then(|| self.connect_state.current_track(|t| t.uri.clone()));

                self.handle_next(next_track, false)?
            }
            PlayerEvent::Loading { .. } => match self.play_status {
                SpircPlayStatus::LoadingPlay { position_ms } => {
                    self.connect_state
                        .update_position(position_ms, self.now_ms());
                    trace!("==> LoadingPlay");
                }
                SpircPlayStatus::LoadingPause { position_ms } => {
                    self.connect_state
                        .update_position(position_ms, self.now_ms());
                    trace!("==> LoadingPause");
                }
                _ => {
                    self.connect_state.update_position(0, self.now_ms());
                    trace!("==> Loading");
                }
            },
            PlayerEvent::Seeked { position_ms, .. } => {
                trace!("==> Seeked");
                self.connect_state
                    .update_position(position_ms, self.now_ms())
            }
            PlayerEvent::Playing { position_ms, .. }
            | PlayerEvent::PositionCorrection { position_ms, .. } => {
                trace!("==> Playing");
                let now = self.now_ms();
                if !self
                    .play_status
                    .player_started(position_ms, now, playing_event)
                {
                    return Ok(());
                }
                self.connect_state
                    .update_position(position_ms, self.now_ms());
            }
            PlayerEvent::Paused {
                position_ms: new_position_ms,
                ..
            } => {
                trace!("==> Paused");
                match self.play_status {
                    SpircPlayStatus::Paused { .. } | SpircPlayStatus::Playing { .. } => {
                        self.connect_state
                            .update_position(new_position_ms, self.now_ms());
                        self.play_status = SpircPlayStatus::Paused {
                            position_ms: new_position_ms,
                            preloading_of_next_track_triggered: false,
                        };
                    }
                    SpircPlayStatus::LoadingPlay { .. } | SpircPlayStatus::LoadingPause { .. } => {
                        self.connect_state
                            .update_position(new_position_ms, self.now_ms());
                        self.play_status = SpircPlayStatus::Paused {
                            position_ms: new_position_ms,
                            preloading_of_next_track_triggered: false,
                        };
                    }
                    _ => return Ok(()),
                }
            }
            PlayerEvent::Stopped { .. } => {
                trace!("==> Stopped");
                match self.play_status {
                    SpircPlayStatus::Stopped => return Ok(()),
                    _ => self.play_status = SpircPlayStatus::Stopped,
                }
            }
            PlayerEvent::TimeToPreloadNextTrack { .. } => {
                self.handle_preload_next_track();
                return Ok(());
            }
            PlayerEvent::Unavailable { track_id, .. } => {
                self.handle_unavailable(&track_id)?;
                if self.connect_state.current_track(|t| &t.uri) == &track_id.to_uri() {
                    self.handle_next(None, false)?
                }
            }
            _ => return Ok(()),
        }

        self.update_state = true;
        Ok(())
    }

    async fn handle_connection_id_update(&mut self, connection_id: String) -> Result<(), Error> {
        trace!("Received connection ID update: {connection_id:?}");
        self.session.set_connection_id(&connection_id);

        let cluster = match self
            .connect_state
            .notify_new_device_appeared(&self.session)
            .await
        {
            Ok(res) => Cluster::parse_from_bytes(&res).ok(),
            Err(why) => {
                error!("{why:?}");
                None
            }
        }
        .ok_or(SpircError::FailedDealerSetup)?;
        self.devices.update(&cluster);

        debug!(
            "successfully put connect state for {} with connection-id {connection_id}",
            self.session.device_id()
        );

        self.connect_established = true;
        if !self.resume_on_startup
            && let Ok(transfer) =
                <TransferState as protobuf::Message>::parse_from_bytes(&cluster.transfer_data)
            && let (Some(session), Some(playback)) = (
                transfer.current_session.as_ref(),
                transfer.playback.as_ref(),
            )
            && let (Some(context), Some(track)) =
                (session.context.as_ref(), playback.current_track.as_ref())
            && let (Some(uri), Some(track_uri)) = (context.uri.as_ref(), track.uri.as_ref())
        {
            self.player.emit_set_queue_event_with_session(
                uri.clone(),
                Some(QueueTrack {
                    uri: track_uri.clone(),
                    provider: "context".into(),
                }),
                Vec::new(),
                Vec::new(),
                context.metadata.get(LEXICON_CONTEXT_URL).cloned(),
                session.current_uid.clone().or_else(|| track.uid.clone()),
            );
        }
        if !automatic_restore_allowed(self.resume_on_startup, &cluster, &self.session.session_id())
        {
            debug!("New connection remains idle until playback is requested");
            return Ok(());
        }

        use protobuf::Message;

        match TransferState::parse_from_bytes(&cluster.transfer_data) {
            Ok(transfer_state) => self.handle_transfer(transfer_state)?,
            Err(why) => error!("failed to take over control: {why}"),
        }

        Ok(())
    }

    fn handle_user_attributes_update(&mut self, update: UserAttributesUpdate) {
        trace!("Received attributes update: {update:#?}");
        let attributes: UserAttributes = update
            .pairs
            .iter()
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect();
        self.session.set_user_attributes(attributes)
    }

    fn handle_user_attributes_mutation(&mut self, mutation: UserAttributesMutation) {
        for attribute in mutation.fields.iter() {
            let key = &attribute.name;

            if key == "autoplay" && self.session.config().autoplay.is_some() {
                trace!("Autoplay override active. Ignoring mutation.");
                continue;
            }

            if let Some(old_value) = self.session.user_data().attributes.get(key) {
                let new_value = match old_value.as_ref() {
                    "0" => "1",
                    "1" => "0",
                    _ => old_value,
                };
                self.session.set_user_attribute(key, new_value);

                trace!("Received attribute mutation, {key} was {old_value} is now {new_value}");

                if key == "filter-explicit-content" && new_value == "1" {
                    self.player
                        .emit_filter_explicit_content_changed_event(matches!(new_value, "1"));
                }

                if key == "autoplay" && old_value != new_value {
                    self.player
                        .emit_auto_play_changed_event(matches!(new_value, "1"));

                    self.add_autoplay_resolving_when_required()
                }
            } else {
                trace!("Received attribute mutation for {key} but key was not found!");
            }
        }
    }

    async fn handle_cluster_update(
        &mut self,
        mut cluster_update: ClusterUpdate,
    ) -> Result<(), Error> {
        let reason = cluster_update.update_reason.enum_value();

        let device_ids = cluster_update.devices_that_changed.join(", ");
        debug!(
            "cluster update: {reason:?} from {device_ids}, active device: {}",
            cluster_update.cluster.active_device_id
        );

        if let Some(cluster) = cluster_update.cluster.take() {
            self.devices.update(&cluster);
            // Which device Spotify says is playing, each time it changes, to
            // diagnose copies on other computers taking over.
            if cluster.active_device_id != self.active_device_logged {
                let mine = cluster.active_device_id == self.session.device_id();
                log::warn!(
                    target: "librespot_connect::devices",
                    "active device now {} ({}), reason {reason:?}, we were active: {}",
                    short_device(&cluster.active_device_id),
                    if mine { "this computer" } else { "another device" },
                    self.connect_state.is_active()
                );
                self.active_device_logged = cluster.active_device_id.clone();
            }
            // Another device playing, such as Spotify's app hosting a Jam:
            // what its state tells the Jam service, to compare with ours.
            if cluster.active_device_id != self.session.device_id()
                && let Some(device) = cluster.device.get(&cluster.active_device_id)
            {
                let summary = format!(
                    "type={:?} software={} caps={}",
                    device.device_type,
                    device
                        .device_software_version
                        .split([' ', '('])
                        .next()
                        .unwrap_or_default(),
                    device
                        .capabilities
                        .as_ref()
                        .map(|caps| protobuf::text_format::print_to_string(caps))
                        .unwrap_or_default()
                );
                if summary != self.jam_device_logged {
                    log::warn!(target: "librespot_connect::jam", "Active device: {summary}");
                    self.jam_device_logged = summary;
                }
            }
            if cluster.active_device_id != self.session.device_id()
                && let Some(state) = cluster.player_state.as_ref()
            {
                let summary = jam_follow::state_summary(state, self.jam_host.as_deref());
                if summary != self.jam_states_logged.0 {
                    log::warn!(target: "librespot_connect::jam", "Active device state: {summary}");
                    self.jam_states_logged.0 = summary;
                }
            }
            let became_inactive = self.connect_state.is_active()
                && cluster.active_device_id != self.session.device_id();
            if became_inactive {
                info!("device became inactive");
                self.handle_disconnect().await?;
                self.handle_stop();
            } else if self.connect_state.is_active() {
                // fixme: workaround fix, because of missing information why it behaves like it does
                //  background: when another device sends a connect-state update, some player's position de-syncs
                //  tried: providing session_id, playback_id, track-metadata "track_player"
                self.update_state = true;
            }
        } else if self.connect_state.is_active() {
            self.connect_state.became_inactive(&self.session).await?;
        }

        Ok(())
    }

    async fn handle_connect_state_request(
        &mut self,
        (request, sender): RequestReply,
    ) -> Result<(), Error> {
        self.connect_state.set_last_command(request.clone());

        debug!(
            "handling: '{}' from {}",
            request.command, request.sent_by_device_id
        );
        if request
            .sent_by_device_id
            .starts_with(jam_follow::SOCIAL_CONNECT_DEVICE_PREFIX)
        {
            log::warn!(target: "librespot_connect::jam", "Jam request {}", request.command);
        }

        let response = match self.handle_request(request).await {
            Ok(_) => Reply::Success,
            Err(why) => {
                error!("failed to handle request: {why}");
                Reply::Failure
            }
        };

        sender.send(response).map_err(Into::into)
    }

    async fn handle_request(&mut self, request: Request) -> Result<(), Error> {
        use Command::*;

        let sent_by = request.sent_by_device_id;
        log::warn!(
            target: "librespot_connect::devices",
            "remote {} from {}, we are active: {}",
            request.command,
            short_device(&sent_by),
            self.connect_state.is_active()
        );
        match request.command {
            // errors and unknown commands
            Transfer(transfer) if transfer.data.is_none() => {
                warn!("transfer endpoint didn't contain any data to transfer");
                Err(SpircError::NoData)?
            }
            Unknown(unknown) => Err(SpircError::UnknownEndpoint(describe_unknown(&unknown)))?,
            // implicit update of the connect_state
            UpdateContext(update_context) => {
                if matches!(update_context.context.uri, Some(ref uri) if uri != self.connect_state.context_uri())
                {
                    debug!(
                        "ignoring context update for <{:?}>, because it isn't the current context <{}>",
                        update_context.context.uri,
                        self.connect_state.context_uri()
                    )
                } else {
                    self.context_resolver.add(ResolveContext::from_context(
                        update_context.context,
                        ContextType::Default,
                        ContextAction::Replace,
                    ))
                }
                return Ok(());
            }
            // modification and update of the connect_state
            Transfer(transfer) => {
                let transfer = transfer.data.expect("by condition checked");
                if jam_follow::is_jam_transfer(
                    &sent_by,
                    transfer.current_session.context.uri.as_deref(),
                ) {
                    // The host is the Jam's source: following its own Jam
                    // left no device holding pauses, so they bounced back.
                    if self.jam_host.is_none() {
                        return self.handle_jam_transfer(transfer).await;
                    }
                    if self.connect_state.is_active() {
                        log::warn!(target: "librespot_connect::jam", "Host ignored its own Jam's transfer");
                        return Ok(());
                    }
                }
                self.handle_transfer(transfer)?;
                return self.notify().await;
            }
            Play(mut play) => {
                if !self.connect_state.is_active() {
                    self.handle_activate(false)
                }

                let context = match play.context.uri {
                    Some(s) => PlayContext::Uri(s),
                    None if !play.context.pages.is_empty() => PlayContext::Tracks(
                        play.context
                            .pages
                            .iter()
                            .cloned()
                            .flat_map(|p| p.tracks)
                            .flat_map(|t| t.uri)
                            .collect(),
                    ),
                    None => Err(SpircError::NoUri("context"))?,
                };

                let context_options = play
                    .options
                    .player_options_override
                    .map(Into::into)
                    .map(LoadContextOptions::Options);

                let fallback_index = play
                    .options
                    .skip_to
                    .as_ref()
                    .and_then(|s| s.track_index)
                    .map(|i| i as usize);

                self.handle_load(
                    LoadRequest {
                        context,
                        options: LoadRequestOptions {
                            page_provider: None,
                            prepared_context: None,
                            session_url: None,
                            relative_offset: 0,
                            start_playing: true,
                            seek_to: play.options.seek_to.unwrap_or_default(),
                            playing_track: play.options.skip_to.and_then(|s| s.try_into().ok()),
                            context_options,
                        },
                    },
                    play.context.pages.pop(),
                    fallback_index,
                )
                .await?;

                self.connect_state.set_origin(play.play_origin)
            }
            Pause(_) => self.handle_pause(),
            SeekTo(seek_to) => {
                // for some reason the position is stored in value, not in position
                trace!("seek to {seek_to:?}");
                self.handle_seek(seek_to.value)
            }
            SetShufflingContext(shuffle) => self.handle_shuffle(shuffle.value)?,
            SetRepeatingContext(repeat_context) => {
                self.handle_repeat_context(repeat_context.value)?
            }
            SetRepeatingTrack(repeat_track) => self.handle_repeat_track(repeat_track.value),
            AddToQueue(add_to_queue) => {
                self.connect_state.add_to_queue(add_to_queue.track, true);
                self.emit_set_queue_event();
                self.refresh_preload();
            }
            SetQueue(set_queue) => {
                self.connect_state.handle_set_queue(set_queue);
                self.emit_set_queue_event();
                self.refresh_preload();
            }
            SetOptions(set_options) => {
                if let Some(repeat_context) = set_options.repeating_context {
                    self.handle_repeat_context(repeat_context)?
                }

                if let Some(repeat_track) = set_options.repeating_track {
                    self.handle_repeat_track(repeat_track)
                }

                let shuffle = set_options.shuffling_context;
                if let Some(shuffle) = shuffle {
                    self.handle_shuffle(shuffle)?;
                }

                // Spotify's Jam service switches the jam mode on for guests
                // as they join and off as the Jam ends for them.
                for (mode, value) in set_options.modes.unwrap_or_default() {
                    if mode == "jam" {
                        let phase = self.devices.jam_phase().after_jam_mode(value == "on");
                        self.set_jam_phase(phase);
                    }
                    self.connect_state.set_mode(mode, value);
                }
            }
            SkipNext(skip_next) => self.handle_next(skip_next.track.map(|t| t.uri), true)?,
            SkipPrev(_) => self.handle_prev(true)?,
            Resume(_) if matches!(self.play_status, SpircPlayStatus::Stopped) => {
                self.load_track(true, 0)?
            }
            Resume(_) => self.handle_play(),
        }

        self.update_state = true;
        Ok(())
    }

    fn handle_transfer(&mut self, mut transfer: TransferState) -> Result<(), Error> {
        let mut ctx_uri = match transfer.current_session.context.uri {
            None => Err(SpircError::NoUri("transfer context"))?,
            // can apparently happen when a state is transferred and was started with "uris" via the api
            Some(ref uri) if uri == "-" || uri.is_empty() => None,
            Some(ref uri) => Some(uri.clone()),
        };

        self.connect_state.reset_context(
            ctx_uri
                .as_deref()
                .map(ResetContext::WhenDifferent)
                .unwrap_or(ResetContext::Completely),
        );

        match self.connect_state.current_track_from_transfer(&transfer) {
            Err(why) => warn!("didn't find initial track: {why}"),
            Ok(track) => {
                debug!("found initial track <{}>", track.uri);
                self.connect_state.set_track(track)
            }
        };

        let autoplay = self.connect_state.current_track(|t| t.is_autoplay());
        if autoplay {
            ctx_uri = ctx_uri.map(|c| c.replace("station:", ""));
        }

        let fallback = self.connect_state.current_track(|t| &t.uri).clone();
        let load_from_context_uri = ctx_uri.is_some();

        match ctx_uri {
            Some(ref uri) => {
                self.context_resolver.add(ResolveContext::from_uri(
                    uri.clone(),
                    &fallback,
                    ContextType::Default,
                    ContextAction::Replace,
                ));
            }
            None => {
                let all_tracks = transfer
                    .current_session
                    .context
                    .pages
                    .iter()
                    .cloned()
                    .flat_map(|p| p.tracks)
                    .collect::<Vec<_>>();

                if !all_tracks.is_empty() {
                    self.load_context_from_tracks(all_tracks)?;
                } else {
                    warn!(
                        "tried to transfer with an invalid state, using fallback as ctx_uri ({fallback})"
                    );
                    ctx_uri = Some(fallback.clone())
                }
            }
        };

        self.handle_activate(true);

        let timestamp = self.now_ms();
        let state = &mut self.connect_state;
        state.handle_initial_transfer(&mut transfer, ctx_uri.clone());

        // adjust active context, so resolve knows for which context it should set up the state
        state.active_context = if autoplay {
            ContextType::Autoplay
        } else {
            ContextType::Default
        };

        // update position if the track continued playing
        let transfer_timestamp = transfer.playback.timestamp.unwrap_or_default();
        let position = match transfer.playback.position_as_of_timestamp {
            Some(position) if transfer.playback.is_paused.unwrap_or_default() => position.into(),
            // update position if the track continued playing
            Some(position) if position > 0 => {
                let time_since_position_update = timestamp - transfer_timestamp;
                i64::from(position) + time_since_position_update
            }
            _ => 0,
        };

        let is_playing = !transfer.playback.is_paused();

        if self.connect_state.current_track(|t| t.is_autoplay()) || autoplay {
            if let Some(ctx_uri) = ctx_uri {
                debug!("currently in autoplay context, async resolving autoplay for {ctx_uri}");
                self.context_resolver.add(ResolveContext::from_uri(
                    ctx_uri,
                    fallback,
                    ContextType::Autoplay,
                    ContextAction::Replace,
                ))
            } else {
                warn!("couldn't resolve autoplay context without a context uri");
            }
        }

        if load_from_context_uri {
            self.transfer_state = Some(transfer);
        } else {
            match self.connect_state.get_context(ContextType::Default) {
                Err(why) => {
                    warn!("continuing transfer in an unknown state. {why}");
                    self.transfer_state = Some(transfer);
                }
                Ok(ctx) => {
                    let idx = ConnectState::find_index_in_context(ctx, |pt| {
                        self.connect_state.current_track(|t| pt.uri == t.uri)
                    })?;
                    self.connect_state.reset_playback_to_position(Some(idx))?;
                }
            }
        }

        self.load_track(is_playing, position.try_into()?)
    }

    /// Spotify's clock now, by the offset measured from the latest cluster,
    /// else the access point's time delta.
    fn server_now_ms(&self) -> i64 {
        match self.devices.server_clock_offset_ms() {
            Some(offset) => SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |now| now.as_millis().min(i64::MAX as u128) as i64)
                .saturating_sub(offset),
            None => self.now_ms(),
        }
    }

    fn set_jam_phase(&mut self, phase: JamPhase) {
        if self.devices.set_jam_phase(phase) {
            info!("Jam: {phase:?}");
            self.player
                .emit_jam_changed_event(phase == JamPhase::Following, None);
        }
    }

    /// Follows one transfer from Spotify's Jam service. It is resent whenever
    /// the host skips, seeks, pauses or resumes, so a resend for the song
    /// already playing keeps it going. The Jam's queue arrives whole with
    /// it; resolving the jam-list URI again isn't needed.
    async fn handle_jam_transfer(&mut self, mut transfer: TransferState) -> Result<(), Error> {
        if !self.devices.jam_phase().follows_transfers() {
            log::warn!(target: "librespot_connect::jam", "Ignoring a transfer from a Jam this device left");
            return Ok(());
        }

        let session = transfer.current_session.mut_or_insert_default();
        let current_uid = session.current_uid.clone();
        let mut context = session.context.take().unwrap_or_default();
        let host_track = jam_follow::host_track(
            transfer.playback.current_track.clone().into_option(),
            current_uid.as_deref(),
            &context.pages,
        );
        let host_uri = host_track.as_ref().and_then(|t| t.uri.clone());
        let host_uid = host_track.as_ref().and_then(|t| t.uid.clone());
        let host = HostPlayback {
            uri: host_uri.as_deref(),
            uid: host_uid.as_deref(),
            timestamp_ms: transfer.playback.timestamp.unwrap_or_default(),
            position_ms: transfer
                .playback
                .position_as_of_timestamp
                .unwrap_or_default()
                .max(0) as u32,
            paused: transfer.playback.is_paused.unwrap_or_default(),
        };

        let stopped = matches!(self.play_status, SpircPlayStatus::Stopped);
        let local_uri = self.connect_state.current_track(|t| t.uri.clone());
        let local_uid = self.connect_state.current_track(|t| t.uid.clone());
        let local = LocalPlayback {
            uri: (!stopped).then_some(local_uri.as_str()),
            uid: (!stopped).then_some(local_uid.as_str()),
            position_ms: self.position(),
            playing: matches!(
                self.play_status,
                SpircPlayStatus::Playing { .. } | SpircPlayStatus::LoadingPlay { .. }
            ),
        };
        let plan = jam_follow::plan(local, host, self.server_now_ms());
        log::warn!(target: "librespot_connect::jam", "Jam transfer plan: {plan:?}");

        self.set_jam_phase(JamPhase::Following);
        if !self.connect_state.is_active() {
            self.handle_activate(true);
        }
        // A Connect transfer still resolving would replace the Jam's queue.
        self.transfer_state = None;

        let context_uri = context
            .uri
            .clone()
            .filter(|uri| !uri.is_empty() && uri != "-")
            .unwrap_or_else(|| self.connect_state.context_uri().clone());
        context.uri = Some(context_uri.clone());
        if context.url.as_deref().is_none_or(str::is_empty) {
            context.url = Some(format!("context://{context_uri}"));
        }
        let has_queue = context.pages.iter().any(|page| !page.tracks.is_empty());
        // Which context a Jam plays in, to compare with the host's.
        log::warn!(
            target: "librespot_connect::jam",
            "Jam transfer context {context_uri}, Jam list of the joined session={}",
            self.jam_session_seen
                .as_ref()
                .map_or("unknown".to_owned(), |id| context_uri
                    .strip_prefix(jam_follow::JAM_LIST_PREFIX)
                    .is_some_and(|list| list == id)
                    .to_string())
        );
        let queue_hash = {
            use std::hash::{DefaultHasher, Hash, Hasher};
            let mut hash = DefaultHasher::new();
            context_uri.hash(&mut hash);
            for track in context.pages.iter().flat_map(|page| &page.tracks) {
                track.uri.hash(&mut hash);
                track.uid.hash(&mut hash);
            }
            hash.finish()
        };

        let loads = matches!(plan, Plan::Load { .. });
        let new_track = match host_track.as_ref() {
            Some(track) if loads => Some(self.connect_state.context_to_provided_track(
                track,
                Some(&context_uri),
                None,
                None,
                None,
            )?),
            _ => None,
        };

        // A playlist started here since replaced the Jam's queue too.
        let queue_changed = has_queue
            && (self.jam_queue_hash != Some(queue_hash)
                || self.connect_state.context_uri() != &context_uri);
        if queue_changed || (loads && !has_queue) {
            self.context_resolver.clear();
            if !has_queue {
                // Only the song: play it as the context until the queue arrives.
                context.pages = vec![ContextPage {
                    tracks: host_track.clone().into_iter().collect(),
                    ..Default::default()
                }];
            }
            self.jam_queue_hash = has_queue.then_some(queue_hash);
            self.connect_state
                .reset_context(ResetContext::WhenDifferent(&context_uri));
            if let Some(next_pages) = self
                .connect_state
                .update_context(context, ContextType::Default)?
            {
                self.context_resolver.add_list(
                    next_pages
                        .into_iter()
                        .map(ResolveContext::append_context)
                        .collect(),
                );
            }
            self.connect_state.set_active_context(ContextType::Default);
        }

        if let Some(track) = new_track {
            self.connect_state.set_track(track);
        }
        if queue_changed || loads {
            self.line_up_jam_queue()?;
            self.emit_set_queue_event();
        }

        match plan {
            Plan::Ignore => {}
            Plan::Continue { seek_ms, pause } => {
                if let Some(position_ms) = seek_ms {
                    self.handle_seek(position_ms);
                }
                match pause {
                    Some(true) => self.handle_pause(),
                    Some(false) => self.handle_play(),
                    None => {}
                }
                if queue_changed {
                    self.refresh_preload();
                }
            }
            Plan::Load {
                position_ms, play, ..
            } => self.load_track(play, position_ms)?,
        }

        self.notify().await
    }

    /// Lines the Jam's queue up after the current song, wherever it sits in it.
    fn line_up_jam_queue(&mut self) -> Result<(), Error> {
        let (uri, uid) = self
            .connect_state
            .current_track(|t| (t.uri.clone(), t.uid.clone()));
        let Ok(context) = self.connect_state.get_context(ContextType::Default) else {
            return Ok(());
        };
        let index = ConnectState::find_index_in_context(context, |t| t.uid == uid)
            .or_else(|_| ConnectState::find_index_in_context(context, |t| t.uri == uri));
        match index {
            Ok(index) => self.connect_state.reset_playback_to_position(Some(index)),
            Err(_) => self.connect_state.reset_next_tracks_to_context_start(),
        }
    }

    async fn handle_disconnect(&mut self) -> Result<(), Error> {
        self.context_resolver.clear();

        self.play_status = SpircPlayStatus::Stopped {};
        self.connect_state
            .update_position_in_relation(self.now_ms());
        self.notify().await?;

        // Finish queued active states before announcing that this device is inactive.
        self.state_updates.flush().await?;

        self.connect_state.became_inactive(&self.session).await?;

        self.player
            .emit_session_disconnected_event(self.session.connection_id(), self.session.username());

        Ok(())
    }

    fn handle_stop(&mut self) {
        self.player.stop();
        self.connect_state.update_position(0, self.now_ms());
        self.connect_state.clear_next_tracks();

        if let Err(why) = self.connect_state.reset_playback_to_position(None) {
            warn!("failed filling up next_track during stopping: {why}")
        }
    }

    /// `modes`: announce shuffle and repeat. A load announces its own once
    /// applied, so announcing the reset ones first would flicker them off.
    fn handle_activate(&mut self, modes: bool) {
        self.connect_state.set_active(true);
        self.player
            .emit_session_connected_event(self.session.connection_id(), self.session.username());
        self.player.emit_session_client_changed_event(
            self.session.client_id(),
            self.session.client_name(),
            self.session.client_brand_name(),
            self.session.client_model_name(),
        );

        self.player
            .emit_volume_changed_event(self.connect_state.device_info().volume as u16);

        self.player
            .emit_auto_play_changed_event(self.session.autoplay());

        self.player
            .emit_filter_explicit_content_changed_event(self.session.filter_explicit_content());

        if modes {
            self.player
                .emit_shuffle_changed_event(self.connect_state.shuffling_context());

            self.player.emit_repeat_changed_event(
                self.connect_state.repeat_context(),
                self.connect_state.repeat_track(),
            );
        }
    }

    async fn handle_load(
        &mut self,
        mut cmd: LoadRequest,
        page: Option<ContextPage>,
        fallback_index: Option<usize>,
    ) -> Result<(), Error> {
        self.connect_state
            .reset_context(if cmd.prepared_context.is_some() {
                ResetContext::Completely
            } else if let PlayContext::Uri(ref uri) = cmd.context {
                ResetContext::WhenDifferent(uri)
            } else {
                ResetContext::Completely
            });

        // A new context keeps transport preferences unless its caller overrides them.
        // Materialize inherited options so the queue takes the shuffle path below.
        if cmd.options.context_options.is_none() {
            cmd.options.context_options = Some(LoadContextOptions::Options(crate::Options {
                shuffle: self.connect_state.shuffling_context(),
                repeat: self.connect_state.repeat_context(),
                repeat_track: self.connect_state.repeat_track(),
            }));
        }
        // Resolve the selected row in source order before shuffling the new context.
        self.connect_state.reset_options();
        self.context_resolver
            .set_page_provider(cmd.page_provider.clone());

        let autoplay = matches!(cmd.context_options, Some(LoadContextOptions::Autoplay));
        // Playing from the top rather than a chosen track starts a context afresh.
        // Clients start one by picking its first track, so that counts too, unless
        // the context is already playing and the listener picked it from there.
        let fresh = match cmd.options.playing_track {
            None => true,
            Some(PlayingTrack::Index(0)) => !matches!(
                &cmd.context,
                PlayContext::Uri(uri) if uri == self.connect_state.context_uri()
            ),
            Some(_) => false,
        };
        let resumed = match cmd.context {
            PlayContext::Uri(uri) => {
                self.load_context_from_uri(uri, page.as_ref(), autoplay, fresh, &cmd.options)
                    .await?
            }
            PlayContext::Tracks(tracks) => {
                self.load_context_from_tracks(tracks)?;
                true
            }
        };

        let mut cmd_options = cmd.options;
        if !resumed {
            cmd_options.playing_track = None;
            cmd_options.seek_to = 0;
            cmd_options.relative_offset = 0;
        }

        self.connect_state.set_active_context(ContextType::Default);
        // Songs the host starts are the Jam's, as Spotify's app marks them.
        if self.jam_host.is_some() {
            self.connect_state.set_origin(PlayOrigin {
                feature_identifier: "jam".into(),
                ..Default::default()
            });
        }

        // for play commands with skip by uid, the context of the command contains
        // tracks with uri and uid, so we merge the new context with the resolved/existing context
        self.connect_state.merge_context(page);

        // load here, so that we clear the queue only after we definitely retrieved a new context
        self.connect_state.clear_next_tracks();
        self.connect_state.clear_restrictions();

        debug!("play track <{:?}>", cmd_options.playing_track);

        let mut index = match cmd_options.playing_track {
            None => None,
            Some(ref playing_track) => Some(match playing_track {
                PlayingTrack::Index(i) => Ok(*i as usize),
                PlayingTrack::Uri(uri) => {
                    let ctx = self.connect_state.get_context(ContextType::Default)?;
                    ConnectState::find_index_in_context(ctx, |t| &t.uri == uri)
                }
                PlayingTrack::Uid(uid) => {
                    let ctx = self.connect_state.get_context(ContextType::Default)?;
                    ConnectState::find_index_in_context(ctx, |t| &t.uid == uid)
                }
                PlayingTrack::UidOrUri { uid, uri } => {
                    let ctx = self.connect_state.get_context(ContextType::Default)?;
                    ConnectState::find_index_in_context(ctx, |track| &track.uid == uid).or_else(
                        |_| ConnectState::find_index_in_context(ctx, |track| &track.uri == uri),
                    )
                }
            }),
        }
        .map(|i| {
            i.unwrap_or_else(|why| {
                cmd_options.seek_to = 0;
                cmd_options.relative_offset = 0;
                warn!(
                    "Failed to resolve index by {:?}, using fallback index: {:?} (Error: {why})",
                    cmd_options.playing_track, fallback_index
                );
                fallback_index.unwrap_or_default()
            })
        });
        if cmd_options.relative_offset != 0 {
            let length = self
                .connect_state
                .get_context(ContextType::Default)?
                .tracks
                .len();
            index = Some(
                index
                    .unwrap_or_default()
                    .saturating_add_signed(cmd_options.relative_offset as isize)
                    .min(length.saturating_sub(1)),
            );
        }

        if let Some(LoadContextOptions::Options(ref options)) = cmd_options.context_options {
            debug!(
                "loading with shuffle: <{}>, repeat track: <{}> context: <{}>",
                options.shuffle, options.repeat, options.repeat_track
            );

            self.connect_state.set_shuffle(options.shuffle);
            self.connect_state.set_repeat_context(options.repeat);
            self.connect_state.set_repeat_track(options.repeat_track);
        }
        self.player
            .emit_shuffle_changed_event(self.connect_state.shuffling_context());
        self.player.emit_repeat_changed_event(
            self.connect_state.repeat_context(),
            self.connect_state.repeat_track(),
        );

        if matches!(cmd_options.context_options, Some(LoadContextOptions::Options(ref o)) if o.shuffle)
        {
            if let Some(index) = index {
                self.connect_state.set_current_track(index)?;
            } else {
                self.connect_state.set_current_track_random()?;
            }

            if self.context_resolver.has_next() {
                self.connect_state.update_queue_revision()
            } else {
                self.connect_state.shuffle_new()?;
                self.add_autoplay_resolving_when_required();
            }
        } else {
            self.connect_state
                .set_current_track(index.unwrap_or_default())?;
            self.connect_state.reset_playback_to_position(index)?;
            self.add_autoplay_resolving_when_required();
        }

        if self.connect_state.current_track(MessageField::is_some) {
            self.load_track(cmd_options.start_playing, cmd_options.seek_to)?;
        } else {
            info!("No active track, stopping");
            self.handle_stop()
        }

        Ok(())
    }

    async fn load_context_from_uri(
        &mut self,
        context_uri: String,
        page: Option<&ContextPage>,
        autoplay: bool,
        fresh: bool,
        options: &LoadRequestOptions,
    ) -> Result<bool, Error> {
        if !self.connect_state.is_active() {
            self.handle_activate(false);
        }

        let update_context = if autoplay {
            ContextType::Autoplay
        } else {
            ContextType::Default
        };

        self.connect_state.set_active_context(update_context);

        let saved_context = if let Some(context) = options.prepared_context.as_ref() {
            self.context_resolver.clear_prepared_dj();
            Some((**context).clone())
        } else if let Some(url) = options.session_url.as_deref() {
            self.session.spclient().get_context_url(url).await.ok()
        } else {
            None
        };
        let resumed = options.session_url.is_none() || saved_context.is_some();
        let fresh = fresh || !resumed;

        let fallback = match page {
            // check that the uri is valid or the page has a valid uri that can be used
            Some(page) => match ConnectState::find_valid_uri(Some(&context_uri), Some(page)) {
                Some(ctx_uri) => ctx_uri,
                None => return Err(SpircError::InvalidUri(context_uri).into()),
            },
            // when there is no page, the uri should be valid
            None => &context_uri,
        };

        let current_context_uri = self.connect_state.context_uri();
        // A session like DJ's moves on to a new set each time it starts afresh.
        let new_set = fresh
            && self
                .connect_state
                .get_context(update_context)
                .is_ok_and(|ctx| {
                    ctx.metadata.contains_key(LEXICON_CONTEXT_URL) || is_dj_request(&ctx.metadata)
                });

        // The loaded part of the playing context may not hold the track
        // another client picked; resolve it again to page through.
        let requested_track_missing = options.playing_track.as_ref().is_some_and(|track| {
            self.connect_state
                .get_context(update_context)
                .is_ok_and(|ctx| match track {
                    PlayingTrack::Index(index) => *index as usize >= ctx.tracks.len(),
                    PlayingTrack::Uri(uri) => !ctx.tracks.iter().any(|t| &t.uri == uri),
                    PlayingTrack::Uid(uid) => !ctx.tracks.iter().any(|t| &t.uid == uid),
                    PlayingTrack::UidOrUri { uid, uri } => {
                        !ctx.tracks.iter().any(|t| &t.uid == uid || &t.uri == uri)
                    }
                })
        });
        if saved_context.is_none()
            && resumed
            && current_context_uri == &context_uri
            && fallback == context_uri
            && !new_set
            && !requested_track_missing
        {
            debug!("context <{current_context_uri}> didn't change, no resolving required")
        } else {
            debug!("resolving context for load command");
            self.context_resolver.clear();
            if let Some(mut context) = saved_context {
                context.uri = Some(context_uri.clone());
                if let Some(url) = options.session_url.as_ref() {
                    context
                        .metadata
                        .insert(LEXICON_CONTEXT_URL.into(), url.clone());
                }
                self.context_resolver.add(ResolveContext::from_prepared(
                    context,
                    update_context,
                    ContextAction::Replace,
                ));
            } else {
                self.context_resolver.add(
                    ResolveContext::from_uri(
                        &context_uri,
                        fallback,
                        update_context,
                        ContextAction::Replace,
                    )
                    .starting_fresh(fresh),
                );
            }
            let mut context = self.context_resolver.get_next_context(Vec::new).await?;
            if resumed && let Some(track) = options.playing_track.as_ref() {
                let offset = options.relative_offset.max(0) as usize;
                for _ in 0..MAX_REQUESTED_TRACK_PAGES {
                    if !requested_track_unloaded(&context, track, offset) {
                        break;
                    }
                    let Some(url) = context
                        .pages
                        .last()
                        .and_then(|page| page.next_page_url.as_ref())
                        .filter(|url| !url.is_empty())
                    else {
                        break;
                    };
                    let page = self.session.spclient().get_context_page(url).await?;
                    if page.tracks.is_empty() {
                        break;
                    }
                    context.pages.push(page);
                }
            }
            self.handle_next_context(Ok(context));
        }

        Ok(resumed)
    }

    fn load_context_from_tracks(&mut self, tracks: impl Into<ContextPage>) -> Result<(), Error> {
        const WEB_API_URI: &str = "spotify:web-api";
        let ctx = Context {
            // by providing values for uri/url the player in the official client's isn't frozen
            uri: Some(WEB_API_URI.into()),
            url: Some(format!("context://{WEB_API_URI}")),
            pages: vec![tracks.into()],
            ..Default::default()
        };

        let _ = self
            .connect_state
            .update_context(ctx, ContextType::Default)?;

        self.emit_set_queue_event();

        Ok(())
    }

    fn handle_play(&mut self) {
        match self.play_status {
            SpircPlayStatus::Paused {
                position_ms,
                preloading_of_next_track_triggered,
            } => {
                self.player.play();
                self.connect_state
                    .update_position(position_ms, self.now_ms());
                self.play_status = SpircPlayStatus::Playing {
                    nominal_start_time: self.now_ms() - position_ms as i64,
                    preloading_of_next_track_triggered,
                };
            }
            SpircPlayStatus::LoadingPause { position_ms } => {
                self.player.play();
                self.play_status = SpircPlayStatus::LoadingPlay { position_ms };
            }
            _ => return,
        }

        // Synchronize the volume from the mixer. This is useful on
        // systems that can switch sources from and back to librespot.
        let current_volume = self.mixer.volume();
        self.set_volume(current_volume);
    }

    fn handle_play_pause(&mut self) {
        match self.play_status {
            SpircPlayStatus::Paused { .. } | SpircPlayStatus::LoadingPause { .. } => {
                self.handle_play()
            }
            SpircPlayStatus::Playing { .. } | SpircPlayStatus::LoadingPlay { .. } => {
                self.handle_pause()
            }
            _ => (),
        }
    }

    fn handle_pause(&mut self) {
        match self.play_status {
            SpircPlayStatus::Playing {
                nominal_start_time,
                preloading_of_next_track_triggered,
            } => {
                self.player.pause();
                let position_ms = (self.now_ms() - nominal_start_time) as u32;
                self.connect_state
                    .update_position(position_ms, self.now_ms());
                self.play_status = SpircPlayStatus::Paused {
                    position_ms,
                    preloading_of_next_track_triggered,
                };
            }
            SpircPlayStatus::LoadingPlay { position_ms } => {
                self.player.pause();
                self.play_status = SpircPlayStatus::LoadingPause { position_ms };
            }
            _ => (),
        }
    }

    fn handle_seek(&mut self, position_ms: u32) {
        let duration = self.connect_state.player().duration;
        if i64::from(position_ms) > duration {
            warn!("tried to seek to {position_ms}ms of {duration}ms");
            return;
        }

        self.connect_state
            .update_position(position_ms, self.now_ms());
        self.player.seek(position_ms);
        let now = self.now_ms();
        match self.play_status {
            SpircPlayStatus::Stopped => (),
            SpircPlayStatus::LoadingPause {
                position_ms: ref mut position,
            }
            | SpircPlayStatus::LoadingPlay {
                position_ms: ref mut position,
            }
            | SpircPlayStatus::Paused {
                position_ms: ref mut position,
                ..
            } => *position = position_ms,
            SpircPlayStatus::Playing {
                ref mut nominal_start_time,
                ..
            } => *nominal_start_time = now - position_ms as i64,
        };
    }

    fn handle_shuffle(&mut self, shuffle: bool) -> Result<(), Error> {
        self.connect_state.handle_shuffle(shuffle)?;
        self.player
            .emit_shuffle_changed_event(self.connect_state.shuffling_context());
        self.emit_set_queue_event();
        self.refresh_preload();
        Ok(())
    }

    fn handle_repeat_context(&mut self, repeat: bool) -> Result<(), Error> {
        self.player
            .emit_repeat_changed_event(repeat, self.connect_state.repeat_track());
        self.connect_state.handle_set_repeat_context(repeat)
    }

    fn handle_repeat_track(&mut self, repeat: bool) {
        self.player
            .emit_repeat_changed_event(self.connect_state.repeat_context(), repeat);
        self.connect_state.set_repeat_track(repeat);
    }

    async fn handle_add_to_queue(&mut self, uri: SpotifyUri) {
        let track_uris: Vec<String> = match uri {
            SpotifyUri::Track { .. } | SpotifyUri::Episode { .. } | SpotifyUri::Local { .. } => {
                vec![uri.to_uri()]
            }
            SpotifyUri::Album { .. } | SpotifyUri::Playlist { .. } => {
                match self.session.spclient().get_context(&uri.to_uri()).await {
                    Ok(context) => context
                        .pages
                        .iter()
                        .flat_map(|page| page.tracks.iter())
                        .filter_map(|track| track.uri.clone())
                        .collect(),
                    Err(e) => {
                        error!("failed to resolve context for {}: {e}", uri.item_type());
                        return;
                    }
                }
            }
            _ => return,
        };

        for track_uri in track_uris {
            let track = ProvidedTrack {
                uri: track_uri.clone(),
                ..Default::default()
            };
            self.connect_state.add_to_queue(track, true);
        }
        self.emit_set_queue_event();
        self.refresh_preload();
    }

    /// The next track was preloaded when this one started. A reshuffle, a
    /// resolved context or queue edit can pick another, so skipping would
    /// load it cold; preload the new pick instead.
    fn refresh_preload(&mut self) {
        if matches!(
            self.play_status,
            SpircPlayStatus::Playing { .. } | SpircPlayStatus::Paused { .. }
        ) {
            self.handle_preload_next_track();
        }
    }

    fn handle_preload_next_track(&mut self) {
        // Requests the player thread to preload the next track
        match self.play_status {
            SpircPlayStatus::Paused {
                ref mut preloading_of_next_track_triggered,
                ..
            }
            | SpircPlayStatus::Playing {
                ref mut preloading_of_next_track_triggered,
                ..
            } => {
                *preloading_of_next_track_triggered = true;
            }
            _ => (),
        }

        if let Some((track_id, narration)) = self.connect_state.preview_next_track() {
            let narration = self.narrated.unheard(&track_id.to_uri(), narration);
            self.player.set_narration(track_id.clone(), narration);
            self.player.preload(track_id);
        } else {
            // Nothing follows yet, for example when fetching autoplay failed
            // earlier; ask again before the song ends rather than stopping.
            self.add_autoplay_resolving_when_required();
        }
    }

    // Mark unavailable tracks so we can skip them later
    fn handle_unavailable(&mut self, track_id: &SpotifyUri) -> Result<(), Error> {
        self.connect_state.mark_unavailable(track_id)?;
        self.handle_preload_next_track();

        Ok(())
    }

    fn add_autoplay_resolving_when_required(&mut self) {
        let running_low = !self
            .connect_state
            .has_next_tracks(Some(CONTEXT_FETCH_THRESHOLD))
            && !self.connect_state.context_uri().is_empty();

        if !running_low {
            return;
        }

        // A context that streams in pages, like DJ, continues before autoplay takes over.
        if let Some(next_page) = self.connect_state.take_next_page_url() {
            self.context_resolver
                .add(ResolveContext::append_context(next_page));
            return;
        }

        if !self.session.autoplay() {
            return;
        }

        let current_context = self.connect_state.context_uri();
        let fallback = self.connect_state.current_track(|t| &t.uri);

        let has_tracks = self
            .connect_state
            .get_context(ContextType::Autoplay)
            .map(|c| !c.tracks.is_empty())
            .unwrap_or_default();

        let resolve = ResolveContext::from_uri(
            current_context,
            fallback,
            ContextType::Autoplay,
            if has_tracks {
                ContextAction::Append
            } else {
                ContextAction::Replace
            },
        );

        self.context_resolver.add(resolve);
    }

    /// `play`: a skip the listener asked for starts the next song even when
    /// paused, as Spotify's clients do; one that follows a song ending or
    /// failing keeps the current play state.
    fn handle_next(&mut self, track_uri: Option<String>, play: bool) -> Result<(), Error> {
        let continue_playing = play || self.connect_state.is_playing();

        let current_uri = self.connect_state.current_track(|t| &t.uri);
        let mut has_next_track =
            matches!(track_uri, Some(ref track_uri) if current_uri == track_uri);

        if !has_next_track {
            has_next_track = loop {
                let index = self.connect_state.next_track()?;

                let current_uri = self.connect_state.current_track(|t| &t.uri);
                if matches!(track_uri, Some(ref track_uri) if current_uri != track_uri) {
                    continue;
                } else {
                    break index.is_some();
                }
            };
        };

        if has_next_track {
            self.add_autoplay_resolving_when_required();
            self.load_track(continue_playing, 0)
        } else {
            info!("Not playing next track because there are no more tracks left in queue.");
            self.handle_stop();
            Ok(())
        }
    }

    fn handle_prev(&mut self, play: bool) -> Result<(), Error> {
        // Previous behaves differently based on the position
        // Under 3s it goes to the previous song (starts playing when the
        // listener asked, even if paused)
        // Over 3s it seeks to zero (retains previous play status)
        if self.position() < 3000 {
            let repeat_context = self.connect_state.repeat_context();
            match self.connect_state.prev_track()? {
                None if repeat_context => self.connect_state.reset_playback_to_position(None)?,
                None => {
                    self.connect_state.reset_playback_to_position(None)?;
                    self.handle_stop()
                }
                Some(_) => self.load_track(play || self.connect_state.is_playing(), 0)?,
            }
        } else {
            self.handle_seek(0);
        }

        Ok(())
    }

    fn handle_volume_up(&mut self) {
        let volume = (self.connect_state.device_info().volume as u16)
            .saturating_add(self.connect_state.volume_step_size);

        self.set_volume(volume);
    }

    fn handle_volume_down(&mut self) {
        let volume = (self.connect_state.device_info().volume as u16)
            .saturating_sub(self.connect_state.volume_step_size);

        self.set_volume(volume);
    }

    fn handle_playlist_modification(
        &mut self,
        playlist_modification_info: PlaylistModificationInfo,
    ) -> Result<(), Error> {
        let uri = playlist_modification_info
            .uri
            .ok_or(SpircError::NoUri("playlist modification"))?;
        let uri = String::from_utf8(uri)?;

        if self.connect_state.context_uri() != &uri {
            debug!(
                "ignoring playlist modification update for playlist <{uri}>, because it isn't the current context"
            );
            return Ok(());
        }

        debug!("playlist modification for current context: {uri}");
        self.context_resolver.add(ResolveContext::from_uri(
            uri,
            self.connect_state.current_track(|t| &t.uri),
            ContextType::Default,
            ContextAction::Replace,
        ));

        Ok(())
    }

    fn handle_session_update(&mut self, session_update: FallbackWrapper<SessionUpdate>) {
        // we know that this enum value isn't present in our current proto definitions, by that
        // the json parsing fails because the enum isn't known as proto representation
        const WBC: &str = "WIFI_BROADCAST_CHANGED";

        let mut session_update = match session_update {
            FallbackWrapper::Inner(update) => update,
            FallbackWrapper::Fallback(value) => {
                let fallback_inner = value.to_string();
                if fallback_inner.contains(WBC) {
                    log::debug!("Received SessionUpdate::{WBC}");
                } else {
                    log::warn!("SessionUpdate couldn't be parse correctly: {value:?}");
                }
                return;
            }
        };

        let reason = session_update.reason.enum_value();
        if let Some(session) = session_update.session.as_ref() {
            log::warn!(
                target: "librespot_connect::jam",
                "Jam update {reason:?}: host device here={}, hosted session={}",
                session
                    .host_active_device_id
                    .as_deref()
                    .map_or("unnamed".to_owned(), |device| {
                        (device == self.session.device_id()).to_string()
                    }),
                self.jam_host
                    .as_deref()
                    .map_or("none".to_owned(), |id| (id == session.session_id).to_string()),
            );
        }

        // Leaving, removal and a Jam's end stop this device following it;
        // the song playing keeps going, as in Spotify's own app.
        if let Ok(reason) = reason
            && reason != SessionUpdateReason::UNKNOWN_UPDATE_TYPE
        {
            let phase = self.devices.jam_phase().after_session_update(reason);
            if phase == JamPhase::Left {
                self.jam_host = None;
            }
            let guests = session_update
                .session
                .as_ref()
                .is_some_and(|session| session.session_members.len() > 1)
                && phase != JamPhase::Left;
            if guests != self.jam_has_guests {
                self.jam_has_guests = guests;
                self.update_state = true;
            }
            if self.devices.set_jam_phase(phase) {
                info!("Jam: {phase:?} after {reason:?}");
            }
            // Members, hosts and the Jam itself change: the app shows them,
            // from the session in the report as Spotify's own app does.
            let jam = session_update.session.as_ref().map(jam_session_json);
            self.player
                .emit_jam_changed_event(phase == JamPhase::Following, jam);
        }

        if let Some(session) = session_update.session.as_ref()
            && self
                .jam_session_seen
                .as_ref()
                .is_none_or(|id| id != &session.session_id)
        {
            self.jam_session_seen = Some(session.session_id.clone());
        }

        let mut session = match session_update.session.take() {
            None => return,
            Some(session) => session,
        };

        let active_device = session.host_active_device_id.take();
        if matches!(active_device, Some(ref device) if device == self.session.device_id()) {
            // Spotify's own host keeps its playback session ID in a Jam;
            // taking the Jam's in its place hid the host's playback from
            // the Jam service, so guests never followed it.
            info!("session update: <{reason:?}> for self");
        } else {
            debug!("session update: <{reason:?}> from active session host: <{active_device:?}>");
        }

        // this seems to be used for jams or handling the current session_id
        //
        // handling this event was intended to keep the playback when other clients (primarily
        // mobile) connects, otherwise they would steel the current playback when there was no
        // session_id provided on the initial PutStateReason::NEW_DEVICE state update
        //
        // by generating an initial session_id from the get-go we prevent that behavior and
        // currently don't need to handle this event, might still be useful for later "jam" support
    }

    fn position(&mut self) -> u32 {
        match self.play_status {
            SpircPlayStatus::Stopped => 0,
            SpircPlayStatus::LoadingPlay { position_ms }
            | SpircPlayStatus::LoadingPause { position_ms }
            | SpircPlayStatus::Paused { position_ms, .. } => position_ms,
            SpircPlayStatus::Playing {
                nominal_start_time, ..
            } => (self.now_ms() - nominal_start_time) as u32,
        }
    }

    fn load_track(&mut self, start_playing: bool, position_ms: u32) -> Result<(), Error> {
        if self.connect_state.current_track(MessageField::is_none) {
            debug!("current track is none, stopping playback");
            self.handle_stop();
            return Ok(());
        }

        let current_uri = self.connect_state.current_track(|t| &t.uri);
        let id = SpotifyUri::from_uri(current_uri)?;
        self.emit_set_queue_event();
        let narration = self
            .narrated
            .unheard(current_uri, self.connect_state.current_narration());
        self.narrated.spoken(current_uri, narration.as_ref());
        self.player.set_narration(id.clone(), narration);
        self.player.load(id, start_playing, position_ms);
        // Each playback of a song has its own ID in Spotify's apps; Jam
        // guests follow the host by it.
        self.connect_state
            .set_playback_id(uuid::Uuid::new_v4().simple().to_string());

        self.connect_state
            .update_position(position_ms, self.now_ms());
        if start_playing {
            self.play_status = SpircPlayStatus::LoadingPlay { position_ms };
        } else {
            self.play_status = SpircPlayStatus::LoadingPause { position_ms };
        }
        self.connect_state.set_status(&self.play_status);

        Ok(())
    }

    /// Marks this device's state with its place in a Jam, as Spotify's app
    /// does: the Jam mode, on while following a Jam or hosting one with
    /// guests, and the hosted Jam named in the context's metadata from the
    /// moment it starts. Guests keep the Jam name Spotify sends them.
    fn mark_jam_state(&mut self) {
        let following = self.devices.jam_phase() == JamPhase::Following;
        let jam_on = following || (self.jam_host.is_some() && self.jam_has_guests);
        self.connect_state
            .set_mode("jam".into(), if jam_on { "on" } else { "off" }.into());
        if self.jam_host.is_some() || !following {
            self.connect_state
                .set_context_metadata(JAM_SESSION_KEY, self.jam_host.clone());
        }
    }

    async fn notify(&mut self) -> Result<(), Error> {
        self.connect_state.set_status(&self.play_status);

        if self.connect_state.is_playing() {
            self.connect_state
                .update_position_in_relation(self.now_ms());
        }

        self.connect_state.set_now(self.now_ms() as u64);

        self.mark_jam_state();
        if self.jam_host.is_some() || self.devices.jam_phase() == JamPhase::Following {
            let summary =
                jam_follow::state_summary(self.connect_state.player(), self.jam_host.as_deref());
            if summary != self.jam_states_logged.1 {
                log::warn!(target: "librespot_connect::jam", "This device's Jam state: {summary}");
                self.jam_states_logged.1 = summary;
            }
        }
        let update = self.connect_state.send_state(&self.session);
        self.state_updates
            .push(async move { update.await.map(|_| ()) });
        Ok(())
    }

    fn set_volume(&mut self, volume: u16) {
        debug!("SpircTask::set_volume({volume})");

        let old_volume = self.connect_state.device_info().volume;
        let new_volume = volume as u32;
        if old_volume != new_volume || self.mixer.volume() != volume {
            self.update_volume = true;

            self.connect_state.set_volume(new_volume);
            self.mixer.set_volume(volume);
            if let Some(cache) = self.session.cache() {
                cache.save_volume(volume)
            }
            if self.connect_state.is_active() {
                self.player.emit_volume_changed_event(volume);
            }
        }
    }
}

impl Drop for SpircTask {
    fn drop(&mut self) {
        debug!("drop Spirc[{}]", self.spirc_id);
    }
}

#[cfg(test)]
mod shuffle_tests {
    use super::*;
    use crate::playback::{
        audio_backend::{Sink, SinkResult},
        convert::Converter,
        decoder::AudioPacket,
        mixer::softmixer::SoftMixer,
    };

    struct SilentSink;
    impl Sink for SilentSink {
        fn write(&mut self, _: AudioPacket, _: &mut Converter) -> SinkResult<()> {
            Ok(())
        }
    }

    fn tracks() -> Vec<String> {
        (0..16)
            .map(|index| format!("spotify:track:{index:022}"))
            .collect()
    }

    fn task() -> SpircTask {
        let session = Session::new(Default::default(), None);
        let mixer = Arc::new(SoftMixer::open(Default::default()).unwrap());
        let player = Player::new(
            Default::default(),
            session.clone(),
            mixer.get_soft_volume(),
            || Box::new(SilentSink),
        );
        let player_events = Some(player.get_player_event_channel());
        let mut connect_state = ConnectState::new(Default::default(), &session);
        connect_state
            .update_context(
                Context {
                    uri: Some("spotify:playlist:0123456789abcdefghijkl".into()),
                    pages: vec![tracks().into()],
                    ..Default::default()
                },
                ContextType::Default,
            )
            .unwrap();
        connect_state.set_current_track(0).unwrap();
        connect_state.reset_playback_to_position(Some(0)).unwrap();
        SpircTask {
            devices: Default::default(),
            state_updates: Default::default(),
            narrated: Default::default(),
            resume_on_startup: false,
            player,
            mixer,
            connect_state,
            connect_established: false,
            play_request_id: None,
            play_status: SpircPlayStatus::Stopped,
            connection_id_update: Box::pin(futures_util::stream::pending()),
            connect_state_update: Box::pin(futures_util::stream::pending()),
            connect_state_volume_update: Box::pin(futures_util::stream::pending()),
            connect_state_logout_request: Box::pin(futures_util::stream::pending()),
            playlist_update: Box::pin(futures_util::stream::pending()),
            session_update: Box::pin(futures_util::stream::pending()),
            connect_state_command: Box::pin(futures_util::stream::pending()),
            user_attributes_update: Box::pin(futures_util::stream::pending()),
            user_attributes_mutation: Box::pin(futures_util::stream::pending()),
            commands: None,
            player_events,
            context_resolver: ContextResolver::new(session.clone()),
            emit_set_queue_events: true,
            shutdown: false,
            session,
            transfer_state: None,
            jam_queue_hash: None,
            jam_host: None,
            jam_states_logged: Default::default(),
            jam_session_seen: None,
            jam_has_guests: false,
            jam_device_logged: String::new(),
            active_device_logged: String::new(),
            update_volume: false,
            update_state: false,
            spirc_id: 0,
        }
    }

    #[tokio::test]
    async fn shuffle_toggle_publishes_the_actual_upcoming_queue() {
        let mut task = task();
        for enabled in [true, false] {
            task.handle_shuffle(enabled).unwrap();
            let expected: Vec<_> = task
                .connect_state
                .player()
                .next_tracks
                .iter()
                .map(|track| track.uri.clone())
                .collect();
            let published = tokio::time::timeout(Duration::from_millis(500), async {
                while let Some(event) = task.player_events.as_mut().unwrap().recv().await {
                    if let PlayerEvent::SetQueue { next_tracks, .. } = event {
                        return next_tracks
                            .into_iter()
                            .map(|track| track.uri)
                            .collect::<Vec<_>>();
                    }
                }
                panic!("Player event channel closed");
            })
            .await
            .expect("Shuffle changed playback but never published its new queue");
            assert_eq!(published, expected);
            assert_eq!(
                task.connect_state.current_track(|track| track.uri.clone()),
                tracks()[0]
            );
        }
    }

    #[tokio::test]
    async fn shuffle_survives_loading_another_playlist() {
        let mut task = task();
        task.connect_state.handle_shuffle(true).unwrap();
        let uri = "spotify:playlist:1123456789abcdefghijkl";
        let context = Context {
            uri: Some(uri.into()),
            pages: vec![tracks().into()],
            ..Default::default()
        };
        task.handle_load(
            LoadRequest::from_context_uri(
                uri.into(),
                LoadRequestOptions {
                    prepared_context: Some(Box::new(context)),
                    playing_track: Some(PlayingTrack::Index(3)),
                    ..Default::default()
                },
            ),
            None,
            None,
        )
        .await
        .unwrap();
        assert!(
            task.connect_state.shuffling_context(),
            "Loading a playlist cleared the enabled shuffle setting"
        );
        assert_eq!(
            task.connect_state.current_track(|track| track.uri.clone()),
            tracks()[3]
        );
        assert!(
            task.connect_state
                .get_context(ContextType::Default)
                .unwrap()
                .metadata
                .contains_key("shuffle_seed")
        );
        task.handle_shuffle(false).unwrap();
        let upcoming: Vec<_> = task
            .connect_state
            .player()
            .next_tracks
            .iter()
            .map(|track| track.uri.clone())
            .collect();
        assert_eq!(upcoming, tracks()[4..]);
        task.player.stop();
    }

    const JAM: &str = "spotify:list:jam-list:0123abcd";

    /// A synthetic transfer shaped like those Spotify's Jam service sends:
    /// the whole Jam queue, the host's song and where the host is in it.
    fn jam_transfer(current: usize, position_ms: i32, paused: bool, timestamp: i64) -> Request {
        use crate::protocol::{context_page::ContextPage, context_track::ContextTrack};
        let track = |index: usize| ContextTrack {
            uri: Some(tracks()[index].clone()),
            uid: Some(format!("u{index}")),
            ..Default::default()
        };
        let mut state = TransferState::new();
        let playback = state.playback.mut_or_insert_default();
        playback.timestamp = Some(timestamp);
        playback.position_as_of_timestamp = Some(position_ms);
        playback.is_paused = Some(paused);
        playback.current_track = MessageField::some(track(current));
        state.current_session.mut_or_insert_default().context = MessageField::some(Context {
            uri: Some(JAM.into()),
            pages: vec![ContextPage {
                tracks: (0..tracks().len()).map(track).collect(),
                ..Default::default()
            }],
            ..Default::default()
        });
        Request {
            message_id: 1,
            sent_by_device_id: "social-connect-0123abcd".into(),
            command: Command::Transfer(crate::core::dealer::protocol::TransferCommand {
                data: Some(state),
                options: Default::default(),
                from_device_identifier: String::new(),
                logging_params: Default::default(),
            }),
        }
    }

    fn upcoming(task: &SpircTask) -> Vec<String> {
        task.connect_state
            .player()
            .next_tracks
            .iter()
            .map(|track| track.uri.clone())
            .collect()
    }

    #[tokio::test]
    async fn a_jam_transfer_plays_the_hosts_song_from_the_included_queue() {
        let mut task = task();
        let now = task.server_now_ms();
        task.handle_request(jam_transfer(3, 40_000, false, now))
            .await
            .unwrap();
        assert_eq!(task.connect_state.context_uri(), JAM);
        assert_eq!(
            task.connect_state.current_track(|track| track.uri.clone()),
            tracks()[3]
        );
        assert_eq!(upcoming(&task), tracks()[4..]);
        assert!(
            task.transfer_state.is_none(),
            "the queue isn't resolved again"
        );
        match task.play_status {
            SpircPlayStatus::LoadingPlay { position_ms } => {
                assert!((40_000..42_000).contains(&position_ms), "{position_ms}")
            }
            ref other => panic!("not loading to play: {other:?}"),
        }
        assert_eq!(task.devices.jam_phase(), JamPhase::Following);
        // The playback has its own ID, as in Spotify's apps.
        assert_eq!(task.connect_state.player().playback_id.len(), 32);
        // Following, its state reports the Jam mode on, as Spotify's app does.
        let modes = &task.connect_state.player().options.modes;
        assert_eq!(modes.get("jam").map(String::as_str), Some("on"));
        task.player.stop();
    }

    #[tokio::test]
    async fn a_jam_resend_for_the_playing_song_keeps_it_going() {
        let mut task = task();
        let now = task.server_now_ms();
        task.handle_request(jam_transfer(3, 40_000, false, now))
            .await
            .unwrap();
        let started = task.now_ms() - 40_000;
        task.play_status = SpircPlayStatus::Playing {
            nominal_start_time: started,
            preloading_of_next_track_triggered: false,
        };
        let now = task.server_now_ms();
        task.handle_request(jam_transfer(3, 40_300, false, now))
            .await
            .unwrap();
        assert!(
            matches!(
                task.play_status,
                SpircPlayStatus::Playing { nominal_start_time, .. } if nominal_start_time == started
            ),
            "a resend restarted or moved the song: {:?}",
            task.play_status
        );

        // The host pauses: the guest pauses where it is.
        task.handle_request(jam_transfer(3, 40_400, true, now))
            .await
            .unwrap();
        assert!(matches!(task.play_status, SpircPlayStatus::Paused { .. }));

        // The host skips: the guest loads the next song.
        task.handle_request(jam_transfer(4, 0, false, now))
            .await
            .unwrap();
        assert_eq!(
            task.connect_state.current_track(|track| track.uri.clone()),
            tracks()[4]
        );
        assert!(matches!(
            task.play_status,
            SpircPlayStatus::LoadingPlay { .. }
        ));
        assert_eq!(upcoming(&task), tracks()[5..]);
        task.player.stop();
    }

    #[tokio::test]
    async fn a_playing_host_does_not_follow_its_own_jam() {
        let mut task = task();
        task.jam_host = Some("jam-session".into());
        task.handle_activate(true);
        let now = task.server_now_ms();
        task.handle_request(jam_transfer(3, 0, false, now))
            .await
            .unwrap();
        assert_ne!(task.devices.jam_phase(), JamPhase::Following);
        assert!(matches!(task.play_status, SpircPlayStatus::Stopped));
        task.player.stop();
    }

    #[tokio::test]
    async fn a_left_jam_no_longer_drives_playback() {
        let mut task = task();
        let now = task.server_now_ms();
        task.handle_request(jam_transfer(3, 0, false, now))
            .await
            .unwrap();
        task.devices.leave_jam();
        task.handle_request(jam_transfer(7, 0, false, now))
            .await
            .unwrap();
        assert_eq!(
            task.connect_state.current_track(|track| track.uri.clone()),
            tracks()[3]
        );
        task.devices.join_jam();
        task.handle_request(jam_transfer(7, 0, false, now))
            .await
            .unwrap();
        assert_eq!(
            task.connect_state.current_track(|track| track.uri.clone()),
            tracks()[7]
        );
        task.player.stop();
    }

    #[tokio::test]
    async fn jam_mode_options_are_accepted_and_switch_following() {
        let mut task = task();
        for (mode, phase) in [("on", JamPhase::Following), ("off", JamPhase::Left)] {
            let request: Request = serde_json::from_value(serde_json::json!({
                "message_id": 2,
                "sent_by_device_id": "social-connect-0123abcd",
                "command": {"endpoint": "set_options", "modes": {"jam": mode}},
            }))
            .unwrap();
            task.handle_request(request).await.unwrap();
            assert_eq!(task.devices.jam_phase(), phase);
            assert_eq!(
                task.connect_state
                    .player()
                    .options
                    .modes
                    .get("jam")
                    .map(String::as_str),
                Some(mode)
            );
        }
    }

    #[tokio::test]
    async fn a_hosted_jam_is_named_in_the_hosts_state() {
        let mut task = task();
        let jam = |task: &SpircTask| {
            task.connect_state
                .player()
                .context_metadata
                .get(JAM_SESSION_KEY)
                .cloned()
        };
        task.jam_host = Some("jam-session".into());
        task.mark_jam_state();
        assert_eq!(jam(&task).as_deref(), Some("jam-session"));
        task.jam_host = None;
        task.mark_jam_state();
        assert_eq!(jam(&task), None);
        task.player.stop();
    }

    #[tokio::test]
    async fn shuffle_survives_plain_lists_and_respects_explicit_overrides() {
        let mut task = task();
        task.connect_state.handle_shuffle(true).unwrap();
        for explicit in [None, Some(LoadContextOptions::Options(Default::default()))] {
            task.handle_load(
                LoadRequest::from_tracks(
                    tracks(),
                    LoadRequestOptions {
                        context_options: explicit.clone(),
                        playing_track: Some(PlayingTrack::Index(5)),
                        ..Default::default()
                    },
                ),
                None,
                None,
            )
            .await
            .unwrap();
            assert_eq!(task.connect_state.shuffling_context(), explicit.is_none());
            assert_eq!(
                task.connect_state.current_track(|track| track.uri.clone()),
                tracks()[5]
            );
        }
        task.player.stop();
    }
}

#[cfg(test)]
mod startup_tests {
    use super::*;
    #[test]
    fn restored_cursor_keeps_duplicate_occurrence_and_next_page() {
        use crate::protocol::context_track::ContextTrack;
        let track = |uid: &str, uri: &str| ContextTrack {
            uid: Some(uid.into()),
            uri: Some(uri.into()),
            ..ContextTrack::default()
        };
        let mut context = Context {
            pages: vec![ContextPage {
                tracks: vec![
                    track("first-occurrence", "same-song"),
                    track("saved-occurrence", "same-song"),
                ],
                next_page_url: Some("hm://lexicon-session-provider/next".into()),
                ..ContextPage::default()
            }],
            ..Context::default()
        };
        let cursor = PlayingTrack::UidOrUri {
            uid: "saved-occurrence".into(),
            uri: "same-song".into(),
        };
        let current = requested_context_index(&context, &cursor).unwrap();
        assert_eq!(current, 1);
        assert_eq!(current + 1, context.pages[0].tracks.len());
        context.pages.push(ContextPage {
            tracks: vec![track("next-occurrence", "next-song")],
            ..ContextPage::default()
        });
        assert_eq!(
            context
                .pages
                .iter()
                .flat_map(|page| &page.tracks)
                .nth(current + 1)
                .unwrap()
                .uri
                .as_deref(),
            Some("next-song")
        );
        assert_eq!(
            requested_context_index(
                &context,
                &PlayingTrack::UidOrUri {
                    uid: "regenerated".into(),
                    uri: "next-song".into()
                }
            ),
            Some(2)
        );
    }
    #[test]
    fn a_track_past_the_loaded_pages_asks_for_more() {
        use crate::protocol::context_track::ContextTrack;
        let track = |uri: &str| ContextTrack {
            uri: Some(uri.into()),
            ..ContextTrack::default()
        };
        let mut context = Context {
            pages: vec![ContextPage {
                tracks: vec![track("first"), track("second")],
                ..ContextPage::default()
            }],
            ..Context::default()
        };
        let picked = PlayingTrack::Uri("deep".into());
        assert!(requested_track_unloaded(&context, &picked, 0));
        assert!(!requested_track_unloaded(
            &context,
            &PlayingTrack::Uri("second".into()),
            0
        ));
        assert!(requested_track_unloaded(
            &context,
            &PlayingTrack::Uri("second".into()),
            1
        ));
        assert!(requested_track_unloaded(
            &context,
            &PlayingTrack::Index(5),
            0
        ));
        context.pages.push(ContextPage {
            tracks: vec![track("deep")],
            ..ContextPage::default()
        });
        assert!(!requested_track_unloaded(&context, &picked, 0));
    }
    #[test]
    fn passive_startup_does_not_claim_a_restorable_session() {
        let mut cluster = Cluster {
            transfer_data: vec![1],
            ..Cluster::default()
        };
        assert!(automatic_restore_allowed(true, &cluster, "session"));
        assert!(!automatic_restore_allowed(false, &cluster, "session"));
        cluster.active_device_id = "spotify-desktop".into();
        assert!(!automatic_restore_allowed(true, &cluster, "session"));
        cluster.active_device_id.clear();
        cluster.player_state.mut_or_insert_default().session_id = "other-session".into();
        assert!(!automatic_restore_allowed(true, &cluster, "session"));
    }
}
