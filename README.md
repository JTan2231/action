# Action

Action edits local video from the command line. It saves trim and concatenation
results as project clips. FFmpeg reads the source files and renders the export.

## Documentation

- [Editing](docs/editing.md) describes project clips, commands, and exports.
- [Repository tooling](scripts/README.md) describes CI, releases, and installation.

## CI

Commit the changes. Run these commands from the repository root. Replace
`JOB` with the job ID from the submission receipt.

```sh
./ci.sh
./ci.sh wait JOB
```

Use the [release procedure](scripts/ci.md#release-and-install) to install a
checked binary.
