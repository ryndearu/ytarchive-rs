# What is and isn't tested

Keep this file current: when something listed under "Not tested" gets verified, move it to "Tested" (and say how).

## Assessment (v0.1.0)

A good port for Linux x64 and Windows x64, with the gaps listed under "Not tested".

- About 150 real-stream cases on both platforms passed, covering every flag, post-live and scheduled streams, resume, proxy, file names, and Windows path handling.
- Testing found one real bug (`--file-permissions` didn't reach the final file). Go had the same bug; it's fixed and re-verified. Every other difference from Go was either the same behavior as Go or caused by the test setup.
- It uses less memory than Go on both platforms (about 12.8 MB against 18–19 MB on Linux) and less CPU on Linux and Windows, and memory didn't grow over a 5-minute soak.
- 109 unit tests and 3 network tests pass, and CI is green on Windows, Linux, and macOS.

## Tested

- **Unit and integration tests** (`cargo test`) pass on Windows, Linux, and macOS in CI, including the downloader tests against a local fake googlevideo server (ordering, resume, gaps, `--capture-duration`, `--sqlite`, `--recover-private`).
- **Every flag on real streams, Linux x64 and Windows x64** (about 150 short captures with v0.1.0 nightlies, checked with ffprobe where it matters): both modes, quality/codec selection and fallback, muxing options, `--live-from`, `--capture-duration`, waiting for scheduled streams, post-live downloads, `--monitor-channel`, Ctrl-C with each merge/save choice and resuming, `--sqlite` resume, proxies, cookies, Windows file names and MAX_PATH, emoji titles. Found one bug, `--file-permissions` skipping the final file (fixed and re-verified). Same as Go: `--start-delay` with `--live-from` is an error, an empty quality prompt picks `best`, `--video-url`/`--audio-url` assume 5-second fragments.
- **Resource use** against the Go binary (`--threads 4`): about 12.8 MB peak memory against 18–19 MB on Linux, lower CPU on Linux and Windows, no growth over a 5-minute soak.
- **`--recover-private`**:
  - Fake-server tests: a stream going private mid-download is recovered with an earlier URL; when nothing works, recovery gives up and keeps the database; a later run fetches only the missing fragments; a killed download still has the head; without the flag nothing is recovered or kept.
  - Real runs on Linux: the database holds the fragments, URLs with `expire`, the stream info, and the head (also after SIGKILL); Ctrl-C stops cleanly and merges.
  - Nightly `ba20b69` on Linux x64 and Windows 11 x64 (by a separate tester): captures leave nothing behind, interrupted downloads resume and merge, `.ts` downloads stay `.ts`, and on Windows the database is closed before the temp folder is removed. `--potoken`/`--visitor-data` are rejected. This found two bugs (fixed): a repeated flag was an error, and a killed download lost the head.
- **Ended streams whose live download is gone** print "Livestream has already ended; its live download no longer exists. Use yt-dlp instead."; a regular video is rejected.
- **Version strings** for release, nightly, and local builds.
- **Release and nightly workflows**: the `v0.1.0` tag published all six zips; nightly builds its three targets, replaces the pre-release with linked commit notes, and skips an unchanged `main`; Linux zips are static, and `cross` builds get the version string.

## Not tested

- **Other platforms**: no real run on macOS, Windows arm64, or Linux arm64. CI covers only building and tests there.
- **Windows items that need a person**: turning QuickEdit off and restoring it on exit, the Ctrl-C prompts answered by hand, keeping the PC awake while waiting for a scheduled stream, a real Ctrl-C in a normal console with `--recover-private` (expect "User Interrupt, Stopping download..." and `download` and `head` in the database's `meta` table), and Ctrl-Break stopping the download (only type-checked for Windows; CI builds it). Closing the console window still ends ytarchive without merging (as in Go).
- **Members-only streams and cookies** with a real account.
- **A stream that ends mid-download.**
- **`--recover-private` against a real stream going private**: whether YouTube keeps serving the old URLs, the recovery rounds against googlevideo, and the 5-minute round spacing and one-hour limit in practice (only the fake server was used).
- **Retrying a kept download for real**: the "kept" message, rerunning when the stream is private or its live download is gone (rebuilding from the database, the `-1` file), and on Windows reopening the kept database.
- **Long-running behavior**: the 5-hour background URL refresh and URL refresh after repeated 403s. The longest real run was 5 minutes.
- **Channel monitoring of a channel going live** while being watched.
- **Retry exhaustion**: whether exiting with code 0 after saving a partial file, when fragment retries run out, matches Go.
- **The release zips other than Linux x64** (Windows and macOS, and every arm64 one) haven't been run on a real machine.
- **The nightly skip for docs-only commits doesn't work yet.** The scheduled run on 2026-11-08 found only a `docs:` commit (`0e3bdb9`) since the last nightly (`a1d543d`) and built anyway. Running the same check script locally on those two commits skips as it should, so the cause on the runner isn't known yet. The check step now prints the tag, the commits since it, and its decision, so the next docs-only run's log should show why.
- **Publishing a nightly can leave no nightly at all.** The publish step deletes the old pre-release and tag before creating the new one. On 2026-11-08 a run for an older `main` commit deleted them, then got `HTTP 403: Resource not accessible by integration` creating the release (rerunning the job failed the same way); a new run on `main` published fine. Likely cause, not confirmed: a workflow-file change (the nightly check's debug output) was pushed to `main` while that run was in progress, and the workflow's token can't create a tag on a commit whose workflow files differ from `main`'s.
- **The weekly nightly schedule** (Fridays 00:00 UTC) hasn't triggered yet; the earlier daily schedule did.
- **The workflow files haven't been linted** (e.g. with actionlint); they've only been checked by running.
- **An ended stream whose processed VOD is already playable**, in default mode.
- **`-6`** (the test machines had no IPv6) and a **SOCKS5 proxy**.
- **`--no-windows-filenames` on Windows** in real downloads (unit-tested only).
- **Non-UTF-8 command-line arguments and cookie files** (converted lossily; not exercised).
