# Grabbit

**English** · [Italiano](README.it.md)

Download **video** and **music** from YouTube and Spotify with a simple interface.
Written in Rust (lightweight: the AppImage is about 4 MB), it relies on
[yt-dlp](https://github.com/yt-dlp/yt-dlp) and runs on Windows, Linux and macOS.
The interface is in Italian if your system language is Italian, and in English otherwise.

## What it does

- **Video**: MP4 / MKV / WebM, maximum quality of your choice (up to 4K).
- **Audio**: MP3, M4A (AAC), Opus, OGG Vorbis, FLAC, WAV, with a **bitrate** choice.
- Choose the destination folder; paste several links at once (one per line).
- **Search**: type an artist and title (or a video name) in the *Search* bar, pick from the
  YouTube results and press *Download* (right away) or *+* (add to the links). You can tick
  several results and download them together, in the chosen format (Video or Audio).
  Search **YouTube** or **YouTube Music** (songs, albums or videos): an album is downloaded
  in full, in a folder with its name. To use your Google account, pick the browser you are
  signed in with in *Advanced options → Use browser cookies*.
- **Playlists**: files go into a folder named after the playlist and are called
  `1 - Title.mp4`, `2 - Title.mp4`… (the playlist name is not part of the file name).
- Cover art and metadata embedded in the files.
- Queue with live progress and a **Cancel** button.
- **Sets itself up**: on first launch it downloads yt-dlp, ffmpeg and Deno
  (the JavaScript runtime yt-dlp needs for YouTube); on every launch it checks
  whether a newer yt-dlp exists and updates it.
- **Updates itself**: on every launch, right away (without waiting for the rest), it looks at the
  latest GitHub release and, if it is newer, downloads it, replaces the app and restarts it.
  A line at the bottom of the window always tells you how the check went.
  On Linux it works from the AppImage; it can be turned off in the advanced options.

### How Spotify works

Spotify encrypts its audio (DRM) and yt-dlp cannot download it. Grabbit only reads the
**public data** of the link (title and artist of a track, album or playlist), then
**searches for the same track on YouTube** and downloads the audio from there. So:

- the result is the audio of the YouTube version, not the original Spotify file;
- a playlist can show at most about 100 tracks;
- if Spotify changes the structure of its public pages, reading links may stop
  working until Grabbit is updated.

> Use Grabbit only for content you have the right to download and in compliance with
> the copyright and terms of service of the platforms.

## Installation

| System | File from the *Releases* page | Notes |
|---|---|---|
| Windows | `grabbit-windows-x86_64.exe` | double-click; SmartScreen may warn because the executable is not signed |
| Linux | `grabbit-linux-x86_64.AppImage` | `chmod +x` and double-click (auto-update only works from the AppImage) |
| macOS | `Grabbit-macos-aarch64.app.zip` (Apple Silicon) or `…-x86_64.app.zip` (Intel) | unzip, drag into *Applications*; the first time right-click → Open (the app is not notarized). If macOS calls it "damaged": `xattr -cr /Applications/Grabbit.app` |

Data and downloaded tools are stored in:
Windows `%APPDATA%\Grabbit\Grabbit\data`, Linux `~/.local/share/grabbit`,
macOS `~/Library/Application Support/org.Grabbit.Grabbit`.

## Creating a release

The Windows, Linux and macOS versions are built **on GitHub**, automatically
(`.github/workflows/release.yml`). Nothing needs to be installed on your PC.

### Preparation (once)

1. Create a repository on GitHub and upload the project (including the `.github` folder).
2. You do not need to edit `Cargo.toml`: the versions built by GitHub look for
   updates in the very repository they were built from. (Only if you build locally,
   set `repository` in `Cargo.toml` to your GitHub address.)
3. On GitHub: *Settings → Actions → General → Workflow permissions* and choose
   **Read and write permissions** (needed to create tags and releases).

### Every time you want to publish a version

1. On GitHub open the **Actions** tab → **Release** on the left → **Run workflow**.
2. In the *New version* field type the number, for example `0.2.0`, and confirm.
3. Wait 10-20 minutes. The workflow, by itself:
   - updates the version in `Cargo.toml`/`Cargo.lock` and creates the tag `v0.2.0`;
   - builds Windows, Linux and macOS (Intel and Apple Silicon);
   - publishes the release on the **Releases** page with all the files.
4. Apps that are already installed find the new release on their next launch and update.

Numbers follow the `major.minor.patch` format: `0.1.0` → `0.1.1` for a fix,
`0.2.0` for a new feature, `1.0.0` for the first stable version.
A version that has already been used cannot be published again: pick a higher one.

**Alternatives**

- *From a terminal*: raise `version` in `Cargo.toml`, commit, then
  `git tag v0.2.0 && git push --tags`. The same workflow starts
  (the tag must match the version in `Cargo.toml`, otherwise it stops).
- *Test only*: *Run workflow* leaving the version field **empty**. It builds everything
  and leaves the files in *Artifacts* at the bottom of the page, publishing nothing.

### Files produced (do not rename them: automatic updates look for these names)

| File | Purpose |
|---|---|
| `grabbit-windows-x86_64.exe` | Windows |
| `grabbit-linux-x86_64.AppImage` | Linux Intel/AMD |
| `grabbit-linux-aarch64.AppImage` | Linux ARM (only if the repository is public) |
| `Grabbit-macos-aarch64.app.zip` / `Grabbit-macos-x86_64.app.zip` | macOS, first installation |
| `grabbit-macos-aarch64` / `grabbit-macos-x86_64` | macOS, used by automatic updates |

### If something goes wrong

- **"Permission denied" or "protected branch"** in the *prepare* step: the permissions in
  step 3 of the preparation are missing, or the `main` branch is protected and does not accept
  commits from the workflow. In that case raise the version by hand and use the tag method.
- **"Process completed with exit code 126"**: a script file lost its execute permission
  during upload (zip/Windows). The workflow already runs scripts with `bash`,
  so using the latest version of the files is enough.
- **"Node.js 20 is deprecated" warnings**: they are just GitHub warnings, not errors.
- **A build fails**: open the red job and read the error (usually in the last lines),
  then fix the code. The release is never published halfway: it only goes out if Windows,
  Linux and macOS all succeed. After the fix, run again with a new version
  (if the workflow already created the tag, delete it from *Code → Tags* or use the next number).
- The **CI** workflow runs on every push and checks formatting, warnings and tests on
  all three systems, so errors show up before the release.

## Accessibility

Grabbit exposes its interface to screen readers through
[AccessKit](https://github.com/AccessKit/accesskit): Narrator and NVDA on Windows,
VoiceOver on macOS, Orca on Linux. Buttons, fields, lists and progress bars have names and
roles, the link field is labelled, the download state is also spoken in words (waiting,
in progress, completed, failed) and not only shown with colour, and everything can be
reached with the keyboard (Tab / Shift+Tab, Space or Enter). Text size can be increased in
the advanced options. If you find something a screen reader does not read correctly,
please open an issue.

## Building locally on Windows

With Rust installed (<https://rustup.rs>) double-click `packaging\windows\build.bat`:
it creates `grabbit-windows-x86_64.exe`.

## Building from source

```bash
cargo run --release                      # run
cargo test                               # unit tests
packaging/linux/build-appimage.sh        # create the AppImage (Linux)
python3 scripts/make_icon.py             # regenerate the icons (needs Pillow)
```

Requires Rust 1.85+. On Linux no special development libraries are needed.

Tests against a real yt-dlp (optional):

```bash
GRABBIT_YTDLP=/path/to/yt-dlp cargo test -- --ignored
```

To force the interface language (for testing): `GRABBIT_LANG=it` or `GRABBIT_LANG=en`.

## Structure

| File | Purpose |
|---|---|
| `src/app.rs` | graphical interface (egui) |
| `src/downloader.rs` | builds the yt-dlp commands, reads progress, cancels |
| `src/tools.rs` | downloads/updates yt-dlp, Deno, ffmpeg |
| `src/spotify.rs` | reads title/artist from Spotify links |
| `src/selfupdate.rs` | automatic app updates |
| `src/config.rs` | saved settings |
| `src/i18n.rs` | interface language (Italian / English) |

## Common problems

- **Automatic update does not start**: read the status line at the bottom of the window.
  It says whether Grabbit is already up to date, whether there is no release (or the repository
  is private: the app cannot read it, a public repository is needed), whether GitHub rate-limited
  the requests, or whether the release does not contain the file for your system. The *Check for
  Grabbit updates* button in the advanced options repeats the check immediately. After an
  update the app restarts once and does not update again in that run, to avoid loops.
- **"Sign in to confirm you're not a bot" / restricted videos**: in
  *Advanced options* choose the browser to take cookies from.
- **macOS**: ffmpeg is downloaded as the Intel version (it runs through Rosetta on Apple
  Silicon); if you have Homebrew (`brew install ffmpeg`) that one is used instead.
- **Linux without AppImage** (binary started by hand): it works, but the app cannot
  replace itself; it only shows the link to the new release.

## License

Grabbit is free software, released under the
[GNU General Public License v3.0 or later](https://www.gnu.org/licenses/gpl-3.0.html)
(`GPL-3.0-or-later`); see `LICENSE`. You may use, study, share and modify it; if you
distribute modified versions, they must be released under the same license with their source code.
