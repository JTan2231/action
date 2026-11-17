# Repository CI

Use the [Windows procedure](#windows) to run checks and install a release in
PowerShell.

## macOS

Telete runs checks on a Git commit in a private checkout. Linked Git worktrees
share one queue. A release job installs Action for the current user after the
checks pass.

### Setup

Install Telete 0.7.0 or later. Read `man telete` before you operate the queue.
Make `git`, `jq`, `rsync`, `shellcheck`, `shasum`, and `rustup` available on
`PATH`. Use the toolchain in `rust-toolchain.toml`. It specifies Rust 1.97.1,
`rustfmt`, and Clippy.

Install the [FFmpeg tools](../docs/editing.md) before deployment.

Run `cargo fetch --locked` in the repository before the first check and after
each change to `Cargo.lock`. Use the same Cargo home as the CI job. This command
downloads the dependencies for the offline build.

### Submit and observe

Commit changes before submission. Use a clean Git worktree. Run the commands
from the repository root.

```sh
./ci.sh
./ci.sh status JOB
./ci.sh wait JOB
```

Record the job ID from the submission receipt. Use it in place of `JOB`.
Submission puts the commit in the queue. The checks pass when Telete shows
`succeeded`.

Use `./ci.sh submit REVISION` to select a different Git revision. Add `--follow`
to wait for the result before the command returns.

Run `./ci.sh worktree PATH` to create a linked worktree at the current commit.
Use a new directory with an existing parent directory. Telete uses the new
directory's name for the branch. Run `./ci.sh --help` for other commands and
options.

### Checks and storage

CI runs `rustfmt`, Clippy, and ShellCheck. A compiler warning or lint finding
stops the job. CI builds a release binary with the locked Cargo dependencies.
The Cargo build runs offline.

The CI root defaults to `$XDG_STATE_HOME/action/ci`. If `XDG_STATE_HOME` has no
value, the default is `$HOME/.local/state/action/ci`. Set `ACTION_CI_ROOT` to
select a different root before the first submission. Use an absolute path
outside the worktree. Use the same CI root from all linked worktrees.

The CI root has a subdirectory for each Git common directory. The subdirectory
name is the SHA-256 digest of the absolute Git common directory path. Linked
worktrees use the same subdirectory.

```text
<ci-root>/<repository-digest>/
  telete/
  cache/source/
  cache/cargo/<compiler-fingerprint>/
  artifacts/<job-id>/check.log
  artifacts/<job-id>/package/manifest.json
  artifacts/<job-id>/package/action-<version>-<host>
```

CI uses one source path for Cargo across Telete checkouts. It compares file
contents and preserves modification times for files with the same contents.
CI removes source cache files that are absent from the check's source.

CI keeps Cargo build output by compiler and target. Subsequent jobs can use
this output again. CI keeps each release package outside the build cache.
The submission command writes the artifact directory path to standard error.

Use `./ci.sh status JOB` to examine a job with status `failed`. Read its
`check.log` for the error. Commit the repair. Submit the new commit.

### Release and install

`Cargo.toml` defines the release version. Run `./ci.sh bump-version` to increase
the patch version. Run `./ci.sh bump-version minor` to increase the minor version.
Run `./ci.sh bump-version major` to increase the major version. The script updates
`Cargo.toml` and `Cargo.lock`. Commit these changes before you submit the release.
Deployment uses the version in the package manifest.

Run a release job to do the source checks and install the binary.

```sh
./ci.sh release
./ci.sh wait JOB
```

Install a retained binary from an earlier successful check.

```sh
./ci.sh deploy --from JOB
```

Deployment makes sure that the retained binary matches its package manifest.
It installs the same binary that passed the checks. The previous executable
stays in place until the replacement is ready.

If there is a deployment error, read the job's `deploy.log`. Correct the
installation problem. Use `deploy --from` with the job that did the source
checks. Use the same command with an older job ID to restore an earlier release.

The default installation path is `$HOME/.local/bin/action`. Set
`ACTION_INSTALL_DIR` before submission to select a different directory. Use an
absolute path. Use these commands to start Action from the default directory in
the current shell.

```sh
export PATH="$HOME/.local/bin:$PATH"
action --version
```

Use the same `export` command in your shell profile to set `PATH` for future
sessions.

## Windows

1. Install Git, PowerShell 5.1 or later, Rustup, and the x64 MSVC build tools
   with a Windows SDK. Make Git and Rustup available on `PATH`. Use the Rust
   toolchain in `rust-toolchain.toml`.
2. Install [FFmpeg for Windows](https://ffmpeg.org/download.html#build-windows).
   Use the [FFmpeg requirements](../docs/editing.md) in the editing guide.
3. Run `cargo fetch --locked` in the repository before the first check and after
   each change to `Cargo.lock`. Use the same Cargo home as the CI job.
4. Commit changes. Open PowerShell in the repository root. Use a clean Git
   worktree.
5. Run `.\ci.ps1` to check the source and build a release. Record the job ID.

The job runs `rustfmt` and Clippy on a copy of the commit. It builds an
offline release for `x86_64-pc-windows-msvc` with the locked Cargo
dependencies. The job saves the binary and its manifest under
`%LOCALAPPDATA%\Action\ci` by default. Set `ACTION_CI_ROOT` to select a
different absolute path outside the worktree.

### Install on Windows

1. Run `.\ci.ps1 deploy -From JOB` to install the binary from a successful
   Windows check. Replace `JOB` with the job ID. Use `.\ci.ps1 release` to
   run a new check and install its binary in one command.
2. Add `%LOCALAPPDATA%\Action\bin` to your user `PATH`. If you set
   `ACTION_INSTALL_DIR` before installation, add that directory instead.
3. Open a new PowerShell session. Run `action.exe --version`.

Deployment checks the saved binary against its package manifest. It installs the
checked binary as `action.exe`.
