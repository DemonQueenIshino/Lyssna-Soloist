use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context, Result, bail, ensure};
use reqwest::{Method, StatusCode, blocking::Client};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use url::Url;

use crate::{
    auth::Tokens,
    model::{
        Album, Artist, BrowseInfo, Device, Playback, Playlist, Profile, Track, unique_additions,
    },
};

#[derive(Clone)]
pub struct Spotify {
    #[cfg(test)]
    request_probe: Option<Arc<std::sync::atomic::AtomicUsize>>,
    pub client: Client,
    pub client_id: String,
    pub tokens: Tokens,
    backoff: crate::api_backoff::Backoff,
    active: Arc<AtomicBool>,
    background: bool,
}

impl Spotify {
    pub fn new(client: Client, client_id: String, tokens: Tokens) -> Self {
        let backoff = crate::api_backoff::Backoff::new(&client_id);
        Self {
            #[cfg(test)]
            request_probe: None,
            client,
            client_id,
            tokens,
            backoff,
            active: Arc::new(AtomicBool::new(true)),
            background: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn count_network_attempts(&mut self) -> Arc<std::sync::atomic::AtomicUsize> {
        let probe = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        self.request_probe = Some(probe.clone());
        probe
    }
    pub fn background(&self) -> Self {
        let mut snapshot = self.clone();
        snapshot.background = true;
        snapshot
    }
    pub fn cooling_down(&self) -> bool {
        self.backoff.remaining() > 0
    }
    pub fn cancel_background(&self) {
        self.active.store(false, Ordering::Release);
    }
    fn request(
        &mut self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Value>,
    ) -> Result<Value> {
        #[cfg(test)]
        if let Some(probe) = &self.request_probe {
            probe.fetch_add(1, Ordering::Relaxed);
            bail!("Network disabled by test request probe");
        }
        ensure!(
            self.active.load(Ordering::Acquire),
            "Spotify session signed out"
        );
        self.backoff.check()?;
        if self.background {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs();
            ensure!(
                self.tokens.expires_at > now,
                "Background Spotify session expired"
            );
        } else {
            self.tokens
                .refresh_if_needed(&self.client, &self.client_id)?;
        }
        let url = if path.starts_with("https://") {
            Url::parse(path)?
        } else {
            Url::parse(&format!("https://api.spotify.com/v1/{path}"))?
        };
        ensure!(
            url.scheme() == "https"
                && url.host_str() == Some("api.spotify.com")
                && url.port_or_known_default() == Some(443)
                && url.path().starts_with("/v1/")
                && url.username().is_empty()
                && url.password().is_none(),
            "Rejected unexpected Spotify API URL"
        );
        for attempt in 0..2 {
            let mut request = self
                .client
                .request(method.clone(), url.clone())
                .query(query)
                .bearer_auth(&self.tokens.access_token);
            if let Some(body) = &body {
                request = request.json(body);
            } else if matches!(method, Method::PUT | Method::POST | Method::DELETE) {
                request = request.header(reqwest::header::CONTENT_LENGTH, "0");
            }
            let response = request
                .send()
                .map_err(|_| anyhow::anyhow!("Cannot reach Spotify. Check your connection."))?;
            match response.status() {
                StatusCode::UNAUTHORIZED if attempt == 0 && !self.background => {
                    self.tokens.expires_at = 0;
                    self.tokens
                        .refresh_if_needed(&self.client, &self.client_id)?;
                    continue;
                }
                StatusCode::NO_CONTENT => return Ok(Value::Null),
                StatusCode::TOO_MANY_REQUESTS => {
                    let seconds = response
                        .headers()
                        .get("retry-after")
                        .and_then(|header| header.to_str().ok())
                        .and_then(|value| value.parse::<u64>().ok())
                        .unwrap_or(30)
                        .max(1);
                    self.backoff.pause(seconds);
                    return Err(crate::api_backoff::CoolingDown(seconds).into());
                }
                StatusCode::FORBIDDEN => bail!(
                    "Spotify denied this action. Check Premium, Developer Dashboard access, and playlist ownership."
                ),
                StatusCode::NOT_FOUND => bail!(
                    "Spotify could not find this item or an active playback device. Select a device and retry."
                ),
                status if !status.is_success() => {
                    bail!("Spotify request failed (HTTP {})", status.as_u16())
                }
                _ => {}
            }
            let bytes = response.bytes()?;
            return decode_response(&method, url.path(), &bytes);
        }
        bail!("Spotify login expired. Sign in again.")
    }

    fn get<T: DeserializeOwned>(&mut self, path: &str, query: &[(&str, String)]) -> Result<T> {
        serde_json::from_value(self.request(Method::GET, path, query, None)?)
            .context("Unexpected Spotify response format")
    }

    pub fn profile(&mut self) -> Result<Profile> {
        self.get("me", &[])
    }

    pub fn playback(&mut self) -> Result<Playback> {
        let response = self.request(Method::GET, "me/player", &[], None)?;
        if response.is_null() {
            return Ok(Playback::default());
        }
        serde_json::from_value(response).context("Cannot read Spotify playback state")
    }

    pub fn devices(&mut self) -> Result<Vec<Device>> {
        let response = self.request(Method::GET, "me/player/devices", &[], None)?;
        Ok(serde_json::from_value(response["devices"].clone())?)
    }

    pub fn playlists(&mut self) -> Result<Vec<Playlist>> {
        playlist_pages(|path| self.request(Method::GET, path, &[], None))
    }

    pub fn search(&mut self, text: &str) -> Result<Vec<Track>> {
        let response = self.request(
            Method::GET,
            "search",
            &[
                ("q", text.to_owned()),
                ("type", "track".into()),
                ("limit", "10".into()),
            ],
            None,
        )?;
        Ok(serde_json::from_value(response["tracks"]["items"].clone())?)
    }

    pub fn search_playlists(&mut self, text: &str) -> Result<Vec<Playlist>> {
        let response = self.request(
            Method::GET,
            "search",
            &[
                ("q", text.to_owned()),
                ("type", "playlist".into()),
                ("limit", "10".into()),
            ],
            None,
        )?;
        crate::search::parse_public(&response)
    }
    pub fn search_catalogue(&mut self, text: &str) -> Result<crate::search::SearchResults> {
        let response = self.request(
            Method::GET,
            "search",
            &[
                ("q", text.to_owned()),
                ("type", "track,artist,album,playlist".into()),
                ("limit", "10".into()),
            ],
            None,
        )?;
        crate::search::parse_public_catalogue(&response)
    }

    pub fn saved_tracks(&mut self, offset: usize) -> Result<Vec<Track>> {
        let response = self.request(
            Method::GET,
            "me/tracks",
            &[("limit", "50".into()), ("offset", offset.to_string())],
            None,
        )?;
        parse_track_items(&response)
    }

    /// Listening history, newest first. `added_at` carries the play time and
    /// `played_context` the album, playlist or artist URI it played from.
    pub fn recent(&mut self) -> Result<Vec<Track>> {
        let response = self.request(
            Method::GET,
            "me/player/recently-played",
            &[("limit", "50".into())],
            None,
        )?;
        let mut tracks = parse_track_items(&response)?;
        let items = response["items"]
            .as_array()
            .context("Track page has no items")?;
        let played = items.iter().filter(|item| {
            let track = item
                .get("item")
                .or_else(|| item.get("track"))
                .unwrap_or(&Value::Null);
            !track.is_null() && track["type"].as_str().is_none_or(|kind| kind == "track")
        });
        for (track, item) in tracks.iter_mut().zip(played) {
            track.added_at = item["played_at"].as_str().unwrap_or_default().into();
            track.played_context = item["context"]["uri"].as_str().unwrap_or_default().into();
        }
        Ok(tracks)
    }
    pub fn last_played(&mut self) -> Result<Option<crate::resume::LastPlayed>> {
        let response = self.request(
            Method::GET,
            "me/player/recently-played",
            &[("limit", "1".into())],
            None,
        )?;
        let Some(item) = response["items"].as_array().and_then(|items| items.first()) else {
            return Ok(None);
        };
        let track = serde_json::from_value(
            item.get("track")
                .or_else(|| item.get("item"))
                .context("Missing recent track")?
                .clone(),
        )?;
        let played_at = time::OffsetDateTime::parse(
            item["played_at"]
                .as_str()
                .context("Missing playback date")?,
            &time::format_description::well_known::Rfc3339,
        )?
        .unix_timestamp_nanos()
            / 1_000_000;
        Ok(Some(crate::resume::LastPlayed {
            dj_context: item["context"]["uri"]
                .as_str()
                .filter(|uri| *uri == crate::home::DJ_URI || uri.starts_with("spotify:lexicon:"))
                .map(|uri| crate::resume::DJContext {
                    uri: uri.into(),
                    session_url: None,
                    track_uid: None,
                }),
            context: item["context"]["uri"]
                .as_str()
                .filter(|uri| crate::resume::valid_context(uri))
                .map(Into::into),
            queued: Vec::new(),
            queue: Vec::new(),
            track,
            position_ms: 0,
            played_at: played_at.max(0) as u64,
        }))
    }

    pub fn playlist_tracks(&mut self, playlist_id: &str, offset: usize) -> Result<Vec<Track>> {
        ensure!(valid_id(playlist_id), "Invalid playlist ID");
        let response = self.request(
            Method::GET,
            &format!("playlists/{playlist_id}/items"),
            &[("limit", "50".into()), ("offset", offset.to_string())],
            None,
        )?;
        parse_track_items(&response)
    }

    pub fn queue(&mut self) -> Result<Vec<Track>> {
        let response = self.request(Method::GET, "me/player/queue", &[], None)?;
        let mut tracks = Vec::new();
        for item in response["queue"].as_array().context("Queue has no items")? {
            if item["type"].as_str() == Some("track") {
                tracks.push(serde_json::from_value(item.clone())?);
            }
        }
        Ok(tracks)
    }

    pub fn artist(&mut self, id: &str) -> Result<Artist> {
        ensure!(valid_id(id), "Invalid artist ID");
        self.get(&format!("artists/{id}"), &[])
            .or_else(|_| crate::artist::basic(&self.client, id))
    }

    pub fn track(&mut self, uri: &str) -> Result<Track> {
        let id = uri
            .strip_prefix("spotify:track:")
            .context("Invalid track URI")?;
        ensure!(valid_id(id), "Invalid track ID");
        self.get(&format!("tracks/{id}"), &[])
    }

    pub fn browse_info(&mut self, key: &str) -> Result<BrowseInfo> {
        if let Some(id) = key.strip_prefix("album:") {
            ensure!(valid_id(id), "Invalid album ID");
            let album: Album = self.get(&format!("albums/{id}"), &[])?;
            Ok(BrowseInfo {
                key: key.into(),
                title: album.name.clone(),
                subtitle: album
                    .artists
                    .iter()
                    .map(|artist| artist.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                images: album.images.clone(),
                album: Some(album),
                ..BrowseInfo::default()
            })
        } else if let Some(id) = key.strip_prefix("artist:") {
            let artist = self.artist(id)?;
            let response = self
                .request(
                    Method::GET,
                    &format!("artists/{id}/albums"),
                    &[
                        ("limit", "10".into()),
                        ("include_groups", "album,single".into()),
                    ],
                    None,
                )
                .unwrap_or_else(|_| json!({"items":[],"next":null}));
            let mut albums: Vec<crate::model::Album> =
                serde_json::from_value(response["items"].clone()).unwrap_or_default();
            let mut next = response["next"].as_str().map(str::to_owned);
            let mut visited = HashSet::new();
            while let Some(path) = next {
                if !visited.insert(path.clone()) {
                    break;
                }
                let Ok(response) = self.request(Method::GET, &path, &[], None) else {
                    break;
                };
                let Ok(page) =
                    serde_json::from_value::<Vec<crate::model::Album>>(response["items"].clone())
                else {
                    break;
                };
                albums.extend(page);
                next = response["next"].as_str().map(str::to_owned);
            }
            Ok(BrowseInfo {
                key: key.into(),
                title: artist.name.clone(),
                subtitle: "Artist".into(),
                images: artist.images.clone(),
                artist: Some(artist),
                albums,
                ..BrowseInfo::default()
            })
        } else {
            anyhow::bail!("Unknown album or artist page")
        }
    }

    pub fn album_tracks(&mut self, id: &str, offset: usize) -> Result<Vec<Track>> {
        ensure!(valid_id(id), "Invalid album ID");
        let response = self.request(
            Method::GET,
            &format!("albums/{id}/tracks"),
            &[("limit", "50".into()), ("offset", offset.to_string())],
            None,
        )?;
        Ok(serde_json::from_value(response["items"].clone())?)
    }

    pub fn saved_item(&mut self, uri: &str) -> Result<bool> {
        let response = self.request(
            Method::GET,
            "me/library/contains",
            &[("uris", uri.into())],
            None,
        )?;
        response
            .get(0)
            .and_then(Value::as_bool)
            .context("Cannot read library status")
    }

    pub fn set_saved_item(&mut self, uri: &str, saved: bool) -> Result<()> {
        self.request(
            if saved { Method::PUT } else { Method::DELETE },
            "me/library",
            &[("uris", uri.into())],
            None,
        )?;
        Ok(())
    }
    pub fn saved_items(&mut self, uris: &[String]) -> Result<Vec<bool>> {
        ensure!(uris.len() <= 50, "Too many library items");
        let response = self.request(
            Method::GET,
            "me/library/contains",
            &[("uris", uris.join(","))],
            None,
        )?;
        serde_json::from_value(response).context("Cannot read saved songs")
    }

    pub fn player_action(&mut self, action: PlayerAction, device: Option<&str>) -> Result<()> {
        let mut query = Vec::new();
        if let Some(device) = device {
            query.push(("device_id", device.to_owned()));
        }
        let (method, endpoint, body) = match action {
            PlayerAction::Play(uri) => (
                Method::PUT,
                "play",
                Some(uri.map_or(json!({}), |uri| json!({"uris":[uri]}))),
            ),
            PlayerAction::PlayContext { context, offset } => (
                Method::PUT,
                "play",
                Some(match offset {
                    Some(uri) => json!({"context_uri": context, "offset": {"uri": uri}}),
                    None => json!({"context_uri": context}),
                }),
            ),
            PlayerAction::RestoreDJ { .. } | PlayerAction::PreparedDJ { .. } => {
                anyhow::bail!("DJ session restore requires native playback")
            }
            PlayerAction::PlayUris { uris, offset, .. } => (
                Method::PUT,
                "play",
                Some(json!({"uris": uris, "offset": {"position": offset}})),
            ),
            PlayerAction::Resume { .. } => {
                anyhow::bail!("Saved-position resume requires native playback")
            }
            PlayerAction::Pause => (Method::PUT, "pause", None),
            PlayerAction::Next => (Method::POST, "next", None),
            // The Web API cannot skip to a queued song; play it instead.
            PlayerAction::SkipTo(uri) => (Method::PUT, "play", Some(json!({"uris": [uri]}))),
            PlayerAction::Previous => (Method::POST, "previous", None),
            PlayerAction::Seek(position) => {
                query.push(("position_ms", position.to_string()));
                (Method::PUT, "seek", None)
            }
            PlayerAction::Volume(volume) => {
                query.push(("volume_percent", volume.min(100).to_string()));
                (Method::PUT, "volume", None)
            }
            PlayerAction::Shuffle(state) => {
                query.push(("state", state.to_string()));
                (Method::PUT, "shuffle", None)
            }
            PlayerAction::Repeat(state) => {
                ensure!(
                    ["off", "context", "track"].contains(&state.as_str()),
                    "Invalid repeat state"
                );
                query.push(("state", state));
                (Method::PUT, "repeat", None)
            }
            PlayerAction::Enqueue(uri) => {
                query.push(("uri", uri));
                (Method::POST, "queue", None)
            }
            PlayerAction::RemoveFromQueue(_) | PlayerAction::ClearQueue => {
                anyhow::bail!(QUEUE_EDIT_LOCAL)
            }
            PlayerAction::HostJam(_) => anyhow::bail!("Only this computer's player hosts a Jam"),
            PlayerAction::Transfer(id) => {
                self.request(
                    Method::PUT,
                    "me/player",
                    &[],
                    Some(json!({"device_ids":[id],"play":false})),
                )?;
                return Ok(());
            }
        };
        self.request(method, &format!("me/player/{endpoint}"), &query, body)?;
        Ok(())
    }

    pub fn save_track(&mut self, uri: &str) -> Result<()> {
        self.request(Method::PUT, "me/library", &[("uris", uri.to_owned())], None)?;
        Ok(())
    }

    pub fn add_to_playlist(&mut self, playlist_id: &str, tracks: &[Track]) -> Result<usize> {
        ensure!(valid_id(playlist_id), "Invalid playlist ID");
        let mut existing = HashSet::new();
        let mut visited = HashSet::new();
        let mut next = Some(format!(
            "https://api.spotify.com/v1/playlists/{playlist_id}/items?limit=50"
        ));
        while let Some(path) = next {
            ensure!(
                visited.insert(path.clone()),
                "Spotify repeated a playlist page"
            );
            let response = self.request(Method::GET, &path, &[], None)?;
            for track in parse_track_items(&response)? {
                existing.insert(track.uri);
            }
            next = response["next"].as_str().map(str::to_owned);
        }
        let additions = unique_additions(&existing, tracks);
        let mut added = 0;
        for batch in additions.chunks(100) {
            if let Err(error) = self.request(
                Method::POST,
                &format!("playlists/{playlist_id}/items"),
                &[],
                Some(json!({"uris": batch})),
            ) {
                bail!(
                    "Added {added} tracks before stopping: {error}. Retry safely; existing tracks are skipped."
                );
            }
            added += batch.len();
        }
        Ok(added)
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn decode_response(method: &Method, path: &str, bytes: &[u8]) -> Result<Value> {
    if bytes.iter().all(u8::is_ascii_whitespace)
        || (*method != Method::GET
            && (path == "/v1/me/player" || path.starts_with("/v1/me/player/")))
    {
        return Ok(Value::Null);
    }
    serde_json::from_slice(bytes).context("Spotify returned invalid JSON")
}

fn playlist_pages(mut fetch: impl FnMut(&str) -> Result<Value>) -> Result<Vec<Playlist>> {
    let mut playlists = Vec::new();
    let mut next = Some("https://api.spotify.com/v1/me/playlists?limit=50".to_owned());
    let mut visited = HashSet::new();
    let mut ids = HashSet::new();
    while let Some(path) = next {
        ensure!(
            visited.insert(path.clone()),
            "Spotify repeated a playlist page"
        );
        let page = fetch(&path)?;
        for item in page["items"]
            .as_array()
            .context("Playlist page has no items")?
        {
            if !item.is_null() {
                let playlist: Playlist = serde_json::from_value(item.clone())?;
                if ids.insert(playlist.id.clone()) {
                    playlists.push(playlist);
                }
            }
        }
        next = page["next"].as_str().map(str::to_owned);
    }
    Ok(playlists)
}

fn parse_track_items(response: &Value) -> Result<Vec<Track>> {
    let mut tracks = Vec::new();
    for item in response["items"]
        .as_array()
        .context("Track page has no items")?
    {
        let track = item
            .get("item")
            .or_else(|| item.get("track"))
            .unwrap_or(&Value::Null);
        if track.is_null() || track["type"].as_str().is_some_and(|kind| kind != "track") {
            continue;
        }
        let mut track: Track =
            serde_json::from_value(track.clone()).context("Invalid track in playlist")?;
        track.added_at = item["added_at"].as_str().unwrap_or_default().into();
        tracks.push(track);
    }
    Ok(tracks)
}

/// Spotify's Web API can add to the queue but not take songs out of it.
pub(crate) const QUEUE_EDIT_LOCAL: &str =
    "Removing from the queue works while songs play on this computer.";

#[derive(Clone)]
pub enum PlayerAction {
    PreparedDJ {
        context: Box<librespot_protocol::context::Context>,
        provider: std::sync::Arc<dyn librespot_connect::ContextPageProvider>,
        cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    },
    /// Load the saved queue directly at its listening position.
    Resume {
        uris: Vec<String>,
        offset: usize,
        position_ms: u64,
        /// The saved playlist, album or Liked Songs to carry on through.
        context: Option<String>,
        /// Songs the listener had queued, added back after loading.
        queued: Vec<String>,
        /// `uris` is the whole list in the order it was played from (such as
        /// a Random sort), so the context plays in that order.
        listed: bool,
    },
    /// Restore a paged DJ session before resuming or advancing its queue.
    RestoreDJ {
        context: String,
        session_url: Option<String>,
        track: String,
        track_uid: Option<String>,
        position_ms: u64,
        relative_offset: i32,
    },
    Play(Option<String>),
    /// Start a playlist or album context, optionally at a track URI.
    PlayContext {
        context: String,
        offset: Option<String>,
    },
    /// Play a list of track URIs starting at `offset`.
    PlayUris {
        uris: Vec<String>,
        offset: usize,
        /// The playlist, album or collection these songs are, already loaded:
        /// this computer plays them under it without asking Spotify for it.
        context: Option<String>,
    },
    Pause,
    Next,
    /// Skip forward to a song coming up in the queue, keeping the rest.
    SkipTo(String),
    Previous,
    Seek(u64),
    Volume(u8),
    Shuffle(bool),
    Repeat(String),
    Enqueue(String),
    /// Remove a song the listener queued; songs from the context stay.
    RemoveFromQueue(String),
    /// Remove every song the listener queued.
    ClearQueue,
    Transfer(String),
    /// This computer hosts the Jam with this session ID, or no Jam.
    HostJam(Option<String>),
}

impl PlayerAction {
    pub fn supports_native(&self) -> bool {
        !matches!(self, Self::Transfer(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_catalogue_requests_cannot_refresh_or_outlive_signout() {
        let tokens = Tokens {
            access_token: "test".into(),
            refresh_token: "test".into(),
            expires_at: 0,
            scopes: String::new(),
            profile: None,
        };
        let spotify = Spotify::new(Client::new(), "test".into(), tokens);
        assert!(
            spotify
                .background()
                .profile()
                .err()
                .unwrap()
                .to_string()
                .contains("session expired")
        );
        let mut background = spotify.background();
        spotify.cancel_background();
        assert!(
            background
                .profile()
                .err()
                .unwrap()
                .to_string()
                .contains("signed out")
        );
    }
    #[test]
    fn successful_player_commands_do_not_require_json_bodies() {
        for body in [&b""[..], &b" \r\n"[..], &b"OK"[..]] {
            assert!(
                decode_response(&Method::POST, "/v1/me/player/queue", body)
                    .unwrap()
                    .is_null()
            );
        }
        assert!(decode_response(&Method::GET, "/v1/me/player/queue", b"OK").is_err());
        assert_eq!(
            decode_response(&Method::GET, "/v1/me/player/queue", br#"{"queue":[]}"#).unwrap()["queue"],
            json!([])
        );
    }

    #[test]
    fn library_follows_all_pages_including_private_and_collaborative_playlists() {
        let mut requests = Vec::new();
        let playlists = playlist_pages(|path| {
            requests.push(path.to_owned());
            Ok(if requests.len() == 1 {
                json!({"items":[{"id":"public","name":"Public","public":true},{"id":"private","name":"Private","public":false},null],"next":"https://api.spotify.com/v1/me/playlists?offset=50&limit=50"})
            } else {
                json!({"items":[{"id":"shared","name":"Collaborative","collaborative":true},{"id":"public","name":"Public"}],"next":null})
            })
        }).unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].contains("offset=50"));
        assert_eq!(
            playlists
                .iter()
                .map(|playlist| playlist.id.as_str())
                .collect::<Vec<_>>(),
            ["public", "private", "shared"]
        );
        assert!(playlists[2].collaborative);
    }

    #[test]
    fn repeated_playlist_page_stops_instead_of_looping() {
        let result = playlist_pages(|path| Ok(json!({"items":[],"next":path})));
        assert!(result.err().unwrap().to_string().contains("repeated"));
    }

    #[test]
    fn playlist_parser_handles_2026_and_legacy_fields_and_missing_tracks() {
        let page = json!({"items":[{"item":{"id":"one","uri":"spotify:track:one","name":"New","type":"track"}}, {"track":{"id":"two","name":"Old"}}, {"item":null}, {"item":{"type":"episode","name":"Podcast"}}]});
        let tracks = parse_track_items(&page).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].name, "New");
        assert_eq!(tracks[1].name, "Old");
    }
}
