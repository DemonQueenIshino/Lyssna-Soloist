//! Browser PKCE login and verified restore without a developer Web API app.
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use crate::{
    auth,
    model::{Artwork, Profile},
    native,
};
use anyhow::{Context, Result, ensure};
use librespot_core::{Session, authentication::Credentials};
use librespot_protocol::authentication::AuthenticationType;

pub(crate) struct Connected(pub Session);
impl Drop for Connected {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

/// A verified account and its live connection. Never serialized or sent to the UI.
pub struct Authenticated {
    pub(crate) profile: Profile,
    pub(crate) credentials: Credentials,
    pub(crate) connection: Connected,
    pub(crate) runtime: tokio::runtime::Runtime,
    /// A restored session unlocks with the saved profile; Spotify's current
    /// one arrives here afterwards.
    /// Held in a mutex so the login result stays shareable across threads.
    pub(crate) profile_refresh: Option<std::sync::Mutex<std::sync::mpsc::Receiver<Profile>>>,
}

impl Authenticated {
    #[cfg(test)]
    pub(crate) fn test(profile: Profile, credentials: Credentials) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let connection = {
            let _entered = runtime.enter();
            Connected(Session::new(
                librespot_core::config::SessionConfig::default(),
                None,
            ))
        };
        Self {
            profile,
            credentials,
            connection,
            runtime,
            profile_refresh: None,
        }
    }
}

fn previous_profile() -> Result<Option<Profile>> {
    if let Some(profile) = native::saved_profile()? {
        return Ok(Some(profile));
    }
    // Upgrade existing installations using only their stored account identity.
    let settings = crate::config::Settings::load()?;
    Ok(auth::Tokens::load(&settings.client_id)?.and_then(|tokens| tokens.profile))
}

pub fn authenticate(
    client: &reqwest::blocking::Client,
    restoring: bool,
    cancelled: &AtomicBool,
) -> Result<Option<Authenticated>> {
    ensure!(!cancelled.load(Ordering::Acquire), "Sign-in cancelled");
    let previous = if restoring { previous_profile()? } else { None };
    let credentials = if restoring {
        let Some(profile) = previous.as_ref() else {
            return Ok(None);
        };
        let Some(credentials) = native::load_credentials(&profile.id)? else {
            return Ok(None);
        };
        credentials
    } else {
        Credentials::with_access_token(auth::authorize_audio(client, cancelled)?)
    };
    let started = std::time::Instant::now();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let result = runtime.block_on(async {
        let connected = Connected(Session::new(native::session_config(), crate::audio_cache::cache().ok()));
        let session = &connected.0;
        let operation = async {
            tokio::time::timeout(Duration::from_secs(40), session.connect(credentials, false)).await
                .map_err(|_| anyhow::anyhow!("Spotify sign-in timed out. Check your connection and retry."))?
                .map_err(native::sign_in_error)?;
            let account = session.username();
            ensure!(!account.is_empty(), "Spotify did not authenticate an account");
            if let Some(previous) = previous.as_ref() { ensure!(account == previous.id, "Saved Spotify sign-in belongs to a different account"); }
            // Restoring, the saved profile unlocks the app as soon as Spotify
            // authenticates; its refresh follows instead of delaying startup.
            let profile = match previous.clone() {
                Some(previous) => previous,
                None => match fetch_profile(session, &account).await {
                    Some(result) => result?,
                    None => Profile { id: account.clone(), ..Default::default() },
                },
            };
            let reusable = Credentials { username: Some(account), auth_type: AuthenticationType::AUTHENTICATION_STORED_SPOTIFY_CREDENTIALS, auth_data: session.auth_data() };
            ensure!(!cancelled.load(Ordering::Acquire), "Sign-in cancelled");
            Ok((profile, reusable))
        };
        let result = tokio::select! {
            result = operation => result,
            () = async { while !cancelled.load(Ordering::Acquire) { tokio::time::sleep(Duration::from_millis(50)).await; } } => anyhow::bail!("Sign-in cancelled"),
        };
        let (profile, credentials) = result?;
        native::diagnostic(&format!(
            "sign-in {} {}ms",
            if restoring { "restore" } else { "fresh" },
            started.elapsed().as_millis()
        ));
        Ok((profile, credentials, connected))
    });
    result.map(|(profile, credentials, connection)| {
        let profile_refresh = previous.is_some().then(|| {
            let (send, receive) = std::sync::mpsc::channel();
            let session = connection.0.clone();
            let account = profile.id.clone();
            runtime.spawn(async move {
                if let Some(Ok(profile)) = fetch_profile(&session, &account).await {
                    let _ = send.send(profile);
                }
            });
            std::sync::Mutex::new(receive)
        });
        Some(Authenticated {
            profile,
            credentials,
            connection,
            runtime,
            profile_refresh,
        })
    })
}

/// Spotify's profile for `account`; `None` when it cannot be reached in time.
async fn fetch_profile(session: &Session, account: &str) -> Option<Result<Profile>> {
    let encoded: String = url::form_urlencoded::byte_serialize(account.as_bytes()).collect();
    let bytes = tokio::time::timeout(
        Duration::from_secs(8),
        session
            .spclient()
            .get_user_profile(&encoded, Some(0), Some(0)),
    )
    .await
    .ok()?
    .ok()?;
    Some(profile_from_response(account, &bytes))
}

pub(crate) fn profile_from_response(account: &str, bytes: &[u8]) -> Result<Profile> {
    ensure!(
        bytes.len() <= 1_000_000,
        "Spotify profile response too large"
    );
    let value: serde_json::Value =
        serde_json::from_slice(bytes).context("Cannot read Spotify profile")?;
    if let Some(uri) = value["uri"].as_str() {
        ensure!(
            uri == format!("spotify:user:{account}"),
            "Spotify profile belongs to a different account"
        );
    }
    let images = value["image_url"]
        .as_str()
        .and_then(|image| url::Url::parse(image).ok())
        .filter(|url| {
            url.scheme() == "https" && url.username().is_empty() && url.password().is_none()
        })
        .map(|url| {
            vec![Artwork {
                url: url.into(),
                ..Default::default()
            }]
        })
        .unwrap_or_default();
    Ok(Profile {
        id: account.into(),
        display_name: value["name"]
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned),
        images,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authenticated_identity_is_authoritative_and_profile_images_are_https() {
        let profile = profile_from_response("listener", br#"{"uri":"spotify:user:listener","name":" Listener ","image_url":"https://i.scdn.co/image/avatar"}"#).unwrap();
        assert_eq!(profile.id, "listener");
        assert_eq!(profile.display_name.as_deref(), Some("Listener"));
        assert_eq!(profile.images.len(), 1);
        assert!(
            profile_from_response("listener", br#"{"uri":"spotify:user:someone-else"}"#).is_err()
        );
        assert!(
            profile_from_response("listener", br#"{"image_url":"http://localhost/avatar"}"#)
                .unwrap()
                .images
                .is_empty()
        );
    }
    #[test]
    fn cancelled_login_cannot_open_browser_or_read_credentials() {
        assert!(
            authenticate(
                &reqwest::blocking::Client::new(),
                false,
                &AtomicBool::new(true)
            )
            .err()
            .unwrap()
            .to_string()
            .contains("cancelled")
        );
    }
    #[test]
    fn discarding_a_login_closes_its_session() {
        let login = Authenticated::test(
            Profile {
                id: "cancelled".into(),
                ..Default::default()
            },
            Credentials::with_access_token("test"),
        );
        let session = login.connection.0.clone();
        assert!(!session.is_invalid());
        drop(login);
        assert!(
            session.is_invalid(),
            "Rejected login must retire its live session"
        );
    }

    #[test]
    #[ignore = "Read-only native sign-in, library, likes, search and Home using saved credentials; no playback or writes"]
    fn live_native_login_and_browsing_without_developer_app() {
        let login = authenticate(&auth::http_client().unwrap(), true, &AtomicBool::new(false))
            .unwrap()
            .expect("Saved native login required");
        assert!(!login.profile.id.is_empty());
        login.runtime.block_on(async {
            let session = &login.connection.0;
            assert_eq!(session.username(), login.profile.id);
            let library = crate::native_browse::library_with_folders(session)
                .await
                .expect("Native library failed");
            assert!(
                !library.0.is_empty(),
                "Saved account has no playlists to validate"
            );
            let liked = crate::native_browse::read(session, "liked", 0)
                .await
                .expect("Native liked songs failed");
            let search = crate::search::catalogue(session, "Daft Punk")
                .await
                .expect("Native search failed");
            assert!(
                !search.tracks.is_empty() || !search.artists.is_empty(),
                "Native search returned no music"
            );
            crate::home::fetch(session, "Australia/Sydney")
                .await
                .expect("Native Home failed");
            if let Some(track) = liked.first().filter(|track| track.album.id.is_some()) {
                crate::catalogue::page(session, "album", track.album.id.as_deref().unwrap(), 0)
                    .await
                    .expect("Native album failed");
            }
        });
    }
}
