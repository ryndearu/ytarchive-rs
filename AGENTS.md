# AGENTS.md

Guidelines for working on ytarchive-rs, for people and coding agents alike. These are the rules the project was built with.

## What this project is

A Rust port of [dreammu/ytarchive](https://github.com/dreammu/ytarchive) (a fork of [Kethsar/ytarchive](https://github.com/Kethsar/ytarchive)): a YouTube livestream archiver. URL extraction goes through yt-dlp (run as a subprocess); ytarchive downloads the fragments and muxes with ffmpeg. `--sqlite` storage borrows its idea from [CanOfSocks/livestream_dl](https://github.com/CanOfSocks/livestream_dl).

## Porting rules

- **Behave like dreammu/ytarchive** unless there is a reason not to. Flag names, defaults, file names (`.f{itag}.ts`, `{id}.f{itag}.state`, `{id}__XXXX` temp dirs), state file format, and log messages follow Go.
- **Fix Go bugs instead of copying them**, and record every fix in `docs/CHANGES_FROM_GO.md` (what Go did, what the port does).
- **Record every intentional behavior change** in the same file.
- **Don't port dead code.** Code that is unreachable in Go (InnerTube request, ytcfg, SAPISIDHASH, DASH manifest parsing, UMP) stays out; list it under "Not ported".
- To compare with the originals, clone them into `refs/` (gitignored): `git clone https://github.com/dreammu/ytarchive refs/dreammu-ytarchive` (likewise Kethsar/ytarchive and CanOfSocks/livestream_dl).

## Platforms

- **Windows, Linux, and macOS are supported**, each as x64 and arm64; every change must build and pass tests on all three.
- **File names**: anything that becomes a file or folder name goes through `util::sterilize_filename` with a `NameStyle`, and through `util::portable_file_name` when `--windows-filenames` is on (default on Windows: forbidden and control characters, reserved names like `CON`/`NUL`/`COM1`, trailing dots and spaces). `--no-windows-filenames` (default elsewhere) only replaces path separators and NUL. With Windows names, `output::final_location` also shortens the title so every path fits in MAX_PATH; if you add a longer file suffix than `.f{itag}.frag{seq}.ts`, raise `SUFFIX_ROOM`.
- Don't assume Unix in code or tests: no hard-coded `/tmp`, `sh`, or unescaped paths in JSON; gate platform code with `#[cfg(windows)]` / `#[cfg(unix)]` (see `src/platform.rs`, `util::set_mode`).
- TLS is rustls with the `ring` provider and the OS certificate store; don't add OpenSSL.
- Only the platforms above are supported; don't add code or docs for others.

## Code layout and style

- One module per file. Shared types live in `src/types/`; define or update them before the feature that uses them.
- Keep files under ~300 lines; split a file only when you are already editing it, and say so in the commit.
- No crate-wide `allow(dead_code)`. Remove unused code and dependencies.
- No `unsafe` unless it's truly necessary; prefer a maintained safe wrapper crate. `main.rs` has `#![forbid(unsafe_code)]`; lifting it needs a good reason.
- Comments explain *why*, especially where behavior differs from Go.

## Testing

Run these as separate commands before committing:

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release --locked   # when dependencies or build config change
```

- Downloader tests use a local fake googlevideo server (`src/download/tests.rs`); add cases there for download behavior.
- Network tests are `#[ignore]`d. Run them against a real 24/7 stream with: `YTDLP_TEST_OPTS="--js-runtimes node" cargo test -- --ignored` (drop the env var if deno is installed).
- For changes to the download/mux flow, also do a short real run, e.g. `--live-from now --capture-duration 20s -o '/tmp/x/%(id)s'`, and check the result with ffprobe.
- CI runs clippy and tests on Windows, Linux, and macOS; a change isn't done until all three pass.
- Record what has and hasn't been verified in `docs/NOT_TESTED.md`.

## Commits

- Title format: `<scope>[/<subscope>]: <lowercase imperative summary>`, no trailing period, scope = module path. Examples: `download/db_sink: close the database before cleanup in test`, `output: make template folder names windows-safe`. Broad changes may use a plain sentence-case title.
- Body: bullet list of what changed, plus a paragraph on Go bugs fixed or behavior changes.
- One commit per completed step. When a change touches code and docs, commit the code first and the docs last, as separate commits.
- If a change is minor, put the code and its docs update in a single commit instead.
- After each commit, update the docs it affects (`README.md`, `docs/CHANGES_FROM_GO.md`, `docs/NOT_TESTED.md`, this file).
- When reorganizing commits, consolidate all docs commits into a single `docs:` commit at the end.
- Pause after each commit for review before starting the next step, and push once the work is reviewed.
- Each commit should build on its own.
- Never force-push or rewrite pushed history. Unpushed commits may be reorganized.

## Releases

- **Releases** are manual: push a `v*` tag (e.g. `v0.1.0`). `release.yml` builds all platforms and publishes a normal release.
- **Nightly** (`nightly.yml`) runs daily at 00:00 UTC (or by hand). It skips when `main` hasn't changed since the last nightly or every new commit is a `docs:` commit, otherwise replaces the `nightly` pre-release, listing commits since the last `v*` release as links to their commit pages (GitHub doesn't link bare hashes in release notes). Docs commits must use the `docs:` scope for this to work.
- Both use `build.yml`, which publishes one zip per platform, `ytarchive_<windows|linux|macos>_<x64|arm64>.zip`, holding only `ytarchive`/`ytarchive.exe` (Linux builds are static musl). Releases build x64 and arm64 for every OS; nightly (`all_targets: false`) builds only Windows x64, Linux x64, and macOS arm64. It sets `YTARCHIVE_CHANNEL`/`YTARCHIVE_VERSION`/`YTARCHIVE_COMMIT`/`YTARCHIVE_DATE` for `build.rs`'s version string; `Cross.toml` passes them into `cross` containers.
- Workflows force JavaScript actions onto the newest Node GitHub's runners provide (`FORCE_JAVASCRIPT_ACTIONS_TO_NODE24`); bump it when GitHub adds a newer one.
- **CI** doesn't run for pushes that only touch `*.md`, `docs/`, or `LICENSE`.
