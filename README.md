# ytarchive-rs

Archive a YouTube livestream from the start, or wait for a scheduled stream and record it as soon as it begins. A Rust port of [dreammu/ytarchive](https://github.com/dreammu/ytarchive), itself a fork of [Kethsar/ytarchive](https://github.com/Kethsar/ytarchive).

For finished videos (VODs), use [yt-dlp](https://github.com/yt-dlp/yt-dlp) instead.

## What's different from Go ytarchive

- `--sqlite` stores fragments in a database, so a resumed download fetches only the fragments that are missing.
- `--recover-private` keeps retrying missing fragments after a stream goes private mid-recording, in the same run or a later one (see [Private streams and missing fragments](#private-streams-and-missing-fragments)).
- The channel monitor doesn't record the same stream twice, and keeps checking `/live` so it doesn't miss a stream that starts before the scheduled one.

The full list is in [docs/CHANGES_FROM_GO.md](docs/CHANGES_FROM_GO.md); what hasn't been tested yet is in [docs/NOT_TESTED.md](docs/NOT_TESTED.md).

## Features

- Downloads live streams from the beginning (up to YouTube's 7-day seek window), or from a chosen point with `--live-from`.
- Downloads a stream that just ended as long as its live download still exists.
- Waits for scheduled streams: until the scheduled time, or by polling.
- Watches a channel and records each new stream (`--monitor-channel`), including members-only streams with cookies.
- Parallel fragment downloads, URL refresh when fragments start failing, and a background refresh every 5 hours before URLs expire.
- Resumable downloads: interrupt with Ctrl-C, keep the state, rerun to continue.
- Muxes to mp4/mkv/m4a with ffmpeg, with optional thumbnail, metadata, and description files.
- Builds are for Windows, Linux (static), and macOS, each as x64 and arm64.

## Requirements

- [ffmpeg](https://ffmpeg.org/) to mux the final file.
- [yt-dlp](https://github.com/yt-dlp/yt-dlp), kept up to date. It extracts the download URLs. The **nightly** channel is recommended, since YouTube changes often and fixes land there first: `yt-dlp --update-to nightly`, `pip install -U --pre "yt-dlp[default]"`, or the binary from [yt-dlp-nightly-builds](https://github.com/yt-dlp/yt-dlp-nightly-builds/releases) (distro packages and Homebrew only have stable).
- A JavaScript runtime for yt-dlp's YouTube support: [deno](https://deno.com/) by default. If you only have node, add `--ytdlp-opts "--js-runtimes node"`.

ytarchive looks for `ffmpeg` and `yt-dlp` next to the `ytarchive` executable first, and then on your `PATH` (on Windows, put `ffmpeg.exe` and `yt-dlp.exe` in the same folder as `ytarchive.exe`).

## Installing

Download the zip for your platform (Windows, Linux, or macOS; x64 or arm64) from the [releases page](../../releases), extract it, and run `ytarchive` (`ytarchive.exe` on Windows) from a terminal. Each zip holds just the executable:

| Platform | x64 | arm64 |
| --- | --- | --- |
| Windows | [`ytarchive_windows_x64.zip`](https://github.com/ryndearu/ytarchive-rs/releases/latest/download/ytarchive_windows_x64.zip) | [`ytarchive_windows_arm64.zip`](https://github.com/ryndearu/ytarchive-rs/releases/latest/download/ytarchive_windows_arm64.zip) |
| Linux | [`ytarchive_linux_x64.zip`](https://github.com/ryndearu/ytarchive-rs/releases/latest/download/ytarchive_linux_x64.zip) | [`ytarchive_linux_arm64.zip`](https://github.com/ryndearu/ytarchive-rs/releases/latest/download/ytarchive_linux_arm64.zip) |
| macOS | [`ytarchive_macos_x64.zip`](https://github.com/ryndearu/ytarchive-rs/releases/latest/download/ytarchive_macos_x64.zip) (Intel) | [`ytarchive_macos_arm64.zip`](https://github.com/ryndearu/ytarchive-rs/releases/latest/download/ytarchive_macos_arm64.zip) (Apple Silicon) |

Nightly builds, with the newest changes: [Windows x64](https://github.com/ryndearu/ytarchive-rs/releases/download/nightly/ytarchive_windows_x64.zip), [Linux x64](https://github.com/ryndearu/ytarchive-rs/releases/download/nightly/ytarchive_linux_x64.zip).

### Linux

```sh
# Arch:          sudo pacman -S ffmpeg deno
# Debian/Ubuntu: sudo apt install ffmpeg, then install deno from its website
unzip ytarchive_linux_x64.zip   # arm64: ytarchive_linux_arm64.zip
./ytarchive --help
```

### Windows

```powershell
winget install Gyan.FFmpeg yt-dlp.yt-dlp DenoLand.Deno
yt-dlp --update-to nightly
# then extract ytarchive_windows_x64.zip (arm64: ytarchive_windows_arm64.zip) and, from PowerShell or cmd:
.\ytarchive.exe --help
```

While running on Windows, ytarchive keeps the PC from going to sleep and turns off the console's QuickEdit mode, so a stray click in the window can't pause the download. The console's previous setting is restored when ytarchive exits.

### macOS

```sh
brew install ffmpeg deno
unzip ytarchive_macos_arm64.zip   # Intel: ytarchive_macos_x64.zip
xattr -d com.apple.quarantine ytarchive   # unsigned binary
./ytarchive --help
```

### Building from source

Needs Rust 1.89 or newer (the oldest that builds it; rustup installs the version the project is developed with) and a C compiler (for SQLite and the TLS library; on Windows, the Visual Studio Build Tools that rustup asks for):

```sh
cargo build --release
# binary: target/release/ytarchive (ytarchive.exe on Windows)
```

## Usage and options

    ytarchive [OPTIONS] [URL] [QUALITY]

`URL` can be a watch URL (`youtube.com/watch?v=`, `youtu.be/`, `/live/ID`), a channel URL (`/@name`, `/channel/ID`, `/c/name`; its `/live` page is used), or a direct googlevideo fragment URL. `QUALITY` is a slash-delimited preference list such as `1080p60/720p/best`. Either one is prompted for if omitted.

Flag names match Go ytarchive, and Go-style single-dash long flags (`-threads 4`, `-td DIR`, `-dp`, `-fp`) are still accepted. `ytarchive --help` prints the same list.

### General Options:
    -h, --help             Print this help text and exit
    -V, --version          Print the version and exit
        --monitor-channel  Continually monitor a channel URL for new streams (requires URL and QUALITY)
        --members-only     Only download members-only streams (channel URLs; requires cookies)

### Waiting Options:
    -w, --wait                    Wait for a scheduled stream to start [default: ask]
    -n, --no-wait                 Do not wait for a scheduled stream to start
    -r, --retry-stream <SECS>     Seconds between stream status checks while waiting (min 15; with --monitor-channel, under 30 means 60) [default: the scheduled start time; 60 with --monitor-channel]
        --start-delay <DURATION>  Wait this long after the stream starts before capturing [default: none]

### Download Options:
        --live-from <WHEN>             Start from a point in the stream: duration (-30m), time string, or 'now' [default: the start of the stream]
        --capture-duration <DURATION>  Capture this much of the stream, then finish [default: until the stream ends]
        --threads <N>                  Download threads per stream type [default: 1]
        --retry-frags <ATTEMPTS>       Attempts per fragment before refreshing URLs / giving up (0 = unlimited) [default: 10]
        --no-frag-files                Keep out-of-order fragments in memory instead of temp files
        --sqlite                       Store fragments in a SQLite database (more robust to gaps)
        --recover-private              If the stream goes private, retry missing fragments with the URLs already in use (implies --sqlite)

### Format Selection Options:
        --codec <CODECS>  Video codec preference, e.g. vp9/h264 [default: best = av1/vp9/h264]
        --av1             Prefer AV1 video when available (same as --codec av1)
        --vp9             Prefer VP9 video when available (same as --codec vp9)
        --h264            Prefer H.264 video (same as --codec h264)
        --no-video        Only download the audio stream
        --no-audio        Only download the video stream

### Filesystem Options:
    -o, --output <FORMAT>               Output filename template, e.g. '%(channel)s/%(upload_date)s %(title)s' [default: %(title)s-%(id)s]
        --temporary-dir <DIR>           Directory for temporary files (created if missing; also -td) [default: the output file's folder; .state files in the current folder]
        --windows-filenames             Force filenames to be Windows-compatible (default on Windows)
        --no-windows-filenames          Sanitize filenames only minimally (default on other than Windows)
        --no-path-limit                 Don't shorten titles to fit Windows' 260-character path limit (ffmpeg may need long paths enabled in Windows)
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
        --metadata <KEY=VALUE>  Override metadata written by --add-metadata, KEY=VALUE (VALUE is a format template; empty omits the key)
        --mkv                   Mux into mkv instead of mp4 (ignored for audio only) [default: mp4]
        --separate-audio        Also save the audio as a separate file
        --write-mux-file        Write the ffmpeg mux command to a file instead of muxing
        --ffmpeg-path <PATH>    ffmpeg executable to use [default: ffmpeg]

### Interrupt (Ctrl-C) Options:
        --merge          Automatically merge files when the download is cancelled [default: ask]
        --no-merge       Skip merging files when the download is cancelled
        --save           Keep downloaded files on cancel if not merging [default: ask]
        --no-save        Delete downloaded files on cancel if not merging
        --save-state     Leave files for resuming later on cancel, if not saving files [default: ask]
        --no-save-state  Do not leave files for resuming later on cancel

### Extraction Options:
    -c, --cookies <FILE>        Netscape-format cookies file (needed for members-only streams)
        --ytdlp-path <PATH>     yt-dlp executable used for URL extraction [default: yt-dlp]
        --ytdlp-opts <OPTIONS>  Extra arguments for yt-dlp, e.g. '--extractor-args youtube:player_client=web'
        --ytdlp-info            Let yt-dlp extract stream info; ytarchive only downloads fragments
        --video-url <URL>       Googlevideo URL to use for the video stream
        --audio-url <URL>       Googlevideo URL to use for the audio stream (itag 140)

### Network Options:
    -4, --ipv4         Force IPv4
    -6, --ipv6         Force IPv6
        --proxy <URL>  Proxy URL (http, https, socks5, or socks5h)

### Verbosity Options:
    -q, --quiet    Print nothing except prompts
        --error    Print only errors
        --warn     Print warnings and errors (default)
    -v, --verbose  Verbose output
        --debug    Debug output
        --trace    Trace output
        --newline  Print progress on new lines instead of updating one line

Durations accept units (`1d8h10m`, `90s`) or time strings (`01:30:00`). `--live-from` also accepts negative durations (`-30m`) and `now`.

Without `--proxy`, the proxy environment variables (`HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY`) are used. The proxy configured in the Windows or macOS system settings is not.

With `--video-url`/`--audio-url` ytarchive gets no stream info, so it assumes 5-second fragments (like Go). On streams with shorter fragments, `--capture-duration` then records less than asked: `10s` gives 4 seconds with 2-second fragments.

### Quality and codec

`QUALITY` is a `/`-separated list, most wanted first: `audio_only`, `144p`, `240p`, `360p`, `480p`, `720p`, `720p60`, `1080p`, `1080p60`, `1440p`, `1440p60`, `2160p`, `2160p60`, `best`. The first one the stream offers wins. If it offers none, `best` is used without asking (e.g. when the quality was picked while waiting for a scheduled stream).

`--codec` is a `/`-separated list of `av1`, `vp9`, `h264`, or `best` (= `av1/vp9/h264`). Codecs are tried in the order listed, so `vp9/best` tries VP9 first, then AV1 and H.264. Unlisted codecs are a fallback when none of the listed ones are offered. Without the option, `best` is used and the codec is never asked for.

`best` quality and a codec combine: the highest quality is picked first, then the codec at that quality, falling back to other codecs there.

### Examples

```sh
# Record a stream from the beginning in the best quality (and best codec)
ytarchive https://www.youtube.com/watch?v=VIDEO_ID best

# Wait for a scheduled stream, 4 threads, embed thumbnail, add metadata
ytarchive -w --threads 4 -t --add-metadata URL 1080p60/best

# 1080p in VP9, falling back to H.264
ytarchive --codec vp9/h264 URL 1080p

# Best quality, but in H.264 (plays everywhere)
ytarchive --codec h264 URL best

# Only the last 30 minutes, then 10 more minutes of the stream
ytarchive --live-from -30m --capture-duration 40m URL best

# Keep recording a channel's streams, checking every 2 minutes
ytarchive --monitor-channel -r 120 -o '%(channel)s/%(upload_date)s %(title)s' https://www.youtube.com/@channel 1080p/best
```

On Windows, use double quotes instead of single quotes around templates, e.g. `-o "%(channel)s/%(title)s"`. In cmd (not PowerShell), write `%` as `%%` inside batch files.

### File names

- `--windows-filenames` (default on Windows) replaces characters Windows forbids (`<>:"/\|?*`, control characters), renames reserved names (`CON`, `NUL`, …) in folders too, and shortens titles so every file stays within the 260-character path limit. Use it on Linux/macOS for files headed to Windows.
- `--no-path-limit` keeps full titles. ffmpeg and other tools may then need Windows' `LongPathsEnabled` setting.
- `--no-windows-filenames` (default on Linux and macOS) only replaces path separators. On Windows, a title with `?` or `:` can't be saved, and ytarchive stops without merging.
- Resume state (`{id}.f{itag}.state`) goes to `--temporary-dir`, or the current folder. Resume from the same folder; a different `-o` still finds it by video ID.
- Data that can't be continued is kept as `{name}.f{itag}.ts.old` (or `.sqlite.old` with `--sqlite`) in the temporary folder. Delete it yourself when done.
- If data can't be stored (full disk, failed `--sqlite` export), ytarchive exits 1 without merging and keeps everything. Run the same command again to continue.
- If `{name}.mp4` already exists, the new file is `{name}-1`, up to `{name}-10`. Past that, the `.ts` files stay unmuxed.

### Output template keys

`-o` accepts `%(key)s` placeholders: `id`, `title`, `channel_id`, `channel`, `upload_date`, `start_date`, `publish_date`, `year`, `month`, `day`, `start_time`, `hours`, `minutes`, `seconds`, `url`. `description` is accepted but expands to nothing in file names.

### Private streams and missing fragments

By default, a stream that goes private mid-recording ends the download. With `--recover-private`, ytarchive keeps trying:

- The URLs it already has usually keep working until they expire, about 6 hours after issue. Every missing fragment is retried with every URL used, in rounds 5 minutes apart, until all are recovered, the URLs expire, or an hour passes with nothing recovered. Ctrl-C stops it as usual. This is best effort: YouTube often refuses the URLs right away once a stream is private.
- It turns on `--sqlite`, since recovered fragments arrive out of order. Fragments go to `{name}.f{itag}.sqlite` in the temporary folder and export in order at the end, so the download needs about as much extra space as the stream while it runs.
- Fragments still missing at the end: the file is merged anyway and the missing sequence numbers are listed. If the URLs haven't expired, the temporary folder (the database also holds the stream info and URLs) and `.state` files are kept, and ytarchive says until when they're usable.
- To retry, rerun the command with `--recover-private` and the video's URL (not the channel's). It needs the state files, so the two can't be combined with `--disable-save-state`. If the stream can still be looked up, the download resumes with fresh URLs; otherwise it uses the info and URLs saved in the database. The new file gets a `-1` suffix. Delete the temporary folder to stop retrying.
- A download always resumes in the storage it started with. Adding `--recover-private` when resuming a non-`--sqlite` download keeps its `.ts` data, but that download continues without recovery.

### PO tokens and cookies

yt-dlp does the extraction, so pass PO tokens to it with `--ytdlp-opts`, e.g. `--ytdlp-opts "--extractor-args youtube:po_token=..."`. Cookies (`-c cookies.txt`, Netscape format) are used by yt-dlp and by ytarchive's own requests to the watch, `/live`, and `/streams` pages.

yt-dlp gets the cookies file itself, so it can save refreshed cookies back. If it can't read the file, ytarchive warns and passes a repaired copy instead, and cookies refreshed in that run aren't saved back. JSON cookie exports aren't supported.

## Credits

- [Kethsar/ytarchive](https://github.com/Kethsar/ytarchive): the original ytarchive. This port follows its design, flags, and file formats.
- [dreammu/ytarchive](https://github.com/dreammu/ytarchive): the fork this port is based on (yt-dlp extraction, background URL refresh, fragment retry logic, `--ytdlp-info`).
- [CanOfSocks/livestream_dl](https://github.com/CanOfSocks/livestream_dl): the ideas behind `--sqlite` storage, resuming by re-fetching only gaps, and `--recover-private`.