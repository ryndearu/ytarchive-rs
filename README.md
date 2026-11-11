# ytarchive-rs

Archive a YouTube livestream from the start, or wait for a scheduled stream and record it as soon as it begins. A Rust port of [dreammu/ytarchive](https://github.com/dreammu/ytarchive), itself a fork of [Kethsar/ytarchive](https://github.com/Kethsar/ytarchive).

For finished videos (VODs), use [yt-dlp](https://github.com/yt-dlp/yt-dlp) instead.

## Features

- Downloads live streams from the beginning (up to YouTube's 7-day seek window), or from a chosen point with `--live-from`.
- Downloads a stream that just ended as long as its live download still exists.
- Waits for scheduled streams: until the scheduled time, or by polling.
- Watches a channel and records each new stream (`--monitor-channel`), including members-only streams with cookies.
- Parallel fragment downloads, URL refresh when fragments start failing, and a background refresh every 5 hours before URLs expire.
- Resumable downloads: interrupt with Ctrl-C, keep the state, rerun to continue.
- Optional SQLite fragment storage (`--sqlite`) that resumes by re-fetching only missing fragments.
- Optional recovery after a stream goes private mid-recording (`--recover-private`; see [Private streams and missing fragments](#private-streams-and-missing-fragments)).
- Muxes to mp4/mkv/m4a with ffmpeg, with optional thumbnail, metadata, and description files.

## Requirements

- [ffmpeg](https://ffmpeg.org/) to mux the final file.
- [yt-dlp](https://github.com/yt-dlp/yt-dlp), kept up to date. It extracts the download URLs. The **nightly** channel is recommended, since YouTube changes often and fixes land there first: `yt-dlp --update-to nightly` (or `pip install -U --pre "yt-dlp[default]"` for pip installs).
- A JavaScript runtime for yt-dlp's YouTube support: [deno](https://deno.com/) by default. If you only have node, add `--ytdlp-opts "--js-runtimes node"`.

ytarchive looks for `ffmpeg` and `yt-dlp` on your `PATH`. On Windows you can also put `ffmpeg.exe` and `yt-dlp.exe` in the same folder as `ytarchive.exe`.

## Installing

Download the zip for your platform (Windows, Linux, or macOS; x64 or arm64) from the [releases page](../../releases), extract it, and run `ytarchive` (`ytarchive.exe` on Windows) from a terminal. Each zip holds just the executable:

| Platform | x64 | arm64 |
| --- | --- | --- |
| Windows | [`ytarchive_windows_x64.zip`](https://github.com/ryndearu/ytarchive-rs/releases/latest/download/ytarchive_windows_x64.zip) | [`ytarchive_windows_arm64.zip`](https://github.com/ryndearu/ytarchive-rs/releases/latest/download/ytarchive_windows_arm64.zip) |
| Linux | [`ytarchive_linux_x64.zip`](https://github.com/ryndearu/ytarchive-rs/releases/latest/download/ytarchive_linux_x64.zip) | [`ytarchive_linux_arm64.zip`](https://github.com/ryndearu/ytarchive-rs/releases/latest/download/ytarchive_linux_arm64.zip) |
| macOS | [`ytarchive_macos_x64.zip`](https://github.com/ryndearu/ytarchive-rs/releases/latest/download/ytarchive_macos_x64.zip) (Intel) | [`ytarchive_macos_arm64.zip`](https://github.com/ryndearu/ytarchive-rs/releases/latest/download/ytarchive_macos_arm64.zip) (Apple Silicon) |

Nightly builds, with the newest changes: [Windows x64](https://github.com/ryndearu/ytarchive-rs/releases/download/nightly/ytarchive_windows_x64.zip), [Linux x64](https://github.com/ryndearu/ytarchive-rs/releases/download/nightly/ytarchive_linux_x64.zip), [macOS arm64](https://github.com/ryndearu/ytarchive-rs/releases/download/nightly/ytarchive_macos_arm64.zip).

### Linux

```sh
# Arch:          sudo pacman -S ffmpeg deno
# Debian/Ubuntu: sudo apt install ffmpeg, then install deno from its website
# yt-dlp nightly (distro packages are stable-only and often old):
#   download the yt-dlp binary from https://github.com/yt-dlp/yt-dlp-nightly-builds/releases, or
#   pip install -U --pre "yt-dlp[default]"
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
# yt-dlp nightly: download yt-dlp_macos from https://github.com/yt-dlp/yt-dlp-nightly-builds/releases (Homebrew only has stable)
unzip ytarchive_macos_arm64.zip   # Intel: ytarchive_macos_x64.zip
xattr -d com.apple.quarantine ytarchive   # unsigned binary
./ytarchive --help
```

### Building from source

Needs Rust 1.89 or newer (rustup installs the version the project is developed with) and a C compiler (for SQLite and the TLS library; on Windows, the Visual Studio Build Tools that rustup asks for):

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
        --metadata <KEY=VALUE>  Override metadata, KEY=VALUE (VALUE is a format template; empty omits the key)
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

Without `--proxy`, the proxy environment variables (`HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY`) are used. The proxy configured in the Windows or macOS system settings is not.

With `--video-url`/`--audio-url` ytarchive gets no stream info, so it assumes 5-second fragments (like Go). On streams with shorter fragments, `--capture-duration` then records less than asked: `10s` gives 4 seconds with 2-second fragments.

### Quality and codec

`QUALITY` values, from most to least wanted, separated by `/`: `audio_only`, `144p`, `240p`, `360p`, `480p`, `720p`, `720p60`, `1080p`, `1080p60`, `1440p`, `1440p60`, `2160p`, `2160p60`, `best`. The first one the stream offers is used; if it offers none of them, `best` is used without asking (for example when the quality was chosen while waiting for a scheduled stream).

`--codec` takes a slash-delimited list too: `av1`, `vp9`, `h264`, or `best` (= `av1/vp9/h264`). Codecs you don't list are used as a fallback when none of the listed ones are offered. Without a codec option, `best` is used; the codec is never asked for.

`best` quality and a codec option combine: the highest quality the stream offers is picked first, then the codec at that quality, falling back to the other codecs if it isn't offered there.

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

- `--windows-filenames` (default on Windows) makes file and folder names Windows-compatible: characters like `<>:"/\|?*` and control characters are replaced, and names Windows reserves (`CON`, `NUL`, …) or that end in a dot or space are adjusted. This includes folders created from the template, such as `%(channel)s/`. The title is also shortened so every file ytarchive writes (final and temporary, including fragment files) stays within Windows' 260-character path limit (MAX_PATH). Use it on Linux/macOS if the files will end up on Windows or a Windows-formatted drive.
- `--no-path-limit` keeps full titles instead. ytarchive itself can write longer paths, but ffmpeg and other programs may not open them unless long paths are enabled in Windows (the `LongPathsEnabled` setting).
- `--no-windows-filenames` (default on Linux and macOS) only replaces path separators, so titles keep characters like `?` and `:`. On Windows a title with such a character can't be saved; ytarchive then stops without merging and cleans up.
- Resume state (`{id}.f{itag}.state`) is written to `--temporary-dir`, or the current folder without it, not next to the `-o` file. Resume from the same folder; a different `-o` still finds the state by video ID.
- If a stream's earlier data can't be continued (its state file is damaged while the other stream's is fine, or the data file is shorter than the state says), that stream starts over and the earlier data is kept as `{name}.f{itag}.ts.old`. The temporary folder holding it is not deleted after the merge; delete it yourself once you don't need the file.
- File names are always at most 255 bytes.

### Output template keys

`-o` accepts `%(key)s` placeholders: `id`, `title`, `channel_id`, `channel`, `upload_date`, `start_date`, `publish_date`, `year`, `month`, `day`, `start_time`, `hours`, `minutes`, `seconds`, `url`. `description` is accepted but expands to nothing in file names.

### Private streams and missing fragments

By default a stream that goes private during the recording ends the download. With `--recover-private`, ytarchive tries to get the rest:

- When a stream goes private (or otherwise unavailable), the download URLs ytarchive already has usually keep working for a while: until their `expire` time, about 6 hours after they were issued. ytarchive retries every missing fragment with every URL it has used, in rounds 5 minutes apart, until all are recovered, the URLs expire, or an hour passes without recovering anything. Ctrl-C stops it as usual. It's a best effort: YouTube often refuses the URLs right away once a stream is private.
- `--recover-private` turns on `--sqlite`, since recovered fragments arrive out of order. Fragments go into a database (`{name}.f{itag}.sqlite` in the temporary folder) and are exported in order to the `.ts` file at the end, so the download needs about as much extra space as the stream itself while it runs.
- If fragments are still missing at the end, the file is merged anyway and the missing sequence numbers are listed. When the URLs haven't expired yet, the temporary folder (with the database, which also holds the stream info and URLs) and the `.state` files are kept, and ytarchive says until when they can be used.
- To retry only the missing fragments, run the same command again (with `--recover-private` and the video's URL, not the channel's; it needs the state files, so not with `--disable-save-state`). If the stream can still be looked up, the download resumes with fresh URLs; if not, ytarchive uses the stream info and URLs saved in the database. The new file gets a `-1` suffix. Delete the temporary folder if you don't want to retry.
- A download is always resumed in the storage it was started with. Adding `--recover-private` when resuming a download started without `--sqlite` keeps its `.ts` data, but that download continues without recovery.

### PO tokens and cookies

yt-dlp handles extraction, so pass PO token settings to it with `--ytdlp-opts`, e.g. `--ytdlp-opts "--extractor-args youtube:po_token=..."`. Cookies (`-c cookies.txt`, Netscape format) are used both by yt-dlp and by ytarchive's own requests to YouTube pages (watch, `/live`, and `/streams` pages).

yt-dlp gets the cookies file itself, so it can save refreshed cookies back to it. If yt-dlp couldn't read the file (it isn't valid UTF-8, the `# Netscape HTTP Cookie File` header is missing, a cookie's second column doesn't match whether its domain starts with `.`, or a stray line starts like JSON), ytarchive warns and passes yt-dlp a repaired copy in the temp folder instead, deleted when ytarchive exits. Cookies yt-dlp refreshes during that run aren't saved back to your file. A JSON cookie export isn't repaired; yt-dlp reports that it must be Netscape formatted.

## Differences from Go and testing status

- [docs/CHANGES_FROM_GO.md](docs/CHANGES_FROM_GO.md): behavior changes, new options, Go bugs fixed during the port, and Go code that was not ported.
- [docs/NOT_TESTED.md](docs/NOT_TESTED.md): what has not been tested yet.

## Credits

- [Kethsar/ytarchive](https://github.com/Kethsar/ytarchive): the original ytarchive. This port follows its design, flags, and file formats.
- [dreammu/ytarchive](https://github.com/dreammu/ytarchive): the fork this port is based on (yt-dlp extraction, background URL refresh, fragment retry logic, `--ytdlp-info`).
- [CanOfSocks/livestream_dl](https://github.com/CanOfSocks/livestream_dl): the ideas behind `--sqlite` storage, resuming by re-fetching only gaps, and `--recover-private`.