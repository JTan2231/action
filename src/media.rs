use std::collections::{BTreeMap, btree_map::Entry};
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::project::{Clip, Level, OriginalLevel, Span};

const MICROS_PER_SECOND: u64 = 1_000_000;
const SAMPLE_RATE: u64 = 48_000;
const PROBE_FIELDS: &str = concat!(
    "format=duration:",
    "stream=index,codec_type,duration,width,height,sample_aspect_ratio:",
    "stream_disposition=attached_pic,timed_thumbnails,still_image:",
    "stream_side_data=rotation"
);

pub struct Source {
    pub duration: u64,
    video: u32,
    audio: Option<u32>,
    geometry: Geometry,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Geometry {
    width: u32,
    height: u32,
    pixel_aspect: (u32, u32),
}

pub fn probe(path: &Path) -> Result<Source, String> {
    let metadata = probe_metadata(path)?;
    let file = path.display();
    let duration =
        format_duration(&metadata).ok_or_else(|| format!("Cannot get the duration of {file}."))?;
    let video_line = metadata
        .lines()
        .find(|line| regular_video(line))
        .ok_or_else(|| format!("{file} must contain a video stream."))?;
    let video = stream_index(video_line)
        .ok_or_else(|| format!("Cannot get the video stream index of {file}."))?;
    let geometry = video_geometry(video_line)
        .ok_or_else(|| format!("Cannot get the frame dimensions or orientation of {file}."))?;
    let audio = metadata
        .lines()
        .find(|line| is_audio(line))
        .map(|line| audio_index(line, path))
        .transpose()?;

    Ok(Source {
        duration,
        video,
        audio,
        geometry,
    })
}

pub fn probe_audio(path: &Path) -> Result<u64, String> {
    Ok(probe_audio_source(path)?.duration)
}

struct AudioSource {
    index: u32,
    duration: u64,
}

fn probe_audio_source(path: &Path) -> Result<AudioSource, String> {
    let metadata = probe_metadata(path)?;
    let file = path.display();
    let line = metadata
        .lines()
        .find(|line| is_audio(line))
        .ok_or_else(|| format!("{file} must contain an audio stream."))?;
    let duration = field(line, "duration")
        .and_then(duration_micros)
        .or_else(|| {
            // In an audio file, the format duration can describe the only audio stream.
            let audio_only = !metadata.lines().any(regular_video)
                && metadata.lines().filter(|line| is_audio(line)).count() == 1;
            if audio_only {
                format_duration(&metadata)
            } else {
                None
            }
        })
        .ok_or_else(|| format!("Cannot get the audio stream duration of {file}."))?;

    Ok(AudioSource {
        index: audio_index(line, path)?,
        duration,
    })
}

fn probe_metadata(path: &Path) -> Result<String, String> {
    let file = path.display();
    #[cfg(windows)]
    check_media_path(path)?;
    let metadata = fs::metadata(path).map_err(|error| format!("Cannot read {file}. {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{file} must be a regular file."));
    }

    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file",
            "-show_entries",
            PROBE_FIELDS,
            "-of",
            "compact=p=1:nk=0",
            "-i",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("Cannot start ffprobe. {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Cannot read media in {file}. {}",
            detail.trim_end()
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn format_duration(metadata: &str) -> Option<u64> {
    metadata
        .lines()
        .find(|line| line.starts_with("format|"))
        .and_then(|line| field(line, "duration"))
        .and_then(duration_micros)
}

fn is_audio(line: &str) -> bool {
    line.starts_with("stream|") && field(line, "codec_type") == Some("audio")
}

fn audio_index(line: &str, path: &Path) -> Result<u32, String> {
    stream_index(line)
        .ok_or_else(|| format!("Cannot get the audio stream index of {}.", path.display()))
}

fn duration_micros(value: &str) -> Option<u64> {
    let (seconds, fraction) = value.split_once('.')?;
    // FFprobe prints durations with six decimal places.
    if fraction.len() != 6 {
        return None;
    }
    let duration = seconds
        .parse::<u64>()
        .ok()?
        .checked_mul(MICROS_PER_SECOND)?
        .checked_add(fraction.parse().ok()?)?;
    (duration > 0).then_some(duration)
}

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split('|')
        .filter_map(|part| part.split_once('='))
        .find_map(|(name, value)| (name.rsplit(':').next() == Some(key)).then_some(value))
}

fn regular_video(line: &str) -> bool {
    line.starts_with("stream|")
        && field(line, "codec_type") == Some("video")
        && !["attached_pic", "timed_thumbnails", "still_image"]
            .iter()
            .any(|disposition| field(line, disposition) == Some("1"))
}

fn stream_index(line: &str) -> Option<u32> {
    field(line, "index")?.parse().ok()
}

fn video_geometry(line: &str) -> Option<Geometry> {
    let mut width = field(line, "width")?.parse().ok()?;
    let mut height = field(line, "height")?.parse().ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    let mut pixel_aspect = field(line, "sample_aspect_ratio")
        .and_then(|value| value.split_once(':'))
        .and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?)))
        .filter(|&(x, y)| x > 0 && y > 0)
        .unwrap_or((1, 1));
    let rotation = field(line, "rotation").unwrap_or("0").parse::<i32>().ok()?;
    match rotation.rem_euclid(360) {
        0 | 180 => {}
        90 | 270 => {
            std::mem::swap(&mut width, &mut height);
            pixel_aspect = (pixel_aspect.1, pixel_aspect.0);
        }
        _ => return None,
    }
    Some(Geometry {
        width,
        height,
        pixel_aspect,
    })
}

pub fn render(clip: &Clip, output: &Path) -> Result<(), String> {
    let output = output_path(output)?;
    let mut command = Command::new("ffmpeg");
    command.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-n",
        "-xerror",
        "-abort_on",
        "empty_output_stream",
    ]);

    let (graph, has_audio) = filter_graph(&mut command, clip)?;
    command.args(["-filter_complex", &graph]);
    apply_mp4_preset(&mut command, has_audio);

    let parent = output.parent().ok_or("Specify an output file.")?;
    let staging = StagingFile::create(parent)?;
    let status = command
        .args([
            "-map_metadata",
            "-1",
            "-map_chapters",
            "-1",
            "-movflags",
            "+faststart",
            "-f",
            "mp4",
        ])
        .arg(&staging.file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .map_err(|error| format!("Cannot start ffmpeg. {error}"))?;
    if !status.success() {
        return Err(format!("FFmpeg could not export the clip. {status}"));
    }

    fs::hard_link(&staging.file, &output)
        .map_err(|error| format!("Cannot save {}. {error}", output.display()))
}

fn filter_graph(command: &mut Command, clip: &Clip) -> Result<(String, bool), String> {
    let edited = !clip.audio.is_empty() || !clip.original.is_empty();
    let mut sources = BTreeMap::new();
    let mut geometry = None;
    let mut filters = Vec::new();
    let mut inputs = Vec::new();
    let mut sound_inputs = Vec::new();
    let mut position = 0;
    let mut has_original = false;

    for (index, span) in clip.spans.iter().enumerate() {
        let source = match sources.entry(span.path.as_path()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(probe(span.path.as_path())?),
        };
        check_video_span(span, source, &mut geometry)?;
        let next = position + (span.end - span.start);
        let duration = timestamp(next - position);
        command
            .args([
                "-protocol_whitelist",
                "file",
                "-ss",
                &timestamp(span.start),
                "-t",
                &duration,
                "-i",
            ])
            .arg(&span.path);
        let video = source.video;
        let (aspect_x, aspect_y) = source.geometry.pixel_aspect;
        let timing = if edited {
            // Use the same timeline offsets for the picture and the sound.
            format!("settb=AVTB,setpts=PTS-STARTPTS+{position}")
        } else {
            "setpts=PTS-STARTPTS".to_owned()
        };
        filters.push(format!(
            "[{index}:{video}]trim=end={duration},{timing},setsar={aspect_x}/{aspect_y}[v{index}]"
        ));
        inputs.push(format!("[v{index}]"));
        let samples = edited.then(|| sample_at(next) - sample_at(position));
        if samples != Some(0) {
            filters.push(span_audio_filter(index, source.audio, &duration, samples));
            has_original |= source.audio.is_some();
            if edited {
                sound_inputs.push(format!("[a{index}]"));
            } else {
                inputs.push(format!("[a{index}]"));
            }
        }
        position = next;
    }

    let count = clip.spans.len();
    let has_audio = has_original || !clip.audio.is_empty();
    if edited {
        filters.push(format!("{}interleave=n={count}[picture]", inputs.concat()));
        let count = sound_inputs.len();
        filters.push(format!(
            "{}concat=n={count}:v=0:a=1,asettb=1/{SAMPLE_RATE},asetpts=N[original]",
            sound_inputs.concat()
        ));
        if has_audio {
            mix_audio(command, clip, &mut filters)?;
        } else {
            filters.push("[original]anullsink".to_owned());
        }
    } else {
        filters.push(format!(
            "{}concat=n={count}:v=1:a=1[picture][audio]",
            inputs.concat()
        ));
        if !has_audio {
            filters.push("[audio]anullsink".to_owned());
        }
    }
    // H.264 with 4:2:0 chroma requires even frame dimensions.
    filters.push("[picture]pad=ceil(iw/2)*2:ceil(ih/2)*2[video]".to_owned());
    Ok((filters.join(";"), has_audio))
}

fn check_video_span(
    span: &Span,
    source: &Source,
    geometry: &mut Option<Geometry>,
) -> Result<(), String> {
    let path = span.path.display();
    let Span { start, end, .. } = span;
    if start >= end || *end > source.duration {
        return Err(format!(
            "The saved range is outside the duration of {path}."
        ));
    }
    if *geometry.get_or_insert(source.geometry) != source.geometry {
        return Err(format!(
            "{path} must have the same frame dimensions and pixel aspect ratio as the first source."
        ));
    }
    Ok(())
}

fn span_audio_filter(
    index: usize,
    audio: Option<u32>,
    duration: &str,
    samples: Option<u64>,
) -> String {
    let end = samples.map_or_else(|| format!("end={duration}"), |n| format!("end_sample={n}"));
    if let Some(audio) = audio {
        // Preserve the audio offset and fill gaps to the end of this range.
        return format!(
            "[{index}:{audio}]atrim=end={duration},aformat=sample_rates={SAMPLE_RATE}:channel_layouts=stereo,\
             aresample=async=1:first_pts=0,apad,atrim={end}[a{index}]"
        );
    }
    if samples.is_some() {
        format!("anullsrc=r={SAMPLE_RATE}:cl=stereo,atrim={end}[a{index}]")
    } else {
        // The timing track gives one-frame segments an explicit duration.
        format!("anullsrc=r={SAMPLE_RATE}:cl=stereo:d={duration}[a{index}]")
    }
}

fn mix_audio(command: &mut Command, clip: &Clip, filters: &mut Vec<String>) -> Result<(), String> {
    let original = original_audio(&clip.original, filters)?;
    let mut sources = BTreeMap::new();
    let mut inputs = vec![original];
    for (index, placement) in clip.audio.iter().enumerate() {
        let source = match sources.entry(placement.path.as_path()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(probe_audio_source(placement.path.as_path())?),
        };
        if placement
            .source_end()
            .is_none_or(|end| end > source.duration)
        {
            return Err(format!(
                "The saved audio range is outside the duration of {}.",
                placement.path.display()
            ));
        }
        let start = sample_at(placement.start);
        let samples = sample_at(placement.end) - start;
        if samples == 0 {
            continue;
        }
        let input = clip.spans.len() + inputs.len() - 1;
        command
            .args(["-protocol_whitelist", "file", "-i"])
            .arg(&placement.path);
        let stream = source.index;
        let from = sample_at(placement.from);
        let end = from + samples;
        let gain = placement.gain;
        // Start at the beginning of the selected audio stream, even when it starts late.
        filters.push(format!(
            "[{input}:{stream}]asetpts=PTS-STARTPTS,aformat=sample_rates={SAMPLE_RATE}:channel_layouts=stereo,\
             aresample=async=1:first_pts=0,atrim=start_sample={from}:end_sample={end},asetpts=PTS-STARTPTS,\
             volume={gain}dB,adelay={start}S:all=1[placement{index}]"
        ));
        inputs.push(format!("[placement{index}]"));
    }
    let count = inputs.len();
    let samples = sample_at(clip.duration());
    filters.push(format!(
        "{}amix=inputs={count}:normalize=0:duration=first,apad=pad_len=240,\
         alimiter=level=0:latency=1,atrim=end_sample={samples}[audio]",
        inputs.concat()
    ));
    Ok(())
}

fn original_audio(levels: &[OriginalLevel], filters: &mut Vec<String>) -> Result<String, String> {
    if levels.is_empty() {
        return Ok("[original]".to_owned());
    }
    let mut volume = "1".to_owned();
    for level in levels {
        let start = sample_at(level.start);
        let end = sample_at(level.end);
        if start < end {
            let delta = match level.level {
                Level::Gain(db) => 10.0_f64.powf(db / 20.0) - 1.0,
                Level::Mute => -1.0,
            };
            write!(volume, "+between(n,{start},{})*({delta})", end - 1)
                .map_err(|error| error.to_string())?;
        }
    }
    filters.push(format!(
        "[original]aeval='val(ch)*({volume})':c=same[adjusted]"
    ));
    Ok("[adjusted]".to_owned())
}

const fn sample_at(micros: u64) -> u64 {
    let seconds = micros / MICROS_PER_SECOND;
    let fraction = micros % MICROS_PER_SECOND;
    seconds * SAMPLE_RATE + (fraction * SAMPLE_RATE).div_ceil(MICROS_PER_SECOND)
}

fn apply_mp4_preset(command: &mut Command, has_audio: bool) {
    command.args([
        "-map",
        "[video]",
        "-c:v",
        "libx264",
        "-crf",
        "18",
        "-preset",
        "medium",
        "-pix_fmt",
        "yuv420p",
        "-fps_mode",
        "vfr",
        "-enc_time_base:v",
        "1:1000000",
    ]);
    if has_audio {
        command.args(["-map", "[audio]", "-c:a", "aac", "-b:a", "192k"]);
    }
}

fn timestamp(micros: u64) -> String {
    let seconds = micros / MICROS_PER_SECOND;
    let fraction = micros % MICROS_PER_SECOND;
    format!("{seconds}.{fraction:06}")
}

fn output_path(path: &Path) -> Result<PathBuf, String> {
    if !path
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mp4"))
    {
        return Err("Use the .mp4 extension for the output file.".to_owned());
    }
    let name = path.file_name().ok_or("Specify an output file.")?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let directory = parent.canonicalize().map_err(|error| {
        format!(
            "Cannot open the output directory {}. {error}",
            parent.display()
        )
    })?;
    #[cfg(windows)]
    check_media_path(&directory)?;
    let output = directory.join(name);
    match output.symlink_metadata() {
        Ok(_) => Err(format!("{} already exists.", output.display())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(output),
        Err(error) => Err(format!("Cannot open {}. {error}", output.display())),
    }
}

#[cfg(windows)]
fn check_media_path(path: &Path) -> Result<(), String> {
    if path.to_str().is_none() {
        return Err(format!("Use a Unicode path for {}.", path.display()));
    }
    Ok(())
}

struct StagingFile {
    directory: PathBuf,
    file: PathBuf,
}

impl StagingFile {
    fn create(parent: &Path) -> Result<Self, String> {
        let pid = process::id();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let directory = parent.join(format!(".action-render-{pid}-{stamp}"));
        #[cfg(unix)]
        let result = {
            use std::os::unix::fs::DirBuilderExt as _;

            fs::DirBuilder::new().mode(0o700).create(&directory)
        };
        #[cfg(not(unix))]
        let result = fs::create_dir(&directory);
        result.map_err(|error| format!("Cannot create {}. {error}", directory.display()))?;
        let file = directory.join("render.mp4");
        Ok(Self { directory, file })
    }
}

impl Drop for StagingFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.file);
        let _ = fs::remove_dir(&self.directory);
    }
}
