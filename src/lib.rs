pub mod album;
pub mod api;
pub mod api_backoff;
#[cfg(feature = "legacy-ui")]
pub mod app;
#[path = "ui/icon_pixels.rs"]
pub mod app_icon;
pub mod artist;
mod audio;
pub mod audio_cache;
mod audio_output;
pub mod auth;
#[cfg(feature = "desktop")]
pub mod autostart;
pub mod canvas;
pub mod catalogue;
pub mod collection;
pub mod config;
pub mod credential;
#[cfg(feature = "desktop")]
pub mod desktop;
pub mod dj;
pub mod home;
pub mod jam;
pub mod library_cache;
pub mod library_items;
pub mod links;
pub mod local_files;
pub mod lyric_sources;
pub mod lyrics;
#[cfg(target_os = "macos")]
pub mod mac_bundle;
#[cfg(target_os = "macos")]
mod macos_audio;
#[cfg(feature = "desktop")]
pub mod media;
pub mod model;
pub mod native;
pub mod native_browse;
mod native_login;
mod native_remote;
pub mod nightly;
pub mod notify;
mod playback_intent;
pub mod provider;
mod reconnect;
pub mod rename;
pub mod resume;
pub mod search;
pub mod spotify;
pub mod track_facts;
#[cfg(feature = "legacy-ui")]
pub mod ui;
pub mod updater;
pub mod user_profile;
pub mod whats_new;
pub mod worker;
