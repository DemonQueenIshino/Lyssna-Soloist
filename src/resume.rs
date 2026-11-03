//! Per-account listening position, without credentials or automatic playback.
use crate::model::{Playback, Track};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Deserialize, Serialize)]
pub struct LastPlayed {
    pub track: Track,
    pub position_ms: u64,
    pub played_at: u64,
    #[serde(default)]
    pub queue: Vec<String>,
    #[serde(default)]
    pub dj_context: Option<DJContext>,
    /// The playlist, album, artist or Liked Songs the song played from, so a
    /// restart carries on through it instead of a short saved list.
    #[serde(default)]
    pub context: Option<String>,
    /// Songs the listener queued, put back in the queue on restore.
    #[serde(default)]
    pub queued: Vec<String>,
}

fn valid_track(uri: &str) -> bool {
    uri.strip_prefix("spotify:track:")
        .is_some_and(|id| id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}

/// A context a restart may load: a playlist, album or artist by ID, or a
/// user's Liked Songs collection.
pub(crate) fn valid_context(uri: &str) -> bool {
    let id = |id: &str| id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric());
    if let Some(user) = uri
        .strip_prefix("spotify:user:")
        .and_then(|rest| rest.strip_suffix(":collection"))
    {
        return !user.is_empty()
            && user.len() <= 64
            && user
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte));
    }
    // DJ restores through its own session, never as a plain playlist.
    uri != crate::home::DJ_URI
        && ["spotify:playlist:", "spotify:album:", "spotify:artist:"]
            .iter()
            .any(|prefix| uri.strip_prefix(prefix).is_some_and(id))
}

type Contexts = HashMap<String, (Option<String>, Vec<String>)>;
static CONTEXTS: LazyLock<Mutex<Contexts>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// What is playing from, and which songs the listener queued, as the native
/// player last reported them.
pub fn remember_native_context(account: &str, context: Option<String>, queued: Vec<String>) {
    if let Ok(mut contexts) = CONTEXTS.lock() {
        contexts.insert(
            account.into(),
            (
                context.filter(|uri| valid_context(uri)),
                queued
                    .into_iter()
                    .filter(|uri| valid_track(uri))
                    .take(50)
                    .collect(),
            ),
        );
    }
}
fn saved_context(account: &str) -> (Option<String>, Vec<String>) {
    CONTEXTS
        .lock()
        .ok()
        .and_then(|contexts| contexts.get(account).cloned())
        .unwrap_or_default()
}
/// Load a restored song from its playlist, album or Liked Songs, with the
/// listener's queue after it, when the saved song played from one.
pub fn in_context(
    account: &str,
    action: crate::api::PlayerAction,
    with_queue: bool,
) -> crate::api::PlayerAction {
    match action {
        crate::api::PlayerAction::Resume {
            uris,
            offset,
            position_ms,
            ..
        } => {
            let (context, queued) = saved_context(account);
            // The list as it was played, such as a Random sort, keeps its
            // order instead of reloading the playlist's own.
            let listed = context.as_ref().and_then(|context| {
                let order = saved_order(account, context)?;
                let at = order.iter().position(|uri| Some(uri) == uris.get(offset))?;
                Some((order, at))
            });
            let (uris, offset, listed) = match listed {
                Some((order, at)) => (order, at, true),
                None => (uris, offset, false),
            };
            crate::api::PlayerAction::Resume {
                uris,
                offset,
                position_ms,
                context,
                queued: if with_queue { queued } else { Vec::new() },
                listed,
            }
        }
        action => action,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DJContext {
    pub uri: String,
    pub session_url: Option<String>,
    pub track_uid: Option<String>,
}
impl DJContext {
    fn valid(&self) -> bool {
        (self.uri == crate::home::DJ_URI || self.uri.starts_with("spotify:lexicon:"))
            && self.uri.len() < 1024
            && !self.uri.chars().any(char::is_control)
            && self
                .track_uid
                .as_ref()
                .is_none_or(|uid| uid.len() < 1024 && !uid.chars().any(char::is_control))
            && self.session_url.as_ref().is_none_or(|url| {
                url.starts_with("hm://lexicon-session-provider/context-resolve/")
                    && url.len() < 8192
                    && !url.contains("..")
                    && !url.chars().any(char::is_control)
            })
    }
}
type DJHistory = HashMap<String, (String, DJContext)>;
static DJ_CONTEXTS: LazyLock<Mutex<DJHistory>> = LazyLock::new(|| Mutex::new(HashMap::new()));
pub fn remember_dj_context(account: &str, track: &str, context: Option<DJContext>) {
    let changed = if let Ok(mut contexts) = DJ_CONTEXTS.lock() {
        if let Some(context) = context.filter(DJContext::valid) {
            let changed = contexts.get(account) != Some(&(track.into(), context.clone()));
            contexts.insert(account.into(), (track.into(), context));
            changed
        } else {
            contexts.remove(account).is_some()
        }
    } else {
        false
    };
    if changed && let Ok(mut saves) = SAVES.lock() {
        saves.remove(account);
    }
}
pub fn remember_native_queue(account: &str, uris: Vec<String>) {
    if let Ok(mut queues) = QUEUES.lock() {
        queues.insert(account.into(), uris.into_iter().take(100).collect());
    }
}
fn dj_context(account: &str, track: &str) -> Option<DJContext> {
    DJ_CONTEXTS
        .lock()
        .ok()?
        .get(account)
        .filter(|(uri, _)| uri == track)
        .map(|(_, context)| context.clone())
}
pub fn restore_dj_action(
    account: &str,
    track: &str,
    position_ms: u64,
    requested: &crate::api::PlayerAction,
) -> Option<crate::api::PlayerAction> {
    let relative_offset = match requested {
        crate::api::PlayerAction::Play(None) => 0,
        crate::api::PlayerAction::Next => 1,
        crate::api::PlayerAction::Previous => {
            if position_ms >= 3000 {
                0
            } else {
                -1
            }
        }
        _ => return None,
    };
    let context = dj_context(account, track)?;
    Some(crate::api::PlayerAction::RestoreDJ {
        context: context.uri,
        session_url: context.session_url,
        track: track.into(),
        track_uid: context.track_uid,
        position_ms: if matches!(requested, crate::api::PlayerAction::Play(None)) {
            position_ms
        } else {
            0
        },
        relative_offset,
    })
}
static QUEUES: LazyLock<Mutex<HashMap<String, Vec<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// Longest list order kept for a restart.
const ORDER_LIMIT: usize = 10_000;
type Orders = HashMap<String, (String, Vec<String>)>;
static ORDERS: LazyLock<Mutex<Orders>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// The order a playlist, album or Liked Songs was started in, when it was
/// played from its list (any sort, Random included). Kept in its own file,
/// written only when a list starts, not with every position save.
fn remember_order(account: &str, order: Option<(String, Vec<String>)>) {
    let order = order.filter(|(context, uris)| valid_order(context, uris));
    if let Ok(mut orders) = ORDERS.lock() {
        if orders.get(account) == order.as_ref() {
            return;
        }
        match &order {
            Some(order) => orders.insert(account.into(), order.clone()),
            None => orders.remove(account),
        };
    }
    // Tests keep orders in memory, away from the real application data.
    let Ok(path) = order_path(account).and_then(|path| {
        anyhow::ensure!(!cfg!(test), "Not saved in tests");
        Ok(path)
    }) else {
        return;
    };
    match order {
        Some(order) => {
            let _ = std::fs::create_dir_all(path.parent().unwrap()).and_then(|()| {
                let temporary = path.with_extension("tmp");
                std::fs::write(&temporary, serde_json::to_vec(&order)?)?;
                std::fs::rename(temporary, &path)
            });
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
}
fn valid_order(context: &str, uris: &[String]) -> bool {
    valid_context(context)
        && !uris.is_empty()
        && uris.len() <= ORDER_LIMIT
        && uris.iter().all(|uri| valid_track(uri))
}
fn saved_order(account: &str, context: &str) -> Option<Vec<String>> {
    let cached = ORDERS.lock().ok()?.get(account).cloned();
    let (saved, order) = match cached {
        Some(order) => order,
        None => {
            let path = order_path(account).ok().filter(|_| !cfg!(test))?;
            if std::fs::metadata(&path).ok()?.len() > 2_000_000 {
                return None;
            }
            let order: (String, Vec<String>) =
                serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
            if !valid_order(&order.0, &order.1) {
                return None;
            }
            if let Ok(mut orders) = ORDERS.lock() {
                orders.insert(account.into(), order.clone());
            }
            order
        }
    };
    (saved == context).then_some(order)
}
pub fn remember_queue(account: &str, action: &crate::api::PlayerAction) {
    let queue = match action {
        crate::api::PlayerAction::PlayUris { uris, .. }
        | crate::api::PlayerAction::Resume { uris, .. } => uris.clone(),
        crate::api::PlayerAction::Play(Some(uri)) => vec![uri.clone()],
        // A list started in its own order forgets an earlier sorted one.
        crate::api::PlayerAction::PlayContext { .. }
        | crate::api::PlayerAction::PreparedDJ { .. } => {
            remember_order(account, None);
            return;
        }
        _ => return,
    };
    remember_dj_context(account, "", None);
    let context = match action {
        crate::api::PlayerAction::PlayUris { context, .. } => context.clone(),
        _ => None,
    };
    remember_native_context(account, context.clone(), Vec::new());
    match action {
        crate::api::PlayerAction::PlayUris { uris, .. } => {
            remember_order(account, context.map(|context| (context, uris.clone())))
        }
        crate::api::PlayerAction::Play(Some(_)) => remember_order(account, None),
        _ => {}
    }
    if let Ok(mut queues) = QUEUES.lock() {
        queues.insert(account.into(), queue.into_iter().take(100).collect());
    }
}
pub fn restore_action(uri: &str, queue: &[String], position_ms: u64) -> crate::api::PlayerAction {
    let (uris, offset) = queue
        .iter()
        .position(|entry| entry == uri)
        .map_or_else(|| (vec![uri.into()], 0), |offset| (queue.to_vec(), offset));
    crate::api::PlayerAction::Resume {
        uris,
        offset,
        position_ms,
        context: None,
        queued: Vec::new(),
        listed: false,
    }
}
/// Next or Previous on a restored, not yet loaded song: start the saved queue
/// one song on or back. Previous restarts the song once past three seconds,
/// as Spotify does. None when there is no song in that direction.
pub fn skip_action(
    uri: &str,
    queue: &[String],
    position_ms: u64,
    forward: bool,
) -> Option<crate::api::PlayerAction> {
    let index = queue.iter().position(|entry| entry == uri)?;
    let offset = if forward {
        Some(index + 1).filter(|next| *next < queue.len())?
    } else if position_ms > 3_000 {
        index
    } else {
        index.checked_sub(1)?
    };
    Some(crate::api::PlayerAction::Resume {
        uris: queue.to_vec(),
        offset,
        position_ms: 0,
        context: None,
        queued: Vec::new(),
        listed: false,
    })
}
pub fn queue(account: &str) -> Vec<String> {
    QUEUES
        .lock()
        .ok()
        .and_then(|queues| queues.get(account).cloned())
        .unwrap_or_default()
}
type SaveHistory = HashMap<String, (String, bool, Instant)>;
static SAVES: LazyLock<Mutex<SaveHistory>> = LazyLock::new(|| Mutex::new(HashMap::new()));
fn path(account: &str) -> Result<std::path::PathBuf> {
    let digest = Sha256::digest(account.as_bytes());
    let key = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(crate::config::data_dir()?
        .join("resume")
        .join(format!("{key}.json")))
}
fn order_path(account: &str) -> Result<std::path::PathBuf> {
    Ok(path(account)?.with_extension("order.json"))
}
pub fn load(account: &str) -> Option<LastPlayed> {
    let path = path(account).ok()?;
    if std::fs::metadata(&path).ok()?.len() > 2_000_000 {
        return None;
    }
    let value = decode(&std::fs::read(path).ok()?)?;
    if let Ok(mut queues) = QUEUES.lock() {
        queues.insert(account.into(), value.queue.clone());
    }
    if let Ok(mut contexts) = CONTEXTS.lock() {
        contexts
            .entry(account.into())
            .or_insert_with(|| (value.context.clone(), value.queued.clone()));
    }
    if value.dj_context.is_some() && dj_context(account, &value.track.uri).is_none() {
        remember_dj_context(account, &value.track.uri, value.dj_context.clone());
    }
    Some(value)
}
fn decode(bytes: &[u8]) -> Option<LastPlayed> {
    if bytes.len() > 2_000_000 {
        return None;
    }
    let mut value: LastPlayed = serde_json::from_slice(bytes).ok()?;
    if value.queue.len() > 100 || value.queue.iter().any(|uri| !valid_track(uri)) {
        return None;
    }
    value.context = value.context.filter(|uri| valid_context(uri));
    value.queued.retain(|uri| valid_track(uri));
    value.queued.truncate(50);
    value.dj_context = value.dj_context.filter(DJContext::valid);
    value
        .track
        .uri
        .starts_with("spotify:track:")
        .then_some(value)
}
pub fn newer(local: Option<LastPlayed>, spotify: Option<LastPlayed>) -> Option<LastPlayed> {
    match (local, spotify) {
        (Some(mut local), Some(spotify)) => Some(if spotify.played_at > local.played_at {
            spotify
        } else {
            if local.dj_context.is_none() && local.track.uri == spotify.track.uri {
                local.dj_context = spotify.dj_context;
            }
            local
        }),
        (local, spotify) => local.or(spotify),
    }
}
pub fn save(account: &str, playback: &Playback, observed_at: u64, force: bool) -> Result<()> {
    let Some(track) = playback.item.as_ref().filter(|track| !track.is_local) else {
        return Ok(());
    };
    let mut saves = SAVES
        .lock()
        .map_err(|_| anyhow::anyhow!("Resume storage unavailable"))?;
    if !force
        && saves.get(account).is_some_and(|(uri, playing, saved)| {
            uri == &track.uri
                && *playing == playback.is_playing
                && saved.elapsed() < Duration::from_secs(5)
        })
    {
        return Ok(());
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let position_ms = (playback.progress_ms.unwrap_or_default()
        + if playback.is_playing && playback.device.name != crate::native::device_name() {
            now.saturating_sub(observed_at)
        } else {
            0
        })
    .min(track.duration_ms);
    let value = LastPlayed {
        track: track.clone(),
        position_ms,
        played_at: now,
        queue: QUEUES
            .lock()
            .ok()
            .and_then(|queues| {
                queues
                    .get(account)
                    .filter(|queue| queue.contains(&track.uri))
                    .cloned()
            })
            .unwrap_or_default(),
        dj_context: dj_context(account, &track.uri),
        context: saved_context(account).0,
        queued: saved_context(account).1,
    };
    let path = path(account)?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, serde_json::to_vec(&value)?)?;
    std::fs::rename(temporary, path)?;
    saves.insert(
        account.into(),
        (track.uri.clone(), playback.is_playing, Instant::now()),
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    #[test]
    fn skipping_a_restored_song_moves_through_the_saved_queue() {
        use crate::api::PlayerAction;
        let queue: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        let offset = |action: Option<PlayerAction>| match action {
            Some(PlayerAction::Resume {
                offset,
                position_ms: 0,
                ..
            }) => Some(offset),
            _ => None,
        };
        assert_eq!(
            offset(super::skip_action("b", &queue, 60_000, true)),
            Some(2)
        );
        assert_eq!(
            offset(super::skip_action("b", &queue, 1_000, false)),
            Some(0)
        );
        assert_eq!(
            offset(super::skip_action("b", &queue, 60_000, false)),
            Some(1)
        );
        assert_eq!(offset(super::skip_action("c", &queue, 0, true)), None);
        assert_eq!(offset(super::skip_action("a", &queue, 0, false)), None);
        assert_eq!(offset(super::skip_action("x", &queue, 0, true)), None);
    }
    use super::*;
    #[test]
    fn reopening_dj_restores_session_position_and_can_skip_without_refresh() {
        let account = "test-dj-reopen";
        let uri = "spotify:track:abcdefghijklmnopqrstuv";
        let context = DJContext {
            uri: crate::home::DJ_URI.into(),
            session_url: Some(
                "hm://lexicon-session-provider/context-resolve/v2/session?session_id=test".into(),
            ),
            track_uid: Some("set-2-song-3".into()),
        };
        let saved = LastPlayed {
            track: Track {
                uri: uri.into(),
                ..Track::default()
            },
            position_ms: 77000,
            played_at: 10,
            queue: vec![uri.into(), "spotify:track:bcdefghijklmnopqrstuvw".into()],
            dj_context: Some(context.clone()),
            context: None,
            queued: Vec::new(),
        };
        let reopened = decode(&serde_json::to_vec(&saved).unwrap()).unwrap();
        remember_dj_context(account, uri, reopened.dj_context);
        remember_native_queue(account, reopened.queue);
        assert!(
            matches!(restore_dj_action(account,uri,reopened.position_ms,&crate::api::PlayerAction::Play(None)),Some(crate::api::PlayerAction::RestoreDJ{position_ms:77000,relative_offset:0,session_url,..}) if session_url==context.session_url)
        );
        assert!(
            matches!(restore_dj_action(account,uri,reopened.position_ms,&crate::api::PlayerAction::Next),Some(crate::api::PlayerAction::RestoreDJ{position_ms:0,relative_offset:1,track_uid,..}) if track_uid==context.track_uid)
        );
        assert!(matches!(
            restore_dj_action(account, uri, 77000, &crate::api::PlayerAction::Previous),
            Some(crate::api::PlayerAction::RestoreDJ {
                position_ms: 0,
                relative_offset: 0,
                ..
            })
        ));
        assert!(
            restore_dj_action(
                "another-account",
                uri,
                77000,
                &crate::api::PlayerAction::Next
            )
            .is_none()
        );
        assert!(
            restore_dj_action(
                account,
                "spotify:track:different",
                77000,
                &crate::api::PlayerAction::Next
            )
            .is_none()
        );
        remember_queue(account, &crate::api::PlayerAction::Play(Some(uri.into())));
        assert!(restore_dj_action(account, uri, 77000, &crate::api::PlayerAction::Next).is_none());
    }
    #[test]
    fn older_saved_files_remain_readable_and_reject_other_service_urls() {
        let saved = serde_json::json!({"track":{"uri":"spotify:track:abcdefghijklmnopqrstuv"},"position_ms":77000,"played_at":10});
        let value = decode(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert!(value.dj_context.is_none());
        let context = DJContext {
            uri: crate::home::DJ_URI.into(),
            session_url: Some("hm://keymaster/token/authenticated".into()),
            track_uid: None,
        };
        assert!(!context.valid());
    }
    #[test]
    fn a_restore_carries_on_through_its_playlist_with_the_queue() {
        let account = "context-restore-test";
        let song = "spotify:track:abcdefghijklmnopqrstuv";
        let queued = "spotify:track:bcdefghijklmnopqrstuvw";
        remember_native_context(
            account,
            Some("spotify:playlist:0123456789abcdefghijkl".into()),
            vec![queued.into(), "spotify:episode:nope".into()],
        );
        let restored = in_context(account, restore_action(song, &[song.into()], 5000), true);
        assert!(matches!(
            &restored,
            crate::api::PlayerAction::Resume { context: Some(context), queued: list, .. }
                if context == "spotify:playlist:0123456789abcdefghijkl" && list == &[queued]
        ));
        // A finished song starts over without bringing the old queue back.
        let finished = in_context(account, restore_action(song, &[song.into()], 0), false);
        assert!(matches!(
            finished,
            crate::api::PlayerAction::Resume { queued, .. } if queued.is_empty()
        ));
        // DJ and unknown contexts are never saved as a playlist.
        remember_native_context(account, Some(crate::home::DJ_URI.into()), vec![]);
        assert!(saved_context(account).0.is_none());
    }
    #[test]
    fn a_restore_keeps_the_order_the_list_was_played_in() {
        use crate::api::PlayerAction;
        let account = "order-restore-test";
        let playlist = "spotify:playlist:0123456789abcdefghijkl";
        let song = |n: usize| format!("spotify:track:{n:0>22}");
        // Played from a Random sort: the list order differs from the
        // playlist's own, and is longer than the saved window.
        let order: Vec<String> = (0..300).rev().map(song).collect();
        remember_queue(
            account,
            &PlayerAction::PlayUris {
                uris: order.clone(),
                offset: 0,
                context: Some(playlist.into()),
            },
        );
        remember_native_context(account, Some(playlist.into()), vec![]);
        let window = vec![song(151), song(150), song(149)];
        let restored = in_context(account, restore_action(&song(150), &window, 5000), true);
        assert!(matches!(
            &restored,
            PlayerAction::Resume { uris, offset, listed: true, context: Some(context), position_ms: 5000, .. }
                if uris == &order && uris[*offset] == song(150) && context == playlist
        ));
        // Starting the playlist in its own order forgets the sorted one.
        remember_queue(
            account,
            &PlayerAction::PlayContext {
                context: playlist.into(),
                offset: None,
            },
        );
        let restored = in_context(account, restore_action(&song(150), &window, 5000), true);
        assert!(matches!(
            restored,
            PlayerAction::Resume {
                listed: false,
                offset: 1,
                ..
            }
        ));
    }
    #[test]
    fn saved_contexts_are_limited_to_playlists_albums_artists_and_liked_songs() {
        for uri in [
            "spotify:playlist:0123456789abcdefghijkl",
            "spotify:album:0123456789abcdefghijkl",
            "spotify:artist:0123456789abcdefghijkl",
            "spotify:user:some.user_1:collection",
        ] {
            assert!(valid_context(uri), "{uri}");
        }
        for uri in [
            "spotify:playlist:short",
            "spotify:user::collection",
            "spotify:user:a/b:collection",
            "https://example.test/",
            "spotify:show:0123456789abcdefghijkl",
        ] {
            assert!(!valid_context(uri), "{uri}");
        }
    }
    #[test]
    fn restoring_playback_preserves_its_queue_position() {
        let queue: Vec<String> = vec![
            "spotify:track:first".into(),
            "spotify:track:second".into(),
            "spotify:track:third".into(),
        ];
        assert!(
            matches!(restore_action(&queue[1],&queue,77000),crate::api::PlayerAction::Resume{uris,offset:1,position_ms:77000,..}if uris==queue)
        );
        assert!(
            matches!(restore_action("spotify:track:other",&queue,45000),crate::api::PlayerAction::Resume{uris,offset:0,position_ms:45000,..}if uris==["spotify:track:other"])
        );
    }
    #[test]
    fn most_recent_listening_source_wins() {
        let value = |played_at| LastPlayed {
            track: Track::default(),
            position_ms: 50,
            played_at,
            queue: Vec::new(),
            dj_context: None,
            context: None,
            queued: Vec::new(),
        };
        assert_eq!(
            newer(Some(value(20)), Some(value(10))).unwrap().played_at,
            20
        );
        assert_eq!(
            newer(Some(value(20)), Some(value(30))).unwrap().played_at,
            30
        );
        assert!(newer(None, None).is_none());
    }
}
