//! Remote Connect commands and state using the account's native subscription.
use anyhow::{Context, Result, ensure};
use reqwest::Method;
use serde_json::{Value, json};
use std::io::Write;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
};

type TracksByAccount = HashMap<(String, String), Track>;
static REMOTE_TRACKS: LazyLock<Mutex<TracksByAccount>> = LazyLock::new(Default::default);

use crate::{
    api::PlayerAction,
    model::{Device, Playback, Track},
    native::NativeControls,
};

impl NativeControls {
    pub(crate) fn remote_playback(&self) -> Result<Playback> {
        let session = self.session()?;
        let Some(cluster) = self.connect_cluster()? else {
            return Ok(Playback::default());
        };
        if cluster.active_device_id.is_empty() || cluster.active_device_id == session.device_id() {
            return Ok(Playback::default());
        }
        let device = self
            .devices()?
            .into_iter()
            .find(|device| device.id.as_deref() == Some(&cluster.active_device_id))
            .unwrap_or(Device {
                id: Some(cluster.active_device_id.clone()),
                ..Default::default()
            });
        let Some(state) = cluster.player_state.as_ref() else {
            return Ok(Playback {
                device,
                ..Default::default()
            });
        };
        let track = state
            .track
            .as_ref()
            .filter(|track| track.uri.starts_with("spotify:track:"))
            .and_then(|track| {
                let key = (session.username(), track.uri.clone());
                if let Some(cached) = REMOTE_TRACKS
                    .lock()
                    .ok()
                    .and_then(|cache| cache.get(&key).cloned())
                {
                    return Some(cached);
                }
                let resolved = self.track(&track.uri).ok()?;
                if let Ok(mut cache) = REMOTE_TRACKS.lock() {
                    if cache.len() >= 128 {
                        cache.clear();
                    }
                    cache.insert(key, resolved.clone());
                }
                Some(resolved)
            });
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;
        let now = server_now_ms(now, self.server_clock_offset_ms());
        Ok(playback_from_state(state, device, track, now))
    }

    pub(crate) fn remote_queue(&self) -> Result<Vec<Track>> {
        if self.has_track() {
            return self.queue();
        }
        let Some(cluster) = self.connect_cluster()? else {
            return Ok(Vec::new());
        };
        let uris = cluster
            .player_state
            .as_ref()
            .map(|state| {
                state
                    .next_tracks
                    .iter()
                    .filter(|track| track.uri.starts_with("spotify:track:"))
                    .take(50)
                    .map(|track| track.uri.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if uris.is_empty() {
            return Ok(Vec::new());
        }
        self.resolve(&uris)
    }

    pub(crate) fn remote_action(&self, action: PlayerAction, device: Option<&str>) -> Result<()> {
        if let PlayerAction::Transfer(id) = &action {
            return self.transfer(id);
        }
        let session = self.session()?;
        let devices = self.devices()?;
        let target = device
            .map(str::to_owned)
            .or_else(|| {
                devices
                    .iter()
                    .find(|device| device.is_active)
                    .and_then(|device| device.id.clone())
            })
            .context("Select a Spotify playback device")?;
        ensure!(
            target.len() <= 256
                && !target.is_empty()
                && target
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte)),
            "Invalid Spotify playback device"
        );
        let target = crate::native::listed_device(&target, &devices)
            .and_then(|device| device.id.clone())
            .context("Spotify playback device is no longer available")?;
        if target == session.device_id() {
            return self
                .send(action)
                .map_err(|_| anyhow::anyhow!("Native playback disconnected"));
        }
        self.run(async |session| {
            let request = async {
                if let PlayerAction::Volume(volume) = action {
                    let endpoint = format!(
                        "/connect-state/v1/connect/volume/from/{}/to/{target}",
                        session.device_id()
                    );
                    let level = remote_volume(volume);
                    // Spotify's clients send this as JSON; the protobuf form
                    // was never seen to move a speaker. Setting the same
                    // level twice is harmless, so fall back to it if refused.
                    let mut headers = reqwest::header::HeaderMap::new();
                    headers.insert(
                        reqwest::header::CONTENT_TYPE,
                        reqwest::header::HeaderValue::from_static("application/json"),
                    );
                    let body = serde_json::to_vec(&json!({ "volume": level }))?;
                    match session
                        .spclient()
                        .request(&Method::PUT, &endpoint, Some(headers), Some(&body))
                        .await
                    {
                        Ok(response) => Ok(response),
                        Err(_) => {
                            let message = librespot_protocol::connect::SetVolumeCommand {
                                volume: level,
                                connection_type: "wlan".into(),
                                ..Default::default()
                            };
                            session
                                .spclient()
                                .request_with_protobuf(&Method::PUT, &endpoint, None, &message)
                                .await
                        }
                    }
                } else {
                    let endpoint = format!(
                        "/connect-state/v1/player/command/from/{}/to/{target}",
                        session.device_id()
                    );
                    let command = command_body(&action)?;
                    let intent_id = random_hex()?;
                    let body = encode_command(&command, &intent_id)?;
                    let mut headers = reqwest::header::HeaderMap::new();
                    headers.insert(
                        reqwest::header::CONTENT_TYPE,
                        reqwest::header::HeaderValue::from_static("application/json"),
                    );
                    headers.insert(
                        reqwest::header::CONTENT_ENCODING,
                        reqwest::header::HeaderValue::from_static("gzip"),
                    );
                    session
                        .spclient()
                        .request(&Method::POST, &endpoint, Some(headers), Some(&body))
                        .await
                }
                .map_err(|error| {
                    anyhow::anyhow!(
                        "Spotify playback command failed ({}). Refresh devices and retry.",
                        error.kind
                    )
                })?;
                Ok(())
            };
            tokio::time::timeout(Duration::from_secs(15), request)
                .await
                .map_err(|_| anyhow::anyhow!("Spotify playback command timed out"))?
        })
    }
}

/// Spotify Connect volume runs from 0 to 65535.
fn remote_volume(percent: u8) -> i32 {
    i32::from(percent.min(100)) * 65535 / 100
}

fn random_hex() -> Result<String> {
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce)
        .map_err(|_| anyhow::anyhow!("Cannot prepare Spotify playback command"))?;
    Ok(nonce.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// A playback command as a Jam's members send it to the Jam, which passes it
/// on to everyone in it, as Spotify's apps do. Volume and moving playback
/// between devices stay with each device; `None` for those.
const LISTED_BEFORE: usize = 50;
const LISTED_SONGS: usize = 1000;

pub(crate) fn jam_command(action: &PlayerAction) -> Option<Value> {
    let mut command = match action {
        // Songs from a playlist or album start in it, so the Jam plays on
        // through it.
        PlayerAction::PlayUris {
            uris,
            offset,
            context: Some(context),
        } => {
            let track = uris.get(*offset)?;
            valid_track(track).ok()?;
            let mut command = command_body(&PlayerAction::PlayContext {
                context: context.clone(),
                offset: Some(track.clone()),
            })
            .ok()?;
            // The list's own order, such as a Random sort, for a Lyssna host.
            // A window of it keeps the command small.
            let start = offset.saturating_sub(LISTED_BEFORE);
            let end = (start + LISTED_SONGS).min(uris.len());
            command["context"]["metadata"][librespot_connect::LISTED_CONTEXT] = json!("true");
            command["context"]["pages"] = json!([{
                "tracks": uris[start..end].iter().map(|uri| json!({"uri": uri})).collect::<Vec<_>>()
            }]);
            command["options"]["skip_to"]["track_index"] = json!(offset - start);
            command
        }
        PlayerAction::Volume(_) => return None,
        action => command_body(action).ok()?,
    };
    command["logging_params"] = json!({ "command_id": random_hex().ok()? });
    Some(command)
}

fn encode_command(command: &Value, intent_id: &str) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(
        &json!({"command":command,"connection_type":"wlan","intent_id":intent_id}),
    )?;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&bytes)?;
    Ok(encoder.finish()?)
}

fn command_body(action: &PlayerAction) -> Result<Value> {
    let mut value = match action {
        PlayerAction::Pause => json!({"endpoint":"pause"}),
        PlayerAction::Play(None) => json!({"endpoint":"resume"}),
        PlayerAction::Next => json!({"endpoint":"skip_next"}),
        PlayerAction::SkipTo(uri) => {
            valid_track(uri)?;
            json!({"endpoint":"skip_next","track":{"uri":uri}})
        }
        PlayerAction::Previous => json!({"endpoint":"skip_prev"}),
        PlayerAction::Seek(position) => {
            json!({"endpoint":"seek_to","value":position,"position":position})
        }
        PlayerAction::Shuffle(value) => json!({"endpoint":"set_shuffling_context","value":value}),
        PlayerAction::Repeat(value) => {
            ensure!(
                ["off", "context", "track"].contains(&value.as_str()),
                "Invalid repeat state"
            );
            json!({"endpoint":"set_options","repeating_context":value=="context","repeating_track":value=="track"})
        }
        PlayerAction::RemoveFromQueue(_) | PlayerAction::ClearQueue => {
            anyhow::bail!(crate::api::QUEUE_EDIT_LOCAL)
        }
        PlayerAction::HostJam(_) => anyhow::bail!("Only this computer's player hosts a Jam"),
        PlayerAction::Enqueue(uri) => {
            valid_track(uri)?;
            json!({"endpoint":"add_to_queue","track":{"uri":uri,"provider":"queue","metadata":{"is_queued":"true"}}})
        }
        PlayerAction::Play(Some(uri)) => {
            valid_track(uri)?;
            play_command(
                json!({"uri":uri,"pages":[{"tracks":[{"uri":uri}]}]}),
                json!({"skip_to":{"track_uri":uri}}),
            )
        }
        PlayerAction::PlayContext { context, offset } => {
            ensure!(
                context.starts_with("spotify:")
                    && context.len() <= 512
                    && !context.chars().any(char::is_control),
                "Invalid Spotify context"
            );
            // Without a context url Spotify rejects the command for Alexa
            // speakers with 403 `command_not_allowed`.
            play_command(
                json!({"uri":context,"url":format!("context://{context}")}),
                json!({"skip_to":{"track_uri":offset}}),
            )
        }
        PlayerAction::PlayUris { uris, offset, .. } => {
            ensure!(
                !uris.is_empty() && uris.len() <= 5000 && *offset < uris.len(),
                "Invalid Spotify track selection"
            );
            for uri in uris {
                valid_track(uri)?;
            }
            play_command(
                json!({"uri":"","pages":[{"tracks":uris.iter().map(|uri|json!({"uri":uri})).collect::<Vec<_>>()}]}),
                json!({"skip_to":{"track_index":offset}}),
            )
        }
        _ => anyhow::bail!("This playback action requires this computer's native player"),
    };
    value["logging_params"] = json!({});
    Ok(value)
}

fn valid_track(uri: &str) -> Result<()> {
    ensure!(uri.strip_prefix("spotify:track:").is_some_and(|id|id.len()==22 && id.bytes().all(|byte|byte.is_ascii_alphanumeric())), "Invalid Spotify track");
    Ok(())
}

fn play_command(context: Value, options: Value) -> Value {
    json!({"endpoint":"play","context":context,"options":options,"play_origin":{"feature_identifier":"lyssna"}})
}

/// This computer's time on Spotify's clock, which stamps remote positions.
fn server_now_ms(local_ms: i64, offset_ms: Option<i64>) -> i64 {
    local_ms.saturating_sub(offset_ms.unwrap_or_default())
}

fn playback_from_state(
    state: &librespot_protocol::player::PlayerState,
    device: Device,
    track: Option<Track>,
    now: i64,
) -> Playback {
    let playing = state.is_playing && !state.is_paused && !state.is_buffering;
    let elapsed = if playing && state.timestamp > 0 {
        now.saturating_sub(state.timestamp).max(0)
    } else {
        0
    };
    let position = state
        .position_as_of_timestamp
        .saturating_add(elapsed)
        .max(0) as u64;
    let duration = track
        .as_ref()
        .map(|track| track.duration_ms)
        .unwrap_or(state.duration.max(0) as u64);
    let options = state.options.as_ref();
    Playback {
        item: track,
        device,
        is_playing: playing,
        progress_ms: Some(if duration > 0 {
            position.min(duration)
        } else {
            position
        }),
        shuffle_state: options.is_some_and(|value| value.shuffling_context),
        repeat_state: if options.is_some_and(|value| value.repeating_track) {
            "track"
        } else if options.is_some_and(|value| value.repeating_context) {
            "context"
        } else {
            "off"
        }
        .into(),
        context: (!state.context_uri.is_empty()).then(|| state.context_uri.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_volume_spans_connect_range() {
        assert_eq!(remote_volume(0), 0);
        assert_eq!(remote_volume(50), 32767);
        assert_eq!(remote_volume(100), 65535);
        assert_eq!(remote_volume(250), 65535);
    }
    #[test]
    fn commands_match_librespot_dealer_protocol() {
        let uri = "spotify:track:0123456789abcdefghijkl";
        for action in [
            PlayerAction::Pause,
            PlayerAction::Play(None),
            PlayerAction::Next,
            PlayerAction::SkipTo(uri.into()),
            PlayerAction::Previous,
            PlayerAction::Seek(1234),
            PlayerAction::Shuffle(true),
            PlayerAction::Repeat("track".into()),
            PlayerAction::Enqueue(uri.into()),
            PlayerAction::Play(Some(uri.into())),
            PlayerAction::PlayContext {
                context: "spotify:album:0123456789abcdefghijkl".into(),
                offset: None,
            },
            PlayerAction::PlayUris {
                uris: vec![uri.into()],
                offset: 0,
                context: None,
            },
        ] {
            let value = command_body(&action).unwrap();
            let bytes = encode_command(&value, "test-command").unwrap();
            let decoded: Value =
                serde_json::from_reader(flate2::read::GzDecoder::new(bytes.as_slice())).unwrap();
            assert_eq!(decoded["command"], value);
            assert_eq!(decoded["intent_id"], "test-command");
            let parsed: librespot_core::dealer::protocol::Command =
                serde_json::from_value(value).unwrap();
            assert!(!matches!(
                parsed,
                librespot_core::dealer::protocol::Command::Unknown(_)
            ));
        }
        assert!(command_body(&PlayerAction::Enqueue("https://example.test/".into())).is_err());
        // Skipping to a queued song names it, so Spotify keeps the queue.
        assert_eq!(
            command_body(&PlayerAction::SkipTo(uri.into())).unwrap()["track"]["uri"],
            uri
        );
    }
    #[test]
    fn jam_commands_carry_an_id_and_keep_volume_here() {
        let pause = jam_command(&PlayerAction::Pause).unwrap();
        assert_eq!(pause["endpoint"], "pause");
        assert_eq!(
            pause["logging_params"]["command_id"]
                .as_str()
                .unwrap()
                .len(),
            32
        );
        assert!(jam_command(&PlayerAction::Volume(40)).is_none());
        assert!(jam_command(&PlayerAction::Transfer("device".into())).is_none());
        assert!(jam_command(&PlayerAction::ClearQueue).is_none());

        let track = "spotify:track:0000000000000000000001";
        let play = jam_command(&PlayerAction::PlayUris {
            uris: vec!["spotify:track:0000000000000000000000".into(), track.into()],
            offset: 1,
            context: Some("spotify:playlist:abc".into()),
        })
        .unwrap();
        assert_eq!(play["endpoint"], "play");
        assert_eq!(play["context"]["uri"], "spotify:playlist:abc");
        assert_eq!(play["options"]["skip_to"]["track_uri"], track);
        // A Lyssna host keeps the list's order, such as a Random sort.
        assert_eq!(play["context"]["metadata"]["lyssna.listed"], "true");
        assert_eq!(play["context"]["pages"][0]["tracks"][1]["uri"], track);
        assert_eq!(play["options"]["skip_to"]["track_index"], 1);
    }

    #[test]
    fn a_long_listed_jam_play_sends_a_window_around_the_song() {
        let uris: Vec<String> = (0..3000)
            .map(|index| format!("spotify:track:{index:022}"))
            .collect();
        let play = jam_command(&PlayerAction::PlayUris {
            uris: uris.clone(),
            offset: 2000,
            context: Some("spotify:playlist:abc".into()),
        })
        .unwrap();
        let tracks = play["context"]["pages"][0]["tracks"].as_array().unwrap();
        assert_eq!(tracks.len(), LISTED_SONGS);
        let index = play["options"]["skip_to"]["track_index"].as_u64().unwrap() as usize;
        assert_eq!(index, LISTED_BEFORE);
        assert_eq!(tracks[index]["uri"], uris[2000]);
    }

    #[test]
    fn context_play_carries_the_url_alexa_speakers_require() {
        // Live: Spotify answered 403 `command_not_allowed` for an Echo until
        // the context named its url.
        let value = command_body(&PlayerAction::PlayContext {
            context: "spotify:album:0123456789abcdefghijkl".into(),
            offset: Some("spotify:track:0123456789abcdefghijkl".into()),
        })
        .unwrap();
        assert_eq!(
            value["context"]["url"],
            "context://spotify:album:0123456789abcdefghijkl"
        );
        assert_eq!(
            value["options"]["skip_to"]["track_uri"],
            "spotify:track:0123456789abcdefghijkl"
        );
    }
    #[test]
    fn remote_clock_holds_when_paused_and_clamps_to_duration() {
        let mut state = librespot_protocol::player::PlayerState {
            timestamp: 1000,
            position_as_of_timestamp: 100,
            duration: 2000,
            is_playing: true,
            ..Default::default()
        };
        assert_eq!(
            playback_from_state(&state, Device::default(), None, 2000).progress_ms,
            Some(1100)
        );
        state.is_paused = true;
        assert_eq!(
            playback_from_state(&state, Device::default(), None, 5000).progress_ms,
            Some(100)
        );
        state.is_paused = false;
        assert_eq!(
            playback_from_state(&state, Device::default(), None, 5000).progress_ms,
            Some(2000)
        );
    }
    #[test]
    fn remote_progress_uses_spotify_clock_despite_local_skew() {
        // Live sample: this PC ran about 2 s ahead, so a playing Echo read
        // 103.9 s and then 102.1 s once paused.
        let state = librespot_protocol::player::PlayerState {
            timestamp: 1_000_000,
            position_as_of_timestamp: 100_000,
            duration: 240_000,
            is_playing: true,
            ..Default::default()
        };
        let local = 1_000_000 + 3_000 + 2_000;
        let now = server_now_ms(local, Some(2_000));
        assert_eq!(
            playback_from_state(&state, Device::default(), None, now).progress_ms,
            Some(103_000)
        );
        assert_eq!(server_now_ms(local, None), local);
    }
}
