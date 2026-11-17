# Action

Action edits local video in a desktop window and from the command line. It saves
video and audio edits as project clips. FFmpeg reads the source files and renders
the export.

## Documentation

- [Editing](docs/editing.md) describes the desktop editor, commands, and exports.
- [Repository tooling](scripts/README.md) describes CI, releases, and installation.

## CI

Commit the changes. Run these commands from the repository root. Replace
`JOB` with the job ID from the submission receipt.

```sh
./ci.sh
./ci.sh wait JOB
```

Run the checks in PowerShell on Windows.

```powershell
.\ci.ps1
```

Install a checked binary with the [macOS procedure](scripts/ci.md#release-and-install)
or the [Windows procedure](scripts/ci.md#install-on-windows).
