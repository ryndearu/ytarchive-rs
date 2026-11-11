# What isn't tested

Keep this file current: when something listed here gets verified, delete its line.

## Gaps

- **The short post-live window** right after a stream ends.

## Not tested

- **`--recover-private` against a real stream going private**: whether YouTube keeps serving the old URLs, the recovery rounds against googlevideo, and the 5-minute round spacing and one-hour limit in practice (only the fake server was used).
- **Members-only streams and cookies** with a real account.
- **A stream that ends mid-download** against YouTube (only the fake server was used), and an ended stream whose processed VOD is already playable (default mode).
- **Long-running behavior**: the 5-hour background URL refresh and URL refresh after repeated 403s. The longest real run was 5 minutes.
- **Channel monitoring of a channel going live** while being watched, including going back to `/live` when it shows a stream marked playable that hasn't started (no channel had a scheduled stream; a Windows recheck only showed the retry counter).
- **Retry exhaustion**: whether exiting with code 0 after saving a partial file, when fragment retries run out, matches Go. It needs a download that is still running when the stream ends while its fragments are blocked. On a live stream the retry count is reset ("Stream still live, continuing download attempt", a debug message, as in Go): a nightly run with googlevideo blocked kept retrying for 90 s (39 resets, no exhaustion). On an ended stream ytarchive stops with "Livestream has already ended" and starts no download, so the retry path isn't reached.
- **Backing off and skipping a stuck live fragment** against YouTube: the longer waits after connection errors, 5xx, and 429 (with `Retry-After`), and giving up on one fragment that keeps failing while the rest of a live stream downloads (only unit tests).
- **Falling back to `best` on a scheduled stream** when the quality chosen while waiting isn't offered once it starts (only tried on a live stream).
- **AV1** (none of the test streams offered it), so the AV1-first default order is only covered by unit tests.
- **Proxy environment variables** (`HTTPS_PROXY` and the others) without `--proxy`.
- **`-6`** (the test machines had no IPv6).
