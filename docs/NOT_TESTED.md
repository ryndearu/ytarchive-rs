# What isn't tested

Keep this file current: when something listed here gets verified, delete its line.

- **`-6`** (the test machines had no IPv6).
- **The short post-live window** right after a stream ends.
- **Members-only streams and cookies** with a real account.
- **A stream that ends mid-download** when YouTube itself ends it.
- **Retry exhaustion**: whether exiting with code 0 after saving a partial file, when fragment retries run out, matches Go. It needs a download that is still running when the stream ends while its fragments are blocked: on a live stream the retry count is reset (a run with googlevideo blocked kept retrying for 90 s), and on an ended stream no download starts.
- **Channel monitoring going back to `/live`** when it shows a stream marked playable that hasn't started (a channel going live while being watched was found, downloaded, and monitored further, but that case didn't come up).
- **`--recover-private` against a real stream going private**: whether YouTube keeps serving the old URLs, the recovery rounds against googlevideo, and the 5-minute round spacing and one-hour limit in practice (only the fake server was used).
- **Long-running behavior**: the 5-hour background URL refresh.
- **`--sqlite` on a real hard disk or network share**: the 8 MiB write-ahead log limit was measured on an NVMe drive with writes limited to 1, 5 and 20 MB/s. The download finished normally; at 5 MB/s one insert can pause for about 3.4 seconds (1.7 with SQLite's default).
- **Resuming after a full disk**: that the saved state never counts a fragment that didn't reach the `.ts` file. On a full disk (a 4 MiB tmpfs) the download stopped with an error, ffmpeg failed, and the `.ts` files were kept, but the state files were removed, so the state couldn't be checked and the next run started over. It would only matter for a run that is killed at that moment.
