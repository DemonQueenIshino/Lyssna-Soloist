//! Small, bounded audio queue. Seek acknowledgements discard previously decoded samples.
use librespot_playback::{
    audio_backend::{Sink, SinkError, SinkResult},
    convert::Converter,
    decoder::AudioPacket,
    mixer::{Mixer, NoOpVolume, VolumeGetter},
};
use rodio::{OutputStream, Source};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const CHANNELS: u16 = 2;
const SAMPLE_RATE: u32 = 44_100;
const MAX_SAMPLES: usize = SAMPLE_RATE as usize * CHANNELS as usize * 250 / 1000;
/// How long a playing output may take no samples before it counts as stuck.
const OUTPUT_STALL: Duration = Duration::from_secs(2);

#[derive(Clone, Default)]
pub struct AudioBuffer {
    pub(crate) output: crate::audio_output::OutputChoice,
    #[cfg(test)]
    output_started: Arc<Mutex<Option<std::time::Instant>>>,
    queued: Arc<AtomicUsize>,
    narration: Arc<AtomicBool>,
    generation: Arc<AtomicUsize>,
    target: Arc<Mutex<Option<String>>>,
    crossfade_ms: Arc<AtomicU32>,
    temporary_volume: Arc<AtomicU32>,
}
impl AudioBuffer {
    pub(crate) fn same_output(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.temporary_volume, &other.temporary_volume)
    }

    /// An external client's temporary output level; None follows the user's mixer.
    pub(crate) fn set_temporary_volume(&self, percent: Option<u8>) {
        self.temporary_volume.store(
            percent.map_or(0, |value| u32::from(value.min(100)) + 1),
            Ordering::Relaxed,
        );
    }
    /// How long one song fades into the next; 0 plays them back to back.
    pub fn set_crossfade(&self, seconds: u8) {
        self.crossfade_ms
            .store(u32::from(seconds.min(12)) * 1000, Ordering::Relaxed);
    }
    /// Silence old packets immediately, including while Connect is awaiting HTTP.
    pub fn interrupt(&self, target: Option<String>) {
        if let Ok(mut pending) = self.target.lock() {
            *pending = target;
            self.generation.fetch_add(1, Ordering::AcqRel);
            #[cfg(test)]
            if let Ok(mut started) = self.output_started.lock() {
                *started = None;
            }
            self.narration.store(false, Ordering::Release);
        }
    }
    #[cfg(test)]
    pub(crate) fn output_started(&self) -> Option<std::time::Instant> {
        *self.output_started.lock().ok()?
    }
    fn select_track(&self, uri: Option<&str>) -> Option<usize> {
        let mut target = self.target.lock().ok()?;
        if target.as_deref().is_some_and(|wanted| Some(wanted) != uri) {
            return None;
        }
        *target = None;
        Some(self.generation.load(Ordering::Acquire))
    }
    pub fn narrating(&self) -> bool {
        self.narration.load(Ordering::Acquire)
    }
    pub fn latency_ms(&self) -> u64 {
        (self.queued.load(Ordering::Relaxed) as u64) * 1000
            / (u64::from(SAMPLE_RATE) * u64::from(CHANNELS))
    }
}

pub struct AudioSink {
    buffer: AudioBuffer,
    mixer: Option<Arc<dyn Mixer>>,
    stream: Option<OutputStream>,
    applied_output: Option<u64>,
    sink: Option<rodio::Sink>,
    queued: Arc<AtomicUsize>,
    flush: Arc<AtomicBool>,
    seek_ready: tokio::sync::mpsc::UnboundedSender<()>,
    awaiting_seek: bool,
    generation: usize,
    track_uri: Option<String>,
    crossfade: Crossfade,
}
/// The end of one song, held back and mixed into the start of the next.
#[derive(Default)]
struct Crossfade {
    /// Time left in the song for the coming packet, when a next song is ready.
    remaining_ms: Option<u32>,
    /// Samples from the end of the current song, not yet played.
    tail: Vec<f32>,
    /// The previous song's held end, being mixed into this song.
    fading: Vec<f32>,
    mixed: usize,
}
impl Crossfade {
    fn clear(&mut self) {
        self.tail.clear();
        self.fading.clear();
        self.mixed = 0;
    }
    /// Equal-power mix: the old song falls as the new one rises, so the
    /// overall loudness holds steady through the overlap.
    fn mix(&mut self, samples: &mut [f32]) {
        let total = self.fading.len();
        for sample in samples {
            let Some(old) = self.fading.get(self.mixed) else {
                break;
            };
            let progress = self.mixed as f32 / total as f32 * std::f32::consts::FRAC_PI_2;
            *sample = *sample * progress.sin() + old * progress.cos();
            self.mixed += 1;
        }
        if self.mixed >= total {
            self.fading.clear();
            self.mixed = 0;
        }
    }
}
impl AudioSink {
    pub fn new(
        flush: Arc<AtomicBool>,
        seek_ready: tokio::sync::mpsc::UnboundedSender<()>,
        buffer: AudioBuffer,
    ) -> Self {
        Self {
            generation: buffer.generation.load(Ordering::Acquire),
            track_uri: None,
            crossfade: Crossfade::default(),
            buffer: buffer.clone(),
            mixer: None,
            stream: None,
            applied_output: None,
            sink: None,
            queued: buffer.queued,
            flush,
            seek_ready,
            awaiting_seek: false,
        }
    }
    pub fn with_mixer(mut self, mixer: Arc<dyn Mixer>) -> Self {
        self.mixer = Some(mixer);
        self
    }
    pub fn prepare_output(&mut self) -> SinkResult<()> {
        if let Some((stream, revision)) = self.buffer.output.open(self.applied_output)? {
            self.install_sink(rodio::Sink::connect_new(stream.mixer()), revision);
            self.stream = Some(stream);
            #[cfg(windows)]
            let _ = configure_session_identity();
        }
        Ok(())
    }
    fn install_sink(&mut self, sink: rodio::Sink, revision: u64) {
        if self.sink.as_ref().is_some_and(rodio::Sink::is_paused) {
            sink.pause();
        }
        self.sink = Some(sink);
        self.applied_output = Some(revision);
    }
    #[cfg(test)]
    pub(crate) fn test_output(&mut self) -> impl Source<Item = f32> + use<> {
        let (sink, output) = rodio::Sink::new();
        self.install_test_sink(sink);
        output
    }
    #[cfg(test)]
    fn install_test_sink(&mut self, sink: rodio::Sink) {
        self.install_sink(sink, self.buffer.output.revision());
        self.buffer.output.defer_default_probe();
    }
    fn discard_old_audio(&mut self) {
        if self.flush.swap(false, Ordering::AcqRel)
            && let Some(sink) = &self.sink
        {
            sink.stop();
            self.crossfade.clear();
            if let Some(generation) = self.buffer.select_track(self.track_uri.as_deref()) {
                self.generation = generation;
            }
            self.awaiting_seek = true;
        }
    }
}
impl Sink for AudioSink {
    fn set_track(&mut self, track_id: &librespot_core::SpotifyUri) {
        let uri = track_id.to_uri();
        let natural = match self.buffer.select_track(Some(&uri)) {
            Some(generation) => {
                let natural = self.generation == generation;
                if !natural && let Some(sink) = &self.sink {
                    sink.stop();
                }
                self.generation = generation;
                natural
            }
            None => false,
        };
        // Only a song running into the next crossfades; a chosen song
        // starts cleanly.
        self.crossfade.fading = if natural {
            std::mem::take(&mut self.crossfade.tail)
        } else {
            Vec::new()
        };
        self.crossfade.tail.clear();
        self.crossfade.mixed = 0;
        self.crossfade.remaining_ms = None;
        #[cfg(not(test))]
        if !self.crossfade.fading.is_empty() {
            let ms = self.crossfade.fading.len() as u64 * 1000
                / (u64::from(SAMPLE_RATE) * u64::from(CHANNELS));
            crate::native::diagnostic(&format!("event crossfade {ms}ms into {uri}"));
        }
        self.track_uri = Some(uri);
    }
    fn crossfade_hint(&mut self, remaining_ms: Option<u32>) {
        self.crossfade.remaining_ms = remaining_ms;
    }
    fn is_empty(&self) -> bool {
        // A held end counts as played: the next song can start while the
        // last moments before it are still sounding.
        self.queued.load(Ordering::Acquire) == 0 || !self.crossfade.tail.is_empty()
    }
    fn start(&mut self) -> SinkResult<()> {
        self.discard_old_audio();
        self.prepare_output()?;
        self.sink.as_ref().unwrap().play();
        Ok(())
    }
    fn stop(&mut self) -> SinkResult<()> {
        if let Some(sink) = &self.sink {
            sink.stop();
        }
        self.crossfade.clear();
        Ok(())
    }
    fn pause(&mut self) -> SinkResult<()> {
        if let Some(sink) = &self.sink {
            sink.pause();
        }
        Ok(())
    }
    fn write(&mut self, packet: AudioPacket, converter: &mut Converter) -> SinkResult<()> {
        self.prepare_output()?;
        let narration = packet.is_narration();
        self.discard_old_audio();
        let samples = packet
            .samples()
            .map_err(|_| SinkError::OnWrite("Cannot decode audio samples".into()))?;
        let mut samples = converter.f64_to_f32(samples).to_vec();
        let fade_ms = self.buffer.crossfade_ms.load(Ordering::Relaxed);
        if narration {
            // DJ speech never crossfades. A song's held end plays out whole
            // first, so it finishes before DJ talks, and never returns later
            // under the next song.
            let mut held = std::mem::take(&mut self.crossfade.tail);
            if !held.is_empty() {
                self.crossfade.mix(&mut held);
                self.append(held, false)?;
            }
            if !self.crossfade.fading.is_empty() {
                let rest = self.crossfade.fading.split_off(self.crossfade.mixed);
                self.crossfade.fading.clear();
                self.crossfade.mixed = 0;
                self.append(rest, false)?;
            }
        } else if fade_ms > 0
            && self
                .crossfade
                .remaining_ms
                .is_some_and(|remaining| remaining <= fade_ms)
        {
            self.crossfade.tail.extend_from_slice(&samples);
            if self.awaiting_seek {
                self.awaiting_seek = false;
                let _ = self.seek_ready.send(());
            }
            return Ok(());
        } else if !self.crossfade.fading.is_empty() {
            self.crossfade.mix(&mut samples);
        }
        self.append(samples, narration)
    }
}

impl AudioSink {
    fn append(&mut self, samples: Vec<f32>, narration: bool) -> SinkResult<()> {
        let sink = self
            .sink
            .as_ref()
            .ok_or_else(|| SinkError::NotConnected("Audio output not started".into()))?;
        let mut packet = QueuedPacket::with_output(
            samples,
            self.buffer.clone(),
            narration,
            self.mixer.as_ref().map_or_else(
                || Box::new(NoOpVolume) as Box<dyn VolumeGetter + Send>,
                |mixer| mixer.get_soft_volume(),
            ),
        );
        packet.generation = self.generation;
        sink.append(packet);
        if self.awaiting_seek {
            self.awaiting_seek = false;
            let _ = self.seek_ready.send(());
        }
        // An output that opens but never pulls samples (seen on a cold start)
        // would block the player thread for good. Fail the write instead:
        // the player pauses, and Play reopens the output.
        let mut drained = (self.queued.load(Ordering::Relaxed), Instant::now());
        while self.queued.load(Ordering::Relaxed) > MAX_SAMPLES {
            self.prepare_output()?;
            self.discard_old_audio();
            let queued = self.queued.load(Ordering::Relaxed);
            let paused = self.sink.as_ref().is_some_and(rodio::Sink::is_paused);
            if queued < drained.0 || paused {
                drained = (queued, Instant::now());
            } else if drained.1.elapsed() >= OUTPUT_STALL {
                if let Some(revision) = self.applied_output {
                    self.buffer.output.invalidate(revision);
                }
                return Err(SinkError::OnWrite("Audio output stopped responding".into()));
            }
            thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }
}

struct QueuedPacket {
    #[cfg(test)]
    output_started: Option<Arc<Mutex<Option<std::time::Instant>>>>,
    generation: usize,
    active_generation: Option<Arc<AtomicUsize>>,
    marker_applied: bool,
    marker: Option<(Arc<AtomicBool>, bool)>,
    volume: Box<dyn VolumeGetter + Send>,
    temporary_volume: Option<Arc<AtomicU32>>,
    samples: std::vec::IntoIter<f32>,
    queued: Arc<AtomicUsize>,
    duration: Duration,
}
impl QueuedPacket {
    fn with_output(
        samples: Vec<f32>,
        buffer: AudioBuffer,
        narration: bool,
        volume: Box<dyn VolumeGetter + Send>,
    ) -> Self {
        let mut packet = Self::new(samples, buffer.queued);
        packet.generation = buffer.generation.load(Ordering::Acquire);
        packet.active_generation = Some(buffer.generation);
        #[cfg(test)]
        {
            packet.output_started = Some(buffer.output_started);
        }
        packet.marker = Some((buffer.narration, narration));
        packet.volume = volume;
        packet.temporary_volume = Some(buffer.temporary_volume);
        packet
    }
    fn new(samples: Vec<f32>, queued: Arc<AtomicUsize>) -> Self {
        let duration = Duration::from_secs_f64(
            samples.len() as f64 / f64::from(SAMPLE_RATE) / f64::from(CHANNELS),
        );
        queued.fetch_add(samples.len(), Ordering::Relaxed);
        Self {
            #[cfg(test)]
            output_started: None,
            generation: 0,
            active_generation: None,
            marker: None,
            marker_applied: false,
            volume: Box::new(NoOpVolume),
            temporary_volume: None,
            samples: samples.into_iter(),
            queued,
            duration,
        }
    }
}
impl Iterator for QueuedPacket {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let sample = self.samples.next();
        let current = self
            .active_generation
            .as_ref()
            .is_none_or(|generation| generation.load(Ordering::Acquire) == self.generation);
        if sample.is_some() {
            if !self.marker_applied && current {
                #[cfg(test)]
                if let Some(started) = &self.output_started
                    && let Ok(mut started) = started.lock()
                {
                    started.get_or_insert_with(std::time::Instant::now);
                }
                if let Some((marker, narration)) = &self.marker {
                    marker.store(*narration, Ordering::Release);
                }
                self.marker_applied = true;
            }
            self.queued.fetch_sub(1, Ordering::Relaxed);
        }
        sample.map(|value| {
            if current {
                let temporary = self
                    .temporary_volume
                    .as_ref()
                    .map_or(0, |volume| volume.load(Ordering::Relaxed));
                let factor = if temporary == 0 {
                    self.volume.attenuation_factor() as f32
                } else {
                    (temporary - 1) as f32 / 100.0
                };
                value * factor
            } else {
                0.0
            }
        })
    }
}
impl Drop for QueuedPacket {
    fn drop(&mut self) {
        if self.queued.fetch_sub(self.samples.len(), Ordering::AcqRel) == self.samples.len()
            && let Some((marker, _)) = &self.marker
        {
            marker.store(false, Ordering::Release);
        }
    }
}
impl Source for QueuedPacket {
    fn current_span_len(&self) -> Option<usize> {
        Some(self.samples.len())
    }
    fn channels(&self) -> u16 {
        CHANNELS
    }
    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        Some(self.duration)
    }
}

#[cfg(windows)]
fn configure_session_identity() -> windows::core::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::{
            Foundation::RPC_E_CHANGED_MODE,
            Media::Audio::{
                DEVICE_STATE_ACTIVE, IAudioSessionControl2, IAudioSessionManager2,
                IMMDeviceEnumerator, MMDeviceEnumerator, eRender,
            },
            System::Com::{
                CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
            },
        },
        core::{Interface, PCWSTR, w},
    };
    struct Apartment(bool);
    impl Drop for Apartment {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }
    let Ok(executable) = std::env::current_exe() else {
        return Ok(());
    };
    let mut icon = executable.into_os_string();
    icon.push(",0");
    let icon: Vec<u16> = icon.encode_wide().chain(Some(0)).collect();
    // Rodio/CPAL may already have initialized this thread's COM apartment.
    let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if initialized != RPC_E_CHANGED_MODE {
        initialized.ok()?;
    }
    let _apartment = Apartment(initialized.is_ok());
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let endpoints = devices.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        for index in 0..endpoints.GetCount()? {
            let endpoint = endpoints.Item(index)?;
            let Ok(manager) = endpoint.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                continue;
            };
            let sessions = manager.GetSessionEnumerator()?;
            for index in 0..sessions.GetCount()? {
                let session: IAudioSessionControl2 = sessions.GetSession(index)?.cast()?;
                if session.GetProcessId()? == std::process::id() {
                    session.SetIconPath(PCWSTR(icon.as_ptr()), std::ptr::null())?;
                    session.SetDisplayName(w!("Lyssna"), std::ptr::null())?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn track(number: usize) -> librespot_core::SpotifyUri {
        librespot_core::SpotifyUri::from_uri(&format!("spotify:track:{number:022}")).unwrap()
    }

    #[test]
    fn rapid_selection_silences_old_and_late_decoders_until_the_requested_track() {
        let buffer = AudioBuffer::default();
        let (ready, _) = tokio::sync::mpsc::unbounded_channel();
        let mut audio = AudioSink::new(Arc::new(AtomicBool::new(false)), ready, buffer.clone());
        let mut output = audio.test_output();
        let mut converter = Converter::new(None);
        audio.set_track(&track(1));
        audio
            .write(AudioPacket::Samples(vec![0.25; 1000]), &mut converter)
            .unwrap();
        assert_eq!(output.next(), Some(0.25));
        buffer.interrupt(Some(track(2).to_uri()));
        buffer.interrupt(Some(track(3).to_uri()));
        audio.set_track(&track(2));
        audio
            .write(AudioPacket::Samples(vec![0.5; 1000]), &mut converter)
            .unwrap();
        assert!((0..2000).all(|_| matches!(output.next(), Some(0.0) | None)));
        audio.set_track(&track(3));
        audio
            .write(AudioPacket::Samples(vec![0.75; 1000]), &mut converter)
            .unwrap();
        assert!((0..2000).any(|_| output.next() == Some(0.75)));
    }

    #[test]
    fn natural_track_transitions_retain_gapless_audio() {
        let buffer = AudioBuffer::default();
        let (ready, _) = tokio::sync::mpsc::unbounded_channel();
        let mut audio = AudioSink::new(Arc::new(AtomicBool::new(false)), ready, buffer);
        let mut output = audio.test_output();
        let mut converter = Converter::new(None);
        audio.set_track(&track(1));
        audio
            .write(AudioPacket::Samples(vec![0.25; 1000]), &mut converter)
            .unwrap();
        assert_eq!(output.next(), Some(0.25));
        audio.set_track(&track(2));
        audio
            .write(AudioPacket::Samples(vec![0.75; 1000]), &mut converter)
            .unwrap();
        assert!((0..999).all(|_| output.next() == Some(0.25)));
        assert!((0..2000).any(|_| output.next() == Some(0.75)));
    }

    #[test]
    fn natural_transition_mixes_held_end_into_next_song() {
        let buffer = AudioBuffer::default();
        buffer.set_crossfade(1);
        let (ready, _) = tokio::sync::mpsc::unbounded_channel();
        let mut audio = AudioSink::new(Arc::new(AtomicBool::new(false)), ready, buffer);
        let mut output = audio.test_output();
        let mut converter = Converter::new(None);
        audio.set_track(&track(1));
        audio.crossfade_hint(Some(5000));
        audio
            .write(AudioPacket::Samples(vec![0.25; 4]), &mut converter)
            .unwrap();
        // The last second is held back, and counts as played.
        audio.crossfade_hint(Some(900));
        audio
            .write(AudioPacket::Samples(vec![0.5; 400]), &mut converter)
            .unwrap();
        assert!((0..4).all(|_| output.next() == Some(0.25)));
        assert!(audio.is_empty());
        audio.set_track(&track(2));
        audio
            .write(AudioPacket::Samples(vec![1.0; 800]), &mut converter)
            .unwrap();
        let mixed: Vec<f32> = (0..800).filter_map(|_| output.next()).collect();
        // Starts as the old song, rises to the new one, then plays it alone.
        assert!((mixed[0] - 0.5).abs() < 1e-6);
        assert!(mixed[399] > 0.9 && mixed[399] < 1.12);
        assert!(mixed[400..].iter().all(|sample| *sample == 1.0));
    }

    #[test]
    fn dj_speech_waits_for_the_songs_held_end_and_never_crossfades() {
        let buffer = AudioBuffer::default();
        buffer.set_crossfade(1);
        let (ready, _) = tokio::sync::mpsc::unbounded_channel();
        let mut audio = AudioSink::new(Arc::new(AtomicBool::new(false)), ready, buffer);
        let mut output = audio.test_output();
        let mut converter = Converter::new(None);
        audio.set_track(&track(1));
        audio.crossfade_hint(Some(900));
        audio
            .write(AudioPacket::Samples(vec![0.5; 400]), &mut converter)
            .unwrap();
        // DJ signs off after the song: its end plays first, then the line.
        audio
            .write(AudioPacket::Narration(vec![0.25; 400]), &mut converter)
            .unwrap();
        assert!((0..400).all(|_| output.next() == Some(0.5)));
        assert!((0..400).all(|_| output.next() == Some(0.25)));
        // Nothing of the old song is left to return under the next one.
        audio.set_track(&track(2));
        assert!(audio.crossfade.fading.is_empty());

        // A song held for a next track that DJ introduces ends before he talks.
        audio.crossfade_hint(Some(900));
        audio
            .write(AudioPacket::Samples(vec![0.5; 400]), &mut converter)
            .unwrap();
        audio.set_track(&track(3));
        audio
            .write(AudioPacket::Narration(vec![0.25; 400]), &mut converter)
            .unwrap();
        assert!((0..400).all(|_| output.next() == Some(0.5)));
        assert!((0..400).all(|_| output.next() == Some(0.25)));
    }

    #[test]
    fn chosen_song_drops_held_end_and_short_queue_never_holds() {
        let buffer = AudioBuffer::default();
        buffer.set_crossfade(1);
        let (ready, _) = tokio::sync::mpsc::unbounded_channel();
        let mut audio = AudioSink::new(Arc::new(AtomicBool::new(false)), ready, buffer.clone());
        let mut output = audio.test_output();
        let mut converter = Converter::new(None);
        audio.set_track(&track(1));
        // Without a next song ready the end plays normally.
        audio.crossfade_hint(None);
        audio
            .write(AudioPacket::Samples(vec![0.25; 1000]), &mut converter)
            .unwrap();
        assert_eq!(output.next(), Some(0.25));
        audio.crossfade_hint(Some(500));
        audio
            .write(AudioPacket::Samples(vec![0.5; 1000]), &mut converter)
            .unwrap();
        assert!(audio.is_empty());
        // The held end never reaches a chosen song.
        buffer.interrupt(Some(track(3).to_uri()));
        audio.set_track(&track(3));
        assert!(audio.crossfade.tail.is_empty());
        assert!(audio.crossfade.fading.is_empty());
    }

    #[test]
    fn previous_restarting_the_same_song_resumes_after_the_seek_flush() {
        let buffer = AudioBuffer::default();
        let flush = Arc::new(AtomicBool::new(false));
        let (ready, mut acknowledgments) = tokio::sync::mpsc::unbounded_channel();
        let mut audio = AudioSink::new(flush.clone(), ready, buffer.clone());
        let mut output = audio.test_output();
        let mut converter = Converter::new(None);
        audio.set_track(&track(1));
        audio
            .write(AudioPacket::Samples(vec![0.25; 1000]), &mut converter)
            .unwrap();
        assert_eq!(output.next(), Some(0.25));
        buffer.interrupt(None);
        assert_eq!(output.next(), Some(0.0));
        assert!((0..2000).all(|_| matches!(output.next(), Some(0.0) | None)));
        flush.store(true, Ordering::Release);
        audio
            .write(AudioPacket::Samples(vec![0.75; 1000]), &mut converter)
            .unwrap();
        assert!(acknowledgments.try_recv().is_ok());
        assert!((0..2000).any(|_| output.next() == Some(0.75)));
    }

    #[test]
    fn stale_seek_cannot_unmute_a_song_after_another_is_selected() {
        let buffer = AudioBuffer::default();
        let flush = Arc::new(AtomicBool::new(false));
        let (ready, _) = tokio::sync::mpsc::unbounded_channel();
        let mut audio = AudioSink::new(flush.clone(), ready, buffer.clone());
        let mut output = audio.test_output();
        let mut converter = Converter::new(None);
        audio.set_track(&track(1));
        audio
            .write(AudioPacket::Samples(vec![0.25; 1000]), &mut converter)
            .unwrap();
        assert_eq!(output.next(), Some(0.25));
        buffer.interrupt(Some(track(2).to_uri()));
        assert!((0..2000).all(|_| matches!(output.next(), Some(0.0) | None)));
        flush.store(true, Ordering::Release);
        audio
            .write(AudioPacket::Samples(vec![0.5; 1000]), &mut converter)
            .unwrap();
        assert!((0..2000).all(|_| matches!(output.next(), Some(0.0) | None)));
        audio.set_track(&track(2));
        audio
            .write(AudioPacket::Samples(vec![0.75; 1000]), &mut converter)
            .unwrap();
        assert!((0..2000).any(|_| output.next() == Some(0.75)));
    }

    #[test]
    fn pause_retains_ready_audio_until_resume_without_new_packets() {
        let buffer = AudioBuffer::default();
        let (ready, _) = tokio::sync::mpsc::unbounded_channel();
        let mut audio = AudioSink::new(Arc::new(AtomicBool::new(false)), ready, buffer.clone());
        let (sink, mut output) = rodio::Sink::new();
        sink.append(QueuedPacket::new(
            vec![0.25; MAX_SAMPLES],
            buffer.queued.clone(),
        ));
        audio.install_test_sink(sink);
        assert_eq!(output.next(), Some(0.25));
        audio.pause().unwrap();
        for _ in 0..4410 {
            output.next();
        }
        let held = buffer.queued.load(Ordering::Relaxed);
        assert!(
            held > 0,
            "Pausing discarded audio already ready for unpause"
        );
        for _ in 0..44100 {
            assert_eq!(output.next(), Some(0.0));
        }
        assert_eq!(buffer.queued.load(Ordering::Relaxed), held);
        audio.start().unwrap();
        assert!(
            (0..882).any(|_| output.next() == Some(0.25)),
            "Unpause waited for a new decoded packet"
        );
        audio.stop().unwrap();
        for _ in 0..4410 {
            output.next();
        }
        assert!(
            audio.is_empty(),
            "Stop must discard audio retained by Pause"
        );
    }
    #[test]
    fn seek_while_paused_clears_retained_audio_before_resuming() {
        let buffer = AudioBuffer::default();
        let flush = Arc::new(AtomicBool::new(false));
        let (ready, _) = tokio::sync::mpsc::unbounded_channel();
        let mut audio = AudioSink::new(flush.clone(), ready, buffer.clone());
        let (sink, mut output) = rodio::Sink::new();
        sink.append(QueuedPacket::new(
            vec![0.25; MAX_SAMPLES],
            buffer.queued.clone(),
        ));
        audio.install_test_sink(sink);
        audio.pause().unwrap();
        for _ in 0..4410 {
            output.next();
        }
        assert!(!audio.is_empty());
        flush.store(true, Ordering::Release);
        audio.start().unwrap();
        assert!(
            (0..4410).all(|_| output.next() != Some(0.25)),
            "Seek replayed audio from before the pause"
        );
        assert!(audio.is_empty());
    }
    #[test]
    fn eof_waits_for_last_spoken_sample() {
        let buffer = AudioBuffer::default();
        let (ready, _) = tokio::sync::mpsc::unbounded_channel();
        let sink = AudioSink::new(Arc::new(AtomicBool::new(false)), ready, buffer.clone());
        let mut speech =
            QueuedPacket::with_output(vec![0.5; 4], buffer, true, Box::new(NoOpVolume));
        assert!(!sink.is_empty());
        for _ in 0..3 {
            assert!(speech.next().is_some());
            assert!(!sink.is_empty());
        }
        assert!(speech.next().is_some());
        assert!(sink.is_empty());
    }
    #[test]
    fn narration_labels_follow_consumed_audio_not_decoding() {
        let buffer = AudioBuffer::default();
        let mut speech =
            QueuedPacket::with_output(vec![0.5; 8], buffer.clone(), true, Box::new(NoOpVolume));
        let mut music =
            QueuedPacket::with_output(vec![0.5; 8], buffer.clone(), false, Box::new(NoOpVolume));
        assert!(!buffer.narrating());
        speech.next();
        assert!(buffer.narrating());
        drop(speech);
        assert!(buffer.narrating());
        music.next();
        assert!(!buffer.narrating());
    }
    #[test]
    fn replacing_an_active_output_releases_old_audio_and_consumes_new_samples() {
        let buffer = AudioBuffer::default();
        let (ready, _) = tokio::sync::mpsc::unbounded_channel();
        let mut audio = AudioSink::new(Arc::new(AtomicBool::new(false)), ready, buffer.clone());
        let mut old_output = audio.test_output();
        audio
            .write(
                AudioPacket::Samples(vec![0.25; 64]),
                &mut Converter::new(None),
            )
            .unwrap();
        assert_eq!(old_output.next(), Some(0.25));
        buffer.output.request(Some("New output".into()));
        let mut new_output = audio.test_output();
        drop(old_output);
        assert_eq!(buffer.queued.load(Ordering::Acquire), 0);
        audio
            .write(
                AudioPacket::Samples(vec![0.75; 64]),
                &mut Converter::new(None),
            )
            .unwrap();
        assert_eq!(new_output.next(), Some(0.75));
    }
    #[test]
    fn replacing_a_paused_output_stays_silent_until_resume() {
        let buffer = AudioBuffer::default();
        let (ready, _) = tokio::sync::mpsc::unbounded_channel();
        let mut audio = AudioSink::new(Arc::new(AtomicBool::new(false)), ready, buffer.clone());
        let old_output = audio.test_output();
        audio.pause().unwrap();
        buffer.output.request(Some("New output".into()));
        let mut new_output = audio.test_output();
        drop(old_output);
        assert!(audio.sink.as_ref().unwrap().is_paused());
        audio
            .write(
                AudioPacket::Samples(vec![0.75; 64]),
                &mut Converter::new(None),
            )
            .unwrap();
        assert!((0..100).all(|_| new_output.next() == Some(0.0)));
        audio.start().unwrap();
        assert!((0..1000).any(|_| new_output.next() == Some(0.75)));
    }
    #[test]
    fn an_output_that_never_drains_fails_the_write_instead_of_hanging() {
        let buffer = AudioBuffer::default();
        let (sink, _output) = rodio::Sink::new();
        let (ready, _) = tokio::sync::mpsc::unbounded_channel();
        let mut audio = AudioSink::new(Arc::new(AtomicBool::new(false)), ready, buffer.clone());
        audio.install_test_sink(sink);
        let started = std::time::Instant::now();
        let result = audio.write(
            AudioPacket::Samples(vec![0.25; MAX_SAMPLES * 2]),
            &mut Converter::new(None),
        );
        assert!(result.is_err());
        assert!(started.elapsed() < OUTPUT_STALL + Duration::from_secs(1));
        assert_eq!(
            buffer.output.take_issue(),
            Some(crate::audio_output::OutputIssue::Unavailable)
        );
    }
    #[test]
    fn volume_changes_affect_already_buffered_audio() {
        use librespot_playback::{
            config::VolumeCtrl,
            mixer::{MixerConfig, softmixer::SoftMixer},
        };
        let mixer = Arc::new(
            SoftMixer::open(MixerConfig {
                volume_ctrl: VolumeCtrl::Linear,
                ..Default::default()
            })
            .unwrap(),
        );
        mixer.set_volume(u16::MAX);
        let (sink, mut output) = rodio::Sink::new();
        let (ready, _) = tokio::sync::mpsc::unbounded_channel();
        let mut audio = AudioSink::new(
            Arc::new(AtomicBool::new(false)),
            ready,
            AudioBuffer::default(),
        )
        .with_mixer(mixer.clone());
        audio.install_test_sink(sink);
        audio
            .write(
                AudioPacket::Samples(vec![0.5; MAX_SAMPLES]),
                &mut Converter::new(None),
            )
            .unwrap();
        assert_eq!(output.next(), Some(0.5));
        mixer.set_volume(0);
        assert_eq!(output.next(), Some(0.0));
        mixer.set_volume(u16::MAX);
        assert_eq!(output.next(), Some(0.5));
    }
    #[test]
    fn drained_speech_clears_narration_without_another_song() {
        let buffer = AudioBuffer::default();
        let mut speech =
            QueuedPacket::with_output(vec![0.5; 4], buffer.clone(), true, Box::new(NoOpVolume));
        speech.next();
        assert!(buffer.narrating());
        drop(speech);
        assert!(!buffer.narrating());
    }
    #[test]
    fn output_survives_a_short_producer_scheduling_gap() {
        let queued = Arc::new(AtomicUsize::new(0));
        let paused = Arc::new(AtomicBool::new(false));
        let (sink, mut output) = rodio::Sink::new();
        let (ready, _) = tokio::sync::mpsc::unbounded_channel();
        let mut audio = AudioSink::new(
            Arc::new(AtomicBool::new(false)),
            ready,
            AudioBuffer {
                queued: queued.clone(),
                ..Default::default()
            },
        );
        audio.install_test_sink(sink);
        let producer_paused = paused.clone();
        let producer = thread::spawn(move || {
            let mut converter = Converter::new(None);
            while !producer_paused.load(Ordering::Acquire) {
                audio
                    .write(
                        AudioPacket::Samples(vec![
                            0.25;
                            SAMPLE_RATE as usize * CHANNELS as usize / 100
                        ]),
                        &mut converter,
                    )
                    .unwrap();
            }
            // Keep the sink alive in the join result while output drains.
            audio
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while queued.load(Ordering::Acquire) < MAX_SAMPLES {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        paused.store(true, Ordering::Release);
        let samples = SAMPLE_RATE as usize * CHANNELS as usize * 150 / 1000;
        let gaps = (0..samples).filter(|_| output.next() != Some(0.25)).count();
        let _audio = producer.join().unwrap();
        assert_eq!(gaps, 0, "audio ran dry during a 150 ms scheduling gap");
    }
    #[test]
    fn seek_discards_old_samples_before_new_audio_without_waiting_for_playback() {
        let queued = Arc::new(AtomicUsize::new(0));
        let (sink, mut output) = rodio::Sink::new();
        sink.append(QueuedPacket::new(vec![0.25; MAX_SAMPLES], queued.clone()));
        assert_eq!(output.next(), Some(0.25));
        sink.stop();
        for _ in 0..1000 {
            output.next();
            if sink.empty() {
                break;
            }
        }
        assert!(sink.empty());
        assert_eq!(queued.load(Ordering::Relaxed), 0);
        sink.append(QueuedPacket::new(vec![0.75; 100], queued.clone()));
        assert!((0..1024).any(|_| output.next() == Some(0.75)));
    }
    #[test]
    fn queued_samples_account_for_playback_and_dropped_packets() {
        let queued = Arc::new(AtomicUsize::new(0));
        let mut first = QueuedPacket::new(vec![0.0; 100], queued.clone());
        let second = QueuedPacket::new(vec![0.0; 200], queued.clone());
        assert_eq!(queued.load(Ordering::Relaxed), 300);
        first.next();
        assert_eq!(queued.load(Ordering::Relaxed), 299);
        drop(first);
        assert_eq!(queued.load(Ordering::Relaxed), 200);
        drop(second);
        assert_eq!(queued.load(Ordering::Relaxed), 0);
    }
}
