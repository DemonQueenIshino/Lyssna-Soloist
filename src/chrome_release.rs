//! The Chrome release most Windows users run right now, read from Google's
//! public release feed. spicylyrics.org answers only clients whose client
//! hints match real Chrome's, greased brand included, so its profiles and
//! profile lookups present themselves as this release.
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

const FEED: &str = "https://versionhistory.googleapis.com/v1/chrome/platforms/win/channels/stable/versions/all/releases?filter=endtime=none";
const REFRESH: Duration = Duration::from_secs(6 * 60 * 60);
const FALLBACK: (u32, &str) = (156, "156.0.8078.80");

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub major: u32,
    pub version: String,
}

static CURRENT: Mutex<Option<(Release, Instant)>> = Mutex::new(None);

/// The last release read, without waiting on the network.
pub fn cached() -> Release {
    CURRENT
        .lock()
        .ok()
        .and_then(|current| current.as_ref().map(|(release, _)| release.clone()))
        .unwrap_or_else(|| Release {
            major: FALLBACK.0,
            version: FALLBACK.1.into(),
        })
}

/// The current release, asking Google again once the last answer is old.
pub fn current() -> Release {
    let fresh = CURRENT.lock().ok().and_then(|current| {
        current
            .as_ref()
            .filter(|(_, at)| at.elapsed() < REFRESH)
            .map(|(release, _)| release.clone())
    });
    if let Some(release) = fresh {
        return release;
    }
    match fetch() {
        Ok(release) => {
            if let Ok(mut current) = CURRENT.lock() {
                *current = Some((release.clone(), Instant::now()));
            }
            release
        }
        Err(error) => {
            log::warn!("Chrome release lookup failed: {error:#}");
            cached()
        }
    }
}

fn fetch() -> Result<Release> {
    let body: Value = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?
        .get(FEED)
        .send()?
        .error_for_status()?
        .json()?;
    most_served(&body).context("Chrome release feed listed no serving release")
}

/// The major version serving the largest share of users, and its most
/// served build. A release still rolling out serves only a minority.
fn most_served(body: &Value) -> Option<Release> {
    let mut shares: HashMap<u32, (f64, f64, String)> = HashMap::new();
    for release in body["releases"].as_array()? {
        let version = release["version"].as_str()?;
        let major = version.split('.').next()?.parse::<u32>().ok()?;
        let fraction = release["fraction"].as_f64().unwrap_or(0.0);
        let entry = shares.entry(major).or_insert((0.0, -1.0, String::new()));
        entry.0 += fraction;
        if fraction > entry.1 {
            entry.1 = fraction;
            entry.2 = version.to_owned();
        }
    }
    shares
        .into_iter()
        .max_by(|a, b| a.1.0.total_cmp(&b.1.0))
        .map(|(major, (_, _, version))| Release { major, version })
}

/// Chrome's brand list as it sends it: Chromium shuffles a made-up "greased"
/// brand in by a rule seeded with the major version, and sites can check it.
pub fn brands(major: u32, version: &str) -> Vec<(String, String)> {
    const CHARS: [&str; 11] = [" ", "(", ":", "-", ".", "/", ")", ";", "=", "?", "_"];
    const VERSIONS: [&str; 3] = ["8", "99", "24"];
    const ORDERS: [[usize; 3]; 6] = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let seed = major as usize;
    let greased = (
        format!(
            "Not{}A{}Brand",
            CHARS[seed % CHARS.len()],
            CHARS[(seed + 1) % CHARS.len()]
        ),
        VERSIONS[seed % VERSIONS.len()].to_owned(),
    );
    let mut list = vec![(String::new(), String::new()); 3];
    let order = ORDERS[seed % ORDERS.len()];
    list[order[0]] = greased;
    list[order[1]] = ("Chromium".into(), version.into());
    list[order[2]] = ("Google Chrome".into(), version.into());
    list
}

/// A client whose requests read as this release's own fetches from a
/// spicylyrics.org page. Redirects are left for the caller to check.
pub fn site_client(timeout: Duration) -> Result<reqwest::blocking::Client> {
    use reqwest::header::{ACCEPT, HeaderMap, HeaderValue};
    let release = current();
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
    headers.insert("sec-ch-ua", HeaderValue::from_str(&release.client_hints())?);
    headers.insert("sec-ch-ua-mobile", HeaderValue::from_static("?0"));
    headers.insert(
        "sec-ch-ua-platform",
        HeaderValue::from_static("\"Windows\""),
    );
    headers.insert("sec-fetch-dest", HeaderValue::from_static("empty"));
    headers.insert("sec-fetch-mode", HeaderValue::from_static("cors"));
    headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
    Ok(reqwest::blocking::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(release.user_agent())
        .default_headers(headers)
        .build()?)
}

impl Release {
    pub fn user_agent(&self) -> String {
        format!(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36",
            self.major
        )
    }

    /// The `sec-ch-ua` request header.
    pub fn client_hints(&self) -> String {
        brands(self.major, &self.major.to_string())
            .iter()
            .map(|(brand, version)| format!("\"{brand}\";v=\"{version}\""))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// `Emulation.setUserAgentOverride` parameters for a WebView2 window.
    pub fn override_params(&self) -> String {
        let list = |version: &str| -> Value {
            brands(self.major, version)
                .into_iter()
                .map(|(brand, version)| json!({"brand": brand, "version": version}))
                .collect()
        };
        json!({
            "userAgent": self.user_agent(),
            "platform": "Win32",
            "userAgentMetadata": {
                "brands": list(&self.major.to_string()),
                "fullVersionList": list(&self.version),
                "fullVersion": self.version,
                "platform": "Windows",
                "platformVersion": "15.0.0",
                "architecture": "x86",
                "bitness": "64",
                "model": "",
                "mobile": false,
            },
        })
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brands_match_what_chrome_sends() {
        let release = Release {
            major: 156,
            version: "156.0.8078.80".into(),
        };
        assert_eq!(
            release.client_hints(),
            r#""Not:A-Brand";v="8", "Chromium";v="156", "Google Chrome";v="156""#
        );
        assert!(release.user_agent().contains("Chrome/156.0.0.0"));
        let params: Value = serde_json::from_str(&release.override_params()).unwrap();
        assert_eq!(
            params["userAgentMetadata"]["fullVersionList"][1]["version"],
            "156.0.8078.80"
        );
    }

    #[test]
    fn the_majority_release_wins_over_a_rollout() {
        let feed = json!({"releases": [
            {"version": "156.0.8078.80", "fraction": 0.495},
            {"version": "156.0.8078.81", "fraction": 0.2475},
            {"version": "157.0.8103.38", "fraction": 0.2475},
        ]});
        assert_eq!(
            most_served(&feed),
            Some(Release {
                major: 156,
                version: "156.0.8078.80".into()
            })
        );
        assert_eq!(most_served(&json!({"releases": []})), None);
    }
}
