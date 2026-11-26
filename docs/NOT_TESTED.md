# What isn't tested

- **`-6`** (the test machines had no IPv6).
- **Members-only streams and cookies** with a real account.
- **A stream that ends mid-download** when YouTube itself ends it.
- **Retry exhaustion against a real stream** (fragments blocked when the stream ends), only against the fake server: on a live stream the retry count resets, and on an ended stream no download starts.
- **Near-head transport error retries on a real ended stream** (tested only against the fake server).
- **Channel monitoring going back to `/live` against a real channel** when it shows a playable stream that hasn't started (only the fake server was used).
- **`--recover-private` against a real stream going private** (only the fake server was used).
- **The 5-hour background URL refresh.**
- **`--sqlite` on a hard disk or network share**: on NVMe limited to 5 MB/s, one insert paused up to about 3.4 s.
- **Sleep/suspend during waits:** scheduled streams, `-r`, `--monitor-channel`, `--start-delay`, `--recover-private` rounds, and the 15-second extraction cooldown.
- **Windows rename retries** while antivirus holds a `.state` or storage file.
- **The default consent cookie from an EU/EEA address.**
