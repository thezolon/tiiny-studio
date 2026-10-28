# Roadmap

Where Tiiny Studio stands, what's next, and what doesn't work yet. Version **0.2.0**, checked against TiinyOS
device services 0.1.35 and 0.1.36 on a Tiiny Pocket.

## Works now

- **Two ways to run it, one codebase (Rust):** a desktop app for Windows, macOS and Linux (x64 and ARM), and a small server in
  Docker (amd64 + arm64) for a home server, used from any browser on your network.
- **Image:** create and edit (plain-language instructions, circled areas, up to 10 references), transparent
  PNGs, before/after, ×4 variations; **Text** mode reads pictures, scans and documents (PDF, Office,
  OpenDocument, EPUB, RTF, HTML, plain text) with PP-OCRv6 or GLM-OCR, as plain text or Markdown.
- **Chat:** streaming replies, thinking view, pictures and documents as attachments, speech input, and tools
  that create/edit images, speak, and make loops and songs, swapping models on the Tiiny when they don't fit
  together. Attached documents are delimited and screened for prompt injection; suspicious ones are flagged.
- **Audio:** voice cloning with a saved voice library, tempo/key-locked loops, full songs with an 8-second
  preview, MP3 copies, transcription in 30 languages.
- **3D:** image → shape → simplify → texture → GLB, with a viewer.
- **Gallery, models panel and device:** search and smart search, favourites, zip export, bulk delete; load and
  unload models with the NPU memory budget shown; live device activity; power off.
- **Jobs:** one queue on the server (the Tiiny does one thing at a time); jobs keep running if you close the
  page; **cancel** any running or queued job, and the Tiiny's work stops too. Quitting the app or stopping the
  server stops running work on the device first.
- **Connections:** Wi-Fi/LAN and USB, several saved devices with auto-switch.
- **Private by default:** results stay on your computer, complete deletes, no telemetry, network access off
  unless you turn it on (with an optional password).

Tested: unit tests and contract tests (the Rust server against the original Python one, both talking to a fake
Tiiny) on Windows (x64 and ARM), macOS and Linux, plus live runs on a real Tiiny. **Hands-on, only the Windows x64
app and the Docker image (amd64) have been tested**; everything else is built and smoke-tested by CI only (see
Known limits).

## Planned

- **Faster cancel for image and audio jobs** once TiinyOS offers an interrupt for them (asked for).
- **Hands-on checks of the macOS, Linux and Windows ARM apps and the arm64 Docker image** (built by CI, see Known
  limits).
- Desktop notifications when a long job finishes.
- A clearer "Stopping…" message while a cancel is in progress.
- Code-signed installers (Windows) and a notarized macOS app, then automatic updates.
- Following TiinyOS updates: the API is snapshotted and checked for drift after each one.

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
- **Power off doesn't work on the latest TiinyOS** (device services after 0.1.36): a known firmware issue that Tiiny
  will fix in a patch release. Use the TiinyOS app or the power button until then.
- **Docker and USB:** if the Tiiny's USB address is `172.17.x.x`, Docker can't reach it (use the desktop app or
  move Docker's network).
- **Linux desktop needs WebKitGTK 4.1.**
- **Not read:** old binary Office files (.doc/.xls/.ppt) and iPhone HEIC photos. OCR runs up to 50 pages per run.
- **What the Tiiny itself keeps is outside the studio's control.** See [Privacy](README.md#privacy).
