use librespot_playback::audio_backend::{SinkError, SinkResult};
use rodio::{
    OutputStreamBuilder,
    cpal::{
        self,
        traits::{DeviceTrait, HostTrait},
    },
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const DEFAULT_PROBE_INTERVAL: Duration = Duration::from_millis(250);

pub(crate) enum OutputStream {
    Rodio(rodio::OutputStream),
    #[cfg(target_os = "macos")]
    Native(crate::macos_audio::OutputStream),
}

impl OutputStream {
    pub(crate) fn mixer(&self) -> &rodio::mixer::Mixer {
        match self {
            Self::Rodio(stream) => stream.mixer(),
            #[cfg(target_os = "macos")]
            Self::Native(stream) => stream.mixer(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum OutputIssue {
    UsingDefault,
    Unavailable,
}

#[derive(Default)]
struct Selection {
    name: Option<String>,
    revision: u64,
    issue: Option<OutputIssue>,
    reported: Option<u64>,
    default_endpoint: Option<Option<String>>,
    using_default: bool,
    next_probe: Option<Instant>,
}

#[derive(Clone, Default)]
pub(crate) struct OutputChoice(Arc<Mutex<Selection>>);

pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 512 && !name.chars().any(char::is_control)
}

#[cfg(any(feature = "desktop", test))]
pub(crate) fn devices() -> anyhow::Result<Vec<String>> {
    let mut names: Vec<_> = cpal::default_host()
        .output_devices()
        .map_err(|_| anyhow::anyhow!("Cannot list audio outputs"))?
        .filter_map(|device| device.name().ok())
        .filter(|name| valid_name(name))
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    Ok(names)
}

impl OutputChoice {
    #[cfg(test)]
    pub(crate) fn defer_default_probe(&self) {
        // Synthetic PCM sinks must never probe or open physical audio hardware.
        self.0.lock().unwrap().next_probe = Some(Instant::now() + Duration::from_secs(3600));
    }
    pub fn request(&self, name: Option<String>) {
        if let Ok(mut selection) = self.0.lock()
            && selection.name != name
        {
            selection.name = name;
            selection.revision = selection.revision.wrapping_add(1);
            selection.issue = None;
            selection.reported = None;
            selection.default_endpoint = None;
            selection.using_default = false;
            selection.next_probe = None;
        }
    }
    #[cfg(test)]
    pub fn revision(&self) -> u64 {
        self.0.lock().map_or(0, |selection| selection.revision)
    }
    pub fn take_issue(&self) -> Option<OutputIssue> {
        self.0.lock().ok()?.issue.take()
    }
    pub fn retire_stream(&self) {
        if let Ok(mut selection) = self.0.lock() {
            selection.revision = selection.revision.wrapping_add(1);
            selection.issue = None;
            selection.reported = None;
        }
    }
    fn report(&self, revision: u64, issue: OutputIssue) {
        if let Ok(mut selection) = self.0.lock()
            && selection.revision == revision
            && selection.reported != Some(revision)
        {
            selection.issue = Some(issue);
            selection.reported = Some(revision);
        }
    }
    pub(crate) fn invalidate(&self, revision: u64) {
        if let Ok(mut selection) = self.0.lock()
            && selection.revision == revision
        {
            selection.revision = selection.revision.wrapping_add(1);
            selection.issue = Some(OutputIssue::Unavailable);
            selection.reported = Some(selection.revision);
        }
    }
    fn retry_stream(&self, revision: u64) {
        if let Ok(mut selection) = self.0.lock()
            && selection.revision == revision
        {
            selection.revision = selection.revision.wrapping_add(1);
            selection.issue = None;
            selection.reported = None;
            selection.next_probe = None;
        }
    }
    fn stream(&self, name: Option<&str>, revision: u64) -> SinkResult<OutputStream> {
        #[cfg(target_os = "macos")]
        {
            let choice = self.clone();
            match crate::macos_audio::OutputStream::open(name, move || {
                choice.retry_stream(revision)
            }) {
                Ok(stream) => return Ok(OutputStream::Native(stream)),
                Err(error) => {
                    log::warn!("Native macOS audio unavailable: {error}; using CoreAudio output")
                }
            }
        }
        let builder = if let Some(name) = name {
            let device = cpal::default_host()
                .output_devices()
                .ok()
                .and_then(|mut devices| {
                    devices.find(|device| device.name().ok().as_deref() == Some(name))
                })
                .ok_or_else(|| {
                    SinkError::ConnectionRefused("Selected audio output unavailable".into())
                })?;
            OutputStreamBuilder::from_device(device)
        } else {
            OutputStreamBuilder::from_default_device()
        }
        .map_err(|_| SinkError::ConnectionRefused("Cannot configure audio output".into()))?;
        let choice = self.clone();
        let mut stream = builder
            .with_error_callback(move |_| choice.retry_stream(revision))
            .open_stream_or_fallback()
            .map_err(|_| SinkError::ConnectionRefused("Cannot open audio output".into()))?;
        stream.log_on_drop(false);
        Ok(OutputStream::Rodio(stream))
    }
    pub fn open(&self, applied: Option<u64>) -> SinkResult<Option<(OutputStream, u64)>> {
        self.open_with(
            applied,
            Instant::now(),
            default_output_id,
            |name, revision| self.stream(name, revision),
        )
    }
    fn observe_default(&self, now: Instant, endpoint: impl FnOnce() -> Option<String>) {
        let revision = {
            let Ok(selection) = self.0.lock() else {
                return;
            };
            if (selection.name.is_some() && !selection.using_default)
                || selection.next_probe.is_some_and(|next| now < next)
            {
                return;
            }
            selection.revision
        };
        let endpoint = endpoint();
        if let Ok(mut selection) = self.0.lock()
            && selection.revision == revision
        {
            selection.next_probe = Some(now + DEFAULT_PROBE_INTERVAL);
            if selection
                .default_endpoint
                .as_ref()
                .is_some_and(|previous| previous != &endpoint)
            {
                selection.revision = selection.revision.wrapping_add(1);
                selection.issue = None;
                selection.reported = None;
            }
            selection.default_endpoint = Some(endpoint);
        }
    }
    fn open_with<T>(
        &self,
        applied: Option<u64>,
        now: Instant,
        mut endpoint: impl FnMut() -> Option<String>,
        mut open: impl FnMut(Option<&str>, u64) -> SinkResult<T>,
    ) -> SinkResult<Option<(T, u64)>> {
        let mut attempts = 0;
        loop {
            self.observe_default(now, &mut endpoint);
            let (name, revision) = {
                let selection = self
                    .0
                    .lock()
                    .map_err(|_| SinkError::ConnectionRefused("Audio output unavailable".into()))?;
                if applied == Some(selection.revision) {
                    return Ok(None);
                }
                (selection.name.clone(), selection.revision)
            };
            attempts += 1;
            if attempts > 8 {
                self.report(revision, OutputIssue::Unavailable);
                return Err(SinkError::ConnectionRefused(
                    "Audio output could not stabilize".into(),
                ));
            }
            let mut using_default = name.is_none();
            let stream = match open(name.as_deref(), revision) {
                Ok(stream) => stream,
                Err(_) if name.is_some() => match open(None, revision) {
                    Ok(stream) => {
                        using_default = true;
                        self.report(revision, OutputIssue::UsingDefault);
                        stream
                    }
                    Err(error) => {
                        self.report(revision, OutputIssue::Unavailable);
                        return Err(error);
                    }
                },
                Err(error) => {
                    self.report(revision, OutputIssue::Unavailable);
                    return Err(error);
                }
            };
            {
                let mut selection = self
                    .0
                    .lock()
                    .map_err(|_| SinkError::ConnectionRefused("Audio output unavailable".into()))?;
                if selection.revision != revision {
                    if selection.issue == Some(OutputIssue::Unavailable) {
                        return Err(SinkError::ConnectionRefused(
                            "Audio output disconnected".into(),
                        ));
                    }
                    continue;
                }
                selection.using_default = using_default;
            }
            // A missing named device may have opened the default as fallback.
            self.observe_default(now, &mut endpoint);
            return Ok(Some((stream, revision)));
        }
    }
}

#[cfg(windows)]
fn default_output_id() -> Option<String> {
    use windows::Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender},
        System::Com::{
            CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
            CoUninitialize,
        },
    };
    struct Apartment(bool);
    impl Drop for Apartment {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }
    let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if initialized != RPC_E_CHANGED_MODE {
        initialized.ok().ok()?;
    }
    let _apartment = Apartment(initialized.is_ok());
    unsafe {
        let devices: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = devices.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        let id = device.GetId().ok()?;
        let value = id.to_string().ok();
        CoTaskMemFree(Some(id.0.cast()));
        value
    }
}

#[cfg(not(windows))]
fn default_output_id() -> Option<String> {
    cpal::default_host().default_output_device()?.name().ok()
}

/// Quiet, bounded output check. This never changes the saved device choice.
#[cfg(any(feature = "desktop", test))]
pub(crate) fn test_sound(name: Option<String>) -> anyhow::Result<()> {
    use rodio::Source;
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        time::{Duration, Instant},
    };
    static TESTING: AtomicBool = AtomicBool::new(false);
    anyhow::ensure!(
        TESTING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok(),
        "An audio output test is already playing"
    );
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            TESTING.store(false, Ordering::Release);
        }
    }
    let _reset = Reset;
    anyhow::ensure!(
        name.as_ref().is_none_or(|name| valid_name(name)),
        "Invalid audio output"
    );
    let choice = OutputChoice::default();
    let stream = choice.stream(name.as_deref(), 0).map_err(|_| {
        anyhow::anyhow!(
            "Cannot test the selected audio output. Refresh outputs and choose an available device."
        )
    })?;
    let sink = rodio::Sink::connect_new(stream.mixer());
    for frequency in [440.0, 660.0] {
        sink.append(
            rodio::source::SineWave::new(frequency)
                .take_duration(Duration::from_millis(350))
                .amplify(0.04)
                .delay(Duration::from_millis(400)),
        );
    }
    let started = Instant::now();
    while !sink.empty() && started.elapsed() < Duration::from_secs(3) {
        anyhow::ensure!(
            choice.take_issue() != Some(OutputIssue::Unavailable),
            "Audio output disconnected during the test"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    anyhow::ensure!(sink.empty(), "Audio output did not consume the test sound");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_endpoint_change_reopens_active_playback() {
        let choice = OutputChoice::default();
        let now = Instant::now();
        let (_, first) = choice
            .open_with(None, now, || Some("speakers-id".into()), |_, _| Ok(()))
            .unwrap()
            .unwrap();
        let unchanged = choice
            .open_with(
                Some(first),
                now + DEFAULT_PROBE_INTERVAL,
                || Some("speakers-id".into()),
                |_, _| Ok(()),
            )
            .unwrap();
        assert!(unchanged.is_none());
        let switched = choice
            .open_with(
                Some(first),
                now + DEFAULT_PROBE_INTERVAL * 2,
                || Some("headphones-id".into()),
                |_, _| Ok(()),
            )
            .unwrap();
        assert!(
            switched.is_some(),
            "Playback kept the cached stream after the Windows default endpoint changed"
        );
        assert_eq!(choice.take_issue(), None);
    }
    #[test]
    fn a_driver_reset_reopens_before_reporting_output_unavailable() {
        let choice = OutputChoice::default();
        let now = Instant::now();
        let (_, first) = choice
            .open_with(None, now, || Some("speakers-id".into()), |_, _| Ok(()))
            .unwrap()
            .unwrap();
        choice.retry_stream(first);
        assert_eq!(choice.take_issue(), None);
        let (_, replacement) = choice
            .open_with(
                Some(first),
                now,
                || Some("headphones-id".into()),
                |_, _| Ok(()),
            )
            .unwrap()
            .unwrap();
        assert_ne!(replacement, first);
        choice.retry_stream(first);
        assert_eq!(
            choice.revision(),
            replacement,
            "Retired callbacks must not invalidate the new device"
        );
        assert_eq!(choice.take_issue(), None);
    }
    #[test]
    fn named_outputs_stay_pinned_but_default_fallback_follows_windows() {
        let now = Instant::now();
        for missing in [false, true] {
            let choice = OutputChoice::default();
            choice.request(Some("Headphones".into()));
            let open = |name: Option<&str>, _| {
                if missing && name.is_some() {
                    Err(SinkError::ConnectionRefused(
                        "Test device disconnected".into(),
                    ))
                } else {
                    Ok(())
                }
            };
            let (_, first) = choice
                .open_with(None, now, || Some("first-id".into()), open)
                .unwrap()
                .unwrap();
            choice.take_issue();
            let next = choice
                .open_with(
                    Some(first),
                    now + DEFAULT_PROBE_INTERVAL,
                    || Some("second-id".into()),
                    open,
                )
                .unwrap();
            assert_eq!(next.is_some(), missing);
        }
    }
    #[test]
    fn repeatedly_failing_streams_stop_reopening_and_report_failure() {
        let choice = OutputChoice::default();
        let mut opens = 0;
        let result = choice.open_with(
            None,
            Instant::now(),
            || Some("speakers-id".into()),
            |_, revision| {
                opens += 1;
                choice.retry_stream(revision);
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(opens, 8);
        assert_eq!(choice.take_issue(), Some(OutputIssue::Unavailable));
    }
    #[test]
    #[ignore = "Plays two quiet tones on the saved output"]
    fn live_saved_output_test_sound() {
        let settings = crate::config::Settings::load().unwrap();
        test_sound(settings.audio_output).unwrap();
        println!("Two quiet tones consumed by the selected output");
    }
    #[test]
    fn late_output_failures_cannot_replace_a_newer_selection() {
        let choice = OutputChoice::default();
        let old = choice.revision();
        choice.request(Some("Headphones".into()));
        choice.report(old, OutputIssue::Unavailable);
        choice.invalidate(old);
        assert_eq!(choice.revision(), old + 1);
        assert_eq!(choice.take_issue(), None);
        choice.report(choice.revision(), OutputIssue::UsingDefault);
        assert_eq!(choice.take_issue(), Some(OutputIssue::UsingDefault));
        choice.report(choice.revision(), OutputIssue::UsingDefault);
        assert_eq!(choice.take_issue(), None);
    }
    #[test]
    fn a_disconnected_stream_can_be_reopened_without_changing_the_preference() {
        let choice = OutputChoice::default();
        let old = choice.revision();
        choice.invalidate(old);
        assert_ne!(old, choice.revision());
        assert_eq!(choice.take_issue(), Some(OutputIssue::Unavailable));
        assert!(choice.0.lock().unwrap().name.is_none());
    }
    #[test]
    fn callbacks_from_a_retired_connection_cannot_pause_its_replacement() {
        let choice = OutputChoice::default();
        choice.request(Some("Headphones".into()));
        let old = choice.revision();
        choice.retire_stream();
        choice.invalidate(old);
        assert_eq!(choice.take_issue(), None);
        assert_eq!(choice.0.lock().unwrap().name.as_deref(), Some("Headphones"));
    }
    #[test]
    #[ignore = "Opens native audio outputs without playing audio"]
    fn live_output_selection_and_missing_device_fallback() {
        let available = devices().unwrap();
        assert!(
            !available.is_empty(),
            "Connect an audio output before testing"
        );
        let choice = OutputChoice::default();
        let default = cpal::default_host()
            .default_output_device()
            .and_then(|device| device.name().ok())
            .expect("System default output required");
        assert!(available.contains(&default));
        choice.request(Some(default));
        let (stream, revision) = choice.open(None).unwrap().unwrap();
        assert_eq!(choice.take_issue(), None);
        assert!(choice.open(Some(revision)).unwrap().is_none());
        drop(stream);
        choice.request(Some("Lyssna unavailable test output".into()));
        let (stream, _) = choice.open(None).unwrap().unwrap();
        assert_eq!(choice.take_issue(), Some(OutputIssue::UsingDefault));
        drop(stream);
    }
}
