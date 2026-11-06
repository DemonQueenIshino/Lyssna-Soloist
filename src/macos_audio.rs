//! Feed the existing mixer to AVFoundation so macOS can spatialize stereo.
use rodio::mixer::{Mixer, mixer};
use std::{
    ffi::{CString, c_char, c_void},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

unsafe extern "C" {
    fn lyssna_audio_create(name: *const c_char) -> *mut c_void;
    fn lyssna_audio_ready(output: *mut c_void) -> i32;
    fn lyssna_audio_enqueue(output: *mut c_void, samples: *const f32, frames: usize) -> bool;
    fn lyssna_audio_destroy(output: *mut c_void);
}

pub(crate) struct OutputStream {
    mixer: Mixer,
    stopped: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl OutputStream {
    pub(crate) fn open(
        name: Option<&str>,
        failed: impl FnOnce() + Send + 'static,
    ) -> Result<Self, String> {
        let name = name
            .map(CString::new)
            .transpose()
            .map_err(|error| error.to_string())?;
        let (mixer, mut source) = mixer(2, 44_100);
        let stopped = Arc::new(AtomicBool::new(false));
        let stopping = stopped.clone();
        let (send, receive) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("macOS audio".into())
            .spawn(move || {
                // The native renderer is created, used and released on this thread.
                let output = unsafe {
                    lyssna_audio_create(
                        name.as_ref().map_or(std::ptr::null(), |name| name.as_ptr()),
                    )
                };
                if output.is_null() {
                    let _ = send.send(false);
                    return;
                }
                if send.send(true).is_err() {
                    unsafe { lyssna_audio_destroy(output) };
                    return;
                }
                let mut samples = [0.0_f32; 882]; // 10 ms of interleaved stereo.
                let mut error = false;
                while !stopping.load(Ordering::Acquire) {
                    match unsafe { lyssna_audio_ready(output) } {
                        0 => thread::sleep(Duration::from_millis(2)),
                        1 => {
                            for sample in &mut samples {
                                *sample = source.next().unwrap_or(0.0);
                            }
                            if !unsafe {
                                lyssna_audio_enqueue(output, samples.as_ptr(), samples.len() / 2)
                            } {
                                error = true;
                                break;
                            }
                        }
                        _ => {
                            error = true;
                            break;
                        }
                    }
                }
                unsafe { lyssna_audio_destroy(output) };
                if error && !stopping.load(Ordering::Acquire) {
                    failed();
                }
            })
            .map_err(|error| error.to_string())?;
        if receive.recv().unwrap_or(false) {
            Ok(Self {
                mixer,
                stopped,
                worker: Some(worker),
            })
        } else {
            let _ = worker.join();
            Err("Cannot initialize AVFoundation audio output".into())
        }
    }

    pub(crate) fn mixer(&self) -> &Mixer {
        &self.mixer
    }
}

impl Drop for OutputStream {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
