//! Bounded source analysis using the player's normal loader and decoder, without an output sink.
use super::{AudioPacket, Error, PlayerConfig, PlayerTrackLoader, Session, SpotifyUri};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

static ANALYSIS: Mutex<()> = Mutex::new(());

pub struct AudioEnvelope {
    pub start_ms: u32,
    pub step_ms: u32,
    pub duration_ms: u32,
    pub rms: Vec<f32>,
}

/// Decode at most 90 seconds of a track to 20 ms RMS windows. Never changes playback or its queue.
pub fn audio_envelope(session: Session, uri: SpotifyUri) -> Result<AudioEnvelope, Error> {
    let _analysis = ANALYSIS
        .try_lock()
        .map_err(|_| Error::unavailable("Another source audio analysis is in progress"))?;
    if !matches!(uri, SpotifyUri::Track { .. }) {
        return Err(Error::invalid_argument("Source analysis requires a track"));
    }
    // Decoder reads can wait for streamed bytes; keep the network driven on separate workers.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let loader = PlayerTrackLoader {
            load_cache: Arc::default(),
            session,
            config: PlayerConfig::default(),
            local_file_lookup: Arc::default(),
            normalise_as_album: false,
        };
        let deadline = Instant::now() + Duration::from_secs(45);
        let mut loaded = loader
            .load_track(uri, 0, None)
            .await
            .ok_or_else(|| Error::unavailable("Cannot load source audio for analysis"))?;
        const STEP_SAMPLES: usize = crate::SAMPLES_PER_SECOND as usize / 50;
        const MAX_WINDOWS: usize = 90 * 50;
        let mut rms = Vec::with_capacity(MAX_WINDOWS);
        let mut start_ms = None;
        let mut sum = 0.0;
        let mut count = 0;
        while rms.len() < MAX_WINDOWS {
            if Instant::now() > deadline {
                return Err(Error::deadline_exceeded("Source audio analysis timed out"));
            }
            let Some((position, packet)) = loaded.decoder.next_packet()? else {
                break;
            };
            if position.skipped {
                return Err(Error::unavailable("Source audio contains a decoding gap"));
            }
            let AudioPacket::Samples(samples) = packet else {
                continue;
            };
            start_ms.get_or_insert(position.position_ms);
            for sample in samples {
                if !sample.is_finite() {
                    return Err(Error::unavailable("Source audio contains invalid samples"));
                }
                sum += sample * sample;
                count += 1;
                if count == STEP_SAMPLES {
                    rms.push((sum / count as f64).sqrt() as f32);
                    sum = 0.0;
                    count = 0;
                    if rms.len() == MAX_WINDOWS {
                        break;
                    }
                }
            }
        }
        if rms.len() < 400 {
            return Err(Error::unavailable(
                "Source audio is shorter than eight seconds",
            ));
        }
        Ok(AudioEnvelope {
            start_ms: start_ms.unwrap_or(0),
            step_ms: 20,
            duration_ms: loaded.duration_ms,
            rms,
        })
    })
}
