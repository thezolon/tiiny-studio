# Changelog

## Unreleased

- **Error handling:** a job keeps talking to the device it started on (and sends only that device's key) even if the studio switches devices meanwhile; a result the disk can't hold is reported ("Couldn't save the result") instead of silently missing; a failed or cancelled job leaves no files behind; an answer that isn't JSON, a bad request body or one over 150 MB gets a clear error instead of a crash.

## 0.4.0 (2026-10-30)

Video with sound, panels that follow the loaded model, and a queue you can leave running overnight.

**Video**

- **New Video tab** for **MiniMax-H3-Turbo**: write a prompt, or start from a picture, and get a short clip **with
  matching sound** (MP4). Sizes (wide or tall, 480p or 768p), steps and frame rate come from the loaded model. Any
  image has an **Animate** button that opens it here.
- **Video is slow on the Tiiny:** about 7 minutes for 2 seconds at 480p with the default 4 steps (0.1.38), over 15 at
  7 steps, longer for bigger or longer clips. The panel shows the expected time before you start, and each clip says
  how long it took to render.
- Videos can be scrubbed: both servers now answer byte-range requests for media files.

**Panels follow the loaded model** ([#23](https://github.com/thezolon/tiiny-studio/issues/23))

- **Chat's Thinking switch:** models that can't reason (Qwen3-4B-Instruct) no longer show it, models that always reason
  (gpt-oss) say "Always thinks", and the Qwen3.5/3.6 family keeps the on/off switch. Read from the model, for chat and
  the helpers, instead of guessed from its name.
- **Voice:** built-in voices to pick from (Qwen3-CustomVoice, MOSS-TTS-Nano), a box to describe a new voice
  (Qwen3-VoiceDesign), your cloned voices (Qwen3-VoiceClone), the style field only where it works, and the model's own
  languages. Before, every voice model was treated as a cloning one.
- **Songs:** **YuE2-3B** works now. Each model's own defaults and limits for steps, temperature, top-k, top-p and length
  (AceStep up to 8 minutes). **AceStep gets its own settings:** tempo, key, time signature, the language it sings in,
  and things to avoid. Preview and separate tracks only show for SongGeneration, the model that has them.
- **3D:** the loaded model's own detail levels (Hunyuan3D: octree 192/288/384; TRELLIS.2: 512 or 1024 px), sampling
  qualities, a new **Mesh quality** setting, and texture/simplify only when the model has those steps, with each model's
  own timings. The cut-out prefers an image model that writes a real alpha channel (Qwen-Image).
- **Supertonic-3 speaks over Wi-Fi/LAN.** TiinyOS's gateway fails it (it sends the model a name it doesn't know); the
  studio now asks the model's own port directly. Over USB it still can't work, and the Voice panel says so.
- **A "needs Wi-Fi" banner** where a model only works over Wi-Fi/LAN and you're on USB (Supertonic-3, GLM-OCR). Text
  mode reads with PP-OCRv6 instead of failing when GLM-OCR is picked over USB.
- PP-DocLayoutV3 is no longer offered in Text mode: it finds a page's layout but doesn't read text.

**Queue and jobs** (overnight queues requested on the Tiiny Discord)

- **The jobs menu shows when each job should start and finish**, and when the whole queue will be done. **Remove all
  waiting** clears what hasn't started.
- **The desktop app keeps the computer awake** while jobs are queued or running (the screen can still turn off).
- **A dropped connection doesn't end a video or 3D job:** it keeps trying for about 5 minutes.
- **Results left on the Tiiny are saved automatically:** videos and songs/loops from the last day that never reached your
  gallery (the studio was closed mid-render, or another studio made them) are pulled in when you open the studio. The
  first time only takes stock, so old results are never imported. 3D can't be recovered: the device can't list it.
  Two studios sharing one Tiiny each pull the other's recent results.
- **Deleting a song or loop also deletes the Tiiny's copy.**
- **The Tiiny's free space is watched:** the Models panel shows it, and a job won't start with under 2 GB free (it
  would fail halfway).
- A chat where the model spends all its tokens thinking and gives no answer (seen once with gpt-oss-20b) now says so,
  instead of showing an empty reply.

**Device notes**

- A voice model that fails inside TiinyOS now says so, instead of "The upstream TTS model failed" (on 0.1.38:
  Supertonic-3 over USB, and cloning with MOSS-TTS-Nano; reported to Tiiny).
- Tested on TiinyOS 0.1.38.

## 0.3.2 (2026-10-29)

3D with TRELLIS.2-4B.

- **3D works with TRELLIS.2-4B** ([#20](https://github.com/thezolon/tiiny-studio/issues/20), reported on the Tiiny Discord), not only Hunyuan3D. It failed
  with "simplify: Missing stage input 'shape_artifact'": the studio sent every model Hunyuan3D's input names. It now
  asks the loaded 3D model which stages, inputs and settings it takes, and fetches the results TRELLIS keeps on the
  device. A shape-only build with TRELLIS uses its preview model.
- **3D with two 3D models loaded** builds with the one you picked: the device sent an unnamed request to either one.
- **A failed or cancelled 3D build leaves no files behind.** It kept its copy of the source picture (and, since 0.3.0,
  the cut-out) without a gallery entry to delete them by.

## 0.3.1 (2026-10-29)

Clearer messages found while testing 0.3.0 by hand.

- **A wrong API key now says so.** The Models panel, the Image notice and the model cards showed "Device offline" or
  "not installed" when the Tiiny was answering but refused the key; they now say the key was rejected, and where to
  copy the right one (it's easy to grab the device ID instead), with a button to Settings.
- **In Docker, a USB address in `172.17.x.x` is explained**: Test connection and Settings say it's Docker's own network
  and what to do (Wi-Fi address, the desktop app, or move Docker's network).
- **"Load a chat model" opens the Models panel at the chat models.** The panel lists the current workspace's models
  first, but Chat and 3D never matched, so it always started with Image.

## 0.3.0 (2026-10-29)

Two requests from the Tiiny Discord and a cleaner 3D, all live-tested on TiinyOS 0.1.37.

- **Create from references** ([#8](https://github.com/thezolon/tiiny-studio/issues/8), requested on the Tiiny Discord): with an image model that can
  edit (Qwen-Image), Create takes up to 10 reference pictures and makes a brand-new picture from them, in the shape
  you pick. Call them "image 1", "image 2" in the prompt. The gallery marks these REFS, and Reuse brings the
  references back. Create and Edit keep their own pictures.
- **Helpers swap models for you** ([#5](https://github.com/thezolon/tiiny-studio/issues/5), requested on the Tiiny Discord): Improve prompt, Describe,
  Write lyrics and loop tags no longer ask you to load a chat model first. The studio unloads what's in the way, runs
  the helper, and loads your models back (about 30 s with Qwen-Image loaded). Settings → Model swapping has a global
  switch and one each for Chat tools, Image helpers and Audio helpers, all on by default; off, nothing is loaded or
  unloaded for you. You can also pick which text model the helpers use.
- **3D: Cut out the subject first** ([#6](https://github.com/thezolon/tiiny-studio/issues/6)), on by default:
  Qwen-Image puts the subject on a transparent background before Hunyuan3D builds it, so the background no longer
  ends up in the model (about 1.5 min more). The studio loads Qwen-Image if needed (Settings → Model swapping → 3D)
  and skips the step for a picture that's already transparent. The 3D view shows the cut-out it was built from.
  Hunyuan3D's own background step is still broken on TiinyOS 0.1.37.
- **Power off works again** with TiinyOS 0.1.37 (it didn't on 0.1.36); the known-limits note now says so.

## 0.2.1 (2026-10-28)

Fixes from the first day of use, and screenshots in the README.

- **Songs with AceStep** (new in TiinyOS 0.1.36) work: the device has no preview for AceStep, so the studio now asks
  for the whole song. A "preview first" request makes an 8-second song.
- **Cancel works while a 3D model or audio is being made.** The view redrew every 1.5 s and could swallow the click.
- **A model load the Tiiny gives up on** shows an error after about 30 s instead of spinning for 6 minutes.
- No more stray "null" text in the 3D and Song panels.
- **Test connection spots a relay or proxy** (an OpenAI-compatible chat gateway answers like a server but has no Tiiny
  model controls) and says to use the Tiiny's own address, instead of reporting "Connected".
- Known: **Power off** doesn't work on the latest TiinyOS; Tiiny says it's a firmware issue fixed in a coming patch.
- README: screenshots, and a tip for 3D (cut the subject out first: Hunyuan3D builds the background too).

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
