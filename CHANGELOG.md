# Changelog

## 0.6.6 (2026-11-21)

Songs can borrow the feel of a piece of music, Fedora gets its own package, and long chat replies are lighter on the
window.

- **Song style reference:** give a song 10-30 s of music and it borrows the feel (requested on the Tiiny Discord). It
  works with SongGeneration (at least 10 s) and AceStep (any length); YuE2 doesn't take one, so the field only shows
  for models that do. "Use as song style" on any audio result, songs included, sets it. Turned on now that the current
  TiinyOS handles it; it failed on older versions.
- **Fedora (.rpm):** a new `.rpm` package for Fedora and similar systems, x64 and ARM. It uses the system's own
  WebKitGTK, which can help if the AppImage's window goes white (reported on the Tiiny Discord). The README says what
  else to try in that case.
- **Long chat replies:** while a reply arrives, only its new part is redrawn instead of the whole reply, so a long
  answer full of tables keeps the window far less busy (reported on the Tiiny Discord, on Linux).
- Technical: each Linux release job now installs the `.rpm` in a Fedora container and checks every library resolves;
  the style reference's limits (SongGeneration refuses under 10 s; 10-120 s, mono or stereo, Wi-Fi and USB routes;
  AceStep 5-120 s) are in `docs/DEVICE-API.md`; a flaky MCP test waits for the other job instead of a fixed 200 ms.

## 0.6.5 (2026-11-21)

The studio fits big-scaling screens (a TV at 200%), and you can read the model's thinking while it works.

- **Settings at high display scaling:** on a TV at 200%, Settings ran off the bottom of the screen and OK couldn't be
  reached, so nothing could be saved (#124, reported on GitHub). Every dialog now fits the window and scrolls.
- **The top bar on narrow or scaled screens:** it pushed Models and the settings button off the right edge and cut off
  the right side of the studio (Save went missing). It now wraps onto a second row. The Audio intro scrolls instead of
  disappearing under the gallery.
- **Chat thinking:** while a model thinks, its thinking box no longer jumps back to the top on every update: it stays
  where you scrolled it and follows the newest thinking while you're at its end. If you open it to read, it stays open
  when the answer arrives.
- **Layout checks:** every change is now tested at four window sizes (a TV at 200%, a small window, a wide and short
  one, a laptop), so size bugs that were fixed stay fixed.

## 0.6.4 (2026-11-21)

The picture stays in the middle of the window, however many results the gallery holds.

- **Picture sliding off to the right:** once the gallery strip held lots of results, the stage stretched to the
  strip's full width, so the picture (and its Save and Edit buttons) drifted right with every new result. The strip
  now scrolls and the stage keeps the window's width, in every tab (reported on the Tiiny Discord).

## 0.6.3 (2026-11-20)

Speak works again with VoxCPM2, MOSS-TTS-Nano, OmniVoice and Qwen3-TTS CustomVoice.

- **Speak:** these voice models failed on ordinary sentences ("The upstream TTS model failed"): they need text that
  ends with a full stop (or ! ?) followed by a space, and "How are you." on its own didn't qualify. The studio now
  ends every piece of text that way; models that never had the problem sound exactly the same.

## 0.6.2 (2026-11-19)

Improve prompt works with Qwen-Image's prompt enhancer (and uses it), and long recordings transcribe again.

- **Improve prompt with Qwen-Image-2.1-PE-T2I:** it failed after ~10 s with "The model returned nothing". The prompt
  enhancer says it doesn't "think", yet it did, until its whole answer was used up. Helpers now tell every model not
  to think. And when the prompt enhancer is loaded, Improve prompt uses it: it's made for exactly that job (reported
  on the Tiiny Discord).
- **Long transcriptions:** anything longer than about a minute and a half failed ("Transcription failed: received 1000
  (OK)…"), because the Tiiny's speech-to-text can't take it in one go. Long recordings now go in parts of about a
  minute, cut at quiet moments, and the text is joined.

## 0.6.1 (2026-11-19)

Two fixes from your reports: the Mac app reaches the Tiiny again, and 3D models from transparent pictures lose their
phantom floor.

- **macOS: "Device unreachable" fixed.** macOS never gave the unsigned app its Local Network permission, so it couldn't
  reach the Tiiny (launching it from Terminal worked). The app now asks for local network access properly and is
  signed (ad-hoc), so the permission sticks; no Terminal commands needed after updating (#113, reported on GitHub and
  the Tiiny Discord, workaround found by a user).
- **3D: no more phantom floor.** A picture that was already transparent skipped the studio's clean-up, so faint leftover
  pixels turned into a big floor or a flat card. Every picture is now cleaned before 3D, and its subject is cropped and
  centred so it fills the frame (reported and requested on the Tiiny Discord).

## 0.6.0 (2026-11-18)

Agents can now use your Tiiny through the studio, the queue works across tabs, long jobs explain themselves, and every
result is easy to find on disk.

- **Agents (MCP):** a built-in MCP server lets Claude, Cline, Hermes or your own scripts make images, speech, music, 3D
  and video on your Tiiny through the studio, with its queue and gallery. Off by default: turn it on in Settings →
  Agents (MCP) and paste the address and token into your agent. An agent sees only what it made, never your gallery,
  chats or vault, and text it gets back (transcripts, documents) is marked as data and checked for prompt injection
  (#87, requested on the Tiiny Discord).
- **Queue jobs for any model:** in Audio, 3D and Video you can queue a job while its model isn't loaded ("Queue (loads
  Hunyuan3D)"). When its turn comes the studio loads the model, unloading others only if they don't fit, so you can
  line up a song, a 3D model and a video and walk away. Settings → Model swapping → "Queued jobs" turns it off (#78).
- **Find your results:** "Show in Explorer / Finder / folder" next to Download opens the folder with that file
  selected. New files are named by date and time (`2026-11-18_2215_…`) so they sort by when you made them, and images,
  MP3s and videos carry their prompt and settings inside the file (#58).
- **Long jobs:**
  - A job that fails stays in the jobs menu ("1 failed") and on its tab with the reason, until you dismiss it, instead of
    a toast that disappears (#80).
  - Video: a clip estimated past the Tiiny's 1-hour limit gets a warning and a suggestion that fits; a clip the Tiiny
    stopped at the limit offers to unload the video model, which may still be busy (#80).
  - Songs: if the long answer for a YuE2 or AceStep song never arrives (a dropped Wi-Fi connection), the studio notices
    the finished song on the Tiiny and saves it, instead of "composing" forever (#91, reported on the Tiiny Discord).
  - YuE2 time estimates are now measured: about 10 seconds per second of music, so a full song is ~45 minutes.
  - Results found on the Tiiny are announced before they're saved ("Found 1 song on the Tiiny…") (#80, requested on the
    Tiiny Discord).
- **Fixes:** typing in the chat keeps its focus when the model list refreshes (reported on the Tiiny Discord), and the
  Audio, 3D and Video panels keep your place, cursor and open Advanced section when they redraw.
- **Also:** a contributing guide (CONTRIBUTING.md), and a Sponsor button on the repository.

## 0.5.2 (2026-11-18)

Fixes for problems you reported: the Linux AppImage plays audio and video, the upload buttons work again, chat stops
jumping while you read, and two confusing errors now say what to do.

- **Linux AppImage:** the Audio tab no longer hangs, and audio and video play. The AppImage was missing the media
  plugins WebKit needs ("autoaudiosink not found"); they're inside now, and the .deb lists them as dependencies (#89,
  reported on the Tiiny Discord). Tested in CI only so far: if you're on Linux, please tell us whether it works for you.
- **Upload buttons:** picking a file with Speech's "Upload…", Transcribe's "click to browse" or an image slot loads
  it again; before, the choice could be silently lost (drag and drop always worked) (#95, reported on GitHub).
- **Chat:** scrolling up to read while a reply streams no longer jumps you back to the bottom (#94, reported on GitHub).
- **Image tab:** the preview and its Save button stay in view in a short or narrow window (#92, reported on the Tiiny
  Discord).
- **Video:** "Weight installation failed … UNINITIALIZED" now says what it means: the video model didn't fit next to
  the other loaded models, so unload them and try again (#90, reported on the Tiiny Discord).
- **Songs with YuE2:** only "With vocals" is offered, the one mix YuE2 can make; asking it for an instrumental says so
  instead of a raw device error (found with a user on the Tiiny Discord).
- **Version:** hover the gear icon (top right) to see which version you're running.
- **macOS:** the app is still unsigned, so macOS may say it "is damaged". It isn't: run
  `xattr -cr "/Applications/Tiiny Studio.app"` once in Terminal, then open it.

## 0.5.1 (2026-11-01)

Pick the model each tab uses, plus fixes from a day of real use: clearer messages during long jobs, drag and drop in
the desktop app, and a Models list that stays put.

- **Pick the model a tab uses:** when more than one installed model can do a tab's job (two image models, Hunyuan3D and
  TRELLIS.2, several voice or song models), the tab shows a card with the model in use. Click it for the list: pick a
  loaded model, or load or unload any of them with its own button. Nothing loads by itself, and each tab remembers its
  choice (#44).
- **Long 3D and video jobs:** while the Tiiny is too busy to answer, the studio now says "No answer from the Tiiny yet
  (it's usually just busy)" instead of "Can't reach the Tiiny". A job running past its estimate says "taking longer
  than usual" instead of a finish time that kept moving, and the jobs menu no longer shows a stray "null".
- **Desktop app:** dropping a picture from Explorer onto the studio works again (Windows).
- **Models list:** stays where you scrolled while it updates, instead of jumping back to the last thing you clicked.
- **Good to know:** the Tiiny stops a video after 1 hour (TiinyOS 0.1.38), and the render may keep the NPU busy
  afterwards; unloading the video model frees it. Keep long clips at 480p and 4 steps (7 s takes about 12 minutes).
  A warning in the studio is planned (#80).

## 0.5.0 (2026-10-31)

Cleanup and accessibility: the studio now works from the keyboard and with screen readers, explains a locked Tiiny,
tells you when a new version is out, and the code underneath was audited and tidied from end to end.

- **Accessibility:** the studio now follows WCAG 2.2 level AA (#54). Everything works from the keyboard (the workspace tabs take the arrow keys, thumbnails, voices and the seed are real buttons), dialogs and the Models drawer trap and restore focus, controls have names, jobs are announced to screen readers, both themes meet the contrast ratios, and animations stop when your system asks for reduced motion.
- **After a Tiiny restart:** the studio now says the Tiiny is locked and how to unlock it (open the TiinyOS app once, or
  run `tiiny login`), instead of "Can't reach the Tiiny", and notices the unlock within seconds (#46).
- **New version notice:** when a newer release is out, a small pill in the top bar says so; click it for the release notes and a link to the release page, or ✕ to hide it until the next version. The studio asks GitHub once a day (anonymous, nothing about you or your Tiiny); **Settings → Check for updates** turns it off, and it is off by default in Docker. Nothing is downloaded or installed for you (#45).
- **Video:** a clip with no step count now uses 4 steps (the app always did) instead of the device's 7, so a 2 s clip takes about 7 minutes rather than 12.
- **Model icons:** a model the Tiiny has no icon for (MiniMax-H3-Turbo) shows a plain chip tile instead of a broken image, and the studio stops asking the Tiiny for it on every redraw (#43).
- **Fixes:** chat no longer stays busy forever when the studio restarts mid-reply; Escape stops the OCR/document reader
  as well as closing it; image shortcuts no longer fire behind open dialogs; the jobs pill's NPU % keeps updating; the
  recording level meter moves again; "Delete everything" also clears Chat and Video; a failed voice delete says so (#53).
- **Error handling:** a job keeps talking to the device it started on (and sends only that device's key) even if the studio switches devices meanwhile; a result the disk can't hold is reported ("Couldn't save the result") instead of silently missing; a failed or cancelled job leaves no files behind; an answer that isn't JSON, a bad request body or one over 150 MB gets a clear error instead of a crash.
- **Docker:** the server never keeps root. It switches to the data folder's owner as before, and to an unprivileged user
  (uid 10001) when root owns the folder (a named volume, or one Docker created); if neither works it says so and stops.
  Rootless Docker/Podman is unchanged.
- **Release builds:** each job gets only the permissions it needs, the GitHub Actions, Tauri CLI and Docker base images
  are pinned to exact versions, and a release stops early if the tag, VERSION, Cargo.toml and tauri.conf.json disagree.
- **Under the hood:** a full code audit (#55): the web page's code is split into files with shared helpers for the
  Audio, 3D and Video panels; both servers share their helpers; the Rust code uses standard formatting; and every pull
  request now runs formatting, lint (clippy, ruff) and an accessibility check (axe-core) on top of the tests. The
  tests wait for what they need instead of sleeping, and `tools/device_watch.py` checks before each release whether
  Tiiny has fixed any of the device issues the studio works around.

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
