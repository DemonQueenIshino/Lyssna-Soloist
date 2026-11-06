//! Live connection to Spotify Soloist's local WebSocket: commands reach it
//! without a round trip through Spotify, and its state arrives as it changes.
use crate::api::PlayerAction;
use crate::model::{Album, Artist, Artwork, Track};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs,
    net::TcpStream,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket};

#[derive(Clone, Default)]
pub struct State {
    pub active: bool,
    pub playing: bool,
    pub track: Option<Track>,
    pub position_ms: u64,
    /// Unix time in milliseconds when `position_ms` was measured.
    pub measured_at: i64,
    pub context: Option<String>,
    pub shuffle: bool,
    pub repeat: String,
    pub volume: Option<u8>,
}

/// Soloist's upcoming songs and how many of them were queued by hand,
/// while Soloist is the active device.
static QUEUE: Mutex<Option<(Vec<Track>, usize)>> = Mutex::new(None);

pub fn queue() -> Option<(Vec<Track>, usize)> {
    QUEUE.lock().ok().and_then(|queue| queue.clone())
}

fn set_queue(queue: Option<(Vec<Track>, usize)>) {
    if let Ok(mut current) = QUEUE.lock() {
        *current = queue;
    }
}

pub struct Link {
    commands: mpsc::Sender<Value>,
    live: Arc<AtomicBool>,
    /// Soloist is the account's active device.
    active: Arc<AtomicBool>,
    /// Soloist has a song loaded that Play can resume.
    loaded: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl Link {
    pub fn start(
        data: PathBuf,
        mut on_state: impl FnMut(State) + Send + 'static,
        mut on_queue: impl FnMut() + Send + 'static,
    ) -> Self {
        let (commands, receive) = mpsc::channel();
        let live = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicBool::new(false));
        let loaded = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_live, thread_stop) = (live.clone(), stop.clone());
        let flags = Flags {
            active: active.clone(),
            loaded: loaded.clone(),
        };
        let _ = thread::Builder::new()
            .name("soloist-link".into())
            .spawn(move || {
                while !thread_stop.load(Ordering::Acquire) {
                    if let Ok(mut socket) = open(&data) {
                        thread_live.store(true, Ordering::Release);
                        run(
                            &mut socket,
                            &receive,
                            &thread_stop,
                            &flags,
                            &mut on_state,
                            &mut on_queue,
                        );
                        thread_live.store(false, Ordering::Release);
                        flags.set(false, false);
                        set_queue(None);
                        on_state(State::default());
                        on_queue();
                    }
                    thread::sleep(Duration::from_secs(1));
                }
            });
        Self {
            commands,
            live,
            active,
            loaded,
            stop,
        }
    }

    pub fn live(&self) -> bool {
        self.live.load(Ordering::Acquire)
    }

    pub fn active(&self) -> bool {
        self.live() && self.active.load(Ordering::Acquire)
    }

    pub fn loaded(&self) -> bool {
        self.loaded.load(Ordering::Acquire)
    }

    /// Starts a song in Soloist at a position, for when it has none loaded.
    pub fn play_from(&self, uri: &str, position_ms: u64) -> bool {
        if !self.live() || !uri.starts_with("spotify:track:") {
            return false;
        }
        let mut sent = vec![command("play", json!({"uri": uri}))];
        if position_ms > 0 {
            sent.push(command("seek", json!({"position_ms": position_ms})));
        }
        sent.into_iter()
            .all(|command| self.commands.send(command).is_ok())
    }

    /// Makes Soloist the account's active device.
    pub fn activate(&self) -> bool {
        self.live() && self.commands.send(command("activate", json!({}))).is_ok()
    }

    /// Picks up where the account last stopped, as Spotify's own apps do
    /// when they become the active device; plays `uri` from `position_ms`
    /// when Spotify has nothing to hand over.
    pub fn resume(&self, uri: &str, position_ms: u64) -> bool {
        if !self.activate() {
            return false;
        }
        let (commands, live, active, loaded) = (
            self.commands.clone(),
            self.live.clone(),
            self.active.clone(),
            self.loaded.clone(),
        );
        let uri = uri.to_owned();
        thread::spawn(move || {
            let waited = Instant::now();
            while waited.elapsed() < Duration::from_millis(2500) {
                if live.load(Ordering::Acquire)
                    && active.load(Ordering::Acquire)
                    && loaded.load(Ordering::Acquire)
                {
                    let _ = commands.send(command("play", json!({})));
                    return;
                }
                thread::sleep(Duration::from_millis(50));
            }
            if uri.starts_with("spotify:track:") {
                let _ = commands.send(command("play", json!({"uri": uri})));
                if position_ms > 0 {
                    let _ = commands.send(command("seek", json!({"position_ms": position_ms})));
                }
            }
        });
        true
    }

    /// Gives up Soloist's place as the account's active device.
    pub fn deactivate(&self) -> bool {
        self.live() && self.commands.send(command("deactivate", json!({}))).is_ok()
    }

    /// Sends the action straight to Soloist; false when it needs Connect.
    pub fn send(&self, action: &PlayerAction) -> bool {
        let commands = commands(action);
        if commands.is_empty() || !self.live() {
            return false;
        }
        commands
            .into_iter()
            .all(|command| self.commands.send(command).is_ok())
    }
}

struct Flags {
    active: Arc<AtomicBool>,
    loaded: Arc<AtomicBool>,
}

impl Flags {
    fn set(&self, active: bool, loaded: bool) {
        self.active.store(active, Ordering::Release);
        self.loaded.store(loaded, Ordering::Release);
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

fn open(data: &std::path::Path) -> anyhow::Result<WebSocket<TcpStream>> {
    let port: u16 = fs::read_to_string(data.join("ws.port"))?.trim().parse()?;
    let stream =
        TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(2))?;
    let (socket, _) = tungstenite::client(format!("ws://127.0.0.1:{port}"), stream)?;
    // Short reads let queued commands go out between Soloist's messages.
    socket
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(20)))?;
    Ok(socket)
}

fn run(
    socket: &mut WebSocket<TcpStream>,
    commands: &mpsc::Receiver<Value>,
    stop: &AtomicBool,
    flags: &Flags,
    on_state: &mut impl FnMut(State),
    on_queue: &mut impl FnMut(),
) {
    let ask_queue = command("get_queue", json!({"limit": QUEUE_LENGTH}));
    if socket.send(Message::text(ask_queue.to_string())).is_err() {
        return;
    }
    let mut playing: Option<String> = None;
    while !stop.load(Ordering::Acquire) {
        while let Ok(command) = commands.try_recv() {
            if socket.send(Message::text(command.to_string())).is_err() {
                return;
            }
        }
        match socket.read() {
            Ok(Message::Text(text)) => {
                let Ok(message) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                match message["type"].as_str() {
                    Some("auth_state") => {
                        let active = message["is_active"].as_bool().unwrap_or(false);
                        flags.active.store(active, Ordering::Release);
                        if !active {
                            flags.loaded.store(false, Ordering::Release);
                            set_queue(None);
                            on_state(State::default());
                            on_queue();
                        }
                    }
                    Some("playback_state") => {
                        if let Ok(playback) = serde_json::from_value::<Playback>(message) {
                            let state = playback.state();
                            flags.set(state.active, state.track.is_some());
                            let uri = state.track.as_ref().map(|track| track.uri.clone());
                            if uri != playing {
                                playing = uri;
                                if socket.send(Message::text(ask_queue.to_string())).is_err() {
                                    return;
                                }
                            }
                            on_state(state);
                        }
                    }
                    Some("queue_changed") => {
                        if let Ok(reply) = serde_json::from_value::<QueueReply>(message) {
                            set_queue(
                                flags
                                    .active
                                    .load(Ordering::Acquire)
                                    .then(|| reply.upcoming()),
                            );
                            on_queue();
                        }
                    }
                    _ => {}
                }
            }
            Ok(Message::Close(_)) => return,
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return,
        }
    }
    let _ = socket.close(None);
}

const QUEUE_LENGTH: usize = 50;

#[derive(Deserialize)]
struct QueueReply {
    #[serde(default)]
    upcoming: Vec<QueueEntry>,
}

#[derive(Deserialize)]
struct QueueEntry {
    #[serde(default)]
    source: String,
    item: Option<Item>,
}

impl QueueReply {
    fn upcoming(self) -> (Vec<Track>, usize) {
        let queued = self
            .upcoming
            .iter()
            .take_while(|entry| entry.source == "queue")
            .count();
        let tracks = self
            .upcoming
            .into_iter()
            .filter_map(|entry| entry.item.and_then(track))
            .collect();
        (tracks, queued)
    }
}

fn command(name: &str, fields: Value) -> Value {
    let mut command = json!({"type": "command", "command": name});
    if let (Some(command), Value::Object(fields)) = (command.as_object_mut(), fields) {
        command.extend(fields);
    }
    command
}

fn commands(action: &PlayerAction) -> Vec<Value> {
    match action {
        PlayerAction::Play(None) => vec![command("play", json!({}))],
        PlayerAction::Pause => vec![command("pause", json!({}))],
        PlayerAction::Next => vec![command("skip_next", json!({}))],
        PlayerAction::Previous => vec![command("skip_prev", json!({}))],
        PlayerAction::Seek(position) => vec![command("seek", json!({"position_ms": position}))],
        PlayerAction::Volume(volume) => {
            vec![command("set_volume", json!({"volume": (*volume).min(100)}))]
        }
        PlayerAction::Shuffle(enabled) => {
            vec![command("set_shuffle", json!({"enabled": enabled}))]
        }
        PlayerAction::Repeat(mode) => match mode.as_str() {
            "track" => vec![command("set_repeat_track", json!({"enabled": true}))],
            "context" => vec![
                command("set_repeat_track", json!({"enabled": false})),
                command("set_repeat_context", json!({"enabled": true})),
            ],
            _ => vec![
                command("set_repeat_track", json!({"enabled": false})),
                command("set_repeat_context", json!({"enabled": false})),
            ],
        },
        PlayerAction::Enqueue(uri) if uri.starts_with("spotify:track:") => {
            vec![command("add_to_queue", json!({"uri": uri}))]
        }
        _ => Vec::new(),
    }
}

#[derive(Deserialize)]
struct Playback {
    #[serde(default)]
    status: String,
    item: Option<Item>,
    context: Option<Entity>,
    #[serde(default)]
    position: Position,
    volume: Option<u8>,
    #[serde(default)]
    is_active: bool,
    #[serde(default)]
    options: Options,
}

#[derive(Default, Deserialize)]
struct Position {
    #[serde(default)]
    position_ms: u64,
    #[serde(default)]
    timestamp_ms: i64,
}

#[derive(Default, Deserialize)]
struct Options {
    #[serde(default)]
    shuffle: bool,
    #[serde(default)]
    repeat: String,
}

#[derive(Default, Deserialize)]
struct Entity {
    #[serde(default)]
    uri: String,
    #[serde(default)]
    decorations: Decorations,
}

#[derive(Default, Deserialize)]
struct Decorations {
    #[serde(default)]
    identity: Identity,
    #[serde(default)]
    visual_identity: Visual,
}

#[derive(Default, Deserialize)]
struct Identity {
    #[serde(default)]
    name: String,
}

#[derive(Default, Deserialize)]
struct Visual {
    #[serde(default)]
    cover: Vec<Cover>,
}

#[derive(Deserialize)]
struct Cover {
    url: String,
    #[serde(default)]
    size: String,
}

#[derive(Deserialize)]
struct Item {
    uri: String,
    #[serde(default)]
    decorations: Decorations,
    parent: Option<Wrapped>,
    #[serde(default)]
    creators: Vec<Wrapped>,
    playback: Option<ItemPlayback>,
}

#[derive(Deserialize)]
struct Wrapped {
    entity: Entity,
}

#[derive(Deserialize)]
struct ItemPlayback {
    #[serde(default)]
    duration_ms: u64,
}

fn id(uri: &str) -> Option<String> {
    uri.rsplit(':').next().map(str::to_owned)
}

fn size(cover: &Cover) -> Option<u32> {
    match cover.size.as_str() {
        "small" => Some(64),
        "default" => Some(300),
        "large" | "xlarge" => Some(640),
        _ => None,
    }
}

fn track(item: Item) -> Option<Track> {
    if !item.uri.starts_with("spotify:track:") {
        return None;
    }
    let mut covers = item.decorations.visual_identity.cover;
    if let Some(parent) = item.parent.as_ref() {
        covers.extend(
            parent
                .entity
                .decorations
                .visual_identity
                .cover
                .iter()
                .map(|cover| Cover {
                    url: cover.url.clone(),
                    size: cover.size.clone(),
                }),
        );
    }
    let mut images = Vec::new();
    for cover in &covers {
        let width = size(cover);
        if width == Some(640) && images.iter().any(|image: &Artwork| image.width == width) {
            continue;
        }
        if !images.iter().any(|image: &Artwork| image.url == cover.url) {
            images.push(Artwork {
                url: cover.url.clone(),
                width,
            });
        }
    }
    images.sort_by_key(|image| std::cmp::Reverse(image.width));
    let artists: Vec<Artist> = item
        .creators
        .iter()
        .map(|creator| Artist {
            id: id(&creator.entity.uri),
            uri: creator.entity.uri.clone(),
            name: creator.entity.decorations.identity.name.clone(),
            ..Default::default()
        })
        .collect();
    let album = item
        .parent
        .map(|parent| Album {
            id: id(&parent.entity.uri),
            uri: parent.entity.uri,
            name: parent.entity.decorations.identity.name,
            images: images.clone(),
            artists: artists.clone(),
            ..Default::default()
        })
        .unwrap_or_else(|| Album {
            images,
            ..Default::default()
        });
    Some(Track {
        id: id(&item.uri),
        uri: item.uri,
        name: item.decorations.identity.name,
        artists,
        album,
        duration_ms: item.playback.map_or(0, |playback| playback.duration_ms),
        ..Default::default()
    })
}

impl Playback {
    fn state(self) -> State {
        let track = self.item.and_then(track);
        State {
            active: self.is_active,
            playing: self.status == "playing",
            track,
            position_ms: self.position.position_ms,
            measured_at: self.position.timestamp_ms,
            context: self
                .context
                .map(|context| context.uri)
                .filter(|uri| !uri.is_empty()),
            shuffle: self.options.shuffle,
            repeat: if self.options.repeat.is_empty() {
                "off".into()
            } else {
                self.options.repeat
            },
            volume: self.volume,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_reply_keeps_upcoming_songs_and_counts_queued_ones() {
        let entry = |source: &str, id: &str| {
            json!({"uid": id, "source": source, "item": {
                "uri": format!("spotify:track:{id}"),
                "entity_type": "track",
                "decorations": {"identity": {"name": id}},
                "creators": [{"entity": {"uri": "spotify:artist:a", "decorations": {"identity": {"name": "Rihanna"}}}}],
                "playback": {"duration_ms": 243000}
            }})
        };
        let reply: QueueReply = serde_json::from_value(json!({
            "type": "queue_changed",
            "previous": [entry("context", "old")],
            "upcoming": [entry("queue", "picked"), entry("context", "next"), {"source": "context", "item": {"uri": "spotify:episode:x"}}],
        }))
        .unwrap();
        let (tracks, queued) = reply.upcoming();
        assert_eq!(queued, 1);
        assert_eq!(
            tracks
                .iter()
                .map(|track| track.name.as_str())
                .collect::<Vec<_>>(),
            ["picked", "next"]
        );
        assert_eq!(tracks[1].artists[0].name, "Rihanna");
        assert_eq!(tracks[1].duration_ms, 243000);
    }

    #[test]
    fn playback_state_becomes_a_track_and_position() {
        let message = json!({
            "type": "playback_state",
            "status": "playing",
            "item": {
                "uri": "spotify:track:0l4DTppOxy7NUaEcwXuOb6",
                "decorations": {
                    "identity": {"name": "Animal"},
                    "visual_identity": {"cover": [
                        {"url": "https://i.scdn.co/image/small", "size": "small"},
                        {"url": "https://i.scdn.co/image/large", "size": "large"},
                        {"url": "https://i.scdn.co/image/xlarge", "size": "xlarge"}
                    ]}
                },
                "parent": {"entity": {"uri": "spotify:album:6oZ5iDw1LT25svEK0g5OqT",
                    "decorations": {"identity": {"name": "Habits"}}}},
                "creators": [{"entity": {"uri": "spotify:artist:0RpddSzUHfncUWNJXKOsjy",
                    "decorations": {"identity": {"name": "Neon Trees"}}}}],
                "playback": {"duration_ms": 212306}
            },
            "context": {"uri": ""},
            "position": {"position_ms": 74713, "timestamp_ms": 1793951134685i64, "speed": 1},
            "volume": 100,
            "is_active": true,
            "options": {"shuffle": false, "repeat": "context"}
        });
        let state = serde_json::from_value::<Playback>(message).unwrap().state();
        let track = state.track.unwrap();
        assert_eq!(track.name, "Animal");
        assert_eq!(track.artist(), "Neon Trees");
        assert_eq!(track.album.name, "Habits");
        assert_eq!(track.duration_ms, 212306);
        assert_eq!(track.album.images[0].url, "https://i.scdn.co/image/large");
        assert_eq!(track.album.images.len(), 2);
        assert!(state.playing && state.active);
        assert_eq!(
            (state.position_ms, state.measured_at),
            (74713, 1793951134685)
        );
        assert_eq!(state.context, None);
        assert_eq!(state.repeat, "context");
    }

    #[test]
    fn transport_actions_become_soloist_commands() {
        assert_eq!(
            commands(&PlayerAction::Seek(5000)),
            vec![json!({"type":"command","command":"seek","position_ms":5000})]
        );
        assert_eq!(
            commands(&PlayerAction::Next),
            vec![json!({"type":"command","command":"skip_next"})]
        );
        assert_eq!(commands(&PlayerAction::Repeat("context".into())).len(), 2);
        assert!(commands(&PlayerAction::Play(Some("spotify:track:x".into()))).is_empty());
        assert!(commands(&PlayerAction::Enqueue("spotify:episode:x".into())).is_empty());
    }
}
