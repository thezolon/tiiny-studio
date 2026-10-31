# Roadmap

Where Tiiny Studio stands, what's next, and what doesn't work yet. Version **0.4.0**, checked against TiinyOS
0.1.35 to 0.1.38 on a Tiiny Pocket.

## Works now

- **Two ways to run it, one codebase (Rust):** a desktop app for Windows, macOS and Linux (x64 and ARM), and a small server in
  Docker (amd64 + arm64) for a home server, used from any browser on your network.
- **Image:** create (from a prompt, or a new picture from up to 10 reference pictures) and edit (plain-language instructions, circled areas, up to 10 references), transparent
  PNGs, before/after, ×4 variations; **Text** mode reads pictures, scans and documents (PDF, Office,
  OpenDocument, EPUB, RTF, HTML, plain text) with PP-OCRv6 or GLM-OCR, as plain text or Markdown.
- **Chat:** streaming replies, thinking view, pictures and documents as attachments, speech input, and tools
  that create/edit images, speak, and make loops and songs, swapping models on the Tiiny when they don't fit
  together. Attached documents are delimited and screened for prompt injection; suspicious ones are flagged.
- **Audio:** voice cloning with a saved voice library, built-in and described voices, tempo/key-locked loops, full
  songs (SongGeneration with an 8-second preview, AceStep with its own settings, YuE2), MP3 copies, transcription in
  30 languages.
- **Video** (MiniMax-H3-Turbo): text or a start picture to a short clip with sound.
- **Panels follow the loaded model:** Chat's thinking, Voice, Songs, 3D and Video read their options from the model.
- **3D** (Hunyuan3D-2.1 or TRELLIS.2-4B): image → shape → simplify → texture → GLB, with a viewer. The subject is cut out first (Qwen-Image), so the
  background doesn't end up in the model.
- **Gallery, models panel and device:** search and smart search, favourites, zip export, bulk delete; load and
  unload models with the NPU memory budget shown; live device activity; power off.
- **Model swapping:** helpers (Improve prompt, Describe, Write lyrics, loop tags) and chat tools load the model they
  need and put yours back afterwards. A global switch and one per section in Settings (all on by default), and a
  choice of which text model the helpers use.
- **Jobs:** one queue on the server (the Tiiny does one thing at a time) with start and finish times; jobs keep
  running if you close the page, ride out a dropped connection, and the desktop app keeps the computer awake for
  them; **cancel** any running or queued job (or remove all waiting ones), and the Tiiny's work stops too. Quitting
  the app or stopping the server stops running work on the device first. Results left on the Tiiny are saved, and
  deleting a song or loop deletes the Tiiny's copy too. The Tiiny's free space is watched.
- **Connections:** Wi-Fi/LAN and USB, several saved devices with auto-switch.
- **Private by default:** results stay on your computer, complete deletes, no telemetry, network access off
  unless you turn it on (with an optional password).

Tested: unit tests and contract tests (the Rust server against the original Python one, both talking to a fake
Tiiny) on Windows (x64 and ARM), macOS and Linux, plus live runs on a real Tiiny. **Hands-on, only the Windows x64
app and the Docker image (amd64) have been tested**; everything else is built and smoke-tested by CI only (see
Known limits).

## Planned

Ideas and requests are welcome as [feature requests](https://github.com/thezolon/tiiny-studio/issues/new/choose).

- **Faster cancel for image and audio jobs** once TiinyOS offers an interrupt for them (asked for).
- **Hands-on checks of the macOS, Linux and Windows ARM apps and the arm64 Docker image** (built by CI, see Known
  limits).
- **Several users on one Docker studio (being designed).** Each person signs in to their own gallery, chats
  and voices, and the others can't see them. Chat memory would be kept per user by the studio, not on the Tiiny.
  This only separates people inside the studio: the Tiiny's own vault still keeps a copy of every chat it receives,
  from any user (see [Privacy](README.md#privacy)).
- A clearer "Stopping…" message while a cancel is in progress.
- Code-signed installers (Windows) and a notarized macOS app, then automatic updates.
- Following TiinyOS updates: the API is snapshotted and checked for drift after each one.

### Next release

- **After a Tiiny restart, say why it doesn't answer:** it stays locked until the TiinyOS app has been opened once
  ([#46](https://github.com/thezolon/tiiny-studio/issues/46)).
- **Pick which model a tab uses** when several fit (Hunyuan3D or TRELLIS.2, the song models, voices) ([#44](https://github.com/thezolon/tiiny-studio/issues/44)).
- **A small notice when a new version is out,** with its release notes, and a Check for updates button ([#45](https://github.com/thezolon/tiiny-studio/issues/45)).
- Fix the broken model icon on the Video tab ([#43](https://github.com/thezolon/tiiny-studio/issues/43)), and the rest of the panels that should follow the loaded
  model: image steps and guidance, transcription languages, loops ([#41](https://github.com/thezolon/tiiny-studio/issues/41)).

### Ideas

**Make more with what's already there**
- **Longer videos by chaining clips:** each clip starts from the last frame of the one before, and the parts are
  joined into one video, so a 10-20 s video can be made from 2 s pieces in an overnight queue.
- **Time estimates that learn** from how long jobs really took on your Tiiny, in every tab.
- **Batches:** one prompt with several seeds, or a small grid of prompts and styles, queued in one click.
- **Music videos:** a song and video clips joined into one MP4 with the song as the soundtrack.
- **Read a document aloud:** a PDF or Office file from Text mode, voiced as a long recording.
- **Subtitles:** transcribe a clip or recording into an `.srt` file.

**Queue and device**
- **Notifications when a job or the whole queue is done** (or fails): on the desktop, and optionally on your phone.
- **Start a queue at a set time.**
- **Energy per job,** from the power the Tiiny reports.
- **Install and remove models from the studio,** without the TiinyOS app.

**Library**
- **Projects and tags** instead of one long gallery per tab.
- **The prompt and settings stored in the files themselves** (PNG text, MP3/MP4 tags), so they travel with them.
- **Saved presets** per tab.

**Let other tools use it**
- **An MCP server,** so Claude, Cline or other agents can make images, speech, music and video on your Tiiny through
  the studio, with its queue and model swapping.
- **Chat with your documents,** using the Tiiny's embedding model.
- Later: **a coding-agent tab** for the Tiiny's larger chat models.

## Known limits

- **Cancelling an image or audio job takes about 25 seconds.** The Tiiny has no way to interrupt those models,
  so the studio unloads the model and loads it again; that's the only way to stop the work. Chat is interrupted
  immediately. An interrupt for image and audio has been requested from Tiiny.
- **The installers are unsigned.** Windows shows "Windows protected your PC" (More info → Run anyway) and macOS
  asks before the first open (right-click → Open). There's no automatic updater until the apps are signed.
- **Only the Windows x64 app and the Docker image (amd64) have been tested hands-on.** The macOS (Apple Silicon and
  Intel), Linux (x64 and ARM) and Windows ARM apps and the arm64 Docker image are built by CI, which checks that
  the server inside starts and serves the page, but none has been installed and used with a Tiiny yet (one user
  reports the Linux AppImage starts and runs).
- **Over USB, GLM-OCR isn't available and songs come as one mix** (no separate tracks): the USB connection blocks
  the models' own ports, which both need. Over Wi-Fi/LAN both work (live-tested 2026-10-27).
- **Power off needs TiinyOS 0.1.37 or later** (on 0.1.36 the Tiiny answers but stays on). Update TiinyOS, or use
  the TiinyOS app or the power button.
- **Docker and USB:** if the Tiiny's USB address is `172.17.x.x`, Docker can't reach it (use the desktop app or
  move Docker's network).
- **Linux desktop needs WebKitGTK 4.1.**
- **Not read:** old binary Office files (.doc/.xls/.ppt) and iPhone HEIC photos. OCR runs up to 50 pages per run.
- **What the Tiiny itself keeps is outside the studio's control.** See [Privacy](README.md#privacy).
