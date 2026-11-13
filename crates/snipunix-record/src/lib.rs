//! Video recording engine for Snipunix.
//!
//! The desktop portal supplies a restricted PipeWire remote and a stream node.
//! This crate consumes that stream in-process with GStreamer, crops a selected
//! region after the real frame size is known, and writes an MP4 with H.264
//! video plus optional microphone and system audio.
//!
//! The [`Recorder`] type owns the whole lifecycle: start, pause, resume, stop
//! and cancel. Dropping a recorder tears the pipeline down and removes the
//! partial file, so a crashed or cancelled recording never leaves a misleading
//! half-written MP4 behind.

#![forbid(unsafe_code)]

pub mod clock;
pub mod crop;

pub use clock::ElapsedTracker;
pub use crop::{LogicalCrop, PixelCrop};

use std::os::fd::RawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gstreamer as gst;
use gstreamer::prelude::*;
use thiserror::Error;

/// Default frame rate for recordings.
pub const DEFAULT_FPS: u32 = 30;
/// Lowest supported recording frame rate.
pub const MIN_FPS: u32 = 10;
/// Highest supported recording frame rate.
pub const MAX_FPS: u32 = 60;
/// Default H.264 bitrate in kbit/s.
pub const DEFAULT_BITRATE_KBPS: u32 = 6000;
/// Lowest accepted bitrate in kbit/s.
pub const MIN_BITRATE_KBPS: u32 = 500;
/// Highest accepted bitrate in kbit/s.
pub const MAX_BITRATE_KBPS: u32 = 50_000;
/// How long prerolling may take before the recorder gives up.
pub const START_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the muxer may take to finalise the file at EOS.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(15);

/// A restricted PipeWire stream returned by the ScreenCast portal.
#[derive(Debug, Clone)]
pub struct PipeWireStream {
    /// The portal-provided PipeWire remote descriptor.
    pub fd: RawFd,
    /// The node ID of the selected stream.
    pub node_id: u32,
    /// Monotonic serial on portal v6+, preferred over the reusable node ID.
    pub serial: Option<u64>,
    /// Stream size in compositor-logical coordinates, when the portal knows it.
    pub logical_size: Option<(i32, i32)>,
}

/// Where frames come from.
#[derive(Debug, Clone)]
pub enum VideoSource {
    /// A live ScreenCast stream.
    PipeWire(PipeWireStream),
    /// A synthetic live pattern. Used by the self-test and by tests so the
    /// encoder path can be verified without opening a portal dialog.
    TestPattern { width: i32, height: i32 },
}

/// Independent audio consent toggles.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AudioOptions {
    /// Capture the default microphone.
    pub microphone: bool,
    /// Capture the default output monitor (system audio).
    pub system: bool,
}

impl AudioOptions {
    /// True when any audio source is enabled.
    #[must_use]
    pub fn any(self) -> bool {
        self.microphone || self.system
    }
}

/// Everything that describes one recording.
#[derive(Debug, Clone)]
pub struct RecordOptions {
    pub output: PathBuf,
    pub fps: u32,
    pub bitrate_kbps: u32,
    /// Region relative to the stream, in logical coordinates.
    pub crop: Option<LogicalCrop>,
    pub audio: AudioOptions,
}

impl RecordOptions {
    #[must_use]
    pub fn new(output: impl Into<PathBuf>) -> Self {
        Self {
            output: output.into(),
            fps: DEFAULT_FPS,
            bitrate_kbps: DEFAULT_BITRATE_KBPS,
            crop: None,
            audio: AudioOptions::default(),
        }
    }

    /// Clamps user-controlled values into the supported ranges.
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        self.fps = self.fps.clamp(MIN_FPS, MAX_FPS);
        self.bitrate_kbps = self.bitrate_kbps.clamp(MIN_BITRATE_KBPS, MAX_BITRATE_KBPS);
        self
    }
}

/// H.264 encoder backends in preference order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoder {
    /// VAAPI hardware encoder.
    VaapiH264,
    /// libx264 (software).
    X264,
    /// OpenH264 (software).
    OpenH264,
    /// libav's H.264 encoder (software).
    LibavH264,
}

impl Encoder {
    /// All supported encoders, best first.
    pub const ALL: [Self; 4] = [Self::VaapiH264, Self::X264, Self::OpenH264, Self::LibavH264];

    #[must_use]
    pub fn element(self) -> &'static str {
        match self {
            Self::VaapiH264 => "vah264enc",
            Self::X264 => "x264enc",
            Self::OpenH264 => "openh264enc",
            Self::LibavH264 => "avenc_h264",
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::VaapiH264 => "VAAPI H.264",
            Self::X264 => "x264",
            Self::OpenH264 => "OpenH264",
            Self::LibavH264 => "libav H.264",
        }
    }

    #[must_use]
    pub fn is_hardware(self) -> bool {
        matches!(self, Self::VaapiH264)
    }

    fn properties(self, fps: u32, bitrate_kbps: u32) -> String {
        let bits_per_second = u64::from(bitrate_kbps) * 1000;
        match self {
            Self::VaapiH264 => format!("bitrate={bitrate_kbps}"),
            Self::X264 => format!(
                "bitrate={bitrate_kbps} speed-preset=veryfast tune=zerolatency bframes=0 key-int-max={}",
                fps * 2
            ),
            Self::OpenH264 | Self::LibavH264 => format!("bitrate={bits_per_second}"),
        }
    }
}

/// Installed encoders and the one that would be selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderSupport {
    pub available: Vec<Encoder>,
    pub preferred: Option<Encoder>,
    pub hardware: bool,
}

/// Probes the GStreamer registry for installed H.264 encoders.
#[must_use]
pub fn encoder_support() -> EncoderSupport {
    let available = Encoder::ALL
        .into_iter()
        .filter(|encoder| gst::ElementFactory::find(encoder.element()).is_some())
        .collect::<Vec<_>>();
    let preferred = available.first().copied();
    EncoderSupport {
        hardware: available.iter().any(|encoder| encoder.is_hardware()),
        available,
        preferred,
    }
}

/// The encoder that a new recording would use.
#[must_use]
pub fn preferred_encoder() -> Option<Encoder> {
    encoder_support().preferred
}

/// A finished recording.
#[derive(Debug, Clone)]
pub struct RecordedFile {
    pub path: PathBuf,
    pub duration: Duration,
    pub bytes: u64,
    pub width: i32,
    pub height: i32,
    pub encoder: Encoder,
    pub has_audio: bool,
}

/// Recording lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderState {
    Recording,
    Paused,
    Stopped,
}

/// Errors surfaced by the recording engine.
#[derive(Debug, Error)]
pub enum RecordError {
    #[error("GStreamer could not be initialised: {0}")]
    Init(String),
    #[error("no H.264 encoder is installed (tried VAAPI, x264, OpenH264 and libav)")]
    NoEncoder,
    #[error("GStreamer element '{0}' is missing; install the matching plugin package")]
    MissingElement(&'static str),
    #[error("recordings must use the .mp4 or .mkv container, not '{0}'")]
    UnsupportedContainer(String),
    #[error("the recording pipeline failed: {0}")]
    Pipeline(String),
    #[error("the recording pipeline did not start within {0:?}")]
    StartTimeout(Duration),
    #[error("the recording did not finalise within {0:?}")]
    StopTimeout(Duration),
    #[error("the selected region is outside the captured stream")]
    CropOutOfBounds,
    #[error("could not write the recording: {0}")]
    Io(#[from] std::io::Error),
    #[error("the recorder is not {0}")]
    InvalidState(&'static str),
}

/// One live recording.
pub struct Recorder {
    pipeline: gst::Pipeline,
    bus: gst::Bus,
    output: PathBuf,
    encoder: Encoder,
    width: i32,
    height: i32,
    audio: bool,
    state: RecorderState,
    clock: ElapsedTracker,
    pause_offset: Arc<AtomicU64>,
    pause_flag: Arc<AtomicBool>,
    pause_started: Option<Instant>,
    finished: bool,
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Recorder")
            .field("output", &self.output)
            .field("encoder", &self.encoder)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("audio", &self.audio)
            .field("state", &self.state)
            .field("elapsed", &self.elapsed())
            .finish()
    }
}

impl Recorder {
    /// Builds the pipeline, prerolls it, maps the crop and starts recording.
    pub fn start(source: VideoSource, options: RecordOptions) -> Result<Self, RecordError> {
        init()?;
        let options = options.sanitized();
        let container = container_for(&options.output)?;
        ensure_elements(&source, options.audio, container)?;

        let encoder = preferred_encoder().ok_or(RecordError::NoEncoder)?;
        if let Some(parent) = options.output.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }

        let (source_description, logical_size) = source_description(&source)?;
        let description = build_description(
            &source_description,
            encoder,
            options.fps,
            options.bitrate_kbps,
            &options.output,
            options.audio,
            container,
        );
        tracing::debug!(%description, "starting recording pipeline");

        let element = gst::parse::launch(&description)
            .map_err(|error| RecordError::Pipeline(error.to_string()))?;
        let pipeline = element
            .downcast::<gst::Pipeline>()
            .map_err(|_| RecordError::Pipeline("parser did not return a pipeline".to_string()))?;
        let bus = pipeline
            .bus()
            .ok_or_else(|| RecordError::Pipeline("pipeline has no bus".to_string()))?;

        let preroll = match pipeline.set_state(gst::State::Paused) {
            Ok(result) => result,
            Err(error) => {
                let detail = bus.pop_filtered(&[gst::MessageType::Error]).map_or_else(
                    || RecordError::Pipeline(format!("could not preroll: {error}")),
                    |message| pipeline_error(&message),
                );
                return Err(fail(&pipeline, &options.output, detail));
            }
        };
        // A live source (both PipeWire and the self-test pattern) never
        // prerolls. Only wait for async completion when GStreamer says it is
        // actually prerolling.
        if preroll != gst::StateChangeSuccess::NoPreroll {
            match wait_for(
                &bus,
                &[gst::MessageType::AsyncDone, gst::MessageType::Error],
                START_TIMEOUT,
            ) {
                Some(message) if message.type_() == gst::MessageType::Error => {
                    return Err(fail(&pipeline, &options.output, pipeline_error(&message)));
                }
                Some(_) => {}
                None => {
                    return Err(fail(
                        &pipeline,
                        &options.output,
                        RecordError::StartTimeout(START_TIMEOUT),
                    ));
                }
            }
        }

        let negotiated = negotiated_size(&pipeline);
        let (width, height) = if let Some(crop) = options.crop {
            let logical = logical_size.ok_or(RecordError::CropOutOfBounds)?;
            let pixels = negotiated.ok_or(RecordError::CropOutOfBounds)?;
            let mapped =
                PixelCrop::map(crop, logical, pixels).ok_or(RecordError::CropOutOfBounds)?;
            let crop_element = pipeline
                .by_name("crop")
                .ok_or_else(|| RecordError::Pipeline("crop element is missing".to_string()))?;
            crop_element.set_property("left", mapped.left);
            crop_element.set_property("top", mapped.top);
            crop_element.set_property("right", mapped.right);
            crop_element.set_property("bottom", mapped.bottom);
            (mapped.width, mapped.height)
        } else {
            negotiated.or(logical_size).unwrap_or((0, 0))
        };

        let pause_offset = Arc::new(AtomicU64::new(0));
        let pause_flag = Arc::new(AtomicBool::new(false));
        let encoder_element = pipeline
            .by_name("encoder")
            .ok_or_else(|| RecordError::Pipeline("encoder element is missing".to_string()))?;
        let encoder_pad = encoder_element
            .static_pad("sink")
            .ok_or_else(|| RecordError::Pipeline("encoder has no sink pad".to_string()))?;
        install_pause_probe(&encoder_pad, &pause_flag, &pause_offset);
        for name in ["mic_src", "system_src"] {
            if let Some(source) = pipeline.by_name(name) {
                let pad = source
                    .static_pad("src")
                    .ok_or_else(|| RecordError::Pipeline(format!("{name} has no source pad")))?;
                install_pause_probe(&pad, &pause_flag, &pause_offset);
            }
        }

        if let Err(error) = pipeline.set_state(gst::State::Playing) {
            return Err(fail(
                &pipeline,
                &options.output,
                RecordError::Pipeline(format!("could not start playing: {error}")),
            ));
        }

        Ok(Self {
            pipeline,
            bus,
            output: options.output,
            encoder,
            width,
            height,
            audio: options.audio.any(),
            state: RecorderState::Recording,
            clock: ElapsedTracker::new(Instant::now()),
            pause_offset,
            pause_flag,
            pause_started: None,
            finished: false,
        })
    }

    /// Pauses frame consumption and the elapsed-time counter.
    ///
    /// The pipeline keeps running behind a closed valve. Tearing a live
    /// PipeWire pipeline down to `PAUSED` and back can leave the source stuck,
    /// so pause only stops buffers reaching the encoder. A pad probe rebases
    /// timestamps when recording resumes, which keeps the paused interval out
    /// of the file.
    pub fn pause(&mut self) -> Result<(), RecordError> {
        if self.state != RecorderState::Recording {
            return Err(RecordError::InvalidState("recording"));
        }
        self.pause_flag.store(true, Ordering::SeqCst);
        let now = Instant::now();
        self.clock.pause(now);
        self.pause_started = Some(now);
        self.state = RecorderState::Paused;
        tracing::debug!("recording paused");
        Ok(())
    }

    /// Resumes a paused recording.
    pub fn resume(&mut self) -> Result<(), RecordError> {
        if self.state != RecorderState::Paused {
            return Err(RecordError::InvalidState("paused"));
        }
        if let Some(started) = self.pause_started.take() {
            let paused_ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
            let offset = self.pause_offset.load(Ordering::Relaxed);
            self.pause_offset
                .store(offset.saturating_add(paused_ns), Ordering::Relaxed);
        }
        self.pause_flag.store(false, Ordering::SeqCst);
        self.clock.resume(Instant::now());
        self.state = RecorderState::Recording;
        tracing::debug!(
            offset_ms = self.pause_offset.load(Ordering::Relaxed) / 1_000_000,
            "recording resumed"
        );
        Ok(())
    }

    /// Running time excluding pauses.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.clock.elapsed(Instant::now())
    }

    #[must_use]
    pub fn state(&self) -> RecorderState {
        self.state
    }

    #[must_use]
    pub fn output_path(&self) -> &Path {
        &self.output
    }

    #[must_use]
    pub fn dimensions(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    #[must_use]
    pub fn encoder(&self) -> Encoder {
        self.encoder
    }

    /// Pops a pipeline error, if one arrived since the last call.
    pub fn take_error(&mut self) -> Option<RecordError> {
        let message = self.bus.pop_filtered(&[gst::MessageType::Error])?;
        Some(pipeline_error(&message))
    }

    /// Finalises the file with EOS and returns its metadata.
    pub fn stop(mut self) -> Result<RecordedFile, RecordError> {
        let duration = self.elapsed();
        self.clock.pause(Instant::now());
        self.state = RecorderState::Stopped;
        self.finished = true;

        let started = Instant::now();
        if !self.pipeline.send_event(gst::event::Eos::new()) {
            let _ = self.pipeline.set_state(gst::State::Null);
            let _ = std::fs::remove_file(&self.output);
            return Err(RecordError::Pipeline(
                "could not send EOS to the recording pipeline".to_string(),
            ));
        }

        let message = wait_for(
            &self.bus,
            &[gst::MessageType::Eos, gst::MessageType::Error],
            STOP_TIMEOUT,
        );
        tracing::debug!(
            elapsed_ms = started.elapsed().as_millis(),
            "recording pipeline finalised"
        );
        let _ = self.pipeline.set_state(gst::State::Null);

        match message {
            Some(message) if message.type_() == gst::MessageType::Eos => {
                let bytes = std::fs::metadata(&self.output)?.len();
                if bytes == 0 {
                    let _ = std::fs::remove_file(&self.output);
                    return Err(RecordError::Pipeline(
                        "the muxer produced an empty file".to_string(),
                    ));
                }
                Ok(RecordedFile {
                    path: self.output.clone(),
                    duration,
                    bytes,
                    width: self.width,
                    height: self.height,
                    encoder: self.encoder,
                    has_audio: self.audio,
                })
            }
            Some(message) => {
                let error = pipeline_error(&message);
                let _ = std::fs::remove_file(&self.output);
                Err(error)
            }
            None => {
                let _ = std::fs::remove_file(&self.output);
                Err(RecordError::StopTimeout(STOP_TIMEOUT))
            }
        }
    }

    /// Abandons the recording and deletes the partial file.
    pub fn cancel(mut self) {
        self.finished = true;
        let _ = self.pipeline.set_state(gst::State::Null);
        let _ = std::fs::remove_file(&self.output);
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if !self.finished {
            self.finished = true;
            let _ = self.pipeline.set_state(gst::State::Null);
            let _ = std::fs::remove_file(&self.output);
        }
    }
}

/// Installs the pause probe on a pad.
///
/// While paused, buffers are dropped before they reach the encoder so a pause
/// costs no encoding work. After a resume the accumulated pause duration is
/// subtracted from buffer timestamps, keeping the output continuous.
fn install_pause_probe(pad: &gst::Pad, paused: &Arc<AtomicBool>, offset: &Arc<AtomicU64>) {
    let probe_paused = Arc::clone(paused);
    let probe_offset = Arc::clone(offset);
    pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, info| {
        if probe_paused.load(Ordering::Relaxed) {
            return gst::PadProbeReturn::Drop;
        }
        let offset = probe_offset.load(Ordering::Relaxed);
        if offset == 0 {
            return gst::PadProbeReturn::Ok;
        }
        let Some(buffer) = info.buffer_mut() else {
            return gst::PadProbeReturn::Ok;
        };
        let Some(buffer) = buffer.get_mut() else {
            return gst::PadProbeReturn::Ok;
        };
        let offset = gst::ClockTime::from_nseconds(offset);
        if let Some(pts) = buffer.pts() {
            let pts = pts.nseconds().saturating_sub(offset.nseconds());
            buffer.set_pts(gst::ClockTime::from_nseconds(pts));
        }
        if let Some(dts) = buffer.dts() {
            let dts = dts.nseconds().saturating_sub(offset.nseconds());
            buffer.set_dts(gst::ClockTime::from_nseconds(dts));
        }
        gst::PadProbeReturn::Ok
    });
}

/// Calls `gst::init`. Safe to call more than once; GStreamer is idempotent.
pub fn init() -> Result<(), RecordError> {
    gst::init().map_err(|error| RecordError::Init(error.to_string()))
}

/// The linked GStreamer runtime version.
#[must_use]
pub fn version() -> String {
    gst::version_string().to_string()
}

/// A container supported by the recording engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
    Mp4,
    Matroska,
}

impl Container {
    fn muxer(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4mux",
            Self::Matroska => "matroskamux",
        }
    }
}

fn container_for(path: &Path) -> Result<Container, RecordError> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) if extension.eq_ignore_ascii_case("mp4") => Ok(Container::Mp4),
        Some(extension)
            if extension.eq_ignore_ascii_case("mkv")
                || extension.eq_ignore_ascii_case("matroska") =>
        {
            Ok(Container::Matroska)
        }
        Some(other) => Err(RecordError::UnsupportedContainer(other.to_string())),
        None => Err(RecordError::UnsupportedContainer(
            path.display().to_string(),
        )),
    }
}

fn ensure_elements(
    source: &VideoSource,
    audio: AudioOptions,
    container: Container,
) -> Result<(), RecordError> {
    let mut required: Vec<&'static str> = vec![
        "videoconvert",
        "videorate",
        "videocrop",
        "capsfilter",
        "queue",
        container.muxer(),
        "filesink",
    ];
    match source {
        VideoSource::PipeWire(_) => required.push("pipewiresrc"),
        VideoSource::TestPattern { .. } => required.push("videotestsrc"),
    }
    if audio.microphone || audio.system {
        required.extend([
            "pulsesrc",
            "audioconvert",
            "audioresample",
            "avenc_aac",
            "aacparse",
        ]);
    }
    for name in required {
        if gst::ElementFactory::find(name).is_none() {
            return Err(RecordError::MissingElement(name));
        }
    }
    Ok(())
}

fn source_description(source: &VideoSource) -> Result<(String, Option<(i32, i32)>), RecordError> {
    match source {
        VideoSource::PipeWire(stream) => {
            let target = match stream.serial {
                Some(serial) => format!("target-object={serial}"),
                None => format!("path={}", stream.node_id),
            };
            Ok((
                format!(
                    "pipewiresrc name=src fd={} {} do-timestamp=true",
                    stream.fd, target
                ),
                stream.logical_size,
            ))
        }
        VideoSource::TestPattern { width, height } => {
            if *width < 2 || *height < 2 {
                return Err(RecordError::CropOutOfBounds);
            }
            Ok((
                format!(
                    "videotestsrc name=src is-live=true pattern=smpte \
                     ! video/x-raw,width={width},height={height},framerate=30/1"
                ),
                Some((*width, *height)),
            ))
        }
    }
}

fn build_description(
    source: &str,
    encoder: Encoder,
    fps: u32,
    bitrate_kbps: u32,
    output: &Path,
    audio: AudioOptions,
    container: Container,
) -> String {
    let location = quote_for_parse(&output.display().to_string());
    let muxer = container.muxer();
    let mut description = format!(
        "{source} ! queue max-size-buffers=8 max-size-time=1000000000 leaky=downstream \
         ! videoconvert ! videorate ! video/x-raw,framerate={fps}/1 \
         ! capsfilter name=raw \
         ! videocrop name=crop left=0 top=0 right=0 bottom=0 \
         ! queue ! {} name=encoder {} ! video/x-h264,stream-format=avc,alignment=au \
         ! {muxer} name=mux ! filesink name=sink location={location}",
        encoder.element(),
        encoder.properties(fps, bitrate_kbps),
    );
    if audio.microphone {
        description.push_str(
            " pulsesrc name=mic_src ! queue ! audioconvert ! audioresample \
             ! audio/x-raw,rate=48000,channels=2 ! avenc_aac bitrate=128000 \
             ! aacparse ! mux.",
        );
    }
    if audio.system {
        description.push_str(
            " pulsesrc name=system_src device=\"@DEFAULT_MONITOR@\" ! queue \
             ! audioconvert ! audioresample ! audio/x-raw,rate=48000,channels=2 \
             ! avenc_aac bitrate=128000 ! aacparse ! mux.",
        );
    }
    description
}

/// Quotes a filesystem path for a `gst::parse` pipeline description.
///
/// `filesink location` takes a path, not a URI, so spaces and quotes must be
/// escaped for the parser rather than percent-encoded.
fn quote_for_parse(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        if matches!(character, '"' | '\\') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('"');
    quoted
}

fn negotiated_size(pipeline: &gst::Pipeline) -> Option<(i32, i32)> {
    let element = pipeline.by_name("raw")?;
    let pad = element.static_pad("src")?;
    let caps = pad.current_caps()?;
    let structure = caps.structure(0)?;
    let width = structure.get::<i32>("width").ok()?;
    let height = structure.get::<i32>("height").ok()?;
    if width < 2 || height < 2 {
        return None;
    }
    Some((width, height))
}

fn wait_for(bus: &gst::Bus, types: &[gst::MessageType], timeout: Duration) -> Option<gst::Message> {
    let timeout = u64::try_from(timeout.as_nanos()).unwrap_or(u64::MAX);
    bus.timed_pop_filtered(gst::ClockTime::from_nseconds(timeout), types)
}

fn pipeline_error(message: &gst::Message) -> RecordError {
    match message.view() {
        gst::MessageView::Error(message) => {
            let debug = message.debug();
            let error = message.error();
            match debug {
                Some(debug) if !debug.is_empty() => {
                    RecordError::Pipeline(format!("{error} ({debug})"))
                }
                _ => RecordError::Pipeline(error.to_string()),
            }
        }
        _ => RecordError::Pipeline("unexpected pipeline message".to_string()),
    }
}

fn fail(pipeline: &gst::Pipeline, output: &Path, error: RecordError) -> RecordError {
    let _ = pipeline.set_state(gst::State::Null);
    let _ = std::fs::remove_file(output);
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoder_support_has_a_consistent_preference() {
        init().unwrap();
        let support = encoder_support();
        assert_eq!(support.preferred, support.available.first().copied());
        assert_eq!(
            support.hardware,
            support.available.contains(&Encoder::VaapiH264)
        );
    }

    #[test]
    fn x264_uses_kilobits_and_open_h264_uses_bits() {
        assert!(Encoder::X264.properties(30, 6000).contains("bitrate=6000 "));
        assert!(
            Encoder::OpenH264
                .properties(30, 6000)
                .contains("bitrate=6000000")
        );
        assert!(
            Encoder::X264
                .properties(30, 6000)
                .contains("key-int-max=60")
        );
    }

    #[test]
    fn options_are_clamped() {
        let options = RecordOptions {
            fps: 5,
            bitrate_kbps: 1,
            ..RecordOptions::new("/tmp/a.mp4")
        }
        .sanitized();
        assert_eq!(options.fps, MIN_FPS);
        assert_eq!(options.bitrate_kbps, MIN_BITRATE_KBPS);

        let options = RecordOptions {
            fps: 200,
            bitrate_kbps: 999_999,
            ..RecordOptions::new("/tmp/a.mp4")
        }
        .sanitized();
        assert_eq!(options.fps, MAX_FPS);
        assert_eq!(options.bitrate_kbps, MAX_BITRATE_KBPS);
    }

    #[test]
    fn only_mp4_and_matroska_are_accepted() {
        assert_eq!(
            container_for(Path::new("/tmp/a.mp4")).ok(),
            Some(Container::Mp4)
        );
        assert_eq!(
            container_for(Path::new("/tmp/a.MP4")).ok(),
            Some(Container::Mp4)
        );
        assert_eq!(
            container_for(Path::new("/tmp/a.mkv")).ok(),
            Some(Container::Matroska)
        );
        assert_eq!(
            container_for(Path::new("/tmp/a.MATROSKA")).ok(),
            Some(Container::Matroska)
        );
        assert!(matches!(
            container_for(Path::new("/tmp/a.webm")),
            Err(RecordError::UnsupportedContainer(_))
        ));
        assert!(matches!(
            container_for(Path::new("/tmp/a")),
            Err(RecordError::UnsupportedContainer(_))
        ));
    }

    #[test]
    fn pipeline_description_contains_source_crop_encoder_and_sink() {
        let description = build_description(
            "pipewiresrc name=src fd=7 path=42 do-timestamp=true",
            Encoder::X264,
            30,
            6000,
            Path::new("/tmp/a b.mp4"),
            AudioOptions::default(),
            Container::Mp4,
        );
        assert!(description.contains("pipewiresrc name=src fd=7 path=42"));
        assert!(description.contains("videocrop name=crop"));
        assert!(description.contains("x264enc"));
        assert!(description.contains("stream-format=avc"));
        assert!(description.contains("mp4mux name=mux"));
        assert!(description.contains("location=\"/tmp/a b.mp4\""));
        assert!(!description.contains("pulsesrc"));
    }

    #[test]
    fn matroska_uses_the_matroska_muxer() {
        let description = build_description(
            "videotestsrc name=src",
            Encoder::X264,
            30,
            6000,
            Path::new("/tmp/a.mkv"),
            AudioOptions::default(),
            Container::Matroska,
        );
        assert!(description.contains("matroskamux name=mux"));
        assert!(!description.contains("mp4mux"));
    }

    #[test]
    fn audio_branches_are_added_independently() {
        let base = "videotestsrc name=src";
        let microphone = build_description(
            base,
            Encoder::X264,
            30,
            6000,
            Path::new("/tmp/a.mp4"),
            AudioOptions {
                microphone: true,
                system: false,
            },
            Container::Mp4,
        );
        assert!(microphone.contains("pulsesrc name=mic_src"));
        assert!(!microphone.contains("@DEFAULT_MONITOR@"));

        let system = build_description(
            base,
            Encoder::X264,
            30,
            6000,
            Path::new("/tmp/a.mp4"),
            AudioOptions {
                microphone: false,
                system: true,
            },
            Container::Mp4,
        );
        assert!(system.contains("@DEFAULT_MONITOR@"));

        let both = build_description(
            base,
            Encoder::X264,
            30,
            6000,
            Path::new("/tmp/a.mp4"),
            AudioOptions {
                microphone: true,
                system: true,
            },
            Container::Mp4,
        );
        assert_eq!(both.matches("pulsesrc").count(), 2);
        assert_eq!(both.matches("mux.").count(), 2);
    }

    #[test]
    fn portal_serial_is_preferred_over_the_reusable_node_id() {
        let (description, size) = source_description(&VideoSource::PipeWire(PipeWireStream {
            fd: 9,
            node_id: 42,
            serial: Some(1234),
            logical_size: Some((1920, 1080)),
        }))
        .unwrap();
        assert!(description.contains("target-object=1234"));
        assert!(!description.contains("path=42"));
        assert_eq!(size, Some((1920, 1080)));
    }

    #[test]
    fn node_id_is_used_when_no_serial_is_available() {
        let (description, _) = source_description(&VideoSource::PipeWire(PipeWireStream {
            fd: 9,
            node_id: 42,
            serial: None,
            logical_size: None,
        }))
        .unwrap();
        assert!(description.contains("path=42"));
    }

    #[test]
    fn test_pattern_source_reports_its_size() {
        let (description, size) = source_description(&VideoSource::TestPattern {
            width: 320,
            height: 240,
        })
        .unwrap();
        assert!(description.contains("videotestsrc"));
        assert_eq!(size, Some((320, 240)));
        assert!(
            source_description(&VideoSource::TestPattern {
                width: 0,
                height: 240
            })
            .is_err()
        );
    }

    #[test]
    fn records_a_short_test_pattern_when_plugins_are_installed() {
        if init().is_err() || preferred_encoder().is_none() {
            return;
        }
        if ensure_elements(
            &VideoSource::TestPattern {
                width: 320,
                height: 240,
            },
            AudioOptions::default(),
            Container::Mp4,
        )
        .is_err()
        {
            return;
        }

        let unique = format!(
            "snipunix-record-test-{}-{}.mp4",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let output = std::env::temp_dir().join(unique);
        let recorder = Recorder::start(
            VideoSource::TestPattern {
                width: 320,
                height: 240,
            },
            RecordOptions {
                fps: 15,
                bitrate_kbps: 1000,
                ..RecordOptions::new(&output)
            },
        )
        .expect("test pattern pipeline should start");
        assert_eq!(recorder.dimensions(), (320, 240));
        std::thread::sleep(Duration::from_millis(700));
        let file = recorder.stop().expect("test pattern recording should stop");
        assert!(file.bytes > 1000, "MP4 should contain encoded frames");
        assert_eq!(file.width, 320);
        assert_eq!(file.height, 240);
        assert!(!file.has_audio);
        std::fs::remove_file(&output).unwrap();
    }

    #[test]
    fn pause_and_resume_keep_the_file_playable() {
        if init().is_err() || preferred_encoder().is_none() {
            return;
        }
        if ensure_elements(
            &VideoSource::TestPattern {
                width: 320,
                height: 240,
            },
            AudioOptions::default(),
            Container::Mp4,
        )
        .is_err()
        {
            return;
        }

        let unique = format!(
            "snipunix-record-pause-{}-{}.mp4",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let output = std::env::temp_dir().join(unique);
        let mut recorder = Recorder::start(
            VideoSource::TestPattern {
                width: 320,
                height: 240,
            },
            RecordOptions {
                fps: 15,
                bitrate_kbps: 1000,
                ..RecordOptions::new(&output)
            },
        )
        .expect("test pattern pipeline should start");
        std::thread::sleep(Duration::from_millis(600));
        recorder.pause().expect("recording should pause");
        assert_eq!(recorder.state(), RecorderState::Paused);
        std::thread::sleep(Duration::from_millis(600));
        recorder.resume().expect("recording should resume");
        assert_eq!(recorder.state(), RecorderState::Recording);
        std::thread::sleep(Duration::from_millis(600));
        let file = recorder.stop().expect("paused recording should finalise");
        assert!(file.bytes > 1000, "MP4 should contain encoded frames");
        assert!(
            file.duration >= Duration::from_millis(900)
                && file.duration < Duration::from_millis(1900),
            "paused time should be excluded from the elapsed clock: {:?}",
            file.duration
        );
        std::fs::remove_file(&output).unwrap();
    }
}
