# Changelog

## 0.2.0 (2026-10-28)

The studio is rewritten in Rust and now comes as a **desktop app** for Windows, macOS and Linux as well as the
Docker server. Same features, same data folder: an existing `data/` keeps working. (0.1.0 was never published, so
this is the first public release.)

**Desktop app (new)**
- Installers for Windows (setup / .msi, per-user, no admin), macOS (.dmg) and Linux (.deb, AppImage).
- Its own window with a tray icon: open, results folder, start at login, power off the Tiiny (with a
  confirmation), quit. A second launch brings the open window forward; window size and position are remembered.
- Downloads land in your Downloads folder with a "Saved to …" note (never overwriting); the Paste buttons read
  pictures from the system clipboard; the microphone works in Chat and Audio.
- Only this computer can reach it; the settings text says so instead of showing Docker/.env advice.

**Server (Rust)**
- The same routes and answers as the Python server, checked by contract tests that send identical requests to
  both against a fake Tiiny (and compare what each asks the device), plus live runs on a real Tiiny.
- Docker image built from the Rust server: about 150 MB, multi-arch.

**Security (both servers)**
- Other websites can't use the studio: changes sent from another site's page are refused (cross-site request
  forgery), and while network access is off only `localhost` names are answered (DNS rebinding). Before this, a
  page open in the same browser could, for example, delete everything or power off the Tiiny.

**New in both**
- **Open folder** button in the gallery (on the computer running the studio).
- **Prompt-injection screening for chat attachments:** documents are wrapped as untrusted content, the model is
  told never to follow instructions inside them, and suspicious ones are flagged above the reply.
- Image jobs wait and retry when a freshly loaded model isn't ready yet ("xh2a backend fail").
- A 3D job's Stop now ends it even if the device stops reporting, instead of holding the device.
- Privacy notes now explain what the Tiiny itself keeps (since TiinyOS 0.1.36 its vault stores every gateway chat
  as full text, whatever the memory scope) and how to delete a day, with a "What your Tiiny keeps" guide.
- ROADMAP.md: what works, what's planned, known limits.

**Checked against** TiinyOS device services 0.1.35 and 0.1.36. **Tested hands-on:** the Windows x64 app and the
Docker image (amd64). The macOS, Linux and Windows ARM apps and the arm64 image are built and smoke-tested by CI
only.

## 0.1.0 (2026-10-23, not published)

The Python version. Built and tested against TiinyOS 0.1.35 on a Tiiny Pocket.

**Image**
- Create from a prompt, edit with plain-language instructions, circle an area to change, up to 10
  reference images, transparent PNGs, before/after compare.
- ×4 variations shown as a grid; sizes and shapes limited to the ones each model is verified for.
- Steps, guidance, size and negative prompt remembered per model.

**Text (OCR and documents)**
- Read the text in photos, scans and screenshots (PP-OCRv6 Small/Medium or GLM-OCR).
- Documents: PDF (text read directly, scanned pages OCR'd), multi-page TIFF, Word, PowerPoint, Excel,
  OpenDocument, EPUB, RTF, HTML and plain text, with pictures inside documents OCR'd in place.
  Progress and a Stop button, up to 50 pages per run.
- View as plain text (with Join lines) or Markdown that keeps headings, lists and tables; copy, or save
  as .txt / .md, or a .zip with the pictures for Obsidian or any notes folder. Nothing is saved.

**Chat**
- Streaming replies with an optional thinking view; attach pictures and documents; speak your message.
- The model can make things with the studio's tools: create and edit images, read text aloud in a
  saved voice, make loops and song previews. When a tool's model doesn't fit next to the chat model,
  the studio unloads the chat model, loads the tool's, does the work and loads the chat model back.
- Chats saved in `data/chats` (switchable off); each delete removes a conversation completely.

**Audio**
- Voice cloning with a saved voice library; long text voiced in parts and joined.
- Tempo- and key-locked loops; full songs with lyrics, instrumental, vocals only or separate tracks,
  an 8-second preview you can continue to the full song, and an MP3 copy.
- Transcription in 30 languages.

**3D**
- Image → shape → simplify → PBR texture → GLB, with an interactive viewer.

**Everywhere**
- Gallery with search, smart search by meaning, favorites, multi-select delete and zip export.
- Models panel: load/unload any installed model or all at once, NPU budget, live device stats.
- Drop, paste (Ctrl+V, aimed at the zone you point at) or Paste buttons on every file slot.
- Saved devices (home Wi-Fi, hotspot, USB) with auto-switching to whichever one answers.
- Works over USB as well as Wi-Fi/LAN.
- Private by default: only this computer can open it unless you opt in (with an optional password);
  deleting a result removes its prompt, files and every copy; Delete everything in Settings.
- Three ways to run it: a standalone download for Windows, macOS and Linux (no Python or Docker needed),
  Docker with a prebuilt image for Intel and ARM, or `python server.py` (Python 3.10+, no packages).
- One small standard-library Python server plus a vanilla-JS front end.

**Known device issues (TiinyOS 0.1.35)**
- Hunyuan3D's background stage fails (BriefNet missing); SongGeneration's style reference fails.
- GLM-OCR can't be reached through the device gateway, so it works over Wi-Fi/LAN only.
- No video generation on the device yet.
