use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

use crate::media;
use crate::project::Clip;

const FRAME_RATE: u16 = 25;
const FRAME_TIME: u64 = 1_000_000 / FRAME_RATE as u64;
const MAX_SKIPPED_FRAMES: u64 = 6;

pub struct Frame {
    pub texture: TextureHandle,
    pub timestamp: u64,
    size: [u16; 2],
    rgb: Vec<u8>,
}

impl Frame {
    pub fn save_region(&self, rect: [f32; 4], path: &Path) -> Result<(), String> {
        let [left, top, right, bottom] = rect;
        let [width, height] = self.size;
        let (left, right) = pixel_bounds(left, right, width)?;
        let (top, bottom) = pixel_bounds(top, bottom, height)?;
        let row_size = usize::from(right - left) * 3;
        let mut pixels = Vec::with_capacity(row_size * usize::from(bottom - top));
        for row in top..bottom {
            let offset = (usize::from(row) * usize::from(width) + usize::from(left)) * 3;
            pixels.extend_from_slice(&self.rgb[offset..offset + row_size]);
        }
        image::save_buffer_with_format(
            path,
            &pixels,
            u32::from(right - left),
            u32::from(bottom - top),
            image::ColorType::Rgb8,
            image::ImageFormat::Png,
        )
        .map_err(|error| {
            format!(
                "Cannot save preview selection to {}. {error}",
                path.display()
            )
        })
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn pixel_bounds(min: f32, max: f32, extent: u16) -> Result<(u16, u16), String> {
    if !min.is_finite() || !max.is_finite() || min >= max {
        return Err("Select a region of the preview.".to_owned());
    }
    let extent = f32::from(extent);
    let min = (min.clamp(0.0, 1.0) * extent).floor() as u16;
    let max = (max.clamp(0.0, 1.0) * extent).ceil() as u16;
    if min == max {
        return Err("Select a region of the preview.".to_owned());
    }
    Ok((min, max))
}

pub struct Preview {
    clip: Option<Clip>,
    position: u64,
    revision: u64,
    playback: Playback,
    frame: Option<Frame>,
    requested_frame: Option<u64>,
    video_error: Option<String>,
    audio_warning: Option<String>,
    video_requests: Sender<VideoRequest>,
    video_updates: Receiver<VideoUpdate>,
    audio_requests: Sender<AudioRequest>,
    audio_updates: Receiver<AudioUpdate>,
}

enum Playback {
    Paused,
    Starting,
    WaitingForAudio,
    Playing {
        since: Instant,
        start: u64,
        audio: Option<AudioPlayback>,
    },
}

impl Preview {
    pub fn new() -> Self {
        let (video_requests, video_input) = mpsc::channel();
        let (video_output, video_updates) = mpsc::channel();
        thread::spawn(move || decode_video(&video_input, &video_output));
        let (audio_requests, audio_input) = mpsc::channel();
        let (audio_output, audio_updates) = mpsc::channel();
        thread::spawn(move || prepare_audio(&audio_input, &audio_output));
        Self {
            clip: None,
            position: 0,
            revision: 0,
            playback: Playback::Paused,
            frame: None,
            requested_frame: None,
            video_error: None,
            audio_warning: None,
            video_requests,
            video_updates,
            audio_requests,
            audio_updates,
        }
    }

    pub fn set_clip(&mut self, clip: Option<&Clip>) {
        self.clip = clip.cloned();
        self.seek(0);
    }

    pub fn seek(&mut self, position: u64) {
        self.playback = Playback::Paused;
        self.position = position.min(self.duration());
        self.revision = self.revision.wrapping_add(1);
        self.frame = None;
        self.requested_frame = None;
        self.video_error = None;
        self.audio_warning = None;
    }

    pub fn pause(&mut self) {
        if self.is_playing() {
            self.playback = Playback::Paused;
            self.revision = self.revision.wrapping_add(1);
            self.requested_frame = self.frame.as_ref().map(|frame| frame.timestamp);
            if let Some(frame) = &self.frame {
                self.position = frame.timestamp;
            }
        }
    }

    pub fn toggle_playback(&mut self) {
        if self.is_playing() {
            self.pause();
        } else if self.clip.is_some() {
            if self.position == self.duration() {
                self.seek(0);
            }
            self.audio_warning = None;
            self.playback = Playback::Starting;
        }
    }

    pub const fn position(&self) -> u64 {
        self.position
    }

    pub const fn is_playing(&self) -> bool {
        !matches!(self.playback, Playback::Paused)
    }

    pub const fn frame(&self) -> Option<&Frame> {
        self.frame.as_ref()
    }

    pub fn error(&self) -> Option<&str> {
        self.video_error.as_deref()
    }

    pub fn audio_warning(&self) -> Option<&str> {
        self.audio_warning.as_deref()
    }

    pub fn poll(&mut self, context: &egui::Context) {
        self.poll_audio(context);
        if let Some(update) = self.video_updates.try_iter().last()
            && update.revision == self.revision
        {
            match update.result {
                Ok(raw) => {
                    let size = raw.size.map(usize::from);
                    let image = ColorImage::from_rgb(size, &raw.rgb);
                    let texture = if let Some(previous) = self.frame.as_mut() {
                        previous.texture.set(image, TextureOptions::LINEAR);
                        previous.texture.clone()
                    } else {
                        context.load_texture("video preview", image, TextureOptions::LINEAR)
                    };
                    self.frame = Some(Frame {
                        texture,
                        timestamp: update.timestamp,
                        size: raw.size,
                        rgb: raw.rgb,
                    });
                    self.video_error = None;
                }
                Err(error) => self.video_error = Some(error),
            }
        }
        self.request_frame(context);
    }

    fn duration(&self) -> u64 {
        self.clip.as_ref().map_or(0, Clip::duration)
    }

    fn request_frame(&mut self, context: &egui::Context) {
        let Some(clip) = &self.clip else {
            return;
        };
        let target = self.position.min(clip.duration() - 1);
        let mut position = 0;
        for span in &clip.spans {
            let end = position + (span.end - span.start);
            if target < end {
                let number = (target - position) / FRAME_TIME;
                let timestamp = position + number * FRAME_TIME;
                if Some(timestamp) != self.requested_frame {
                    self.requested_frame = Some(timestamp);
                    if self
                        .video_requests
                        .send(VideoRequest {
                            context: context.clone(),
                            revision: self.revision,
                            timestamp,
                            path: span.path.clone(),
                            start: span.start,
                            number,
                        })
                        .is_err()
                    {
                        self.video_error = Some("The preview decoder stopped.".to_owned());
                    }
                }
                break;
            }
            position = end;
        }
    }

    fn poll_audio(&mut self, context: &egui::Context) {
        if matches!(self.playback, Playback::Starting)
            && let Some(clip) = &self.clip
        {
            let request = AudioRequest {
                context: context.clone(),
                revision: self.revision,
                clip: clip.clone(),
                start: self.position,
            };
            if self.audio_requests.send(request).is_ok() {
                self.playback = Playback::WaitingForAudio;
            } else {
                self.start_playback(Err("The audio preview worker stopped.".to_owned()));
            }
        }
        if let Some(update) = self.audio_updates.try_iter().last()
            && update.revision == self.revision
            && matches!(self.playback, Playback::WaitingForAudio)
        {
            let audio = update
                .result
                .and_then(|command| command.map(AudioPlayback::start).transpose());
            self.start_playback(audio);
        }
        let duration = self.duration();
        if let Playback::Playing {
            since,
            start,
            audio,
        } = &mut self.playback
        {
            let elapsed = u64::try_from(since.elapsed().as_micros()).unwrap_or(u64::MAX);
            self.position = start.saturating_add(elapsed).min(duration);
            if self.position == duration {
                self.playback = Playback::Paused;
            } else {
                if let Some(running) = audio
                    && let Some(warning) = running.error(duration - self.position)
                {
                    self.audio_warning = Some(warning);
                    *audio = None;
                }
                context.request_repaint_after(Duration::from_millis(16));
            }
        }
    }

    fn start_playback(&mut self, audio: Result<Option<AudioPlayback>, String>) {
        let audio = match audio {
            Ok(audio) => audio,
            Err(error) => {
                self.audio_warning = Some(error);
                None
            }
        };
        self.playback = Playback::Playing {
            since: Instant::now(),
            start: self.position,
            audio,
        };
    }
}

struct VideoRequest {
    context: egui::Context,
    revision: u64,
    timestamp: u64,
    path: PathBuf,
    start: u64,
    number: u64,
}

struct VideoUpdate {
    revision: u64,
    timestamp: u64,
    result: Result<RawFrame, String>,
}

struct RawFrame {
    size: [u16; 2],
    rgb: Vec<u8>,
}

fn decode_video(input: &Receiver<VideoRequest>, output: &Sender<VideoUpdate>) {
    let mut decoder = None;
    while let Ok(request) = input.recv() {
        let request = input.try_iter().last().unwrap_or(request);
        let result = read_frame(&mut decoder, &request);
        if result.is_err() {
            decoder = None;
        }
        if output
            .send(VideoUpdate {
                revision: request.revision,
                timestamp: request.timestamp,
                result,
            })
            .is_err()
        {
            break;
        }
        request.context.request_repaint();
    }
}

fn read_frame(decoder: &mut Option<Decoder>, request: &VideoRequest) -> Result<RawFrame, String> {
    if !decoder
        .as_ref()
        .is_some_and(|decoder| decoder.matches(request))
    {
        *decoder = None;
        *decoder = Some(Decoder::open(&request.path, request.start, request.number)?);
    }
    decoder
        .as_mut()
        .ok_or("Cannot open the preview decoder.")?
        .read(request.number)
}

struct Decoder {
    path: PathBuf,
    start: u64,
    next: u64,
    size: [u16; 2],
    _child: RunningChild,
    pipe: ChildStdout,
}

impl Decoder {
    fn open(path: &Path, start: u64, number: u64) -> Result<Self, String> {
        let (mut command, size) =
            media::preview_video(path, start + number * FRAME_TIME, FRAME_RATE)?;
        let mut child = RunningChild(
            command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .map_err(|error| format!("Cannot start FFmpeg. {error}"))?,
        );
        let pipe = child
            .0
            .stdout
            .take()
            .ok_or("Cannot open the preview stream.")?;
        Ok(Self {
            path: path.to_owned(),
            start,
            next: number,
            size,
            _child: child,
            pipe,
        })
    }

    fn matches(&self, request: &VideoRequest) -> bool {
        self.path == request.path
            && self.start == request.start
            && (self.next..=self.next.saturating_add(MAX_SKIPPED_FRAMES)).contains(&request.number)
    }

    fn read(&mut self, number: u64) -> Result<RawFrame, String> {
        let [width, height] = self.size.map(usize::from);
        let mut rgb = vec![0; width * height * 3];
        while self.next <= number {
            self.pipe.read_exact(&mut rgb).map_err(|error| {
                format!(
                    "Cannot read the preview frame from {}. {error}",
                    self.path.display()
                )
            })?;
            self.next += 1;
        }
        Ok(RawFrame {
            size: self.size,
            rgb,
        })
    }
}

struct AudioRequest {
    context: egui::Context,
    revision: u64,
    clip: Clip,
    start: u64,
}

struct AudioUpdate {
    revision: u64,
    result: Result<Option<Command>, String>,
}

fn prepare_audio(input: &Receiver<AudioRequest>, output: &Sender<AudioUpdate>) {
    while let Ok(request) = input.recv() {
        let request = input.try_iter().last().unwrap_or(request);
        let duration = request.clip.duration();
        let result = request
            .clip
            .trim(request.start..duration)
            .and_then(|clip| media::preview_audio(&clip));
        if output
            .send(AudioUpdate {
                revision: request.revision,
                result,
            })
            .is_err()
        {
            break;
        }
        request.context.request_repaint();
    }
}

struct AudioPlayback {
    decoder: RunningChild,
    player: RunningChild,
}

impl AudioPlayback {
    fn start(mut command: Command) -> Result<Self, String> {
        let mut decoder = RunningChild(
            command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .map_err(|error| format!("Cannot start FFmpeg for audio preview. {error}"))?,
        );
        let pipe = decoder
            .0
            .stdout
            .take()
            .ok_or("Cannot open the audio stream.")?;
        let player = RunningChild(
            Command::new("ffplay")
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-nodisp",
                    "-autoexit",
                    "-f",
                    "wav",
                    "-probesize",
                    "32",
                    "-analyzeduration",
                    "0",
                    "-protocol_whitelist",
                    "pipe",
                    "-i",
                    "pipe:0",
                ])
                .stdin(pipe)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|error| format!("Cannot start FFplay for audio preview. {error}"))?,
        );
        Ok(Self { decoder, player })
    }

    fn error(&mut self, remaining: u64) -> Option<String> {
        if let Ok(Some(status)) = self.decoder.0.try_wait()
            && !status.success()
        {
            return Some(format!("FFmpeg could not decode preview audio. {status}"));
        }
        if let Ok(Some(status)) = self.player.0.try_wait()
            && (!status.success() || remaining > 250_000)
        {
            return Some(format!(
                "FFplay stopped before the audio preview ended. {status}"
            ));
        }
        None
    }
}

struct RunningChild(Child);

impl Drop for RunningChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
