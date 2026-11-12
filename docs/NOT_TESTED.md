# What isn't tested

Keep this file current: when something listed here gets verified, delete its line.

- **`-6`** (the test machines had no IPv6).
- **The short post-live window** right after a stream ends.
- **Members-only streams and cookies** with a real account.
- **A stream that ends mid-download** against YouTube (only the fake server was used).
- **Retry exhaustion**: whether exiting with code 0 after saving a partial file, when fragment retries run out, matches Go. It needs a download that is still running when the stream ends while its fragments are blocked: on a live stream the retry count is reset (a run with googlevideo blocked kept retrying for 90 s), and on an ended stream no download starts.
- **Backing off after 5xx and 429, and skipping a stuck live fragment** against YouTube: the longer waits after a 5xx or a 429 (with `Retry-After`), and giving up on one fragment that keeps failing while the rest of a live stream downloads (only unit tests). With googlevideo blocked, the waits grew to the 15-second limit and nothing was skipped.
- **Channel monitoring going back to `/live`** when it shows a stream marked playable that hasn't started (a channel going live while being watched was found, downloaded, and monitored further, but that case didn't come up).
- **`--recover-private` against a real stream going private**: whether YouTube keeps serving the old URLs, the recovery rounds against googlevideo, and the 5-minute round spacing and one-hour limit in practice (only the fake server was used).
- **Long-running behavior**: the 5-hour background URL refresh and URL refresh after repeated 403s.
- **A full disk while writing the `.ts` file**: that the saved state then never counts a fragment that didn't reach the file, so the next run resumes instead of starting over.
