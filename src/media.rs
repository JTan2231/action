use std::collections::{BTreeMap, btree_map::Entry};
use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::project::Span;

const MICROS_PER_SECOND: u64 = 1_000_000;
const PROBE_FIELDS: &str = concat!(
    "format=duration:",
    "stream=index,codec_type,width,height,sample_aspect_ratio:",
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
    let file = path.display();
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

    let metadata = String::from_utf8_lossy(&output.stdout);
    let duration = metadata
        .lines()
        .find(|line| line.starts_with("format|"))
        .and_then(|line| field(line, "duration"))
        .and_then(duration_micros)
        .ok_or_else(|| format!("Cannot get the duration of {file}."))?;
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
        .find(|line| line.starts_with("stream|") && field(line, "codec_type") == Some("audio"))
        .map(|line| {
            stream_index(line)
                .ok_or_else(|| format!("Cannot get the audio stream index of {file}."))
        })
        .transpose()?;

    Ok(Source {
        duration,
        video,
        audio,
        geometry,
    })
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

pub fn render(spans: &[Span], output: &Path) -> Result<(), String> {
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

    let mut sources = BTreeMap::new();
    let mut geometry_and_audio = None;
    let mut filters = Vec::new();
    let mut concat_inputs = Vec::new();

    for (index, span) in spans.iter().enumerate() {
        let source = match sources.entry(span.path.as_path()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(probe(span.path.as_path())?),
        };
        let path = span.path.display();
        let start = span.start;
        let end = span.end;
        if start >= end || end > source.duration {
            return Err(format!(
                "The saved range is outside the duration of {path}."
            ));
        }
        let signature = (source.geometry, source.audio.is_some());
        if *geometry_and_audio.get_or_insert(signature) != signature {
            return Err(format!(
                "{path} must have the same frame dimensions, pixel aspect ratio, and audio presence as the first source."
            ));
        }
        let duration = timestamp(end - start);
        command
            .args([
                "-protocol_whitelist",
                "file",
                "-ss",
                &timestamp(start),
                "-t",
                &duration,
                "-i",
            ])
            .arg(&span.path);

        let video = source.video;
        let (aspect_x, aspect_y) = source.geometry.pixel_aspect;
        filters.push(format!(
            "[{index}:{video}]trim=end={duration},setpts=PTS-STARTPTS,setsar={aspect_x}/{aspect_y}[v{index}]"
        ));
        concat_inputs.push(format!("[v{index}]"));
        if let Some(audio) = source.audio {
            // Preserve the audio offset and fill gaps to the end of this range.
            filters.push(format!(
                "[{index}:{audio}]atrim=end={duration},aformat=sample_rates=48000:channel_layouts=stereo,\
                 aresample=async=1:first_pts=0,apad,atrim=end={duration}[a{index}]"
            ));
        } else {
            // The timing track gives one-frame segments an explicit duration.
            filters.push(format!("anullsrc=r=48000:cl=stereo:d={duration}[a{index}]"));
        }
        concat_inputs.push(format!("[a{index}]"));
    }

    let (_, has_audio) = geometry_and_audio.ok_or("Select a clip with at least one range.")?;
    let count = spans.len();
    filters.push(format!(
        "{}concat=n={count}:v=1:a=1[picture][audio]",
        concat_inputs.concat()
    ));
    if !has_audio {
        filters.push("[audio]anullsink".to_owned());
    }
    // H.264 with 4:2:0 chroma requires even frame dimensions.
    filters.push("[picture]pad=ceil(iw/2)*2:ceil(ih/2)*2[video]".to_owned());
    command.args(["-filter_complex", &filters.join(";")]);
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
    let output = directory.join(name);
    match output.symlink_metadata() {
        Ok(_) => Err(format!("{} already exists.", output.display())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(output),
        Err(error) => Err(format!("Cannot open {}. {error}", output.display())),
    }
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
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;

            builder.mode(0o700);
        }
        builder
            .create(&directory)
            .map_err(|error| format!("Cannot create {}. {error}", directory.display()))?;
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
