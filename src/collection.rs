//! Saved songs, albums and followed artists through Spotify's own collection
//! service, mirrored locally so library checks need no request per item.
use anyhow::{Context, Result, ensure};
use librespot_core::Session;
use protobuf::{CodedInputStream, CodedOutputStream};
use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
    time::{Duration, Instant},
};

const CONTENT_TYPE: &str = "application/vnd.collection-v2.spotify.proto";
const PAGE_LIMIT: i32 = 300;
/// Changes made on other devices appear after this long.
const SYNC_AFTER: Duration = Duration::from_secs(30);

#[derive(Debug, Default, PartialEq)]
struct Item {
    uri: String,
    added_at: i32,
    removed: bool,
}

#[derive(Debug, Default)]
struct Page {
    items: Vec<Item>,
    next: String,
    sync: String,
}

#[derive(Debug, Default)]
struct Delta {
    possible: bool,
    items: Vec<Item>,
    sync: String,
}

struct Mirror {
    /// Each saved item and when it was added, in Unix seconds.
    uris: HashMap<String, i32>,
    sync: String,
    checked: Instant,
}

static MIRRORS: LazyLock<tokio::sync::Mutex<HashMap<(String, &'static str), Mirror>>> =
    LazyLock::new(Default::default);

/// Whether the item can be saved or followed through the collection.
pub fn supports(uri: &str) -> bool {
    set_for(uri).is_some()
}

/// Collection set holding this kind of item.
fn set_for(uri: &str) -> Option<&'static str> {
    // Local songs are liked by their `spotify:local:` URI, as in Spotify.
    if uri.starts_with("spotify:local:") {
        return (uri.len() <= 1_000
            && matches!(
                librespot_core::SpotifyUri::from_uri(uri),
                Ok(librespot_core::SpotifyUri::Local { .. })
            ))
        .then_some("collection");
    }
    let (kind, id) = uri.strip_prefix("spotify:")?.split_once(':')?;
    if id.len() != 22 || !id.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return None;
    }
    match kind {
        "track" | "album" => Some("collection"),
        "artist" => Some("artist"),
        _ => None,
    }
}

/// Whether each item is saved (songs, albums) or followed (artists).
pub async fn contains(session: &Session, uris: &[String]) -> Result<Vec<bool>> {
    ensure!(uris.len() <= 500, "Too many library items");
    let sets = uris
        .iter()
        .map(|uri| set_for(uri).context("Choose a Spotify song, album or artist"))
        .collect::<Result<Vec<_>>>()?;
    let account = session.username();
    let mut mirrors = MIRRORS.lock().await;
    for set in sets.iter().copied().collect::<HashSet<_>>() {
        refresh(session, &mut mirrors, &account, set).await?;
    }
    Ok(uris
        .iter()
        .zip(sets)
        .map(|(uri, set)| {
            mirrors
                .get(&(account.clone(), set))
                .is_some_and(|mirror| mirror.uris.contains_key(uri))
        })
        .collect())
}

/// Save or remove one item, then confirm the change through a sync.
pub async fn write(session: &Session, uri: &str, saved: bool) -> Result<()> {
    let set = set_for(uri).context("Choose a Spotify song, album or artist")?;
    let account = session.username();
    let mut mirrors = MIRRORS.lock().await;
    refresh(session, &mut mirrors, &account, set).await?;
    let added_at = i32::try_from(time::OffsetDateTime::now_utc().unix_timestamp()).unwrap_or(0);
    let item = Item {
        uri: uri.into(),
        added_at,
        removed: !saved,
    };
    let mut nonce = [0; 16];
    getrandom::fill(&mut nonce).map_err(|_| anyhow::anyhow!("Cannot create library update ID"))?;
    let update: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let body = encode_write(&account, set, &[item], &update)?;
    request(session, "/collection/v2/write", body).await?;
    if let Some(mirror) = mirrors.get_mut(&(account.clone(), set)) {
        // Force the confirming sync below.
        mirror.checked = Instant::now() - SYNC_AFTER;
    }
    refresh(session, &mut mirrors, &account, set).await?;
    let stored = mirrors
        .get(&(account, set))
        .is_some_and(|mirror| mirror.uris.contains_key(uri));
    ensure!(
        stored == saved,
        "Spotify did not confirm the library change"
    );
    Ok(())
}

async fn refresh(
    session: &Session,
    mirrors: &mut HashMap<(String, &'static str), Mirror>,
    account: &str,
    set: &'static str,
) -> Result<()> {
    let key = (account.to_owned(), set);
    if let Some(mirror) = mirrors.get_mut(&key) {
        if mirror.checked.elapsed() < SYNC_AFTER {
            return Ok(());
        }
        let body = encode_delta_request(account, set, &mirror.sync)?;
        let delta = decode_delta(&request(session, "/collection/v2/delta", body).await?)?;
        if delta.possible {
            apply(&mut mirror.uris, delta.items);
            if !delta.sync.is_empty() {
                mirror.sync = delta.sync;
            }
            mirror.checked = Instant::now();
            return Ok(());
        }
    }
    let mut uris = HashMap::new();
    let mut token = String::new();
    let mut sync = String::new();
    let mut seen = HashSet::new();
    for _ in 0..1000 {
        let body = encode_page_request(account, set, &token, PAGE_LIMIT)?;
        let page = decode_page(&request(session, "/collection/v2/paging", body).await?)?;
        apply(&mut uris, page.items);
        if !page.sync.is_empty() {
            sync = page.sync;
        }
        if page.next.is_empty() {
            if mirrors.len() >= 16 {
                mirrors.clear();
            }
            mirrors.insert(
                key,
                Mirror {
                    uris,
                    sync,
                    checked: Instant::now(),
                },
            );
            return Ok(());
        }
        ensure!(
            seen.insert(page.next.clone()),
            "Spotify repeated a library page"
        );
        token = page.next;
    }
    anyhow::bail!("Spotify library has too many pages")
}

fn apply(uris: &mut HashMap<String, i32>, items: Vec<Item>) {
    for item in items {
        if item.removed {
            uris.remove(&item.uri);
        } else if uris.len() < 200_000 {
            uris.insert(item.uri, item.added_at);
        }
    }
}

/// Saved albums or followed artists with when each was added (Unix
/// seconds), most recently added first.
pub async fn listed(session: &Session, kind: &str) -> Result<Vec<(String, i32)>> {
    let (set, prefix) = match kind {
        "album" => ("collection", "spotify:album:"),
        "artist" => ("artist", "spotify:artist:"),
        _ => anyhow::bail!("Choose albums or artists"),
    };
    let account = session.username();
    let mut mirrors = MIRRORS.lock().await;
    refresh(session, &mut mirrors, &account, set).await?;
    Ok(newest_first(
        mirrors.get(&(account, set)).map(|mirror| &mirror.uris),
        prefix,
    ))
}

fn newest_first(uris: Option<&HashMap<String, i32>>, prefix: &str) -> Vec<(String, i32)> {
    let mut items: Vec<_> = uris
        .into_iter()
        .flatten()
        .filter(|(uri, _)| uri.starts_with(prefix))
        .collect();
    items.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    items
        .into_iter()
        .map(|(uri, added)| (uri.clone(), *added))
        .collect()
}

async fn request(session: &Session, endpoint: &str, body: Vec<u8>) -> Result<Vec<u8>> {
    use reqwest::header::{HeaderMap, HeaderValue};
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static(CONTENT_TYPE));
    headers.insert("accept", HeaderValue::from_static(CONTENT_TYPE));
    let bytes = tokio::time::timeout(
        Duration::from_secs(10),
        session
            .spclient()
            .request(&reqwest::Method::POST, endpoint, Some(headers), Some(&body)),
    )
    .await
    .context("Spotify library request timed out")??;
    ensure!(
        bytes.len() <= 16_000_000,
        "Spotify library page is too large"
    );
    Ok(bytes.to_vec())
}

fn encode(fields: impl FnOnce(&mut CodedOutputStream) -> protobuf::Result<()>) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    {
        let mut stream = CodedOutputStream::vec(&mut bytes);
        fields(&mut stream)?;
        stream.flush()?;
    }
    Ok(bytes)
}

fn encode_page_request(account: &str, set: &str, token: &str, limit: i32) -> Result<Vec<u8>> {
    encode(|stream| {
        stream.write_string(1, account)?;
        stream.write_string(2, set)?;
        if !token.is_empty() {
            stream.write_string(3, token)?;
        }
        stream.write_int32(4, limit)
    })
}

fn encode_delta_request(account: &str, set: &str, sync: &str) -> Result<Vec<u8>> {
    encode(|stream| {
        stream.write_string(1, account)?;
        stream.write_string(2, set)?;
        stream.write_string(3, sync)
    })
}

fn encode_write(account: &str, set: &str, items: &[Item], update: &str) -> Result<Vec<u8>> {
    let items = items
        .iter()
        .map(|item| {
            encode(|stream| {
                stream.write_string(1, &item.uri)?;
                stream.write_int32(2, item.added_at)?;
                if item.removed {
                    stream.write_bool(3, true)?;
                }
                Ok(())
            })
        })
        .collect::<Result<Vec<_>>>()?;
    encode(|stream| {
        stream.write_string(1, account)?;
        stream.write_string(2, set)?;
        for item in &items {
            stream.write_bytes(3, item)?;
        }
        stream.write_string(4, update)
    })
}

fn decode_item(bytes: &[u8]) -> Result<Item> {
    let mut stream = CodedInputStream::from_bytes(bytes);
    let mut item = Item::default();
    while let Some(tag) = stream.read_raw_tag_or_eof()? {
        match tag {
            10 => item.uri = stream.read_string()?,
            16 => item.added_at = stream.read_int32()?,
            24 => item.removed = stream.read_bool()?,
            _ => protobuf::rt::skip_field_for_tag(tag, &mut stream)?,
        }
    }
    Ok(item)
}

fn decode_page(bytes: &[u8]) -> Result<Page> {
    let mut stream = CodedInputStream::from_bytes(bytes);
    let mut page = Page::default();
    while let Some(tag) = stream.read_raw_tag_or_eof()? {
        match tag {
            10 => page.items.push(decode_item(&stream.read_bytes()?)?),
            18 => page.next = stream.read_string()?,
            26 => page.sync = stream.read_string()?,
            _ => protobuf::rt::skip_field_for_tag(tag, &mut stream)?,
        }
    }
    Ok(page)
}

fn decode_delta(bytes: &[u8]) -> Result<Delta> {
    let mut stream = CodedInputStream::from_bytes(bytes);
    let mut delta = Delta::default();
    while let Some(tag) = stream.read_raw_tag_or_eof()? {
        match tag {
            8 => delta.possible = stream.read_bool()?,
            18 => delta.items.push(decode_item(&stream.read_bytes()?)?),
            26 => delta.sync = stream.read_string()?,
            _ => protobuf::rt::skip_field_for_tag(tag, &mut stream)?,
        }
    }
    Ok(delta)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_songs_albums_and_artists_use_the_collection() {
        assert_eq!(
            set_for("spotify:track:0123456789abcdefghijkl"),
            Some("collection")
        );
        assert_eq!(
            set_for("spotify:album:0123456789abcdefghijkl"),
            Some("collection")
        );
        assert_eq!(
            set_for("spotify:artist:0123456789abcdefghijkl"),
            Some("artist")
        );
        assert_eq!(set_for("spotify:playlist:0123456789abcdefghijkl"), None);
        assert_eq!(set_for("spotify:track:short"), None);
        assert_eq!(set_for("spotify:local:a:b:c:1"), Some("collection"));
        assert_eq!(set_for("spotify:local:a:b"), None);
    }
    #[test]
    fn page_and_delta_decode_items_tokens_and_removals() {
        let item = |uri: &str, removed: bool| {
            encode(|stream| {
                stream.write_string(1, uri)?;
                stream.write_int32(2, 7)?;
                stream.write_bool(3, removed)
            })
            .unwrap()
        };
        let page = encode(|stream| {
            stream.write_bytes(1, &item("spotify:track:a", false))?;
            stream.write_bytes(1, &item("spotify:track:b", false))?;
            stream.write_string(2, "next")?;
            stream.write_string(3, "sync")?;
            stream.write_int32(9, 1)
        })
        .unwrap();
        let page = decode_page(&page).unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!((page.next.as_str(), page.sync.as_str()), ("next", "sync"));
        let delta = encode(|stream| {
            stream.write_bool(1, true)?;
            stream.write_bytes(2, &item("spotify:track:a", true))?;
            stream.write_bytes(2, &item("spotify:track:c", false))?;
            stream.write_string(3, "later")
        })
        .unwrap();
        let delta = decode_delta(&delta).unwrap();
        assert!(delta.possible);
        let mut uris: HashMap<String, i32> = page
            .items
            .into_iter()
            .map(|item| (item.uri, item.added_at))
            .collect();
        apply(&mut uris, delta.items);
        let mut uris: Vec<_> = uris.into_keys().collect();
        uris.sort();
        assert_eq!(uris, ["spotify:track:b", "spotify:track:c"]);
    }
    #[test]
    fn listed_items_are_newest_first_and_of_one_kind() {
        let uris = HashMap::from([
            ("spotify:album:old".to_owned(), 10),
            ("spotify:track:song".to_owned(), 30),
            ("spotify:album:new".to_owned(), 20),
        ]);
        assert_eq!(
            newest_first(Some(&uris), "spotify:album:"),
            [
                ("spotify:album:new".to_owned(), 20),
                ("spotify:album:old".to_owned(), 10)
            ]
        );
        assert!(newest_first(None, "spotify:album:").is_empty());
    }
    #[test]
    fn write_request_carries_account_set_item_and_update_id() {
        let body = encode_write(
            "me",
            "collection",
            &[Item {
                uri: "spotify:track:a".into(),
                added_at: 5,
                removed: true,
            }],
            "update",
        )
        .unwrap();
        let mut stream = CodedInputStream::from_bytes(&body);
        let mut fields = Vec::new();
        while let Some(tag) = stream.read_raw_tag_or_eof().unwrap() {
            match tag {
                10 | 18 | 34 => fields.push(stream.read_string().unwrap()),
                26 => {
                    let item = decode_item(&stream.read_bytes().unwrap()).unwrap();
                    assert_eq!(
                        item,
                        Item {
                            uri: "spotify:track:a".into(),
                            added_at: 5,
                            removed: true
                        }
                    );
                    fields.push("item".into());
                }
                _ => panic!("unexpected field {tag}"),
            }
        }
        assert_eq!(fields, ["me", "collection", "item", "update"]);
    }
    #[test]
    #[ignore = "Likes one unliked recent song through the saved sign-in, then unlikes it"]
    fn live_like_is_confirmed_and_undone() {
        crate::native::with_live_session(async |session: &Session| {
            let recent = crate::native_browse::recent(session)
                .await
                .expect("Native play history failed");
            let uris: Vec<_> = recent.iter().map(|track| track.uri.clone()).collect();
            let saved = contains(session, &uris)
                .await
                .expect("Native saved status failed");
            let like = uris
                .iter()
                .zip(&saved)
                .find(|(_, saved)| !**saved)
                .map(|(uri, _)| uri.clone())
                .expect("Expected a recent song that is not liked");
            let liked = write(session, &like, true).await;
            let after_like = contains(session, std::slice::from_ref(&like)).await;
            let unliked = write(session, &like, false).await;
            let after_unlike = contains(session, &[like]).await;
            liked.expect("Native like failed");
            unliked.expect("Native unlike failed");
            assert_eq!(after_like.unwrap(), [true]);
            assert_eq!(after_unlike.unwrap(), [false]);
            eprintln!("Native like and unlike confirmed");
        });
    }
    #[test]
    #[ignore = "Uses saved native sign-in for read-only library checks; never writes"]
    fn live_library_reads_without_writes() {
        crate::native::with_live_session(async |session: &Session| {
            let library = crate::native_browse::library(session)
                .await
                .expect("Native library failed");
            assert!(!library.is_empty(), "Expected saved playlists");
            let liked = crate::native_browse::read(session, "liked", 0)
                .await
                .expect("Native Liked Songs failed");
            assert!(
                liked.iter().all(|track| track.added_at.starts_with("20")),
                "Expected added dates on Liked Songs"
            );
            let listed = crate::native_browse::read(session, &library[0].id, 0)
                .await
                .expect("Native playlist failed");
            assert!(
                listed.iter().any(|track| track.added_at.starts_with("20")),
                "Expected added dates on a playlist"
            );
            let recent = crate::native_browse::recent(session)
                .await
                .expect("Native play history failed");
            assert!(!recent.is_empty(), "Expected recent plays");
            assert!(
                recent.iter().all(|track| track.added_at.starts_with("20")),
                "Expected play dates"
            );
            let saved = contains(session, &[recent[0].uri.clone()])
                .await
                .expect("Native saved status failed");
            let size = MIRRORS
                .lock()
                .await
                .get(&(session.username(), "collection"))
                .map_or(0, |mirror| mirror.uris.len());
            assert!(size > 0, "Expected saved songs");
            let folders: HashSet<&str> = library
                .iter()
                .flat_map(|item| item.folders.iter().map(|folder| folder.id.as_str()))
                .collect();
            let foldered = library
                .iter()
                .filter(|item| !item.folders.is_empty())
                .count();
            eprintln!(
                "Native reads: {} playlists ({foldered} in {} folders), {} recent plays, {size} saved songs and albums, latest saved {}",
                library.len(),
                folders.len(),
                recent.len(),
                saved[0]
            );
        });
    }
}
