# What is and isn't tested

Keep this file current: when something listed under "Not tested" gets verified, move it to "Tested" (and say how).

## Assessment (v0.1.0)

A good port for Linux x64 and Windows x64, with the gaps listed under "Not tested".

- About 150 real-stream cases on both platforms passed, covering every flag, post-live and scheduled streams, resume, proxy, file names, and Windows path handling.
- Testing found one real bug (`--file-permissions` didn't reach the final file). Go had the same bug; it's fixed and re-verified. Every other difference from Go was either the same behavior as Go or caused by the test setup.
- It uses less memory than Go on both platforms (about 12.8 MB against 18–19 MB on Linux) and less CPU on Linux and Windows, and memory didn't grow over a 5-minute soak.
- 109 unit tests and 3 network tests pass, and CI is green on Windows, Linux, and macOS.

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
  - Executables and network: `--ffmpeg-path`, `--ytdlp-path`, `--ytdlp-info`, `-4`, `-c` with a cookies file, `--proxy` to a dead port (fails after the retries).
  - Waiting and timing: `-w`, `-n`, `-r 15`, `--live-from -1m`, `--start-delay 5s` (and its error when combined with `--live-from`, same as Go).
  - Verbosity: `-q`, `--error`, `--warn`, `-v`, `--debug`, `--trace`, `--newline`.
  - Input: channel `/live` URL, `--info-only`, `-h`, `-V`, `--members-only` on a stream that isn't members-only (fails), an invalid quality (falls back to the prompt; empty input picks `best`, same as Go).
  - `--monitor-channel`: recorded a stream, kept monitoring, and stopped cleanly on Ctrl-C.
  - Ctrl-C (SIGINT after 15 seconds) with `--merge`, `--no-merge` (asks), `--no-merge --save`, `--no-merge --no-save`, `--no-save --save-state` (temp dir and `.state` files left for resuming), and `--no-save-state`.
  - `--video-url`/`--audio-url` with googlevideo URLs from `yt-dlp -g`. The capture was 4 seconds instead of 10 because fragment length is assumed to be 5 seconds without stream info (same as Go; noted in the README).
- **Second round on Linux x64 (after the permissions fix) and the same suite on Windows x64** (nightly build, about 150 cases in total):
  - `--file-permissions 0600` on the final `.mp4`, `.mkv`, and `--separate-audio` files (default stays `644`).
  - `--live-from` with a time string, a googlevideo URL as the input URL, `--mkv` with an embedded thumbnail, `--proxy` through a working HTTP proxy.
  - Waiting for a scheduled stream in both modes, post-live downloads (ended streams that still have their live download, from the start, middle, and a negative `--live-from`).
  - `--sqlite` resume after a real interruption, and the Ctrl-C prompts answered through piped input.
  - On Windows: real downloads, Windows file names (forbidden characters, reserved names), and the MAX_PATH limit.
  - Resource use against the Go binary with `--threads 4`: about 12.8 MB peak memory against 18–19 MB for Go on Linux, lower CPU on Linux and Windows, and no memory growth over a 5-minute soak.
- **Ended streams whose live download is gone** (both modes) print "Livestream has already ended; its live download no longer exists. Use yt-dlp instead."
- **Version stamping**: release (`YTARCHIVE_CHANNEL=release`), commit/date overrides, and the default local string, checked locally with `--version`.
- **A regular (non-live) video** is rejected with the right message.
- **Nightly workflow** (run by hand twice):
  - The first run built all six targets (Windows, Linux, and macOS, each x64 and arm64, with Linux arm64 in `cross`) and published the `nightly` pre-release. The Linux arm64 build carries the right version string (`v0.1.0-03e95ee (2026-11-07)`), so the version variables reach the `cross` container.
  - The second run built only the nightly targets (Windows x64, Linux x64, macOS arm64), published them as zips, and replaced the previous pre-release; its notes link every commit. The Linux x64 zip holds a single static `ytarchive` that keeps its executable bit and prints `v0.1.0-5f65677 (2026-11-07)`.

## Not tested

- **Other platforms**: no real run on macOS, Windows arm64, or Linux arm64. CI covers only building and tests there.
- **Windows items that need a person**: turning QuickEdit off and restoring it on exit, the Ctrl-C prompts answered by hand, and keeping the PC awake while waiting for a scheduled stream.
- **Members-only streams and cookies** with a real account.
- **A stream that ends mid-download.**
- **Long-running behavior**: the 5-hour background URL refresh and URL refresh after repeated 403s. The longest real run was 5 minutes.
- **Channel monitoring of a channel going live** while being watched.
- **Retry exhaustion**: whether exiting with code 0 after saving a partial file, when fragment retries run out, matches Go.
- **The release workflow** (`release.yml`, on a `v*` tag) hadn't run before v0.1.0, so the arm64 and macOS x64 zips and the release version string had only been checked locally.
- **The nightly skip for docs-only commits.** The scheduled run has triggered and correctly skipped an unchanged `main` (after a manual run had already built it).
- **The workflow files haven't been linted** (e.g. with actionlint); they've only been checked by running.
- **An ended stream whose processed VOD is already playable**, in default mode.
- **`-6`** (the test machines had no IPv6) and a **SOCKS5 proxy**.
- **`--no-windows-filenames` on Windows** in real downloads (unit-tested only).
- **Non-UTF-8 command-line arguments and cookie files** (converted lossily; not exercised).
