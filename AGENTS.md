# AGENTS.md

Guidelines for working on ytarchive-rs, for people and coding agents alike. These are the rules the project was built with.

## What this project is

A Rust port of [dreammu/ytarchive](https://github.com/dreammu/ytarchive) (a fork of [Kethsar/ytarchive](https://github.com/Kethsar/ytarchive)): a YouTube livestream archiver. URL extraction goes through yt-dlp (run as a subprocess); ytarchive downloads the fragments and muxes with ffmpeg. `--sqlite` storage and `--recover-private` borrow their ideas from [CanOfSocks/livestream_dl](https://github.com/CanOfSocks/livestream_dl).

## Porting rules

- **Behave like dreammu/ytarchive** unless there is a reason not to. Flag names, defaults, file names (`.f{itag}.ts`, `{id}.f{itag}.state`, `{id}__XXXX` temp dirs), state file format, and log messages follow Go.
- **Fix Go bugs instead of copying them**, and record every fix in `docs/CHANGES_FROM_GO.md` (what Go did, what the port does).
- **Record every intentional behavior change** in the same file.
- **Don't port dead code.** Code that is unreachable in Go (InnerTube request, ytcfg, SAPISIDHASH, DASH manifest parsing, UMP) stays out; list it under "Not ported".
- To compare with the originals, clone them into `refs/` (gitignored): `git clone https://github.com/dreammu/ytarchive refs/dreammu-ytarchive` (likewise Kethsar/ytarchive and CanOfSocks/livestream_dl).

## Platforms

- **Windows, Linux, and macOS are supported**, each as x64 and arm64, and nothing else (no code or docs for other platforms); every change must build and pass tests on all three.
- **File names**: anything that becomes a file or folder name goes through `util::sterilize_filename` with a `NameStyle`, and through `util::portable_file_name` when `--windows-filenames` is on (default on Windows: forbidden and control characters, reserved names like `CON`/`NUL`/`COM1`, trailing dots and spaces). `--no-windows-filenames` (default elsewhere) only replaces path separators and NUL. With Windows names, `output::final_location` also shortens the title so every path fits in MAX_PATH (unless `--no-path-limit`); if you add a longer file suffix than `.f{itag}.frag{seq}.ts`, raise `SUFFIX_ROOM`.
- Don't assume Unix in code or tests: no hard-coded `/tmp`, `sh`, or unescaped paths in JSON; gate platform code with `#[cfg(windows)]` / `#[cfg(unix)]` (see `src/platform.rs`, `util::set_mode`).
- The Rust version is pinned in `rust-toolchain.toml` (development, CI, and releases all use it); raise it in its own commit. `rust-version` in `Cargo.toml` is the oldest Rust that builds the crate; CI checks it (`cargo check` with that version), so raise it when a dependency needs a newer Rust.
- TLS is rustls with the `ring` provider and the OS certificate store; don't add OpenSSL.
- The global allocator is mimalloc, and the release profile uses fat LTO with `codegen-units = 1`. Keep panic unwinding (no `panic = "abort"`): the download code catches a panicked worker so the other stream is still finished and muxed, and `ConsoleGuard` restores the Windows console.

## Code layout and style

- One module per file. Shared types live in `src/types/`; define or update them before the feature that uses them.
- A file over 400 lines must be split, but only when you are already editing it; say so in the commit.
- No crate-wide `allow(dead_code)`. Remove unused code, tests, and dependencies. Tests for minor or unimportant features go only in `src/minor_tests.rs` (`#[cfg(test)] mod minor_tests;`), so they can be deleted all at once.
- No `unsafe` unless it's truly necessary; prefer a maintained safe wrapper crate. `main.rs` has `#![forbid(unsafe_code)]`; lifting it needs a good reason.
- Outside tests: no `unwrap()`/`expect()`, no unchecked indexing or slicing, and no `let _ =` on a `Result` (handle it, or log it if it can be ignored). Clippy enforces this (`[lints.clippy]` in `Cargo.toml`; tests are exempt through `clippy.toml`). Use `util::lock` for mutexes and `util::apply_mode` for permissions that may fail.
- Names are full words (`sequence`, not `sq` or `s`), including closure parameters.
- Comments explain *why*, especially where behavior differs from Go.
- Cookies are read twice, on purpose: `cookies.rs` follows Go for ytarchive's own requests, and `ytdlp_cookies.rs` mirrors yt-dlp's loader only to decide whether yt-dlp needs a repaired copy. Keep them separate; each must match its own target.

## Testing

Run these as separate commands before pushing (CI runs clippy and tests on all three platforms; a change isn't done until they pass). Clippy is also the typecheck; there's no separate `cargo check` step:

```sh
cargo fmt
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release --locked   # when dependencies or build config change
```

- Keep tests essential: every option under "Usage and options", downloading and storage, quality/codec selection, file names, ffmpeg arguments, and parsing of URLs, yt-dlp output, watch pages, durations, and cookies. Don't keep tests for logging, the progress line, help layout, version strings, or small helpers.
- Downloader tests use a local fake googlevideo server (`src/download/tests/mod.rs`; SQLite and recovery cases in `tests/sqlite.rs`); add cases there for download behavior. `serve_until` makes fragments return 403 from a given sequence on, as for a stream that went private.
- `--recover-private` waits 5 minutes between recovery rounds; tests use a 20 ms interval (`cfg(test)` in `src/download/recovery.rs`).
- With `--recover-private`, the SQLite database also stores the stream info, every download URL used, and the newest fragment (`urls`/`meta` tables) for retrying in a later run (`src/kept.rs`). Without the flag nothing new is stored, recovered, or kept. Don't rename those tables or keys without handling old databases.
- Network tests are marked `#[ignore]`. Run them against a real 24/7 stream with: `YTDLP_TEST_OPTS="--js-runtimes node" cargo test -- --ignored` (drop the env var if deno is installed).
- For changes to the download/mux flow, also do a short real run, e.g. `--live-from now --capture-duration 10s -o '/tmp/x/%(id)s'`, and check the result with ffprobe.
- On Windows, processes started from a coding agent's shell can inherit "ignore Ctrl-C", so a scripted Ctrl-C never reaches ytarchive and it gets killed instead. Send Ctrl-Break, or reset the flag with `SetConsoleCtrlHandler(NULL, FALSE)` in the launcher, before judging interrupt behavior.
- Prompts where an empty answer means "go ahead" must use `prompt::get_line_or_eof` and treat closed stdin as "no", so non-interactive runs never wait unattended.
- List what hasn't been verified in `docs/NOT_TESTED.md`, and delete a line once it is verified.

## Commits

- Title format (Conventional Commits): `<type>(<scope>): <lowercase imperative summary>`, no trailing period. Types: `feat`, `fix`, `refactor`, `test`, `docs`, `ci`, `chore`. The scope is the module (`download`, `cli`, `tui`, …) and can be left out for broad changes (`docs: …`). Add the PR number at the end, `(#12)`, only when the commit comes from a pull request. Releases are `chore: release 0.2.0`. Examples: `fix(download): close the database before cleanup in test`, `feat(cli): add --recover-private`, `docs: describe --recover-private`.- Body: bullet list of what changed, plus a paragraph on Go bugs fixed or behavior changes.
- One commit per completed step. Each commit also updates the docs it affects (`README.md`, `docs/CHANGES_FROM_GO.md`, `docs/NOT_TESTED.md`, this file) in the same commit.
- Exception: when the user asks for a large docs update, attach each docs change to the commit it belongs to; only changes with no relevant commit go into a separate `docs:` commit.
- Pause after each commit for review before starting the next step, and push once the work is reviewed.
- Each commit should build on its own.
- Never force-push or rewrite pushed history. Unpushed commits may be reorganized.

## Releases

- **Releases** are manual: push a `v*` tag (e.g. `v0.1.0`). `release.yml` builds all platforms and publishes a normal release, with a `SHA256SUMS` file and a build provenance attestation for the zips.
- **Nightly** (`nightly.yml`) runs weekly, Fridays at 00:00 UTC (or by hand). It skips when `main` hasn't changed since the last nightly or every new commit is a docs commit (`docs:` or `docs(...):`), otherwise replaces the `nightly` pre-release, listing commits since the last `v*` release as links to their commit pages.
- Both use `build.yml`, which publishes one zip per platform, `ytarchive_<windows|linux|macos>_<x64|arm64>.zip`, holding only `ytarchive`/`ytarchive.exe` (Linux builds are static musl). Releases build x64 and arm64 for every OS; nightly (`all_targets: false`) builds only Windows x64, Linux x64, and macOS arm64. It sets `YTARCHIVE_CHANNEL`/`YTARCHIVE_VERSION`/`YTARCHIVE_COMMIT`/`YTARCHIVE_DATE` for `build.rs`'s version string; `Cross.toml` passes them into `cross` containers.
- **CI** runs only for pushes that touch the code or what builds and checks it: `src/`, `build.rs`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, or `ci.yml` itself. Besides the three-platform tests, it checks that the crate builds with the `rust-version` in `Cargo.toml`.
- **Dependabot** (`.github/dependabot.yml`) opens one grouped pull request a week for Cargo dependencies and one for GitHub Actions.
- **Audit** (`audit.yml`) runs `cargo audit` every Monday and whenever `Cargo.toml` or `Cargo.lock` changes. It fails on a vulnerable dependency; "unmaintained" notices are only warnings. `cargo-audit` is installed as a prebuilt binary.
