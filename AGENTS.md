# AGENTS.md

## What this project is

A Rust port of [dreammu/ytarchive](https://github.com/dreammu/ytarchive) (a fork of [Kethsar/ytarchive](https://github.com/Kethsar/ytarchive)), a YouTube livestream archiver: yt-dlp extracts the URLs, ytarchive downloads the fragments and muxes with ffmpeg. `--sqlite` and `--recover-private` come from [CanOfSocks/livestream_dl](https://github.com/CanOfSocks/livestream_dl).

## Porting rules

- **Behave like dreammu/ytarchive** unless there is a reason not to. Flag names, defaults, file names (`.f{itag}.ts`, `{id}.f{itag}.state`, `{id}__XXXX` temp dirs), state file format, and log messages follow Go.
- **Fix Go bugs instead of copying them.** Record every fix and every intentional behavior change in `docs/CHANGES_FROM_GO.md` (what Go did, what the port does).
- **Don't port dead code.** Code that is unreachable in Go (InnerTube request, ytcfg, SAPISIDHASH, DASH manifest parsing, UMP) stays out; list it under "Not ported".
- To compare with the originals, clone them into `refs/` (gitignored): `git clone https://github.com/dreammu/ytarchive refs/dreammu-ytarchive` (likewise Kethsar/ytarchive and CanOfSocks/livestream_dl).

## Platforms

- **Windows, Linux, and macOS are supported**, each as x64 and arm64, and nothing else (no code or docs for other platforms); every change must build and pass tests on all three.
- **File names**: anything that becomes a file or folder name goes through `util::sterilize_filename` with a `NameStyle`, and through `util::portable_file_name` when `--windows-filenames` is on. If you add a longer file suffix than `.f{itag}.frag{seq}.ts`, raise `SUFFIX_ROOM` (`output::final_location` keeps paths within MAX_PATH).
- Don't assume Unix in code or tests: no hard-coded `/tmp`, `sh`, or unescaped paths in JSON; gate platform code with `#[cfg(windows)]` / `#[cfg(unix)]` (see `src/platform.rs`, `util::set_mode`).
- The Rust version is pinned in `rust-toolchain.toml`; raise it in its own commit. Raise `rust-version` in `Cargo.toml` (checked by CI) when a dependency needs a newer Rust.
- TLS is rustls with the `ring` provider and the OS certificate store; don't add OpenSSL.
- Don't add a custom allocator without measuring (mimalloc used two to four times the memory). Keep panic unwinding (no `panic = "abort"`): the download code catches a panicked worker, and `ConsoleGuard` restores the Windows console.

## Code layout and style

- One module per file. Shared types live in `src/types/`; define or update them before the feature that uses them.
- A file over 400 lines must be split, but only when you are already editing it; say so in the commit. Don't refactor files you aren't otherwise touching unless asked.
- No crate-wide `allow(dead_code)`. Remove unused code and dependencies, and the tests of removed code. Tests for minor or unimportant features go only in `src/minor_tests.rs` (`#[cfg(test)] mod minor_tests;`), so they can be deleted all at once.
- No `unsafe` unless it's truly necessary; prefer a maintained safe wrapper crate. `main.rs` has `#![forbid(unsafe_code)]`; lifting it needs a good reason, and every `unsafe` block then needs a `// SAFETY:` comment.
- Outside tests: no `unwrap()`/`expect()` (propagate with `?`), no unchecked indexing or slicing, and no `let _ =` on a `Result` (handle it, or log it if it can be ignored). Clippy enforces this (`[lints.clippy]` in `Cargo.toml`; tests are exempt through `clippy.toml`). Use `util::lock` for mutexes and `util::apply_mode` for permissions that may fail.
- Names are full words (`sequence`, not `sq` or `s`), including closure parameters.
- A clone made for an async block goes inside the block passed to the spawn, under the same name: `tokio::spawn({ let session = session.clone(); async move { … } })`.
- Comments explain a non-obvious *why*, especially where behavior differs from Go. No comments that summarize the code or divide it into sections.
- Cookies are read twice, on purpose: `cookies.rs` follows Go for ytarchive's own requests, and `ytdlp_cookies.rs` mirrors yt-dlp's loader only to decide whether yt-dlp needs a repaired copy. Keep them separate; each must match its own target.

## Testing

Run these as separate commands before pushing (CI runs clippy and tests on all three platforms). Clippy is also the typecheck; there's no separate `cargo check` step:

```sh
cargo fmt
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release --locked   # only when Cargo.toml, dependencies, or build.rs change
```

- For a local test or debugging binary, use `cargo build --profile fast` (output in `target/fast/`); it rebuilds in seconds. Use `--release` only to check the release build or to measure speed or memory.
- While working, run only the tests for what you changed (`cargo test <name or module>`) after each change, and clippy once after a set of related changes. Run the whole suite when the change is broad and before pushing. A passing run stays valid while its inputs are unchanged.
- If you don't know whether the tests you're about to affect already fail, run them once before the first edit and report failures that were already there.
- Keep tests essential: every option under "Usage and options", downloading and storage, quality/codec selection, file names, ffmpeg arguments, and parsing of URLs, yt-dlp output, watch pages, durations, and cookies. Don't keep tests for logging, the progress line, help layout, version strings, or small helpers.
- Add download behavior cases to the fake googlevideo server tests (`src/download/tests/mod.rs`; SQLite and recovery in `tests/sqlite.rs`).
- Don't rename the `urls`/`meta` tables or their keys in the SQLite database without handling old databases; `--recover-private` reads them in a later run.
- Measure on a slow or throttled disk before changing `wal_autocheckpoint` in `src/download/db_sink.rs`: a larger value makes single inserts pause longer.
- Network tests are marked `#[ignore]`. Run them against a real 24/7 stream with: `YTDLP_TEST_OPTS="--js-runtimes node" cargo test -- --ignored` (drop the env var if deno is installed).
- For changes to the download/mux flow, also do a short real run, e.g. `--live-from now --capture-duration 10s -o '/tmp/x/%(id)s'`, and check the result with ffprobe.
- To test failing fragments (5xx, 429, 403, one that never arrives) against a real stream, put a local HTTP server in between: a `--ytdlp-path` wrapper rewrites the googlevideo URLs in yt-dlp's output to `http://127.0.0.1:PORT/HOST/…`, and the server forwards every request (GET and HEAD) except the ones it fails on purpose.
- A live fragment is given up on only while other fragments download, while the stream is held at `MEMORY_LIMIT`, or once a `--capture-duration` download has started its last fragment. Keep all three working when changing the retry rules, or a download can wait forever.
- On Windows, a scripted Ctrl-C may never reach ytarchive (agent shells can pass on "ignore Ctrl-C"). Send Ctrl-Break, or reset the flag with `SetConsoleCtrlHandler(NULL, FALSE)` in the launcher, before judging interrupt behavior.
- Prompts where an empty answer means "go ahead" must tell closed stdin from an empty line (`prompt::get_user_input` returns an empty string for both) and treat closed stdin as "no", so non-interactive runs never wait unattended.
- List what hasn't been verified in `docs/NOT_TESTED.md`, and delete a line once it is verified.

## Commits

- Title format (Conventional Commits): `<type>(<scope>): <lowercase imperative summary>`, no trailing period. Types: `feat`, `fix`, `refactor`, `test`, `docs`, `ci`, `chore`. The scope is the module (`download`, `cli`, `tui`, …) and can be left out for broad changes (`docs: …`). Add the PR number at the end, `(#12)`, only when the commit comes from a pull request. Releases are `chore: release 0.2.0`. Examples: `fix(download): close the database before cleanup in test`, `feat(cli): add --recover-private`, `docs: describe --recover-private`.
- Body: bullet list of what changed, plus a paragraph on Go bugs fixed or behavior changes.
- One commit per completed step. Each commit also updates the docs it affects (`README.md`, `docs/CHANGES_FROM_GO.md`, `docs/NOT_TESTED.md`, this file) in the same commit.
- Exception: when the user asks for a large docs update, attach each docs change to the commit it belongs to; only changes with no relevant commit go into a separate `docs:` commit.
- Each commit should build on its own. Commit and push only when asked, and pause after each step for review.
- Never force-push or rewrite pushed history. Unpushed commits may be reorganized. Never commit secrets.

## Releases

- **Releases** are manual: push a `v*` tag (e.g. `v0.1.0`); `release.yml` builds x64 and arm64 for every OS.
- **Nightly** (`nightly.yml`) runs weekly and replaces the `nightly` pre-release. It skips when `main` only has docs commits (`docs:` or `docs(...):`) since the last one, so title docs commits that way.
- Both use `build.yml`: one zip per platform, `ytarchive_<windows|linux|macos>_<x64|arm64>.zip`, holding only the executable. It sets the `YTARCHIVE_*` variables for `build.rs`; `Cross.toml` passes them into `cross` containers.
- **CI** (`ci.yml`) runs only for pushes that touch the code or what builds and checks it; add a new such file to its `paths` list.
- **Audit** (`audit.yml`) fails on a vulnerable dependency; "unmaintained" notices are only warnings.
