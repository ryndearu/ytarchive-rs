# What isn't tested

- **`-6`** (the test machines had no IPv6).
- **Members-only streams and cookies** with a real account.
- **A stream that ends mid-download** when YouTube itself ends it.
- **Retry exhaustion against a real stream** (fragments blocked when the stream ends), only against the fake server: on a live stream the retry count resets, and on an ended stream no download starts.
- **Channel monitoring going back to `/live`** when it shows a playable stream that hasn't started.
- **`--recover-private` against a real stream going private** (only the fake server was used).
- **The 5-hour background URL refresh.**
- **`--sqlite` on a hard disk or network share**: on NVMe limited to 5 MB/s, one insert paused up to about 3.4 s.
- **A disk that fills up with `--sqlite`**, and a failed `--sqlite` export (without `--sqlite`, a full disk kept the data and state, and the next run resumed).
- **Filesystems without Unix permissions** (exFAT, FAT32, SMB shares) on Linux/macOS.
