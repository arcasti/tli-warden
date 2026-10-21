# Warden

A Windows companion app for the Tartarus prison board (season 16) of *Torchlight: Infinite*. Warden watches the game window, reads the board from the screen, and shows the move it recommends: which destination to take, which Inmate to consume or replace, and which Torture Instrument or Warden's Treasure reward to pick.

Unofficial. Not affiliated with or endorsed by the developers or publishers of *Torchlight: Infinite*. Warden only reads pixels from the game window: it never touches the game's memory, files or network traffic, and it never clicks or types for you.

## Requirements

- Windows 10 or 11.
- The game at 16:9 or wider (1920 x 1080, 2560 x 1080 and so on): fullscreen, borderless or windowed, maximized or not. Windows narrower than 16:9 are not read: the Board tab says why above the last reading and marks it "last reading".
- Any interface scale. Warden measures it from the gate icon and the back arrow, and keeps the last measured scale for screens that show neither, such as the Riot. Scales above the default have not been tested yet.
- The English Windows OCR language. Warden reads the days counter, the destination cards and the panels with it. If the Board tab says English OCR is missing, open PowerShell as administrator and run:

  ```powershell
  Add-WindowsCapability -Online -Name "Language.OCR~~~en-US~0.0.1.0"
  ```

  Then restart Warden.

## Use

Download `warden.exe` from the [latest release](https://github.com/arcasti/tli-warden/releases/latest) and run it. It offers to install itself for the current Windows user, with a Start menu entry, a desktop shortcut and an entry in Apps & features, and no admin rights; say Not now and it runs from wherever it is, with an Install button left in the settings. At start it asks GitHub for a newer release and, when there is one, shows an Update button, which downloads the new `warden.exe`, checks it against the SHA-256 GitHub lists for it, and restarts Warden on it. The check can be turned off in the settings. Uninstall in Apps & features removes the program and keeps `%APPDATA%\warden`.

Then play. The window stays on top of the game.

- Auto read (on by default) captures the game window whenever the screen changes and reads it. F8 turns it on or off and F9 reads once, but both only work while the game is not focused, because the game swallows key presses.
- The Board tab shows the ring of cells as read, the recommended move in the middle, and under the day the run's objective with its progress ("Undercurrents 1/2"), which the advice plays toward. Click a cell to correct a misread one. While a game panel covers the board, such as the Chromatic reward after a Chromatic is vaulted, the Board tab keeps the last reading: close the panel and Warden reads the cells again.
- The Moves tab ranks every option by the mean Riot loot over the playouts, and lists the reader's warnings. Each Inmate left at the Riot and each vaulted Chromatic yields a Warden's Cache of its rarity, weighted Chromatic 1, Ultimate 0.2 and Legendary 0.05.
- The Run tab lists the Torture Instruments you hold and the run's theme. Warden follows both from the game; set them there if a read missed one.

Settings are in `%APPDATA%\warden\settings.json`.

## Run logs

Each run gets a folder in `%APPDATA%\warden\runs` (the newest 10 are kept) with `log.jsonl`, one JSON line per read, advice and correction, and `frame-NNNN.jpg`, the game frames behind the reads. To report a wrong read or a bad recommendation, zip that run's folder and send it.

Warden asks once whether to share run logs. With sharing on, each finished run's log goes to warden.arcasti.dev: the board reads, the advice, the corrections, the game window's size and the OCR language, with the app version and a random install id. It never sends the screenshots or frames, window titles, or anything outside the run log. A run is sent when it ends, and at start Warden sends the runs not sent yet; only runs started after sharing was turned on go. To stop sharing, clear "Share run logs" in Settings.

## Build

Needs Rust (edition 2024) and the Windows SDK's `rc.exe`, which the Visual Studio Build Tools install, to build the icon into `warden.exe`.

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
