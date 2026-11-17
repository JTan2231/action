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

## Add music across a cut

1. Complete the procedure in [Make an edit](#make-an-edit). Use an outro with
   a duration of at least 2.5 seconds.
2. Select a music file with at least 36 seconds of audio. Use the name
   `music.wav` for this procedure.
3. Add music from second 12 to second 18 of the combined clip. Start at second
   30 of the music. The cut between the interview and the outro is at second
   15.5 of the combined clip.
4. Decrease the original sound by 8 dB for the same range.
5. Export the result to `with-music.mp4`. Play it and check that the music
   continues across the cut into the outro.

On macOS, run these commands in the same shell as the earlier procedure.

```sh
music=$(action --project xyz audio add "$edit" ./music.wav 12..18 --from 30 --gain -12)
mix=$(action --project xyz audio original "$music" 12..18 -8)
action --project xyz export "$mix" with-music.mp4
```

On Windows, run these commands in the same PowerShell session.

```powershell
$music = action.exe --project xyz audio add $edit .\music.wav 12..18 --from 30 --gain -12
$mix = action.exe --project xyz audio original $music 12..18 -8
action.exe --project xyz export $mix with-music.mp4
```

## Projects and clips

Select the project directory with `--project`. A relative path starts from the
current working directory. `init` creates a project in a new or existing
directory. An existing project causes `init` to exit with an error.

The `.action-project` manifest stores clip IDs, source ranges, audio edits,
and absolute paths to source files. Action uses `.action.lock` to let one edit
save changes at a time. Commands that read the project can use the last saved
manifest.

Keep the source contents and paths fixed until you complete the export.

A clip contains an ordered list of video source ranges and can contain audio
edits. A clip ID starts with `@c`. Action assigns clip IDs in sequence,
starting with `@c1`. Use a clip ID with the project that created it. Use a path
such as `./@camera.mp4` for a file whose name starts with `@`.

When Action saves a new clip, it writes the clip ID to standard output. Saved
clips are available in subsequent shell sessions. Use `list clips` to see the
ID, duration, source ranges, and audio edits of each clip. The commands write
errors to standard error and return a nonzero exit status.

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

## Edit audio

Use a video path or a clip ID for `INPUT`. Each audio edit saves a new clip.
Use the same `START..END` range format as for `trim`. Measure the range from
the start of the video clip.

```text
action --project DIRECTORY audio add INPUT AUDIO START..END [--from SECONDS] [--gain DB]
action --project DIRECTORY audio edit INPUT PLACEMENT [--at START..END] [--from SECONDS] [--gain DB]
action --project DIRECTORY audio remove INPUT PLACEMENT
action --project DIRECTORY audio original INPUT START..END DB|mute
```

Use `audio add` with the path to a file that contains audio. Action uses the
first audio stream. `--from` gives the start time in that stream in decimal
seconds. Use a maximum of six fractional digits. The default start time is
zero. The length of the clip range gives the length of the audio to use. Make
sure that the full range is available in the video clip and the audio stream.
To play the same song across several clips, concatenate the clips and add one
placement that covers the cuts.

Use `--gain` to set the level of a placement. Enter a value from `-120` to `60`
decibels (dB). The default is `0` dB. Use a negative value to decrease the
level, or a positive value to increase it. Action mixes overlapping placements
with the original sound.

Use `list clips` to find the placement ID, such as `a1`, for the clip that you
want to edit. Use `audio edit` with that ID and one or more of `--at`, `--from`,
and `--gain`. Use `--at` to move or resize the placement. Action keeps the
audio source start time unless you set `--from`. Use `audio remove` with the
ID to remove the placement from the result.

Use `audio original` to set the level of the original sound for a range.
Use a gain in decibels or use `mute` to make the original sound silent in that
range. Use `0` to restore its original level. If two original sound edits
overlap, Action uses the most recent edit in the overlap. These edits apply to
the original sound only.

If you trim a clip, Action keeps the parts of its audio edits inside the trim
range. It measures those parts from the start of the result. If the trim cuts
off the start of a placement, Action advances the start in the audio source
by the same amount.

If you concatenate clips, Action shifts each clip's audio edits by the total
duration of the preceding clips. Use `list clips` to check the placement IDs
in the result.

## Export

Give `export` a source path or a clip ID. Set the output path to a file with
the `.mp4` extension. The extension can use uppercase or lowercase letters.

Use inputs with the same display dimensions and pixel aspect ratio. Action
compares the dimensions after it applies rotation metadata. You can combine
video sources with sound and silent video sources.

Use source files with rotation metadata in multiples of 90 degrees.

Action uses the first video stream with moving pictures. It uses the first
audio stream from each video source if one is available. FFmpeg renders the
selected video ranges at frame boundaries. The export uses H.264 video. If a
video source has audio or the clip has an audio placement, the export uses AAC
stereo audio. If a video dimension is odd, FFmpeg adds one pixel to that
dimension.

Use an output directory with write access and hard link support. Action creates
the output file after a successful render. If the output file exists, select a
new path. If a source file is missing, put it at the path in the error message.
