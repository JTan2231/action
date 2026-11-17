mod media;
mod project;

use std::ffi::OsStr;
use std::io::{self, Write};
use std::ops::Range;
use std::path::Path;
use std::process::ExitCode;

use project::{Level, Project};

const HELP: &str = "Edit video clips with FFmpeg.

Usage
  action --project DIRECTORY init
  action --project DIRECTORY trim INPUT START..END
  action --project DIRECTORY concatenate INPUT INPUT [INPUT ...]
  action --project DIRECTORY audio add INPUT AUDIO START..END [--from SECONDS] [--gain DB]
  action --project DIRECTORY audio edit INPUT PLACEMENT [--at START..END] [--from SECONDS] [--gain DB]
  action --project DIRECTORY audio remove INPUT PLACEMENT
  action --project DIRECTORY audio original INPUT START..END DB|mute
  action --project DIRECTORY list clips
  action --project DIRECTORY export INPUT OUTPUT.mp4
  action --help
  action --version

Use a file path or a clip ID such as @c1 for INPUT.
Use decimal seconds for START and END, measured from the start of INPUT.
Use --from to select the start time in the audio source. The default is 0.
Use list clips to find placement IDs such as a1. Give audio edit at least one option.
Enter gain in dB from -120 to 60. The default is 0.
";

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let arguments: Vec<_> = arguments.iter().map(AsRef::<OsStr>::as_ref).collect();
    match run(&arguments, &mut io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("action: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: &[&OsStr], output: &mut impl Write) -> Result<(), String> {
    let stdout_error = |error: io::Error| format!("Cannot write to standard output: {error}");
    match arguments {
        [] => return output.write_all(HELP.as_bytes()).map_err(stdout_error),
        [flag] if *flag == "--help" || *flag == "-h" => {
            return output.write_all(HELP.as_bytes()).map_err(stdout_error);
        }
        [flag] if *flag == "--version" || *flag == "-V" => {
            return writeln!(output, "action {}", env!("CARGO_PKG_VERSION")).map_err(stdout_error);
        }
        _ => {}
    }

    let [flag, directory, command @ ..] = arguments else {
        return Err(usage_error());
    };
    if *flag != "--project" || directory.is_empty() {
        return Err(usage_error());
    }
    let directory = Path::new(directory);
    match command {
        [verb] if *verb == "init" => Project::init(directory),
        [verb, input, bounds] if *verb == "trim" => {
            let bounds = parse_bounds(bounds)?;
            let id = Project::edit(directory, |project| project.resolve(input)?.trim(bounds))?;
            writeln!(output, "@c{id}").map_err(stdout_error)
        }
        [verb, first, second, rest @ ..] if *verb == "concatenate" => {
            let id = Project::edit(directory, |project| {
                std::iter::once(second)
                    .chain(rest)
                    .try_fold(project.resolve(first)?, |clip, input| {
                        clip.concatenate(project.resolve(input)?)
                    })
            })?;
            writeln!(output, "@c{id}").map_err(stdout_error)
        }
        [verb, arguments @ ..] if *verb == "audio" => {
            let id = audio_command(directory, arguments)?;
            writeln!(output, "@c{id}").map_err(stdout_error)
        }
        [verb, object] if *verb == "list" && *object == "clips" => {
            let project = Project::load(directory)?;
            list_clips(&project, output).map_err(stdout_error)
        }
        [verb, input, destination] if *verb == "export" => {
            let project = Project::load(directory)?;
            let clip = project.resolve(input)?;
            media::render(&clip, Path::new(destination))
        }
        _ => Err(usage_error()),
    }
}

fn audio_command(directory: &Path, arguments: &[&OsStr]) -> Result<usize, String> {
    match arguments {
        [verb, input, source, bounds, options @ ..] if *verb == "add" => {
            let bounds = parse_bounds(bounds)?;
            let options = AudioOptions::parse(options, false)?;
            Project::edit(directory, |project| {
                project.resolve(input)?.add_audio(
                    Path::new(source),
                    bounds,
                    options.from.unwrap_or(0),
                    options.gain.unwrap_or(0.0),
                )
            })
        }
        [verb, input, placement, options @ ..] if *verb == "edit" && !options.is_empty() => {
            let placement = parse_placement(placement)?;
            let options = AudioOptions::parse(options, true)?;
            Project::edit(directory, |project| {
                project.resolve(input)?.edit_audio(
                    placement,
                    options.bounds,
                    options.from,
                    options.gain,
                )
            })
        }
        [verb, input, placement] if *verb == "remove" => {
            let placement = parse_placement(placement)?;
            Project::edit(directory, |project| {
                project.resolve(input)?.remove_audio(placement)
            })
        }
        [verb, input, bounds, level] if *verb == "original" => {
            let bounds = parse_bounds(bounds)?;
            let level = if *level == "mute" {
                Level::Mute
            } else {
                Level::Gain(parse_gain(level)?)
            };
            Project::edit(directory, |project| {
                project.resolve(input)?.original_level(bounds, level)
            })
        }
        _ => Err(usage_error()),
    }
}

#[derive(Default)]
struct AudioOptions {
    bounds: Option<Range<u64>>,
    from: Option<u64>,
    gain: Option<f64>,
}

impl AudioOptions {
    fn parse(arguments: &[&OsStr], allow_bounds: bool) -> Result<Self, String> {
        let pairs = arguments.chunks_exact(2);
        if !pairs.remainder().is_empty() {
            return Err(usage_error());
        }
        let mut options = Self::default();
        for pair in pairs {
            let [flag, value] = pair else {
                return Err(usage_error());
            };
            match flag.to_str() {
                Some("--at") if allow_bounds && options.bounds.is_none() => {
                    options.bounds = Some(parse_bounds(value)?);
                }
                Some("--from") if options.from.is_none() => {
                    options.from =
                        Some(value.to_str().and_then(parse_seconds).ok_or(
                            "Enter --from in decimal seconds with up to six decimal places.",
                        )?);
                }
                Some("--gain") if options.gain.is_none() => {
                    options.gain = Some(parse_gain(value)?);
                }
                _ => return Err(usage_error()),
            }
        }
        Ok(options)
    }
}

fn parse_placement(value: &OsStr) -> Result<u64, String> {
    value
        .to_str()
        .and_then(|value| value.strip_prefix('a'))
        .and_then(|value| {
            let id = value.parse::<u64>().ok()?;
            (id > 0 && id.to_string() == value).then_some(id)
        })
        .ok_or_else(|| "Use a placement ID such as a1. Run list clips to see IDs.".to_owned())
}

fn parse_gain(value: &OsStr) -> Result<f64, String> {
    value
        .to_str()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| "Enter a gain in dB from -120 to 60.".to_owned())
}

#[expect(
    clippy::unnecessary_debug_formatting,
    reason = "Escape path characters in the listing."
)]
fn list_clips(project: &Project, output: &mut impl Write) -> io::Result<()> {
    for (index, clip) in project.clips.iter().enumerate() {
        writeln!(output, "@c{}  {} s", index + 1, seconds(clip.duration()))?;
        for span in &clip.spans {
            writeln!(
                output,
                "  {:?}  {}..{} s",
                span.path,
                seconds(span.start),
                seconds(span.end)
            )?;
        }
        for audio in &clip.audio {
            writeln!(
                output,
                "  a{}  {}..{} s  {:?}  {}..{} s  {} dB",
                audio.id,
                seconds(audio.start),
                seconds(audio.end),
                audio.path,
                seconds(audio.from),
                seconds(audio.from + (audio.end - audio.start)),
                audio.gain
            )?;
        }
        for original in &clip.original {
            let level = match original.level {
                Level::Gain(gain) => format!("{gain} dB"),
                Level::Mute => "mute".to_owned(),
            };
            writeln!(
                output,
                "  original  {}..{} s  {level}",
                seconds(original.start),
                seconds(original.end)
            )?;
        }
    }
    Ok(())
}

fn usage_error() -> String {
    "Invalid command. Run action --help for usage.".to_owned()
}

fn parse_bounds(value: &OsStr) -> Result<Range<u64>, String> {
    let invalid = || "Use START..END in decimal seconds, with up to six decimal places.".to_owned();
    let (start, end) = value
        .to_str()
        .and_then(|value| value.split_once(".."))
        .ok_or_else(invalid)?;
    let start = parse_seconds(start).ok_or_else(invalid)?;
    let end = parse_seconds(end).ok_or_else(invalid)?;
    if start >= end {
        return Err("The start time must be less than the end time.".to_owned());
    }
    Ok(start..end)
}

fn parse_seconds(time: &str) -> Option<u64> {
    let parse_digits = |digits: &str| {
        if digits.bytes().all(|digit| digit.is_ascii_digit()) {
            digits.parse::<u64>().ok()
        } else {
            None
        }
    };
    let (whole, fraction) = if let Some((whole, fraction)) = time.split_once('.') {
        let digits = u32::try_from(fraction.len()).ok()?;
        if digits > 6 {
            return None;
        }
        (whole, parse_digits(fraction)? * 10_u64.pow(6 - digits))
    } else {
        (time, 0)
    };
    parse_digits(whole)?
        .checked_mul(1_000_000)?
        .checked_add(fraction)
}

fn seconds(time: u64) -> String {
    let whole = time / 1_000_000;
    let fraction = time % 1_000_000;
    if fraction == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{fraction:06}")
            .trim_end_matches('0')
            .to_owned()
    }
}
