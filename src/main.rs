mod media;
mod project;

use std::ffi::OsStr;
use std::io::{self, Write};
use std::ops::Range;
use std::path::Path;
use std::process::ExitCode;

use project::{Clip, Project};

const HELP: &str = "Edit video clips with FFmpeg.

Usage
  action --project DIRECTORY init
  action --project DIRECTORY trim INPUT START..END
  action --project DIRECTORY concatenate INPUT INPUT [INPUT ...]
  action --project DIRECTORY list clips
  action --project DIRECTORY export INPUT OUTPUT.mp4
  action --help
  action --version

Use a file path or a clip ID such as @c1 for INPUT.
Use decimal seconds for START and END, measured from the start of INPUT.
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
                let mut spans = project.resolve(first)?.spans;
                spans.extend(project.resolve(second)?.spans);
                for input in rest {
                    spans.extend(project.resolve(input)?.spans);
                }
                Clip::new(spans)
            })?;
            writeln!(output, "@c{id}").map_err(stdout_error)
        }
        [verb, object] if *verb == "list" && *object == "clips" => {
            let project = Project::load(directory)?;
            for (index, clip) in project.clips.iter().enumerate() {
                writeln!(output, "@c{}  {} s", index + 1, seconds(clip.duration()))
                    .map_err(stdout_error)?;
                for span in &clip.spans {
                    #[expect(
                        clippy::unnecessary_debug_formatting,
                        reason = "Escape path characters in the listing."
                    )]
                    writeln!(
                        output,
                        "  {:?}  {}..{} s",
                        span.path,
                        seconds(span.start),
                        seconds(span.end)
                    )
                    .map_err(stdout_error)?;
                }
            }
            Ok(())
        }
        [verb, input, destination] if *verb == "export" => {
            let project = Project::load(directory)?;
            let clip = project.resolve(input)?;
            media::render(&clip.spans, Path::new(destination))
        }
        _ => Err(usage_error()),
    }
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
