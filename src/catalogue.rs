//! Read-only desktop catalogue queries. Native credentials stay inside Rust.
use anyhow::{Context, Result, ensure};
use librespot_core::Session;
use serde_json::{Value, json};
use std::time::Duration;
pub fn base_info(entity: &Value, kind: &str, id: &str) -> Result<crate::model::BrowseInfo> {
    use crate::model::{Album, Artist, BrowseInfo};
    fn images(value: &Value) -> Vec<crate::model::Artwork> {
        serde_json::from_value(value.clone()).unwrap_or_default()
    }
    fn artist(value: &Value) -> Artist {
        let uri = value["uri"].as_str().unwrap_or_default();
        Artist {
            id: uri.strip_prefix("spotify:artist:").map(str::to_owned),
            uri: uri.to_owned(),
            name: value["profile"]["name"]
                .as_str()
                .or_else(|| value["name"].as_str())
                .unwrap_or_default()
                .to_owned(),
            images: images(&value["visuals"]["avatarImage"]["sources"]),
            ..Artist::default()
        }
    }
    let mut info = BrowseInfo {
        key: format!("{kind}:{id}"),
        ..BrowseInfo::default()
    };
    if kind == "artist" {
        let artist = artist(entity);
        ensure!(!artist.name.is_empty(), "Artist name unavailable");
        info.title = artist.name.clone();
        info.images = artist.images.clone();
        info.artist = Some(artist);
    } else if kind == "album" {
        let name = entity["name"].as_str().context("Album name unavailable")?;
        let artists = entity["artists"]["items"]
            .as_array()
            .map(|items| items.iter().map(artist).collect::<Vec<_>>())
            .unwrap_or_default();
        let album = Album {
            id: Some(id.to_owned()),
            uri: format!("spotify:album:{id}"),
            name: name.to_owned(),
            images: images(&entity["coverArt"]["sources"]),
            artists,
            release_date: entity["date"]["isoString"]
                .as_str()
                .unwrap_or_default()
                .chars()
                .take(10)
                .collect(),
            album_type: entity["type"].as_str().unwrap_or_default().to_owned(),
            total_tracks: entity["tracksV2"]["totalCount"]
                .as_u64()
                .unwrap_or_default(),
            ..Album::default()
        };
        info.title = album.name.clone();
        info.images = album.images.clone();
        info.subtitle = album
            .artists
            .iter()
            .map(|artist| artist.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        info.album = Some(album);
    } else {
        anyhow::bail!("Unsupported catalogue entity");
    }
    Ok(info)
}

pub async fn entity(session: &Session, kind: &str, id: &str) -> Result<Value> {
    page(session, kind, id, 0).await
}
pub async fn page(session: &Session, kind: &str, id: &str, offset: usize) -> Result<Value> {
    ensure!(
        id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()),
        "Invalid catalogue ID"
    );
    // Persisted query identifiers verified against Spotify's desktop web client.
    let (operation, hash, field) = match kind {
        "artist" => (
            "queryArtistOverview",
            "9f8134ef565e78621f1e1793555bd6633c5ac144ae0f89604ed3ae3f80b3c8e6",
            "artistUnion",
        ),
        "album" => (
            "getAlbum",
            "0b6ce3511e1b02e4a40fa95440f992acbe85080eb308af9194322d708fc1914f",
            "albumUnion",
        ),
        _ => anyhow::bail!("Unsupported catalogue query"),
    };
    let mut data=query(session,operation,hash,json!({"uri":format!("spotify:{kind}:{id}"),"locale":"","preReleaseV2":false,"offset":offset,"limit":50})).await?;
    let entity = data["data"][field].take();
    ensure!(
        entity["uri"].as_str() == Some(format!("spotify:{kind}:{id}").as_str()),
        "Desktop catalogue data unavailable"
    );
    entity.as_object().context("Invalid catalogue data")?;
    Ok(entity)
}
/// One page of an artist's whole discography, newest first, as Spotify's
/// Discography page lists it.
pub async fn discography(session: &Session, id: &str, offset: usize) -> Result<Value> {
    ensure!(
        id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()) && offset <= 5000,
        "Invalid artist ID"
    );
    // Persisted query identifier from Spotify's desktop Discography page.
    let mut data = query(
        session,
        "queryArtistDiscographyAll",
        "5e07d323febb57b4a56a42abbf781490e58764aa45feb6e3dc0591564fc56599",
        json!({"uri":format!("spotify:artist:{id}"),"offset":offset,"limit":50,"order":"DATE_DESC"}),
    )
    .await?;
    Ok(data["data"]["artistUnion"]["discography"]["all"].take())
}
/// Writers from Spotify's track credits, for songs whose metadata names no
/// composer.
pub async fn songwriters(session: &Session, id: &str) -> Result<Vec<String>> {
    ensure!(
        id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()),
        "Invalid track ID"
    );
    // Persisted query identifier from Spotify's desktop credits dialog.
    let data = query(
        session,
        "queryTrackCreditsGroupedModal",
        "f135fb9be58a72d041ab5d214d817021a272405d883860468e2627afb01a3ca9",
        json!({"trackUri":format!("spotify:track:{id}"),"contributorsLimit":100,"contributorsOffset":0}),
    )
    .await?;
    Ok(credited_writers(&data))
}
/// A song's full credits, as Spotify's credits dialog shows them.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct TrackCredits {
    pub name: String,
    pub groups: Vec<CreditGroup>,
    pub sources: Vec<String>,
}
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct CreditGroup {
    pub name: String,
    pub credits: Vec<Credit>,
}
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct Credit {
    pub name: String,
    pub roles: Vec<String>,
    /// The person's artist page, when they have one.
    pub uri: Option<String>,
}
pub async fn credits(session: &Session, id: &str) -> Result<TrackCredits> {
    ensure!(
        id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()),
        "Invalid track ID"
    );
    let data = query(
        session,
        "queryTrackCreditsGroupedModal",
        "f135fb9be58a72d041ab5d214d817021a272405d883860468e2627afb01a3ca9",
        json!({"trackUri":format!("spotify:track:{id}"),"contributorsLimit":100,"contributorsOffset":0}),
    )
    .await?;
    Ok(grouped_credits(&data))
}
/// Groups contributors by their role group in Spotify's order, each person
/// once per group with all their roles in it. As in Spotify, only the
/// track's own artists, in the first group, link and can be followed.
fn grouped_credits(data: &Value) -> TrackCredits {
    let track = &data["data"]["trackUnion"];
    let text = |value: &Value| value.as_str().map(str::trim).unwrap_or_default().to_owned();
    let main: Option<std::collections::HashSet<&str>> =
        track["artists"]["items"].as_array().map(|items| {
            items
                .iter()
                .filter_map(|item| item["uri"].as_str())
                .collect()
        });
    let mut groups: Vec<CreditGroup> = Vec::new();
    for item in track["creditsTrait"]["contributors"]["items"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let name = text(&item["name"]);
        if name.is_empty() {
            continue;
        }
        let group_name = Some(text(&item["roleGroup"]["name"]))
            .filter(|group| !group.is_empty())
            .unwrap_or_else(|| "Other".into());
        let role = text(&item["role"]);
        let first = groups.first().is_none_or(|group| group.name == group_name);
        let uri = item["uri"].as_str().filter(|uri| {
            first
                && main.as_ref().is_none_or(|main| main.contains(uri))
                && uri.strip_prefix("spotify:artist:").is_some_and(|id| {
                    id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
                })
        });
        let group = match groups.iter().position(|group| group.name == group_name) {
            Some(index) => &mut groups[index],
            None => {
                groups.push(CreditGroup {
                    name: group_name,
                    credits: Vec::new(),
                });
                groups.last_mut().unwrap()
            }
        };
        match group.credits.iter_mut().find(|credit| credit.name == name) {
            Some(credit) => {
                if !role.is_empty() && !credit.roles.contains(&role) {
                    credit.roles.push(role);
                }
                if credit.uri.is_none() {
                    credit.uri = uri.map(str::to_owned);
                }
            }
            None => group.credits.push(Credit {
                name,
                roles: Some(role)
                    .filter(|role| !role.is_empty())
                    .into_iter()
                    .collect(),
                uri: uri.map(str::to_owned),
            }),
        }
    }
    TrackCredits {
        name: text(&track["name"]),
        groups,
        sources: track["creditsTrait"]["sources"]["items"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|source| text(&source["name"]))
            .filter(|name| !name.is_empty())
            .collect(),
    }
}
fn credited_writers(data: &Value) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for item in data["data"]["trackUnion"]["creditsTrait"]["contributors"]["items"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let role = item["role"].as_str().unwrap_or_default().to_lowercase();
        let writer = ["writer", "composer", "lyricist"]
            .iter()
            .any(|kind| role.contains(kind));
        if let Some(name) = item["name"].as_str().map(str::trim)
            && writer
            && !name.is_empty()
            && !names.iter().any(|known| known == name)
        {
            names.push(name.to_owned());
        }
    }
    names
}
const HEDGE_AFTER: Duration = Duration::from_secs(2);

/// Some networks stall a fresh TLS handshake to Spotify for ~12 s. A second
/// attempt on a new connection after `after` usually lands at once, so take
/// whichever answers first. Only for idempotent requests.
async fn hedged<F, Fut, T, E>(attempt: F, after: Duration) -> std::result::Result<T, E>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, E>>,
{
    let first = attempt();
    tokio::pin!(first);
    tokio::select! {
        result = &mut first => return result,
        () = tokio::time::sleep(after) => {}
    }
    let second = attempt();
    tokio::pin!(second);
    tokio::select! {
        result = &mut first => match result {
            Ok(value) => Ok(value),
            Err(_) => second.await,
        },
        result = &mut second => match result {
            Ok(value) => Ok(value),
            Err(_) => first.await,
        },
    }
}

pub(crate) async fn query(
    session: &Session,
    operation: &str,
    hash: &str,
    variables: Value,
) -> Result<Value> {
    let token = session.login5().auth_token().await?;
    let client_token = session.spclient().client_token().await?;
    let body = json!({"operationName":operation,"variables":variables,"extensions":{"persistedQuery":{"version":1,"sha256Hash":hash}}});
    let attempt = || async {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(8))
            .build()?
            .post("https://api-partner.spotify.com/pathfinder/v2/query")
            .bearer_auth(&token.access_token)
            .header("client-token", &client_token)
            .header("app-platform", "WebPlayer")
            .header("spotify-app-version", "896000000")
            .json(&body)
            .send()
            .await
    };
    let mut response = hedged(attempt, HEDGE_AFTER).await?.error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 2_000_000,
            "Catalogue response too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}
#[cfg(test)]
mod base_tests {
    use super::*;
    #[tokio::test]
    async fn a_stalled_request_is_overtaken_by_a_fresh_attempt() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let tries = AtomicUsize::new(0);
        let started = std::time::Instant::now();
        let result: std::result::Result<usize, ()> = hedged(
            || {
                let n = tries.fetch_add(1, Ordering::SeqCst);
                async move {
                    if n == 0 {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                    Ok(n)
                }
            },
            Duration::from_millis(50),
        )
        .await;
        assert_eq!(result, Ok(1));
        assert!(started.elapsed() < Duration::from_secs(1));

        let tries = AtomicUsize::new(0);
        let fast: std::result::Result<usize, ()> = hedged(
            || {
                tries.fetch_add(1, Ordering::SeqCst);
                async { Ok(7) }
            },
            Duration::from_millis(50),
        )
        .await;
        assert_eq!(fast, Ok(7));
        assert_eq!(tries.load(Ordering::SeqCst), 1);

        let failed_first: std::result::Result<u8, u8> = hedged(
            {
                let tries = AtomicUsize::new(0);
                move || {
                    let n = tries.fetch_add(1, Ordering::SeqCst);
                    async move {
                        if n == 0 {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            Err(1)
                        } else {
                            tokio::time::sleep(Duration::from_millis(300)).await;
                            Ok(2)
                        }
                    }
                }
            },
            Duration::from_millis(50),
        )
        .await;
        assert_eq!(failed_first, Ok(2));
    }
    #[test]
    fn credits_keep_each_writer_once_and_skip_other_roles() {
        let data = json!({"data":{"trackUnion":{"creditsTrait":{"contributors":{"items":[
            {"name":"Kevin Parker","role":"Writer","roleGroup":{"name":"Composition & Lyrics"}},
            {"name":"Kevin Parker","role":"Producer","roleGroup":{"name":"Production & Engineering"}},
            {"name":"Sarah Aarons","role":"Composer"},
            {"name":"Kevin Parker","role":"Lyricist"},
            {"name":"Main Artist","role":"Main Artist"}
        ]}}}}});
        assert_eq!(
            credited_writers(&data),
            vec!["Kevin Parker", "Sarah Aarons"]
        );
        assert!(credited_writers(&json!({"data":{"trackUnion":null}})).is_empty());
    }
    #[test]
    fn credits_group_like_spotifys_dialog() {
        let artist = "spotify:artist:0123456789abcdefghijkl";
        let writer = "spotify:artist:abcdefghijkl0123456789";
        let data = json!({"data":{"trackUnion":{"name":"it boy",
            "artists":{"items":[{"uri":artist}]},
            "creditsTrait":{
            "contributors":{"items":[
                {"name":"bbno$","role":"Main Artist","roleGroup":{"name":"Artist"},"uri":artist},
                {"name":"Vi","role":"Composer","roleGroup":{"name":"Composition & Lyrics"},"uri":writer},
                {"name":"Kyle","role":"Lyricist","roleGroup":{"name":"Composition & Lyrics"}},
                {"name":"Kyle","role":"Composer","roleGroup":{"name":"Composition & Lyrics"}},
                {"name":"Pink Slip","role":"Producer","roleGroup":{"name":"Production & Engineering"},"uri":"spotify:artist:bad"},
                {"name":"Kyle","role":"Musician","roleGroup":{"name":"Performers"}},
                {"name":"Someone","role":"Mixer"}
            ]},
            "sources":{"items":[{"name":"bbno$$"}]}
        }}}});
        let credits = grouped_credits(&data);
        assert_eq!(credits.name, "it boy");
        let names: Vec<_> = credits
            .groups
            .iter()
            .map(|group| group.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "Artist",
                "Composition & Lyrics",
                "Production & Engineering",
                "Performers",
                "Other"
            ]
        );
        assert_eq!(credits.groups[0].credits[0].uri.as_deref(), Some(artist));
        // Spotify offers Follow only for the track's own artists.
        assert_eq!(credits.groups[1].credits[0].name, "Vi");
        assert_eq!(credits.groups[1].credits[0].uri, None);
        assert_eq!(credits.groups[1].credits[1].roles, ["Lyricist", "Composer"]);
        assert_eq!(credits.groups[2].credits[0].uri, None);
        assert_eq!(credits.sources, ["bbno$$"]);
    }
    #[test]
    fn native_album_supplies_header_without_public_api() {
        let value = json!({"name":"Album title","coverArt":{"sources":[{"url":"https://artwork.example.test/cover","width":640,"height":640}]},"artists":{"items":[{"uri":"spotify:artist:artist","profile":{"name":"Album artist"}}]},"tracksV2":{"totalCount":13},"date":{"isoString":"2026-05-04T00:00:00Z"}});
        let info = base_info(&value, "album", "album").unwrap();
        assert_eq!(info.title, "Album title");
        assert_eq!(info.images.len(), 1);
        let album = info.album.unwrap();
        assert_eq!(album.artists[0].name, "Album artist");
        assert_eq!(album.total_tracks, 13);
        assert_eq!(album.release_date, "2026-05-04");
        assert!(info.album_details.is_none());
    }
}
