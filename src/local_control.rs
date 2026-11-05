//! Request/reply playback sessions on the authenticated local control connection.
use super::{DeckState, DeckTrack, Shared, cover, read_state};
use crate::desktop::{Desktop, PlayerRequest, lock, player};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
use tauri::Manager;

struct SavedPlayback {
    state: DeckState,
    at: Instant,
}

pub(super) struct Session<'a> {
    app: &'a tauri::AppHandle,
    shared: &'a Mutex<Shared>,
    id: u64,
    outputs: Vec<crate::audio::AudioBuffer>,
    calibration: Option<SavedPlayback>,
    gameplay: bool,
    loaded: Option<String>,
}

impl<'a> Session<'a> {
    pub(super) fn new(app: &'a tauri::AppHandle, shared: &'a Mutex<Shared>, id: u64) -> Self {
        Self {
            app,
            shared,
            id,
            outputs: Vec::new(),
            calibration: None,
            gameplay: false,
            loaded: None,
        }
    }

    pub(super) fn execute(&mut self, request: &Value) -> Value {
        let id = request["id"].as_u64().unwrap_or(0);
        let result = if id == 0 {
            Err(anyhow::anyhow!("Request ID missing"))
        } else {
            self.action(request)
        };
        match result {
            Ok(data) => {
                json!({"type":"response", "id":id, "ok":true, "data":data, "state":self.state().ok()})
            }
            Err(error) => {
                match request["action"].as_str() {
                    Some("play" | "prepare") => {
                        let _ = self.pause_owned();
                        self.release_audio();
                    }
                    Some("calibrate-start") => {
                        let _ = self.finish_calibration();
                    }
                    _ => {}
                }
                json!({"type":"response", "id":id, "ok":false, "error":error.to_string(), "state":self.state().ok()})
            }
        }
    }

    fn state(&self) -> Result<DeckState> {
        let desktop = self
            .app
            .try_state::<Desktop>()
            .context("Player unavailable")?;
        let snapshot = lock(&desktop.0.snapshot)?;
        let mut state = read_state(&snapshot);
        let now = crate::desktop::now_ms();
        if state.playing {
            state.position_ms = state
                .position_ms
                .saturating_add(now.saturating_sub(state.observed_at));
            if let Some(track) = &state.track
                && track.duration_ms > 0
            {
                state.position_ms = state.position_ms.min(track.duration_ms);
            }
        }
        state.observed_at = now;
        Ok(state)
    }

    fn control(&self, command: &str, uri: Option<&str>, position: u64) -> Result<()> {
        let desktop = self
            .app
            .try_state::<Desktop>()
            .context("Player unavailable")?;
        ensure!(
            lock(&desktop.0.jam_commands)?.is_none(),
            "Leave the Jam before using local playback control"
        );
        let request = PlayerRequest {
            command: command.to_owned(),
            uri: uri.map(str::to_owned),
            position_ms: Some(position),
            ..Default::default()
        };
        player(self.app.clone(), request, Some("native".into()), desktop)
            .map_err(anyhow::Error::msg)
    }

    fn wait_for(&self, uri: &str, playing: bool) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            let state = self.state()?;
            if state.track.as_ref().is_some_and(|track| track.uri == uri)
                && state.playing == playing
            {
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "Local playback did not reach the requested track"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn temporary_volume(&mut self, percent: u8) -> Result<()> {
        let desktop = self
            .app
            .try_state::<Desktop>()
            .context("Player unavailable")?;
        let output = desktop.0.controls.audio_buffer()?;
        let mut shared = lock(self.shared)?;
        ensure!(
            shared.audio_owner.is_none_or(|owner| owner == self.id),
            "Another local client owns temporary audio control"
        );
        shared.audio_owner = Some(self.id);
        if !self
            .outputs
            .iter()
            .any(|existing| existing.same_output(&output))
        {
            self.outputs.push(output);
        }
        for output in &self.outputs {
            output.set_temporary_volume(Some(percent));
        }
        Ok(())
    }

    fn release_audio(&mut self) {
        for output in self.outputs.drain(..) {
            output.set_temporary_volume(None);
        }
        if let Ok(mut shared) = lock(self.shared)
            && shared.audio_owner == Some(self.id)
        {
            shared.audio_owner = None;
        }
        self.gameplay = false;
    }

    fn pause_owned(&self) -> Result<()> {
        if self.gameplay
            && self.state()?.track.as_ref().map(|track| track.uri.as_str())
                == self.loaded.as_deref()
        {
            self.control("pause", None, 0)?;
            if let Some(uri) = &self.loaded {
                self.wait_for(uri, false)?;
            }
        }
        Ok(())
    }

    fn finish_calibration(&mut self) -> Result<()> {
        let Some(saved) = self.calibration.take() else {
            return Ok(());
        };
        let restore = (|| -> Result<()> {
            // A manual selection made during calibration belongs to the user.
            if self.state()?.track.as_ref().map(|track| track.uri.as_str())
                != self.loaded.as_deref()
            {
                return Ok(());
            }
            self.temporary_volume(0)?;
            if let Some(track) = &saved.state.track {
                self.control("play", Some(&track.uri), 0)?;
                self.wait_for(&track.uri, true)?;
                let elapsed = if saved.state.playing {
                    saved.at.elapsed().as_millis() as u64
                } else {
                    0
                };
                self.control(
                    "seek",
                    None,
                    saved.state.position_ms.saturating_add(elapsed),
                )?;
            }
            if !saved.state.playing || saved.state.track.is_none() {
                self.control("pause", None, 0)?;
                if let Some(track) = &saved.state.track {
                    self.wait_for(&track.uri, false)?;
                }
            }
            thread::sleep(Duration::from_millis(200));
            Ok(())
        })();
        if restore.is_err() {
            let _ = self.control("pause", None, 0);
            thread::sleep(Duration::from_millis(200));
        }
        self.release_audio();
        restore
    }

    fn action(&mut self, request: &Value) -> Result<Value> {
        let action = request["action"].as_str().context("Action missing")?;
        if action == "audio-analysis" {
            let uri = track_uri(request)?;
            let desktop = self
                .app
                .try_state::<Desktop>()
                .context("Player unavailable")?;
            let session = desktop.0.controls.session()?;
            let envelope = librespot_playback::player::audio_envelope(
                session,
                librespot_core::SpotifyUri::from_uri(uri)?,
            )?;
            return Ok(json!({"uri":uri, "startMs":envelope.start_ms,
                "stepMs":envelope.step_ms, "durationMs":envelope.duration_ms, "rms":envelope.rms}));
        }
        if matches!(action, "search" | "track") {
            let desktop = self
                .app
                .try_state::<Desktop>()
                .context("Player unavailable")?;
            let (reply, response) = mpsc::channel();
            let command = if action == "search" {
                let query = request["query"]
                    .as_str()
                    .context("Search query missing")?
                    .trim();
                ensure!(
                    !query.is_empty() && query.len() <= 1024,
                    "Search query is empty or too long"
                );
                let limit = request["limit"].as_u64().unwrap_or(20).clamp(1, 20) as usize;
                crate::worker::Command::LocalSearch {
                    query: query.to_owned(),
                    limit,
                    reply,
                }
            } else {
                crate::worker::Command::LocalTrack {
                    uri: track_uri(request)?.to_owned(),
                    reply,
                }
            };
            desktop.0.commands.send(command)?;
            let tracks = response
                .recv_timeout(Duration::from_secs(15))
                .context("Track lookup timed out")?
                .map_err(anyhow::Error::msg)?;
            return if action == "search" {
                Ok(Value::Array(
                    tracks
                        .iter()
                        .filter(|track| valid_uri(&track.uri))
                        .map(track_value)
                        .collect(),
                ))
            } else {
                tracks.first().map(track_value).context("Track unavailable")
            };
        }
        ensure!(
            lock(self.shared)?
                .audio_owner
                .is_none_or(|owner| owner == self.id),
            "Another local client owns temporary audio control"
        );
        let position = request["positionMs"]
            .as_u64()
            .unwrap_or(0)
            .min(i32::MAX as u64);
        match action {
            "prepare" => {
                let uri = track_uri(request)?.to_owned();
                self.finish_calibration()?;
                self.temporary_volume(0)?;
                self.gameplay = true;
                self.loaded = Some(uri.clone());
                self.control("play", Some(&uri), 0)?;
                self.wait_for(&uri, true)?;
                self.control("pause", None, 0)?;
                self.control("seek", None, 0)?;
            }
            "play" => {
                let uri = track_uri(request)?.to_owned();
                self.calibration = None;
                if request["quiet"].as_bool().unwrap_or(false) {
                    self.temporary_volume(0)?;
                    self.gameplay = true;
                } else {
                    self.release_audio();
                }
                self.loaded = Some(uri.clone());
                let current = self.state()?;
                if current.track.as_ref().is_none_or(|track| track.uri != uri) {
                    self.control("play", Some(&uri), 0)?;
                } else if !current.playing {
                    self.control("play", None, 0)?;
                }
                self.wait_for(&uri, true)?;
                self.control("seek", None, position)?;
            }
            "seek" => {
                let uri = track_uri(request)?;
                let state = self.state()?;
                ensure!(
                    state.playing && state.track.as_ref().is_some_and(|track| track.uri == uri),
                    "Requested track is not playing"
                );
                self.control("seek", None, position)?;
            }
            "pause" => {
                let result = self.control("pause", None, 0);
                self.calibration = None;
                thread::sleep(Duration::from_millis(200));
                self.release_audio();
                result?;
            }
            "release" => {
                if self.calibration.is_some() {
                    self.finish_calibration()?;
                } else {
                    self.release_audio();
                }
            }
            "calibrate-start" => {
                let uri = track_uri(request)?.to_owned();
                ensure!(self.calibration.is_none(), "Calibration already active");
                let saved = SavedPlayback {
                    state: self.state()?,
                    at: Instant::now(),
                };
                self.temporary_volume(1)?;
                self.calibration = Some(saved);
                self.loaded = Some(uri.clone());
                self.control("play", Some(&uri), 0)?;
                self.wait_for(&uri, true)?;
                self.control("seek", None, 0)?;
            }
            "calibrate-quiet" => {
                ensure!(self.calibration.is_some(), "Calibration is not active");
                ensure!(
                    self.state()?.track.as_ref().is_some_and(
                        |track| track.uri == request["uri"].as_str().unwrap_or_default()
                    ),
                    "Calibration track changed"
                );
                self.temporary_volume(1)?;
            }
            "calibrate-stop" => self.finish_calibration()?,
            _ => bail!("Unknown local playback action"),
        }
        if matches!(
            action,
            "play" | "seek" | "pause" | "prepare" | "calibrate-start" | "calibrate-stop"
        ) {
            thread::sleep(Duration::from_millis(200));
        }
        Ok(Value::Null)
    }
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        let _ = self.finish_calibration();
        let _ = self.pause_owned();
        self.release_audio();
    }
}

fn valid_uri(uri: &str) -> bool {
    uri.strip_prefix("spotify:track:")
        .is_some_and(|id| id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}

fn track_uri(request: &Value) -> Result<&str> {
    let uri = request["uri"].as_str().context("Track URI missing")?;
    ensure!(valid_uri(uri), "Invalid Spotify track URI");
    Ok(uri)
}

fn track_value(track: &crate::model::Track) -> Value {
    serde_json::to_value(DeckTrack {
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
    })
    .unwrap_or(Value::Null)
}
