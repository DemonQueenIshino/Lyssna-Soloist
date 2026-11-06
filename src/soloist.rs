//! Spotify Soloist, Spotify's own headless Connect player, plays for accounts
//! whose audio keys Spotify refuses to Lyssna's engine (librespot #1649).
//! Spotify asks that it is not redistributed, so Lyssna fetches the official
//! build from Spotify's CDN on the user's machine, checks that Spotify signed
//! it, and swaps in a fresh build before the 90-day expiry stops it.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub const DEVICE_NAME: &str = "Lyssna Player";
const DOWNLOAD_URL: &str =
    "https://soloist-builds.spotifycdn.com/soloist_release_windows_x86_64.zip";
#[cfg(not(target_os = "macos"))]
const KEY_TARGET: &str = "Lyssna/Soloist";
#[cfg(not(target_os = "macos"))]
const KEY_USER: &str = "api-key";
const SIGNER: &str = "Spotify AB";
const MAX_ARCHIVE: u64 = 128 * 1024 * 1024;
/// Soloist exits with this once its build is older than 90 days.
const EXPIRED: i32 = 10;
const UPDATE_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "message", rename_all = "snake_case")]
pub enum Status {
    Unsupported,
    NeedsKey,
    Installing,
    Starting,
    /// Running, but not signed in: pick the device once in a Spotify app.
    NeedsPairing,
    Ready,
    Failed(String),
}

#[cfg(not(target_os = "macos"))]
fn key_entry() -> Result<keyring::Entry> {
    Ok(keyring::Entry::new_with_target(
        KEY_TARGET, KEY_TARGET, KEY_USER,
    )?)
}

#[cfg(target_os = "macos")]
pub fn api_key() -> Result<Option<String>> {
    Ok(None)
}

#[cfg(not(target_os = "macos"))]
pub fn api_key() -> Result<Option<String>> {
    match key_entry()?.get_password() {
        Ok(key) if !key.trim().is_empty() => Ok(Some(key.trim().to_owned())),
        Ok(_) | Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => bail!("Cannot read the Soloist key from the secure credential store"),
    }
}

pub fn valid_key(key: &str) -> bool {
    (16..=256).contains(&key.len())
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

pub fn set_api_key(key: &str) -> Result<()> {
    let key = key.trim();
    ensure!(valid_key(key), "That doesn't look like a Soloist API key");
    #[cfg(target_os = "macos")]
    bail!("Spotify Soloist is set up on Windows only");
    #[cfg(not(target_os = "macos"))]
    key_entry()?
        .set_password(key)
        .context("Cannot save the Soloist key to the secure credential store")
}

fn root() -> Result<PathBuf> {
    Ok(crate::config::data_dir()?.join("soloist"))
}

pub fn data_dir() -> Result<PathBuf> {
    Ok(root()?.join("data"))
}

#[derive(Default, Serialize, Deserialize)]
struct Release {
    etag: String,
}

fn release_path(bin: &Path) -> PathBuf {
    bin.join("release.json")
}

fn read_release(bin: &Path) -> Release {
    fs::read(release_path(bin))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(concat!("Lyssna/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .build()?)
}

/// Downloads the current build when it differs from the installed one and
/// stages it as `soloist.next.exe`. True when a new build is staged.
fn fetch(bin: &Path) -> Result<bool> {
    let client = client()?;
    let head = client
        .head(DOWNLOAD_URL)
        .send()
        .context("Cannot reach Spotify's Soloist downloads")?;
    ensure!(
        head.status().is_success(),
        "Soloist download returned HTTP {}",
        head.status().as_u16()
    );
    let etag = head
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let installed = bin.join("soloist.exe").is_file();
    if installed && !etag.is_empty() && read_release(bin).etag == etag {
        return Ok(false);
    }
    let response = client.get(DOWNLOAD_URL).send()?;
    ensure!(
        response.status().is_success(),
        "Soloist download returned HTTP {}",
        response.status().as_u16()
    );
    let mut archive = Vec::new();
    response.take(MAX_ARCHIVE + 1).read_to_end(&mut archive)?;
    ensure!(
        archive.len() as u64 <= MAX_ARCHIVE,
        "Soloist download is too large"
    );
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))?;
    let mut entry = zip
        .by_name("soloist.exe")
        .context("Soloist download has no soloist.exe")?;
    fs::create_dir_all(bin)?;
    let partial = bin.join("soloist.download");
    let mut file = fs::File::create(&partial)?;
    std::io::copy(&mut entry, &mut file)?;
    file.sync_all()?;
    drop(file);
    let staged = bin.join("soloist.next.exe");
    let result = verify_signature(&partial).and_then(|()| {
        let output = Command::new(&partial)
            .arg("--version")
            .creation_flags_hidden()
            .output()?;
        ensure!(
            output.status.success() && output.stdout.starts_with(b"soloist "),
            "Downloaded Soloist did not start"
        );
        Ok(())
    });
    if let Err(error) = result {
        let _ = fs::remove_file(&partial);
        return Err(error);
    }
    fs::rename(&partial, &staged)?;
    fs::write(
        bin.join("release.next.json"),
        serde_json::to_vec(&Release { etag })?,
    )?;
    Ok(true)
}

/// Moves a staged build into place; only while Soloist is not running.
fn swap_in(bin: &Path) -> Result<()> {
    let staged = bin.join("soloist.next.exe");
    if !staged.is_file() {
        return Ok(());
    }
    fs::rename(&staged, bin.join("soloist.exe"))?;
    let _ = fs::rename(bin.join("release.next.json"), release_path(bin));
    Ok(())
}

trait HiddenCommand {
    fn creation_flags_hidden(&mut self) -> &mut Self;
}
impl HiddenCommand for Command {
    fn creation_flags_hidden(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}

#[cfg(windows)]
fn verify_signature(path: &Path) -> Result<()> {
    signature::signer(path).and_then(|signer| {
        ensure!(
            signer == SIGNER,
            "Soloist download is signed by {signer}, not {SIGNER}"
        );
        Ok(())
    })
}

#[cfg(not(windows))]
fn verify_signature(_: &Path) -> Result<()> {
    bail!("Soloist is only set up on Windows")
}

#[cfg(windows)]
mod signature {
    use anyhow::{Result, bail};
    use std::{os::windows::ffi::OsStrExt, path::Path};
    use windows::{
        Win32::{
            Foundation::{HANDLE, HWND},
            Security::{
                Cryptography::{CERT_NAME_SIMPLE_DISPLAY_TYPE, CertGetNameStringW},
                WinTrust::{
                    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0,
                    WINTRUST_FILE_INFO, WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE,
                    WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvSignerFromChain,
                    WTHelperProvDataFromStateData, WinVerifyTrust,
                },
            },
        },
        core::PCWSTR,
    };

    /// The display name of the certificate that validly signed `path`.
    pub fn signer(path: &Path) -> Result<String> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut file = WINTRUST_FILE_INFO {
            cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: PCWSTR(wide.as_ptr()),
            ..Default::default()
        };
        let mut data = WINTRUST_DATA {
            cbStruct: size_of::<WINTRUST_DATA>() as u32,
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: WTD_REVOKE_NONE,
            dwUnionChoice: WTD_CHOICE_FILE,
            Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
            dwStateAction: WTD_STATEACTION_VERIFY,
            ..Default::default()
        };
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        let status = unsafe {
            WinVerifyTrust(
                HWND(std::ptr::null_mut()),
                &mut action,
                (&raw mut data).cast(),
            )
        };
        let name = (status == 0).then(|| unsafe { signer_name(data.hWVTStateData) });
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        unsafe {
            WinVerifyTrust(
                HWND(std::ptr::null_mut()),
                &mut action,
                (&raw mut data).cast(),
            );
        }
        match name {
            Some(Some(name)) => Ok(name),
            Some(None) => bail!("Cannot read who signed the Soloist download"),
            None => bail!("Soloist download is not validly signed (0x{status:08x})"),
        }
    }

    unsafe fn signer_name(state: HANDLE) -> Option<String> {
        unsafe {
            let provider = WTHelperProvDataFromStateData(state);
            if provider.is_null() {
                return None;
            }
            let signer = WTHelperGetProvSignerFromChain(provider, 0, false, 0);
            if signer.is_null() || (*signer).csCertChain == 0 {
                return None;
            }
            let certificate = (*(*signer).pasCertChain).pCert;
            let mut name = [0u16; 256];
            let length = CertGetNameStringW(
                certificate,
                CERT_NAME_SIMPLE_DISPLAY_TYPE,
                0,
                None,
                Some(&mut name),
            );
            (length > 1).then(|| String::from_utf16_lossy(&name[..length as usize - 1]))
        }
    }
}

#[derive(Deserialize)]
struct AuthState {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    logged_in: bool,
}

/// Whether the running Soloist is signed in, asked over its local WebSocket.
fn signed_in(data: &Path) -> Result<bool> {
    let port = fs::read_to_string(data.join("ws.port"))?;
    let port: u16 = port.trim().parse()?;
    let stream = std::net::TcpStream::connect_timeout(
        &([127, 0, 0, 1], port).into(),
        Duration::from_secs(2),
    )?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    let (mut socket, _) = tungstenite::client(format!("ws://127.0.0.1:{port}"), stream)?;
    for _ in 0..8 {
        if let tungstenite::Message::Text(text) = socket.read()?
            && let Ok(state) = serde_json::from_str::<AuthState>(&text)
            && state.kind == "auth_state"
        {
            let _ = socket.close(None);
            return Ok(state.logged_in);
        }
    }
    bail!("Soloist sent no sign-in state")
}

/// Runs Soloist while Lyssna runs; reports its state and keeps its build fresh.
pub struct Supervisor {
    status: Arc<Mutex<Status>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Supervisor {
    pub fn start() -> Self {
        let status = Arc::new(Mutex::new(Status::Starting));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = (cfg!(windows)).then(|| {
            let status = status.clone();
            let stop = stop.clone();
            thread::Builder::new()
                .name("soloist".into())
                .spawn(move || supervise(&status, &stop))
                .expect("spawn Soloist supervisor")
        });
        if thread.is_none() {
            *status.lock().unwrap() = Status::Unsupported;
        }
        Self {
            status,
            stop,
            thread,
        }
    }

    pub fn status(&self) -> Status {
        self.status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_else(|_| Status::Failed("Soloist state unavailable".into()))
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn set(status: &Mutex<Status>, next: Status) {
    if let Ok(mut current) = status.lock()
        && *current != next
    {
        log::info!("Soloist: {next:?}");
        *current = next;
    }
}

fn wait(stop: &AtomicBool, duration: Duration) -> bool {
    let until = Instant::now() + duration;
    while Instant::now() < until {
        if stop.load(Ordering::Acquire) {
            return false;
        }
        thread::sleep(Duration::from_millis(200));
    }
    !stop.load(Ordering::Acquire)
}

fn spawn(exe: &Path, data: &Path, cache: &Path, key: &str) -> Result<Child> {
    let _ = fs::remove_file(data.join("ws.port"));
    let child = Command::new(exe)
        .args([
            "--device-name",
            DEVICE_NAME,
            "--api-key",
            key,
            "--ws",
            "127.0.0.1:0",
        ])
        .arg("--data-dir")
        .arg(data)
        .arg("--cache-dir")
        .arg(cache)
        .args(["--cache-size", "500"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags_hidden()
        .spawn()
        .context("Cannot start Soloist")?;
    Ok(child)
}

const NORMALIZE: &str = "audio.normalize_v2";
const SEEDED_MARKER: &str = ".soloist-prefs-seeded";

/// Updates `existing` prefs with volume normalization turned off, preserving every other key.
pub fn merge_prefs(existing: &str) -> String {
    let wanted = [(NORMALIZE, "false")];
    let mut lines: Vec<String> = existing
        .split('\n')
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.is_empty())
        .filter(|line| {
            let key = line.split('=').next().unwrap_or_default();
            !wanted.iter().any(|(wanted, _)| *wanted == key)
        })
        .map(str::to_string)
        .collect();
    lines.extend(wanted.iter().map(|(key, value)| format!("{key}={value}")));
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn users_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("settings").join("Users")
}

pub fn user_dirs(data_dir: &Path) -> BTreeSet<String> {
    let Ok(entries) = fs::read_dir(users_dir(data_dir)) else {
        return BTreeSet::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("prefs").is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect()
}

pub fn write_normalization(data_dir: &Path) -> std::io::Result<usize> {
    let mut written = 0;
    for user in user_dirs(data_dir) {
        let path = users_dir(data_dir).join(user).join("prefs");
        let existing = fs::read_to_string(&path)?;
        let merged = merge_prefs(&existing);
        if merged != existing {
            fs::write(&path, merged.as_bytes())?;
            written += 1;
        }
    }
    Ok(written)
}

pub fn seeded(data_dir: &Path) -> BTreeSet<String> {
    fs::read_to_string(data_dir.join(SEEDED_MARKER))
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub fn mark_seeded(data_dir: &Path, users: &BTreeSet<String>) -> std::io::Result<()> {
    let text: String = users.iter().map(|user| format!("{user}\n")).collect();
    fs::write(data_dir.join(SEEDED_MARKER), text)
}

fn supervise(status: &Mutex<Status>, stop: &AtomicBool) {
    let Ok(root) = root() else {
        set(
            status,
            Status::Failed("Cannot find Lyssna's data folder".into()),
        );
        return;
    };
    let (bin, data, cache) = (root.join("bin"), root.join("data"), root.join("cache"));
    let job = job::Job::new().ok();
    let mut checked: Option<Instant> = None;
    let mut failures = 0u32;
    while !stop.load(Ordering::Acquire) {
        let key = match api_key() {
            Ok(Some(key)) => key,
            Ok(None) => {
                set(status, Status::NeedsKey);
                if !wait(stop, Duration::from_secs(2)) {
                    return;
                }
                continue;
            }
            Err(error) => {
                set(status, Status::Failed(error.to_string()));
                if !wait(stop, Duration::from_secs(10)) {
                    return;
                }
                continue;
            }
        };
        let exe = bin.join("soloist.exe");
        if !exe.is_file() || checked.is_none_or(|at| at.elapsed() >= UPDATE_EVERY) {
            if !exe.is_file() {
                set(status, Status::Installing);
            }
            match fetch(&bin) {
                Ok(_) => checked = Some(Instant::now()),
                Err(error) if !exe.is_file() => {
                    set(
                        status,
                        Status::Failed(format!("Cannot install Soloist: {error:#}")),
                    );
                    if !wait(stop, Duration::from_secs(60)) {
                        return;
                    }
                    continue;
                }
                Err(error) => log::warn!("Soloist update check failed: {error:#}"),
            }
            if let Err(error) = swap_in(&bin) {
                log::warn!("Cannot install the new Soloist build: {error:#}");
            }
        }
        let _ = fs::create_dir_all(&data);
        match write_normalization(&data) {
            Ok(count) if count > 0 => {
                log::info!("Soloist: turned off volume normalization in {count} account prefs")
            }
            Ok(_) => {}
            Err(error) => log::warn!("Soloist: could not write normalization prefs: {error}"),
        }
        let users_before = user_dirs(&data);
        let _ = mark_seeded(&data, &users_before);

        set(status, Status::Starting);
        let mut child = match spawn(&exe, &data, &cache, &key) {
            Ok(child) => child,
            Err(error) => {
                set(status, Status::Failed(format!("{error:#}")));
                if !wait(stop, Duration::from_secs(30)) {
                    return;
                }
                continue;
            }
        };
        if let Some(job) = &job {
            job.assign(&child);
        }
        let started = Instant::now();
        let mut seed_restart = false;
        let code = loop {
            if stop.load(Ordering::Acquire) {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            if let Ok(Some(exit)) = child.try_wait() {
                break exit.code();
            }
            match signed_in(&data) {
                Ok(true) => {
                    failures = 0;
                    set(status, Status::Ready);
                    let present = user_dirs(&data);
                    let seeded_users = seeded(&data);
                    if present.difference(&seeded_users).next().is_some() {
                        let _ = write_normalization(&data);
                        let _ = mark_seeded(&data, &present);
                        log::info!(
                            "Soloist: new login detected; restarting Soloist once to disable normalization"
                        );
                        seed_restart = true;
                        let _ = child.kill();
                        let _ = child.wait();
                        break None;
                    }
                }
                Ok(false) => set(status, Status::NeedsPairing),
                Err(_) => {}
            }
            // A newer build waits in staging; Soloist swaps it in next start.
            if checked.is_some_and(|at| at.elapsed() >= UPDATE_EVERY) {
                checked = Some(Instant::now());
                if let Err(error) = fetch(&bin) {
                    log::warn!("Soloist update check failed: {error:#}");
                }
            }
            if !wait(stop, Duration::from_secs(3)) {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        };
        if seed_restart {
            continue;
        }
        if code == Some(EXPIRED) {
            log::info!("Soloist build expired; fetching the current one");
            checked = None;
            let _ = fs::remove_file(release_path(&bin));
            continue;
        }
        failures += 1;
        let message = format!(
            "Soloist stopped (exit {})",
            code.map_or("unknown".into(), |code| code.to_string())
        );
        set(status, Status::Failed(message));
        let backoff = if started.elapsed() > Duration::from_secs(60) {
            Duration::from_secs(2)
        } else {
            Duration::from_secs(5 * u64::from(failures.min(12)))
        };
        if !wait(stop, backoff) {
            return;
        }
    }
}

#[cfg(windows)]
mod job {
    use anyhow::Result;
    use std::os::windows::io::AsRawHandle;
    use windows::{
        Win32::{
            Foundation::HANDLE,
            System::JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
        },
        core::PCWSTR,
    };

    /// Closes Soloist with Lyssna, even when Lyssna is killed.
    pub struct Job(HANDLE);

    impl Job {
        pub fn new() -> Result<Self> {
            let job = unsafe { CreateJobObjectW(None, PCWSTR::null())? };
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )?;
            }
            Ok(Self(job))
        }

        pub fn assign(&self, child: &std::process::Child) {
            let process = HANDLE(child.as_raw_handle());
            if let Err(error) = unsafe { AssignProcessToJobObject(self.0, process) } {
                log::warn!("Soloist runs outside its job: {error}");
            }
        }
    }
}

#[cfg(not(windows))]
mod job {
    pub struct Job;
    impl Job {
        pub fn new() -> anyhow::Result<Self> {
            Ok(Self)
        }
        pub fn assign(&self, _: &std::process::Child) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_plain_tokens() {
        assert!(valid_key("abcdefghijklmnop-1234_5678.90"));
        assert!(!valid_key("short"));
        assert!(!valid_key("has spaces in the key value"));
    }

    #[test]
    fn staged_builds_replace_the_installed_one() {
        let dir = std::env::temp_dir().join(format!("lyssna-soloist-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("soloist.exe"), b"old").unwrap();
        fs::write(dir.join("soloist.next.exe"), b"new").unwrap();
        fs::write(dir.join("release.next.json"), br#"{"etag":"\"b\""}"#).unwrap();
        swap_in(&dir).unwrap();
        assert_eq!(fs::read(dir.join("soloist.exe")).unwrap(), b"new");
        assert!(!dir.join("soloist.next.exe").exists());
        assert_eq!(read_release(&dir).etag, "\"b\"");
        swap_in(&dir).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn merge_prefs_sets_normalization_off() {
        let sample = "audio.play_bitrate_non_metered_migrated=true\naudio.crossfade_migrated_to_ps=true\n";
        let merged = merge_prefs(sample);
        assert!(merged.contains("audio.normalize_v2=false\n"));
        assert!(merged.contains("audio.crossfade_migrated_to_ps=true\n"));
        assert_eq!(
            merged.matches("audio.normalize_v2=false").count(),
            1
        );
    }

    #[test]
    fn merge_prefs_replaces_existing_normalization_flag() {
        let sample = "audio.normalize_v2=true\naudio.gapless=true\n";
        let merged = merge_prefs(sample);
        assert!(!merged.contains("audio.normalize_v2=true"));
        assert!(merged.contains("audio.normalize_v2=false\n"));
        assert!(merged.contains("audio.gapless=true\n"));
    }

    #[test]
    fn merge_prefs_is_idempotent() {
        let sample = "audio.play_bitrate_non_metered_migrated=true\n";
        let once = merge_prefs(sample);
        assert_eq!(merge_prefs(&once), once);
    }

    #[test]
    fn merge_prefs_handles_crlf() {
        let sample = "a=1\r\nb=2\r\n";
        let merged = merge_prefs(sample);
        assert!(!merged.contains('\r'));
        assert!(merged.contains("audio.normalize_v2=false\n"));
    }

    #[test]
    fn write_normalization_updates_account_prefs() {
        let dir = tempfile::tempdir().unwrap();
        let user_prefs = dir.path().join("settings").join("Users").join("test_user").join("prefs");
        fs::create_dir_all(user_prefs.parent().unwrap()).unwrap();
        fs::write(&user_prefs, "audio.normalize_v2=true\n").unwrap();

        assert_eq!(write_normalization(dir.path()).unwrap(), 1);
        assert_eq!(write_normalization(dir.path()).unwrap(), 0);

        let content = fs::read_to_string(&user_prefs).unwrap();
        assert!(content.contains("audio.normalize_v2=false\n"));

        let users = user_dirs(dir.path());
        assert_eq!(users, ["test_user".to_string()].into());
        mark_seeded(dir.path(), &users).unwrap();
        assert_eq!(seeded(dir.path()), users);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "needs a Spotify-signed soloist.exe in LYSSNA_SOLOIST_EXE"]
    fn spotify_signed_builds_pass() {
        let path = std::env::var("LYSSNA_SOLOIST_EXE").unwrap();
        verify_signature(Path::new(&path)).unwrap();
        assert!(verify_signature(&std::env::current_exe().unwrap()).is_err());
    }
}
