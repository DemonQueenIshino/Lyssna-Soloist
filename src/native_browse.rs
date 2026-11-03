use crate::model::Track;
use anyhow::{Context, Result, ensure};
use librespot_core::{Session, SpotifyUri};
use librespot_metadata::{Metadata, playlist::Playlist};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::OnceLock,
    time::{Duration, Instant},
};
struct Listing {
    uris: Vec<String>,
    /// When each song was added, in Unix milliseconds; 0 when unknown.
    added: Vec<i64>,
    pages: VecDeque<Value>,
    visited: HashSet<String>,
    created: Instant,
}
type Key = (String, String);
static LISTINGS: OnceLock<tokio::sync::Mutex<HashMap<Key, Listing>>> = OnceLock::new();
static TRACKS: OnceLock<std::sync::Mutex<HashMap<Key, Track>>> = OnceLock::new();
type CachedCover = (Instant, Vec<crate::model::Artwork>);
static PLAYLIST_COVERS: OnceLock<std::sync::Mutex<HashMap<Key, CachedCover>>> = OnceLock::new();
/// Playlist contexts already checked for Spotify's hidden Liked Songs
/// playlist, so each is looked up once per run.
static LIKED_PLAYLISTS: OnceLock<std::sync::Mutex<HashMap<Key, bool>>> = OnceLock::new();
fn listing_pages(context: librespot_protocol::context::Context) -> VecDeque<Value> {
    context.pages.into_iter().map(|page|json!({"tracks":page.tracks.into_iter().filter(|track|track.uri.is_some()).map(|track|json!({"uri":track.uri,"added_at":track.metadata.get("added_at")})).collect::<Vec<_>>(),"page_url":page.page_url,"next_page_url":page.next_page_url})).collect()
}
pub async fn playlist_info(session: &Session, id: &str) -> Result<crate::model::BrowseInfo> {
    ensure!(
        id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()),
        "Invalid playlist ID"
    );
    let uri = SpotifyUri::from_uri(&format!("spotify:playlist:{id}"))?;
    let playlist = tokio::time::timeout(Duration::from_secs(8), Playlist::get(session, &uri))
        .await
        .context("Playlist metadata request timed out")??;
    ensure!(
        !playlist.attributes.name.is_empty(),
        "Playlist name unavailable"
    );
    let mut info = playlist_header(id, &playlist);
    if info.images.is_empty() {
        info.images = default_playlist_artwork(session, &playlist)
            .await
            .unwrap_or_default();
    }
    if let Some(details) = &mut info.playlist
        && details.owner != session.username()
    {
        details.owner_profile = owner_profile(session, &details.owner).await;
    }
    Ok(info)
}
fn playlist_header(id: &str, playlist: &Playlist) -> crate::model::BrowseInfo {
    use crate::model::{Artwork, BrowseInfo, PlaylistDetails};
    let mut images: Vec<_> = playlist
        .attributes
        .picture_sizes
        .iter()
        .filter(|picture| picture.url.starts_with("https://"))
        .map(|picture| Artwork {
            url: picture.url.clone(),
            width: None,
        })
        .collect();
    if images.is_empty() && playlist.attributes.picture.len() == 20 {
        let file: String = playlist
            .attributes
            .picture
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        images.push(Artwork {
            url: format!("https://i.scdn.co/image/{file}"),
            width: None,
        });
    }
    let owner = match &playlist.id {
        SpotifyUri::Playlist { user, .. } => user.clone().unwrap_or_default(),
        _ => String::new(),
    };
    BrowseInfo {
        key: format!("playlist:{id}"),
        title: playlist.attributes.name.clone(),
        subtitle: playlist.attributes.description.clone(),
        images,
        playlist: Some(PlaylistDetails {
            owner,
            owner_profile: None,
            total_tracks: usize::try_from(playlist.length).ok(),
            format: playlist.attributes.format.clone(),
        }),
        ..BrowseInfo::default()
    }
}

const ROOTLIST_PAGE: usize = 120;

/// How many songs the user has liked by this artist, from Spotify's own
/// per-artist view of the collection (what its "You Liked" card counts).
pub async fn liked_by_artist(session: &Session, artist: &str) -> Result<usize> {
    ensure!(
        artist.len() == 22 && artist.bytes().all(|byte| byte.is_ascii_alphanumeric()),
        "Invalid artist ID"
    );
    let uri = format!(
        "spotify:user:{}:collection:artist:{artist}",
        session.username()
    );
    count_rows(session, &uri).await
}

/// How many songs are in Liked Songs. Spotify reports no total, so count rows.
pub async fn liked_count(session: &Session) -> Result<usize> {
    let uri = format!("spotify:user:{}:collection", session.username());
    count_rows(session, &uri).await
}

async fn count_rows(session: &Session, uri: &str) -> Result<usize> {
    Ok(context_uris(session, uri).await?.len())
}

/// Every item URI in a browsing context, following all of its pages.
async fn context_uris(session: &Session, uri: &str) -> Result<Vec<String>> {
    let context =
        tokio::time::timeout(Duration::from_secs(10), session.spclient().get_context(uri))
            .await
            .context("Spotify library request timed out")??;
    let mut pages = listing_pages(context);
    let mut visited = HashSet::new();
    let mut uris = Vec::new();
    for _ in 0..512 {
        let Some(page) = pages.pop_front() else {
            return Ok(uris);
        };
        let rows = page["tracks"].as_array();
        // A page may arrive as only a URL; fetch it in place.
        let url = if rows.is_none_or(Vec::is_empty) {
            page["page_url"].as_str()
        } else {
            page["next_page_url"].as_str()
        };
        uris.extend(
            rows.into_iter()
                .flatten()
                .map(|row| row["uri"].as_str().unwrap_or_default().to_owned()),
        );
        let Some(url) = url.filter(|url| !url.is_empty() && visited.insert(url.to_string())) else {
            continue;
        };
        ensure!(
            url.starts_with("hm://") || url.starts_with('/'),
            "Rejected unexpected Spotify page URL"
        );
        let bytes = session.spclient().get_next_page(url).await?;
        ensure!(bytes.len() <= 4_000_000, "Spotify page is too large");
        pages.push_back(serde_json::from_slice(&bytes)?);
    }
    Ok(uris)
}

/// Exact song count and total length of a playlist or Liked Songs, so the
/// header shows Spotify's figure at once instead of summing pages as they load.
pub async fn list_duration(session: &Session, key: &str) -> Result<(usize, u64)> {
    let uris = if key == "liked" {
        context_uris(
            session,
            &format!("spotify:user:{}:collection", session.username()),
        )
        .await?
    } else {
        let id = key.strip_prefix("playlist:").context("Invalid list")?;
        valid_playlist_id(id)?;
        let uri = SpotifyUri::from_uri(&format!("spotify:playlist:{id}"))?;
        let playlist = tokio::time::timeout(Duration::from_secs(8), Playlist::get(session, &uri))
            .await
            .context("Playlist request timed out")??;
        playlist.tracks().map(|uri| uri.to_uri()).collect()
    };
    ensure!(uris.len() <= 100_000, "Playlist exceeds browsing limit");
    let identity = session.username();
    let mut total = 0;
    // A song can repeat in a playlist; each copy counts toward the length.
    let mut missing: HashMap<String, u64> = HashMap::new();
    if let Ok(cache) = TRACKS.get_or_init(Default::default).lock() {
        for uri in uris.iter().filter(|uri| uri.starts_with("spotify:track:")) {
            match cache.get(&(identity.clone(), uri.clone())) {
                Some(track) => total += track.duration_ms,
                None => *missing.entry(uri.clone()).or_default() += 1,
            }
        }
    }
    let copies = missing;
    let missing: Vec<String> = copies.keys().cloned().collect();
    // Uncached songs load in parallel batches; 800 songs take two requests.
    let mut batches = tokio::task::JoinSet::new();
    for chunk in missing.chunks(500) {
        let session = session.clone();
        let chunk = chunk.to_vec();
        batches.spawn(async move {
            tokio::time::timeout(Duration::from_secs(10), tracks_batched(&session, &chunk)).await
        });
    }
    let remember = missing.len() <= 2_000;
    while let Some(batch) = batches.join_next().await {
        let tracks = batch
            .context("Song lookup failed")?
            .context("Song lookup timed out")??;
        for (uri, track) in tracks {
            total += track.duration_ms * copies.get(&uri).copied().unwrap_or(1);
            if remember {
                remember_track(&identity, &uri, &track);
            }
        }
    }
    Ok((uris.len(), total))
}

/// Every saved playlist in Spotify library order, read from the account's
/// rootlist like the desktop client. Folder markers are skipped.
pub async fn library(session: &Session) -> Result<Vec<crate::model::Playlist>> {
    Ok(library_with_folders(session).await?.0)
}

/// The library plus every Spotify folder in it, empty ones included.
pub async fn library_with_folders(
    session: &Session,
) -> Result<(
    Vec<crate::model::Playlist>,
    Vec<crate::model::LibraryFolder>,
)> {
    use protobuf::Message;
    let mut folders = Vec::new();
    let mut playlists = Vec::new();
    let mut ids = HashSet::new();
    // Folders open across page boundaries.
    let mut open = Vec::new();
    let mut from = 0;
    for _ in 0..100 {
        let bytes = tokio::time::timeout(
            Duration::from_secs(10),
            session.spclient().get_rootlist(from, Some(ROOTLIST_PAGE)),
        )
        .await
        .context("Spotify library request timed out")??;
        ensure!(
            bytes.len() <= 8_000_000,
            "Spotify library page is too large"
        );
        let page =
            librespot_protocol::playlist4_external::SelectedListContent::parse_from_bytes(&bytes)
                .context("Invalid Spotify library page")?;
        let count = rootlist_page(&page, &mut ids, &mut open, &mut folders, &mut playlists)?;
        from += count;
        let total = usize::try_from(page.length()).unwrap_or_default();
        if count == 0 || (from >= total && count < ROOTLIST_PAGE) {
            hydrate_playlist_artwork(session, &mut playlists).await;
            name_owners(session, &mut playlists).await;
            return Ok((playlists, folders));
        }
    }
    anyhow::bail!("Spotify library has too many pages")
}

/// Spotify omits picture attributes for playlists that use album artwork.
/// Resolve a bounded sample of their songs rather than leaving a blank tile.
async fn default_playlist_artwork(
    session: &Session,
    playlist: &Playlist,
) -> Result<Vec<crate::model::Artwork>> {
    let uris: Vec<_> = playlist
        .tracks()
        .filter(|uri| matches!(uri, SpotifyUri::Track { .. }))
        .take(20)
        .map(|uri| uri.to_uri())
        .collect();
    if uris.is_empty() {
        return Ok(Vec::new());
    }
    Ok(album_mosaic(&resolve_tracks(session, &uris).await?))
}

fn album_mosaic(tracks: &[Track]) -> Vec<crate::model::Artwork> {
    let mut seen = HashSet::new();
    let covers: Vec<_> = tracks
        .iter()
        .filter_map(|track| {
            let image = track
                .album
                .images
                .iter()
                .find(|image| image.url.starts_with("https://"))?;
            seen.insert(image.url.as_str()).then_some(image.clone())
        })
        .take(4)
        .collect();
    if covers.len() == 4 {
        let ids: Option<Vec<_>> = covers
            .iter()
            .map(|image| {
                image
                    .url
                    .strip_prefix("https://i.scdn.co/image/")
                    .filter(|id| id.len() == 40 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
            })
            .collect();
        if let Some(ids) = ids {
            return vec![crate::model::Artwork {
                url: format!("https://mosaic.scdn.co/640/{}", ids.concat()),
                width: Some(640),
            }];
        }
    }
    covers.into_iter().take(1).collect()
}

async fn playlist_artwork(session: &Session, id: &str) -> Result<Vec<crate::model::Artwork>> {
    let key = (session.username(), id.to_owned());
    if let Some(images) = PLAYLIST_COVERS
        .get_or_init(Default::default)
        .lock()
        .ok()
        .and_then(|cache| {
            cache
                .get(&key)
                .filter(|(when, _)| when.elapsed() < Duration::from_secs(300))
                .map(|(_, images)| images.clone())
        })
    {
        return Ok(images);
    }
    let uri = SpotifyUri::from_uri(&format!("spotify:playlist:{id}"))?;
    let playlist = Playlist::get(session, &uri).await?;
    let mut images = playlist_header(id, &playlist).images;
    if images.is_empty() {
        images = default_playlist_artwork(session, &playlist).await?;
    }
    if !images.is_empty()
        && let Ok(mut cache) = PLAYLIST_COVERS.get_or_init(Default::default).lock()
    {
        if cache.len() >= 2048 {
            cache.clear();
        }
        cache.insert(key, (Instant::now(), images.clone()));
    }
    Ok(images)
}

async fn hydrate_playlist_artwork(session: &Session, playlists: &mut [crate::model::Playlist]) {
    let mut pending: VecDeque<_> = playlists
        .iter()
        .enumerate()
        .filter(|(_, playlist)| playlist.images.is_empty())
        .map(|(index, playlist)| (index, playlist.id.clone()))
        .collect();
    let mut tasks = tokio::task::JoinSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        while tasks.len() < 4 {
            let Some((index, id)) = pending.pop_front() else {
                break;
            };
            let session = session.clone();
            tasks.spawn(async move { (index, playlist_artwork(&session, &id).await) });
        }
        if tasks.is_empty() {
            break;
        }
        match tokio::time::timeout_at(deadline, tasks.join_next()).await {
            Ok(Some(Ok((index, Ok(images))))) => playlists[index].images = images,
            Err(_) => break,
            _ => {}
        }
    }
    // Dropping JoinSet cancels unfinished work; no late results can replace a
    // later library snapshot or a different account's artwork.
}

/// The latest 50 played contexts, newest first, each as the song last heard
/// in it. Spotify's play history is per context, not per play.
/// When each context (playlist, album, artist) was last played, in Unix
/// milliseconds, from Spotify's play history.
pub async fn played_contexts(session: &Session) -> Result<HashMap<String, i64>> {
    let mut played = HashMap::new();
    for play in recent_plays(&play_history(session).await?)? {
        let at = time::OffsetDateTime::parse(
            &play.played_at,
            &time::format_description::well_known::Rfc3339,
        )
        .map_or(0, |at| {
            i64::try_from(at.unix_timestamp_nanos() / 1_000_000).unwrap_or(0)
        });
        let latest = played.entry(play.context).or_insert(0);
        *latest = (*latest).max(at);
    }
    Ok(played)
}

async fn play_history(session: &Session) -> Result<Value> {
    let user: String =
        url::form_urlencoded::byte_serialize(session.username().as_bytes()).collect();
    let path = format!(
        "/recently-played/v3/user/{user}/recently-played?format=json&offset=0&limit=50&filter=default"
    );
    let bytes = tokio::time::timeout(
        Duration::from_secs(10),
        session
            .spclient()
            .request_as_json(&reqwest::Method::GET, &path, None, None),
    )
    .await
    .context("Spotify play history request timed out")??;
    ensure!(
        bytes.len() <= 2_000_000,
        "Spotify play history is too large"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

pub async fn recent(session: &Session) -> Result<Vec<Track>> {
    let plays = recent_plays(&play_history(session).await?)?;
    let uris: Vec<String> = plays.iter().map(|play| play.track.clone()).collect();
    let mut tracks = resolve_tracks(session, &uris).await?;
    // Spotify records Liked Songs plays under a hidden playlist (format
    // "liked-songs"); show them as Liked Songs, as Spotify's Recents does.
    let collection = format!("spotify:user:{}:collection", session.username());
    let mut checks = tokio::task::JoinSet::new();
    let contexts: HashSet<String> = plays.iter().map(|play| play.context.clone()).collect();
    for context in contexts {
        if let Some(id) = context.strip_prefix("spotify:playlist:").map(str::to_owned) {
            let session = session.clone();
            checks.spawn(async move { is_liked_playlist(&session, &id).await.then_some(context) });
        }
    }
    let mut liked = HashSet::new();
    while let Some(result) = checks.join_next().await {
        liked.extend(result.ok().flatten());
    }
    for (track, play) in tracks.iter_mut().zip(plays) {
        track.added_at = play.played_at;
        track.played_context = if liked.contains(&play.context) {
            collection.clone()
        } else {
            play.context
        };
    }
    Ok(tracks)
}

async fn is_liked_playlist(session: &Session, id: &str) -> bool {
    let key = (session.username(), id.to_owned());
    let known = LIKED_PLAYLISTS
        .get_or_init(Default::default)
        .lock()
        .ok()
        .and_then(|checked| checked.get(&key).copied());
    if let Some(liked) = known {
        return liked;
    }
    let Ok(playlist) = read_playlist(session, id).await else {
        // Not remembered: a failed lookup is tried again next time.
        return false;
    };
    let liked = playlist.attributes.format == "liked-songs";
    if let Ok(mut checked) = LIKED_PLAYLISTS.get_or_init(Default::default).lock() {
        if checked.len() >= 1024 {
            checked.clear();
        }
        checked.insert(key, liked);
    }
    liked
}

#[derive(Debug, PartialEq)]
struct RecentPlay {
    context: String,
    track: String,
    played_at: String,
}

fn recent_plays(value: &Value) -> Result<Vec<RecentPlay>> {
    let contexts = value["playContexts"]
        .as_array()
        .context("Spotify play history unavailable")?;
    Ok(contexts
        .iter()
        .filter_map(|item| {
            let track = item["lastPlayedTrackUri"]
                .as_str()
                .filter(|uri| uri.starts_with("spotify:track:"))?;
            let time = item["lastPlayedTime"].as_i64()?;
            // Milliseconds; older entries may carry seconds.
            let ms = if time < 100_000_000_000 {
                time * 1000
            } else {
                time
            };
            let played_at =
                time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
                    .ok()?
                    .format(&time::format_description::well_known::Rfc3339)
                    .ok()?;
            Some(RecentPlay {
                context: item["uri"].as_str().unwrap_or_default().to_owned(),
                track: track.to_owned(),
                played_at,
            })
        })
        .take(50)
        .collect())
}

/// Result of a native playlist write that may be retried elsewhere.
pub enum Added {
    Count(usize),
    /// Nothing reached the playlist, so another path can retry safely.
    NotWritten(anyhow::Error),
}

/// Add songs not already in the playlist, 100 per change, at the end.
pub async fn add_to_playlist(session: &Session, id: &str, tracks: &[Track]) -> Result<Added> {
    ensure!(
        id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()),
        "Invalid playlist ID"
    );
    let uri = SpotifyUri::from_uri(&format!("spotify:playlist:{id}"))?;
    let read = async || -> Result<Playlist> {
        let playlist = tokio::time::timeout(Duration::from_secs(10), Playlist::get(session, &uri))
            .await
            .context("Playlist request timed out")??;
        ensure!(
            playlist.contents.position == 0 && !playlist.contents.is_truncated,
            "Spotify returned an incomplete playlist"
        );
        Ok(playlist)
    };
    let playlist = match read().await {
        Ok(playlist) => playlist,
        Err(error) => return Ok(Added::NotWritten(error)),
    };
    let existing: HashSet<String> = playlist.tracks().map(SpotifyUri::to_uri).collect();
    let additions = crate::model::unique_additions(&existing, tracks);
    let mut added = 0;
    for batch in additions.chunks(100) {
        let written = playlist_json(
            session,
            &format!("/playlist/v2/playlist/{id}/changes"),
            &add_changes(batch),
        )
        .await;
        let confirmed = match written {
            Ok(_) => read().await,
            Err(error) => Err(error),
        };
        match confirmed {
            Ok(after) => {
                let present: HashSet<String> = after.tracks().map(SpotifyUri::to_uri).collect();
                if !batch.iter().all(|uri| present.contains(uri)) {
                    let error = anyhow::anyhow!("Spotify did not confirm the playlist change");
                    if added == 0 && !batch.iter().any(|uri| present.contains(uri)) {
                        return Ok(Added::NotWritten(error));
                    }
                    return Err(error);
                }
                added += batch.len();
            }
            Err(error) if added == 0 => return Ok(Added::NotWritten(error)),
            Err(error) => anyhow::bail!(
                "Added {added} tracks before stopping: {error}. Retry safely; existing tracks are skipped."
            ),
        }
    }
    Ok(Added::Count(added))
}

/// Create a private playlist and put it first in the library. Returns its ID.
pub async fn create_playlist(session: &Session, name: &str) -> Result<String> {
    let name = name.trim();
    ensure!(
        !name.is_empty() && name.chars().count() <= 100 && !name.chars().any(char::is_control),
        "Choose a playlist name up to 100 characters"
    );
    // Same body as Spotify's web player: one list-attribute update.
    let reply = playlist_json(
        session,
        "/playlist/v2/playlist",
        &json!({"ops": [{
            "kind": 6,
            "updateListAttributes": {"newAttributes": {"values": {"name": name}}}
        }]}),
    )
    .await
    .context("Spotify could not create the playlist")?;
    let id = reply["uri"]
        .as_str()
        .unwrap_or_default()
        .rsplit_once("playlist:")
        .map(|(_, id)| id)
        .filter(|id| id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        .context("Spotify returned an invalid playlist")?
        .to_owned();
    rootlist_change(session, rootlist_add(&format!("spotify:playlist:{id}"))).await?;
    Ok(id)
}

/// JSON request to Spotify's playlist service, as the web player sends it.
async fn playlist_json(session: &Session, path: &str, body: &Value) -> Result<Value> {
    use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue};
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    let body = serde_json::to_vec(body)?;
    let bytes = tokio::time::timeout(
        Duration::from_secs(15),
        session
            .spclient()
            .request(&reqwest::Method::POST, path, Some(headers), Some(&body)),
    )
    .await
    .context("Spotify playlist request timed out")??;
    ensure!(bytes.len() <= 16_000_000, "Playlist response is too large");
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

/// Remove the song at `index`, checking first that it is still there.
pub async fn remove_from_playlist(
    session: &Session,
    id: &str,
    index: usize,
    uri: &str,
) -> Result<()> {
    ensure!(
        id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()),
        "Invalid playlist ID"
    );
    let playlist_uri = SpotifyUri::from_uri(&format!("spotify:playlist:{id}"))?;
    // Each row carries its own ID, so one copy of a repeated song can go.
    let read = async || -> Result<Vec<(String, String)>> {
        let playlist = tokio::time::timeout(
            Duration::from_secs(10),
            Playlist::get(session, &playlist_uri),
        )
        .await
        .context("Playlist request timed out")??;
        ensure!(
            playlist.contents.position == 0 && !playlist.contents.is_truncated,
            "Spotify returned an incomplete playlist"
        );
        Ok(playlist
            .contents
            .items
            .iter()
            .map(|item| {
                use base64::{Engine, engine::general_purpose::STANDARD};
                // JSON carries protobuf bytes as base64.
                let row = STANDARD.encode(&item.attributes.item_id);
                (item.id.to_uri(), row)
            })
            .collect())
    };
    let before = read().await?;
    let row = before
        .get(index)
        .filter(|(item, row)| item == uri && !row.is_empty())
        .map(|(_, row)| row.clone())
        .context("The playlist changed. Refresh it and try again.")?;
    playlist_json(
        session,
        &format!("/playlist/v2/playlist/{id}/changes"),
        &remove_changes(uri, &row),
    )
    .await
    .context("Spotify could not remove the song")?;
    let after = read().await?;
    ensure!(
        after.len() + 1 == before.len() && !after.iter().any(|(_, other)| *other == row),
        "Spotify did not confirm the removal"
    );
    Ok(())
}

/// One playlist change in the web player's JSON format.
fn playlist_changes(op: Value) -> Value {
    playlist_change_ops(vec![op])
}

/// Several operations applied together as one atomic change.
fn playlist_change_ops(ops: Vec<Value>) -> Value {
    json!({
        "deltas": [{"ops": ops, "info": {"source": {"client": 5}}}],
        "wantResultingRevisions": false,
        "wantSyncResult": false,
        "nonces": []
    })
}

fn valid_playlist_id(id: &str) -> Result<()> {
    ensure!(
        id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()),
        "Invalid playlist ID"
    );
    Ok(())
}

async fn read_playlist(session: &Session, id: &str) -> Result<Playlist> {
    let uri = SpotifyUri::from_uri(&format!("spotify:playlist:{id}"))?;
    let playlist = tokio::time::timeout(Duration::from_secs(10), Playlist::get(session, &uri))
        .await
        .context("Playlist request timed out")??;
    ensure!(
        playlist.contents.position == 0 && !playlist.contents.is_truncated,
        "Spotify returned an incomplete playlist"
    );
    Ok(playlist)
}

/// Rename a playlist and set its description; an empty description clears it.
pub async fn edit_details(
    session: &Session,
    id: &str,
    name: &str,
    description: &str,
) -> Result<()> {
    valid_playlist_id(id)?;
    let (name, description) = (name.trim(), description.trim());
    ensure!(
        !name.is_empty() && name.chars().count() <= 100 && !name.chars().any(char::is_control),
        "Choose a playlist name up to 100 characters"
    );
    ensure!(
        description.chars().count() <= 300
            && !description.chars().any(|c| c.is_control() && c != '\n'),
        "Keep the description under 300 characters"
    );
    let mut values = json!({"name": name});
    let mut empty = Vec::new();
    if description.is_empty() {
        empty.push("LIST_DESCRIPTION");
    } else {
        values["description"] = json!(description);
    }
    playlist_json(
        session,
        &format!("/playlist/v2/playlist/{id}/changes"),
        &playlist_changes(json!({"kind": 6, "updateListAttributes": {
            "newAttributes": {"values": values, "noValue": empty}
        }})),
    )
    .await
    .context("Spotify could not change the playlist details")?;
    let after = read_playlist(session, id).await?;
    ensure!(
        after.attributes.name == name && after.attributes.description == description,
        "Spotify did not confirm the new details"
    );
    Ok(())
}

/// Largest cover accepted; the app sends a 640 px JPEG well under this.
const MAX_COVER_BYTES: usize = 4 * 1024 * 1024;

/// Replace a playlist's cover with a JPEG, as the desktop client does:
/// upload the image, register it on the playlist, set it as the picture,
/// then re-read the playlist to confirm. Returns the new image URL.
pub async fn set_cover(session: &Session, id: &str, jpeg: &[u8]) -> Result<String> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    valid_playlist_id(id)?;
    ensure!(
        jpeg.len() > 3 && jpeg.len() <= MAX_COVER_BYTES && jpeg.starts_with(&[0xFF, 0xD8, 0xFF]),
        "Choose a JPEG image under 4 MB"
    );
    let token = session.login5().auth_token().await?;
    let client_token = session.spclient().client_token().await?;
    let upload: Value = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()?
        .post("https://image-upload.spotify.com/v4/playlist")
        .bearer_auth(&token.access_token)
        .header("client-token", client_token)
        .header(reqwest::header::CONTENT_TYPE, "image/jpeg")
        .body(jpeg.to_vec())
        .send()
        .await?
        .error_for_status()
        .context("Spotify did not accept the image")?
        .json()
        .await?;
    let upload_token = upload["uploadToken"]
        .as_str()
        .filter(|token| !token.is_empty() && token.len() <= 4096)
        .context("Spotify did not accept the image")?;
    let registered = playlist_json(
        session,
        &format!("/playlist/v2/playlist/{id}/register-image"),
        &json!({ "uploadToken": upload_token }),
    )
    .await
    .context("Spotify could not attach the image")?;
    let picture = registered["picture"]
        .as_str()
        .and_then(|value| STANDARD.decode(value).ok())
        .filter(|bytes| !bytes.is_empty() && bytes.len() <= 64)
        .context("Spotify could not attach the image")?;
    playlist_json(
        session,
        &format!("/playlist/v2/playlist/{id}/changes"),
        &playlist_changes(json!({"kind": 6, "updateListAttributes": {
            "newAttributes": {"values": {"picture": STANDARD.encode(&picture)}, "noValue": []}
        }})),
    )
    .await
    .context("Spotify could not change the cover")?;
    let after = read_playlist(session, id).await?;
    ensure!(
        after.attributes.picture == picture,
        "Spotify did not confirm the new cover"
    );
    Ok(format!("https://i.scdn.co/image/{}", hex(&picture)))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Each row's song and its playlist item ID (base64, as the JSON API expects).
async fn playlist_rows(session: &Session, id: &str) -> Result<Vec<(String, String)>> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    Ok(read_playlist(session, id)
        .await?
        .contents
        .items
        .iter()
        .map(|item| (item.id.to_uri(), STANDARD.encode(&item.attributes.item_id)))
        .collect())
}

/// Move the song at `from` so it ends up at position `to`.
pub async fn move_song(
    session: &Session,
    id: &str,
    from: usize,
    uri: &str,
    to: usize,
) -> Result<()> {
    valid_playlist_id(id)?;
    let before = playlist_rows(session, id).await?;
    ensure!(
        before
            .get(from)
            .is_some_and(|(item, row)| item == uri && !row.is_empty())
            && to < before.len(),
        "The playlist changed. Refresh it and try again."
    );
    if from == to {
        return Ok(());
    }
    let mut expected = before.clone();
    let moved = expected.remove(from);
    // Place it before the row that will follow it, or last.
    let key = |(uri, row): &(String, String)| json!({"uri": uri, "attributes": {"itemId": row}});
    let mut mov = json!({"items": [key(&moved)]});
    match expected.get(to) {
        Some(next) => mov["addBeforeItem"] = key(next),
        None => mov["addLast"] = json!(true),
    }
    expected.insert(to, moved);
    playlist_json(
        session,
        &format!("/playlist/v2/playlist/{id}/changes"),
        &playlist_changes(json!({"kind": 4, "mov": mov})),
    )
    .await
    .context("Spotify could not move the song")?;
    let after = playlist_rows(session, id).await?;
    ensure!(after == expected, "Spotify did not confirm the new order");
    Ok(())
}

fn remove_changes(uri: &str, row: &str) -> Value {
    json!({
        "deltas": [{
            "ops": [{"kind": 3, "rem": {
                "items": [{"uri": uri, "attributes": {"itemId": row}}],
                "itemsAsKey": true
            }}],
            "info": {"source": {"client": 5}}
        }],
        "wantResultingRevisions": false,
        "wantSyncResult": false,
        "nonces": []
    })
}

/// Every library row in order: its URI and item ID (base64), folder markers included.
async fn library_rows(session: &Session) -> Result<Vec<(String, String)>> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use protobuf::Message;
    let mut rows = Vec::new();
    for _ in 0..100 {
        let bytes = tokio::time::timeout(
            Duration::from_secs(10),
            session
                .spclient()
                .get_rootlist(rows.len(), Some(ROOTLIST_PAGE)),
        )
        .await
        .context("Spotify library request timed out")??;
        let page =
            librespot_protocol::playlist4_external::SelectedListContent::parse_from_bytes(&bytes)
                .context("Invalid Spotify library page")?;
        let items = &page.contents.get_or_default().items;
        rows.extend(items.iter().map(|item| {
            let id = item
                .attributes
                .as_ref()
                .map(|attributes| STANDARD.encode(attributes.item_id()))
                .unwrap_or_default();
            (item.uri().to_owned(), id)
        }));
        let total = usize::try_from(page.length()).unwrap_or_default();
        if items.is_empty() || rows.len() >= total {
            return Ok(rows);
        }
    }
    anyhow::bail!("Spotify library has too many pages")
}

fn row_key((uri, id): &(String, String)) -> Value {
    if id.is_empty() {
        json!({"uri": uri})
    } else {
        json!({"uri": uri, "attributes": {"itemId": id}})
    }
}

fn valid_folder_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty() && id.len() <= 64 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()),
        "Invalid folder ID"
    );
    Ok(())
}

fn folder_name(name: &str) -> Result<String> {
    let name = name.trim();
    ensure!(
        !name.is_empty() && name.chars().count() <= 100 && !name.chars().any(char::is_control),
        "Choose a folder name up to 100 characters"
    );
    Ok(url::form_urlencoded::byte_serialize(name.as_bytes()).collect())
}

/// Create an empty Spotify folder at the top of the library. Returns its ID.
pub async fn create_folder(session: &Session, name: &str) -> Result<String> {
    let encoded = folder_name(name)?;
    let mut bytes = [0; 8];
    getrandom::fill(&mut bytes).map_err(|_| anyhow::anyhow!("Cannot create a folder ID"))?;
    let id: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let (start, end) = (
        format!("spotify:start-group:{id}:{encoded}"),
        format!("spotify:end-group:{id}"),
    );
    rootlist_change(
        session,
        json!({"kind": 2, "add": {
            "items": [{"uri": start}, {"uri": end}],
            "addFirst": true
        }}),
    )
    .await?;
    let rows = library_rows(session).await?;
    ensure!(
        rows.windows(2)
            .any(|pair| pair[0].0 == start && pair[1].0 == end),
        "Spotify did not confirm the new folder"
    );
    Ok(id)
}

/// Rename a Spotify folder; its name lives in its start marker.
pub async fn rename_folder(session: &Session, id: &str, name: &str) -> Result<()> {
    valid_folder_id(id)?;
    let encoded = folder_name(name)?;
    let prefix = format!("spotify:start-group:{id}:");
    let rows = library_rows(session).await?;
    let index = rows
        .iter()
        .position(|(uri, _)| uri.starts_with(&prefix))
        .context("This folder is no longer in Your Library")?;
    let next = rows
        .get(index + 1)
        .context("This folder has no end marker")?;
    let renamed = format!("{prefix}{encoded}");
    // Positional edits are rejected; swap the marker by URI in one change,
    // re-adding it before the row that followed it.
    rootlist_changes(
        session,
        vec![
            json!({"kind": 3, "rem": {"items": [row_key(&rows[index])], "itemsAsKey": true}}),
            json!({"kind": 2, "add": {"items": [{"uri": renamed}], "addBeforeItem": row_key(next)}}),
        ],
    )
    .await?;
    ensure!(
        library_rows(session)
            .await?
            .iter()
            .any(|(uri, _)| *uri == renamed),
        "Spotify did not confirm the new folder name"
    );
    Ok(())
}

/// Move a playlist into the end of a folder, or out to the end of the library.
pub async fn move_to_folder(session: &Session, playlist: &str, folder: Option<&str>) -> Result<()> {
    valid_playlist_id(playlist)?;
    if let Some(folder) = folder {
        valid_folder_id(folder)?;
    }
    let rows = library_rows(session).await?;
    let suffix = format!("playlist:{playlist}");
    let row = rows
        .iter()
        .find(|(uri, _)| uri.starts_with("spotify:") && uri.ends_with(&suffix))
        .context("This playlist is not in Your Library")?;
    // Positional moves are rejected; remove and re-add the playlist by URI in
    // one change, before the folder's end marker or at the end.
    let mut add = json!({"items": [row_key(row)]});
    match folder {
        Some(folder) => {
            let end = format!("spotify:end-group:{folder}");
            let end = rows
                .iter()
                .find(|(uri, _)| *uri == end)
                .context("This folder is no longer in Your Library")?;
            add["addBeforeItem"] = row_key(end);
        }
        None => add["addLast"] = json!(true),
    }
    rootlist_changes(
        session,
        vec![
            json!({"kind": 3, "rem": {"items": [row_key(row)], "itemsAsKey": true}}),
            json!({"kind": 2, "add": add}),
        ],
    )
    .await?;
    let placed = library(session).await?;
    let now = placed
        .iter()
        .find(|item| item.id == playlist)
        .context("This playlist is not in Your Library")?;
    ensure!(
        now.folders.last().map(|item| item.id.as_str()) == folder,
        "Spotify did not confirm the move"
    );
    Ok(())
}

/// Remove only the folder's two markers, preserving every contained row.
fn delete_folder_change(rows: &[(String, String)], id: &str) -> Result<Value> {
    valid_folder_id(id)?;
    let prefix = format!("spotify:start-group:{id}:");
    let end = format!("spotify:end-group:{id}");
    let markers: Vec<_> = rows
        .iter()
        .filter(|(uri, _)| uri.starts_with(&prefix) || *uri == end)
        .collect();
    ensure!(
        markers.len() == 2 && markers[0].0.starts_with(&prefix) && markers[1].0 == end,
        "This folder is no longer in Your Library"
    );
    let keys: Vec<_> = markers.into_iter().map(row_key).collect();
    Ok(json!({"kind": 3, "rem": {"items": keys, "itemsAsKey": true}}))
}

/// Delete a Spotify folder but keep its playlists and subfolders, which move up a level.
pub async fn delete_folder(session: &Session, id: &str) -> Result<()> {
    valid_folder_id(id)?;
    let rows = library_rows(session).await?;
    rootlist_change(session, delete_folder_change(&rows, id)?).await?;
    let prefix = format!("spotify:start-group:{id}:");
    let end = format!("spotify:end-group:{id}");
    ensure!(
        !library_rows(session)
            .await?
            .iter()
            .any(|(uri, _)| uri.starts_with(&prefix) || *uri == end),
        "Spotify did not confirm the folder removal"
    );
    Ok(())
}

/// Remove a playlist from the library, as Spotify's Delete does for owners.
pub async fn remove_from_library(session: &Session, id: &str) -> Result<()> {
    ensure!(
        id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()),
        "Invalid playlist ID"
    );
    rootlist_change(
        session,
        json!({"kind": 3, "rem": {
            "items": [{"uri": format!("spotify:playlist:{id}")}],
            "itemsAsKey": true
        }}),
    )
    .await
}

fn rootlist_add(uri: &str) -> Value {
    json!({"kind": 2, "add": {
        "items": [{"uri": uri, "attributes": {
            "timestamp": (time::OffsetDateTime::now_utc().unix_timestamp() * 1000).to_string()
        }}],
        "addFirst": true
    }})
}

async fn rootlist_change(session: &Session, op: Value) -> Result<()> {
    rootlist_changes(session, vec![op]).await
}

/// Library edits applied together, so a remove and re-add cannot half-happen.
async fn rootlist_changes(session: &Session, ops: Vec<Value>) -> Result<()> {
    let user: String =
        url::form_urlencoded::byte_serialize(session.username().as_bytes()).collect();
    playlist_json(
        session,
        &format!("/playlist/v2/user/{user}/rootlist/changes"),
        &playlist_change_ops(ops),
    )
    .await
    .context("Spotify could not change the library")?;
    Ok(())
}

/// Append songs, in the web player's change format.
fn add_changes(uris: &[String]) -> Value {
    let now = (time::OffsetDateTime::now_utc().unix_timestamp() * 1000).to_string();
    json!({
        "deltas": [{
            "ops": [{"kind": 2, "add": {
                "items": uris
                    .iter()
                    .map(|uri| json!({"uri": uri, "attributes": {"timestamp": now}}))
                    .collect::<Vec<_>>(),
                "addLast": true
            }}],
            "info": {"source": {"client": 5}}
        }],
        "wantResultingRevisions": false,
        "wantSyncResult": false,
        "nonces": []
    })
}

static OWNER_NAMES: OnceLock<std::sync::Mutex<HashMap<String, String>>> = OnceLock::new();
type OwnerProfiles = HashMap<Key, (Instant, crate::model::Profile)>;
static OWNER_PROFILES: OnceLock<std::sync::Mutex<OwnerProfiles>> = OnceLock::new();

/// Public creator metadata, cached per viewer. Never hold the lock during I/O.
async fn owner_profile(session: &Session, owner: &str) -> Option<crate::model::Profile> {
    if owner.is_empty() || owner.len() > 512 {
        return None;
    }
    let key = (session.username(), owner.to_owned());
    let profiles = OWNER_PROFILES.get_or_init(Default::default);
    if let Some((created, profile)) = profiles.lock().ok()?.get(&key)
        && created.elapsed() < Duration::from_secs(600)
    {
        return Some(profile.clone());
    }
    let encoded: String = url::form_urlencoded::byte_serialize(owner.as_bytes()).collect();
    let bytes = tokio::time::timeout(
        Duration::from_secs(4),
        session
            .spclient()
            .get_user_profile(&encoded, Some(0), Some(0)),
    )
    .await
    .ok()?
    .ok()?;
    let profile = crate::native_login::profile_from_response(owner, &bytes).ok()?;
    let mut profiles = profiles.lock().ok()?;
    if profiles.len() >= 512 {
        profiles.clear();
    }
    profiles.insert(key, (Instant::now(), profile.clone()));
    Some(profile)
}

/// The rootlist names owners by user ID; look up each owner's display name
/// once per run. Failed lookups leave the name empty.
async fn name_owners(session: &Session, playlists: &mut [crate::model::Playlist]) {
    let me = session.username();
    let known = OWNER_NAMES.get_or_init(Default::default);
    let missing: HashSet<String> = {
        let Ok(known) = known.lock() else { return };
        playlists
            .iter()
            .map(|playlist| &playlist.owner.id)
            .filter(|owner| !owner.is_empty() && **owner != me && !known.contains_key(*owner))
            .cloned()
            .collect()
    };
    let mut requests = tokio::task::JoinSet::new();
    for owner in missing.into_iter().take(64) {
        let session = session.clone();
        requests.spawn(async move {
            let name = owner_profile(&session, &owner).await?.display_name?;
            Some((owner, name))
        });
    }
    let mut found = Vec::new();
    while let Some(result) = requests.join_next().await {
        if let Ok(Some(entry)) = result {
            found.push(entry);
        }
    }
    let Ok(mut known) = known.lock() else { return };
    if known.len() >= 4096 {
        known.clear();
    }
    known.extend(found);
    for playlist in playlists.iter_mut() {
        if playlist.owner.display_name.is_none() {
            playlist.owner.display_name = known.get(&playlist.owner.id).cloned();
        }
    }
}

#[cfg(test)]
fn owner_name(bytes: &[u8]) -> Option<String> {
    crate::native_login::profile_from_response("friend", bytes)
        .ok()?
        .display_name
}

/// Adds one rootlist page and returns how many rows it held.
/// A folder marker: `spotify:start-group:<id>:<name>` or `spotify:end-group:<id>`.
enum Marker {
    Start(crate::model::FolderRef),
    End(String),
}

fn folder_marker(uri: &str) -> Option<Marker> {
    if let Some(rest) = uri.strip_prefix("spotify:start-group:") {
        let (id, name) = rest.split_once(':').unwrap_or((rest, ""));
        // Names are form-encoded: `+` for spaces, `%XX` for the rest.
        let name = url::form_urlencoded::parse(format!("n={name}").as_bytes())
            .next()
            .map(|(_, value)| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Folder".into());
        return Some(Marker::Start(crate::model::FolderRef {
            id: id.to_owned(),
            name,
        }));
    }
    uri.strip_prefix("spotify:end-group:")
        .map(|id| Marker::End(id.to_owned()))
}

fn rootlist_page(
    page: &librespot_protocol::playlist4_external::SelectedListContent,
    ids: &mut HashSet<String>,
    open: &mut Vec<crate::model::FolderRef>,
    folders: &mut Vec<crate::model::LibraryFolder>,
    playlists: &mut Vec<crate::model::Playlist>,
) -> Result<usize> {
    let contents = page.contents.get_or_default();
    ensure!(
        contents.items.len() == contents.meta_items.len(),
        "Spotify library page has no playlist details"
    );
    for (item, meta) in contents.items.iter().zip(&contents.meta_items) {
        match folder_marker(item.uri()) {
            Some(Marker::Start(folder)) if open.len() < 32 => {
                if folders.len() < 10_000 {
                    folders.push(crate::model::LibraryFolder {
                        id: folder.id.clone(),
                        name: folder.name.clone(),
                        parents: open.iter().map(|item| item.id.clone()).collect(),
                    });
                }
                open.push(folder);
                continue;
            }
            Some(Marker::End(id)) => {
                // Close back to the matching folder, tolerating a missed start.
                if let Some(depth) = open.iter().rposition(|folder| folder.id == id) {
                    open.truncate(depth);
                }
                continue;
            }
            Some(Marker::Start(_)) => continue,
            None => {}
        }
        let Some(id) = item
            .uri()
            .strip_prefix("spotify:")
            .and_then(|uri| uri.rsplit_once("playlist:"))
            .map(|(_, id)| id)
            .filter(|id| id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        else {
            continue;
        };
        let attributes = meta.attributes.get_or_default();
        if attributes.name().is_empty() || !ids.insert(id.to_owned()) {
            continue;
        }
        playlists.push(crate::model::Playlist {
            id: id.to_owned(),
            name: attributes.name().to_owned(),
            images: list_images(attributes),
            owner: crate::model::Owner {
                id: meta.owner_username().to_owned(),
                display_name: None,
            },
            collaborative: attributes.collaborative(),
            folders: open.clone(),
            added_at: item.attributes.get_or_default().timestamp(),
        });
    }
    Ok(contents.items.len())
}

fn list_images(
    attributes: &librespot_protocol::playlist4_external::ListAttributes,
) -> Vec<crate::model::Artwork> {
    let mut images: Vec<_> = attributes
        .picture_size
        .iter()
        .filter(|picture| picture.url().starts_with("https://"))
        .map(|picture| crate::model::Artwork {
            url: picture.url().to_owned(),
            width: None,
        })
        .collect();
    if images.is_empty() && attributes.picture().len() == 20 {
        let file: String = attributes
            .picture()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        images.push(crate::model::Artwork {
            url: format!("https://i.scdn.co/image/{file}"),
            width: None,
        });
    }
    images
}

pub async fn read(session: &Session, key: &str, offset: usize) -> Result<Vec<Track>> {
    read_range(session, key, offset, 50).await
}

/// Most songs one read returns; a whole playlist, Liked Songs or album is
/// read this way so it opens at once instead of a page at a time.
pub const READ_LIMIT: usize = 2_000;

/// Up to `limit` songs from `offset`. A first page (`offset` 0, `limit` 50)
/// also fetches the rest of the list's songs in the background.
pub async fn read_range(
    session: &Session,
    key: &str,
    offset: usize,
    limit: usize,
) -> Result<Vec<Track>> {
    ensure!((1..=READ_LIMIT).contains(&limit), "Invalid page size");
    let started = Instant::now();
    let mut fetched_pages = 0;
    let identity = session.username();
    let cache_key = (identity.clone(), key.to_owned());
    let wanted = offset.checked_add(limit).context("Invalid page offset")?;
    ensure!(wanted <= 100_000, "Playlist exceeds browsing limit");
    let uris = {
        let mut cache = LISTINGS.get_or_init(Default::default).lock().await;
        if offset == 0
            || cache
                .get(&cache_key)
                .is_some_and(|list| list.created.elapsed() > Duration::from_secs(300))
        {
            cache.remove(&cache_key);
        }
        if !cache.contains_key(&cache_key) {
            if cache.len() >= 16 {
                cache.clear();
            }
            let mut listing = Listing {
                uris: Vec::new(),
                added: Vec::new(),
                pages: VecDeque::new(),
                visited: HashSet::new(),
                created: Instant::now(),
            };
            if key.len() == 22 {
                let uri = SpotifyUri::from_uri(&format!("spotify:playlist:{key}"))?;
                let playlist = Playlist::get(session, &uri)
                    .await
                    .context("Spotify cannot open this playlist")?;
                ensure!(
                    playlist.contents.position == 0 && !playlist.contents.is_truncated,
                    "Spotify returned an incomplete playlist"
                );
                listing.uris = playlist.tracks().map(|uri| uri.to_uri()).collect();
                listing.added = playlist
                    .contents
                    .items
                    .iter()
                    .map(|item| item.attributes.timestamp.as_timestamp_ms())
                    .collect();
            } else {
                let uri = if key == "liked" {
                    format!("spotify:user:{identity}:collection")
                } else if let Some(artist) = key.strip_prefix("liked-artist:") {
                    format!("spotify:user:{identity}:collection:artist:{artist}")
                } else if let Some(query) = key.strip_prefix("search:") {
                    let encoded = url::form_urlencoded::Serializer::new(String::new())
                        .append_pair("q", query)
                        .finish();
                    format!(
                        "spotify:search:{}",
                        encoded.strip_prefix("q=").unwrap_or_default()
                    )
                } else {
                    format!("spotify:{key}")
                };
                listing.pages = listing_pages(
                    session
                        .spclient()
                        .get_context(&uri)
                        .await
                        .context("Spotify browsing context unavailable")?,
                );
            }
            cache.insert(cache_key.clone(), listing);
        }
        let listing = cache
            .get_mut(&cache_key)
            .context("Browse cache unavailable")?;
        let mut visited = 0;
        while listing.uris.len() < wanted {
            let Some(page) = listing.pages.pop_front() else {
                break;
            };
            visited += 1;
            ensure!(visited <= 512, "Spotify returned too many empty pages");
            let rows = page["tracks"].as_array();
            if rows.is_none_or(Vec::is_empty)
                && let Some(url) = page["page_url"].as_str().filter(|url| !url.is_empty())
                && listing.visited.insert(url.to_owned())
            {
                ensure!(
                    url.starts_with("hm://") || url.starts_with('/'),
                    "Rejected unexpected Spotify page URL"
                );
                fetched_pages += 1;
                let bytes = session.spclient().get_next_page(url).await?;
                ensure!(bytes.len() <= 4_000_000, "Spotify page is too large");
                listing.pages.push_front(serde_json::from_slice(&bytes)?);
                continue;
            }
            if let Some(rows) = rows {
                for row in rows {
                    let Some(uri) = row["uri"]
                        .as_str()
                        .filter(|uri| SpotifyUri::from_uri(uri).is_ok())
                    else {
                        continue;
                    };
                    listing.uris.push(uri.to_owned());
                    listing.added.push(
                        row["added_at"]
                            .as_str()
                            .and_then(|ms| ms.parse().ok())
                            .unwrap_or_default(),
                    );
                }
            }
            let next = page["next_page_url"].as_str();
            if let Some(next) = next.filter(|url| !url.is_empty())
                && !listing.visited.contains(next)
            {
                ensure!(
                    next.starts_with("hm://") || next.starts_with('/'),
                    "Rejected unexpected Spotify page URL"
                );
                listing.pages.push_back(json!({"page_url":next}));
            }
        }
        let uris: Vec<_> = listing
            .uris
            .iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect();
        let added: Vec<_> = (offset..offset + uris.len())
            .map(|index| listing.added.get(index).copied().unwrap_or_default())
            .collect();
        // The rest of the list loads right after the first page; fetch its
        // songs in the background so those pages come from the cache.
        let rest: Vec<String> = if offset == 0 && limit == 50 {
            listing
                .uris
                .iter()
                .skip(50)
                .take(5_000)
                .filter(|uri| uri.starts_with("spotify:track:"))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        (uris, added, rest)
    };
    let (uris, added, rest) = uris;
    let listed = started.elapsed();
    if !rest.is_empty() {
        let session = session.clone();
        tokio::spawn(async move { warm_tracks(&session, &rest).await });
    }
    let mut tracks = resolve_tracks(session, &uris).await?;
    for (track, added) in tracks.iter_mut().zip(added) {
        track.added_at = added_date(added);
    }
    // Whole-list reads only: how long the list and its songs took.
    if limit > 50 {
        let kind = key
            .split(':')
            .next()
            .filter(|kind| kind.len() != 22)
            .unwrap_or("playlist");
        crate::native::diagnostic(&format!(
            "browse {kind} {} songs: list {}ms ({fetched_pages} extra pages), songs {}ms",
            tracks.len(),
            listed.as_millis(),
            (started.elapsed() - listed).as_millis()
        ));
    }
    Ok(tracks)
}

/// Unix milliseconds as an RFC 3339 date; empty when unknown.
fn added_date(ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .ok()
        .and_then(|date| {
            date.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_default()
}

pub(crate) async fn resolve_tracks(session: &Session, uris: &[String]) -> Result<Vec<Track>> {
    // A whole list in one read, or a page of one.
    ensure!(uris.len() <= READ_LIMIT, "Too many songs at once");
    let identity = session.username();
    let cached = |uri: &String| {
        TRACKS
            .get_or_init(Default::default)
            .lock()
            .ok()
            .and_then(|cache| cache.get(&(identity.clone(), uri.clone())).cloned())
    };
    let missing: Vec<String> = uris
        .iter()
        .filter(|uri| uri.starts_with("spotify:track:") && cached(uri).is_none())
        .cloned()
        .collect();
    // One batched request, as Spotify's clients make; a page of 50 took up
    // to 1.4 s as single-song requests and about 0.1 s batched.
    if !missing.is_empty() {
        warm_tracks(session, &missing).await;
    }
    let mut result = Vec::with_capacity(uris.len());
    let mut singles = 0;
    let singles_started = Instant::now();
    for uri in uris {
        if let Some(track) = cached(uri) {
            result.push(track);
            continue;
        }
        // Batch misses (rare) and non-song items fall back to one request each.
        singles += 1;
        let track = match SpotifyUri::from_uri(uri) {
            // Spotify knows nothing of local songs; the folder scan does.
            Ok(SpotifyUri::Local { .. }) => None,
            Ok(id) => tokio::time::timeout(
                Duration::from_secs(6),
                librespot_metadata::Track::get(session, &id),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(|item| crate::native::metadata_track(&item, Some(&session.user_data()))),
            Err(_) => None,
        };
        match track {
            Some(track) => {
                remember_track(&identity, uri, &track);
                result.push(track);
            }
            // A playlist's local song plays when it is in a Local Files folder.
            None => result.push(crate::local_files::track(uri).unwrap_or_else(|| Track {
                uri: uri.clone(),
                name: "Unavailable track".into(),
                is_local: true,
                ..Track::default()
            })),
        }
    }
    if singles > 0 {
        crate::native::diagnostic(&format!(
            "browse {singles} songs looked up one at a time: {}ms",
            singles_started.elapsed().as_millis()
        ));
    }
    Ok(result)
}

fn remember_track(identity: &str, uri: &str, track: &Track) {
    if let Ok(mut cache) = TRACKS.get_or_init(Default::default).lock() {
        if cache.len() >= 8192 {
            cache.clear();
        }
        cache.insert((identity.to_owned(), uri.to_owned()), track.clone());
    }
}

/// Fetch song metadata in batches of up to 500 into the track cache.
async fn warm_tracks(session: &Session, uris: &[String]) {
    let identity = session.username();
    // The batches go out together, so a long list costs about one round
    // trip rather than one per batch.
    let mut batches = tokio::task::JoinSet::new();
    for chunk in uris.chunks(500) {
        let session = session.clone();
        let chunk = chunk.to_vec();
        batches.spawn(async move {
            tokio::time::timeout(Duration::from_secs(10), tracks_batched(&session, &chunk)).await
        });
    }
    while let Some(batch) = batches.join_next().await {
        let Ok(Ok(Ok(tracks))) = batch else {
            continue;
        };
        for (uri, track) in tracks {
            remember_track(&identity, &uri, &track);
        }
    }
}

#[cfg(test)]
mod rootlist_tests {
    use super::*;
    #[test]
    fn default_covers_use_distinct_albums_and_keep_empty_playlists_empty() {
        let tracks: Vec<_> = [1, 1, 2, 3, 4, 5]
            .into_iter()
            .map(|album| Track {
                album: crate::model::Album {
                    images: vec![crate::model::Artwork {
                        url: format!("https://i.scdn.co/image/ab67616d0000b273{album:024x}"),
                        width: Some(640),
                    }],
                    ..Default::default()
                },
                ..Default::default()
            })
            .collect();
        let expected: String = (1..=4)
            .map(|album| format!("ab67616d0000b273{album:024x}"))
            .collect();
        assert_eq!(
            album_mosaic(&tracks)[0].url,
            format!("https://mosaic.scdn.co/640/{expected}")
        );
        assert_eq!(
            album_mosaic(&tracks[..3])[0].url,
            tracks[0].album.images[0].url
        );
        assert!(album_mosaic(&[]).is_empty());
        assert!(album_mosaic(&[Track::default()]).is_empty());
    }
    use librespot_protocol::playlist4_external::{
        Item, ListAttributes, ListItems, MetaItem, SelectedListContent,
    };
    fn page(rows: &[(&str, &str, &str)]) -> SelectedListContent {
        let mut contents = ListItems::new();
        for (uri, name, owner) in rows {
            let mut item = Item::new();
            item.set_uri((*uri).into());
            contents.items.push(item);
            let mut meta = MetaItem::new();
            if !name.is_empty() {
                let mut attributes = ListAttributes::new();
                attributes.set_name((*name).into());
                attributes.set_collaborative(*name == "Shared");
                meta.attributes = Some(attributes).into();
                meta.set_owner_username((*owner).into());
            }
            contents.meta_items.push(meta);
        }
        let mut page = SelectedListContent::new();
        page.contents = Some(contents).into();
        page
    }
    #[test]
    fn folder_deletion_keeps_playlists_and_promotes_nested_folders() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let source = [
            ("spotify:start-group:outer:Outer", "", ""),
            ("spotify:start-group:target:Target", "", ""),
            ("spotify:playlist:0123456789abcdefghij01", "Mine", "me"),
            ("spotify:start-group:child:Child", "", ""),
            (
                "spotify:user:friend:playlist:0123456789abcdefghij02",
                "Shared",
                "friend",
            ),
            ("spotify:end-group:child", "", ""),
            ("spotify:start-group:empty:Empty", "", ""),
            ("spotify:end-group:empty", "", ""),
            ("spotify:end-group:target", "", ""),
            ("spotify:start-group:target2:Other", "", ""),
            ("spotify:playlist:0123456789abcdefghij03", "Other", "me"),
            ("spotify:end-group:target2", "", ""),
            ("spotify:end-group:outer", "", ""),
        ];
        let rows: Vec<_> = source
            .iter()
            .map(|(uri, _, _)| (uri.to_string(), STANDARD.encode(uri.as_bytes())))
            .collect();
        for (id, expected) in [
            (
                "target",
                vec![
                    vec!["outer"],
                    vec!["outer", "child"],
                    vec!["outer", "target2"],
                ],
            ),
            (
                "outer",
                vec![vec!["target"], vec!["target", "child"], vec!["target2"]],
            ),
        ] {
            let change = delete_folder_change(&rows, id).unwrap();
            assert_eq!(change["rem"]["itemsAsKey"], true);
            let removed = change["rem"]["items"].as_array().unwrap();
            assert_eq!(removed.len(), 2);
            for key in removed {
                let row = rows
                    .iter()
                    .find(|row| row.0 == key["uri"].as_str().unwrap())
                    .unwrap();
                assert_eq!(key["attributes"]["itemId"].as_str(), Some(row.1.as_str()));
            }
            let remaining: Vec<_> = source
                .iter()
                .copied()
                .filter(|(uri, _, _)| !removed.iter().any(|key| key["uri"] == *uri))
                .collect();
            let (mut ids, mut open, mut folders, mut playlists) =
                (HashSet::new(), Vec::new(), Vec::new(), Vec::new());
            rootlist_page(
                &page(&remaining),
                &mut ids,
                &mut open,
                &mut folders,
                &mut playlists,
            )
            .unwrap();
            assert!(open.is_empty());
            assert!(!folders.iter().any(|folder| folder.id == id));
            for child in ["child", "empty"] {
                let folder = folders.iter().find(|folder| folder.id == child).unwrap();
                assert_eq!(folder.parents, expected[0]);
            }
            let paths: Vec<Vec<_>> = playlists
                .iter()
                .map(|playlist| {
                    playlist
                        .folders
                        .iter()
                        .map(|folder| folder.id.as_str())
                        .collect()
                })
                .collect();
            assert_eq!(paths, expected);
            assert_eq!(
                playlists
                    .iter()
                    .map(|playlist| playlist.name.as_str())
                    .collect::<Vec<_>>(),
                ["Mine", "Shared", "Other"]
            );
            assert_eq!(playlists[1].owner.id, "friend");
            assert!(playlists[1].collaborative);
        }
    }

    #[test]
    fn folder_deletion_rejects_missing_duplicate_reversed_and_invalid_markers() {
        let start = "spotify:start-group:target:Target";
        let end = "spotify:end-group:target";
        for uris in [
            vec![],
            vec![start],
            vec![end],
            vec![start, start],
            vec![end, start],
            vec![start, end, end],
        ] {
            let rows: Vec<_> = uris
                .into_iter()
                .map(|uri| (uri.to_owned(), String::new()))
                .collect();
            assert!(delete_folder_change(&rows, "target").is_err());
        }
        let rows = vec![
            (start.to_owned(), String::new()),
            (end.to_owned(), String::new()),
        ];
        assert!(delete_folder_change(&rows, "target").is_ok());
        for id in ["", "../target", "target:Other"] {
            assert!(delete_folder_change(&rows, id).is_err());
        }
    }

    #[test]
    fn library_keeps_private_and_collaborative_playlists_with_their_folders() {
        let (mut ids, mut open, mut playlists) = (HashSet::new(), Vec::new(), Vec::new());
        let count = rootlist_page(
            &page(&[
                ("spotify:start-group:abc:Folder", "", ""),
                ("spotify:playlist:0123456789abcdefghijkl", "Mine", "me"),
                (
                    "spotify:user:friend:playlist:abcdefghijkl0123456789",
                    "Shared",
                    "friend",
                ),
                ("spotify:end-group:abc", "", ""),
                ("spotify:playlist:0123456789abcdefghijkl", "Mine", "me"),
            ]),
            &mut ids,
            &mut open,
            &mut Vec::new(),
            &mut playlists,
        )
        .unwrap();
        assert_eq!(count, 5);
        assert!(open.is_empty());
        assert_eq!(playlists[0].folders[0].name, "Folder");
        assert_eq!(playlists[1].folders, playlists[0].folders);
        let names: Vec<_> = playlists.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, ["Mine", "Shared"]);
        assert_eq!(playlists[1].id, "abcdefghijkl0123456789");
        assert_eq!(playlists[1].owner.id, "friend");
        assert!(playlists[1].collaborative && !playlists[0].collaborative);
    }
    #[test]
    fn library_page_without_details_is_rejected() {
        let mut broken = page(&[("spotify:playlist:0123456789abcdefghijkl", "Mine", "me")]);
        broken.contents.mut_or_insert_default().meta_items.clear();
        assert!(
            rootlist_page(
                &broken,
                &mut HashSet::new(),
                &mut Vec::new(),
                &mut Vec::new(),
                &mut Vec::new()
            )
            .is_err()
        );
    }
    #[test]
    fn nested_folders_span_pages_and_decode_names() {
        let (mut ids, mut open, mut folders, mut playlists) =
            (HashSet::new(), Vec::new(), Vec::new(), Vec::new());
        for rows in [
            &[
                ("spotify:start-group:outer:Road+%26+Rain", "", ""),
                ("spotify:playlist:0123456789abcdefghij01", "In outer", "me"),
                ("spotify:start-group:inner:Night", "", ""),
            ][..],
            &[
                ("spotify:playlist:0123456789abcdefghij02", "In inner", "me"),
                ("spotify:end-group:inner", "", ""),
                ("spotify:end-group:outer", "", ""),
                ("spotify:playlist:0123456789abcdefghij03", "Top", "me"),
            ][..],
        ] {
            rootlist_page(
                &page(rows),
                &mut ids,
                &mut open,
                &mut folders,
                &mut playlists,
            )
            .unwrap();
        }
        let paths: Vec<Vec<&str>> = playlists
            .iter()
            .map(|item| {
                item.folders
                    .iter()
                    .map(|folder| folder.name.as_str())
                    .collect()
            })
            .collect();
        assert_eq!(
            paths,
            [vec!["Road & Rain"], vec!["Road & Rain", "Night"], vec![]]
        );
        assert_eq!(playlists[1].folders[1].id, "inner");
        assert!(open.is_empty());
        let listed: Vec<_> = folders
            .iter()
            .map(|folder| (folder.name.as_str(), folder.parents.clone()))
            .collect();
        assert_eq!(
            listed,
            [("Road & Rain", vec![]), ("Night", vec!["outer".to_owned()])]
        );
    }
}

#[cfg(test)]
mod playlist_header_tests {
    use super::*;
    #[test]
    fn generated_playlist_metadata_is_independent_of_saved_library() {
        let id = "0123456789abcdefghijkl";
        let mut attributes = librespot_protocol::playlist4_external::ListAttributes::new();
        attributes.set_name("Daily Mix 1".into());
        attributes.set_description("Your favorite artists".into());
        attributes.set_format("daily-mix".into());
        let mut picture = librespot_protocol::playlist4_external::PictureSize::new();
        picture.set_url("https://artwork.example.test/mix".into());
        attributes.picture_size.push(picture);
        let mut data = librespot_protocol::playlist4_external::SelectedListContent::new();
        data.attributes = Some(attributes).into();
        data.set_length(50);
        data.set_owner_username("spotify".into());
        data.set_timestamp(1);
        let playlist = Playlist::parse(
            &data,
            &SpotifyUri::from_uri(&format!("spotify:playlist:{id}")).unwrap(),
        )
        .unwrap();
        let info = playlist_header(id, &playlist);
        assert_eq!(info.title, "Daily Mix 1");
        assert_eq!(info.subtitle, "Your favorite artists");
        assert_eq!(info.images[0].url, "https://artwork.example.test/mix");
        let details = info.playlist.unwrap();
        assert_eq!(details.owner, "spotify");
        assert_eq!(details.total_tracks, Some(50));
        assert_eq!(details.format, "daily-mix");
    }
}

#[cfg(test)]
mod owner_name_tests {
    use super::*;
    #[test]
    #[ignore = "Read-only native library and default playlist artwork verification"]
    fn live_playlist_artwork() {
        crate::native::with_live_session(async |session| {
            use protobuf::Message;
            let bytes = session
                .spclient()
                .get_rootlist(0, Some(ROOTLIST_PAGE))
                .await
                .unwrap();
            let page =
                librespot_protocol::playlist4_external::SelectedListContent::parse_from_bytes(
                    &bytes,
                )
                .unwrap();
            let rows = page.contents.as_ref().unwrap();
            let mut sample = None;
            for (item, meta) in rows.items.iter().zip(&rows.meta_items) {
                let Some(id) = item.uri().strip_prefix("spotify:playlist:") else {
                    continue;
                };
                if !list_images(meta.attributes.get_or_default()).is_empty() {
                    continue;
                }
                let tracks = read(session, &format!("playlist:{id}"), 0).await.unwrap();
                if tracks.iter().any(|track| !track.album.images.is_empty()) {
                    sample = Some(id.to_owned());
                    break;
                }
            }
            let id = sample.expect("Account needs a nonempty playlist with a default album cover");
            let playlists = library(session).await.unwrap();
            let playlist = playlists.iter().find(|playlist| playlist.id == id).unwrap();
            assert!(
                !playlist.images.is_empty(),
                "Library dropped artwork for a nonempty playlist without an uploaded cover"
            );
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap();
            let response = client.get(&playlist.images[0].url).send().await.unwrap();
            assert!(
                response.status().is_success(),
                "Default playlist cover must load from Spotify CDN"
            );
            let image = image::load_from_memory(&response.bytes().await.unwrap()).unwrap();
            assert!(image.width() > 0);
            println!("Default playlist artwork available and decodes successfully");
        });
    }
    #[test]
    fn owner_profile_identity_and_artwork_are_validated() {
        let profile = crate::native_login::profile_from_response("friend", br#"{"uri":"spotify:user:friend","name":" Friend ","image_url":"https://artwork.example.test/avatar"}"#).unwrap();
        assert_eq!(profile.display_name.as_deref(), Some("Friend"));
        assert_eq!(profile.images.len(), 1);
        assert!(
            crate::native_login::profile_from_response(
                "friend",
                br#"{"uri":"spotify:user:someone-else","name":"Other"}"#
            )
            .is_err()
        );
        let profile = crate::native_login::profile_from_response(
            "friend",
            br#"{"name":"Friend","image_url":"http://artwork.example.test/avatar"}"#,
        )
        .unwrap();
        assert!(profile.images.is_empty());
    }

    #[test]
    #[ignore = "Reads public playlist creator profiles using saved native sign-in; no playback or playlist writes"]
    fn live_public_playlist_owner_profile() {
        let login = crate::native_login::authenticate(
            &crate::auth::http_client().unwrap(),
            true,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap()
        .expect("Saved native login required");
        login.runtime.block_on(async {
            let session = &login.connection.0;
            let query =
                std::env::var("LYSSNA_TEST_PLAYLIST_QUERY").unwrap_or_else(|_| "road trip".into());
            let playlists = crate::search::playlists(session, &query).await.unwrap();
            let playlist = playlists
                .iter()
                .find(|playlist| {
                    !playlist.owner.id.is_empty()
                        && playlist.owner.id != "spotify"
                        && playlist.owner.id != session.username()
                })
                .expect("Search needs a public playlist by another user");
            let details = playlist_info(session, &playlist.id)
                .await
                .unwrap()
                .playlist
                .unwrap();
            let profile = details
                .owner_profile
                .expect("Public creator profile was not resolved");
            assert_eq!(profile.id, details.owner);
            assert!(
                profile
                    .display_name
                    .as_ref()
                    .is_some_and(|name| !name.is_empty())
            );
            let started = Instant::now();
            let cached = owner_profile(session, &details.owner).await.unwrap();
            assert_eq!(cached.display_name, profile.display_name);
            assert!(
                started.elapsed() < Duration::from_millis(100),
                "Repeated creator lookup must use cache"
            );
            println!(
                "Public creator display name resolved; avatar available={}; cached lookup={} ms",
                !profile.images.is_empty(),
                started.elapsed().as_millis()
            );
        });
    }

    #[test]
    fn profile_view_display_name_is_trimmed_and_required() {
        assert_eq!(
            owner_name(br#"{"uri":"spotify:user:friend","name":" Friend "}"#).as_deref(),
            Some("Friend")
        );
        assert_eq!(owner_name(br#"{"name":""}"#), None);
        assert_eq!(owner_name(b"not json"), None);
    }
}

#[cfg(test)]
mod library_write_tests {
    use super::*;
    #[test]
    fn play_history_keeps_song_contexts_and_dates() {
        let plays = recent_plays(&json!({"playContexts":[
            {"uri":"spotify:album:a","lastPlayedTrackUri":"spotify:track:t","lastPlayedTime":1_760_000_000_000_i64},
            {"uri":"spotify:show:s","lastPlayedTrackUri":"spotify:episode:e","lastPlayedTime":1_760_000_000_000_i64},
            {"uri":"spotify:playlist:p","lastPlayedTrackUri":"spotify:track:u","lastPlayedTime":1_760_000_000_i64}
        ]}))
        .unwrap();
        assert_eq!(plays.len(), 2);
        assert_eq!(plays[0].context, "spotify:album:a");
        assert_eq!(plays[0].track, "spotify:track:t");
        assert_eq!(plays[0].played_at, plays[1].played_at);
        assert!(plays[0].played_at.starts_with("2025-10-09T"));
        assert!(recent_plays(&json!({})).is_err());
    }
    #[test]
    fn playlist_change_appends_items_at_the_end() {
        let changes = add_changes(&["spotify:track:a".into(), "spotify:track:b".into()]);
        let op = &changes["deltas"][0]["ops"][0];
        assert_eq!(op["kind"], 2);
        assert_eq!(op["add"]["addLast"], true);
        let uris: Vec<_> = op["add"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["uri"].as_str().unwrap())
            .collect();
        assert_eq!(uris, ["spotify:track:a", "spotify:track:b"]);
    }
}

#[cfg(test)]
mod live_playlist_tests {
    use super::*;
    #[test]
    #[ignore = "Creates a temporary private playlist with the saved sign-in, adds songs twice, then removes it"]
    fn live_playlist_create_add_and_remove() {
        crate::native::with_live_session(async |session: &Session| {
            let id = create_playlist(session, "Lyssna write test")
                .await
                .expect("Native playlist creation failed");
            let checked = async {
                let listed = library(session).await?;
                ensure!(
                    listed.iter().any(|playlist| playlist.id == id),
                    "New playlist missing from library"
                );
                let recent = recent(session).await?;
                let mut picks: Vec<Track> = Vec::new();
                for track in recent {
                    if picks.len() < 3 && !picks.iter().any(|pick| pick.uri == track.uri) {
                        picks.push(track);
                    }
                }
                let first = add_to_playlist(session, &id, &picks).await?;
                let again = add_to_playlist(session, &id, &picks).await?;
                edit_details(session, &id, "Lyssna edit test", "Temporary test").await?;
                edit_details(session, &id, "Lyssna edit test", "").await?;
                let mut jpeg = Vec::new();
                image::RgbImage::from_pixel(300, 300, image::Rgb([30, 140, 90])).write_to(
                    &mut std::io::Cursor::new(&mut jpeg),
                    image::ImageFormat::Jpeg,
                )?;
                let cover = set_cover(session, &id, &jpeg).await?;
                ensure!(
                    cover.starts_with("https://i.scdn.co/image/"),
                    "Unexpected cover URL"
                );
                move_song(session, &id, 0, &picks[0].uri, picks.len() - 1).await?;
                move_song(session, &id, picks.len() - 1, &picks[0].uri, 0).await?;
                remove_from_playlist(session, &id, 0, &picks[0].uri).await?;
                Ok::<_, anyhow::Error>((picks.len(), first, again))
            }
            .await;
            let removed = remove_from_library(session, &id).await;
            let gone = library(session)
                .await
                .map(|listed| !listed.iter().any(|playlist| playlist.id == id));
            let (picked, first, again) = checked
                .map_err(|error| anyhow::anyhow!("{error:#}"))
                .expect("Native playlist checks failed");
            removed.expect("Native playlist removal failed");
            assert!(
                gone.expect("Library reread failed"),
                "Playlist still listed"
            );
            match (first, again) {
                (Added::Count(first), Added::Count(again)) => {
                    assert_eq!(first, picked);
                    assert_eq!(again, 0);
                    eprintln!(
                        "Native playlist: created, added {first} songs, skipped {picked} duplicates, edited details, set a cover, moved a song both ways, removed one song, removed playlist"
                    );
                }
                (Added::NotWritten(error), _) | (_, Added::NotWritten(error)) => {
                    panic!("Native playlist change was not written: {error:#}")
                }
            }
        });
    }
}

#[cfg(test)]
mod live_folder_tests {
    use super::*;
    #[test]
    #[ignore = "Creates a temporary folder and playlist with the saved sign-in, edits them, then removes both"]
    fn live_folder_create_move_rename_and_delete() {
        crate::native::with_live_session(async |session: &Session| {
            let folder = create_folder(session, "Lyssna folder test")
                .await
                .expect("Native folder creation failed");
            let playlist = create_playlist(session, "Lyssna folder playlist").await;
            let checked = async {
                let playlist = playlist
                    .as_ref()
                    .map_err(|error| anyhow::anyhow!("{error:#}"))?;
                move_to_folder(session, playlist, Some(&folder)).await?;
                let listed = library(session).await?;
                let inside = listed
                    .iter()
                    .find(|item| item.id == *playlist)
                    .context("Playlist missing")?;
                ensure!(
                    inside.folders.len() == 1 && inside.folders[0].name == "Lyssna folder test",
                    "Folder path not read back"
                );
                rename_folder(session, &folder, "Lyssna folder & test 2").await?;
                let renamed = library(session).await?;
                ensure!(
                    renamed.iter().any(|item| item
                        .folders
                        .iter()
                        .any(|f| f.name == "Lyssna folder & test 2")),
                    "Renamed folder not read back"
                );
                move_to_folder(session, playlist, None).await?;
                Ok::<_, anyhow::Error>(())
            }
            .await;
            let deleted = delete_folder(session, &folder).await;
            if let Ok(playlist) = &playlist {
                let _ = remove_from_library(session, playlist).await;
            }
            checked
                .map_err(|error| anyhow::anyhow!("{error:#}"))
                .expect("Native folder checks failed");
            deleted.expect("Native folder removal failed");
            eprintln!(
                "Native folders: created, moved a playlist in, read its folder path, renamed, moved it out, deleted"
            );
        });
    }
}

#[cfg(test)]
mod live_liked_artist_tests {
    use super::*;
    #[test]
    #[ignore = "Uses saved native sign-in for read-only library checks; never writes"]
    fn live_liked_by_artist_counts_and_lists_the_artists_liked_songs() {
        crate::native::with_live_session(async |session: &Session| {
            let liked = read(session, "liked", 0)
                .await
                .expect("Liked Songs unreadable");
            let artist = liked
                .iter()
                .flat_map(|track| &track.artists)
                .find_map(|artist| artist.id.clone())
                .expect("No liked song with an artist");
            let count = liked_by_artist(session, &artist)
                .await
                .expect("Artist liked count failed");
            let listed = read(session, &format!("liked-artist:{artist}"), 0)
                .await
                .expect("Liked Songs by artist unreadable");
            assert!(count >= 1, "A liked song's artist counted none");
            assert_eq!(listed.len(), count.min(50));
            assert!(listed.iter().all(|track| {
                track
                    .artists
                    .iter()
                    .any(|item| item.id.as_deref() == Some(artist.as_str()))
            }));
            eprintln!("liked-by-artist count: {count}");
        });
    }
}

/// Track metadata for many songs in one extended-metadata request, as
/// Spotify's own clients batch it. Missing or unparsable entries are left out.
pub(crate) async fn tracks_batched(
    session: &Session,
    uris: &[String],
) -> Result<HashMap<String, Track>> {
    use librespot_protocol::extended_metadata::{
        BatchedEntityRequest, EntityRequest, ExtensionQuery,
    };
    use librespot_protocol::extension_kind::ExtensionKind;
    use protobuf::{EnumOrUnknown, Message};
    let request = BatchedEntityRequest {
        entity_request: uris
            .iter()
            .map(|uri| EntityRequest {
                entity_uri: uri.clone(),
                query: vec![ExtensionQuery {
                    extension_kind: EnumOrUnknown::new(ExtensionKind::TRACK_V4),
                    ..Default::default()
                }],
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let response = session.spclient().get_extended_metadata(request).await?;
    let user = session.user_data();
    let mut tracks = HashMap::new();
    for array in response.extended_metadata {
        for entry in array.extension_data {
            let Some(data) = entry.extension_data.as_ref() else {
                continue;
            };
            let Ok(message) = librespot_protocol::metadata::Track::parse_from_bytes(&data.value)
            else {
                continue;
            };
            if let Some(track) = librespot_metadata::Track::try_from(&message)
                .ok()
                .and_then(|item| crate::native::metadata_track(&item, Some(&user)))
            {
                tracks.insert(entry.entity_uri.clone(), track);
            }
        }
    }
    Ok(tracks)
}

#[cfg(test)]
mod live_timing_tests {
    use super::*;
    #[test]
    #[ignore = "Counts unplayable songs in saved playlists with the saved sign-in; never writes"]
    fn live_unplayable_songs_are_a_minority() {
        crate::native::with_live_session(async |session: &Session| {
            let playlists = library(session).await.expect("Library unreadable");
            let (mut total, mut unavailable) = (0, Vec::new());
            for playlist in playlists.iter().take(12) {
                let Ok(read) = read_playlist(session, &playlist.id).await else {
                    continue;
                };
                let uris: Vec<String> = read
                    .tracks()
                    .map(|uri| uri.to_uri())
                    .filter(|uri| uri.starts_with("spotify:track:"))
                    .take(500)
                    .collect();
                let tracks = tracks_batched(session, &uris).await.expect("Batch failed");
                total += tracks.len();
                unavailable.extend(
                    tracks
                        .values()
                        .filter(|track| track.unavailable)
                        .map(|track| format!("{} - {}", track.name, track.artist())),
                );
            }
            eprintln!(
                "{} of {total} unavailable: {unavailable:?}",
                unavailable.len()
            );
            assert!(total > 0);
            assert!(
                unavailable.len() * 10 < total,
                "Most songs must stay playable"
            );
        });
    }
    #[test]
    #[ignore = "Times read-only playlist loads with the saved sign-in; never writes"]
    fn live_playlist_load_timing() {
        crate::native::with_live_session(async |session: &Session| {
            let started = Instant::now();
            let playlists = library(session).await.expect("Library unreadable");
            eprintln!(
                "library: {} playlists in {:?}",
                playlists.len(),
                started.elapsed()
            );
            let mut biggest = (0, String::new(), Vec::new());
            for playlist in playlists.iter().take(12) {
                let started = Instant::now();
                let Ok(read) = read_playlist(session, &playlist.id).await else {
                    continue;
                };
                let uris: Vec<String> = read.tracks().map(|uri| uri.to_uri()).collect();
                eprintln!(
                    "playlist get: {} tracks in {:?}",
                    uris.len(),
                    started.elapsed()
                );
                if uris.len() > biggest.0 {
                    biggest = (uris.len(), playlist.id.clone(), uris);
                }
            }
            let uris: Vec<String> = biggest
                .2
                .into_iter()
                .filter(|uri| uri.starts_with("spotify:track:"))
                .collect();
            let started = Instant::now();
            let mut offset = 0;
            let mut slowest = Duration::ZERO;
            let mut unavailable = 0;
            loop {
                let page = Instant::now();
                let tracks = read(session, &biggest.1, offset)
                    .await
                    .expect("Read failed");
                slowest = slowest.max(page.elapsed());
                if tracks.is_empty() {
                    break;
                }
                assert!(tracks.iter().all(|track| !track.name.is_empty()));
                unavailable += tracks
                    .iter()
                    .filter(|track| track.name == "Unavailable track")
                    .count();
                offset += tracks.len();
            }
            eprintln!(
                "read: {offset} items ({} songs, {unavailable} unavailable) in {:?}, slowest page {:?}",
                uris.len(),
                started.elapsed(),
                slowest
            );
        });
    }
}

#[cfg(test)]
mod live_recent_tests {
    use super::*;
    #[test]
    #[ignore = "Uses saved native sign-in for read-only play history checks; never writes"]
    fn live_recents_label_liked_songs_plays() {
        crate::native::with_live_session(async |session: &Session| {
            for pass in ["first", "second"] {
                let started = Instant::now();
                let tracks = recent(session).await.expect("Play history unreadable");
                let liked = tracks
                    .iter()
                    .filter(|track| track.played_context.ends_with(":collection"))
                    .count();
                eprintln!(
                    "{pass} load: {} plays, {liked} from Liked Songs, {:?}",
                    tracks.len(),
                    started.elapsed()
                );
            }
        });
    }
}
