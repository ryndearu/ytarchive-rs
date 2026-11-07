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
- **A regular (non-live) video** is rejected with the right message.

## Not tested

- **Real downloads on Windows and macOS.** Only CI tests have run there; no live stream has been downloaded on either.
- **Windows console behavior**: keeping the PC awake and ANSI colors (compiled in CI, never observed running).
- **Release and nightly workflows** (`release.yml`, `nightly.yml`, `build.yml`) have never run, so none of the release archives (static Linux musl, Windows, macOS) have been built yet.
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
- **`--windows-filenames` on Linux/macOS and `--no-windows-filenames` on Windows** in real downloads (unit-tested only).
