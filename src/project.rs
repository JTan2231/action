use std::collections::BTreeSet;
use std::ffi::OsStr;
#[cfg(unix)]
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::iter::Peekable;
use std::ops::Range;
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::{media, seconds};

const MANIFEST: &str = ".action-project";
const HEADER: &[u8] = b"action-project 2\0";
const LEGACY_HEADER: &[u8] = b"action-project 1\0";
const LOCK: &str = ".action.lock";

#[derive(Clone)]
pub struct Span {
    pub path: PathBuf,
    pub start: u64,
    pub end: u64,
}

#[derive(Clone)]
pub struct Placement {
    pub id: u64,
    pub path: PathBuf,
    pub from: u64,
    pub start: u64,
    pub end: u64,
    pub gain: f64,
}

impl Placement {
    fn check_source(&self) -> Result<(), String> {
        let duration = media::probe_audio(&self.path)?;
        if self.source_end().is_none_or(|end| end > duration) {
            return Err(format!(
                "The audio range exceeds the duration of {} ({} seconds).",
                self.path.display(),
                seconds(duration)
            ));
        }
        Ok(())
    }

    pub const fn source_end(&self) -> Option<u64> {
        self.from.checked_add(self.end - self.start)
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Level {
    Gain(f64),
    Mute,
}

#[derive(Clone, Copy)]
pub struct OriginalLevel {
    pub start: u64,
    pub end: u64,
    pub level: Level,
}

#[derive(Clone)]
pub struct Clip {
    pub spans: Vec<Span>,
    pub audio: Vec<Placement>,
    pub original: Vec<OriginalLevel>,
    duration: u64,
}

impl Clip {
    pub fn new(spans: Vec<Span>) -> Result<Self, String> {
        if spans.is_empty() {
            return Err("A clip must contain at least one range.".to_owned());
        }
        let duration = spans.iter().try_fold(0_u64, |duration, span| {
            if span.start >= span.end || !span.path.is_absolute() {
                return Err("Invalid source range.".to_owned());
            }
            duration
                .checked_add(span.end - span.start)
                .ok_or_else(|| "The clip is too long.".to_owned())
        })?;
        Ok(Self {
            spans,
            audio: Vec::new(),
            original: Vec::new(),
            duration,
        })
    }

    pub const fn duration(&self) -> u64 {
        self.duration
    }

    pub fn trim(self, bounds: Range<u64>) -> Result<Self, String> {
        check_bounds(&bounds, self.duration)?;
        let mut ranges = Vec::new();
        let mut position = 0;
        for span in self.spans {
            let next = position + (span.end - span.start);
            let start = bounds.start.max(position);
            let end = bounds.end.min(next);
            if start < end {
                ranges.push(Span {
                    path: span.path,
                    start: span.start + (start - position),
                    end: span.start + (end - position),
                });
            }
            position = next;
            if position >= bounds.end {
                break;
            }
        }
        let mut clip = Self::new(ranges)?;
        for mut placement in self.audio {
            let start = placement.start.max(bounds.start);
            let end = placement.end.min(bounds.end);
            if start < end {
                placement.from += start - placement.start;
                placement.start = start - bounds.start;
                placement.end = end - bounds.start;
                clip.audio.push(placement);
            }
        }
        for mut adjustment in self.original {
            let start = adjustment.start.max(bounds.start);
            let end = adjustment.end.min(bounds.end);
            if start < end {
                adjustment.start = start - bounds.start;
                adjustment.end = end - bounds.start;
                clip.original.push(adjustment);
            }
        }
        Ok(clip)
    }

    pub fn concatenate(mut self, other: Self) -> Result<Self, String> {
        let offset = self.duration;
        self.duration = offset
            .checked_add(other.duration)
            .ok_or("The clip is too long.")?;
        if !other.audio.is_empty() {
            let existing_ids: BTreeSet<_> = self.audio.iter().map(|item| item.id).collect();
            let mut max_id = self
                .audio
                .iter()
                .chain(&other.audio)
                .map(|item| item.id)
                .max()
                .unwrap_or(0);
            for mut placement in other.audio {
                if existing_ids.contains(&placement.id) {
                    max_id = next_placement_id(max_id)?;
                    placement.id = max_id;
                }
                placement.start += offset;
                placement.end += offset;
                self.audio.push(placement);
            }
        }
        self.original
            .extend(other.original.into_iter().map(|mut adjustment| {
                adjustment.start += offset;
                adjustment.end += offset;
                adjustment
            }));
        self.spans.extend(other.spans);
        Ok(self)
    }

    pub fn add_audio(
        mut self,
        path: &Path,
        bounds: Range<u64>,
        from: u64,
        gain: f64,
    ) -> Result<Self, String> {
        check_bounds(&bounds, self.duration)?;
        validate_gain(gain)?;
        let path = fs::canonicalize(path)
            .map_err(|error| file_error("resolve audio source", path, &error))?;
        let max_id = self.audio.iter().map(|item| item.id).max().unwrap_or(0);
        let placement = Placement {
            id: next_placement_id(max_id)?,
            path,
            from,
            start: bounds.start,
            end: bounds.end,
            gain,
        };
        placement.check_source()?;
        self.audio.push(placement);
        Ok(self)
    }

    pub fn edit_audio(
        mut self,
        id: u64,
        bounds: Option<Range<u64>>,
        from: Option<u64>,
        gain: Option<f64>,
    ) -> Result<Self, String> {
        let index = self.placement_index(id)?;
        let placement = &mut self.audio[index];
        let bounds = bounds.unwrap_or(placement.start..placement.end);
        check_bounds(&bounds, self.duration)?;
        let from = from.unwrap_or(placement.from);
        let source_changed =
            from != placement.from || bounds.end - bounds.start != placement.end - placement.start;
        let gain = gain.unwrap_or(placement.gain);
        validate_gain(gain)?;
        placement.start = bounds.start;
        placement.end = bounds.end;
        placement.from = from;
        placement.gain = gain;
        if source_changed {
            placement.check_source()?;
        }
        Ok(self)
    }

    pub fn remove_audio(mut self, id: u64) -> Result<Self, String> {
        let index = self.placement_index(id)?;
        self.audio.remove(index);
        Ok(self)
    }

    fn placement_index(&self, id: u64) -> Result<usize, String> {
        self.audio
            .iter()
            .position(|item| item.id == id)
            .ok_or_else(|| {
                format!("Unknown audio placement a{id}. Run action --project DIRECTORY list clips.")
            })
    }

    pub fn original_level(mut self, bounds: Range<u64>, level: Level) -> Result<Self, String> {
        check_bounds(&bounds, self.duration)?;
        if let Level::Gain(gain) = level {
            validate_gain(gain)?;
        }
        let mut original = Vec::new();
        for adjustment in self.original {
            if adjustment.end <= bounds.start || adjustment.start >= bounds.end {
                original.push(adjustment);
                continue;
            }
            if adjustment.start < bounds.start {
                original.push(OriginalLevel {
                    end: bounds.start,
                    ..adjustment
                });
            }
            if adjustment.end > bounds.end {
                original.push(OriginalLevel {
                    start: bounds.end,
                    ..adjustment
                });
            }
        }
        if level != Level::Gain(0.0) {
            original.push(OriginalLevel {
                start: bounds.start,
                end: bounds.end,
                level,
            });
        }
        original.sort_by_key(|item| item.start);
        self.original = original;
        Ok(self)
    }
}

fn check_bounds(bounds: &Range<u64>, duration: u64) -> Result<(), String> {
    if bounds.start >= bounds.end {
        return Err("The start time must be less than the end time.".to_owned());
    }
    if bounds.end > duration {
        return Err(format!(
            "The end time exceeds the clip duration of {} seconds.",
            seconds(duration)
        ));
    }
    Ok(())
}

fn validate_gain(gain: f64) -> Result<(), String> {
    if (-120.0..=60.0).contains(&gain) {
        Ok(())
    } else {
        Err("Set gain to a number between -120 and 60 dB.".to_owned())
    }
}

fn next_placement_id(max_id: u64) -> Result<u64, String> {
    max_id
        .checked_add(1)
        .ok_or_else(|| "Cannot assign an audio placement ID.".to_owned())
}

pub struct Project {
    pub clips: Vec<Clip>,
}

impl Project {
    pub fn init(directory: &Path) -> Result<(), String> {
        fs::create_dir_all(directory).map_err(|error| file_error("create", directory, &error))?;
        let _lock = lock(directory)?;
        let manifest = directory.join(MANIFEST);
        match fs::symlink_metadata(&manifest) {
            Ok(_) => Err(format!(
                "A project already exists at {}.",
                directory.display()
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Self { clips: Vec::new() }.save(directory)
            }
            Err(error) => Err(file_error("inspect", &manifest, &error)),
        }
    }

    pub fn load(directory: &Path) -> Result<Self, String> {
        let manifest = directory.join(MANIFEST);
        let bytes = fs::read(&manifest).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                missing_project(directory)
            } else {
                file_error("read", &manifest, &error)
            }
        })?;
        Self::decode(&bytes)
            .ok_or_else(|| format!("Invalid project manifest at {}.", manifest.display()))
    }

    pub fn edit(
        directory: &Path,
        edit: impl FnOnce(&Self) -> Result<Clip, String>,
    ) -> Result<usize, String> {
        Ok(Self::append(directory, edit)?.clips.len())
    }

    pub fn append(
        directory: &Path,
        edit: impl FnOnce(&Self) -> Result<Clip, String>,
    ) -> Result<Self, String> {
        let manifest = directory.join(MANIFEST);
        if !manifest
            .try_exists()
            .map_err(|error| file_error("inspect", &manifest, &error))?
        {
            return Err(missing_project(directory));
        }
        let _lock = lock(directory)?;
        let mut project = Self::load(directory)?;
        let clip = edit(&project)?;
        project.clips.push(clip);
        project.save(directory)?;
        Ok(project)
    }

    pub fn resolve(&self, input: &OsStr) -> Result<Clip, String> {
        if input.as_encoded_bytes().starts_with(b"@") {
            let index = input
                .to_str()
                .and_then(|value| value.strip_prefix("@c"))
                .and_then(|id| {
                    let index = id.parse::<usize>().ok()?;
                    (index.to_string() == id).then_some(index)
                })
                .and_then(|index| index.checked_sub(1));
            return index
                .and_then(|index| self.clips.get(index))
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "Unknown clip ID {}. Run action --project DIRECTORY list clips.",
                        input.to_string_lossy().escape_debug()
                    )
                });
        }

        Self::resolve_source(Path::new(input))
    }

    pub fn resolve_source(input: &Path) -> Result<Clip, String> {
        let path =
            fs::canonicalize(input).map_err(|error| file_error("resolve source", input, &error))?;
        let source = media::probe(&path)?;
        Clip::new(vec![Span {
            path,
            start: 0,
            end: source.duration,
        }])
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let (bytes, typed) = if let Some(bytes) = bytes.strip_prefix(HEADER) {
            (bytes, true)
        } else {
            (bytes.strip_prefix(LEGACY_HEADER)?, false)
        };
        let mut clips = Vec::new();
        if !bytes.is_empty() {
            // Zero bytes mark the end of each field, including paths with newlines.
            let mut fields = bytes
                .strip_suffix(b"\0")?
                .split(|byte| *byte == 0)
                .peekable();
            while fields.peek().is_some() {
                clips.push(decode_clip(&mut fields, clips.len() + 1, typed)?);
            }
        }
        Some(Self { clips })
    }

    fn save(&self, directory: &Path) -> Result<(), String> {
        let manifest = directory.join(MANIFEST);
        let temporary = directory.join(format!("{MANIFEST}.{}.tmp", std::process::id()));
        let file = writable_project_file()
            .create_new(true)
            .open(&temporary)
            .map_err(|error| file_error("create", &temporary, &error))?;
        let mut writer = BufWriter::new(file);
        let result = self
            .write_manifest(&mut writer)
            .and_then(|()| writer.flush())
            .and_then(|()| writer.get_ref().sync_all());
        drop(writer);
        let result = result.and_then(|()| fs::rename(&temporary, &manifest));
        if let Err(error) = result {
            let _ = fs::remove_file(temporary);
            return Err(file_error("save", &manifest, &error));
        }
        Ok(())
    }

    fn write_manifest(&self, writer: &mut impl Write) -> io::Result<()> {
        writer.write_all(HEADER)?;
        for (index, clip) in self.clips.iter().enumerate() {
            let id = index + 1;
            for span in &clip.spans {
                write!(writer, "{id}\0v\0{}\0{}\0", span.start, span.end)?;
                write_path(writer, &span.path)?;
            }
            for placement in &clip.audio {
                write!(
                    writer,
                    "{id}\0a\0{}\0{}\0{}\0{}\0{}\0",
                    placement.start, placement.end, placement.id, placement.from, placement.gain
                )?;
                write_path(writer, &placement.path)?;
            }
            for adjustment in &clip.original {
                write!(
                    writer,
                    "{id}\0o\0{}\0{}\0",
                    adjustment.start, adjustment.end
                )?;
                match adjustment.level {
                    Level::Mute => writer.write_all(b"mute\0")?,
                    Level::Gain(gain) => write!(writer, "{gain}\0")?,
                }
            }
        }
        Ok(())
    }
}

fn decode_clip<'a>(
    fields: &mut Peekable<impl Iterator<Item = &'a [u8]>>,
    id: usize,
    typed: bool,
) -> Option<Clip> {
    let mut spans = Vec::new();
    let mut audio = Vec::new();
    let mut original = Vec::new();
    let mut placement_ids = BTreeSet::new();
    while fields.peek().and_then(|value| decode_value(value)) == Some(id) {
        fields.next();
        let kind = if typed { fields.next()? } else { b"v" };
        let start = decode_value::<u64>(fields.next()?)?;
        let end = decode_value::<u64>(fields.next()?)?;
        if start >= end {
            return None;
        }
        match kind {
            b"v" => {
                let path = decode_path(fields.next()?)?;
                spans.push(Span { path, start, end });
            }
            b"a" => {
                let id = decode_value(fields.next()?)?;
                let from = decode_value(fields.next()?)?;
                let gain = decode_gain(fields.next()?)?;
                let path = decode_path(fields.next()?)?;
                let placement = Placement {
                    id,
                    path,
                    from,
                    start,
                    end,
                    gain,
                };
                if id == 0 || !placement_ids.insert(id) || placement.source_end().is_none() {
                    return None;
                }
                audio.push(placement);
            }
            b"o" => {
                let level = match fields.next()? {
                    b"mute" => Level::Mute,
                    value => Level::Gain(decode_gain(value)?),
                };
                if original
                    .last()
                    .is_some_and(|previous: &OriginalLevel| previous.end > start)
                {
                    return None;
                }
                original.push(OriginalLevel { start, end, level });
            }
            _ => return None,
        }
    }
    let mut clip = Clip::new(spans).ok()?;
    if audio.iter().any(|item| item.end > clip.duration)
        || original.last().is_some_and(|item| item.end > clip.duration)
    {
        return None;
    }
    clip.audio = audio;
    clip.original = original;
    Some(clip)
}

fn decode_value<T: FromStr>(bytes: &[u8]) -> Option<T> {
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

fn decode_gain(bytes: &[u8]) -> Option<f64> {
    let gain = decode_value(bytes)?;
    validate_gain(gain).ok()?;
    Some(gain)
}

fn decode_path(bytes: &[u8]) -> Option<PathBuf> {
    #[cfg(unix)]
    let path = PathBuf::from(OsString::from_vec(bytes.to_vec()));
    #[cfg(not(unix))]
    let path = PathBuf::from(std::str::from_utf8(bytes).ok()?);
    path.is_absolute().then_some(path)
}

fn write_path(writer: &mut impl Write, path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    let bytes = path.as_os_str().as_bytes();
    #[cfg(not(unix))]
    let bytes = path
        .to_str()
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Source path must use Unicode.")
        })?
        .as_bytes();
    writer.write_all(bytes)?;
    writer.write_all(b"\0")
}

fn writable_project_file() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    options.mode(0o600);
    options
}

fn lock(directory: &Path) -> Result<File, String> {
    let path = directory.join(LOCK);
    let file = writable_project_file()
        .read(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| file_error("open project lock", &path, &error))?;
    file.lock()
        .map_err(|error| file_error("acquire project lock", &path, &error))?;
    Ok(file)
}

fn file_error(operation: &str, path: &Path, error: &io::Error) -> String {
    format!("Cannot {operation} {}: {error}", path.display())
}

fn missing_project(directory: &Path) -> String {
    format!(
        "No project at {}. Run action --project DIRECTORY init.",
        directory.display()
    )
}
