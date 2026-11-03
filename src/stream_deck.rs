//! Local control for the Lyssna Stream Deck plugin.
//!
//! A TCP server on 127.0.0.1 only, on a port chosen at start. The port and a
//! random key go to `stream-deck.json` in Lyssna's application data; the
//! plugin reads them and must send the key before anything else. Messages
//! are one JSON object per line. The plugin sends commands; Lyssna sends
//! the player state whenever it changes, and the playing song's synced
//! lyrics when they arrive. No Spotify credentials or tokens ever pass
//! through it.

use super::{Command, Desktop, PlayerRequest, lock, player};
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use tauri::{Listener, Manager};

/// Longest line a client may send; commands are a few dozen bytes.
const MAX_LINE: usize = 4096;
/// How long a new connection has to send the key.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// Plugin connections at once; Stream Deck runs one plugin process.
const MAX_CLIENTS: usize = 8;
/// Largest lyrics message; past it, word timing is dropped and the lines
/// highlight whole. The plugin accepts lines up to 1 MB.
const MAX_LYRICS: usize = 400_000;

/// What the keys and dials show.
#[derive(Clone, Default, PartialEq, Serialize)]
struct DeckState {
    signed_in: bool,
    playing: bool,
    track: Option<DeckTrack>,
    shuffle: bool,
    /// "off", "context" or "track".
    repeat: String,
    volume: Option<u8>,
    /// Whether the song is in Liked Songs, once known.
    liked: Option<bool>,
    /// Song position when last observed, and that moment in Unix
    /// milliseconds, so the plugin can run the clock between updates.
    position_ms: u64,
    observed_at: u64,
}

#[derive(Clone, PartialEq, Serialize)]
struct DeckTrack {
    uri: String,
    name: String,
    artists: String,
    /// Cover art about 300 px wide, from Spotify's image server.
    image: Option<String>,
    duration_ms: u64,
}

/// The playing song's lyrics for the touch strip: synced lines only, as
/// unsynced lyrics have nothing to follow.
#[derive(Clone, PartialEq, Serialize)]
struct DeckLyrics {
    uri: String,
    lines: Vec<DeckLine>,
}

#[derive(Clone, PartialEq, Serialize)]
struct DeckLine {
    start_ms: u64,
    end_ms: u64,
    text: String,
    /// The provider's reading for non-Latin lines, when it has one.
    #[serde(skip_serializing_if = "String::is_empty")]
    romanization: String,
    /// Each word's start, end and text; empty for line-timed lyrics.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    words: Vec<(u64, u64, String)>,
    /// Background vocals, shown smaller under the line they echo.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    background: bool,
    /// The other singer in a duet, aligned to the right.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    opposite: bool,
}

#[derive(Default)]
struct Shared {
    /// Connected plugins, by connection number.
    clients: Vec<(u64, TcpStream)>,
    next_client: u64,
    state: DeckState,
    /// The lyrics line last sent, so a new plugin gets it on connect.
    lyrics: String,
    saved: HashMap<String, bool>,
    /// Volume before Mute, to restore.
    unmuted: Option<u8>,
}

pub(super) fn start(app: tauri::AppHandle) -> Result<()> {
    let listener =
        TcpListener::bind(("127.0.0.1", 0)).context("Stream Deck control unavailable")?;
    let port = listener.local_addr()?.port();
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("No randomness for Stream Deck key"))?;
    let key: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let path = crate::config::data_dir()?.join("stream-deck.json");
    std::fs::create_dir_all(path.parent().context("No application data folder")?)?;
    std::fs::write(&path, json!({"port": port, "key": key}).to_string())?;

    let shared = Arc::new(Mutex::new(Shared::default()));
    // Liked Songs status arrives with the app's own library events.
    {
        let shared = shared.clone();
        app.listen_any("lyssna", move |event| {
            let Ok(value) = serde_json::from_str::<Value>(event.payload()) else {
                return;
            };
            let Ok(mut shared) = shared.lock() else {
                return;
            };
            match value["type"].as_str() {
                Some("saved") => {
                    if let (Some(uri), Some(saved)) =
                        (value["uri"].as_str(), value["saved"].as_bool())
                    {
                        shared.saved.insert(uri.to_owned(), saved);
                    }
                }
                Some("saved_many") => {
                    if let Some(items) = value["items"].as_object() {
                        for (uri, saved) in items {
                            if let Some(saved) = saved.as_bool() {
                                shared.saved.insert(uri.clone(), saved);
                            }
                        }
                    }
                }
                _ => {}
            }
        });
    }
    // Follow the player and tell the plugin what changed.
    {
        let shared = shared.clone();
        let app = app.clone();
        thread::spawn(move || {
            let mut asked = String::new();
            let mut lyrics_seen = None;
            loop {
                thread::sleep(Duration::from_millis(250));
                let Some(desktop) = app.try_state::<Desktop>() else {
                    continue;
                };
                let (next, lyrics) = {
                    let Ok(snapshot) = lock(&desktop.0.snapshot) else {
                        continue;
                    };
                    // Lyrics are rebuilt only when the song or its lyrics
                    // change, not every poll.
                    let seen = lyrics_key(&snapshot);
                    let lyrics = (seen != lyrics_seen).then(|| {
                        lyrics_seen = seen;
                        lyrics_line(read_lyrics(&snapshot).as_ref())
                    });
                    (read_state(&snapshot), lyrics)
                };
                // Ask once per song whether it is in Liked Songs.
                if let Some(track) = &next.track
                    && asked != track.uri
                {
                    asked = track.uri.clone();
                    let _ = desktop
                        .0
                        .commands
                        .send(Command::SavedStatus(vec![track.uri.clone()]));
                }
                let Ok(mut shared) = shared.lock() else {
                    continue;
                };
                let mut next = next;
                next.liked = next
                    .track
                    .as_ref()
                    .and_then(|track| shared.saved.get(&track.uri).copied());
                if next != shared.state {
                    shared.state = next;
                    let line = state_line(&shared.state);
                    send(&mut shared, &line);
                }
                if let Some(line) = lyrics
                    && line != shared.lyrics
                {
                    shared.lyrics = line;
                    let line = shared.lyrics.clone();
                    send(&mut shared, &line);
                }
            }
        });
    }
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let app = app.clone();
            let shared = shared.clone();
            let key = key.clone();
            thread::spawn(move || {
                let _ = serve(stream, &app, &shared, &key);
            });
        }
    });
    Ok(())
}

fn read_state(snapshot: &super::Snapshot) -> DeckState {
    let playback = &snapshot.playback;
    DeckState {
        signed_in: snapshot.profile.is_some(),
        playing: playback.is_playing,
        track: playback.item.as_ref().map(|track| DeckTrack {
            uri: track.uri.clone(),
            name: track.name.clone(),
            artists: track
                .artists
                .iter()
                .map(|artist| artist.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            image: cover(&track.album.images),
            duration_ms: track.duration_ms,
        }),
        shuffle: playback.shuffle_state,
        repeat: if playback.repeat_state.is_empty() {
            "off".into()
        } else {
            playback.repeat_state.clone()
        },
        volume: playback.device.volume_percent,
        liked: None,
        position_ms: playback.progress_ms.unwrap_or_default(),
        observed_at: snapshot.observed_at,
    }
}

/// The smallest Spotify cover at least 300 px wide, or the largest. A
/// local song's own cover goes inline, as the plugin cannot reach it.
fn cover(images: &[crate::model::Artwork]) -> Option<String> {
    if let Some(local) = images
        .iter()
        .find_map(|image| crate::local_files::inline_cover(&image.url))
    {
        return Some(local);
    }
    let secure = images
        .iter()
        .filter(|image| image.url.starts_with("https://"));
    secure
        .clone()
        .filter(|image| image.width.unwrap_or(0) >= 300)
        .min_by_key(|image| image.width.unwrap_or(u32::MAX))
        .or_else(|| secure.max_by_key(|image| image.width.unwrap_or(0)))
        .map(|image| image.url.clone())
}

/// What identifies the lyrics on hand, cheaply: the song, the source and
/// the line timing.
fn lyrics_key(snapshot: &super::Snapshot) -> Option<(String, String, usize, u64)> {
    let track = snapshot.playback.item.as_ref()?;
    let lyrics = snapshot.lyrics.as_ref()?;
    Some((
        track.uri.clone(),
        lyrics.source.clone(),
        lyrics.lines.len(),
        lyrics.lines.iter().map(|line| line.start_ms).sum(),
    ))
}

fn read_lyrics(snapshot: &super::Snapshot) -> Option<DeckLyrics> {
    let track = snapshot.playback.item.as_ref()?;
    deck_lyrics(&track.uri, snapshot.lyrics.as_ref()?)
}

/// Synced lines with words to show, background vocals and duet sides
/// included.
fn deck_lyrics(uri: &str, lyrics: &crate::lyrics::Lyrics) -> Option<DeckLyrics> {
    if !lyrics.synced || lyrics.instrumental {
        return None;
    }
    let lines: Vec<DeckLine> = lyrics
        .lines
        .iter()
        .filter(|line| !line.text.trim().is_empty())
        .map(|line| DeckLine {
            start_ms: line.start_ms,
            end_ms: line.end_ms,
            text: line.text.clone(),
            romanization: line.romanization.clone(),
            words: line
                .words
                .iter()
                .map(|word| (word.start_ms, word.end_ms, word.text.clone()))
                .collect(),
            background: line.background,
            opposite: line.opposite,
        })
        .collect();
    (!lines.is_empty()).then(|| DeckLyrics {
        uri: uri.to_owned(),
        lines,
    })
}

/// The lyrics message; null when the song has none to follow.
fn lyrics_line(lyrics: Option<&DeckLyrics>) -> String {
    let line = format!("{}\n", json!({"type": "lyrics", "lyrics": lyrics}));
    if line.len() <= MAX_LYRICS {
        return line;
    }
    let lines = lyrics.map(|lyrics| DeckLyrics {
        uri: lyrics.uri.clone(),
        lines: lyrics
            .lines
            .iter()
            .map(|line| DeckLine {
                words: Vec::new(),
                ..line.clone()
            })
            .collect(),
    });
    let line = format!("{}\n", json!({"type": "lyrics", "lyrics": lines}));
    if line.len() <= MAX_LYRICS {
        line
    } else {
        format!("{}\n", json!({"type": "lyrics", "lyrics": null}))
    }
}

fn send(shared: &mut Shared, line: &str) {
    shared
        .clients
        .retain_mut(|(_, client)| client.write_all(line.as_bytes()).is_ok());
}

fn state_line(state: &DeckState) -> String {
    format!("{}\n", json!({"type": "state", "state": state}))
}

fn serve(
    stream: TcpStream,
    app: &tauri::AppHandle,
    shared: &Mutex<Shared>,
    key: &str,
) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    // Only the key has a deadline. The limit goes on the handle that reads:
    // on Windows a cloned socket keeps its own, and left on it, every quiet
    // spell of that length dropped the plugin.
    reader.get_ref().set_read_timeout(Some(HELLO_TIMEOUT))?;
    // The key comes first; anything else closes the connection.
    let hello = read_line(&mut reader)?;
    let hello: Value = serde_json::from_str(&hello)?;
    anyhow::ensure!(
        hello["type"] == "hello" && hello["key"].as_str().is_some_and(|given| same(given, key)),
        "Wrong Stream Deck key"
    );
    reader.get_ref().set_read_timeout(None)?;
    let id = {
        let mut shared = shared
            .lock()
            .map_err(|_| anyhow::anyhow!("State unavailable"))?;
        anyhow::ensure!(
            shared.clients.len() < MAX_CLIENTS,
            "Too many Stream Deck connections"
        );
        let mut writer = stream.try_clone()?;
        writer.write_all(state_line(&shared.state).as_bytes())?;
        if !shared.lyrics.is_empty() {
            writer.write_all(shared.lyrics.as_bytes())?;
        }
        shared.next_client += 1;
        let id = shared.next_client;
        shared.clients.push((id, writer));
        id
    };
    let result = (|| -> Result<()> {
        loop {
            let line = read_line(&mut reader)?;
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message["type"] == "command"
                && let Some(name) = message["command"].as_str()
            {
                let _ = run(app, shared, name, &message);
            }
        }
    })();
    // A plugin that leaves is dropped at once, not at the next update.
    if let Ok(mut shared) = shared.lock() {
        shared.clients.retain(|(client, _)| *client != id);
    }
    result
}

/// One line, refusing overlong ones instead of buffering them.
fn read_line(reader: &mut BufReader<TcpStream>) -> Result<String> {
    let mut line = Vec::new();
    loop {
        let buffer = reader.fill_buf()?;
        anyhow::ensure!(!buffer.is_empty(), "Stream Deck plugin disconnected");
        if let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
            line.extend_from_slice(&buffer[..end]);
            reader.consume(end + 1);
            break;
        }
        line.extend_from_slice(buffer);
        let used = buffer.len();
        reader.consume(used);
        anyhow::ensure!(line.len() <= MAX_LINE, "Stream Deck message too long");
    }
    anyhow::ensure!(line.len() <= MAX_LINE, "Stream Deck message too long");
    Ok(String::from_utf8(line)?)
}

/// Compares keys in time independent of where they differ.
fn same(given: &str, key: &str) -> bool {
    given.len() == key.len()
        && given
            .bytes()
            .zip(key.bytes())
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

fn run(app: &tauri::AppHandle, shared: &Mutex<Shared>, name: &str, message: &Value) -> Result<()> {
    let desktop = app.state::<Desktop>();
    let state = shared
        .lock()
        .map_err(|_| anyhow::anyhow!("State unavailable"))?
        .state
        .clone();
    let request = match name {
        "toggle" => json!({"command": if state.playing { "pause" } else { "play" }}),
        "next" => json!({"command": "next"}),
        "previous" => json!({"command": "previous"}),
        "shuffle" => json!({"command": "shuffle", "enabled": !state.shuffle}),
        "repeat" => json!({"command": "repeat", "repeat": next_repeat(&state.repeat)}),
        "volume" => {
            let step = message["step"].as_i64().unwrap_or(0).clamp(-100, 100);
            let current = i64::from(state.volume.unwrap_or(50));
            let volume = (current + step).clamp(0, 100) as u8;
            if volume > 0 {
                shared
                    .lock()
                    .map_err(|_| anyhow::anyhow!("State unavailable"))?
                    .unmuted = None;
            }
            json!({"command": "volume", "volume": volume})
        }
        "mute" => {
            let mut shared = shared
                .lock()
                .map_err(|_| anyhow::anyhow!("State unavailable"))?;
            let volume = match (state.volume.unwrap_or(0), shared.unmuted.take()) {
                (0, restore) => restore.unwrap_or(50),
                (current, _) => {
                    shared.unmuted = Some(current);
                    0
                }
            };
            json!({"command": "volume", "volume": volume})
        }
        "like" => {
            let track = state.track.context("Nothing playing")?;
            let saved = !state.liked.unwrap_or(false);
            desktop.0.commands.send(Command::SetSaved {
                uri: track.uri.clone(),
                saved,
            })?;
            // Show it at once; the library confirms or corrects it.
            let mut shared = shared
                .lock()
                .map_err(|_| anyhow::anyhow!("State unavailable"))?;
            shared.saved.insert(track.uri, saved);
            return Ok(());
        }
        _ => anyhow::bail!("Unknown Stream Deck command"),
    };
    let request: PlayerRequest = serde_json::from_value(request)?;
    player(app.clone(), request, None, desktop).map_err(anyhow::Error::msg)
}

/// Spotify's repeat cycle: off, then the playlist, then the song.
fn next_repeat(current: &str) -> &'static str {
    match current {
        "context" => "track",
        "track" => "off",
        _ => "context",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_must_match_exactly() {
        assert!(same("abc123", "abc123"));
        assert!(!same("abc124", "abc123"));
        assert!(!same("abc12", "abc123"));
        assert!(!same("", "abc123"));
    }

    #[test]
    fn repeat_cycles_like_spotify() {
        assert_eq!(next_repeat("off"), "context");
        assert_eq!(next_repeat(""), "context");
        assert_eq!(next_repeat("context"), "track");
        assert_eq!(next_repeat("track"), "off");
    }

    #[test]
    fn covers_prefer_the_smallest_secure_image_that_fills_a_key() {
        let image = |url: &str, width| crate::model::Artwork {
            url: url.into(),
            width: Some(width),
        };
        let images = [
            image("https://i.scdn.co/image/large", 640),
            image("https://i.scdn.co/image/medium", 300),
            image("https://i.scdn.co/image/small", 64),
            image("http://insecure.test/cover", 300),
        ];
        assert_eq!(
            cover(&images).as_deref(),
            Some("https://i.scdn.co/image/medium")
        );
        assert_eq!(
            cover(&[image("https://i.scdn.co/image/small", 64)]).as_deref(),
            Some("https://i.scdn.co/image/small")
        );
    }

    #[test]
    fn the_key_deadline_does_not_outlive_the_key() {
        // The same handling as `serve`, with a short deadline.
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let client = thread::spawn(move || {
            let mut client = TcpStream::connect(address).unwrap();
            client.write_all(b"hello\n").unwrap();
            thread::sleep(Duration::from_millis(600));
            client.write_all(b"still here\n").unwrap();
            thread::sleep(Duration::from_millis(100));
        });
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        reader
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        assert_eq!(read_line(&mut reader).unwrap(), "hello");
        reader.get_ref().set_read_timeout(None).unwrap();
        // A quiet spell longer than the deadline must not end the connection.
        assert_eq!(read_line(&mut reader).unwrap(), "still here");
        client.join().unwrap();
    }

    fn timed(text: &str, start_ms: u64, background: bool) -> crate::lyrics::Line {
        crate::lyrics::Line {
            start_ms,
            end_ms: start_ms + 1000,
            text: text.into(),
            words: vec![crate::lyrics::Word {
                start_ms,
                end_ms: start_ms + 1000,
                text: text.into(),
            }],
            background,
            ..Default::default()
        }
    }

    #[test]
    fn the_strip_gets_synced_lines_with_their_singers() {
        let mut lyrics = crate::lyrics::Lyrics {
            lines: vec![
                timed("first", 0, false),
                timed("(echo)", 500, true),
                timed("  ", 900, false),
                timed("second", 1000, false),
            ],
            synced: true,
            ..Default::default()
        };
        lyrics.lines[3].opposite = true;
        let deck = deck_lyrics("spotify:track:a", &lyrics).unwrap();
        let texts: Vec<_> = deck.lines.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(texts, ["first", "(echo)", "second"]);
        assert!(deck.lines[1].background && !deck.lines[1].opposite);
        assert!(deck.lines[2].opposite && !deck.lines[2].background);
        let sent = lyrics_line(Some(&deck));
        assert_eq!(sent.matches("\"background\":true").count(), 1);
        assert_eq!(sent.matches("\"opposite\":true").count(), 1);
        assert_eq!(deck.lines[2].words, [(1000, 2000, "second".into())]);
        lyrics.synced = false;
        assert!(deck_lyrics("spotify:track:a", &lyrics).is_none());
        assert_eq!(lyrics_line(None), "{\"lyrics\":null,\"type\":\"lyrics\"}\n");
    }

    #[test]
    fn huge_lyrics_drop_word_timing_first() {
        let line = DeckLine {
            start_ms: 0,
            end_ms: 1,
            text: "la".into(),
            romanization: String::new(),
            words: vec![(0, 1, "x".repeat(1000)); 10],
            background: false,
            opposite: false,
        };
        let lyrics = DeckLyrics {
            uri: "spotify:track:a".into(),
            lines: vec![line; 50],
        };
        let sent = lyrics_line(Some(&lyrics));
        assert!(sent.len() <= MAX_LYRICS);
        assert!(sent.contains("\"text\":\"la\""));
        assert!(!sent.contains("words"));
    }

    #[test]
    fn overlong_lines_are_refused() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let writer = thread::spawn(move || {
            let mut client = TcpStream::connect(address).unwrap();
            let _ = client.write_all(&vec![b'a'; MAX_LINE * 3]);
            let _ = client.write_all(b"\n");
        });
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        assert!(read_line(&mut reader).is_err());
        writer.join().unwrap();
    }
}
