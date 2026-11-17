use std::ffi::OsStr;
#[cfg(unix)]
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::ops::Range;
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::{media, seconds};

const MANIFEST: &str = ".action-project";
const HEADER: &[u8] = b"action-project 1\0";
const LOCK: &str = ".action.lock";

#[derive(Clone)]
pub struct Span {
    pub path: PathBuf,
    pub start: u64,
    pub end: u64,
}

#[derive(Clone)]
pub struct Clip {
    pub spans: Vec<Span>,
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
        Ok(Self { spans, duration })
    }

    pub const fn duration(&self) -> u64 {
        self.duration
    }

    pub fn trim(self, bounds: Range<u64>) -> Result<Self, String> {
        if bounds.end > self.duration {
            return Err(format!(
                "The end time exceeds the clip duration of {} seconds.",
                seconds(self.duration)
            ));
        }
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
        Self::new(ranges)
    }
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
        Ok(project.clips.len())
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

        let input = Path::new(input);
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
        let bytes = bytes.strip_prefix(HEADER)?;
        let mut clips = Vec::new();
        if !bytes.is_empty() {
            // Zero bytes mark the end of each field, including paths with newlines.
            let mut fields = bytes.strip_suffix(b"\0")?.split(|byte| *byte == 0);
            let mut ranges = Vec::new();
            while let Some(id) = fields.next() {
                let id: usize = std::str::from_utf8(id).ok()?.parse().ok()?;
                if id == clips.len() + 2 && !ranges.is_empty() {
                    clips.push(Clip::new(std::mem::take(&mut ranges)).ok()?);
                }
                if id != clips.len() + 1 {
                    return None;
                }
                let start = std::str::from_utf8(fields.next()?).ok()?.parse().ok()?;
                let end = std::str::from_utf8(fields.next()?).ok()?.parse().ok()?;
                let path = fields.next()?;
                #[cfg(unix)]
                let path = PathBuf::from(OsString::from_vec(path.to_vec()));
                #[cfg(not(unix))]
                let path = PathBuf::from(std::str::from_utf8(path).ok()?);
                ranges.push(Span { path, start, end });
            }
            clips.push(Clip::new(ranges).ok()?);
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
            for span in &clip.spans {
                write!(writer, "{}\0{}\0{}\0", index + 1, span.start, span.end)?;
                #[cfg(unix)]
                let path = span.path.as_os_str().as_bytes();
                #[cfg(not(unix))]
                let path = span
                    .path
                    .to_str()
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "Source path must use Unicode.")
                    })?
                    .as_bytes();
                writer.write_all(path)?;
                writer.write_all(b"\0")?;
            }
        }
        Ok(())
    }
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
