# What is and isn't tested

Keep this file current: when something listed under "Not tested" gets verified, move it to "Tested" (and say how).

## Tested

- **Unit and integration tests** (`cargo test`) pass on Windows, Linux, and macOS in CI, including the downloader tests against a local fake googlevideo server (ordering, resume, gaps, storage errors, `--capture-duration`, `--sqlite`, `--recover-private`) and every answer to the scheduled-stream question.
- **Every flag on real streams, Linux x64 and Windows x64** (about 150 short captures with v0.1.0 nightlies, checked with ffprobe where it matters): both modes, quality/codec selection and fallback, muxing options, `--live-from`, `--capture-duration`, waiting for scheduled streams, post-live downloads, `--monitor-channel`, Ctrl-C with each merge/save choice and resuming, `--sqlite` resume, proxies, cookies, Windows file names and MAX_PATH, emoji titles. Found one bug, `--file-permissions` skipping the final file (fixed and re-verified). Same as Go: `--start-delay` with `--live-from` is an error, an empty quality prompt picks `best`, `--video-url`/`--audio-url` assume 5-second fragments.
- **Resource use** against the Go binary (`--threads 4`): about 12.8 MB peak memory against 18–19 MB on Linux, lower CPU on Linux and Windows, no growth over a 5-minute soak.
- **Changes after v0.1.0 on real streams** (nightlies `ba20b69` on Linux x64 and Windows 11 x64 and `61ea745` on Windows, by a separate tester, plus local Linux runs):
  - `--recover-private`: captures leave nothing behind; the database holds the fragments, URLs with `expire`, the stream info, and the head, also after SIGKILL or `taskkill /F`; interrupted or killed downloads resume and merge; `.ts` downloads stay `.ts`; on Windows the database is closed before the temp folder is removed. Plain `--sqlite` stores none of that.
  - `--potoken`/`--visitor-data` are rejected; repeated flags take the last value.
  - Windows console, by hand: Ctrl-C and Ctrl-Break stop cleanly (twice exits with 130), the three Ctrl-C prompts including resuming, QuickEdit restored on exit, the PC kept awake only while ytarchive runs. Closing the window leaves the files to resume and no merged file, as in Go.
  - A storage error (a resume whose `.ts` was made unreadable, Linux) stops without merging and keeps the files; after fixing it, the same command resumes and merges.
  - Bugs found and fixed: a repeated flag was an error, a killed download lost the head, and a stream whose storage couldn't be opened was still muxed.
- **SOCKS5 proxy, Windows 11 x64** (`--proxy socks5://127.0.0.1:...` through a local proxy, 90 s): the player response and both media streams were downloaded (43 video and 43 audio fragments). The media hosts reached the proxy as IP addresses, so they are resolved locally, not by the proxy.
- **Retrying a kept download after the stream ended, Windows** (nightly `61ea745`): the same command prints "The stream can no longer be looked up; retrying the missing fragments ...". The URLs had expired ("The download URLs have expired; N fragment(s) can't be recovered"), so only the fragments kept earlier (about 7 minutes) were merged, with a "Mismatched number of video and audio fragments" warning, and the temp folder, `.state` and `.sqlite` were removed without asking.
- **Ended streams whose live download is gone** print "Livestream has already ended; its live download no longer exists. Use yt-dlp instead."; a regular video is rejected.
- **Version strings** for release, nightly, and local builds.
- **Release and nightly workflows**: the `v0.1.0` tag published all six zips; nightly builds its three targets, replaces the pre-release with linked commit notes, and skips an unchanged `main`; Linux zips are static, and `cross` builds get the version string.

## Not tested

- **Other platforms**: no real run on macOS, Windows arm64, or Linux arm64, and none of their release zips. CI covers only building and tests there.
- **Since `61ea745`, on a real run**: the numbered scheduled-stream question, and a storage error with nothing downloaded (e.g. on Windows a title with `|` under `--no-windows-filenames`: expect no "Download Finished" or ffmpeg, no temp folder left, exit 1).
- **`--recover-private` against a real stream going private**: whether YouTube keeps serving the old URLs, the recovery rounds against googlevideo, and the 5-minute round spacing and one-hour limit in practice (only the fake server was used).
- **Retrying a kept download while its URLs are still valid**: the "kept" message and the `-1` file (the Windows run above only covered expired URLs). A 20-second network cut was fully recovered by normal retries, so it didn't get that far.
- **Members-only streams and cookies** with a real account.
- **The repaired cookies copy for yt-dlp on Windows and macOS** (temp folder, deletion on exit and after a double Ctrl-C). On Linux a file with an invalid byte, no header, and a mismatched subdomain column downloaded fine (and 15 sample files, including JSON-like lines, matched what yt-dlp accepts), left the original untouched, and the copy was deleted.
- **A stream that ends mid-download**, and an ended stream whose processed VOD is already playable (default mode).
- **Long-running behavior**: the 5-hour background URL refresh and URL refresh after repeated 403s. The longest real run was 5 minutes.
- **Channel monitoring of a channel going live** while being watched, including going back to `/live` when it shows a stream marked playable that hasn't started (no test covers that path).
- **Retry exhaustion**: whether exiting with code 0 after saving a partial file, when fragment retries run out, matches Go.
- **Falling back to `best`** when the quality chosen while waiting for a scheduled stream isn't offered once it starts (a live stream asked for `2160p60` fell back to 1080p VP9 without asking, Linux).
- **AV1** (none of the test streams offered it), so the AV1-first default order is only covered by unit tests.
- **`-6`** (the test machines had no IPv6) and **non-UTF-8 command-line arguments** (converted lossily) and **cookie values with bytes Go drops** (non-ASCII, `;`, `\`; only covered by unit tests).
