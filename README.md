# pass-tui

TUI frontend for pass, the standard UNIX password manager.

## Requirements

* `pass` and `gpg` installed and available on `PATH`
* _Existing_ password store directory located at
    1. `--store`, if specified
    2. `PASSWORD_STORE_DIR` environment variable, if nonempty
    3. `~/.password-store`

## Instructions

Make sure the `EDITOR` environment varialbe points to your preferred editor and run `cargo run --release`.

To see CLI options, run `cargo run --release -- --help`.