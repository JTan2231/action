# Project instructions

Use simple designs.

## Documentation

Keep the root README to purpose, CI examples, and documentation links.
Keep command behavior in the editing guide. Keep CI and release procedures in
the repository tooling guide. Keep each full explanation in one document.

Use ASD-STE100 Issue 9. [ASD-STE100](https://www.asd-ste100.org/about_STE.html)
Preserve technical terms, commands, and field names.
Use short, active sentences and one subject per paragraph.
Start procedure steps with an action verb.
Preserve third-party legal text exactly.

## Editing

Open source files for reading only. Save edits in the project.
Keep clip IDs stable. Write only the new clip ID to standard output when an
edit succeeds. Write error messages to standard error.
Use FFmpeg to process video and audio. Use Rust to manage project state and
command behavior. Use the Rust standard library when it meets the requirement.

## CI and releases

Read `man telete` before queue operation.
Commit changes before submission. Submit from a clean Git worktree.
Use [repository CI](scripts/ci.md) for static analysis and release builds.
Repair all lint findings and compiler warnings before release.
Keep the project free of unit tests.
Keep automated validation to source checks and builds. Use the editing
procedure for manual verification.

Use the release version in `Cargo.toml`.
Install the retained binary from the approved CI job.
Keep build output and release artifacts outside the source checkout.
