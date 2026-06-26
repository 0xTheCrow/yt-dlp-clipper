use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use yt_dlp_clipper::decoder::Decoder;

pub(crate) enum DecodeRequest {
    Seek { secs: f64, gen: u64 },
    StepForward { gen: u64 },
    StepBackward { gen: u64 },
}

pub(crate) enum DecodeEvent {
    Opened { width: u32, height: u32, duration_secs: f64, fps: f64 },
    /// `gen` echoes the request that produced this frame, so the UI can ignore
    /// frames from superseded seeks (e.g. mid-drag decodes after a release).
    Frame { image: egui::ColorImage, secs: f64, gen: u64 },
    Error(String),
}

/// Owns the `Decoder` on a worker thread so decoding never blocks the UI. The
/// decoder is created and used entirely on that thread (it never crosses a
/// thread boundary); only frames and metadata are sent back.
pub(crate) struct DecoderHandle {
    req_tx: Sender<DecodeRequest>,
    pub(crate) event_rx: Receiver<DecodeEvent>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) duration_secs: f64,
    pub(crate) fps: f64,
    pub(crate) current_secs: f64,
    pub(crate) ready: bool,
    /// Monotonic request id; each request returns the gen it was tagged with.
    gen: AtomicU64,
}

impl DecoderHandle {
    pub(crate) fn spawn(path: String) -> Self {
        let (req_tx, req_rx) = channel();
        let (event_tx, event_rx) = channel();
        thread::spawn(move || decoder_loop(path, req_rx, event_tx));
        Self {
            req_tx,
            event_rx,
            width: 0,
            height: 0,
            duration_secs: 0.0,
            fps: 30.0,
            current_secs: 0.0,
            ready: false,
            gen: AtomicU64::new(1),
        }
    }

    fn next_gen(&self) -> u64 {
        self.gen.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn seek_secs(&self, secs: f64) -> u64 {
        let gen = self.next_gen();
        let _ = self.req_tx.send(DecodeRequest::Seek { secs, gen });
        gen
    }
    pub(crate) fn step_forward(&self) -> u64 {
        let gen = self.next_gen();
        let _ = self.req_tx.send(DecodeRequest::StepForward { gen });
        gen
    }
    pub(crate) fn step_backward(&self) -> u64 {
        let gen = self.next_gen();
        let _ = self.req_tx.send(DecodeRequest::StepBackward { gen });
        gen
    }
}

/// Open the file, emit metadata + first frame, then serve decode requests.
/// Pending requests are coalesced to the latest so a fast drag never backlogs.
fn decoder_loop(path: String, req_rx: Receiver<DecodeRequest>, event_tx: Sender<DecodeEvent>) {
    let mut dec = match Decoder::open(&path) {
        Ok(dec) => dec,
        Err(e) => {
            let _ = event_tx.send(DecodeEvent::Error(e.to_string()));
            return;
        }
    };
    let opened = DecodeEvent::Opened {
        width: dec.width,
        height: dec.height,
        duration_secs: dec.duration_secs(),
        fps: dec.fps(),
    };
    if event_tx.send(opened).is_err() {
        return;
    }
    // A successfully opened video always has a first frame; if decoding it
    // yields nothing, no decoder in this build can handle the codec (e.g. AV1
    // on a build without a software AV1 decoder), which otherwise shows only a
    // blank preview. Surface a named error instead.
    match dec.step_forward() {
        Some(image) => {
            let _ = event_tx.send(DecodeEvent::Frame { image, secs: dec.current_secs(), gen: 0 });
        }
        None => {
            let _ = event_tx.send(DecodeEvent::Error(format!(
                "Couldn't decode the video stream (codec {}). This build of \
                 yt-dlp-clipper has no decoder for it.",
                dec.codec_name()
            )));
            return;
        }
    }

    while let Ok(first) = req_rx.recv() {
        // Fold the first request and any already-queued ones. Seeks collapse to
        // the latest (a fast drag never backlogs); steps accumulate into a net
        // frame delta so rapid/held stepping isn't dropped — and a multi-frame
        // backward jump becomes a single seek rather than one seek per frame.
        let mut seek: Option<f64> = None;
        let mut steps: i64 = 0;
        let mut gen = 0;
        let mut req = Some(first);
        while let Some(r) = req {
            match r {
                DecodeRequest::Seek { secs, gen: g } => {
                    seek = Some(secs);
                    steps = 0;
                    gen = g;
                }
                DecodeRequest::StepForward { gen: g } => {
                    steps += 1;
                    gen = g;
                }
                DecodeRequest::StepBackward { gen: g } => {
                    steps -= 1;
                    gen = g;
                }
            }
            req = req_rx.try_recv().ok();
        }

        let mut image = None;
        if let Some(secs) = seek {
            image = dec.seek_secs(secs);
        }
        if steps != 0 {
            image = dec.step_by(steps);
        }
        if let Some(image) = image {
            if event_tx
                .send(DecodeEvent::Frame { image, secs: dec.current_secs(), gen })
                .is_err()
            {
                return;
            }
        }
    }
}
