// One recording: the frames of a display or of a window (svc-capture's Windows
// Graphics Capture session) and the sound of up to a few sources, written by a
// Media Foundation sink writer into one MP4: H.264, and one AAC track per
// source. Windows' own encoders do the work, the graphics card's when it has
// one; nothing is bundled.
//
// A recording is one thread. It owns the capture, the sound sources and the
// file from the first frame to the finished file, so nothing a page does (or
// fails to do) can leave a file unfinished: `Recording::stop` only asks, and
// the thread says `Event::Done` when the file is closed.
//
// Time is the performance counter's, which is also what the frames carry. A
// frame is written when the screen changed (never more often than the frame
// rate) and once a second while it stands still. Sound is placed by how much
// of it has arrived: while a source is silent Windows delivers nothing, and
// the gap is written as silence so that every track is as long as the picture.
//
// The rules (bit rate by picture size and quality, even sizes, the two disk
// floors) are those of the upstream file Services/Recorder/RecorderSupport.swift.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{mpsc, Arc, Mutex, Once};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use notch_core::log;
use svc_capture::{wgc, Rect};
use windows::core::{Interface, Result as WinResult, GUID, HSTRING};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer, IMFMediaType, IMFSinkWriter, IMFSinkWriterEx, MFAudioFormat_AAC, MFAudioFormat_PCM, MFCreateAttributes,
    MFCreateDXGIDeviceManager, MFCreateDXGISurfaceBuffer, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateSinkWriterFromURL, MFMediaType_Audio, MFMediaType_Video, MFShutdown, MFStartup, MFVideoFormat_ARGB32,
    MFVideoFormat_H264, MFVideoInterlace_Progressive, MFSTARTUP_FULL, MFT_CATEGORY_VIDEO_ENCODER,
    MFT_CATEGORY_VIDEO_PROCESSOR, MFT_ENUM_HARDWARE_URL_Attribute, MFT_ENUM_HARDWARE_VENDOR_ID_Attribute,
    MF_MT_ALL_SAMPLES_INDEPENDENT, MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
    MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
    MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SINK_WRITER_D3D_MANAGER,
    MF_SINK_WRITER_DISABLE_THROTTLING, MF_SINK_WRITER_STATISTICS, MF_VERSION, MF_XVP_DISABLE_FRC,
};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::Com::CoIncrementMTAUsage;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

/// Frames of sound a second, on every track: what Windows converts to and what AAC takes.
pub const RATE: u32 = 48_000;
/// Media Foundation counts in 100 ns.
const SECOND: i64 = 10_000_000;
/// A recording does not begin with less room than this on the disk…
const START_FLOOR: u64 = 1_000_000_000;
/// …and ends by itself, with a finished file, when less than this is left.
const STOP_FLOOR: u64 = 500_000_000;
/// Smaller than this no encoder is sure to take.
const MIN_SIDE: u32 = 64;
/// How far a track may run behind the clock before the gap is called silence (0.1 s).
const SLACK: u64 = RATE as u64 / 10;
/// How many pictures the encoder may have waiting before new ones are left out.
const BACKLOG: u64 = 30;
/// AAC at 160 kbit/s, in bytes a second.
const AAC_BYTES: u32 = 20_000;

// ── What a recording is made of ───────────────────────────────────────────────

/// A source of sound, on the recording's thread.
pub trait Sound {
    fn rate(&self) -> u32;
    fn channels(&self) -> u16;
    /// Appends what has arrived since the last call (interleaved, -1 to 1) and
    /// says how many frames that was. Never waits. An error means the device
    /// went away: the track is silent until it opens again.
    fn read(&mut self, out: &mut Vec<f32>) -> Result<usize, String>;
}

impl Sound for svc_audio::Loopback {
    fn rate(&self) -> u32 {
        svc_audio::Loopback::rate(self)
    }
    fn channels(&self) -> u16 {
        svc_audio::Loopback::channels(self)
    }
    fn read(&mut self, out: &mut Vec<f32>) -> Result<usize, String> {
        svc_audio::Loopback::read(self, out, Duration::ZERO)
    }
}

/// One sound track of the file. `open` runs on the recording's thread, at the
/// start and again whenever the source went away.
pub struct Track {
    pub name: &'static str,
    pub open: Box<dyn FnMut() -> Result<Box<dyn Sound>, String> + Send>,
}

/// What the computer plays, as it plays it.
pub fn system_sound() -> Track {
    Track { name: "system", open: Box::new(|| Ok(Box::new(svc_audio::loopback(None)?) as Box<dyn Sound>)) }
}

#[derive(Debug, Clone, Copy)]
pub enum Target {
    /// This rectangle of the desktop (physical pixels), on the display that holds it: a whole display or a part of one.
    Display(Rect),
    Window(isize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    Small,
    Balanced,
    High,
}

impl Quality {
    /// Unknown names are Balanced.
    pub fn named(name: &str) -> Self {
        match name {
            "small" => Quality::Small,
            "high" => Quality::High,
            _ => Quality::Balanced,
        }
    }

    /// Bits for one pixel of one frame. A screen is mostly flat: the encoder
    /// stays far under this and only the busy moments reach it.
    fn bits_per_pixel(self) -> f64 {
        match self {
            Quality::Small => 0.05,
            Quality::Balanced => 0.06,
            Quality::High => 0.09,
        }
    }

    /// Only Small gives up pixels.
    fn scale(self) -> f64 {
        if self == Quality::Small { 0.5 } else { 1.0 }
    }
}

/// The ceiling the encoder is given: floored so that a small area still gets a
/// usable picture, capped at what Windows' encoders sustain.
pub fn average_bit_rate(w: u32, h: u32, fps: u32, quality: Quality) -> u32 {
    (f64::from(w.max(1)) * f64::from(h.max(1)) * f64::from(fps.max(1)) * quality.bits_per_pixel()).clamp(800_000.0, 60_000_000.0).round() as u32
}

/// Encoders refuse odd sizes: one pixel less rather than one the user did not choose.
fn even(side: f64) -> u32 {
    (side.max(0.0) as u32) & !1
}

pub struct Options {
    pub target: Target,
    /// The file to write; its folder exists.
    pub path: PathBuf,
    /// 30 or 60.
    pub fps: u32,
    pub quality: Quality,
    pub pointer: bool,
    pub border: bool,
    /// Windows' own encoder even where the graphics card has one. It is also
    /// what a recording falls back on when the card's encoder refuses the picture.
    pub software: bool,
    pub tracks: Vec<Track>,
}

/// Why a recording ended. Whatever the reason, the file is finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    User,
    /// The disk ran low.
    Disk,
    /// The window that was recorded closed.
    Closed,
    Lock,
    Sleep,
    /// The feature was switched off or the app quit.
    Off,
    /// Windows refused a frame or a piece of sound; what was written until then is kept.
    Failed,
}

impl Stop {
    pub fn name(self) -> &'static str {
        match self {
            Stop::User => "user",
            Stop::Disk => "disk",
            Stop::Closed => "closed",
            Stop::Lock => "lock",
            Stop::Sleep => "sleep",
            Stop::Off => "off",
            Stop::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Done {
    pub path: PathBuf,
    pub seconds: f64,
    pub bytes: u64,
    /// Pictures written.
    pub frames: u64,
    pub why: Stop,
}

pub enum Event {
    /// A track has no source (the microphone is unplugged or refused): it is
    /// silent until the source opens. Said once per track.
    Silent { track: &'static str, why: String },
    /// The file is closed, or could not be.
    Done(Result<Done, String>),
}

/// What was agreed with Windows when the recording began.
#[derive(Debug, Clone, Default)]
pub struct Info {
    /// The picture in the file.
    pub w: u32,
    pub h: u32,
    pub bit_rate: u32,
    /// The encoder is the graphics card's, and whose card that is as Windows
    /// names it ("VEN_8086"); neither for Windows' own encoder.
    pub hardware: bool,
    pub vendor: String,
}

#[derive(Default)]
struct Ctl {
    /// Why the recording is to end, and the clock when that was asked: the film
    /// ends there, however long the thread takes to hear of it (a computer
    /// that goes to sleep may only let it run again hours later).
    stop: Mutex<Option<(Stop, i64)>>,
    paused: AtomicBool,
    /// Milliseconds recorded so far, pauses left out.
    elapsed: AtomicU64,
}

/// A recording that runs.
pub struct Recording {
    ctl: Arc<Ctl>,
    thread: Option<JoinHandle<()>>,
    pub info: Info,
}

impl Recording {
    /// Begins: comes back when the first picture is in the file, or with why
    /// there is no recording. `tell` is called on the recording's thread.
    pub fn start(options: Options, tell: impl Fn(Event) + Send + 'static) -> Result<Recording, String> {
        let folder = options.path.parent().unwrap_or(Path::new("."));
        if free_bytes(folder).is_some_and(|free| free < START_FLOOR) {
            return Err("Less than 1 GB is free on that disk.".into());
        }
        let ctl = Arc::new(Ctl::default());
        let inner = ctl.clone();
        let (ready, began) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("recorder".into())
            .spawn(move || run(options, &inner, ready, &tell))
            .map_err(|e| e.to_string())?;
        match began.recv().unwrap_or_else(|_| Err("The recording could not start.".into())) {
            Ok(info) => Ok(Recording { ctl, thread: Some(thread), info }),
            Err(why) => {
                let _ = thread.join();
                Err(why)
            }
        }
    }

    /// Asks the recording to end; `Event::Done` follows when the file is closed. The first reason is the one that counts.
    pub fn stop(&self, why: Stop) {
        self.ctl.stop.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert((why, clock()));
    }

    /// While paused nothing is written, and the pause is not in the file.
    pub fn pause(&self, on: bool) {
        self.ctl.paused.store(on, Relaxed);
    }

    pub fn paused(&self) -> bool {
        self.ctl.paused.load(Relaxed)
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.ctl.elapsed.load(Relaxed)
    }

    /// The thread, to wait for the finished file after `stop`.
    pub fn thread(&mut self) -> Option<JoinHandle<()>> {
        self.thread.take()
    }
}

// ── The recording's thread ────────────────────────────────────────────────────

fn run(options: Options, ctl: &Ctl, ready: mpsc::Sender<Result<Info, String>>, tell: &dyn Fn(Event)) {
    // The multithreaded apartment is kept for good from the first recording (a
    // count, not a thread). Entered and left with this thread, it would die
    // with the thread when no other is in it, under the feet of whoever uses
    // Windows' media objects at that moment, and with it the factories
    // windows-rs remembers for the next recording.
    static KEEP: Once = Once::new();
    KEEP.call_once(|| unsafe {
        let _ = CoIncrementMTAUsage();
    });
    if let Err(err) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) } {
        let _ = ready.send(Err(format!("Windows' media encoders are not available: {}", err.message())));
        return;
    }
    match Take::begin(options) {
        Err(why) => {
            let _ = ready.send(Err(why));
        }
        Ok(mut take) => {
            let _ = ready.send(Ok(take.info.clone()));
            let (why, when) = take.run(ctl, tell);
            tell(Event::Done(take.finish(why, when)));
        }
    }
    unsafe {
        let _ = MFShutdown();
    }
}

/// The performance counter, in 100 ns: the clock the frames are stamped with.
fn clock() -> i64 {
    let (mut ticks, mut per_second) = (0i64, 0i64);
    unsafe {
        let _ = QueryPerformanceCounter(&mut ticks);
        let _ = QueryPerformanceFrequency(&mut per_second);
    }
    (i128::from(ticks) * i128::from(SECOND) / i128::from(per_second.max(1))) as i64
}

fn free_bytes(folder: &Path) -> Option<u64> {
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(&HSTRING::from(folder.as_os_str()), Some(&mut free), None, None) }.ok().map(|_| free)
}

/// Two numbers in one attribute, as Media Foundation packs a size or a ratio.
fn pair(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

unsafe fn media_type(major: &GUID, sub: &GUID) -> WinResult<IMFMediaType> {
    unsafe {
        let kind = MFCreateMediaType()?;
        kind.SetGUID(&MF_MT_MAJOR_TYPE, major)?;
        kind.SetGUID(&MF_MT_SUBTYPE, sub)?;
        Ok(kind)
    }
}

unsafe fn video_type(sub: &GUID, size: (u32, u32), fps: u32) -> WinResult<IMFMediaType> {
    unsafe {
        let kind = media_type(&MFMediaType_Video, sub)?;
        kind.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        kind.SetUINT64(&MF_MT_FRAME_SIZE, pair(size.0, size.1))?;
        kind.SetUINT64(&MF_MT_FRAME_RATE, pair(fps, 1))?;
        kind.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pair(1, 1))?;
        Ok(kind)
    }
}

unsafe fn sound_type(sub: &GUID, bytes_a_second: u32) -> WinResult<IMFMediaType> {
    unsafe {
        let kind = media_type(&MFMediaType_Audio, sub)?;
        kind.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        kind.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
        kind.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        kind.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, bytes_a_second)?;
        Ok(kind)
    }
}

/// The file's writer with its streams, ready for samples: the video's stream,
/// one for each track, and what `transforms` found. BGRA textures of `from` go
/// in, H.264 of `to` comes out; Windows converts and scales on the graphics
/// card in between.
unsafe fn sink(path: &Path, device: &ID3D11Device, from: (u32, u32), to: (u32, u32), fps: u32, bit_rate: u32, tracks: usize, hardware: bool) -> WinResult<(IMFSinkWriter, u32, Vec<u32>, (String, bool))> {
    unsafe {
        let (mut token, mut manager) = (0u32, None);
        MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
        let manager = manager.ok_or_else(windows::core::Error::empty)?;
        manager.ResetDevice(device, token)?;
        let mut how = None;
        MFCreateAttributes(&mut how, 3)?;
        let how = how.ok_or_else(windows::core::Error::empty)?;
        how.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, u32::from(hardware))?;
        // Throttling holds a stream back until the others have caught up with it:
        // with one thread feeding them all, and a picture that may stand still, that is a stall.
        how.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
        how.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &manager)?;
        let writer = MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, &how)?;

        let coded = video_type(&MFVideoFormat_H264, to, fps)?;
        coded.SetUINT32(&MF_MT_AVG_BITRATE, bit_rate)?;
        let video = writer.AddStream(&coded)?;
        writer.SetInputMediaType(video, &video_type(&MFVideoFormat_ARGB32, from, fps)?, None)?;

        let mut sounds = Vec::new();
        for _ in 0..tracks {
            let stream = writer.AddStream(&sound_type(&MFAudioFormat_AAC, AAC_BYTES)?)?;
            let plain = sound_type(&MFAudioFormat_PCM, RATE * 4)?;
            plain.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
            plain.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
            writer.SetInputMediaType(stream, &plain, None)?;
            sounds.push(stream);
        }
        // Before writing begins: the converter reads its orders when it starts.
        let encoder = transforms(&writer, video);
        writer.BeginWriting()?;
        Ok((writer, video, sounds, encoder))
    }
}

/// Looks at what Windows put on the video stream. Its converter must not fill
/// the time between two pictures with copies: a screen that stands still is one
/// picture, not sixty a second. Its encoder says whether it is the graphics
/// card's, and whose card.
fn transforms(writer: &IMFSinkWriter, stream: u32) -> (String, bool) {
    let mut found = (String::new(), false);
    let Ok(writer) = writer.cast::<IMFSinkWriterEx>() else { return found };
    for index in 0.. {
        let (mut kind, mut transform) = (GUID::zeroed(), None);
        if unsafe { writer.GetTransformForStream(stream, index, Some(&mut kind), &mut transform) }.is_err() {
            break;
        }
        let Some(about) = transform.and_then(|transform| unsafe { transform.GetAttributes() }.ok()) else { continue };
        unsafe {
            if kind == MFT_CATEGORY_VIDEO_PROCESSOR {
                let _ = about.SetUINT32(&MF_XVP_DISABLE_FRC, 1);
            }
            if kind == MFT_CATEGORY_VIDEO_ENCODER {
                let (mut vendor, mut length) = ([0u16; 64], 0u32);
                let _ = about.GetString(&MFT_ENUM_HARDWARE_VENDOR_ID_Attribute, &mut vendor, Some(&mut length));
                found = (String::from_utf16_lossy(&vendor[..length as usize]), about.GetStringLength(&MFT_ENUM_HARDWARE_URL_Attribute).is_ok());
            }
        }
    }
    found
}

/// One sound track while it is recorded.
struct Voice {
    track: Track,
    stream: u32,
    sound: Option<Box<dyn Sound>>,
    channels: usize,
    /// When to try to open the source (again).
    retry: Instant,
    /// The page was told that this track is silent.
    told: bool,
    /// Frames in the file so far.
    written: u64,
    /// The last thing written was silence: sound that arrives is placed exactly.
    filled: bool,
    heard: Vec<f32>,
    plain: Vec<i16>,
}

impl Voice {
    fn new(track: Track, stream: u32) -> Self {
        Voice { track, stream, sound: None, channels: 2, retry: Instant::now(), told: false, written: 0, filled: true, heard: Vec::new(), plain: Vec::new() }
    }

    /// Takes what the source has and writes it where it belongs; `now` is the
    /// recording's clock in frames. With `drop` (a pause) it is read and thrown away.
    fn pump(&mut self, now: u64, drop: bool, writer: &IMFSinkWriter, tell: &dyn Fn(Event)) -> WinResult<()> {
        if self.sound.is_none() && Instant::now() >= self.retry {
            let opened = (self.track.open)().and_then(|sound| {
                // debt: a source Windows does not convert to 48 kHz stays silent; a resampler would take it.
                if sound.rate() == RATE && sound.channels() > 0 { Ok(sound) } else { Err(format!("{} Hz is not a rate that can be recorded", sound.rate())) }
            });
            match opened {
                Ok(sound) => {
                    self.channels = usize::from(sound.channels());
                    self.sound = Some(sound);
                }
                Err(why) => {
                    self.retry = Instant::now() + Duration::from_secs(2);
                    if !self.told {
                        self.told = true;
                        tell(Event::Silent { track: self.track.name, why });
                    }
                }
            }
        }
        self.heard.clear();
        let mut frames = 0u64;
        if let Some(sound) = &mut self.sound {
            match sound.read(&mut self.heard) {
                Ok(count) => frames = count as u64,
                // Unplugged, or its format changed: opened again in a moment, silence until then.
                Err(why) => {
                    log::line(format!("recorder: the {} sound stopped: {why}", self.track.name));
                    self.sound = None;
                    self.retry = Instant::now() + Duration::from_secs(1);
                }
            }
        }
        if drop {
            return Ok(());
        }
        // What just arrived ends about now.
        let start = now.saturating_sub(frames);
        if frames == 0 {
            // Nothing plays: the track follows the clock a little behind, as silence.
            if start > self.written + 2 * SLACK {
                self.silence(start - SLACK, writer)?;
            }
            return Ok(());
        }
        if (self.filled && start > self.written) || start > self.written + SLACK {
            self.silence(start, writer)?;
        }
        // Stereo, 16 bits: the left and the right of whatever the source has.
        self.plain.clear();
        let right = self.channels.min(2) - 1;
        for frame in self.heard.chunks_exact(self.channels) {
            for sample in [frame[0], frame[right]] {
                self.plain.push((sample.clamp(-1.0, 1.0) * 32767.0) as i16);
            }
        }
        self.filled = false;
        self.write(writer)
    }

    /// Silence up to the frame `to`.
    fn silence(&mut self, to: u64, writer: &IMFSinkWriter) -> WinResult<()> {
        while self.written < to {
            let frames = (to - self.written).min(u64::from(RATE)) as usize;
            self.plain.clear();
            self.plain.resize(frames * 2, 0);
            self.write(writer)?;
        }
        self.filled = true;
        Ok(())
    }

    /// `plain` into the file, after what is already there.
    fn write(&mut self, writer: &IMFSinkWriter) -> WinResult<()> {
        let (frames, bytes) = ((self.plain.len() / 2) as u64, self.plain.len() * 2);
        if frames == 0 {
            return Ok(());
        }
        let at = |frame: u64| (frame * SECOND as u64 / u64::from(RATE)) as i64;
        unsafe {
            let buffer = MFCreateMemoryBuffer(bytes as u32)?;
            let mut into = std::ptr::null_mut();
            buffer.Lock(&mut into, None, None)?;
            std::ptr::copy_nonoverlapping(self.plain.as_ptr().cast::<u8>(), into, bytes);
            buffer.Unlock()?;
            buffer.SetCurrentLength(bytes as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(at(self.written))?;
            sample.SetSampleDuration(at(self.written + frames) - at(self.written))?;
            writer.WriteSample(self.stream, &sample)?;
        }
        self.written += frames;
        Ok(())
    }
}

struct Take {
    // Released in this order: the writer and its textures before the capture and its device.
    writer: IMFSinkWriter,
    /// The last picture written: it goes in again while the screen stands still and at the end.
    last: Option<ID3D11Texture2D>,
    session: wgc::Session,
    voices: Vec<Voice>,
    info: Info,
    path: PathBuf,
    video: u32,
    /// The part of a frame that is recorded.
    cut: D3D11_BOX,
    /// The shortest time between two pictures.
    interval: i64,
    /// The clock at the first picture; pauses that are over, and the one that runs.
    origin: i64,
    paused: i64,
    pause_at: Option<i64>,
    /// The file's time of the last picture, and the earliest of the next.
    last_at: i64,
    due: i64,
    frames: u64,
    /// Pictures that were not written because the encoder was behind.
    left_out: u64,
    error: Option<String>,
}

impl Take {
    fn begin(options: Options) -> Result<Take, String> {
        let source = match options.target {
            Target::Display(rect) => wgc::Source::Display(rect),
            Target::Window(hwnd) => wgc::Source::Window(hwnd),
        };
        let session = wgc::session(source, wgc::Options { pointer: options.pointer, border: options.border })
            .map_err(|why| format!("Windows could not capture that: {why}"))?;
        let area = session.area();
        // In pixels of a frame: the part of the display that was asked for, or the whole window.
        let pick = match options.target {
            Target::Display(rect) => {
                let inside = rect.clip(area);
                Rect { x: inside.x - area.x, y: inside.y - area.y, ..inside }
            }
            Target::Window(_) => Rect { x: 0, y: 0, ..area },
        };
        let (w, h) = (even(f64::from(pick.w)), even(f64::from(pick.h)));
        if w < MIN_SIDE || h < MIN_SIDE {
            return Err("That is too small to record.".into());
        }
        let scale = options.quality.scale();
        let out = (even(f64::from(w) * scale).max(MIN_SIDE), even(f64::from(h) * scale).max(MIN_SIDE));
        let fps = if options.fps == 30 { 30 } else { 60 };
        let bit_rate = average_bit_rate(out.0, out.1, fps, options.quality);
        let sink = |hardware: bool| unsafe { sink(&options.path, session.device(), (w, h), out, fps, bit_rate, options.tracks.len(), hardware) };
        let made = if options.software { sink(false) } else { sink(true).or_else(|_| sink(false)) };
        let (writer, video, streams, (vendor, hardware)) = made.map_err(|err| {
            let _ = std::fs::remove_file(&options.path);
            format!("Windows could not start the video file: {}", err.message())
        })?;
        let (left, top) = (pick.x as u32, pick.y as u32);
        let mut take = Take {
            writer,
            last: None,
            session,
            voices: options.tracks.into_iter().zip(streams).map(|(track, stream)| Voice::new(track, stream)).collect(),
            info: Info { w: out.0, h: out.1, bit_rate, hardware, vendor },
            path: options.path,
            video,
            cut: D3D11_BOX { left, top, front: 0, right: left + w, bottom: top + h, back: 1 },
            interval: SECOND / i64::from(fps),
            origin: 0,
            paused: 0,
            pause_at: None,
            last_at: -1,
            due: 0,
            frames: 0,
            left_out: 0,
            error: None,
        };
        // The recording begins now, with the screen as it is now: the frames that
        // came while the encoder was being set up are older than that.
        let first = take.session.frame(Duration::from_secs(3)).ok_or("Windows did not hand over the picture.").and_then(|mut frame| {
            while let Some(newer) = take.session.frame(Duration::ZERO) {
                frame = newer;
            }
            take.origin = clock();
            take.picture(&frame.texture, 0).map_err(|_| "Windows could not encode the picture.")
        });
        if let Err(why) = first {
            let path = take.path.clone();
            drop(take);
            let _ = std::fs::remove_file(path);
            return Err(why.into());
        }
        Ok(take)
    }

    /// The file's time at `clock`: what has passed since the first picture, pauses left out.
    fn at(&self, clock: i64) -> i64 {
        clock - self.origin - self.paused
    }

    /// A copy of the recorded part of a frame into the file at `at`. Windows
    /// takes the frame's own texture back at once, and the encoder works later.
    fn picture(&mut self, frame: &ID3D11Texture2D, at: i64) -> WinResult<()> {
        let like = D3D11_TEXTURE2D_DESC {
            Width: self.cut.right - self.cut.left,
            Height: self.cut.bottom - self.cut.top,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        // debt: a texture per picture; a pool that takes them back from the encoder would spare the allocations.
        let mut copy = None;
        unsafe {
            self.session.device().CreateTexture2D(&like, None, Some(&mut copy))?;
            let copy = copy.ok_or_else(windows::core::Error::empty)?;
            self.session.context().CopySubresourceRegion(&copy, 0, 0, 0, 0, frame, 0, Some(&self.cut));
            self.write(&copy, at)?;
            self.last = Some(copy);
        }
        Ok(())
    }

    fn write(&mut self, texture: &ID3D11Texture2D, at: i64) -> WinResult<()> {
        unsafe {
            let buffer = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, texture, 0, false)?;
            buffer.SetCurrentLength(buffer.cast::<IMF2DBuffer>()?.GetContiguousLength()?)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(at)?;
            sample.SetSampleDuration(self.interval)?;
            self.writer.WriteSample(self.video, &sample)?;
        }
        self.last_at = at;
        self.frames += 1;
        Ok(())
    }

    /// The encoder has more than half a second of pictures waiting: one more
    /// would only add to what it holds in memory. Nothing else bounds that,
    /// since the writer never makes this thread wait.
    fn behind(&self) -> bool {
        let mut count = MF_SINK_WRITER_STATISTICS { cb: size_of::<MF_SINK_WRITER_STATISTICS>() as u32, ..Default::default() };
        unsafe { self.writer.GetStatistics(self.video, &mut count) }.is_ok() && count.qwNumSamplesReceived.saturating_sub(count.qwNumSamplesEncoded) > BACKLOG
    }

    /// One turn: the frame that came, if any, and what the sound sources have.
    fn turn(&mut self, frame: Option<wgc::Frame>, now: i64, paused: bool, tell: &dyn Fn(Event)) -> WinResult<()> {
        if !paused {
            match frame {
                Some(frame) => {
                    let at = self.at(frame.time);
                    // Half a picture of grace: the screen's own beat is never exact, and
                    // `due` moves on by whole pictures, so the rate is kept all the same.
                    if at > self.last_at && at + self.interval / 2 >= self.due {
                        self.due = if at > self.due + self.interval { at + self.interval } else { self.due + self.interval };
                        if self.behind() {
                            self.left_out += 1;
                        } else {
                            self.picture(&frame.texture, at)?;
                        }
                    }
                }
                // A still screen: the same picture once a second keeps the file's tracks side by side.
                None if self.at(now) - self.last_at >= SECOND => {
                    if let Some(last) = self.last.clone() {
                        self.write(&last, self.at(now))?;
                    }
                }
                None => {}
            }
        }
        let frames = (self.at(now).max(0) as u64) * u64::from(RATE) / SECOND as u64;
        let writer = self.writer.clone();
        self.voices.iter_mut().try_for_each(|voice| voice.pump(frames, paused, &writer, tell))
    }

    /// Records until asked to stop or until something ends it; says which, and when on the clock.
    fn run(&mut self, ctl: &Ctl, tell: &dyn Fn(Event)) -> (Stop, i64) {
        let folder = self.path.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut checked = Instant::now();
        loop {
            if let Some(asked) = *ctl.stop.lock().unwrap_or_else(|e| e.into_inner()) {
                return asked;
            }
            if self.session.closed() {
                return (Stop::Closed, clock());
            }
            // Frames wake the thread; the wait is how often the sound is fetched.
            let frame = self.session.frame(Duration::from_millis(10));
            let now = clock();
            let paused = ctl.paused.load(Relaxed);
            match (paused, self.pause_at) {
                (true, None) => self.pause_at = Some(now),
                (false, Some(since)) => {
                    self.paused += now - since;
                    self.pause_at = None;
                }
                _ => {}
            }
            if let Err(err) = self.turn(frame, now, paused, tell) {
                self.error = Some(err.message());
                return (Stop::Failed, now);
            }
            ctl.elapsed.store((self.at(self.pause_at.unwrap_or(now)).max(0) / 10_000) as u64, Relaxed);
            if checked.elapsed() >= Duration::from_secs(1) {
                checked = Instant::now();
                if free_bytes(&folder).is_some_and(|free| free < STOP_FLOOR) {
                    return (Stop::Disk, now);
                }
            }
        }
    }

    /// Closes the file: the last picture once more at the very end, every track
    /// filled up to it, and the writer's own finish.
    fn finish(mut self, why: Stop, when: i64) -> Result<Done, String> {
        if let Some(error) = &self.error {
            log::line(format!("recorder: stopped by an error: {error}"));
        }
        if self.left_out > 0 {
            log::line(format!("recorder: {} pictures were left out, the encoder could not keep up", self.left_out));
        }
        let end = self.at(self.pause_at.unwrap_or(when)).max(self.last_at);
        let writer = self.writer.clone();
        let tail = (|| -> WinResult<()> {
            if let Some(last) = self.last.clone().filter(|_| end > self.last_at) {
                self.write(&last, end)?;
            }
            let frames = end as u64 * u64::from(RATE) / SECOND as u64;
            for voice in &mut self.voices {
                voice.pump(frames, false, &writer, &|_| {})?;
                voice.silence(frames, &writer)?;
            }
            Ok(())
        })();
        // After an error the tail may be refused as well: what is in the file is still closed.
        if let (Err(err), true) = (&tail, why != Stop::Failed) {
            log::line(format!("recorder: the end of the recording was not written: {}", err.message()));
        }
        let closed = unsafe { writer.Finalize() };
        let (path, frames) = (self.path.clone(), self.frames);
        drop(writer);
        drop(self);
        match closed {
            Ok(()) => Ok(Done { bytes: std::fs::metadata(&path).map_or(0, |m| m.len()), path, seconds: end as f64 / SECOND as f64, frames, why }),
            Err(err) => {
                let _ = std::fs::remove_file(&path);
                Err(format!("The recording could not be saved: {}", err.message()))
            }
        }
    }
}
