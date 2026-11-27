# AGENTS.md

Rust port of [dreammu/ytarchive](https://github.com/dreammu/ytarchive) (a fork of [Kethsar/ytarchive](https://github.com/Kethsar/ytarchive)), a YouTube livestream archiver. yt-dlp extracts URLs, ytarchive downloads fragments, ffmpeg muxes. `--sqlite` and `--recover-private` come from [CanOfSocks/livestream_dl](https://github.com/CanOfSocks/livestream_dl).

## Porting

- Match dreammu/ytarchive unless there's a reason not to: flags, defaults, file names (`.f{itag}.ts`, `{id}.f{itag}.state`, `{id}__XXXX` temp dirs), state format, log messages.
- Fix Go bugs instead of copying them. Record every fix and intentional change in `docs/CHANGES_FROM_GO.md` (what Go did, what the port does).
- Don't port dead code (InnerTube, ytcfg, SAPISIDHASH, DASH, UMP). List it under "Not ported".
- Compare against originals cloned into `refs/` (gitignored): `git clone https://github.com/dreammu/ytarchive refs/dreammu-ytarchive`.

## Platforms

- Windows, Linux, and macOS, each x64 and arm64, nothing else. Every change must build and pass tests on all three.
- File and folder names go through `util::sterilize_filename` with a `NameStyle`, and through `util::portable_file_name` when `--windows-filenames` is on. A suffix longer than `.f{itag}.frag{seq}.ts` needs a larger `SUFFIX_ROOM` (`output::final_location` keeps paths within MAX_PATH).
- No Unix assumptions in code or tests: no hard-coded `/tmp` or `sh`, no unescaped paths in JSON. Gate platform code with `#[cfg(windows)]`/`#[cfg(unix)]` (see `src/platform.rs`, `util::set_mode`).
- `rust-toolchain.toml` pins the Rust version; raise it in its own commit. When a dependency needs newer Rust, update `rust-version` in `Cargo.toml` and the oldest version in the README's "Building from source".
- TLS is rustls with `ring` and the OS certificate store. No OpenSSL.
- No custom allocator without measuring (mimalloc used 2–4× the memory).
- Keep panic unwinding (no `panic = "abort"`): download code catches panicked workers, and `ConsoleGuard` restores the Windows console.

## Code

- One module per file. A module with submodules is a folder with `mod.rs`. Shared types live in `src/types/`; update them before the feature that uses them.
- Split a file over 400 lines only when editing it anyway, and say so in the commit. Don't refactor untouched files unless asked.
- No crate-wide `allow(...)`. Fix clippy findings; suppress only a false positive, narrowly, with `reason`. Remove unused code, dependencies, and their tests.
- Change dependencies with `cargo add`/`cargo remove`; never hand-edit `Cargo.lock`.
- No `unsafe` unless truly necessary; prefer a maintained safe wrapper. Lifting `#![forbid(unsafe_code)]` in `main.rs` needs a good reason and a `// SAFETY:` comment on each `unsafe` block.
- Outside tests: no `unwrap()`/`expect()` (use `?`), no unchecked indexing or slicing, no `let _ =` on a `Result` (handle or log it). Clippy enforces this via `[lints.clippy]` in `Cargo.toml`; `clippy.toml` exempts tests. Use `util::lock` for mutexes and `util::apply_mode` for permissions that may fail.
- Full-word names (`sequence`, not `sq`), closure parameters included.
- Import with `use` (std, external crates, then `crate::`): `Duration::from_secs(5)`, not the full path. A single call may stay inline when the short name is unclear (`std::env::temp_dir()`).
- Clone for an async block inside a spawn, under the same name: `tokio::spawn({ let session = session.clone(); async move { … } })`.
- Comments: only a non-obvious *why* (especially where behavior differs from Go), or a contract the code doesn't show (when data is copied, what `None` means, why a value is clamped). No summaries or section headers.
- Cookies are read twice on purpose. `cookies.rs` follows Go for ytarchive's own requests; `ytdlp_cookies/` mirrors yt-dlp's loader only to decide whether yt-dlp needs a repaired copy. Keep them separate.

## Testing

Before pushing, run these as separate commands. Clippy is the typecheck, so don't run `cargo check`. If a quiet command fails, rerun only that target once without `-q`.

```sh
cargo fmt
cargo test -q
cargo clippy -q --all-targets -- -D warnings
cargo build --release --locked   # only when Cargo.toml, dependencies, or build.rs change
```

- While working, run the tests for what changed (`cargo test <name or module>`) after each change, and clippy after a set of related changes. Run the whole suite for broad changes and before pushing. A passing run stays valid while its inputs are unchanged.
- If you don't know whether the affected tests already fail, run them once before the first edit, and report failures that were already there.
- Local test binary: `cargo build --profile fast` (`target/fast/`). Use `--release` only to check the release build or measure speed or memory.
- Keep tests essential: every option under "Usage and options", downloads and storage, quality/codec selection, file names, ffmpeg arguments, and parsing of URLs, yt-dlp output, watch pages, durations, and cookies. None for logging, the progress line, help layout, version strings, or small helpers.
- Minor-feature tests go only in `src/minor_tests.rs` (`#[cfg(test)] mod minor_tests;`, created with the first one), so they can be deleted at once.
- Download behavior tests use the fake googlevideo server in `src/download/tests/`: `mod.rs`, retries in `retry/` (near-head in `retry/near_head.rs`), `resume.rs`, `sqlite.rs`, `recovery.rs`. Build them from the helpers in `tests/mod.rs` (`session_for`, `download_quickly`, `assert_output`, `assert_complete`, `stored_database`, `fail_inserts`, `forbidden_from`).
- A unit test goes in the tests of the module it calls (a `DbSink` test in `db_sink/tests.rs`, not `download/tests/`).
- Don't repeat a test for another input. Add a case to a table-driven test (a `for` over the cases, naming the case in each assert message).
- Tests that need a local HTTP server use `src/test_http.rs` (`serve`, `path`, `response`), not their own socket loop.
- Network tests are `#[ignore]`, and no workflow runs them. Run them by hand against a real 24/7 stream: `YTDLP_TEST_OPTS="--js-runtimes node" cargo test -- --ignored` (drop the variable if deno is installed).
- For download or mux changes, also do a short real run (`--live-from now --capture-duration 10s -o '/tmp/x/%(id)s'`) and check the result with ffprobe.
- To fail fragments on purpose (5xx, 429, 403, never arriving) on a real stream, put a local HTTP server in between: a `--ytdlp-path` wrapper rewrites googlevideo URLs in yt-dlp's output to `http://127.0.0.1:PORT/HOST/…`, and the server forwards every other request (GET and HEAD).
- On Windows, a scripted Ctrl-C may never arrive (agent shells can pass "ignore Ctrl-C"). Send Ctrl-Break, or reset the flag with `SetConsoleCtrlHandler(NULL, FALSE)` in the launcher.
- List what hasn't been verified in `docs/NOT_TESTED.md` (never CI jobs). Delete a line once it's verified.

## Download rules

- A live fragment is given up on only while other fragments download, while the stream is held at `MEMORY_LIMIT`, or once a `--capture-duration` download has started its last fragment. Keep all three working, or a download can wait forever.
- The state file may be behind the data, never ahead: save a state only after its fragment is stored.
- Don't merge a download that returned an error, since merging moves data away from its state. `run/download.rs` treats every error from `download_stream` as a storage error.
- No `sync_all` before the rename in `util::write_atomic` (tried and reverted). A state save went from about 0.03 ms to 0.6 ms on NVMe ext4, once per fragment, for no safer resume: the `.ts` data isn't synced either, and `TsSink::resume` starts over when the `.ts` file is smaller than its state.
- Measure on a slow or throttled disk before changing `wal_autocheckpoint` in `src/download/db_sink/mod.rs`. A larger value makes single inserts pause longer.
- Don't rename the SQLite `urls`/`meta` tables or their keys without handling old databases; `--recover-private` reads them in later runs.
- A prompt where an empty answer means "go ahead" must tell closed stdin from an empty line (`prompt::get_user_input` returns `""` for both), and treat closed stdin as "no", so unattended runs never wait.

## Working in the repo

- Change files with the editor's file tools, never the shell (redirection, `sed -i`, `perl -i`, `tee`). Never edit `.env` or `.env.*` except `.env.example`.
- Grep before opening files, read only what you need, and cap output (`| head`; `| tail` for test and build failures).
- Inspect git with `git status -s`, `git diff --stat`, and `git log --oneline -n 20`.
- Delete temporary files and folders you created (test outputs, downloaded tools) before finishing.
- Be brief, and reply in the user's language. Report completion in one sentence, then list only what applies: failing tests, untested changes, file splits, skipped steps, assumptions.

## Commits

- Title (Conventional Commits): `<type>(<scope>): <lowercase imperative summary>`, no trailing period. Types: `feat`, `fix`, `refactor`, `test`, `docs`, `ci`, `chore`. The scope is the module (`download`, `cli`), omitted for broad changes. `(#12)` only for a PR commit. Releases: `chore: release 0.2.0`. Example: `feat(cli): add --recover-private`.
- Body: a bullet list of what changed, plus a paragraph on Go bugs fixed or behavior changes.
- One commit per completed step. Each builds on its own and updates the docs it affects (`README.md`, `docs/CHANGES_FROM_GO.md`, `docs/NOT_TESTED.md`, this file). In a large docs update, attach each change to its commit; the rest goes in a separate `docs:` commit.
- Commit and push only when asked, and pause after each step for review.
- Never force-push or rewrite pushed history; unpushed commits may be reorganized. Never commit secrets.
- Fold a fix into the unpushed commit it fixes, not a follow-up commit.
- Before pushing, check each message against its diff, and drop lines that no longer match.
- Do everything a handoff asks, including PR descriptions and questions for the user.

## Releases and CI

- **Releases** are manual: push a `v*` tag. `release.yml` builds x64 and arm64 for every OS and uploads `SHA256SUMS` with the zips. Workflows default to `contents: read`; only the `publish` jobs get `contents: write`, since the build jobs run `build.rs`, proc macros, and third-party actions. A tag with a suffix (`v1.2.0-rc.1`) is published as a prerelease, any other as Latest.
- **Nightly** (`nightly.yml`) runs only by hand, on `main`. It builds Windows x64 and Linux x64, stages the zips and `SHA256SUMS` as a draft, then replaces the `nightly` pre-release so its publish time refreshes. Choose targets in `build.yml`'s target list, not with a matrix `exclude` (an `include` brings excluded combinations back).
- Both list the commits since the last `v*` tag with `.github/changelog.sh` (a stable release skips earlier release candidates, so its list starts at the previous stable tag), and build with `build.yml`: one zip per platform, `ytarchive_<windows|linux|macos>_<x64|arm64>.zip`, holding only the executable. Every target builds natively (arm64 Linux on GitHub's arm64 runner). A `v*` tag selects every target and sets `YTARCHIVE_VERSION` for `build.rs`, which then leaves the commit out of the version. The commit and date come from git.
- **CI** (`ci.yml`) runs on pushes and pull requests to `main` that touch the code or what builds and checks it. Add any new such file to its `paths`. Tests run on Windows x64 and Linux x64; macOS and the arm64 targets are built only by releases.
- **Audit** (`audit.yml`) runs cargo-deny (`deny.toml`). A vulnerable dependency, a license not on its list, or a source other than crates.io fails; "unmaintained" and "unsound" only warn. Add new permissive licenses to the list.
