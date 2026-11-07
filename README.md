# ytarchive-rs

Archive a YouTube livestream from the start, or wait for a scheduled stream and record it as soon as it begins. A Rust port of [dreammu/ytarchive](https://github.com/dreammu/ytarchive), itself a fork of [Kethsar/ytarchive](https://github.com/Kethsar/ytarchive).

For finished videos (VODs), use [yt-dlp](https://github.com/yt-dlp/yt-dlp) instead.

## Features

- Downloads live streams from the beginning (up to YouTube's 7-day seek window), or from a chosen point with `--live-from`.
- Downloads a stream that just ended as long as its live download still exists; once it's gone, says so ("Livestream has already ended; its live download no longer exists. Use yt-dlp instead.").
- Waits for scheduled streams: until the scheduled time, or by polling.
- Watches a channel and records each new stream (`--monitor-channel`), including members-only streams with cookies. Each download sticks to the stream it started on, even if the channel goes live again meanwhile.
- Parallel fragment downloads, URL refresh when fragments start failing, and a background refresh every 5 hours before URLs expire.
- Resumable downloads: interrupt with Ctrl-C, keep the state, rerun to continue.
- Optional SQLite fragment storage (`--sqlite`) that resumes by re-fetching only missing fragments.
- Muxes to mp4/mkv/m4a with ffmpeg, with optional thumbnail, metadata, and description files.
- Video codec choice with `--codec` (H.264 by default, see [Quality and codec](#quality-and-codec)).

## Platforms

These platforms are supported, each as x64 and arm64:

- Windows
- Linux
- macOS

## Requirements

- [ffmpeg](https://ffmpeg.org/) to mux the final file.
- [yt-dlp](https://github.com/yt-dlp/yt-dlp), kept up to date. It extracts the download URLs. The **nightly** channel is recommended, since YouTube changes often and fixes land there first: `yt-dlp --update-to nightly` (or `pip install -U --pre "yt-dlp[default]"` for pip installs).
- A JavaScript runtime for yt-dlp's YouTube support: [deno](https://deno.com/) by default. If you only have node, add `--ytdlp-opts "--js-runtimes node"`.

ytarchive looks for `ffmpeg` and `yt-dlp` on your `PATH`. On Windows you can also put `ffmpeg.exe` and `yt-dlp.exe` in the same folder as `ytarchive.exe`.

## Installing

Download the executable for your platform from the [releases page](../../releases) and run it from a terminal. There is nothing to extract:

| Platform | x64 | arm64 |
| --- | --- | --- |
| Windows | `ytarchive_windows_x64.exe` | `ytarchive_windows_arm64.exe` |
| Linux | `ytarchive_linux_x64` | `ytarchive_linux_arm64` |
| macOS | `ytarchive_macos_x64` (Intel) | `ytarchive_macos_arm64` (Apple Silicon) |

The Linux builds are static and run on any distribution. Rename the file to `ytarchive` (`ytarchive.exe` on Windows) if you like.

- **Releases** (`v*`) are published manually and are the recommended builds. `ytarchive --version` shows the version and build date, e.g. `ytarchive v0.1.0 (2026-11-07)`.
- **Nightly** is a pre-release rebuilt each day when `main` has new code commits (docs-only changes don't trigger it). It replaces the previous nightly, and its description lists the commits since the last release. Its version includes the commit, e.g. `ytarchive v0.1.0-abc1234 (2026-11-07)`.

The version is also printed when ytarchive starts (unless `-q`).

### Linux

```sh
# Arch:          sudo pacman -S ffmpeg deno
# Debian/Ubuntu: sudo apt install ffmpeg, then install deno from its website
# yt-dlp nightly (distro packages are stable-only and often old):
#   download the yt-dlp binary from https://github.com/yt-dlp/yt-dlp-nightly-builds/releases, or
#   pip install -U --pre "yt-dlp[default]"
mv ytarchive_linux_x64 ytarchive   # arm64: ytarchive_linux_arm64
chmod +x ytarchive
./ytarchive --help
```

### Windows

```powershell
winget install Gyan.FFmpeg yt-dlp.yt-dlp DenoLand.Deno
yt-dlp --update-to nightly
# then, from PowerShell or cmd (arm64: ytarchive_windows_arm64.exe):
.\ytarchive_windows_x64.exe --help
```

While running on Windows, ytarchive keeps the PC from going to sleep and turns off the console's QuickEdit mode, so a stray click in the window can't pause the download. The console's previous setting is restored when ytarchive exits.

### macOS

```sh
brew install ffmpeg deno
# yt-dlp nightly: download yt-dlp_macos from https://github.com/yt-dlp/yt-dlp-nightly-builds/releases (Homebrew only has stable)
mv ytarchive_macos_arm64 ytarchive   # Intel: ytarchive_macos_x64
chmod +x ytarchive
xattr -d com.apple.quarantine ytarchive   # unsigned binary
./ytarchive --help
```

### Building from source

Needs Rust (stable) and a C compiler (for SQLite and the TLS library; on Windows, the Visual Studio Build Tools that rustup asks for):

```sh
cargo build --release
# binary: target/release/ytarchive (ytarchive.exe on Windows)
```

## Usage and options

    ytarchive [OPTIONS] [URL] [QUALITY]

`URL` can be a watch URL (`youtube.com/watch?v=`, `youtu.be/`, `/live/ID`), a channel URL (`/@name`, `/channel/ID`, `/c/name`; its `/live` page is used), or a direct googlevideo fragment URL. `QUALITY` is a slash-delimited preference list such as `1080p60/720p/best`. Either one is prompted for if omitted; with `--info-only`, only the URL is needed.

Flag names match Go ytarchive, and Go-style single-dash long flags (`-threads 4`, `-td DIR`, `-dp`, `-fp`) are still accepted. `ytarchive --help` prints the same list.

### General Options:
    -h, --help             Print this help text and exit
    -V, --version          Print the version and exit
        --info-only        Print stream information (title, available and selected quality, duration) and exit. Only the URL is needed
        --monitor-channel  Continually monitor a channel URL for new streams (requires URL and QUALITY)
        --members-only     Only download members-only streams (channel URLs; requires cookies)

### Waiting Options:
    -w, --wait                    Wait for a scheduled stream to start
    -n, --no-wait                 Do not wait for a scheduled stream to start
    -r, --retry-stream <SECS>     Seconds between stream status checks while waiting (min 15; 30 when monitoring)
        --start-delay <DURATION>  Wait this long after the stream starts before capturing

### Download Options:
        --live-from <WHEN>             Start from a point in the stream: duration (-30m), time string, or 'now'
        --capture-duration <DURATION>  Capture this much of the stream, then finish
        --threads <N>                  Download threads per stream type [default: 1]
        --retry-frags <ATTEMPTS>       Attempts per fragment before refreshing URLs / giving up (0 = unlimited) [default: 10]
        --no-frag-files                Keep out-of-order fragments in memory instead of temp files
        --sqlite                       Store fragments in a SQLite database (more robust to gaps)

### Format Selection Options:
        --codec <CODECS>  Video codec preference, e.g. vp9/h264. Default: h264, or best when the quality is best
        --av1             Prefer AV1 video when available (same as --codec av1)
        --vp9             Prefer VP9 video when available (same as --codec vp9)
        --h264            Prefer H.264 video (same as --codec h264)
        --no-video        Only download the audio stream
        --no-audio        Only download the video stream

### Filesystem Options:
    -o, --output <FORMAT>               Output filename template, e.g. '%(channel)s/%(upload_date)s %(title)s' [default: %(title)s-%(id)s]
        --temporary-dir <DIR>           Directory for temporary files (created if missing; also -td)
        --windows-filenames             Force filenames to be Windows-compatible (default on Windows)
        --no-windows-filenames          Sanitize filenames only minimally (default on other than Windows)
    -l, --lookalike-chars               Use lookalike characters in place of forbidden filename characters
        --write-description             Write the description to a separate file
        --write-thumbnail               Write the thumbnail to a separate file
    -k, --keep-ts-files                 Keep the raw .ts files after muxing
        --disable-save-state            Disable resumable download state entirely
        --file-permissions <MODE>       Permissions for created files (octal; also -fp) [default: 0644]
        --directory-permissions <MODE>  Permissions for created directories (octal; also -dp) [default: 0755]

### Post-Processing Options:
    -t, --thumbnail             Embed the thumbnail into the final file
        --add-metadata          Write basic metadata to the final file
        --metadata <KEY=VALUE>  Override metadata, KEY=VALUE (VALUE is a format template; empty omits the key)
        --mkv                   Mux into mkv instead of mp4 (ignored for audio only)
        --separate-audio        Also save the audio as a separate file
        --write-mux-file        Write the ffmpeg mux command to a file instead of muxing
        --ffmpeg-path <PATH>    ffmpeg executable to use [default: ffmpeg]

### Interrupt (Ctrl-C) Options:
        --merge          Automatically merge files when the download is cancelled
        --no-merge       Skip merging files when the download is cancelled
        --save           Keep downloaded files on cancel if not merging
        --no-save        Delete downloaded files on cancel if not merging
        --save-state     Leave files for resuming later on cancel, if not saving files
        --no-save-state  Do not leave files for resuming later on cancel

### Extraction Options:
    -c, --cookies <FILE>               Netscape-format cookies file (needed for members-only streams)
        --ytdlp-path <PATH>            yt-dlp executable used for URL extraction [default: yt-dlp]
        --ytdlp-opts <OPTIONS>         Extra arguments for yt-dlp, e.g. '--extractor-args youtube:player_client=web'
        --ytdlp-info                   Let yt-dlp extract stream info; ytarchive only downloads fragments
        --video-url <URL>              Googlevideo URL to use for the video stream
        --audio-url <URL>              Googlevideo URL to use for the audio stream (itag 140)
        --potoken <TOKEN>              Ignored (accepted for compatibility); pass PO tokens to yt-dlp via --ytdlp-opts
        --visitor-data <VISITOR_DATA>  Ignored (accepted for compatibility); pass visitor data to yt-dlp via --ytdlp-opts

### Network Options:
    -4, --ipv4         Force IPv4
    -6, --ipv6         Force IPv6
        --proxy <URL>  Proxy URL (http, https, or socks5)

### Verbosity Options:
    -q, --quiet    Print nothing except prompts
        --error    Print only errors
        --warn     Print warnings and errors (default)
    -v, --verbose  Verbose output
        --debug    Debug output
        --trace    Trace output
        --newline  Print progress on new lines instead of updating one line

Durations accept units (`1d8h10m`, `90s`) or time strings (`01:30:00`). `--live-from` also accepts negative durations (`-30m`) and `now`.

### Quality and codec

`QUALITY` values, from most to least wanted, separated by `/`: `audio_only`, `144p`, `240p`, `360p`, `480p`, `720p`, `720p60`, `1080p`, `1080p60`, `1440p`, `1440p60`, `2160p`, `2160p60`, `best`. The first one the stream offers is used.

`--codec` takes a slash-delimited list too: `av1`, `vp9`, `h264`, or `best` (= `av1/vp9/h264`). Codecs you don't list are used as a fallback when none of the listed ones are offered. Go's `--av1`, `--vp9`, and `--h264` still work and mean the same as `--codec` with that one codec.

Without a codec option, **H.264** is preferred (it plays everywhere), falling back to AV1 and then VP9. The exception is quality `best` (given, picked at the prompt, or the `--info-only` default): it uses codec `best`, AV1 > VP9 > H.264. In a list like `1080p/best`, `1080p` uses H.264 and `best` (if it's reached) uses AV1 > VP9 > H.264. The codec is never asked for.

### Examples

```sh
# (Windows: same commands with ytarchive.exe and double quotes)
# Record a stream from the beginning in the best quality (and best codec)
ytarchive https://www.youtube.com/watch?v=VIDEO_ID best

# Wait for a scheduled stream, 4 threads, embed thumbnail, add metadata
ytarchive -w --threads 4 -t --add-metadata URL 1080p60/best

# 1080p in H.264 (the default codec; AV1, then VP9 if H.264 isn't offered)
ytarchive URL 1080p

# 1080p in VP9, falling back to H.264
ytarchive --codec vp9/h264 URL 1080p

# Best quality, but in H.264 instead of the best codec
ytarchive --codec h264 URL best

# 1440p60, else 1080p60, in the best codec offered (AV1 > VP9 > H.264)
ytarchive --codec best URL 1440p60/1080p60

# Go-style flag, same as --codec av1
ytarchive --av1 URL 1080p

# Only the last 30 minutes, then 10 more minutes of the stream
ytarchive --live-from -30m --capture-duration 40m URL best

# Keep recording a channel's streams, checking every 2 minutes
ytarchive --monitor-channel -r 120 -o '%(channel)s/%(upload_date)s %(title)s' https://www.youtube.com/@channel 1080p/best

# Print stream info only: available qualities and codecs, or the scheduled start time
ytarchive --info-only URL
```

On Windows, use double quotes instead of single quotes around templates, e.g. `-o "%(channel)s/%(title)s"`. In cmd (not PowerShell), write `%` as `%%` inside batch files.

### File names

- `--windows-filenames` (default on Windows) makes file and folder names Windows-compatible: characters like `<>:"/\|?*` and control characters are replaced, and names Windows reserves (`CON`, `NUL`, …) or that end in a dot or space are adjusted. This includes folders created from the template, such as `%(channel)s/`. The title is also shortened so every file ytarchive writes (final and temporary, including fragment files) stays within Windows' 260-character path limit (MAX_PATH). Use it on Linux/macOS if the files will end up on Windows or a Windows-formatted drive.
- `--no-windows-filenames` (default on Linux and macOS) only replaces path separators, so titles keep characters like `?` and `:`.
- File names are always at most 255 bytes, and titles are read from yt-dlp as UTF-8 whatever the console's code page is.

### Output template keys

`-o` accepts `%(key)s` placeholders: `id`, `title`, `channel_id`, `channel`, `upload_date`, `start_date`, `publish_date`, `year`, `month`, `day`, `start_time`, `hours`, `minutes`, `seconds`, `url`. `description` is accepted but expands to nothing in file names.

### PO tokens and cookies

yt-dlp handles extraction, so pass PO token settings to it with `--ytdlp-opts`, e.g. `--ytdlp-opts "--extractor-args youtube:po_token=..."`. `--potoken` and `--visitor-data` are still accepted but ignored. Cookies (`-c cookies.txt`, Netscape format) are used both by yt-dlp and by the fragment downloads.

## Differences from the Go version

See [docs/CHANGES_FROM_GO.md](docs/CHANGES_FROM_GO.md) for the full list: behavior changes, new options, Go bugs fixed during the port, and Go code that was not ported because it was unreachable.

## Testing status

See [docs/NOT_TESTED.md](docs/NOT_TESTED.md) for what has been verified so far and what hasn't (for example, real downloads on Windows and macOS).

## Credits

- [Kethsar/ytarchive](https://github.com/Kethsar/ytarchive): the original ytarchive. This port follows its design, flags, and file formats.
- [dreammu/ytarchive](https://github.com/dreammu/ytarchive): the fork this port is based on (yt-dlp extraction, background URL refresh, fragment retry logic, `--ytdlp-info`).
- [CanOfSocks/livestream_dl](https://github.com/CanOfSocks/livestream_dl): the idea behind `--sqlite` storage and resuming by re-fetching only gaps.

## License

MIT; see [LICENSE](LICENSE). Kethsar/ytarchive's copyright notice is retained as its license requires.
