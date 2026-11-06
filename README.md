# ytarchive-rs

Archive a YouTube livestream from the start, or wait for a scheduled stream
and record it as soon as it begins. A Rust port of
[dreammu/ytarchive](https://github.com/dreammu/ytarchive), itself a fork of
[Kethsar/ytarchive](https://github.com/Kethsar/ytarchive).

For finished videos (VODs), use [yt-dlp](https://github.com/yt-dlp/yt-dlp)
instead.

## Features

- Downloads live streams from the beginning (up to YouTube's 7-day seek
  window), or from a chosen point with `--live-from`.
- Waits for scheduled streams: until the scheduled time, or by polling.
- Watches a channel and records each new stream (`--monitor-channel`),
  including members-only streams with cookies.
- Parallel fragment downloads, URL refresh when fragments start failing, and a
  background refresh every 5 hours before URLs expire.
- Resumable downloads: interrupt with Ctrl-C, keep the state, rerun to continue.
- Optional SQLite fragment storage (`--sqlite`) that resumes by re-fetching
  only missing fragments.
- Muxes to mp4/mkv/m4a with ffmpeg, with optional thumbnail, metadata, and
  description files.

## Platforms

| Platform | Status |
| --- | --- |
| Linux (x86_64, arm64) | Supported |
| Windows (x64) | Supported |
| macOS (Apple Silicon, Intel) | Supported (secondary) |

## Requirements

- [ffmpeg](https://ffmpeg.org/) to mux the final file.
- [yt-dlp](https://github.com/yt-dlp/yt-dlp), kept up to date. It extracts the
  download URLs.
- A JavaScript runtime for yt-dlp's YouTube support:
  [deno](https://deno.com/) by default. If you only have node, add
  `--ytdlp-opts "--js-runtimes node"`.

ytarchive looks for `ffmpeg` and `yt-dlp` on your `PATH`. On Windows you can
also put `ffmpeg.exe` and `yt-dlp.exe` in the same folder as `ytarchive.exe`.

## Installing

Download a release archive for your platform from the
[releases page](../../releases), extract it, and run `ytarchive` from a
terminal. The Linux builds are static and run on any distribution.

### Linux

```sh
# Arch:          sudo pacman -S ffmpeg yt-dlp deno
# Debian/Ubuntu: sudo apt install ffmpeg, then install yt-dlp and deno
#                from their websites (distro yt-dlp packages are often old)
tar xzf ytarchive-*-x86_64-unknown-linux-musl.tar.gz
./ytarchive-*/ytarchive --help
```

### Windows

```powershell
winget install Gyan.FFmpeg yt-dlp.yt-dlp DenoLand.Deno
# then extract the .zip and run, from PowerShell or cmd:
.\ytarchive.exe --help
```

While running on Windows, ytarchive keeps the PC from going to sleep and
turns off the console's QuickEdit mode (otherwise a stray click in the
window pauses output and stalls the download). Both are restored on exit.

### macOS

```sh
brew install ffmpeg yt-dlp deno
tar xzf ytarchive-*-aarch64-apple-darwin.tar.gz   # Intel: x86_64-apple-darwin
xattr -d com.apple.quarantine ytarchive-*/ytarchive   # unsigned binary
./ytarchive-*/ytarchive --help
```

### Building from source

Needs Rust (stable) and a C compiler (for SQLite and the TLS library;
on Windows, the Visual Studio Build Tools that rustup asks for):

```sh
cargo build --release
# binary: target/release/ytarchive (ytarchive.exe on Windows)
```

## Usage

```
ytarchive [OPTIONS] [URL] [QUALITY]
```

`URL` can be a watch URL (`youtube.com/watch?v=`, `youtu.be/`, `/live/ID`), a
channel URL (`/@name`, `/channel/ID`, `/c/name`; its `/live` page is used), or
a direct googlevideo fragment URL. `QUALITY` is a slash-delimited preference
list such as `1080p60/720p/best`. Either one is prompted for if omitted.

```sh
# (Windows: same commands with ytarchive.exe and double quotes)
# Record a stream from the beginning in the best quality
ytarchive https://www.youtube.com/watch?v=VIDEO_ID best

# Wait for a scheduled stream, 4 threads, embed thumbnail, add metadata
ytarchive -w --threads 4 -t --add-metadata URL 1080p60/best

# Only the last 30 minutes, then 10 more minutes of the stream
ytarchive --live-from -30m --capture-duration 40m URL best

# Keep recording a channel's streams, checking every 2 minutes
ytarchive --monitor-channel -r 120 -o '%(channel)s/%(upload_date)s %(title)s' \
    https://www.youtube.com/@channel 1080p/best

# Print stream info only
ytarchive --info-only URL best
```

Run `ytarchive --help` for every option. Flag names match Go ytarchive, and
Go-style single-dash long flags (`-threads 4`, `-td DIR`) are still accepted.

On Windows, use double quotes instead of single quotes around templates, e.g.
`-o "%(channel)s/%(title)s"`. In cmd (not PowerShell), write `%` as `%%`
inside batch files.

File names are made valid on every OS: characters like `<>:"/\|?*` and control
characters are replaced, and names Windows reserves (`CON`, `NUL`, …) or that
end in a dot or space are adjusted.

### Output template keys

`-o` accepts `%(key)s` placeholders: `id`, `title`, `channel_id`, `channel`,
`upload_date`, `start_date`, `publish_date`, `year`, `month`, `day`,
`start_time`, `hours`, `minutes`, `seconds`, `url`. `description` is accepted
but expands to nothing in file names.

### PO tokens and cookies

yt-dlp handles extraction, so pass PO token settings to it with
`--ytdlp-opts`, e.g. `--ytdlp-opts "--extractor-args youtube:po_token=..."`.
`--potoken` and `--visitor-data` are still accepted but ignored. Cookies
(`-c cookies.txt`, Netscape format) are used both by yt-dlp and by the
fragment downloads.

## Differences from the Go version

See [docs/CHANGES_FROM_GO.md](docs/CHANGES_FROM_GO.md) for the full list:
behavior changes, new options, Go bugs fixed during the port, and Go code that
was not ported because it was unreachable.

## Credits

- [Kethsar/ytarchive](https://github.com/Kethsar/ytarchive): the original
  ytarchive. This port follows its design, flags, and file formats.
- [dreammu/ytarchive](https://github.com/dreammu/ytarchive): the fork this port
  is based on (yt-dlp extraction, background URL refresh, fragment retry
  logic, `--ytdlp-info`).
- [CanOfSocks/livestream_dl](https://github.com/CanOfSocks/livestream_dl): the
  idea behind `--sqlite` storage and resuming by re-fetching only gaps.

## License

MIT; see [LICENSE](LICENSE). Kethsar/ytarchive's copyright notice is retained
as its license requires.
