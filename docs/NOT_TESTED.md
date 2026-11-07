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
- **Flag test with the nightly Linux x64 binary** (`v0.1.0-5f65677`): about 10-second captures (`--live-from now --capture-duration 10s`) of several live streams, results checked with ffprobe where it matters. Everything below behaved as expected:
  - Muxing: `--mkv`, `--no-video`, `--no-audio`, `--separate-audio`, `-k`, `--write-mux-file`, `--codec vp9` and `vp9/h264`, `--av1`/`--vp9`/`--h264` (with fallback when the codec isn't offered), quality lists, `144p`, `360p`.
  - Fragments: `--threads 3`, `--no-frag-files`, `--sqlite`, `--retry-frags 3`.
  - Extras: `--write-description`, `--write-thumbnail`, `-t` (embedded cover), `--add-metadata`, `--metadata`.
  - Files: `-l`, `--windows-filenames`/`--no-windows-filenames` with a title containing `:` and quotes, `--temporary-dir`/`-td`, `--disable-save-state`, `--directory-permissions`. `--file-permissions` didn't reach the muxed file; fixed afterwards and checked with a real run (final and `--separate-audio` files got `0600`).
  - Executables and network: `--ffmpeg-path`, `--ytdlp-path`, `--ytdlp-info`, `-4`, `-c` with a cookies file, `--potoken`/`--visitor-data` (accepted and ignored), `--proxy` to a dead port (fails after the retries).
  - Waiting and timing: `-w`, `-n`, `-r 15`, `--live-from -1m`, `--start-delay 5s` (and its error when combined with `--live-from`, same as Go).
  - Verbosity: `-q`, `--error`, `--warn`, `-v`, `--debug`, `--trace`, `--newline`.
  - Input: channel `/live` URL, `--info-only`, `-h`, `-V`, `--members-only` on a stream that isn't members-only (fails), an invalid quality (falls back to the prompt; empty input picks `best`, same as Go).
  - `--monitor-channel`: recorded a stream, kept monitoring, and stopped cleanly on Ctrl-C.
  - Ctrl-C (SIGINT after 15 seconds) with `--merge`, `--no-merge` (asks), `--no-merge --save`, `--no-merge --no-save`, `--no-save --save-state` (temp dir and `.state` files left for resuming), and `--no-save-state`.
  - `--video-url`/`--audio-url` with googlevideo URLs from `yt-dlp -g`. The capture was 4 seconds instead of 10 because fragment length is assumed to be 5 seconds without stream info (same as Go; noted in the README).
- **Ended streams whose live download is gone** (both modes) print "Livestream has already ended; its live download no longer exists. Use yt-dlp instead."
- **Version stamping**: release (`YTARCHIVE_CHANNEL=release`), commit/date overrides, and the default local string, checked locally with `--version`.
- **A regular (non-live) video** is rejected with the right message.
- **Nightly workflow** (run by hand twice):
  - The first run built all six targets (Windows, Linux, and macOS, each x64 and arm64, with Linux arm64 in `cross`) and published the `nightly` pre-release. The Linux arm64 build carries the right version string (`v0.1.0-03e95ee (2026-11-07)`), so the version variables reach the `cross` container.
  - The second run built only the nightly targets (Windows x64, Linux x64, macOS arm64), published them as zips, and replaced the previous pre-release; its notes link every commit. The Linux x64 zip holds a single static `ytarchive` that keeps its executable bit and prints `v0.1.0-5f65677 (2026-11-07)`.

## Not tested

- **Real downloads on Windows and macOS.** Only CI tests have run there; no live stream has been downloaded on either.
- **Windows console behavior**: keeping the PC awake, ANSI colors, and turning QuickEdit off and restoring it on exit (compiled in CI, never observed running).
- **The release workflow** (`release.yml`, on a `v*` tag) has never run, so the release version string (`v0.1.0 (date)` from the tag) has only been checked locally, and the arm64 and macOS x64 builds have never been zipped.
- **The scheduled nightly** (00:00 UTC) and its skip logic (unchanged `main`, docs-only commits) haven't triggered yet; the nightly was only run by hand.
- **The Windows and macOS builds** from the workflows haven't been run on a real machine (only Linux x64 was).
- **The workflow files haven't been linted** (e.g. with actionlint); they've only been checked by running.
- **Ended streams that still have their live download** (post-live), and an ended stream whose processed VOD is already playable, in default mode.
- **The Windows MAX_PATH limit on real Windows** (verified only with `--windows-filenames` on Linux).
- **Scheduled streams**: waiting for an upcoming stream (wait/poll/no prompt, sleeping until the start time, late streams), in both modes.
- **Channel monitoring of a channel going live** while being watched.
- **Members-only streams and cookies** with a real account.
- **A googlevideo URL as the input URL** (only `--video-url`/`--audio-url` were tested).
- **`--live-from` with a time string** (e.g. `01:30:00`).
- **Long-running behavior**: the 5-hour background URL refresh, URL refresh after repeated 403s, and streams that end mid-download (post-live).
- **`--sqlite` resume** after a real interruption (covered only by the fake-server test).
- **Interactive prompts** after Ctrl-C (merge / save files / keep state) answered by a person; tested only through the equivalent flags and with no input.
- **`--mkv` with an embedded thumbnail**: only the generated ffmpeg arguments are unit-tested.
- **`-6`** (the test machine had no IPv6) and **`--proxy` with a working proxy**.
- **`--no-windows-filenames` on Windows** in real downloads (unit-tested only).
- **Non-UTF-8 command-line arguments and cookie files** (converted lossily; not exercised).
