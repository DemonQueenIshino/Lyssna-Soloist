//! Nightly builds: Lyssna built on this computer from the latest source.
//!
//! Tools already on the computer are used as they are: rustup (a missing
//! Rust version is added to it), a recent enough Node.js, CMake and
//! libclang. Anything missing is kept in Lyssna's own folder, pinned and
//! checked against published SHA-256 sums: a private rustup with its own
//! homes, portable Node.js, libclang and CMake. Nothing is added to PATH
//! and no administrator rights are needed, except for the system's C++
//! tools: Visual Studio's build tools, which Windows asks about once, or
//! Apple's Command Line Tools, which macOS offers in its own window.
//! The source comes as GitHub's archive of one commit, so Git isn't needed.
//!
//! Tools run in a cleared environment, on Windows inside a job object at
//! below-normal priority and elsewhere in a process group of their own, so
//! the user's settings can't break the build, music keeps playing
//! smoothly, and Cancel stops every process. Network steps retry, finished
//! steps are kept for the next run, and the running copy is replaced only
//! after the new build has started successfully.

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const REPOSITORY: &str = "Stormanzanii/Lyssna";
const NODE_RELEASES: &str = "https://nodejs.org/dist/latest-v24.x";
const LIBCLANG_VERSION: &str = "18.1.1";
const CMAKE_VERSION: &str = "3.31.6";
const RUSTUP_DIST: &str = "https://static.rust-lang.org/rustup/dist";
const EXE: &str = std::env::consts::EXE_SUFFIX;
/// What Rust builds for, and the ends of this system's download names.
#[cfg(windows)]
mod system {
    pub const HOST: &str = "x86_64-pc-windows-msvc";
    pub const NODE: &str = "-win-x64.zip";
    pub const CMAKE: &str = "-windows-x86_64.zip";
    pub const LIBCLANG_WHEEL: &str = "win_amd64.whl";
    pub const LIBCLANG: &str = "libclang.dll";
    pub const CPP_TOOLS: &str = "C++ build tools";
}
#[cfg(target_os = "macos")]
mod system {
    #[cfg(target_arch = "aarch64")]
    pub const HOST: &str = "aarch64-apple-darwin";
    #[cfg(not(target_arch = "aarch64"))]
    pub const HOST: &str = "x86_64-apple-darwin";
    #[cfg(target_arch = "aarch64")]
    pub const NODE: &str = "-darwin-arm64.tar.gz";
    #[cfg(not(target_arch = "aarch64"))]
    pub const NODE: &str = "-darwin-x64.tar.gz";
    pub const CMAKE: &str = "-macos-universal.tar.gz";
    #[cfg(target_arch = "aarch64")]
    pub const LIBCLANG_WHEEL: &str = "arm64.whl";
    #[cfg(not(target_arch = "aarch64"))]
    pub const LIBCLANG_WHEEL: &str = "x86_64.whl";
    pub const LIBCLANG: &str = "libclang.dylib";
    pub const CPP_TOOLS: &str = "Command Line Tools";
}
#[cfg(not(any(windows, target_os = "macos")))]
mod system {
    pub const HOST: &str = "x86_64-unknown-linux-gnu";
    pub const NODE: &str = "-linux-x64.tar.gz";
    pub const CMAKE: &str = "-linux-x86_64.tar.gz";
    pub const LIBCLANG_WHEEL: &str = "manylinux2010_x86_64.whl";
    pub const LIBCLANG: &str = "libclang.so";
    pub const CPP_TOOLS: &str = "C++ build tools";
}
#[cfg(windows)]
const VS_BOOTSTRAPPER: &str = "https://aka.ms/vs/17/release/vs_BuildTools.exe";
#[cfg(windows)]
const VS_ARGUMENTS: &str = "--wait --passive --norestart --nocache \
    --add Microsoft.VisualStudio.Workload.VCTools \
    --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 \
    --add Microsoft.VisualStudio.Component.VC.CMake.Project --includeRecommended";
const GB: u64 = 1_000_000_000;
/// Tools, source and the build's intermediate files.
const BUILD_SPACE: u64 = 10 * GB;
/// Visual Studio's C++ tools, on the system drive.
#[cfg(windows)]
const VS_SPACE: u64 = 8 * GB;
/// Variables the build tools may see; everything else is left out.
const KEPT_VARIABLES: &[&str] = &[
    "ALLUSERSPROFILE",
    "APPDATA",
    "COMPUTERNAME",
    "ComSpec",
    "CommonProgramFiles",
    "CommonProgramFiles(x86)",
    "CommonProgramW6432",
    "HOMEDRIVE",
    "HOMEPATH",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "LOCALAPPDATA",
    "NO_PROXY",
    "NUMBER_OF_PROCESSORS",
    "OS",
    "PATHEXT",
    "PROCESSOR_ARCHITECTURE",
    "PROCESSOR_IDENTIFIER",
    "PROCESSOR_LEVEL",
    "PROCESSOR_REVISION",
    "PUBLIC",
    "ProgramData",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "SystemDrive",
    "SystemRoot",
    "TEMP",
    "TMP",
    "USERDOMAIN",
    "USERNAME",
    "USERPROFILE",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "windir",
    // macOS
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TMPDIR",
    "LANG",
    "DEVELOPER_DIR",
];

/// Reads a tool's output lines into a step's progress.
type Progress = Box<dyn FnMut(&str) -> Option<f64>>;

/// The commit a Nightly copy was built from.
pub const COMMIT: Option<&str> = match option_env!("LYSSNA_NIGHTLY") {
    Some(commit) => Some(commit),
    // Set by Nightly copies from before the rename that build this source.
    None => option_env!("SPOTIFLEX_NIGHTLY"),
};

static RUNNING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
/// The latest report, for Settings opened while a build runs.
static LAST: Mutex<Option<Report>> = Mutex::new(None);
/// The processes of the tool now running, so Cancel can stop them all.
static JOB: Mutex<Option<Arc<Job>>> = Mutex::new(None);

#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Pending,
    Running,
    Done,
    Skipped,
    Failed,
}

#[derive(Clone, Serialize)]
pub struct Step {
    pub id: &'static str,
    pub label: &'static str,
    pub state: State,
    /// 0 to 1 when known; `None` shows a moving bar.
    pub progress: Option<f64>,
    /// The tool's latest line of output.
    pub detail: String,
}

/// What the interface is told after every change.
#[derive(Clone, Serialize)]
pub struct Report {
    pub steps: Vec<Step>,
    pub running: bool,
    pub error: Option<String>,
    /// Shown when the build ended without a new version to install.
    pub note: Option<String>,
    pub finished: bool,
    /// The full output of this run.
    pub log: Option<String>,
}

pub enum Outcome {
    /// Installed; Lyssna restarts into it.
    Installed,
    /// Built in the background and waiting to be installed.
    Staged(String),
    /// This copy already is the latest Nightly.
    Current,
}

/// Whether a build replaces this copy at once, as when switching to
/// Nightly, or waits until the user chooses to restart.
#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Install,
    Stage,
}

/// What looking for newer Nightly code found, for the update status.
pub enum Status {
    Current,
    Available(String),
    Building,
    Ready(String),
    Message(String),
}

/// How often a Nightly copy looks for newer code.
const CHECK_EVERY: Duration = Duration::from_secs(10 * 60);

type SettingsHook = Box<dyn Fn() -> (bool, bool) + Send + Sync>;
type StatusHook = Box<dyn Fn(Status) + Send + Sync>;
type ReportHook = Arc<dyn Fn(&Report) + Send + Sync>;

/// How the watcher reads the settings and tells the interface.
struct Hooks {
    /// Whether this copy follows Nightly, and builds updates by itself.
    settings: SettingsHook,
    status: StatusHook,
    report: ReportHook,
}

/// Where Nightly can build itself: 64-bit Windows and macOS.
const SUPPORTED: bool = cfg!(any(
    all(windows, target_arch = "x86_64"),
    target_os = "macos"
));

static HOOKS: std::sync::OnceLock<Hooks> = std::sync::OnceLock::new();
/// A finished background build: its commit and executable.
static READY: Mutex<Option<(String, PathBuf)>> = Mutex::new(None);

/// The latest report, if a build ran since Lyssna started.
pub fn last() -> Option<Report> {
    LAST.lock().ok()?.clone()
}

/// Build the latest Nightly on a background thread, reporting each change.
/// `done` is called when the new build is installed and should restart.
pub fn start(
    mode: Mode,
    report: impl Fn(&Report) + Send + 'static,
    done: impl FnOnce(Result<Outcome, String>) + Send + 'static,
) -> Result<()> {
    ensure!(
        SUPPORTED,
        "Nightly builds are made on 64-bit Windows and macOS for now."
    );
    ensure!(
        !RUNNING.swap(true, Ordering::SeqCst),
        "A Nightly build is already running."
    );
    CANCEL.store(false, Ordering::SeqCst);
    thread::spawn(move || {
        let mut run = Run::new(report);
        run.mode = mode;
        let result = run.build();
        if let Err(error) = &result {
            let message = if CANCEL.load(Ordering::SeqCst) {
                "Nightly build cancelled.".to_owned()
            } else {
                format!("{error:#}")
            };
            run.log(&format!("Stopped: {message}"));
            log::warn!("Nightly build stopped: {message}");
            for step in &mut run.steps {
                if step.state == State::Running {
                    step.state = State::Failed;
                }
            }
            run.error = Some(message);
        }
        run.running = false;
        run.finished = result.is_ok();
        run.send(true);
        RUNNING.store(false, Ordering::SeqCst);
        done(result.map_err(|_| run.error.clone().unwrap_or_default()));
    });
    Ok(())
}

/// Look for newer Nightly code every ten minutes, starting shortly after
/// launch, and build it in the background when that is on.
pub fn watch(
    settings: impl Fn() -> (bool, bool) + Send + Sync + 'static,
    status: impl Fn(Status) + Send + Sync + 'static,
    report: impl Fn(&Report) + Send + Sync + 'static,
) {
    let hooks = Hooks {
        settings: Box::new(settings),
        status: Box::new(status),
        report: Arc::new(report),
    };
    if HOOKS.set(hooks).is_err() {
        return;
    }
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(20));
        loop {
            if let Err(error) = check(false) {
                log::warn!("Cannot check for Nightly updates: {error:#}");
            }
            thread::sleep(CHECK_EVERY);
        }
    });
}

/// Look for newer Nightly code now, building it when `build` or when
/// updates build automatically. What was found also goes to the update
/// status, so the update prompt opens for a new version.
pub fn check(build: bool) -> Result<Status> {
    let hooks = HOOKS.get().context("Nightly updates aren't ready yet")?;
    let (nightly, automatic) = (hooks.settings)();
    let status = |status: Status| (hooks.status)(status);
    if !nightly {
        return Ok(Status::Message("Not following Nightly".into()));
    }
    let Some(current) = COMMIT else {
        let message = "Use Build latest in Settings to start Nightly";
        status(Status::Message(message.into()));
        return Ok(Status::Message(message.into()));
    };
    if RUNNING.load(Ordering::SeqCst) {
        return Ok(Status::Building);
    }
    let client = client()?;
    let latest = latest_commit(&client)?;
    if latest == current {
        status(Status::Current);
        return Ok(Status::Current);
    }
    remember_changes(&client, current, &latest);
    if ready().as_deref() == Some(latest.as_str()) {
        status(Status::Ready(latest.clone()));
        return Ok(Status::Ready(latest));
    }
    status(Status::Available(latest.clone()));
    if !(build || automatic) {
        return Ok(Status::Available(latest));
    }
    let report = hooks.report.clone();
    start(
        Mode::Stage,
        move |progress| report(progress),
        |result| {
            let Some(hooks) = HOOKS.get() else { return };
            (hooks.status)(match result {
                Ok(Outcome::Staged(commit)) => Status::Ready(commit),
                Ok(_) => Status::Current,
                Err(error) => Status::Message(format!(
                    "Nightly update failed: {}",
                    error.lines().next().unwrap_or("unknown error")
                )),
            });
        },
    )?;
    status(Status::Building);
    Ok(Status::Building)
}

/// The commit of a Nightly built in the background, waiting to install.
pub fn ready() -> Option<String> {
    READY
        .lock()
        .ok()?
        .as_ref()
        .map(|(commit, _)| commit.clone())
}

/// Replace this copy with the Nightly built in the background; the
/// caller restarts Lyssna.
pub fn install_ready() -> Result<()> {
    let (_, staged) = READY
        .lock()
        .ok()
        .and_then(|mut ready| ready.take())
        .context("No Nightly update is ready")?;
    ensure!(staged.is_file(), "The Nightly update is gone; check again");
    replace_running(&staged)
}

/// Stop the running build and every process it started.
pub fn cancel() {
    CANCEL.store(true, Ordering::SeqCst);
    if let Ok(job) = JOB.lock()
        && let Some(job) = job.as_ref()
    {
        job.terminate();
    }
}

/// Delete Nightly's build output, downloaded source and downloads, keeping
/// its tools so the next build need not install Rust again. Returns the
/// bytes freed. Refused while a build runs, and no build starts until it
/// is done.
pub fn clear_cache() -> Result<u64> {
    ensure!(
        !RUNNING.swap(true, Ordering::SeqCst),
        "Wait for the Nightly build to finish first."
    );
    let result = root().map(|root| {
        ["target", "source", "downloads"]
            .into_iter()
            .map(|name| {
                let dir = root.join(name);
                let size = folder_size(&dir);
                // What could not go, such as a file held open, still counts
                // as not freed.
                let _ = fs::remove_dir_all(&dir);
                size.saturating_sub(folder_size(&dir))
            })
            .sum()
    });
    RUNNING.store(false, Ordering::SeqCst);
    result
}

fn folder_size(dir: &Path) -> u64 {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => folder_size(&entry.path()),
            Ok(kind) if kind.is_file() => entry.metadata().map_or(0, |data| data.len()),
            _ => 0,
        })
        .sum()
}

fn cancelled() -> Result<()> {
    ensure!(!CANCEL.load(Ordering::SeqCst), "Cancelled");
    Ok(())
}

/// Where Nightly keeps its tools, source and build.
fn root() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "Lyssna")
        .context("Cannot locate Lyssna's local data folder")?;
    Ok(dirs.data_local_dir().join("nightly"))
}

/// Tools that were found or installed, for the build's environment.
#[derive(Default)]
struct Tools {
    cmake: Option<PathBuf>,
    libclang: Option<PathBuf>,
    node: Option<Node>,
    rust_home: PathBuf,
    cargo_home: PathBuf,
    /// The folder with `cargo`, and `rustup` when Rust came with it.
    rust_bin: PathBuf,
    toolchain: Option<String>,
    target: PathBuf,
    commit: Option<String>,
}

/// Node.js and the npm it came with.
#[derive(Clone)]
struct Node {
    program: PathBuf,
    npm: PathBuf,
}

impl Node {
    /// Node.js as its download unpacks.
    fn unpacked(dir: &Path) -> Self {
        if cfg!(windows) {
            Self {
                program: dir.join("node.exe"),
                npm: dir.join("node_modules/npm/bin/npm-cli.js"),
            }
        } else {
            Self {
                program: dir.join("bin/node"),
                npm: dir.join("lib/node_modules/npm/bin/npm-cli.js"),
            }
        }
    }
}

struct Run<F: Fn(&Report)> {
    steps: Vec<Step>,
    running: bool,
    error: Option<String>,
    note: Option<String>,
    finished: bool,
    report: F,
    log_path: Option<PathBuf>,
    log_file: Option<fs::File>,
    last_sent: Instant,
    tools: Tools,
    mode: Mode,
    /// Build and check without installing, for the end-to-end test.
    dry_run: bool,
}

impl<F: Fn(&Report)> Run<F> {
    fn new(report: F) -> Self {
        let steps = [
            ("cpp", system::CPP_TOOLS),
            ("cmake", "CMake"),
            ("libclang", "libclang"),
            ("node", "Node.js"),
            ("source", "Latest source code"),
            ("rust", "Rust"),
            ("packages", "Interface packages"),
            ("interface", "Building the interface"),
            ("crates", "Rust packages"),
            ("app", "Building Lyssna"),
            ("check", "Checking the build"),
            ("install", "Installing"),
        ]
        .into_iter()
        .map(|(id, label)| Step {
            id,
            label,
            state: State::Pending,
            progress: None,
            detail: String::new(),
        })
        .collect();
        Self {
            steps,
            running: true,
            error: None,
            note: None,
            finished: false,
            report,
            log_path: None,
            log_file: None,
            last_sent: Instant::now(),
            tools: Tools::default(),
            mode: Mode::Install,
            dry_run: false,
        }
    }

    /// Tell the interface, at most ten times a second unless `now`.
    fn send(&mut self, now: bool) {
        if !now && self.last_sent.elapsed() < Duration::from_millis(100) {
            return;
        }
        self.last_sent = Instant::now();
        let report = Report {
            steps: self.steps.clone(),
            running: self.running,
            error: self.error.clone(),
            note: self.note.clone(),
            finished: self.finished,
            log: self
                .log_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        };
        if let Ok(mut last) = LAST.lock() {
            *last = Some(report.clone());
        }
        (self.report)(&report);
    }

    fn log(&mut self, line: &str) {
        if let Some(file) = self.log_file.as_mut() {
            let _ = writeln!(file, "{line}");
        }
    }

    fn step(&mut self, id: &str) -> &mut Step {
        let index = self
            .steps
            .iter()
            .position(|step| step.id == id)
            .expect("known step");
        &mut self.steps[index]
    }

    fn set(&mut self, id: &str, state: State) {
        let step = self.step(id);
        step.state = state;
        step.progress = (state == State::Done).then_some(1.0);
        if state != State::Running {
            step.detail.clear();
        }
        let label = step.label;
        self.log(&format!("== {label}: {}", state_name(state)));
        self.send(true);
    }

    fn progress(&mut self, id: &str, progress: Option<f64>, detail: impl Into<String>) {
        let step = self.step(id);
        if let Some(progress) = progress {
            step.progress = Some(progress.clamp(0.0, 1.0));
        }
        step.detail = detail.into().chars().take(160).collect();
        self.send(false);
    }

    fn open_log(&mut self) -> Result<()> {
        let dir = root()?.join("logs");
        fs::create_dir_all(&dir)?;
        let mut logs: Vec<PathBuf> = fs::read_dir(&dir)?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "log"))
            .collect();
        logs.sort();
        // Keep the last few runs.
        while logs.len() >= 5 {
            let _ = fs::remove_file(logs.remove(0));
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |time| time.as_secs());
        let path = dir.join(format!("nightly-{stamp}.log"));
        self.log_file = Some(fs::File::create(&path)?);
        self.log_path = Some(path);
        Ok(())
    }

    fn build(&mut self) -> Result<Outcome> {
        let root = root()?;
        fs::create_dir_all(&root)?;
        self.open_log()?;
        self.send(true);
        self.log(&format!(
            "Lyssna {} building Nightly in {}",
            env!("CARGO_PKG_VERSION"),
            root.display()
        ));
        let tools_dir = root.join("tools");
        let downloads = root.join("downloads");
        fs::create_dir_all(&tools_dir)?;
        fs::create_dir_all(&downloads)?;
        let own_rust = installed_rustup();
        (
            self.tools.rust_home,
            self.tools.cargo_home,
            self.tools.rust_bin,
        ) = own_rust.clone().unwrap_or_else(|| {
            let cargo_home = tools_dir.join("cargo");
            (
                tools_dir.join("rustup"),
                cargo_home.clone(),
                cargo_home.join("bin"),
            )
        });
        if let Some((rust_home, ..)) = &own_rust {
            self.log(&format!("Using your rustup in {}", rust_home.display()));
        }
        self.tools.target = cargo_target(&root);
        let client = client()?;

        // Room for everything first, rather than failing half way.
        check_space(&root, BUILD_SPACE)?;
        let studio = self.cpp_tools(&client, &downloads)?;

        // CMake: Visual Studio's or one already installed, else a private one.
        if let Some(cmake) = studio
            .as_deref()
            .and_then(bundled_cmake)
            .or_else(installed_cmake)
        {
            self.log(&format!("Using CMake at {}", cmake.display()));
            self.tools.cmake = Some(cmake);
            self.set("cmake", State::Skipped);
        } else {
            let dir = tools_dir.join(format!("cmake-{CMAKE_VERSION}"));
            let cmake = if cfg!(target_os = "macos") {
                dir.join("CMake.app/Contents/bin/cmake")
            } else {
                dir.join("bin").join(format!("cmake{EXE}"))
            };
            if cmake.is_file() {
                self.set("cmake", State::Skipped);
            } else {
                self.set("cmake", State::Running);
                let base =
                    format!("https://github.com/Kitware/CMake/releases/download/v{CMAKE_VERSION}");
                let sums = get_text(
                    &client,
                    &format!("{base}/cmake-{CMAKE_VERSION}-SHA-256.txt"),
                )?;
                let (name, hash) = checksum_for(&sums, system::CMAKE)
                    .context("CMake's checksums don't list a download for this system")?;
                let archive = downloads.join(&name);
                self.download(
                    &client,
                    "cmake",
                    &format!("{base}/{name}"),
                    &archive,
                    Some(&hash),
                )?;
                self.extract("cmake", &archive, &dir, true)?;
                self.set("cmake", State::Done);
            }
            self.tools.cmake = Some(cmake);
        }

        // libclang, which whisper's bindings are generated with: one already
        // installed, else from PyPI's wheel. Where the wheel keeps it differs
        // between releases, so it is looked for.
        let libclang = tools_dir.join(format!("libclang-{LIBCLANG_VERSION}"));
        let native = match installed_libclang(studio.as_deref())
            .or_else(|| find_file(&libclang, system::LIBCLANG, 6))
        {
            Some(found) => {
                self.log(&format!("Using libclang at {}", found.display()));
                self.set("libclang", State::Skipped);
                found
            }
            None => {
                self.set("libclang", State::Running);
                let release: serde_json::Value = serde_json::from_str(&get_text(
                    &client,
                    &format!("https://pypi.org/pypi/libclang/{LIBCLANG_VERSION}/json"),
                )?)?;
                let (url, hash) =
                    libclang_wheel(&release).context("libclang has no download for this system")?;
                let archive = downloads.join("libclang.whl");
                self.download(&client, "libclang", &url, &archive, Some(&hash))?;
                self.extract("libclang", &archive, &libclang, false)?;
                let found = find_file(&libclang, system::LIBCLANG, 6)
                    .with_context(|| format!("libclang's download has no {}", system::LIBCLANG))?;
                self.set("libclang", State::Done);
                found
            }
        };
        self.tools.libclang = native.parent().map(Path::to_path_buf);

        // Node.js: one already installed when it is recent enough for the
        // interface's tools, else the newest Node.js 24.
        if let Some(node) = installed_node() {
            self.log(&format!("Using Node.js at {}", node.program.display()));
            self.tools.node = Some(node);
            self.set("node", State::Skipped);
        } else {
            self.set("node", State::Running);
            let node = match get_text(&client, &format!("{NODE_RELEASES}/SHASUMS256.txt"))
                .ok()
                .and_then(|sums| checksum_for(&sums, system::NODE))
            {
                Some((name, hash)) => {
                    let dir =
                        tools_dir.join(name.trim_end_matches(".zip").trim_end_matches(".tar.gz"));
                    if !Node::unpacked(&dir).program.is_file() {
                        let archive = downloads.join(&name);
                        self.download(
                            &client,
                            "node",
                            &format!("{NODE_RELEASES}/{name}"),
                            &archive,
                            Some(&hash),
                        )?;
                        self.extract("node", &archive, &dir, true)?;
                    }
                    dir
                }
                // Offline: a Node.js 24 kept from an earlier build still works.
                None => newest_dir(&tools_dir, "node-v24.")
                    .filter(|dir| Node::unpacked(dir).program.is_file())
                    .context("Cannot reach nodejs.org to download Node.js")?,
            };
            self.tools.node = Some(Node::unpacked(&node));
            self.set("node", State::Done);
        }
        self.build_source(&root, &client, &downloads)
    }

    /// Visual Studio's C++ tools: the one install that needs Windows'
    /// permission, through Microsoft's own installer.
    #[cfg(windows)]
    fn cpp_tools(
        &mut self,
        client: &reqwest::blocking::Client,
        downloads: &Path,
    ) -> Result<Option<PathBuf>> {
        let found = visual_studio();
        if found.is_none() {
            let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
            check_space(Path::new(&format!("{system}\\")), VS_SPACE)?;
        }
        Ok(Some(match found {
            Some(found) => {
                self.set("cpp", State::Skipped);
                found
            }
            // Never ask for Windows' permission unprompted.
            None if self.mode == Mode::Stage => bail!(
                "Visual Studio's C++ tools are missing. Use Build latest in Settings to set them up again."
            ),
            None => {
                self.set("cpp", State::Running);
                let installer = downloads.join("vs_BuildTools.exe");
                self.download(client, "cpp", VS_BOOTSTRAPPER, &installer, None)?;
                self.progress(
                    "cpp",
                    None,
                    "Waiting for Windows' permission to install Visual Studio's C++ tools",
                );
                let started = Instant::now();
                let code = run_elevated(&installer, VS_ARGUMENTS, &mut || {
                    let seconds = started.elapsed().as_secs();
                    self.progress(
                        "cpp",
                        None,
                        format!(
                            "Visual Studio Installer is adding the C++ tools; its window shows the progress ({}:{:02})",
                            seconds / 60,
                            seconds % 60
                        ),
                    );
                })?;
                let _ = fs::remove_file(&installer);
                match code {
                    0 | 3010 => {}
                    1618 | 8006 => {
                        bail!("Another installation is running. Let it finish, then try again.")
                    }
                    5007 => bail!("Visual Studio's installer needs administrator rights."),
                    1602 | 1223 => bail!("Visual Studio's installation was cancelled."),
                    code => bail!("Visual Studio's installer stopped with code {code}."),
                }
                let found = visual_studio()
                    .context("Visual Studio's C++ tools were installed but cannot be found")?;
                self.set("cpp", State::Done);
                found
            }
        }))
    }

    /// Apple's Command Line Tools, which macOS installs from a window of
    /// its own when asked.
    #[cfg(not(windows))]
    fn cpp_tools(&mut self, _: &reqwest::blocking::Client, _: &Path) -> Result<Option<PathBuf>> {
        if command_line_tools() {
            self.set("cpp", State::Skipped);
            return Ok(None);
        }
        if self.mode == Mode::Stage {
            bail!(
                "Apple's Command Line Tools are missing. Use Build latest in Settings to set them up again."
            );
        }
        self.set("cpp", State::Running);
        // Fails when the window is already open, which is fine.
        let _ = Command::new("xcode-select").arg("--install").status();
        let started = Instant::now();
        while !command_line_tools() {
            cancelled()?;
            ensure!(
                started.elapsed() < Duration::from_secs(2 * 60 * 60),
                "Apple's Command Line Tools weren't installed. Install them, then try again."
            );
            let seconds = started.elapsed().as_secs();
            self.progress(
                "cpp",
                None,
                format!(
                    "Choose Install in the window macOS opened; it shows the progress ({}:{:02})",
                    seconds / 60,
                    seconds % 60
                ),
            );
            thread::sleep(Duration::from_secs(2));
        }
        self.set("cpp", State::Done);
        Ok(None)
    }

    /// The newest source, built and checked, then installed or staged.
    fn build_source(
        &mut self,
        root: &Path,
        client: &reqwest::blocking::Client,
        downloads: &Path,
    ) -> Result<Outcome> {
        let client = client.clone();
        let downloads = downloads.to_path_buf();
        // The source of the newest commit on main.
        self.set("source", State::Running);
        let latest = latest_commit(&client).ok();
        if let (Some(latest), Some(current)) = (&latest, COMMIT)
            && latest == current
        {
            self.set("source", State::Done);
            for id in [
                "rust",
                "packages",
                "interface",
                "crates",
                "app",
                "check",
                "install",
            ] {
                self.set(id, State::Skipped);
            }
            self.note = Some(format!(
                "You already have the latest Nightly ({}).",
                &current[..7.min(current.len())]
            ));
            return Ok(Outcome::Current);
        }
        let sources = root.join("source");
        let source = match latest.as_ref().map(|commit| sources.join(commit)) {
            Some(dir) if dir.join(".complete").is_file() => dir,
            _ => {
                let reference = latest.as_deref().unwrap_or("refs/heads/main");
                let archive = downloads.join("source.zip");
                self.download(
                    &client,
                    "source",
                    &format!("https://codeload.github.com/{REPOSITORY}/zip/{reference}"),
                    &archive,
                    None,
                )?;
                let staging = sources.join("incoming");
                let comment = self.extract("source", &archive, &staging, true)?;
                // GitHub names the commit in the archive's comment.
                let commit = latest
                    .clone()
                    .or_else(|| is_commit(comment.trim()).then(|| comment.trim().to_owned()))
                    .context("Cannot tell which commit the source is")?;
                let dir = sources.join(&commit);
                if dir.exists() {
                    fs::remove_dir_all(&dir)?;
                }
                fs::rename(&staging, &dir)?;
                fs::write(dir.join(".complete"), &commit)?;
                dir
            }
        };
        let commit = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .context("No source folder")?;
        if let Ok(entries) = fs::read_dir(&sources) {
            for entry in entries.flatten() {
                if entry.path() != source {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }
        self.log(&format!("Source: {commit}"));
        self.tools.commit = Some(commit.clone());
        self.set("source", State::Done);

        // Rust already installed when it is new enough for the source, else
        // the version the source asks for.
        let required = rust_version(&source)?;
        // Named for this system, so a rustup set to another host (as GNU
        // on Windows) still builds with the right one.
        let toolchain = format!("{required}-{}", system::HOST);
        let rustup = self.tools.rust_bin.join(format!("rustup{EXE}"));
        if !rustup.is_file()
            && let Some((bin, version)) = installed_rust(&required)
        {
            // Rust without rustup, as Homebrew installs it.
            self.log(&format!("Using Rust at {} ({version})", bin.display()));
            self.tools.rust_bin = bin;
            self.set("rust", State::Skipped);
        } else if rustup.is_file()
            && let Some(installed) = self.installed_toolchain(&rustup, &required)
        {
            self.log(&format!("Using your Rust toolchain {installed}"));
            self.tools.toolchain = Some(installed);
            self.set("rust", State::Skipped);
        } else {
            self.set("rust", State::Running);
            if !rustup.is_file() {
                let init = downloads.join(format!("rustup-init{EXE}"));
                let url = format!("{RUSTUP_DIST}/{}/rustup-init{EXE}", system::HOST);
                let hash = get_text(&client, &format!("{url}.sha256"))?;
                let hash = hash
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                self.download(&client, "rust", &url, &init, Some(&hash))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&init, fs::Permissions::from_mode(0o755))?;
                }
                self.command(
                    "rust",
                    &init,
                    &[
                        "-y",
                        "--no-modify-path",
                        "--default-toolchain",
                        "none",
                        "--profile",
                        "minimal",
                        "--default-host",
                        system::HOST,
                    ],
                    None,
                    Box::new(|_: &str| None),
                    1,
                )?;
                let _ = fs::remove_file(&init);
            }
            self.tools.toolchain = Some(toolchain.clone());
            if !self.succeeds(&rustup, &["run", &toolchain, "rustc", "--version"]) {
                let mut components = 0.0;
                self.command(
                    "rust",
                    &rustup,
                    &[
                        "toolchain",
                        "install",
                        &toolchain,
                        "--profile",
                        "minimal",
                        "--no-self-update",
                    ],
                    None,
                    Box::new(move |line: &str| {
                        // Three components, each downloaded then installed.
                        if line.contains("downloading component")
                            || line.contains("installing component")
                        {
                            components += 1.0;
                            Some(components / 6.0)
                        } else {
                            None
                        }
                    }),
                    3,
                )?;
            }
            self.set("rust", State::Done);
        }

        // The interface.
        let node = self.tools.node.clone().context("No Node.js")?;
        let npm = node.npm.to_string_lossy().into_owned();
        let packages = fs::read_to_string(source.join("package-lock.json"))
            .map(|lock| lock.matches("\"resolved\":").count())
            .unwrap_or(0)
            .max(1) as f64;
        self.set("packages", State::Running);
        self.command(
            "packages",
            &node.program,
            &[
                &npm,
                "ci",
                "--no-audit",
                "--no-fund",
                "--fetch-retries=5",
                "--loglevel=http",
            ],
            Some(&source),
            Box::new(counter(packages, |line| {
                line.contains("http fetch") && line.contains(".tgz")
            })),
            2,
        )?;
        self.set("packages", State::Done);
        self.set("interface", State::Running);
        self.command(
            "interface",
            &node.program,
            &[&npm, "run", "build"],
            Some(&source),
            Box::new(|_: &str| None),
            1,
        )?;
        self.set("interface", State::Done);

        // Lyssna itself: packages first, so a dropped connection never
        // costs a compile, then the build.
        let cargo = self.tools.rust_bin.join(format!("cargo{EXE}"));
        let lock = fs::read_to_string(source.join("Cargo.lock")).unwrap_or_default();
        let crates = lock.matches("source = \"registry+").count().max(1) as f64;
        let units = lock.matches("[[package]]").count().max(1) as f64;
        self.set("crates", State::Running);
        self.command(
            "crates",
            &cargo,
            &["fetch", "--locked"],
            Some(&source),
            Box::new(counter(crates, |line| line.starts_with("Downloaded "))),
            3,
        )?;
        self.set("crates", State::Done);
        self.set("app", State::Running);
        let build = ["build", "--release", "--locked", "--bin", "lyssna"];
        if let Err(error) = self.command(
            "app",
            &cargo,
            &build,
            Some(&source),
            Box::new(cargo_progress(units)),
            1,
        ) {
            // Files left by a different Rust, or by a build folder since
            // moved (as SpotiFlex's was to Lyssna's: build scripts keep
            // absolute paths), are cleared once; any other failure is the
            // source's and is reported as it is.
            let stale = format!("{error:#}");
            if ![
                "E0514",
                "E0460",
                "incompatible version of rustc",
                "failed to read plugin permissions",
            ]
            .iter()
            .any(|sign| stale.contains(sign))
            {
                return Err(error);
            }
            self.command(
                "app",
                &cargo,
                &["clean", "--release"],
                Some(&source),
                Box::new(|_: &str| None),
                1,
            )?;
            self.command(
                "app",
                &cargo,
                &build,
                Some(&source),
                Box::new(cargo_progress(units)),
                1,
            )?;
        }
        self.set("app", State::Done);

        // The new copy must start before it replaces this one.
        self.set("check", State::Running);
        let built = self
            .tools
            .target
            .join("release")
            .join(format!("lyssna{EXE}"));
        ensure!(built.is_file(), "The build finished without Lyssna");
        let output = self
            .output(&built, &["--version"], Duration::from_secs(60))
            .context("The new build didn't start")?;
        ensure!(
            output.contains("Lyssna") && output.contains(&commit[..7]),
            "The new build reported \"{}\" instead of Nightly {}",
            output.trim(),
            &commit[..7]
        );
        self.set("check", State::Done);

        if self.dry_run {
            self.set("install", State::Skipped);
            return Ok(Outcome::Current);
        }
        self.set("install", State::Running);
        let staged = stage(&built, &commit)?;
        let _ = fs::remove_dir_all(&downloads);
        if self.mode == Mode::Stage {
            if let Ok(mut ready) = READY.lock() {
                *ready = Some((commit.clone(), staged));
            }
            self.set("install", State::Done);
            return Ok(Outcome::Staged(commit));
        }
        replace_running(&staged)?;
        self.set("install", State::Done);
        Ok(Outcome::Installed)
    }

    /// Download with retries, showing progress on step `id`, and check the
    /// file against `sha256` when one is published. A file already there
    /// with the right hash isn't fetched again.
    fn download(
        &mut self,
        client: &reqwest::blocking::Client,
        id: &str,
        url: &str,
        path: &Path,
        sha256: Option<&str>,
    ) -> Result<()> {
        if let Some(expected) = sha256
            && file_hash(path).is_ok_and(|hash| hash.eq_ignore_ascii_case(expected))
        {
            return Ok(());
        }
        self.log(&format!("Downloading {url}"));
        let partial = path.with_extension("part");
        let mut attempt = 0;
        loop {
            attempt += 1;
            cancelled()?;
            match self.fetch(client, id, url, &partial, sha256) {
                Ok(()) => break,
                Err(error) if attempt < 4 && !CANCEL.load(Ordering::SeqCst) => {
                    self.log(&format!("Download failed, retrying: {error:#}"));
                    self.progress(id, None, format!("Retrying the download ({error})"));
                    thread::sleep(Duration::from_secs(2 * attempt));
                }
                Err(error) => {
                    let _ = fs::remove_file(&partial);
                    return Err(error.context(format!("Cannot download {url}")));
                }
            }
        }
        if path.exists() {
            fs::remove_file(path)?;
        }
        fs::rename(&partial, path)?;
        Ok(())
    }

    fn fetch(
        &mut self,
        client: &reqwest::blocking::Client,
        id: &str,
        url: &str,
        partial: &Path,
        sha256: Option<&str>,
    ) -> Result<()> {
        let chunks = stream(client, url);
        let mut file = fs::File::create(partial)?;
        let mut hash = Sha256::new();
        let mut received = 0u64;
        let mut total = None;
        let mut heard = Instant::now();
        loop {
            cancelled()?;
            // A connection that goes quiet fails and is retried, instead of
            // the build waiting on it for an hour.
            let piece = match chunks.recv_timeout(Duration::from_secs(1)) {
                Ok(piece) => piece,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    ensure!(
                        heard.elapsed() < DOWNLOAD_STALL,
                        "The download stopped receiving data"
                    );
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => bail!("The download ended early"),
            };
            heard = Instant::now();
            let data = match piece? {
                Piece::Start(length) => {
                    total = length;
                    continue;
                }
                Piece::End => break,
                Piece::Data(data) => data,
            };
            let size = data.len();
            file.write_all(&data)?;
            hash.update(&data);
            received += size as u64;
            let megabytes = received as f64 / 1e6;
            match total {
                Some(total) if total > 0 => self.progress(
                    id,
                    Some(received as f64 / total as f64),
                    format!(
                        "Downloading · {megabytes:.1} of {:.1} MB",
                        total as f64 / 1e6
                    ),
                ),
                _ => self.progress(id, None, format!("Downloading · {megabytes:.1} MB")),
            }
        }
        file.sync_all()?;
        if let Some(total) = total {
            ensure!(received == total, "The download ended early");
        }
        if let Some(expected) = sha256 {
            let actual = format!("{:x}", hash.finalize());
            ensure!(
                actual.eq_ignore_ascii_case(expected),
                "The download's checksum doesn't match"
            );
        }
        Ok(())
    }

    /// Unpack `archive` into `dir`, without its top folder when `strip`.
    /// Files land in a temporary folder that becomes `dir` when complete.
    /// Returns the archive's comment.
    fn extract(&mut self, id: &str, archive: &Path, dir: &Path, strip: bool) -> Result<String> {
        let staging = dir.with_extension("unpacking");
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        // macOS downloads come as .tar.gz, which its tar unpacks; it
        // refuses names that would leave the folder.
        if archive.to_string_lossy().ends_with(".tar.gz") {
            fs::create_dir_all(&staging)?;
            self.progress(id, None, "Unpacking");
            let mut tar = Command::new("tar");
            tar.arg("-xzf").arg(archive).arg("-C").arg(&staging);
            if strip {
                tar.args(["--strip-components", "1"]);
            }
            let status = tar.status().context("Cannot start tar")?;
            ensure!(status.success(), "Cannot unpack {}", archive.display());
            if dir.exists() {
                fs::remove_dir_all(dir)?;
            }
            fs::rename(&staging, dir)?;
            let _ = fs::remove_file(archive);
            return Ok(String::new());
        }
        let mut zip = zip::ZipArchive::new(fs::File::open(archive)?)?;
        let comment = String::from_utf8_lossy(zip.comment()).into_owned();
        let count = zip.len();
        for index in 0..count {
            cancelled()?;
            let mut entry = zip.by_index(index)?;
            // Names that would leave the folder are refused.
            let Some(name) = entry.enclosed_name() else {
                bail!("The archive holds an unsafe path");
            };
            let name: PathBuf = if strip {
                name.components().skip(1).collect()
            } else {
                name
            };
            if name.as_os_str().is_empty() {
                continue;
            }
            let path = staging.join(name);
            if entry.is_dir() {
                fs::create_dir_all(&path)?;
            } else {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                std::io::copy(&mut entry, &mut fs::File::create(&path)?)?;
            }
            if index % 50 == 0 {
                self.progress(
                    id,
                    Some(index as f64 / count as f64),
                    format!("Unpacking · {index} of {count} files"),
                );
            }
        }
        if dir.exists() {
            fs::remove_dir_all(dir)?;
        }
        fs::rename(&staging, dir)?;
        let _ = fs::remove_file(archive);
        Ok(comment)
    }

    /// The environment every build tool runs in.
    fn environment(&self, command: &mut Command) {
        command.env_clear();
        for name in KEPT_VARIABLES {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let mut path: Vec<PathBuf> = vec![self.tools.rust_bin.clone()];
        path.extend(
            self.tools
                .node
                .as_ref()
                .and_then(|node| node.program.parent().map(Path::to_path_buf)),
        );
        path.extend(
            self.tools
                .cmake
                .as_ref()
                .and_then(|cmake| cmake.parent().map(Path::to_path_buf)),
        );
        if cfg!(windows) {
            let system = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
            path.extend(
                [
                    "System32",
                    "",
                    "System32\\Wbem",
                    "System32\\WindowsPowerShell\\v1.0",
                ]
                .map(|dir| Path::new(&system).join(dir)),
            );
        } else {
            path.extend(["/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(PathBuf::from));
        }
        if let Ok(path) = std::env::join_paths(path) {
            command.env("PATH", path);
        }
        command
            .env("RUSTUP_HOME", &self.tools.rust_home)
            .env("CARGO_HOME", &self.tools.cargo_home)
            .env("CARGO_TARGET_DIR", &self.tools.target)
            .env("CARGO_NET_RETRY", "10")
            .env("RUSTUP_MAX_RETRIES", "5")
            .env("CARGO_TERM_COLOR", "never")
            .env("CARGO_TERM_PROGRESS_WHEN", "always")
            .env("CARGO_TERM_PROGRESS_WIDTH", "120")
            .env("npm_config_update_notifier", "false")
            .env(
                "npm_config_userconfig",
                self.tools.rust_home.with_file_name("npmrc"),
            )
            .env(
                "npm_config_cache",
                self.tools.rust_home.with_file_name("npm-cache"),
            )
            .env("PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD", "1")
            .env("NO_COLOR", "1");
        if let Some(toolchain) = &self.tools.toolchain {
            command.env("RUSTUP_TOOLCHAIN", toolchain);
        }
        if let Some(cmake) = &self.tools.cmake {
            command.env("CMAKE", cmake);
        }
        if let Some(libclang) = &self.tools.libclang {
            command.env("LIBCLANG_PATH", libclang);
        }
        if let Some(commit) = &self.tools.commit {
            // Marks the copy as Nightly; it keeps the updater, so choosing
            // Stable again installs the release.
            command.env("LYSSNA_NIGHTLY", commit);
        }
        isolate(command);
    }

    /// Run a tool, showing its output and progress on step `id`, trying
    /// up to `attempts` times.
    fn command(
        &mut self,
        id: &str,
        program: &Path,
        args: &[&str],
        dir: Option<&Path>,
        mut progress: Progress,
        attempts: u32,
    ) -> Result<()> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.run_once(id, program, args, dir, &mut progress) {
                Ok(()) => return Ok(()),
                Err(error) if attempt < attempts && !CANCEL.load(Ordering::SeqCst) => {
                    self.log(&format!("Failed, trying again: {error:#}"));
                    self.progress(id, None, "Trying again");
                    thread::sleep(Duration::from_secs(5 * u64::from(attempt)));
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn run_once(
        &mut self,
        id: &str,
        program: &Path,
        args: &[&str],
        dir: Option<&Path>,
        progress: &mut Progress,
    ) -> Result<()> {
        cancelled()?;
        let name = program
            .file_stem()
            .map_or("tool".into(), |name| name.to_string_lossy().into_owned());
        self.log(&format!("$ {} {}", program.display(), args.join(" ")));
        let mut command = Command::new(program);
        self.environment(&mut command);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = dir {
            command.current_dir(dir);
        }
        let job = Arc::new(Job::new()?);
        let mut child = command
            .spawn()
            .with_context(|| format!("Cannot start {name}"))?;
        job.assign(&child);
        if let Ok(mut current) = JOB.lock() {
            *current = Some(job.clone());
        }
        let (sender, lines) = mpsc::channel();
        for pipe in [
            child
                .stdout
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
            child
                .stderr
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let sender = sender.clone();
            thread::spawn(move || read_lines(pipe, sender));
        }
        drop(sender);
        let mut tail: VecDeque<String> = VecDeque::new();
        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            self.log(line);
            let fraction = progress(line);
            self.progress(id, fraction, line);
            if !line.starts_with("Building [") {
                tail.push_back(line.to_owned());
                if tail.len() > 12 {
                    tail.pop_front();
                }
            }
        }
        let status = child.wait()?;
        if let Ok(mut current) = JOB.lock() {
            *current = None;
        }
        drop(job);
        cancelled()?;
        if status.success() {
            return Ok(());
        }
        let code = status
            .code()
            .map_or("stopped".into(), |code| format!("exit code {code}"));
        let tail: Vec<String> = tail.into_iter().collect();
        bail!("{name} failed ({code}):\n{}", tail.join("\n"))
    }

    /// Whether a quick command succeeds.
    /// A toolchain the user's rustup already has for this system that is new
    /// enough for the source, the newest first.
    fn installed_toolchain(&self, rustup: &Path, required: &str) -> Option<String> {
        let list = self
            .output(rustup, &["toolchain", "list"], Duration::from_secs(30))
            .ok()?;
        let mut found: Vec<(Vec<u32>, String)> = list
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|name| name.ends_with(system::HOST))
            .filter_map(|name| {
                let version = self
                    .output(
                        rustup,
                        &["run", name, "rustc", "--version"],
                        Duration::from_secs(30),
                    )
                    .ok()?;
                let version = version.trim();
                rust_new_enough(version, required).then(|| {
                    let number = version
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .split('.')
                        .filter_map(|part| part.parse().ok())
                        .collect();
                    (number, name.to_owned())
                })
            })
            .collect();
        found.sort();
        found.pop().map(|(_, name)| name)
    }

    fn succeeds(&self, program: &Path, args: &[&str]) -> bool {
        let mut command = Command::new(program);
        self.environment(&mut command);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// A command's output, stopped after `limit`.
    fn output(&self, program: &Path, args: &[&str], limit: Duration) -> Result<String> {
        let mut command = Command::new(program);
        self.environment(&mut command);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let job = Job::new()?;
        let mut child = command.spawn()?;
        job.assign(&child);
        let mut stdout = child.stdout.take().context("No output")?;
        let reader = thread::spawn(move || {
            let mut text = String::new();
            let _ = stdout.read_to_string(&mut text);
            text
        });
        let started = Instant::now();
        loop {
            if child.try_wait()?.is_some() {
                break;
            }
            if started.elapsed() > limit {
                job.terminate();
                bail!("It didn't finish within {} seconds", limit.as_secs());
            }
            thread::sleep(Duration::from_millis(100));
        }
        Ok(reader.join().unwrap_or_default())
    }
}

fn state_name(state: State) -> &'static str {
    match state {
        State::Pending => "waiting",
        State::Running => "started",
        State::Done => "done",
        State::Skipped => "already there",
        State::Failed => "failed",
    }
}

fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(concat!("Lyssna/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(60 * 60))
        .build()?)
}

/// How long a download may go without data before it is retried.
const DOWNLOAD_STALL: Duration = Duration::from_secs(30);
enum Piece {
    Start(Option<u64>),
    Data(Vec<u8>),
    End,
}
/// Fetches `url` on its own thread, so a stalled connection never blocks
/// Cancel or the stall check. The thread ends when the reader goes away.
fn stream(client: &reqwest::blocking::Client, url: &str) -> mpsc::Receiver<Result<Piece>> {
    let (sender, receiver) = mpsc::sync_channel(8);
    let (client, url) = (client.clone(), url.to_owned());
    thread::spawn(move || {
        let mut response = match client
            .get(&url)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
        {
            Ok(response) => response,
            Err(error) => {
                let _ = sender.send(Err(error.into()));
                return;
            }
        };
        if sender
            .send(Ok(Piece::Start(response.content_length())))
            .is_err()
        {
            return;
        }
        let mut buffer = vec![0u8; 256 * 1024];
        loop {
            let piece = match response.read(&mut buffer) {
                Ok(0) => Ok(Piece::End),
                Ok(size) => Ok(Piece::Data(buffer[..size].to_vec())),
                Err(error) => Err(error.into()),
            };
            let last = !matches!(piece, Ok(Piece::Data(_)));
            if sender.send(piece).is_err() || last {
                return;
            }
        }
    });
    receiver
}

/// A small text file, with retries.
fn get_text(client: &reqwest::blocking::Client, url: &str) -> Result<String> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let result = client
            .get(url)
            .timeout(Duration::from_secs(30))
            .send()
            .and_then(|response| response.error_for_status())
            .and_then(|response| response.text());
        match result {
            Ok(text) => return Ok(text),
            Err(_) if attempt < 4 => thread::sleep(Duration::from_secs(2 * attempt)),
            Err(error) => {
                return Err(anyhow::Error::new(error).context(format!("Cannot reach {url}")));
            }
        }
    }
}

/// The newest commit on main.
fn latest_commit(client: &reqwest::blocking::Client) -> Result<String> {
    let response = client
        .get(format!(
            "https://api.github.com/repos/{REPOSITORY}/commits/main"
        ))
        .header("Accept", "application/vnd.github.sha")
        .timeout(Duration::from_secs(30))
        .send()?
        .error_for_status()?;
    let commit = response.text()?.trim().to_owned();
    ensure!(is_commit(&commit), "GitHub didn't return a commit");
    Ok(commit)
}

/// The commits since this copy for the latest Nightly, and that commit.
static CHANGES: Mutex<(String, String)> = Mutex::new((String::new(), String::new()));
/// At most this many commits are listed.
const CHANGES_SHOWN: usize = 30;

/// What the latest Nightly changes since this copy, newest first: one
/// "abc1234 Subject" line per commit. Empty when GitHub couldn't say.
pub fn changes() -> String {
    CHANGES
        .lock()
        .map(|changes| changes.1.clone())
        .unwrap_or_default()
}

fn remember_changes(client: &reqwest::blocking::Client, current: &str, latest: &str) {
    if CHANGES.lock().is_ok_and(|changes| changes.0 == latest) {
        return;
    }
    let url = format!("https://api.github.com/repos/{REPOSITORY}/compare/{current}...{latest}");
    let lines = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(30))
        .send()
        .and_then(|response| response.error_for_status())
        .and_then(|response| response.json::<serde_json::Value>())
        .map(|comparison| change_lines(&comparison))
        .unwrap_or_else(|error| {
            log::warn!("Cannot list Nightly changes: {error}");
            String::new()
        });
    if let Ok(mut changes) = CHANGES.lock() {
        *changes = (latest.to_owned(), lines);
    }
}

/// GitHub's comparison as change lines, newest first.
fn change_lines(comparison: &serde_json::Value) -> String {
    let commits = comparison["commits"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut lines: Vec<String> = commits
        .iter()
        .rev()
        .filter_map(|commit| {
            let sha = commit["sha"].as_str().filter(|sha| is_commit(sha))?;
            let message = commit["commit"]["message"].as_str()?;
            let (kind, text) = change(message)?;
            Some(format!("{} {kind} {text}", &sha[..7]))
        })
        .take(CHANGES_SHOWN)
        .collect();
    let total = comparison["total_commits"]
        .as_u64()
        .map_or(commits.len(), |total| total as usize);
    if total > lines.len() && !lines.is_empty() {
        lines.push(format!("and {} more", total - lines.len()));
    }
    lines.join("\n")
}

/// What one commit changed for listeners, and whether it is "new",
/// "fixed" or "other". Its "Nightly:" line says it in plain words; older
/// commits only have their subject, shown without its type.
fn change(message: &str) -> Option<(&'static str, String)> {
    let subject = message.lines().next()?.trim();
    let (kind, rest) = match subject.split_once(": ") {
        Some((prefix, rest)) if !prefix.contains(' ') => {
            let kind = prefix.split('(').next().unwrap_or_default();
            let kind = match kind.trim_end_matches('!') {
                "feat" => "new",
                "fix" => "fixed",
                _ => "other",
            };
            (kind, rest)
        }
        _ => ("other", subject),
    };
    let text = message
        .lines()
        .find_map(|line| line.trim().strip_prefix("Nightly:"))
        .unwrap_or(rest)
        .trim();
    let mut text: String = text
        .chars()
        .filter(|character| !character.is_control())
        .take(160)
        .collect();
    if let Some(first) = text.chars().next() {
        text.replace_range(..first.len_utf8(), &first.to_uppercase().to_string());
    }
    (!text.is_empty()).then_some((kind, text))
}

fn is_commit(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The file name and SHA-256 of the line in a checksum list whose file
/// ends with `suffix`.
fn checksum_for(sums: &str, suffix: &str) -> Option<(String, String)> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*').rsplit('/').next()?;
        (name.ends_with(suffix) && hash.len() == 64).then(|| (name.to_owned(), hash.to_owned()))
    })
}

/// This system's wheel in PyPI's release listing, with its SHA-256.
fn libclang_wheel(release: &serde_json::Value) -> Option<(String, String)> {
    wheel_for(release, system::LIBCLANG_WHEEL)
}

fn wheel_for(release: &serde_json::Value, suffix: &str) -> Option<(String, String)> {
    release["urls"].as_array()?.iter().find_map(|file| {
        let name = file["filename"].as_str()?;
        let system = if suffix.starts_with("win") {
            true
        } else if suffix.starts_with("manylinux") {
            name.contains("manylinux")
        } else {
            name.contains("macosx")
        };
        (system && name.ends_with(suffix)).then(|| {
            Some((
                file["url"].as_str()?.to_owned(),
                file["digests"]["sha256"].as_str()?.to_owned(),
            ))
        })?
    })
}

/// A file named `name` in `dir` or its folders, `depth` levels down.
fn find_file(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let mut folders = Vec::new();
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            folders.push(path);
        } else if entry.file_name().eq_ignore_ascii_case(name) {
            return Some(path);
        }
    }
    if depth == 0 {
        return None;
    }
    folders.sort();
    folders
        .iter()
        .find_map(|folder| find_file(folder, name, depth - 1))
}

/// The newest folder in `dir` starting with `prefix`.
fn newest_dir(dir: &Path, prefix: &str) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(prefix))
        })
        .collect();
    found.sort();
    found.pop()
}

fn file_hash(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

/// Lines from a pipe; progress bars rewrite theirs with `\r`, so that
/// ends a line too.
fn read_lines(mut pipe: Box<dyn Read + Send>, lines: mpsc::Sender<String>) {
    let mut buffer = [0u8; 8192];
    let mut line = Vec::new();
    while let Ok(size) = pipe.read(&mut buffer) {
        if size == 0 {
            break;
        }
        for &byte in &buffer[..size] {
            if byte == b'\n' || byte == b'\r' {
                if !line.is_empty() {
                    let _ = lines.send(String::from_utf8_lossy(&line).into_owned());
                    line.clear();
                }
            } else {
                line.push(byte);
            }
        }
    }
    if !line.is_empty() {
        let _ = lines.send(String::from_utf8_lossy(&line).into_owned());
    }
}

/// Progress as matching lines out of `total`.
fn counter(total: f64, matches: fn(&str) -> bool) -> impl FnMut(&str) -> Option<f64> {
    let mut seen = 0.0;
    move |line| {
        if matches(line) {
            seen += 1.0;
            Some((seen / total).min(0.99))
        } else {
            None
        }
    }
}

/// Cargo's `Building [=====>   ] 123/456: crate` bar, or failing that the
/// `Compiling` lines out of the packages in Cargo.lock.
fn cargo_progress(units: f64) -> impl FnMut(&str) -> Option<f64> {
    let mut compiled = 0.0;
    let mut bar = false;
    move |line| {
        if let Some(fraction) = cargo_bar(line) {
            bar = true;
            return Some(fraction);
        }
        if !bar && line.starts_with("Compiling ") {
            compiled += 1.0;
            return Some((compiled / units).min(0.99));
        }
        None
    }
}

fn cargo_bar(line: &str) -> Option<f64> {
    let after = line.strip_prefix("Building [")?.split_once("] ")?.1;
    let (done, total) = after.split(':').next()?.trim().split_once('/')?;
    let total = total.parse::<f64>().ok()?;
    (total > 0.0).then_some(done.parse::<f64>().ok()? / total)
}

/// Cargo.toml's `rust-version`, as a full version rustup installs.
fn rust_version(source: &Path) -> Result<String> {
    let manifest = fs::read_to_string(source.join("Cargo.toml"))?;
    let version = manifest
        .lines()
        .find_map(|line| {
            let value = line
                .strip_prefix("rust-version")?
                .trim()
                .strip_prefix('=')?;
            Some(value.trim().trim_matches('"').to_owned())
        })
        .context("Cargo.toml is missing rust-version")?;
    ensure!(
        version.split('.').all(|part| part.parse::<u32>().is_ok()),
        "Cargo.toml's rust-version is not a version: {version}"
    );
    Ok(if version.split('.').count() == 2 {
        format!("{version}.0")
    } else {
        version
    })
}

/// Where Cargo builds: a folder of its own inside `target`, so a copy run
/// from an earlier build's `target\release` is never in Cargo's way.
/// Clearing build files still removes all of `target`.
fn cargo_target(root: &Path) -> PathBuf {
    root.join("target").join("nightly")
}

/// A copy of the build beside the updates, so the build folder stays
/// free to change while it waits to be installed.
fn stage(built: &Path, commit: &str) -> Result<PathBuf> {
    let updates = crate::config::data_dir()?.join("updates");
    fs::create_dir_all(&updates)?;
    let staged = updates.join(format!("lyssna-nightly-{}{EXE}", &commit[..7]));
    fs::copy(built, &staged)?;
    Ok(staged)
}

/// Replace the running executable, keeping a copy of it.
fn replace_running(staged: &Path) -> Result<()> {
    let updates = crate::config::data_dir()?.join("updates");
    let executable = std::env::current_exe()?;
    fs::copy(&executable, updates.join(format!("previous-lyssna{EXE}")))
        .context("Cannot back up the current executable")?;
    self_replace::self_replace(staged)
        .context("Cannot install. Run Lyssna from a folder your account can write to.")?;
    let _ = fs::remove_file(staged);
    // The app's signature covers the executable it replaced: bring its
    // Info.plist up to date, sign it again as the build script did, and
    // register it so macOS opens it and its links.
    #[cfg(target_os = "macos")]
    if let Some(bundle) = executable
        .ancestors()
        .find(|path| path.extension().is_some_and(|extension| extension == "app"))
        && let Err(error) = crate::mac_bundle::after_replace(bundle)
    {
        log::warn!("Cannot sign {} again: {error:#}", bundle.display());
    }
    Ok(())
}

/// A tool runs without a window on Windows and, elsewhere, in a process
/// group of its own, so Cancel stops it and everything it started.
fn isolate(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(not(any(windows, unix)))]
    let _ = command;
}

/// The user's own rustup, its homes and the folder with its programs, when
/// Rust is installed with it: in Cargo's folder, or elsewhere on PATH (as
/// Homebrew's rustup).
fn installed_rustup() -> Option<(PathBuf, PathBuf, PathBuf)> {
    let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
    let cargo_home =
        std::env::var_os("CARGO_HOME").map_or_else(|| home.join(".cargo"), PathBuf::from);
    let rust_home =
        std::env::var_os("RUSTUP_HOME").map_or_else(|| home.join(".rustup"), PathBuf::from);
    if !rust_home.is_dir() {
        return None;
    }
    let bin = Some(cargo_home.join("bin"))
        .filter(|bin| bin.join(format!("rustup{EXE}")).is_file())
        .or_else(|| {
            installed("rustup")
                .and_then(|rustup| rustup.parent().map(Path::to_path_buf))
                .filter(|bin| bin.join(format!("cargo{EXE}")).is_file())
        })?;
    Some((rust_home, cargo_home, bin))
}

/// Rust installed without rustup, as Homebrew's, when it builds the source.
fn installed_rust(required: &str) -> Option<(PathBuf, String)> {
    let bin = installed("cargo")?.parent()?.to_path_buf();
    let version = version_of(&bin.join(format!("rustc{EXE}")))?;
    rust_new_enough(&version, required).then_some((bin, version))
}

/// Whether `rustc --version` output is a stable Rust at least `required`.
fn rust_new_enough(output: &str, required: &str) -> bool {
    let parse = |version: &str| -> Option<Vec<u32>> {
        version.split('.').map(|part| part.parse().ok()).collect()
    };
    let Some(version) = output
        .strip_prefix("rustc ")
        .and_then(|rest| rest.split_whitespace().next())
    else {
        return false;
    };
    // Beta and nightly compilers may build differently; only releases.
    match (parse(version), parse(required)) {
        (Some(version), Some(required)) => version >= required,
        _ => false,
    }
}

/// A program on PATH or in the folders it is usually installed in, which
/// an app opened from the Dock or Start doesn't always have on PATH.
fn installed(name: &str) -> Option<PathBuf> {
    let file = format!("{name}{EXE}");
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if cfg!(windows) {
        if let Some(files) = std::env::var_os("ProgramFiles").map(PathBuf::from) {
            dirs.extend([files.join("nodejs"), files.join("CMake").join("bin")]);
        }
    } else {
        dirs.extend(
            [
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/Applications/CMake.app/Contents/bin",
            ]
            .map(PathBuf::from),
        );
    }
    dirs.into_iter()
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(&file))
        .find(|path| path.is_file())
}

/// A quick command's output, when it succeeds.
fn version_of(program: &Path) -> Option<String> {
    let mut command = Command::new(program);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    isolate(&mut command);
    let output = command.output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Whether a Node.js version runs the interface's tools: Vite needs
/// 20.19 or 22.12 and later.
fn node_supported(version: &str) -> bool {
    let mut parts = version.trim().trim_start_matches('v').split('.');
    let (Some(Ok(major)), Some(Ok(minor))) = (
        parts.next().map(str::parse::<u32>),
        parts.next().map(str::parse::<u32>),
    ) else {
        return false;
    };
    matches!((major, minor), (20, 19..) | (22, 12..) | (23.., _))
}

/// An installed Node.js recent enough, with its npm.
fn installed_node() -> Option<Node> {
    let program = installed("node")?;
    if !node_supported(&version_of(&program)?) {
        return None;
    }
    // A Homebrew or nvm `node` is often a link into its own folder.
    let real = fs::canonicalize(&program).ok().filter(|_| !cfg!(windows));
    [Some(program.clone()), real]
        .into_iter()
        .flatten()
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .flat_map(|bin| {
            let prefix = bin.parent().map(Path::to_path_buf).unwrap_or_default();
            [
                bin.join("node_modules/npm/bin/npm-cli.js"),
                prefix.join("lib/node_modules/npm/bin/npm-cli.js"),
                prefix.join("libexec/lib/node_modules/npm/bin/npm-cli.js"),
            ]
        })
        .find(|npm| npm.is_file())
        .map(|npm| Node {
            program: program.clone(),
            npm,
        })
}

/// An installed CMake that runs.
fn installed_cmake() -> Option<PathBuf> {
    installed("cmake").filter(|cmake| version_of(cmake).is_some())
}

/// An installed libclang: the one LIBCLANG_PATH names, LLVM's, Visual
/// Studio's or Apple's.
fn installed_libclang(studio: Option<&Path>) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("LIBCLANG_PATH")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    if cfg!(windows) {
        dirs.extend(studio.map(|studio| studio.join("VC\\Tools\\Llvm\\x64\\bin")));
        if let Some(files) = std::env::var_os("ProgramFiles").map(PathBuf::from) {
            dirs.push(files.join("LLVM").join("bin"));
        }
    } else {
        if let Some(developer) = Command::new("xcode-select")
            .arg("-p")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
        {
            dirs.push(developer.join("usr/lib"));
            dirs.push(developer.join("Toolchains/XcodeDefault.xctoolchain/usr/lib"));
        }
        dirs.extend(
            [
                "/Library/Developer/CommandLineTools/usr/lib",
                "/opt/homebrew/opt/llvm/lib",
                "/usr/local/opt/llvm/lib",
            ]
            .map(PathBuf::from),
        );
    }
    dirs.into_iter()
        .map(|dir| dir.join(system::LIBCLANG))
        .find(|path| path.is_file())
}

/// Whether Apple's Command Line Tools, or Xcode, can compile.
#[cfg(not(windows))]
fn command_line_tools() -> bool {
    Command::new("xcode-select")
        .arg("-p")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
        .is_some_and(|developer| {
            developer.join("usr/bin/clang").is_file()
                || developer
                    .join("Toolchains/XcodeDefault.xctoolchain/usr/bin/clang")
                    .is_file()
        })
}

/// A Visual Studio with the C++ build tools.
#[cfg(windows)]
fn visual_studio() -> Option<PathBuf> {
    let program_files = std::env::var("ProgramFiles(x86)").ok()?;
    let vswhere = Path::new(&program_files).join("Microsoft Visual Studio\\Installer\\vswhere.exe");
    let mut command = Command::new(vswhere);
    command.args([
        "-products",
        "*",
        "-requires",
        "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
        "-property",
        "installationPath",
    ]);
    isolate(&mut command);
    let output = command.output().ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(PathBuf::from)
}

/// CMake as Visual Studio's C++ tools ship it.
fn bundled_cmake(visual_studio: &Path) -> Option<PathBuf> {
    let cmake = visual_studio
        .join("Common7\\IDE\\CommonExtensions\\Microsoft\\CMake\\CMake\\bin\\cmake.exe");
    cmake.is_file().then_some(cmake)
}

fn check_space(path: &Path, needed: u64) -> Result<()> {
    if let Some(free) = free_space(path) {
        ensure!(
            free >= needed,
            "{} needs about {} GB free for the build; it has {:.1} GB.",
            path.display(),
            needed / GB,
            free as f64 / GB as f64
        );
    }
    Ok(())
}

#[cfg(not(windows))]
use self::other::{Job, free_space};
#[cfg(windows)]
use self::windows_impl::{Job, free_space, run_elevated};

#[cfg(windows)]
mod windows_impl {
    use anyhow::{Result, bail};
    use std::{os::windows::io::AsRawHandle, path::Path, time::Duration};
    use windows::{
        Win32::{
            Foundation::{CloseHandle, ERROR_CANCELLED, HANDLE, WAIT_OBJECT_0},
            Storage::FileSystem::GetDiskFreeSpaceExW,
            System::{
                JobObjects::{
                    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    JOB_OBJECT_LIMIT_PRIORITY_CLASS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                    JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
                },
                Threading::{BELOW_NORMAL_PRIORITY_CLASS, GetExitCodeProcess, WaitForSingleObject},
            },
            UI::{
                Shell::{
                    SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
                },
                WindowsAndMessaging::SW_SHOWNORMAL,
            },
        },
        core::{HSTRING, PCWSTR, w},
    };

    /// Processes that are stopped together, and when Lyssna exits;
    /// they run below normal priority so playback stays smooth.
    pub struct Job(HANDLE);
    // The handle is only passed to thread-safe system calls.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn new() -> Result<Self> {
            let job = unsafe { CreateJobObjectW(None, PCWSTR::null())? };
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags =
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_PRIORITY_CLASS;
            limits.BasicLimitInformation.PriorityClass = BELOW_NORMAL_PRIORITY_CLASS.0;
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
                log::warn!("Build tool runs outside its job: {error}");
            }
        }

        pub fn terminate(&self) {
            let _ = unsafe { TerminateJobObject(self.0, 1) };
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    pub fn free_space(path: &Path) -> Option<u64> {
        let mut dir = path;
        while !dir.exists() {
            dir = dir.parent()?;
        }
        let name = HSTRING::from(dir);
        let mut free = 0u64;
        unsafe { GetDiskFreeSpaceExW(PCWSTR(name.as_ptr()), Some(&raw mut free), None, None) }
            .ok()?;
        Some(free)
    }

    /// Run `program` with Windows' permission prompt and wait for its exit
    /// code, calling `tick` every second.
    pub fn run_elevated(program: &Path, arguments: &str, tick: &mut dyn FnMut()) -> Result<u32> {
        let file = HSTRING::from(program);
        let parameters = HSTRING::from(arguments);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(parameters.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        if let Err(error) = unsafe { ShellExecuteExW(&mut info) } {
            if error.code() == ERROR_CANCELLED.to_hresult() {
                bail!("Windows' permission to install Visual Studio's C++ tools was declined.");
            }
            bail!("Cannot start Visual Studio's installer: {error}");
        }
        let process = info.hProcess;
        let result = (|| {
            loop {
                if super::CANCEL.load(std::sync::atomic::Ordering::SeqCst) {
                    bail!("Cancelled; Visual Studio's installer finishes on its own");
                }
                if unsafe { WaitForSingleObject(process, 1000) } == WAIT_OBJECT_0 {
                    break;
                }
                tick();
            }
            let mut code = 0u32;
            unsafe { GetExitCodeProcess(process, &mut code)? };
            Ok(code)
        })();
        let _ = unsafe { CloseHandle(process) };
        std::thread::sleep(Duration::from_millis(10));
        result
    }
}

#[cfg(not(windows))]
mod other {
    use anyhow::Result;
    use std::{path::Path, process::Command, sync::Mutex};

    /// The process groups of running tools, stopped together.
    pub struct Job(Mutex<Vec<u32>>);

    impl Job {
        pub fn new() -> Result<Self> {
            Ok(Self(Mutex::new(Vec::new())))
        }
        pub fn assign(&self, child: &std::process::Child) {
            if let Ok(mut groups) = self.0.lock() {
                groups.push(child.id());
            }
        }
        pub fn terminate(&self) {
            for group in self
                .0
                .lock()
                .map(|groups| groups.clone())
                .unwrap_or_default()
            {
                let _ = Command::new("kill")
                    .args(["-KILL", &format!("-{group}")])
                    .status();
            }
        }
    }

    /// Free space from `df`, in bytes.
    pub fn free_space(path: &Path) -> Option<u64> {
        let mut dir = path;
        while !dir.exists() {
            dir = dir.parent()?;
        }
        let output = Command::new("df").arg("-Pk").arg(dir).output().ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        let kilobytes: u64 = text
            .lines()
            .nth(1)?
            .split_whitespace()
            .nth(3)?
            .parse()
            .ok()?;
        Some(kilobytes * 1024)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn builds_stay_clear_of_an_earlier_builds_executable() {
        let root = std::path::Path::new("nightly");
        let target = super::cargo_target(root);
        assert_ne!(target.join("release"), root.join("target").join("release"));
        assert!(target.starts_with(root.join("target")));
    }

    use super::*;

    #[test]
    fn a_silent_download_never_blocks_the_build() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (done, hold) = mpsc::channel::<()>();
        thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc")
                .unwrap();
            // Holds the connection open without sending the rest.
            let _ = hold.recv();
        });
        let client = reqwest::blocking::Client::new();
        let pieces = stream(&client, &format!("http://{address}/file"));
        let wait = || {
            pieces
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap()
        };
        assert!(matches!(wait(), Piece::Start(Some(10))));
        assert!(matches!(wait(), Piece::Data(data) if data == b"abc"));
        // Silence comes back as a timeout the build can act on.
        assert!(matches!(
            pieces.recv_timeout(Duration::from_millis(300)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        drop(done);
    }

    #[test]
    fn progress_is_read_from_each_tool() {
        assert_eq!(
            cargo_bar("Building [=====>      ] 123/492: serde, tokio"),
            Some(0.25)
        );
        assert_eq!(cargo_bar("Compiling serde v1.0.0"), None);
        let mut build = cargo_progress(4.0);
        assert_eq!(build("Compiling serde v1.0.0"), Some(0.25));
        assert_eq!(build("Building [=>   ] 1/2: x"), Some(0.5));
        // Once the bar shows, it alone counts.
        assert_eq!(build("Compiling tokio v1.0.0"), None);
        let mut npm = counter(2.0, |line| line.contains(".tgz"));
        assert_eq!(
            npm("npm http fetch GET 200 https://x/a.tgz 10ms"),
            Some(0.5)
        );
        assert_eq!(
            npm("npm http fetch GET 200 https://x/b.tgz 10ms"),
            Some(0.99)
        );
        assert_eq!(npm("added 2 packages"), None);
    }

    #[test]
    fn checksums_name_the_right_download() {
        let node = "aaaa  node-v24.1.0.pkg\n\
            1111111111111111111111111111111111111111111111111111111111111111  node-v24.1.0-win-x64.zip\n";
        assert_eq!(
            checksum_for(node, "-win-x64.zip"),
            Some((
                "node-v24.1.0-win-x64.zip".into(),
                "1111111111111111111111111111111111111111111111111111111111111111".into()
            ))
        );
        let rustup = "2222222222222222222222222222222222222222222222222222222222222222 *./x86_64-pc-windows-msvc/rustup-init.exe";
        assert_eq!(
            checksum_for(rustup, "rustup-init.exe").map(|(name, _)| name),
            Some("rustup-init.exe".into())
        );
        let pypi = serde_json::json!({"urls": [
            {"filename": "libclang-18.1.1-py2.py3-none-manylinux2010_x86_64.whl", "url": "l", "digests": {"sha256": "x"}},
            {"filename": "libclang-18.1.1-py2.py3-none-win_amd64.whl", "url": "w", "digests": {"sha256": "y"}},
        ]});
        assert_eq!(
            wheel_for(&pypi, "win_amd64.whl"),
            Some(("w".into(), "y".into()))
        );
        let pypi = serde_json::json!({"urls": [
            {"filename": "libclang-18.1.1-py2.py3-none-manylinux2014_aarch64.whl", "url": "l", "digests": {"sha256": "x"}},
            {"filename": "libclang-18.1.1-1-py2.py3-none-macosx_11_0_arm64.whl", "url": "m", "digests": {"sha256": "z"}},
        ]});
        assert_eq!(
            wheel_for(&pypi, "arm64.whl"),
            Some(("m".into(), "z".into()))
        );
    }

    #[test]
    fn rust_versions_get_a_patch_number() {
        let dir = std::env::temp_dir().join(format!("lyssna-nightly-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nrust-version = \"1.98\"\n",
        )
        .unwrap();
        assert_eq!(rust_version(&dir).unwrap(), "1.98.0");
        fs::write(dir.join("Cargo.toml"), "rust-version = \"1.98; rm\"\n").unwrap();
        assert!(rust_version(&dir).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    /// The whole pipeline against the real services, without installing:
    /// `cargo test --lib nightly_end_to_end -- --ignored --nocapture`.
    #[test]
    #[ignore = "downloads the build tools and builds Lyssna"]
    fn nightly_end_to_end() {
        let seen = Mutex::new(Vec::new());
        let mut run = Run::new(move |report: &Report| {
            let states: Vec<String> = report
                .steps
                .iter()
                .map(|step| format!("{} {}", step.label, state_name(step.state)))
                .collect();
            let mut seen = seen.lock().unwrap();
            for state in &states {
                if !seen.contains(state) {
                    eprintln!("{state}");
                }
            }
            *seen = states;
        });
        run.dry_run = true;
        let result = run.build();
        if let Some(log) = &run.log_path {
            eprintln!("Log: {}", log.display());
        }
        result.unwrap();
    }

    #[test]
    fn changes_list_newest_first_and_count_the_rest() {
        let commit = |sha: char, message: &str| serde_json::json!({"sha": sha.to_string().repeat(40), "commit": {"message": message}});
        let comparison = serde_json::json!({
            "total_commits": 40,
            "commits": [commit('a', "fix: first\n\nbody"), commit('b', "feat(x): second\n\nNightly: Second is nicer."), {"sha": "bad"}],
        });
        assert_eq!(
            change_lines(&comparison),
            "bbbbbbb new Second is nicer.\naaaaaaa fixed First\nand 38 more"
        );
        assert_eq!(change_lines(&serde_json::json!({})), "");
        assert_eq!(
            change("perf!: faster lyrics"),
            Some(("other", "Faster lyrics".into()))
        );
        assert_eq!(
            change("Merge pull request #3 from fork: main"),
            Some(("other", "Merge pull request #3 from fork: main".into()))
        );
    }
    #[test]
    fn installed_rust_is_reused_when_it_is_a_new_enough_release() {
        let output = "rustc 1.99.1 (abcdef012 2026-10-01)";
        assert!(rust_new_enough(output, "1.98.0"));
        assert!(rust_new_enough("rustc 1.98.0 (x 2026-08-01)", "1.98.0"));
        assert!(!rust_new_enough("rustc 1.97.2 (x 2026-07-01)", "1.98.0"));
        assert!(!rust_new_enough(
            "rustc 1.99.0-nightly (x 2026-09-01)",
            "1.98.0"
        ));
        assert!(!rust_new_enough("cargo 1.99.0", "1.98.0"));
    }

    #[test]
    fn only_node_versions_vite_runs_on_are_used() {
        for version in ["v20.19.0", "v22.12.0", "v24.1.0", "23.0.0"] {
            assert!(node_supported(version), "{version}");
        }
        for version in ["v20.18.1", "v21.7.3", "v22.11.0", "v18.20.0", "", "vx"] {
            assert!(!node_supported(version), "{version}");
        }
    }
    #[test]
    fn commits_are_full_hashes() {
        assert!(is_commit("0adf971c0ffee0adf971c0ffee0adf971c0ffee0"));
        assert!(!is_commit("0adf971"));
        assert!(!is_commit("refs/heads/main"));
    }

    #[test]
    fn archives_unpack_without_their_top_folder_and_refuse_escapes() {
        let dir = std::env::temp_dir().join(format!("lyssna-unpack-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, entries: &[&str]| {
            let path = dir.join(name);
            let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
            for entry in entries {
                zip.start_file(*entry, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(b"x").unwrap();
            }
            zip.set_comment("0adf971c0ffee0adf971c0ffee0adf971c0ffee0");
            zip.finish().unwrap();
            path
        };
        let mut run = Run::new(|_: &Report| {});
        let good = write(
            "good.zip",
            &["Lyssna-abc/Cargo.toml", "Lyssna-abc/src/main.rs"],
        );
        let comment = run
            .extract("source", &good, &dir.join("good"), true)
            .unwrap();
        assert!(is_commit(&comment));
        assert_eq!(
            find_file(&dir.join("good"), "MAIN.RS", 2),
            Some(dir.join("good").join("src").join("main.rs"))
        );
        assert!(dir.join("good/Cargo.toml").is_file());
        assert!(dir.join("good/src/main.rs").is_file());
        let evil = write("evil.zip", &["top/../../escaped.txt"]);
        assert!(
            run.extract("source", &evil, &dir.join("evil"), true)
                .is_err()
        );
        assert!(!dir.join("escaped.txt").exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
