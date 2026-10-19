# Warden

A Windows companion app for the Tartarus prison board (season 16) of *Torchlight: Infinite*. Warden watches the game window, reads the board from the screen, and shows the move it recommends: which destination to take, which Inmate to consume or replace, and which Torture Instrument or Warden's Treasure reward to pick.

Unofficial. Not affiliated with or endorsed by the developers or publishers of *Torchlight: Infinite*. Warden only reads pixels from the game window: it never touches the game's memory, files or network traffic, and it never clicks or types for you.

## Requirements

- Windows 10 or 11.
- The game at a 16:9 size (1920 x 1080 or any other 16:9 size): fullscreen, borderless, or a window that is not maximized (a maximized window loses the title bar and taskbar height and is no longer 16:9). Other shapes lay the board out differently and are not read: the Board tab says why above the last reading and marks it "last reading".
- The English Windows OCR language. Warden reads the days counter, the destination cards and the panels with it. If the Board tab says English OCR is missing, open PowerShell as administrator and run:

  ```powershell
  Add-WindowsCapability -Online -Name "Language.OCR~~~en-US~0.0.1.0"
  ```

  Then restart Warden.

## Use

Start `warden.exe` and play. The window stays on top of the game.

- Auto read (on by default) captures the game window whenever the screen changes and reads it. F8 turns it on or off and F9 reads once, but both only work while the game is not focused, because the game swallows key presses.
- The Board tab shows the ring of cells as read, the recommended move in the middle and the move after it, and under the day the run's objective with its progress ("Undercurrents 1/2"), which the advice plays toward. Click a cell to correct a misread one.
- The Moves tab ranks every option by the mean number of Chromatic Inmates vaulted over the playouts, and lists the reader's warnings.
- The Run tab lists the Torture Instruments you hold and the run's theme. Warden follows both from the game; set them there if a read missed one.

Settings are in `%APPDATA%\warden\settings.json`.

## Run logs

Each run gets a folder in `%APPDATA%\warden\runs` (the newest 10 are kept) with `log.jsonl`, one JSON line per read, advice and correction, and `frame-NNNN.jpg`, the game frames behind the reads. To report a wrong read or a bad recommendation, zip that run's folder and send it.

## Build

Needs Rust (edition 2024).

```powershell
cargo build --release --bin warden        # target\release\warden.exe
cargo build --release --bin warden-cli    # the command line: solve, read, capture, bench
cargo test --profile fast-test
```

`warden-cli read <frame.png>` reads a saved screenshot and prints the board model, and `warden-cli solve <frame.png | board.json>` ranks the moves. `warden-cli bench --strategy-only --set w_step=0` plays 2000 runs with the strategy alone on the tuned weights and on the changed ones, and prints the Chromatics per run of each and the difference paired by run. `--hold guillotine` starts every run holding that instrument, and `--save file` on one build with `--against file` on another prints the paired difference between the two builds. The strategy tests compare 20 games and the solver's rankings in them with snapshots in `tests/fixtures`; after a deliberate strategy change, rerun `WARDEN_RECORD=1 cargo test --profile fast-test --test decide --test solve` to re-record them and review the fixture diff. The reader tests that need game screenshots look for them in `trainingdata/s16` and skip when it is missing; the screenshots are not in the repository.

## How it works

- `src/engine`: the board rules, from the game's client data (`data/warden-data.json`).
- `src/solver`: the strategy and the playouts that rank the moves.
- `src/reader`: the screen reader. Pixel templates find cells, beasts, rarities and the warden; Windows OCR reads the text.
- `src/app`: the window, the game capture (Windows Graphics Capture), auto read and the run log.

`docs/s16-screen.md` describes the prison screen as the reader sees it.

## More tools

Other Torchlight: Infinite tools are on [arcasti.dev](https://arcasti.dev/).

## License

GPL-3.0. See [LICENSE](LICENSE).
