# What is and isn't tested

Keep this file current: when something listed under "Not tested" gets verified, move it to "Tested" (and say how).

## Assessment (v0.1.0)

A good port for Linux x64 and Windows x64, with the gaps listed under "Not tested".

- About 150 real-stream cases on both platforms passed, covering every flag, post-live and scheduled streams, resume, proxy, file names, and Windows path handling.
- Testing found one real bug (`--file-permissions` didn't reach the final file). Go had the same bug; it's fixed and re-verified. Every other difference from Go was either the same behavior as Go or caused by the test setup.
- It uses less memory than Go on both platforms (about 12.8 MB against 18–19 MB on Linux) and less CPU on Linux and Windows, and memory didn't grow over a 5-minute soak.
- 109 unit tests and 3 network tests pass, and CI is green on Windows, Linux, and macOS.

## Tested

- **Unit and integration tests** (`cargo test`) pass on Windows, Linux, and macOS in CI, including the downloader tests against a local fake googlevideo server (ordering, resume, gaps, storage errors, `--capture-duration`, `--sqlite`, `--recover-private`) and every answer to the scheduled-stream question (including Enter and closed stdin).
- **Every flag on real streams, Linux x64 and Windows x64** (about 150 short captures with v0.1.0 nightlies, checked with ffprobe where it matters): both modes, quality/codec selection and fallback, muxing options, `--live-from`, `--capture-duration`, waiting for scheduled streams, post-live downloads, `--monitor-channel`, Ctrl-C with each merge/save choice and resuming, `--sqlite` resume, proxies, cookies, Windows file names and MAX_PATH, emoji titles. Found one bug, `--file-permissions` skipping the final file (fixed and re-verified). Same as Go: `--start-delay` with `--live-from` is an error, an empty quality prompt picks `best`, `--video-url`/`--audio-url` assume 5-second fragments.
- **Resource use** against the Go binary (`--threads 4`): about 12.8 MB peak memory against 18–19 MB on Linux, lower CPU on Linux and Windows, no growth over a 5-minute soak.
- **Changes after v0.1.0 on real streams** (nightlies `ba20b69` on Linux x64 and Windows 11 x64 and `61ea745` on Windows, by a separate tester, plus local Linux runs):
  - `--recover-private`: captures leave nothing behind; the database holds the fragments, URLs with `expire`, the stream info, and the head, also after SIGKILL or `taskkill /F`; interrupted or killed downloads resume and merge; `.ts` downloads stay `.ts`; on Windows the database is closed before the temp folder is removed. Plain `--sqlite` stores none of that.
  - `--potoken`/`--visitor-data` are rejected; repeated flags take the last value.
  - Windows console, by hand: Ctrl-C and Ctrl-Break stop cleanly (twice exits with 130), the three Ctrl-C prompts including resuming, QuickEdit restored on exit, the PC kept awake only while ytarchive runs. Closing the window leaves the files to resume and no merged file, as in Go.
  - A storage error (a resume whose `.ts` was made unreadable, Linux) stops without merging and keeps the files; after fixing it, the same command resumes and merges.
  - Bugs found and fixed: a repeated flag was an error, a killed download lost the head, and a stream whose storage couldn't be opened was still muxed.
- **Ended streams whose live download is gone** print "Livestream has already ended; its live download no longer exists. Use yt-dlp instead."; a regular video is rejected.
- **Version strings** for release, nightly, and local builds.
- **Release and nightly workflows**: the `v0.1.0` tag published all six zips; nightly builds its three targets, replaces the pre-release with linked commit notes, and skips an unchanged `main`; Linux zips are static, and `cross` builds get the version string.

## Not tested

- **Other platforms**: no real run on macOS, Windows arm64, or Linux arm64, and none of their release zips. CI covers only building and tests there.
- **Since `61ea745`, on a real run**: the scheduled-stream question (e.g. Enter on a scheduled stream), and a storage error with nothing downloaded (e.g. on Windows a title with `|` under `--no-windows-filenames`: expect no "Download Finished" or ffmpeg, no temp folder left, exit 1).
- **`--recover-private` against a real stream going private**: whether YouTube keeps serving the old URLs, the recovery rounds against googlevideo, and the 5-minute round spacing and one-hour limit in practice (only the fake server was used).
- **Retrying a kept download for real**: the "kept" message, and rerunning when the stream is private or its live download is gone (rebuilding from the database, the `-1` file), on any platform. A 20-second network cut was fully recovered by normal retries, so it didn't get that far.
- **Members-only streams and cookies** with a real account.
- **A stream that ends mid-download**, and an ended stream whose processed VOD is already playable (default mode).
- **Long-running behavior**: the 5-hour background URL refresh and URL refresh after repeated 403s. The longest real run was 5 minutes.
- **Channel monitoring of a channel going live** while being watched.
- **Retry exhaustion**: whether exiting with code 0 after saving a partial file, when fragment retries run out, matches Go.
- **`-6`** (the test machines had no IPv6), a **SOCKS5 proxy**, `--no-windows-filenames` on Windows with a title Windows accepts, and **non-UTF-8 command-line arguments and cookie files** (converted lossily).
- **The nightly skip for docs-only commits doesn't work yet.** The scheduled run on 2026-11-08 found only a `docs:` commit (`0e3bdb9`) since the last nightly (`a1d543d`) and built anyway. Running the same check script locally on those two commits skips as it should, so the cause on the runner isn't known yet. The check step now prints the tag, the commits since it, and its decision, so the next docs-only run's log should show why.
- **Publishing a nightly can leave no nightly at all.** The publish step deletes the old pre-release and tag before creating the new one. On 2026-11-08 a run for an older `main` commit deleted them, then got `HTTP 403: Resource not accessible by integration` creating the release (rerunning the job failed the same way); a new run on `main` published fine. Likely cause, not confirmed: a workflow-file change was pushed to `main` while that run was in progress, and the workflow's token can't create a tag on a commit whose workflow files differ from `main`'s.
- **The weekly nightly schedule** (Fridays 00:00 UTC) hasn't triggered yet; the earlier daily schedule did. The workflow files haven't been linted (e.g. with actionlint), only checked by running.
