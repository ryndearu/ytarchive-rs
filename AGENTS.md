# AGENTS.md

A Rust port of [dreammu/ytarchive](https://github.com/dreammu/ytarchive) (a fork of [Kethsar/ytarchive](https://github.com/Kethsar/ytarchive)), a YouTube livestream archiver: yt-dlp extracts the URLs, ytarchive downloads the fragments and muxes with ffmpeg. `--sqlite` and `--recover-private` come from [CanOfSocks/livestream_dl](https://github.com/CanOfSocks/livestream_dl).

## Porting rules

- **Behave like dreammu/ytarchive** unless there is a reason not to: flag names, defaults, file names (`.f{itag}.ts`, `{id}.f{itag}.state`, `{id}__XXXX` temp dirs), state file format, and log messages.
- **Fix Go bugs instead of copying them**, and record every fix and intentional change in `docs/CHANGES_FROM_GO.md` (what Go did, what the port does).
- **Don't port dead code** (InnerTube request, ytcfg, SAPISIDHASH, DASH manifest parsing, UMP); list it under "Not ported".
- Clone the originals into `refs/` (gitignored) to compare, e.g. `git clone https://github.com/dreammu/ytarchive refs/dreammu-ytarchive`.

## Platforms

- **Windows, Linux, and macOS**, each as x64 and arm64, and nothing else. Every change must build and pass tests on all three.
- **File names**: anything that becomes a file or folder name goes through `util::sterilize_filename` with a `NameStyle`, and through `util::portable_file_name` when `--windows-filenames` is on. A suffix longer than `.f{itag}.frag{seq}.ts` needs a larger `SUFFIX_ROOM` (`output::final_location` keeps paths within MAX_PATH).
- Don't assume Unix in code or tests: no hard-coded `/tmp`, `sh`, or unescaped paths in JSON. Gate platform code with `#[cfg(windows)]` / `#[cfg(unix)]` (see `src/platform.rs`, `util::set_mode`).
- `rust-toolchain.toml` pins the Rust version; raise it in its own commit. When a dependency needs a newer Rust, update `rust-version` in `Cargo.toml` and the oldest version named in the README ("Building from source").
- TLS is rustls with `ring` and the OS certificate store; no OpenSSL.
- No custom allocator without measuring (mimalloc used two to four times the memory).
- Keep panic unwinding (no `panic = "abort"`): the download code catches a panicked worker, and `ConsoleGuard` restores the Windows console.

## Code layout and style

- One module per file; a module with submodules is a folder with a `mod.rs`, as everywhere in the crate. Shared types live in `src/types/`; update them before the feature that uses them.
- Split a file over 400 lines only when already editing it, and say so in the commit. Don't refactor untouched files unless asked.
- No crate-wide `allow(dead_code)`. Remove unused code and dependencies, and the tests of removed code.
- No `unsafe` unless truly necessary; prefer a maintained safe wrapper crate. Lifting `#![forbid(unsafe_code)]` in `main.rs` needs a good reason and a `// SAFETY:` comment on every `unsafe` block.
- Outside tests: no `unwrap()`/`expect()` (use `?`), no unchecked indexing or slicing, and no `let _ =` on a `Result` (handle or log it). Clippy enforces this (`[lints.clippy]` in `Cargo.toml`; `clippy.toml` exempts tests). Use `util::lock` for mutexes and `util::apply_mode` for permissions that may fail.
- Names are full words (`sequence`, not `sq`), closure parameters included.
- Import with `use` (std, external crates, then `crate::`) instead of writing out paths: `Duration::from_secs(5)`, not `std::time::Duration::from_secs(5)`. A single call may stay inline when the short name is unclear (`std::env::temp_dir()`).
- Clone for an async block inside the block passed to the spawn, under the same name: `tokio::spawn({ let session = session.clone(); async move { … } })`.
- Comments explain a non-obvious *why*, especially where behavior differs from Go; none that summarize code or divide it into sections. Keep comments stating a contract the code doesn't show (when data is copied, what `None` means, why a value is clamped).
- Cookies are read twice on purpose: `cookies.rs` follows Go for ytarchive's own requests; `ytdlp_cookies.rs` mirrors yt-dlp's loader only to decide whether yt-dlp needs a repaired copy. Keep them separate.

## Testing

Before pushing, run these as separate commands. Clippy is the typecheck; don't run `cargo check`. If a quiet command fails, rerun only the failing target once without `-q`.

```sh
cargo fmt
cargo test -q
cargo clippy -q --all-targets -- -D warnings
cargo build --release --locked   # only when Cargo.toml, dependencies, or build.rs change
```

- While working, run only the tests for what you changed (`cargo test <name or module>`) after each change, and clippy once after a set of related changes. Run the whole suite for a broad change and before pushing. A passing run stays valid while its inputs are unchanged.
- If you don't know whether the tests you'll affect already fail, run them once before the first edit and report failures that were already there.
- Local test binary: `cargo build --profile fast` (`target/fast/`); `--release` only to check the release build or measure speed or memory.
- Keep tests essential: every option under "Usage and options", downloading and storage, quality/codec selection, file names, ffmpeg arguments, and parsing of URLs, yt-dlp output, watch pages, durations, and cookies. None for logging, the progress line, help layout, version strings, or small helpers.
- Minor-feature tests go only in `src/minor_tests.rs` (`#[cfg(test)] mod minor_tests;`, created with the first one), so they can be deleted at once.
- Download behavior cases go in the fake googlevideo server tests: `src/download/tests/mod.rs`, retrying in `tests/retry.rs`, SQLite in `tests/sqlite.rs`, and recovery in `tests/recovery.rs`.
- Network tests are `#[ignore]` and no workflow runs them. Run them by hand against a real 24/7 stream: `YTDLP_TEST_OPTS="--js-runtimes node" cargo test -- --ignored` (drop the variable if deno is installed).
- For download/mux changes, also do a short real run (`--live-from now --capture-duration 10s -o '/tmp/x/%(id)s'`) and check the result with ffprobe.
- To fail fragments on purpose (5xx, 429, 403, never arriving) on a real stream, put a local HTTP server in between: a `--ytdlp-path` wrapper rewrites the googlevideo URLs in yt-dlp's output to `http://127.0.0.1:PORT/HOST/…`, and the server forwards every other request (GET and HEAD).
- On Windows, a scripted Ctrl-C may never arrive (agent shells can pass on "ignore Ctrl-C"). Send Ctrl-Break, or reset the flag with `SetConsoleCtrlHandler(NULL, FALSE)` in the launcher.
- List what hasn't been verified in `docs/NOT_TESTED.md` (never CI jobs); delete a line once it is verified.

## Download rules

- A live fragment is given up on only while other fragments download, while the stream is held at `MEMORY_LIMIT`, or once a `--capture-duration` download has started its last fragment. Keep all three working, or a download can wait forever.
- The state file may be behind the data, never ahead: save a state only after its fragment is stored.
- Don't merge a download that returned an error: merging moves the data away from its state. `run.rs` treats every error from `download_stream` as a storage error.
- No `sync_all` before the rename in `util::write_atomic` (tried and reverted): a state save went from about 0.03 ms to 0.6 ms on NVMe ext4, once per fragment, for no safer resume, since the `.ts` data isn't synced either and `TsSink::resume` starts over when the `.ts` file is smaller than its state.
- Measure on a slow or throttled disk before changing `wal_autocheckpoint` in `src/download/db_sink.rs`: a larger value makes single inserts pause longer.
- Don't rename the SQLite `urls`/`meta` tables or their keys without handling old databases; `--recover-private` reads them in a later run.
- A prompt where an empty answer means "go ahead" must tell closed stdin from an empty line (`prompt::get_user_input` returns `""` for both) and treat closed stdin as "no", so unattended runs never wait.

## Working in the repo

- Create and change files with the editor's file tools, never through the shell (redirection, `sed -i`, `perl -i`, `tee`). Never edit `.env` or `.env.*` files, except `.env.example`.
- Grep to find code before opening files, read only the part you need, and cap noisy output (`| head`; `| tail` for test and build failures).
- Inspect git with `git status -s`, `git diff --stat`, and `git log --oneline -n 20`.
- Delete temporary files and folders you created (test outputs, downloaded tools) before finishing.
- Be brief, and reply in the language of the latest message. Report completion in one sentence, then list only what applies: failing tests, untested changes, file splits, skipped steps, assumptions.

## Commits

- Title (Conventional Commits): `<type>(<scope>): <lowercase imperative summary>`, no trailing period. Types: `feat`, `fix`, `refactor`, `test`, `docs`, `ci`, `chore`. The scope is the module (`download`, `cli`, …), left out for broad changes. `(#12)` only for a commit from a pull request. Releases: `chore: release 0.2.0`. Example: `feat(cli): add --recover-private`.
- Body: a bullet list of what changed, plus a paragraph on Go bugs fixed or behavior changes.
- One commit per completed step, each building on its own and updating the docs it affects (`README.md`, `docs/CHANGES_FROM_GO.md`, `docs/NOT_TESTED.md`, this file). In a large docs update, attach each docs change to its commit; only the rest goes into a separate `docs:` commit.
- Commit and push only when asked, and pause after each step for review.
- Never force-push or rewrite pushed history; unpushed commits may be reorganized. Never commit secrets.
- Fold a fix into the unpushed commit it fixes instead of adding a follow-up commit.
- Before pushing, check each message against its diff; drop lines that no longer match (e.g. "Document in …" for a file the commit doesn't touch).
- Do everything a handoff asks, including PR descriptions and questions for the user, not only the code.

## Releases and CI

- **Releases** are manual: push a `v*` tag, and `release.yml` builds x64 and arm64 for every OS and uploads `SHA256SUMS` alongside the zips.
- **Nightly** (`nightly.yml`) runs only by hand, on `main`, builds Windows x64 and Linux x64, stages the zips and `SHA256SUMS` as a draft release, then replaces the `nightly` pre-release so its publish timestamp refreshes. Choose the targets in `build.yml`'s target list, not with a matrix `exclude`: an `include` entry brings an excluded combination back.
- Both list the commits since the last `v*` tag with `.github/changelog.sh`, and build with `build.yml`: one zip per platform, `ytarchive_<windows|linux|macos>_<x64|arm64>.zip`, holding only the executable. Every target builds natively (arm64 Linux on GitHub's arm64 runner). A `v*` tag selects every target and sets `YTARCHIVE_VERSION` for `build.rs` (which then leaves the commit out of the version); the commit and date come from git.
- **CI** (`ci.yml`) runs on pushes and pull requests to `main` touching the code or what builds and checks it; add any new such file to its `paths`. Tests run on Windows x64 and Linux x64; macOS and the arm64 targets are built only by releases.
- **Audit** (`audit.yml`) runs cargo-deny (`deny.toml`): a vulnerable dependency, a license not on its list, or a source other than crates.io fails ("unmaintained" and "unsound" only warn). Add new permissive licenses to the list.
