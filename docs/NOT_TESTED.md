# What is and isn't tested

Keep this file current: when something listed under "Not tested" gets verified, move it to "Tested" (and say how).

## Tested

- **Unit and integration tests** (`cargo test`) pass on Windows, Linux, and macOS in CI, including the downloader tests against a local fake googlevideo server (ordering, resume, gaps, `--capture-duration`, `--sqlite`).
- **Real downloads on Linux** against a live 24/7 stream:
  - Default mode and `--ytdlp-info` mode, channel `/live` URL resolution, quality and codec selection, `--info-only`.
  - `--live-from now`, `--live-from -30m`/`-1m`, `--capture-duration`, `--threads`.
  - Muxing to mp4/m4a with thumbnail embedding and metadata (checked with ffprobe), templated output folders.
  - Ctrl-C with `--no-merge --no-save --save-state`, then resuming and merging; Ctrl-C discarding everything.
  - `--sqlite` download.
  - `--monitor-channel`: recording a stream, then skipping it on the next check.
  - `--ytdlp-info` with a channel `/live` URL pinning the download to the stream's watch URL (seen in the debug log; no URL refresh happened during the short test).
  - `--windows-filenames` with a deep output folder: the title was shortened to fit MAX_PATH (and fully removed, with a warning, when the folder alone was too long).
  - Titles with emoji coming through yt-dlp as UTF-8.
- **Ended streams whose live download is gone** (both modes) print "Livestream has already ended; its live download no longer exists. Use yt-dlp instead."
- **Version stamping**: release (`YTARCHIVE_CHANNEL=release`), commit/date overrides, and the default local string, checked locally with `--version`.
- **A regular (non-live) video** is rejected with the right message.

## Not tested

- **Real downloads on Windows and macOS.** Only CI tests have run there; no live stream has been downloaded on either.
- **Windows console behavior**: keeping the PC awake, ANSI colors, and turning QuickEdit off and restoring it on exit (compiled in CI, never observed running).
- **Release and nightly workflows** (`release.yml`, `nightly.yml`, `build.yml`) have never run, so none of the release executables have been built yet. Windows arm64 (cross-compiled on an x64 runner), the `cross` containers receiving the version variables and the Node 24 setting are unverified (CI itself passes with it). The workflow files haven't been linted (e.g. with actionlint) either. The Windows arm64 build needs clang for the TLS library (`ring`); it's assumed to be on the `windows-latest` runner. Release executables lose their executable bit when uploaded, so Linux/macOS users must `chmod +x` them (as the README says).
- **Ended streams that still have their live download** (post-live), and an ended stream whose processed VOD is already playable, in default mode.
- **The Windows MAX_PATH limit on real Windows** (verified only with `--windows-filenames` on Linux).
- **Scheduled streams**: waiting for an upcoming stream (wait/poll/no prompt, sleeping until the start time, late streams), in both modes.
- **Channel monitoring of a channel going live** while being watched, and `--members-only`.
- **Members-only streams and cookies** with a real account.
- **Direct googlevideo URLs** (`--video-url`, `--audio-url`, or a googlevideo URL as input).
- **Long-running behavior**: the 5-hour background URL refresh, URL refresh after repeated 403s, and streams that end mid-download (post-live).
- **`--start-delay`.**
- **`--sqlite` resume** after a real interruption (covered only by the fake-server test).
- **Interactive prompts** after Ctrl-C (merge / save files / keep state); tested only through the equivalent flags.
- **`--separate-audio`, `--write-mux-file`, `--mkv` with a thumbnail, `--keep-ts-files`**: only the generated ffmpeg arguments are unit-tested.
- **`--proxy`, `-4`, `-6`**: only checked to build a client, no traffic sent.
- **`--no-windows-filenames` on Windows** in real downloads (unit-tested only).
- **Non-UTF-8 command-line arguments and cookie files** (converted lossily; not exercised).
