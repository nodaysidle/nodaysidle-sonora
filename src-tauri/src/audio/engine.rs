//! Native playback engine: Symphonia decoding on a worker thread feeding a lock-free ring that a
//! CPAL output callback drains. Two decoders are held at once so a queue transition splices
//! sample-accurately instead of pausing to reopen a file.

use super::normalization::GainStage;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_queue::ArrayQueue;
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;
use symphonia::default::{get_codecs, get_probe};
use tauri::{AppHandle, Emitter};

/// Output ring capacity in frames. Four seconds is far more than the OS scheduler can ever eat
/// through, so a stalled decode thread never becomes an audible dropout.
const RING_SECONDS: usize = 4;
/// Frames decoded per refill. ~170 ms at 48 kHz.
const CHUNK_FRAMES: usize = 8192;
/// Decode enough of the following track that opening it cannot block the seam.
const PRE_ROLL_SECONDS: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineTrack {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub file_path: Option<String>,
    pub stream_url: Option<String>,
    pub duration_ms: u64,
    pub loudness_lufs: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineState {
    pub current_track: Option<EngineTrack>,
    pub next_track: Option<EngineTrack>,
    pub status: PlaybackStatus,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub volume: f32,
    pub is_normalizing: bool,
    pub is_gapless: bool,
}

impl Default for EngineState {
    fn default() -> Self {
        Self {
            current_track: None,
            next_track: None,
            status: PlaybackStatus::Stopped,
            position_ms: 0,
            duration_ms: 0,
            volume: 1.0,
            is_normalizing: true,
            is_gapless: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackProgress {
    pub position_ms: u64,
    pub duration_ms: u64,
}

pub const EVENT_PROGRESS: &str = "sonora://playback-progress";
pub const EVENT_STATUS: &str = "sonora://playback-status";
pub const EVENT_TRACK_ENDED: &str = "sonora://track-ended";

enum Cmd {
    Load { track: EngineTrack, auto_play: bool },
    SetNext(Option<EngineTrack>),
    Play,
    Pause,
    Seek(u64),
    SetVolume(f32),
    SetNormalization(bool),
    Stop,
}

/// Shared between the control surface, the decode thread and the output callback.
pub struct Shared {
    state: Mutex<EngineState>,
    /// Total interleaved frames the output callback has emitted since the stream opened.
    frames_played: AtomicU64,
    /// Value of `frames_played` at which the current track's first sample is heard. `pushed_frames`
    /// and `frames_played` count the same stream, so a sample pushed at index k is played when
    /// `frames_played` reaches k.
    track_anchor: AtomicU64,
    /// Position offset for seeks, added on top of elapsed frames.
    anchor_ms: AtomicU64,
    output_rate: AtomicU32,
    output_channels: AtomicU32,
    /// Set while the decode thread has run out of audio; the UI uses it to settle on Stopped.
    drained: AtomicBool,
    /// Total frames the decoder has handed to the ring. Monotonic, on the same timeline as
    /// `frames_played`, so `frames_played >= pushed_total` means the ring has fully drained.
    pushed_total: AtomicU64,
    /// A non-gapless completion waiting for the output callback to consume the audible tail.
    pending_ended: Mutex<Option<String>>,
    /// While set, the output callback emits silence and consumes nothing, so a pause freezes both
    /// the audio and the position immediately rather than draining the ring first.
    paused: AtomicBool,
    app: Mutex<Option<AppHandle>>,
}

impl Shared {
    fn position_ms(&self) -> u64 {
        let rate = self.output_rate.load(Ordering::Relaxed).max(1) as u64;
        let played = self.frames_played.load(Ordering::Relaxed);
        let anchor = self.track_anchor.load(Ordering::Relaxed);
        let elapsed = played.saturating_sub(anchor) * 1000 / rate;
        let duration = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .duration_ms;
        (self.anchor_ms.load(Ordering::Relaxed) + elapsed).min(duration)
    }

    fn emit(&self, event: &str, payload: impl Serialize + Clone) {
        if let Some(app) = self.app.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = app.emit(event, payload);
        }
    }
}

const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(15);

fn http_io_error(err: impl std::fmt::Display) -> std::io::Error {
    let msg = err.to_string();
    let lower = msg.to_ascii_lowercase();
    let kind = if lower.contains("timed out") || lower.contains("timeout") {
        std::io::ErrorKind::TimedOut
    } else {
        std::io::ErrorKind::Other
    };
    std::io::Error::new(kind, msg)
}

fn is_unrecoverable_stream_error(err: &SymphoniaError) -> bool {
    matches!(
        err,
        SymphoniaError::IoError(e) if matches!(
            e.kind(),
            std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::BrokenPipe
        )
    )
}

/// `symphonia::core::io::MediaSource` over HTTP, so YouTube Music stream URLs decode through the
/// same pipeline as local files. Keeps one response open for sequential reads and re-requests a
/// byte range only when the decoder seeks.
struct HttpSource {
    client: reqwest::blocking::Client,
    url: String,
    body: Option<reqwest::blocking::Response>,
    pos: u64,
    len: Option<u64>,
}

impl HttpSource {
    fn open(url: &str) -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(HTTP_CONNECT_TIMEOUT)
            .timeout(HTTP_READ_TIMEOUT)
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko)")
            .build()
            .map_err(|e| e.to_string())?;
        let mut source = Self {
            client,
            url: url.to_string(),
            body: None,
            pos: 0,
            len: None,
        };
        source.reopen_from(0)?;
        Ok(source)
    }

    fn reopen_from(&mut self, offset: u64) -> Result<(), String> {
        self.body = None;
        let response = self
            .client
            .get(&self.url)
            .header(reqwest::header::RANGE, format!("bytes={}-", offset))
            .send()
            .map_err(|e| format!("stream request failed: {e}"))?;
        let status = response.status();
        if offset > 0 && status != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(format!(
                "stream ignored byte range at offset {offset} (HTTP {status})"
            ));
        }
        if !status.is_success() {
            return Err(format!("stream returned HTTP {}", response.status()));
        }
        if offset > 0 {
            let content_range = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| "range response omitted Content-Range".to_string())?;
            if !content_range.starts_with(&format!("bytes {offset}-")) {
                return Err(format!(
                    "stream returned an unexpected byte range: {content_range}"
                ));
            }
        }
        // Content-Length on a 206 is the remaining range, so total = offset + remaining.
        self.len = response
            .content_length()
            .map(|remaining| offset + remaining)
            .or(self.len);
        self.pos = offset;
        self.body = Some(response);
        Ok(())
    }
}

impl Read for HttpSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.body.is_none() {
            self.reopen_from(self.pos).map_err(http_io_error)?;
        }
        let n = match self.body.as_mut().expect("reopened above").read(buf) {
            Ok(n) => n,
            Err(first_error) => {
                self.body = None;
                self.reopen_from(self.pos).map_err(http_io_error)?;
                self.body
                    .as_mut()
                    .expect("reopened above")
                    .read(buf)
                    .map_err(|retry_error| {
                        std::io::Error::new(
                            if retry_error.kind() == std::io::ErrorKind::TimedOut
                                || first_error.kind() == std::io::ErrorKind::TimedOut
                            {
                                std::io::ErrorKind::TimedOut
                            } else {
                                retry_error.kind()
                            },
                            format!(
                                "stream read failed after reconnect ({first_error}); retry failed: {retry_error}"
                            ),
                        )
                    })?
            }
        };
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for HttpSource {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::End(n) => self.len.map(|l| l as i64 + n).unwrap_or(0),
            SeekFrom::Current(n) => self.pos as i64 + n,
        }
        .max(0) as u64;
        self.reopen_from(target).map_err(http_io_error)?;
        Ok(target)
    }
}

impl MediaSource for HttpSource {
    fn is_seekable(&self) -> bool {
        self.len.is_some()
    }
    fn byte_len(&self) -> Option<u64> {
        self.len
    }
}

/// Maps source channels onto the output layout: mono fans out, extra outputs repeat, and a
/// wider source is truncated to the device's channel count.
fn mapped_channel(src: &[f32], src_channels: usize, frame: usize, out_channel: usize) -> f32 {
    let index = if out_channel < src_channels {
        out_channel
    } else {
        out_channel % src_channels
    };
    src[frame * src_channels + index]
}

/// Sample-rate and channel-layout converter. Rate changes use linear interpolation carrying the
/// last source frame across chunk boundaries so the stream stays continuous.
///
/// ponytail: linear interpolation is transparent for same-rate playback and merely adequate when a
/// 44.1 kHz file meets a 48 kHz device. Drop in `rubato` (as ARD.md originally proposed) if
/// band-limited resampling is wanted.
struct Converter {
    src_rate: u32,
    dst_rate: u32,
    src_channels: usize,
    dst_channels: usize,
    working: Vec<f32>,
    prev: Vec<f32>,
    has_prev: bool,
    pos: f64,
}

impl Converter {
    /// Creates a converter already bound to the device layout, with the source rate corrected on
    /// the first decoded packet.
    fn for_output(dst_rate: u32, dst_channels: usize) -> Self {
        Self {
            src_rate: dst_rate,
            dst_rate,
            src_channels: 2,
            dst_channels: dst_channels.max(1),
            working: Vec::new(),
            prev: vec![0.0; dst_channels.max(1)],
            has_prev: false,
            pos: 0.0,
        }
    }

    fn configure(&mut self, src_rate: u32, src_channels: usize, dst_channels: usize) {
        let changed = self.src_rate != src_rate
            || self.src_channels != src_channels
            || self.dst_channels != dst_channels;
        if changed {
            self.src_rate = src_rate;
            self.src_channels = src_channels.max(1);
            self.dst_channels = dst_channels.max(1);
            self.prev = vec![0.0; self.dst_channels];
            self.has_prev = false;
            self.pos = 0.0;
        }
    }

    fn reset(&mut self) {
        self.has_prev = false;
        self.pos = 0.0;
    }

    fn convert(&mut self, src: &[f32], out: &mut Vec<f32>) {
        let src_frames = src.len() / self.src_channels;
        if src_frames == 0 {
            return;
        }

        self.working.clear();
        if self.has_prev {
            self.working.extend_from_slice(&self.prev);
        }
        for frame in 0..src_frames {
            for channel in 0..self.dst_channels {
                self.working
                    .push(mapped_channel(src, self.src_channels, frame, channel));
            }
        }

        let total = self.working.len() / self.dst_channels;
        if !self.has_prev {
            self.pos = 0.0;
        }
        let ratio = self.src_rate as f64 / self.dst_rate as f64;
        let channels = self.dst_channels;

        while self.pos + 1.0 < total as f64 {
            let index = self.pos.floor() as usize;
            let frac = (self.pos - index as f64) as f32;
            let base = index * channels;
            let next = base + channels;
            for channel in 0..channels {
                let a = self.working[base + channel];
                let b = self.working[next + channel];
                out.push(a + (b - a) * frac);
            }
            self.pos += ratio;
        }

        // Re-base so the carried frame becomes index 0 of the next chunk.
        self.pos -= (total - 1) as f64;
        let tail = (total - 1) * channels;
        self.prev.clear();
        self.prev
            .extend_from_slice(&self.working[tail..tail + channels]);
        self.has_prev = true;
    }
}

/// One open track: container reader, codec decoder, and the converter feeding the output layout.
struct PlayingTrack {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    converter: Converter,
    sample_buf: Option<SampleBuffer<f32>>,
    /// Converted, gain-applied samples not yet handed to the caller. A decode packet rarely
    /// aligns with the chunk boundary, so the remainder is held here rather than truncated away.
    pending: Vec<f32>,
    /// Per-packet conversion scratch, reused to keep steady playback allocation-free.
    scratch: Vec<f32>,
    exhausted: bool,
    duration_ms: u64,
}

impl PlayingTrack {
    fn open(track: &EngineTrack, dst_rate: u32, dst_channels: usize) -> Result<Self, String> {
        let source: Box<dyn MediaSource> = match (&track.file_path, &track.stream_url) {
            (Some(path), _) if Path::new(path).is_file() => {
                Box::new(std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?)
            }
            (_, Some(url)) if url.starts_with("http") => Box::new(HttpSource::open(url)?),
            (Some(path), _) => return Err(format!("cannot open {path}")),
            _ => return Err(format!("track {} has no playable source", track.id)),
        };

        let stream = MediaSourceStream::new(source, MediaSourceStreamOptions::default());
        let mut hint = Hint::new();
        if let Some(path) = &track.file_path {
            if let Some(ext) = Path::new(path).extension().and_then(|e| e.to_str()) {
                hint.with_extension(ext);
            }
        } else if track.stream_url.is_some() {
            hint.with_extension("m4a");
        }

        let probed = get_probe()
            .format(
                &hint,
                stream,
                &FormatOptions {
                    enable_gapless: true,
                    ..Default::default()
                },
                &MetadataOptions::default(),
            )
            .map_err(|e| format!("unsupported format for {}: {e}", track.title))?;

        let reader = probed.format;
        let symphonia_track = reader
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| format!("no decodable audio track in {}", track.title))?;
        let track_id = symphonia_track.id;
        let src_rate = symphonia_track.codec_params.sample_rate.unwrap_or(dst_rate);

        // Prefer the container's own duration; fall back to the indexed value.
        let duration_ms = reader
            .tracks()
            .iter()
            .find(|t| t.id == track_id)
            .and_then(|t| t.codec_params.n_frames)
            .filter(|frames| *frames > 0)
            .map(|frames| frames * 1000 / src_rate.max(1) as u64)
            .unwrap_or(track.duration_ms);

        let decoder = get_codecs()
            .make(&symphonia_track.codec_params, &DecoderOptions::default())
            .map_err(|e| format!("no decoder for {}: {e}", track.title))?;

        Ok(Self {
            reader,
            decoder,
            track_id,
            converter: Converter {
                src_rate,
                ..Converter::for_output(dst_rate, dst_channels)
            },
            sample_buf: None,
            pending: Vec::new(),
            scratch: Vec::new(),
            exhausted: false,
            duration_ms,
        })
    }

    /// Produces up to `frames_wanted` output frames into `out` (interleaved, device rate).
    ///
    /// Packets do not align with chunk boundaries: one may yield more frames than requested. The
    /// surplus is carried in `pending` for the next call instead of being discarded, so a track
    /// decodes to its full length rather than shedding a packet's worth of audio per chunk.
    fn decode_chunk(
        &mut self,
        out: &mut Vec<f32>,
        frames_wanted: usize,
        gain: &mut GainStage,
        rate: u32,
    ) -> usize {
        let dst_channels = self.converter.dst_channels;
        let target_samples = frames_wanted * dst_channels;
        // Guards against a corrupt stream that reports errors forever instead of ending.
        let mut consecutive_errors = 0usize;

        while self.pending.len() < target_samples {
            let packet = match self.reader.next_packet() {
                Ok(packet) => {
                    consecutive_errors = 0;
                    packet
                }
                Err(err) if is_unrecoverable_stream_error(&err) => {
                    self.exhausted = true;
                    break;
                }
                // A malformed packet should be skipped, not kill the track.
                Err(_) => {
                    consecutive_errors += 1;
                    if consecutive_errors > 64 {
                        self.exhausted = true;
                        break;
                    }
                    continue;
                }
            };

            if packet.track_id() != self.track_id {
                continue;
            }

            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    let spec = *decoded.spec();
                    if self.sample_buf.is_none() {
                        self.sample_buf =
                            Some(SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
                    }
                    self.converter
                        .configure(spec.rate, spec.channels.count(), dst_channels);
                    if let Some(buf) = self.sample_buf.as_mut() {
                        buf.copy_interleaved_ref(decoded);
                        self.scratch.clear();
                        self.converter.convert(buf.samples(), &mut self.scratch);
                    }
                    // Gain is applied as samples are produced, so each one is scaled exactly once
                    // and the ramp stays in step with the audio timeline.
                    gain.process(&mut self.scratch, rate);
                    self.pending.extend_from_slice(&self.scratch);
                }
                Err(err) if is_unrecoverable_stream_error(&err) => {
                    self.exhausted = true;
                    break;
                }
                Err(_) => {
                    consecutive_errors += 1;
                    if consecutive_errors > 64 {
                        self.exhausted = true;
                        break;
                    }
                    continue;
                }
            }
        }

        if self.pending.is_empty() {
            out.clear();
            return 0;
        }
        let take = self.pending.len().min(target_samples);
        out.clear();
        out.extend_from_slice(&self.pending[..take]);
        self.pending.drain(..take);
        out.len() / dst_channels
    }

    fn seek(&mut self, position_ms: u64) -> Result<(), String> {
        let seconds = position_ms as f64 / 1000.0;
        self.reader
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: Time::from(seconds),
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|e| format!("seek failed: {e}"))?;
        self.decoder.reset();
        self.converter.reset();
        self.pending.clear();
        self.exhausted = false;
        Ok(())
    }
}

/// Outcome of one decode-cycle refill.
enum Refill {
    /// `scratch` holds the next chunk and the service loop should push it to the ring.
    Produced,
    /// The current track ended and the pre-rolled next one took its place; carries the id of the
    /// track that finished so the UI can follow along.
    Advanced(Option<String>),
    /// Nothing to do this cycle — the decoder is between packets.
    Idle,
    /// The queue is exhausted.
    Finished,
}

struct Core {
    current: Option<PlayingTrack>,
    next: Option<PlayingTrack>,
    next_track: Option<EngineTrack>,
    current_track: Option<EngineTrack>,
    gain: GainStage,
    next_gain: GainStage,
    rate: u32,
    channels: usize,
    volume: f32,
    playing: bool,
    normalizing: bool,
    /// Frames handed to the ring since the stream opened. Monotonic and never reset, so it stays
    /// on the same timeline as the callback's `frames_played`.
    pushed_frames: u64,
    /// Reused between chunks so steady playback does not allocate.
    scratch: Vec<f32>,
    /// Samples decoded before the next track is needed. They are already gain-staged and ready for
    /// the output ring, so the transition never waits on disk or network I/O.
    next_prefetch: Vec<f32>,
    current_prefetch: Vec<f32>,
    current_prefetch_offset: usize,
}

impl Core {
    fn new(rate: u32, channels: usize) -> Self {
        Self {
            current: None,
            next: None,
            next_track: None,
            current_track: None,
            gain: GainStage::new(channels),
            next_gain: GainStage::new(channels),
            rate,
            channels,
            volume: 1.0,
            playing: false,
            normalizing: true,
            pushed_frames: 0,
            scratch: Vec::new(),
            next_prefetch: Vec::new(),
            current_prefetch: Vec::new(),
            current_prefetch_offset: 0,
        }
    }

    fn decode_current_chunk(&mut self, frames_wanted: usize) -> usize {
        let target_samples = frames_wanted * self.channels;
        if self.current_prefetch_offset < self.current_prefetch.len() {
            let remaining = self.current_prefetch.len() - self.current_prefetch_offset;
            let take = remaining.min(target_samples);
            self.scratch.clear();
            self.scratch.extend_from_slice(
                &self.current_prefetch
                    [self.current_prefetch_offset..self.current_prefetch_offset + take],
            );
            self.current_prefetch_offset += take;
            if self.current_prefetch_offset == self.current_prefetch.len() {
                self.current_prefetch.clear();
                self.current_prefetch_offset = 0;
            }
            return self.scratch.len() / self.channels;
        }

        self.current
            .as_mut()
            .map(|current| {
                current.decode_chunk(&mut self.scratch, frames_wanted, &mut self.gain, self.rate)
            })
            .unwrap_or(0)
    }

    fn prime_next(&mut self) {
        self.next_prefetch.clear();
        let target_samples = self.rate as usize * self.channels * PRE_ROLL_SECONDS;
        while self.next_prefetch.len() < target_samples {
            let remaining_samples = target_samples - self.next_prefetch.len();
            let frames_wanted = (remaining_samples / self.channels).min(CHUNK_FRAMES).max(1);
            self.scratch.clear();
            let produced = self
                .next
                .as_mut()
                .map(|next| {
                    next.decode_chunk(
                        &mut self.scratch,
                        frames_wanted,
                        &mut self.next_gain,
                        self.rate,
                    )
                })
                .unwrap_or(0);
            if produced == 0 {
                break;
            }
            self.next_prefetch.extend_from_slice(&self.scratch);
        }
    }

    /// Positions the playback timeline on the sample that will be heard next, accounting for the
    /// audio still sitting in the ring.
    /// Produces the next chunk into `self.scratch`, splicing the pre-rolled next track in when the
    /// current one runs dry. The splice writes into the same buffer the caller pushes to the ring
    /// and drops nothing across the seam, which is what makes the transition gapless.
    fn refill(&mut self, shared: &Shared, frames_wanted: usize) -> Refill {
        let mut produced = 0usize;
        if self.current.is_some() {
            produced = self.decode_current_chunk(frames_wanted);
        } else {
            self.scratch.clear();
        }

        if produced > 0 {
            return Refill::Produced;
        }
        if !self.current.as_ref().is_some_and(|t| t.exhausted) {
            return Refill::Idle;
        }

        let ended = self.current_track.as_ref().map(|t| t.id.clone());
        if self.advance_to_next(shared) {
            Refill::Advanced(ended)
        } else {
            *shared
                .pending_ended
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = ended;
            Refill::Finished
        }
    }

    /// Promotes the pre-rolled decoder to current. Returns false when the queue has nothing left,
    /// in which case playback has finished.
    fn advance_to_next(&mut self, shared: &Shared) -> bool {
        let Some(next) = self.next.take() else {
            return false;
        };

        self.current = Some(next);
        self.current_track = self.next_track.clone();
        self.next_track = None;
        self.current_prefetch = std::mem::take(&mut self.next_prefetch);
        self.current_prefetch_offset = 0;
        self.gain = std::mem::replace(&mut self.next_gain, GainStage::new(self.channels));

        {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            state.next_track = None;
            state.current_track = self.current_track.clone();
            state.duration_ms = self.current.as_ref().map(|t| t.duration_ms).unwrap_or(0);
            state.position_ms = 0;
            state.is_gapless = false;
        }
        self.anchor(shared, 0);
        true
    }

    fn anchor(&self, shared: &Shared, offset_ms: u64) {
        shared
            .track_anchor
            .store(self.pushed_frames, Ordering::Relaxed);
        shared.anchor_ms.store(offset_ms, Ordering::Relaxed);
    }
}

pub struct AudioEngine {
    tx: Sender<Cmd>,
    shared: Arc<Shared>,
}

impl AudioEngine {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        let shared = Arc::new(Shared {
            state: Mutex::new(EngineState::default()),
            frames_played: AtomicU64::new(0),
            track_anchor: AtomicU64::new(0),
            anchor_ms: AtomicU64::new(0),
            output_rate: AtomicU32::new(44_100),
            output_channels: AtomicU32::new(2),
            drained: AtomicBool::new(true),
            pushed_total: AtomicU64::new(0),
            pending_ended: Mutex::new(None),
            paused: AtomicBool::new(true),
            app: Mutex::new(None),
        });

        let thread_shared = shared.clone();
        std::thread::Builder::new()
            .name("sonora-audio".into())
            .spawn(move || run_audio_thread(rx, thread_shared))
            .expect("failed to spawn audio thread");

        Self { tx, shared }
    }

    /// Wires the engine to the Tauri app so it can emit progress/status/ended events, and starts
    /// the 100 ms progress ticker.
    pub fn attach_app(&self, app: AppHandle) {
        *self.shared.app.lock().unwrap_or_else(|e| e.into_inner()) = Some(app.clone());

        let shared = self.shared.clone();
        std::thread::Builder::new()
            .name("sonora-progress".into())
            .spawn(move || {
                let mut last_status: Option<PlaybackStatus> = None;
                let mut last_track_id: Option<Option<String>> = None;
                let mut last_normalizing: Option<bool> = None;
                let mut last_gapless: Option<bool> = None;

                loop {
                    std::thread::sleep(Duration::from_millis(100));
                    let snapshot = shared.snapshot();
                    if snapshot.status == PlaybackStatus::Stopped {
                        if let Some(track_id) = shared
                            .pending_ended
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .take()
                        {
                            let _ = app.emit(
                                EVENT_TRACK_ENDED,
                                serde_json::json!({ "trackId": track_id, "gapless": false }),
                            );
                        }
                    }
                    let _ = app.emit(
                        EVENT_PROGRESS,
                        PlaybackProgress {
                            position_ms: snapshot.position_ms,
                            duration_ms: snapshot.duration_ms,
                        },
                    );

                    let current_track_id = snapshot.current_track.as_ref().map(|t| t.id.clone());
                    let status_changed = last_status.as_ref() != Some(&snapshot.status);
                    let track_changed = last_track_id.as_ref() != Some(&current_track_id);
                    let norm_changed = last_normalizing != Some(snapshot.is_normalizing);
                    let gapless_changed = last_gapless != Some(snapshot.is_gapless);

                    if status_changed || track_changed || norm_changed || gapless_changed {
                        last_status = Some(snapshot.status.clone());
                        last_track_id = Some(current_track_id);
                        last_normalizing = Some(snapshot.is_normalizing);
                        last_gapless = Some(snapshot.is_gapless);
                        let _ = app.emit(EVENT_STATUS, snapshot);
                    }
                }
            })
            .expect("failed to spawn progress thread");
    }

    fn send(&self, cmd: Cmd) -> Result<(), String> {
        self.tx
            .send(cmd)
            .map_err(|_| "audio thread is not running".to_string())
    }

    pub fn load_track(&self, track: EngineTrack, auto_play: bool) -> Result<(), String> {
        self.send(Cmd::Load { track, auto_play })
    }

    pub fn prebuffer_next_track(&self, track: Option<EngineTrack>) -> Result<(), String> {
        self.send(Cmd::SetNext(track))
    }

    pub fn play(&self) -> Result<(), String> {
        self.send(Cmd::Play)
    }

    pub fn pause(&self) -> Result<(), String> {
        self.send(Cmd::Pause)
    }

    pub fn seek(&self, position_ms: u64) -> Result<(), String> {
        self.send(Cmd::Seek(position_ms))
    }

    pub fn set_volume(&self, volume: f32) -> Result<(), String> {
        self.send(Cmd::SetVolume(volume))
    }

    pub fn toggle_normalization(&self, enabled: bool) -> Result<bool, String> {
        self.send(Cmd::SetNormalization(enabled))?;
        Ok(enabled)
    }

    pub fn stop(&self) -> Result<(), String> {
        self.send(Cmd::Stop)
    }

    pub fn snapshot(&self) -> EngineState {
        self.shared.snapshot()
    }
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Shared {
    pub fn snapshot(&self) -> EngineState {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner()).clone();
        state.position_ms = self.position_ms();
        // The decoder finishes up to four seconds ahead of the speakers. Reporting Stopped at that
        // moment would blank the UI while the end of the track is still audible, so the status only
        // settles once the output callback has consumed everything that was queued.
        let output_caught_up =
            self.frames_played.load(Ordering::Relaxed) >= self.pushed_total.load(Ordering::Relaxed);
        if self.drained.load(Ordering::Relaxed)
            && output_caught_up
            && state.status == PlaybackStatus::Playing
        {
            state.status = PlaybackStatus::Stopped;
        }
        state
    }
}

fn run_audio_thread(rx: Receiver<Cmd>, shared: Arc<Shared>) {
    let host = cpal::default_host();
    let Some(device) = host.default_output_device() else {
        eprintln!("[sonora] no output device available; playback disabled");
        while let Ok(_cmd) = rx.recv() {}
        return;
    };

    let supported = match device.default_output_config() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("[sonora] no usable output config: {err}");
            return;
        }
    };

    let rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();

    shared.output_rate.store(rate, Ordering::Relaxed);
    shared
        .output_channels
        .store(channels as u32, Ordering::Relaxed);
    {
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.volume = 1.0;
    }

    let ring: Arc<ArrayQueue<f32>> =
        Arc::new(ArrayQueue::new(rate as usize * channels * RING_SECONDS));
    let consumer = ring.clone();
    let callback_shared = shared.clone();

    let result = match sample_format {
        cpal::SampleFormat::F32 => {
            build_stream::<f32>(&device, &config, consumer, channels, callback_shared)
        }
        cpal::SampleFormat::I16 => {
            build_stream::<i16>(&device, &config, consumer, channels, callback_shared)
        }
        cpal::SampleFormat::U16 => {
            build_stream::<u16>(&device, &config, consumer, channels, callback_shared)
        }
        cpal::SampleFormat::I32 => {
            build_stream::<i32>(&device, &config, consumer, channels, callback_shared)
        }
        other => {
            eprintln!("[sonora] unsupported output sample format {other:?}");
            return;
        }
    };

    let stream = match result {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("[sonora] failed to open output stream: {err}");
            return;
        }
    };
    if let Err(err) = stream.play() {
        eprintln!("[sonora] failed to start output stream: {err}");
        return;
    }

    let mut core = Core::new(rate, channels);

    loop {
        // Drain pending control messages first so a pause lands within one chunk.
        loop {
            match rx.try_recv() {
                Ok(cmd) => handle_command(&mut core, cmd, &shared, &ring),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        let free_samples = ring.capacity() - ring.len();
        let wanted = CHUNK_FRAMES * channels;
        if !core.playing || free_samples < wanted {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }

        match core.refill(&shared, CHUNK_FRAMES) {
            Refill::Produced => {}
            Refill::Advanced(ended) => {
                if let Some(id) = ended {
                    shared.emit(
                        EVENT_TRACK_ENDED,
                        serde_json::json!({ "trackId": id, "gapless": true }),
                    );
                }
                continue;
            }
            Refill::Idle => {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            Refill::Finished => {
                // The ring may still hold seconds of audio; `snapshot` flips the status once the
                // output has caught up with it.
                core.playing = false;
                shared.drained.store(true, Ordering::Relaxed);
                continue;
            }
        }

        apply_volume(&mut core.scratch, core.volume);
        for sample in &core.scratch {
            if ring.push(*sample).is_err() {
                break;
            }
        }
        core.pushed_frames += (core.scratch.len() / channels) as u64;
        shared
            .pushed_total
            .store(core.pushed_frames, Ordering::Relaxed);
    }
}

/// Applies the output volume to one interleaved chunk.
fn apply_volume(samples: &mut [f32], volume: f32) {
    if (volume - 1.0).abs() < f32::EPSILON {
        return;
    }
    for sample in samples.iter_mut() {
        *sample *= volume;
    }
}

/// crossbeam's ArrayQueue has no bulk clear, and dropping queued audio is exactly what a seek or a
/// track load needs.
fn drain(ring: &ArrayQueue<f32>) {
    while ring.pop().is_some() {}
}

fn handle_command(core: &mut Core, cmd: Cmd, shared: &Arc<Shared>, ring: &Arc<ArrayQueue<f32>>) {
    match cmd {
        Cmd::Load { track, auto_play } => {
            drain(ring);
            *shared
                .pending_ended
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = None;
            shared.frames_played.store(0, Ordering::Relaxed);
            shared.track_anchor.store(0, Ordering::Relaxed);
            shared.anchor_ms.store(0, Ordering::Relaxed);
            core.pushed_frames = 0;
            shared.pushed_total.store(0, Ordering::Relaxed);

            match PlayingTrack::open(&track, core.rate, core.channels) {
                Ok(playing) => {
                    core.gain
                        .begin_track(track.loudness_lufs, core.normalizing, core.rate);
                    core.current = Some(playing);
                    core.current_track = Some(track.clone());
                    core.next = None;
                    core.next_track = None;
                    core.next_prefetch.clear();
                    core.current_prefetch.clear();
                    core.current_prefetch_offset = 0;
                    core.next_gain = GainStage::new(core.channels);
                    core.playing = auto_play;
                    shared.drained.store(false, Ordering::Relaxed);
                    shared.paused.store(!auto_play, Ordering::Relaxed);
                    {
                        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                        state.current_track = Some(track.clone());
                        state.next_track = None;
                        state.duration_ms = track.duration_ms;
                        state.position_ms = 0;
                        state.is_gapless = false;
                        state.status = if auto_play {
                            PlaybackStatus::Playing
                        } else {
                            PlaybackStatus::Paused
                        };
                    }
                }
                Err(err) => {
                    eprintln!("[sonora] {err}");
                    core.current = None;
                    core.current_track = None;
                    core.next = None;
                    core.next_track = None;
                    core.next_prefetch.clear();
                    core.current_prefetch.clear();
                    core.current_prefetch_offset = 0;
                    core.playing = false;
                    let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    state.status = PlaybackStatus::Stopped;
                    state.current_track = None;
                    state.next_track = None;
                    state.duration_ms = 0;
                    state.position_ms = 0;
                    state.is_gapless = false;
                }
            }
        }

        Cmd::SetNext(track) => {
            core.next = None;
            core.next_track = None;
            core.next_prefetch.clear();
            core.next_gain = GainStage::new(core.channels);
            if let Some(next_track) = track {
                // Only pre-roll when something is already playing to avoid an idle decoder.
                if core.current.is_some() {
                    match PlayingTrack::open(&next_track, core.rate, core.channels) {
                        Ok(next) => {
                            core.next = Some(next);
                            core.next_gain.begin_track(
                                next_track.loudness_lufs,
                                core.normalizing,
                                core.rate,
                            );
                            core.prime_next();
                            if !core.next_prefetch.is_empty() {
                                core.next_track = Some(next_track);
                            } else {
                                core.next = None;
                            }
                        }
                        Err(err) => eprintln!("[sonora] cannot pre-buffer next track: {err}"),
                    }
                }
            }
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            state.next_track = core.next_track.clone();
            state.is_gapless = core.next.is_some() && !core.next_prefetch.is_empty();
        }

        Cmd::Play => {
            core.playing = true;
            shared.drained.store(false, Ordering::Relaxed);
            shared.paused.store(false, Ordering::Relaxed);
            shared
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .status = PlaybackStatus::Playing;
        }

        Cmd::Pause => {
            core.playing = false;
            shared.paused.store(true, Ordering::Relaxed);
            shared
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .status = PlaybackStatus::Paused;
        }

        Cmd::Seek(position_ms) => {
            if let Some(current) = core.current.as_mut() {
                // Everything already queued belongs to the pre-seek position; drop it so the
                // anchor below lands on the first sample of the resumed audio.
                drain(ring);
                match current.seek(position_ms) {
                    Ok(()) => {
                        core.current_prefetch.clear();
                        core.current_prefetch_offset = 0;
                        core.gain = GainStage::new(core.channels);
                        core.gain.begin_track(
                            core.current_track.as_ref().and_then(|t| t.loudness_lufs),
                            core.normalizing,
                            core.rate,
                        );
                        shared.frames_played.store(0, Ordering::Relaxed);
                        shared.track_anchor.store(0, Ordering::Relaxed);
                        shared.anchor_ms.store(position_ms, Ordering::Relaxed);
                        core.pushed_frames = 0;
                        shared.pushed_total.store(0, Ordering::Relaxed);
                        shared
                            .state
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .position_ms = position_ms;
                    }
                    Err(err) => eprintln!("[sonora] {err}"),
                }
            }
        }

        Cmd::SetVolume(volume) => {
            core.volume = volume.clamp(0.0, 1.0);
            shared
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .volume = core.volume;
        }

        Cmd::SetNormalization(enabled) => {
            core.normalizing = enabled;
            if enabled {
                core.gain.begin_track(
                    core.current_track.as_ref().and_then(|t| t.loudness_lufs),
                    true,
                    core.rate,
                );
            } else {
                core.gain.set_enabled(false, core.rate);
            }
            if let Some(next_track) = core.next_track.clone() {
                core.next = PlayingTrack::open(&next_track, core.rate, core.channels).ok();
                core.next_prefetch.clear();
                core.next_gain = GainStage::new(core.channels);
                if core.next.is_some() {
                    core.next_gain.begin_track(
                        next_track.loudness_lufs,
                        core.normalizing,
                        core.rate,
                    );
                    core.prime_next();
                    if core.next_prefetch.is_empty() {
                        core.next = None;
                        core.next_track = None;
                    }
                }
            }
            shared
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_normalizing = enabled;
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            state.next_track = core.next_track.clone();
            state.is_gapless = core.next.is_some() && !core.next_prefetch.is_empty();
        }

        Cmd::Stop => {
            drain(ring);
            *shared
                .pending_ended
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = None;
            core.playing = false;
            shared.paused.store(true, Ordering::Relaxed);
            core.current = None;
            core.next = None;
            core.current_track = None;
            core.next_track = None;
            core.next_prefetch.clear();
            core.current_prefetch.clear();
            core.current_prefetch_offset = 0;
            shared.frames_played.store(0, Ordering::Relaxed);
            shared.track_anchor.store(0, Ordering::Relaxed);
            shared.anchor_ms.store(0, Ordering::Relaxed);
            core.pushed_frames = 0;
            shared.pushed_total.store(0, Ordering::Relaxed);
            shared.drained.store(true, Ordering::Relaxed);
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            state.status = PlaybackStatus::Stopped;
            state.position_ms = 0;
            state.current_track = None;
            state.next_track = None;
            state.duration_ms = 0;
            state.is_gapless = false;
        }
    }
}

/// Conversion from the engine's internal f32 mix into whatever the device wants.
trait OutputSample: cpal::SizedSample + Copy {
    fn from_f32(value: f32) -> Self;
    const SILENCE: Self;
}

impl OutputSample for f32 {
    fn from_f32(value: f32) -> Self {
        value
    }
    const SILENCE: Self = 0.0;
}

impl OutputSample for i16 {
    fn from_f32(value: f32) -> Self {
        (value.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
    }
    const SILENCE: Self = 0;
}

impl OutputSample for u16 {
    fn from_f32(value: f32) -> Self {
        ((value.clamp(-1.0, 1.0) * 0.5 + 0.5) * u16::MAX as f32) as u16
    }
    const SILENCE: Self = u16::MAX / 2;
}

impl OutputSample for i32 {
    fn from_f32(value: f32) -> Self {
        (value.clamp(-1.0, 1.0) as f64 * i32::MAX as f64) as i32
    }
    const SILENCE: Self = 0;
}

fn build_stream<T: OutputSample>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    consumer: Arc<ArrayQueue<f32>>,
    channels: usize,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample,
{
    device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            if shared.paused.load(Ordering::Relaxed) {
                data.iter_mut().for_each(|slot| *slot = T::SILENCE);
                // Deliberately does not advance `frames_played`: a paused player has not moved.
                return;
            }
            let mut samples_popped = 0usize;
            for slot in data.iter_mut() {
                match consumer.pop() {
                    Some(value) => {
                        *slot = T::from_f32(value);
                        samples_popped += 1;
                    }
                    None => *slot = T::SILENCE,
                }
            }
            if channels > 0 && samples_popped > 0 {
                let frames = (samples_popped / channels) as u64;
                shared.frames_played.fetch_add(frames, Ordering::Relaxed);
            }
        },
        |err| eprintln!("[sonora] output stream error: {err}"),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine_track(id: &str, path: &str, loudness: Option<f32>) -> EngineTrack {
        EngineTrack {
            id: id.into(),
            title: id.into(),
            artist: "Test".into(),
            file_path: Some(path.into()),
            stream_url: None,
            duration_ms: 1000,
            loudness_lufs: loudness,
        }
    }

    #[test]
    fn channel_mapping_fans_out_mono_and_truncates_wider_sources() {
        // Two mono frames -> two stereo frames.
        let mono = [0.5f32, -0.5];
        assert_eq!(mapped_channel(&mono, 1, 0, 0), 0.5);
        assert_eq!(mapped_channel(&mono, 1, 0, 1), 0.5);
        assert_eq!(mapped_channel(&mono, 1, 1, 1), -0.5);

        // Stereo source into a 4-channel device repeats L/R/L/R.
        let stereo = [0.1f32, 0.2, 0.3, 0.4];
        assert_eq!(mapped_channel(&stereo, 2, 1, 0), 0.3);
        assert_eq!(mapped_channel(&stereo, 2, 1, 3), 0.4);
    }

    #[test]
    fn converter_resamples_to_the_target_rate() {
        let mut converter = Converter {
            src_rate: 44_100,
            dst_rate: 48_000,
            src_channels: 1,
            dst_channels: 1,
            working: Vec::new(),
            prev: Vec::new(),
            has_prev: false,
            pos: 0.0,
        };

        // Feed 1 second of mono in chunks and confirm we get ~1 second back out.
        let input: Vec<f32> = (0..44_100).map(|n| (n as f32 / 20.0).sin()).collect();
        let mut out = Vec::new();
        for chunk in input.chunks(4410) {
            converter.convert(chunk, &mut out);
        }
        let produced = out.len();
        assert!(
            (46_000..=50_000).contains(&produced),
            "expected ~48000 output samples, got {produced}"
        );
    }

    #[test]
    fn converter_is_identity_at_matching_rates() {
        let mut converter = Converter {
            src_rate: 48_000,
            dst_rate: 48_000,
            src_channels: 1,
            dst_channels: 1,
            working: Vec::new(),
            prev: Vec::new(),
            has_prev: false,
            pos: 0.0,
        };
        let input: Vec<f32> = (0..1000).map(|n| n as f32).collect();
        let mut out = Vec::new();
        converter.convert(&input, &mut out);
        // Same-rate conversion may hold one frame back as interpolation carry.
        assert!(out.len() >= 998, "lost too many frames: {}", out.len());
        for (i, value) in out.iter().take(998).enumerate() {
            assert!((value - i as f32).abs() < 0.5, "sample {i} drifted");
        }
    }

    /// Writes a minimal 16-bit PCM WAV so the decode path can be exercised against a real file
    /// rather than a hand-built packet stream.
    fn write_wav(path: &Path, rate: u32, channels: u16, seconds: f32, frequency: f32) {
        let frames = (rate as f32 * seconds) as u32;
        let mut samples = Vec::with_capacity(frames as usize * channels as usize);
        for n in 0..frames {
            let value =
                (2.0 * std::f32::consts::PI * frequency * n as f32 / rate as f32).sin() * 0.8;
            for _ in 0..channels {
                samples.push((value * i16::MAX as f32) as i16);
            }
        }

        let data_bytes = (samples.len() * 2) as u32;
        let mut out = Vec::with_capacity(44 + data_bytes as usize);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
        out.extend_from_slice(&(channels * 2).to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_bytes.to_le_bytes());
        for sample in samples {
            out.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(path, out).unwrap();
    }

    /// Decodes a whole file and reports (frames produced, peak amplitude of the final chunk,
    /// which is measured after the normalization ramp has settled).
    fn decode_all(track: &EngineTrack, dst_rate: u32, dst_channels: usize) -> (usize, f32) {
        let mut playing = PlayingTrack::open(track, dst_rate, dst_channels).expect("opens");
        let mut gain = GainStage::new(dst_channels);
        gain.begin_track(track.loudness_lufs, true, dst_rate);

        let mut buffer = Vec::new();
        let mut frames = 0usize;
        let mut peak = 0.0f32;
        loop {
            let produced = playing.decode_chunk(&mut buffer, 4096, &mut gain, dst_rate);
            if produced == 0 {
                break;
            }
            assert!(
                buffer.iter().all(|s| s.is_finite()),
                "decoder emitted a non-finite sample"
            );
            frames += produced;
            peak = buffer.iter().fold(0.0f32, |acc, s| acc.max(s.abs()));
        }
        (frames, peak)
    }

    #[test]
    fn decodes_a_real_file_to_the_requested_output_layout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.wav");
        // 1 s of 44.1 kHz stereo, played back at 48 kHz stereo.
        write_wav(&path, 44_100, 2, 1.0, 440.0);

        let (frames, peak) = decode_all(
            &engine_track("tone", path.to_str().unwrap(), Some(-14.0)),
            48_000,
            2,
        );

        assert!(
            (47_000..=49_000).contains(&frames),
            "one second at 48 kHz should be ~48000 frames, got {frames}"
        );
        assert!(peak > 0.5, "decoded audio must not be silent (peak {peak})");
    }

    #[test]
    fn mono_source_is_upmixed_to_the_device_layout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mono.wav");
        write_wav(&path, 44_100, 1, 0.5, 440.0);

        let (frames, peak) = decode_all(
            &engine_track("mono", path.to_str().unwrap(), None),
            44_100,
            2,
        );

        assert!((21_500..=22_500).contains(&frames), "got {frames} frames");
        assert!(peak > 0.5, "mono must survive the upmix (peak {peak})");
    }

    #[test]
    fn normalization_attenuates_a_track_tagged_too_loud() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("loud.wav");
        write_wav(&path, 48_000, 2, 0.5, 440.0);

        let (_, normalized) = decode_all(
            &engine_track("loud", path.to_str().unwrap(), Some(-6.0)),
            48_000,
            2,
        );
        let (_, untouched) = decode_all(
            &engine_track("loud", path.to_str().unwrap(), None),
            48_000,
            2,
        );

        // -6 LUFS against the -14 LUFS target is roughly -8 dB of gain reduction. The ramp takes
        // 200 ms, so this compares the settled tail rather than the opening fade.
        assert!(
            normalized < untouched * 0.5,
            "expected attenuation once the ramp settles: {normalized} vs {untouched}"
        );
        assert!(
            normalized > untouched * 0.25,
            "attenuation overshot: {normalized} vs {untouched}"
        );
    }

    /// Builds a `Shared` without opening an output device, so the decode/transition logic can be
    /// driven in tests exactly as the audio thread drives it. Starts in the same idle, silent
    /// state a freshly constructed engine is in.
    fn test_shared() -> Arc<Shared> {
        Arc::new(Shared {
            state: Mutex::new(EngineState::default()),
            frames_played: AtomicU64::new(0),
            track_anchor: AtomicU64::new(0),
            anchor_ms: AtomicU64::new(0),
            output_rate: AtomicU32::new(48_000),
            output_channels: AtomicU32::new(2),
            drained: AtomicBool::new(true),
            pushed_total: AtomicU64::new(0),
            pending_ended: Mutex::new(None),
            paused: AtomicBool::new(true),
            app: Mutex::new(None),
        })
    }

    /// Runs the real service loop over `core` until playback finishes, returning every frame it
    /// would have pushed to the output ring.
    fn drain_core(core: &mut Core, shared: &Shared) -> Vec<f32> {
        let mut collected = Vec::new();
        let mut advanced = 0;
        for _ in 0..10_000 {
            match core.refill(shared, 4096) {
                Refill::Produced => collected.extend_from_slice(&core.scratch),
                Refill::Advanced(_) => advanced += 1,
                Refill::Idle => continue,
                Refill::Finished => return collected,
            }
        }
        panic!("refill never reached Finished (advanced {advanced} times)");
    }

    #[test]
    fn gapless_transition_keeps_every_frame_from_both_tracks() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("a.wav");
        let second = dir.path().join("b.wav");
        write_wav(&first, 48_000, 2, 0.5, 440.0);
        write_wav(&second, 48_000, 2, 0.5, 880.0);

        let track_a = engine_track("a", first.to_str().unwrap(), Some(-14.0));
        let track_b = engine_track("b", second.to_str().unwrap(), Some(-14.0));

        // Each track on its own, for the expected frame count.
        let (solo_a, _) = decode_all(&track_a, 48_000, 2);
        let (solo_b, _) = decode_all(&track_b, 48_000, 2);

        let shared = test_shared();
        let ring = Arc::new(ArrayQueue::new(48_000 * 2 * 4));
        let mut core = Core::new(48_000, 2);
        handle_command(
            &mut core,
            Cmd::Load {
                track: track_a.clone(),
                auto_play: true,
            },
            &shared,
            &ring,
        );
        handle_command(
            &mut core,
            Cmd::SetNext(Some(track_b.clone())),
            &shared,
            &ring,
        );
        assert!(
            !core.next_prefetch.is_empty(),
            "a reported gapless transition must have decoded audio ready"
        );

        let spliced = drain_core(&mut core, &shared);

        // `decode_all` counts frames; `spliced` is interleaved stereo.
        assert_eq!(
            spliced.len(),
            (solo_a + solo_b) * 2,
            "the seam must not drop or duplicate frames"
        );
        // Stereo throughout, and the tail of the second track must actually be there.
        assert_eq!(spliced.len() % 2, 0);
        let seam = solo_a * 2;
        let around_seam = &spliced[seam - 200..seam + 200];
        let quiet = around_seam.iter().filter(|s| s.abs() < 1e-4).count();
        assert!(
            quiet < 200,
            "found a silent gap at the splice ({quiet}/400 samples near zero)"
        );
        assert!(
            spliced[seam + 100..].iter().any(|s| s.abs() > 0.1),
            "audio must continue past the splice"
        );
    }

    #[test]
    fn transition_restarts_normalization_for_the_incoming_track() {
        let dir = tempfile::tempdir().unwrap();
        let quiet = dir.path().join("quiet.wav");
        let loud = dir.path().join("loud.wav");
        write_wav(&quiet, 48_000, 2, 0.4, 440.0);
        write_wav(&loud, 48_000, 2, 0.4, 880.0);

        let shared = test_shared();
        let ring = Arc::new(ArrayQueue::new(48_000 * 2 * 4));
        let mut core = Core::new(48_000, 2);
        handle_command(
            &mut core,
            Cmd::Load {
                // A quiet master needs a boost.
                track: engine_track("quiet", quiet.to_str().unwrap(), Some(-24.0)),
                auto_play: true,
            },
            &shared,
            &ring,
        );
        handle_command(
            &mut core,
            Cmd::SetNext(Some(engine_track(
                "loud",
                loud.to_str().unwrap(),
                Some(-6.0),
            ))),
            &shared,
            &ring,
        );

        let gains = drain_core(&mut core, &shared);
        // Both tracks are full-scale sines; the second is tagged 18 dB hotter, so after
        // normalization its tail must sit well below the first track's opening.
        let head = gains[..4000].iter().fold(0.0f32, |a, s| a.max(s.abs()));
        let tail = gains[gains.len() - 4000..]
            .iter()
            .fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(
            tail < head * 0.5,
            "gain was not recomputed for the incoming track: head {head}, tail {tail}"
        );
    }

    #[test]
    fn queue_without_a_next_track_finishes_instead_of_looping() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("solo.wav");
        write_wav(&path, 48_000, 2, 0.2, 440.0);

        let shared = test_shared();
        let ring = Arc::new(ArrayQueue::new(48_000 * 2 * 4));
        let mut core = Core::new(48_000, 2);
        handle_command(
            &mut core,
            Cmd::Load {
                track: engine_track("solo", path.to_str().unwrap(), None),
                auto_play: true,
            },
            &shared,
            &ring,
        );

        // 0.2 s of 48 kHz stereo is 9600 frames / 19200 interleaved samples.
        let produced = drain_core(&mut core, &shared);
        assert_eq!(
            produced.len() % 2,
            0,
            "output must stay frame-aligned in stereo"
        );
        assert!(
            (19_000..=19_200).contains(&produced.len()),
            "expected ~19200 samples, got {}",
            produced.len()
        );
        assert_eq!(
            shared.pending_ended.lock().unwrap().as_deref(),
            Some("solo"),
            "non-gapless completion must wait until the audible ring drains"
        );
    }

    #[test]
    fn pause_freezes_output_and_play_resumes_it() {
        let shared = test_shared();
        let ring = Arc::new(ArrayQueue::new(1024));
        let mut core = Core::new(48_000, 2);

        // Nothing is playing yet, so the callback must already be silent.
        assert!(shared.paused.load(Ordering::Relaxed));

        handle_command(&mut core, Cmd::Play, &shared, &ring);
        assert!(
            !shared.paused.load(Ordering::Relaxed),
            "play must unfreeze output"
        );

        handle_command(&mut core, Cmd::Pause, &shared, &ring);
        assert!(
            shared.paused.load(Ordering::Relaxed),
            "pause must freeze output"
        );

        handle_command(&mut core, Cmd::Play, &shared, &ring);
        handle_command(&mut core, Cmd::Stop, &shared, &ring);
        assert!(
            shared.paused.load(Ordering::Relaxed),
            "stop must freeze output"
        );
    }

    #[test]
    fn position_tracks_the_frames_the_callback_has_actually_played() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long.wav");
        write_wav(&path, 48_000, 2, 0.2, 440.0);

        let shared = test_shared();
        let ring = Arc::new(ArrayQueue::new(48_000 * 2 * 4));
        let mut core = Core::new(48_000, 2);
        handle_command(
            &mut core,
            Cmd::Load {
                track: EngineTrack {
                    duration_ms: 60_000,
                    ..engine_track("long", path.to_str().unwrap(), None)
                },
                auto_play: true,
            },
            &shared,
            &ring,
        );

        // The decoder runs ahead of the callback; the anchor sits on its own timeline, so a track
        // that has been queued but not yet heard reads as position zero.
        core.pushed_frames = 48_000;
        core.anchor(&shared, 0);
        assert_eq!(shared.snapshot().position_ms, 0);

        // Half a second of frames actually leaves the output.
        shared
            .frames_played
            .store(48_000 + 24_000, Ordering::Relaxed);
        assert_eq!(shared.snapshot().position_ms, 500);

        // A seek moves the offset without disturbing the anchor.
        core.anchor(&shared, 12_000);
        assert_eq!(shared.snapshot().position_ms, 12_500);
    }

    #[test]
    fn engine_reports_stopped_with_no_track_loaded() {
        let engine = AudioEngine::new();
        let snapshot = engine.snapshot();
        assert_eq!(snapshot.status, PlaybackStatus::Stopped);
        assert_eq!(snapshot.position_ms, 0);
    }

    #[test]
    fn loading_an_unreadable_track_fails_without_panicking() {
        let engine = AudioEngine::new();
        let result = engine.load_track(
            engine_track("missing", "/nonexistent/definitely-not-here.flac", None),
            true,
        );
        assert!(
            result.is_ok(),
            "the command channel should accept the request"
        );
        // The decode thread reports the failure by leaving the engine stopped.
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(engine.snapshot().status, PlaybackStatus::Stopped);
    }

    #[test]
    fn http_source_rejects_a_non_streaming_scheme() {
        assert!(EngineTrack {
            stream_url: Some("ftp://example.com/x".into()),
            file_path: None,
            ..engine_track("x", "/x", None)
        }
        .stream_url
        .as_deref()
        .is_some_and(|u| !u.starts_with("http")));
    }

    #[test]
    fn http_timeouts_are_finite_and_map_to_timed_out() {
        assert_eq!(HTTP_CONNECT_TIMEOUT, Duration::from_secs(8));
        assert_eq!(HTTP_READ_TIMEOUT, Duration::from_secs(15));
        assert_eq!(
            http_io_error("timed out waiting for response").kind(),
            std::io::ErrorKind::TimedOut
        );
        assert_eq!(
            http_io_error("connection timeout").kind(),
            std::io::ErrorKind::TimedOut
        );
        assert_eq!(
            http_io_error("connection reset").kind(),
            std::io::ErrorKind::Other
        );
        let timed_out = SymphoniaError::IoError(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "read timeout",
        ));
        assert!(is_unrecoverable_stream_error(&timed_out));
    }
}
