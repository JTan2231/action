# Editing

Action uses local video files with a known duration. Make clips in a project.
Export a clip as an MP4 file. Install `ffmpeg` and `ffprobe`. Make them available
on `PATH`. Use an FFmpeg build with the `libx264` and `aac` encoders.

Install `action` on [macOS](../scripts/ci.md#release-and-install) or
[Windows](../scripts/ci.md#install-on-windows). Run `action --help` for command
syntax.

## Make an edit

1. Create a project in the `xyz` directory.
2. Make a clip from seconds 12.5 to 28 of `interview.mov`.
3. Put `outro.mp4` after the interview clip.
4. Show the saved clips.
5. Export the combined clip to `final.mp4`.

On macOS, run these commands in a shell.

```sh
action --project xyz init
intro=$(action --project xyz trim ./interview.mov 12.5..28)
edit=$(action --project xyz concatenate "$intro" ./outro.mp4)
action --project xyz list clips
action --project xyz export "$edit" final.mp4
```

On Windows, run these commands in PowerShell. Use an NTFS output directory.

```powershell
action.exe --project xyz init
$intro = action.exe --project xyz trim .\interview.mov 12.5..28
$edit = action.exe --project xyz concatenate $intro .\outro.mp4
action.exe --project xyz list clips
action.exe --project xyz export $edit final.mp4
```

Put literal clip IDs in quotes in PowerShell. For example, use `'@c1'`.

## Projects and clips

Select the project directory with `--project`. A relative path starts from the
current working directory. `init` creates a project in a new or existing
directory. An existing project causes `init` to exit with an error.

The `.action-project` manifest stores clip IDs, source ranges, and absolute
paths to source files. Action uses `.action.lock` to let one edit save changes
at a time. Commands that read the project can use the last saved manifest.

Keep the source contents and paths fixed until you complete the export.

A clip is an ordered list of ranges in source files. A clip ID starts with
`@c`. Action assigns clip IDs in sequence, starting with `@c1`. Use a clip ID
with the project that created it. Use a path such as `./@camera.mp4` for a file
whose name starts with `@`.

When Action saves a new clip, it writes the clip ID to standard output. Saved
clips are available in subsequent shell sessions. Use `list clips` to see the
ID, duration, and source ranges of each clip. The commands write errors to
standard error and return a nonzero exit status.

## Trim and concatenate

Enter the trim bounds as decimal seconds in the form `START..END`. Enter the
two values without spaces or signs. Put a digit before the decimal point.
Use a maximum of six fractional digits. The end must be after the start.
The end must be at or before the end of the input.

The range includes the start and excludes the end. Use the start of the input
as zero. In the example, `trim "$intro" 2..8` selects seconds 14.5 to 20.5
of the source.

Give `concatenate` two or more inputs. Put them in playback order. An input
can be a path or a saved clip ID. You can trim or concatenate the result again.

## Export

Give `export` a source path or a clip ID. Set the output path to a file with
the `.mp4` extension. The extension can use uppercase or lowercase letters.

Use inputs with the same display dimensions and pixel aspect ratio. Action
compares the dimensions after it applies rotation metadata. If one input has
audio, all inputs must have audio. Action examines these properties at export.

Use source files with rotation metadata in multiples of 90 degrees.

Action uses the first video stream with moving pictures. It uses the first
audio stream if one is available. FFmpeg renders the selected ranges at video
frame boundaries. The export uses H.264 video. If the source has audio, the
export uses AAC stereo audio. If a video dimension is odd, FFmpeg adds one
pixel to that dimension.

Use an output directory with write access and hard link support. Action creates
the output file after a successful render. If the output file exists, select a
new path. If a source file is missing, put it at the path in the error message.
