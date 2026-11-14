# AGENTS.md

A Rust port of [dreammu/ytarchive](https://github.com/dreammu/ytarchive) (a fork of [Kethsar/ytarchive](https://github.com/Kethsar/ytarchive)), a YouTube livestream archiver: yt-dlp extracts the URLs, ytarchive downloads the fragments and muxes with ffmpeg. `--sqlite` and `--recover-private` come from [CanOfSocks/livestream_dl](https://github.com/CanOfSocks/livestream_dl).

## Porting rules

- **Behave like dreammu/ytarchive** unless there is a reason not to: flag names, defaults, file names (`.f{itag}.ts`, `{id}.f{itag}.state`, `{id}__XXXX` temp dirs), state file format, and log messages.
- **Fix Go bugs instead of copying them.** Record every fix and intentional change in `docs/CHANGES_FROM_GO.md` (what Go did, what the port does).
- **Don't port dead code** (InnerTube request, ytcfg, SAPISIDHASH, DASH manifest parsing, UMP); list it under "Not ported".
- To compare with the originals, clone them into `refs/` (gitignored), e.g. `git clone https://github.com/dreammu/ytarchive refs/dreammu-ytarchive`.

## Platforms

- **Windows, Linux, and macOS**, each as x64 and arm64, and nothing else. Every change must build and pass tests on all three.
- **File names**: anything that becomes a file or folder name goes through `util::sterilize_filename` with a `NameStyle`, and through `util::portable_file_name` when `--windows-filenames` is on. A file suffix longer than `.f{itag}.frag{seq}.ts` needs a larger `SUFFIX_ROOM` (`output::final_location` keeps paths within MAX_PATH).
- Don't assume Unix in code or tests: no hard-coded `/tmp`, `sh`, or unescaped paths in JSON. Gate platform code with `#[cfg(windows)]` / `#[cfg(unix)]` (see `src/platform.rs`, `util::set_mode`).
- The Rust version is pinned in `rust-toolchain.toml`; raise it in its own commit. Raise `rust-version` in `Cargo.toml` (checked by CI) when a dependency needs a newer Rust.
- TLS is rustls with the `ring` provider and the OS certificate store; don't add OpenSSL.
- Don't add a custom allocator without measuring (mimalloc used two to four times the memory).
- Keep panic unwinding (no `panic = "abort"`): the download code catches a panicked worker, and `ConsoleGuard` restores the Windows console.

## Code layout and style

- One module per file. Shared types live in `src/types/`; update them before the feature that uses them.
- Split a file over 400 lines, but only when already editing it, and say so in the commit. Don't refactor files you aren't otherwise touching unless asked.
- No crate-wide `allow(dead_code)`. Remove unused code and dependencies, and the tests of removed code.
- No `unsafe` unless truly necessary; prefer a maintained safe wrapper crate. `main.rs` has `#![forbid(unsafe_code)]`; lifting it needs a good reason and a `// SAFETY:` comment on every `unsafe` block.
- Outside tests: no `unwrap()`/`expect()` (use `?`), no unchecked indexing or slicing, and no `let _ =` on a `Result` (handle it, or log it). Clippy enforces this (`[lints.clippy]` in `Cargo.toml`; `clippy.toml` exempts tests). Use `util::lock` for mutexes and `util::apply_mode` for permissions that may fail.
- Names are full words (`sequence`, not `sq` or `s`), including closure parameters.
- A clone for an async block goes inside the block passed to the spawn, under the same name: `tokio::spawn({ let session = session.clone(); async move { … } })`.
- Comments explain a non-obvious *why*, especially where behavior differs from Go. None that summarize the code or divide it into sections.
- Cookies are read twice, on purpose: `cookies.rs` follows Go for ytarchive's own requests, and `ytdlp_cookies.rs` mirrors yt-dlp's loader only to decide whether yt-dlp needs a repaired copy. Keep them separate.

## Testing

Run these as separate commands before pushing. Clippy is also the typecheck:

```sh
cargo fmt
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release --locked   # only when Cargo.toml, dependencies, or build.rs change
```

- While working, run only the tests for what you changed (`cargo test <name or module>`) after each change, and clippy once after a set of related changes. Run the whole suite for a broad change and before pushing. A passing run stays valid while its inputs are unchanged.
- If you don't know whether the tests you're about to affect already fail, run them once before the first edit and report failures that were already there.
- For a local test binary use `cargo build --profile fast` (`target/fast/`); use `--release` only to check the release build or to measure speed or memory.
- Keep tests essential: every option under "Usage and options", downloading and storage, quality/codec selection, file names, ffmpeg arguments, and parsing of URLs, yt-dlp output, watch pages, durations, and cookies. No tests for logging, the progress line, help layout, version strings, or small helpers.
- Tests for minor features go only in `src/minor_tests.rs` (`#[cfg(test)] mod minor_tests;`; create it with the first such test), so they can be deleted all at once.
- Download behavior cases go in the fake googlevideo server tests (`src/download/tests/mod.rs`; retrying in `tests/retry.rs`, SQLite and recovery in `tests/sqlite.rs`).
- Network tests are marked `#[ignore]`. Run them against a real 24/7 stream: `YTDLP_TEST_OPTS="--js-runtimes node" cargo test -- --ignored` (drop the variable if deno is installed).
- For changes to the download/mux flow, also do a short real run, e.g. `--live-from now --capture-duration 10s -o '/tmp/x/%(id)s'`, and check the result with ffprobe.
- To fail fragments on purpose (5xx, 429, 403, one that never arrives) against a real stream, put a local HTTP server in between: a `--ytdlp-path` wrapper rewrites the googlevideo URLs in yt-dlp's output to `http://127.0.0.1:PORT/HOST/…`, and the server forwards every other request (GET and HEAD).
- On Windows, a scripted Ctrl-C may never reach ytarchive (agent shells can pass on "ignore Ctrl-C"). Send Ctrl-Break, or reset the flag with `SetConsoleCtrlHandler(NULL, FALSE)` in the launcher.
- List what hasn't been verified in `docs/NOT_TESTED.md` (never CI jobs), and delete a line once it is verified.

## Download rules

- A live fragment is given up on only while other fragments download, while the stream is held at `MEMORY_LIMIT`, or once a `--capture-duration` download has started its last fragment. Keep all three working, or a download can wait forever.
- The state file may be behind the data but never ahead of it: save a state only after its fragment is stored.
- Don't merge a download that returned an error: merging moves the data away from its state. `run.rs` treats every error from `download_stream` as a storage error.
- Don't add `sync_all` before the rename in `util::write_atomic` (tried and reverted): each state save went from about 0.03 ms to 0.6 ms on NVMe ext4, once per fragment, and resuming got no safer, since the `.ts` data isn't synced either and `TsSink::resume` starts over when the `.ts` file is smaller than its state.
- Measure on a slow or throttled disk before changing `wal_autocheckpoint` in `src/download/db_sink.rs`: a larger value makes single inserts pause longer.
- Don't rename the `urls`/`meta` tables or their keys in the SQLite database without handling old databases; `--recover-private` reads them in a later run.
- A prompt where an empty answer means "go ahead" must tell closed stdin from an empty line (`prompt::get_user_input` returns an empty string for both) and treat closed stdin as "no", so unattended runs never wait.

## Commits

- Title (Conventional Commits): `<type>(<scope>): <lowercase imperative summary>`, no trailing period. Types: `feat`, `fix`, `refactor`, `test`, `docs`, `ci`, `chore`. The scope is the module (`download`, `cli`, …), left out for broad changes. Add `(#12)` only for a commit from a pull request. Releases are `chore: release 0.2.0`. Examples: `fix(download): close the database before cleanup in test`, `feat(cli): add --recover-private`.
- Body: a bullet list of what changed, plus a paragraph on Go bugs fixed or behavior changes.
- One commit per completed step, each building on its own and updating the docs it affects (`README.md`, `docs/CHANGES_FROM_GO.md`, `docs/NOT_TESTED.md`, this file). In a large docs update, attach each docs change to the commit it belongs to; only the rest goes into a separate `docs:` commit.
- Commit and push only when asked, and pause after each step for review.
- Never force-push or rewrite pushed history; unpushed commits may be reorganized. Never commit secrets.

## Releases and CI

- **Releases** are manual: push a `v*` tag; `release.yml` builds x64 and arm64 for every OS.
- **Nightly** (`nightly.yml`) runs only by hand and replaces the `nightly` pre-release. It skips when `main` has only docs commits (`docs:` or `docs(...):`) since the last one, so title docs commits that way.
- Both use `build.yml`: one zip per platform, `ytarchive_<windows|linux|macos>_<x64|arm64>.zip`, holding only the executable. It sets the `YTARCHIVE_*` variables for `build.rs`; `Cross.toml` passes them into `cross` containers.
- **CI** (`ci.yml`) runs only for pushes that touch the code or what builds and checks it; add a new such file to its `paths` list. Tests run on Windows x64, Linux x64, and macOS arm64; the other three release targets are built only by releases.
- **Network tests** (`network.yml`) run the tests marked `#[ignore]` weekly with yt-dlp nightly. They never block a push; a failure can also mean YouTube refused the runner's address.
- **Audit** (`audit.yml`) fails on a vulnerable dependency ("unmaintained" only warns) and runs cargo-deny (`deny.toml`): a license not on its list, or a source other than crates.io, fails. Add a new permissive license to the list.
