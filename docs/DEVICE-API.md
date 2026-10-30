# Tiiny device API reference

Everything the Tiiny Pocket answers on the network, in one place. Collected 2026-10-25 against
**TiinyOS 0.1.35** (device system API 0.1.0, knowledge base 2.0.0) from four sources, marked per endpoint:

| mark | meaning |
|---|---|
| **tested** | called and checked by Tiiny Studio or while building it |
| **spec** | in an OpenAPI spec the device publishes (exact paths and schemas) |
| **docs** | in Tiiny's developer docs (dev.tiiny.ai), with example requests |
| **app** | only seen in the TiinyOS desktop app's code; behaviour not checked |

After a TiinyOS update, run `python tools/api_snapshot.py` (or `--check` to only compare): it refetches
the device's specs, lists every added/removed/changed endpoint, regenerates
[api/gateway.md](api/gateway.md) and [api/kb.md](api/kb.md), and probes the read-only routes below.

## Services and ports

| port | service | reachable over | notes |
|---|---|---|---|
| **80** | gateway: routes to everything below | Wi-Fi/LAN and USB | the only port you normally need |
| 80 `/api/v1/sys`, `/api/v1/account`, `/api/v1/apps`, `/api/v1/webapp`, `/api/v1/chat/history` | system & account service (FastAPI) | " | **spec**: `/openapi.json`, `/docs` → [api/gateway.md](api/gateway.md) |
| 80 `/api/v1/models`, `/api/v1/npu`, `/v1/*`, `/api/tags`, `/api/chat` | model service (also on 8800 on the device) | " | no published spec; documented below |
| 80 `/kb/*` | knowledge base / vault ("SimpleMem") | " | **spec**: `/kb/openapi.json`, `/kb/docs` → [api/kb.md](api/kb.md) |
| 80 `/v1/config`, `/v1/<connector>/…`, `/v1/mcp/…` | on-device assistant: connectors and MCP | " | **docs** |
| 80 `/api/services` | app services (Tiiny App Farm) | " | **docs**/**app** |
| dynamic | each loaded model's own container | Wi-Fi/LAN only | port from `GET /api/v1/models/running`; no auth |
| 8800 | model service directly | on the device only | the docs use `:8800`; not reachable from the network |
| 5555 | upgrade/OTA service | on the device only | `/api/upgrade/*` |
| 39218 | discovery metadata `GET /device.json` | Wi-Fi/LAN | no auth; names, addresses, USB link details |
| 39217/udp | discovery broadcast (`GADGET_DISCOVER_V1`) | LAN | `_gadget._tcp` mDNS/DNS-SD too |
| 9109 | community NPU dashboard + `/metrics` (optional, not part of TiinyOS) | Wi-Fi/LAN | only if installed on the device |
| 3588 | open on the USB link only | USB | purpose unknown; not listed in device.json |

**Interactive docs:** only two services publish them: `/docs` and `/redoc` (system & account) and
`/kb/docs`, `/kb/redoc` (vault). Models/inference, the assistant's connectors and `/api/services` have no
docs or spec (`/v1/docs` 404); model containers vary (Foundation-1 has `/openapi.json`, the llama.cpp chat
container doesn't). The pages need the API key for "Try it out" calls.

**Auth:** `Authorization: Bearer <API key>` on port 80 (the key TiinyOS shows; the device accepts more
than one). The Anthropic-style route also accepts `x-api-key` (**tested**); no key → 401. Discovery (39218) and model containers
need no key. **Until the TiinyOS desktop app connects after the device boots, port 80 answers 502 on
every route** (TiinyOS "Auto Start" avoids this).

**Host names** (TiinyOS maps them to the device): `openai.api.tiiny`, `anthropic.api.tiiny`,
`ollama.api.tiiny`, `api.tiiny`. All of them are the same port 80 — the routes below work on the plain IP
too (**tested**).

## System and power

| method | path | what | mark |
|---|---|---|---|
| POST | `/api/v1/sys/shutdown/` | power off (empty body). Replies `{"status":"ok","message":"System shutdown will be initiated in seconds via D-Bus"}` (0.1.36 and earlier: `…via host API`); off within ~10 s. **Broken in TiinyOS 0.1.36** (same reply, the device stays on); **fixed in 0.1.37** (tested over USB and Wi-Fi, 2026-10-29) | **tested** |
| POST | `/api/v1/sys/reboot/` | restart | **spec** |
| GET / POST | `/api/v1/sys/ac_power/` | power on automatically when plugged in: GET → `{"status","ac_power": bool}`; POST `{"switch": 0\|1}` | **tested** (GET), **app** (POST) |
| GET | `/api/v1/sys/status` | live `cpu`, `memory`, `gpu`, `npus`, `disk` | **tested** |
| GET | `/api/v1/sys/performance` | request timings per endpoint: `summary`, `endpoints` (`limit`, `sort_by`, `include_stats_endpoint`) | **tested** |
| GET | `/api/v1/sys/device_info` | cpu, gpu, dnpu, ram, storage, network, versions, device name, settings | **tested** |
| POST | `/api/v1/sys/setting` | change a device setting | **spec** |
| POST | `/api/v1/sys/base_urls` | runtime base URLs | **spec** |
| GET | `/api/v1/sys/storage`, `/storage/llm`, `/storage/agent` | storage use | **spec** |
| DELETE | `/api/v1/sys/storage/agent/{agent_name}` | delete an agent's data | **spec** |
| GET/POST | `/api/v1/sys/wifi/wifi_list`, `wifi_connect`, `wifi_disconnect`, `connection_status`, `connection_switch`, `wifi_change`, `wifi_auto_connect` | Wi-Fi setup | **docs**/**app** — 404 on port 80 over Wi-Fi (2026-10-25); probably USB/first-setup only |
| GET | `/api/v1/npu/status` | NPU memory budget and utilisation (util only while a model is loaded) | **tested** |
| GET | `:39218/device.json` | discovery: device name, IPv4/IPv6, USB network (`usb.network`, `device_ip`, `host_ip`), metadata URLs | **tested** |
| POST | `:5555/api/upgrade/check`, `download_and_ota_prepare?target_version=`, `upgrade_to?target_version=prepared`, `pause_prepare`, `resume_prepare`, `reboot`; GET `status`, `status/v2` | firmware updates | **docs**/**app** (on-device only) |

## Models

All through port 80 (the model service). `{id}` is the URL-encoded model id, e.g. `Qwen%2FQwen-Image-2.1`.

| method | path | what | mark |
|---|---|---|---|
| GET | `/api/v1/models/?including_downloading=false` | installed models: `id`, `status` (running/stopped…), `type`, `input`, `supported` (e.g. `Image Editing`, `Tool Use`), `npu_usage` (% of NPU), `load_time`, `desc` | **tested** |
| GET | `/api/v1/models/running` | running instances, each container's `port` | **tested** |
| GET | `/api/v1/models/{id}/unload_candidate` | does it fit? `unload_required`, `model_id[]` to unload, `npu_required/available` | **tested** |
| POST | `/api/v1/models/{id}/launch` `{"model_id": id}` | load (async; poll `running`) | **tested** |
| POST | `/api/v1/models/{id}/launch/stream` | load with progress stream | **app** |
| POST | `/api/v1/models/{id}/stop` | unload | **tested** |
| POST | `/api/v1/models/unload_all` | unload everything | **tested** |
| POST | `/api/v1/models/{id}/download`, `/pause_download`, `/interrupt`; `/api/v1/models/interrupt_all` | downloads | **docs**/**app** |
| DELETE | `/api/v1/models/{id}` | delete a downloaded model | **docs** |
| POST | `/api/v1/models/import`, `/import/inspect`; GET `/import/toolkits` | import a custom model (checks toolkit support) | **docs**/**app** |
| GET | `/api/v1/models/types`, `/storage`, `/online_models`, `/api/v1/model-stats` | model types, disk use, store catalogue, stats | probed |
| GET (ws) | `/api/v1/models/running/ws`, `/api/v1/models/npu/status/ws` | live updates | **app** |
| GET | `/api/v1/statistics/week_usage?start_date=…`, `/app_week_usage` | usage stats | **app** (needs `start_date`) |

## Inference: OpenAI-compatible (`/v1`)

The **device handles one request at a time**; parallel requests to different models slow each other down badly.

| method | path | what | mark |
|---|---|---|---|
| GET | `/v1/models` | loaded models; each has `aliases` (`default` = the loaded chat model), `supports_vision`, `supports_chat`, `thinking`, `context_window` | **tested** |
| POST | `/v1/chat/completions` | OpenAI chat. Streaming (`reasoning_content` deltas when thinking), tools/tool calls, images as `image_url` data URIs. Thinking off: `chat_template_kwargs: {"enable_thinking": false}` (gpt-oss rejects it; use `reasoning_effort`). Model `default` = whatever is loaded | **tested** |
| GET | `/v1/capabilities?model=<id>` | what a loaded **chat or voice** model can do (0.1.38). Chat: `thinking` {`supported`, `toggleable`, `enabled_by_default`, `levels`, `default_level`}, vision, `input_modalities`, `supported`. Voice: `support_voice_clone`, `support_custom_voice` (built-in `supported_speakers`), `support_voice_design`, `support_instruct`, `support_speed`, `supported_languages`, `sample_rate`. Other models answer 400 "does not support tts"; a model that isn't loaded, 503. Music: `/v1/music/info?model=` (`config_schema`, `default_config`, per-task `defaults`); 3D: `/v1/3d/capabilities?model=`. Image, OCR and ASR models have no such route (catalog row and `desc` only) | **tested** |
| POST | `/v1/embeddings` | embeddings (load an embedding model, e.g. Qwen3-Embedding-0.6B) | **tested** |
| POST | `/v1/rerank` | `{model, query, documents[]}` (not in the OpenAI spec) | **docs** |
| POST | `/v1/images/generations` | create: `model, prompt, size "WxH", negative_prompt, seed, steps, guidance_scale, response_format`; **edit**: add `image` (one, base64/data URI) or `images` (array, up to 10) with an edit-capable model. `n` > 1 fails: one image per request. `/images/edits` and `/variations` don't exist | **tested** |
| POST | `/v1/image/generate/stream` | image generation with progress | **app** |
| POST | `/v1/audio/speech` | voice clone: `model, input, ref_audio` (base64 WAV), `ref_text` (both required), `language, instruct, response_format wav\|pcm\|mp3`. The Base model only clones | **tested** |
| POST | `/v1/audio/transcriptions` | multipart `model, file, language`; json/text only. Resample 48 kHz stereo to 16 kHz mono first | **tested** |
| POST | `/v1/ocr?model=<id>` | `{"input": data URI, "response_format": "json"}` → `data[0].text` + lines. PP-OCRv6 works; GLM-OCR isn't routed (use its own port's chat API) | **tested** |
| GET/POST | `/v1/music/info`, `/v1/music/sessions`, `/v1/music/generate[/wav\|/preview\|/resume]`, `/v1/music/sessions/{id}/download?format=mp3` | music (Foundation-1 loops via `/generate/wav` with `{model, prompt, config}`; SongGeneration via `/generate/preview` + `/resume`) | **tested** |
| GET/POST | `/v1/3d/capabilities`, `/v1/3d/tasks`, `/v1/3d/tasks/{id}`, `/v1/3d/tasks/{id}/cancel` | 3D models' stages: multipart `stage` (background, shape, simplify, texture, export), `params` JSON, files. Each model lists its stages, inputs and params at `/v1/3d/capabilities` (`api_schema: image-to-3d/v2`); see the 3D notes below | **tested** (Hunyuan3D-2.1, TRELLIS.2-4B) |
| GET | `/v1/video/info?model=<id>` | a loaded video model's options (0.1.38): `canvases` (the only sizes it takes: MiniMax-H3-Turbo 1344x768, 768x1344, 832x480, 480x832), `step_options` (4-8), `default_steps` (7), `fps` (24), `max_keyframes` (2), `audio` (32 kHz stereo). `/v1/video/capabilities` doesn't exist | **tested** |
| POST/GET/DELETE | `/v1/videos`, `/v1/videos/{id}`, `/v1/videos/{id}/content` | OpenAI-style video jobs (0.1.38). POST `{model, prompt, size "WxH", seconds "2", steps, seed, image}` (a start frame as a data URI) queues a clip and answers `{id, status: queued}`; only an empty `prompt` is refused there, anything else (a size the model wasn't made for) fails the clip a few seconds later with `error.message`. Poll `GET /v1/videos/{id}` (`status` queued → in_progress → completed/failed, `progress` 0-100, `frames`, `fps`; the length is rounded up to whole latent frames, 2 s → 56 frames). `GET …/content` is the MP4 (H.264 + AAC). **DELETE** removes it and also stops a render (there's no `/cancel`); otherwise clips stay listed at `GET /v1/videos` for an hour. Took 435 s for 2 s at 832x480, 4 steps | **tested** (MiniMax-H3-Turbo) |

## Inference: Anthropic- and Ollama-compatible

| method | path | what | mark |
|---|---|---|---|
| POST | `/v1/messages` | Anthropic Messages API (on `anthropic.api.tiiny` or the IP) | **docs** |
| GET | `/v1/models` | model list in Anthropic format | probed |
| POST | `/api/chat` | Ollama chat | **docs** |
| GET | `/api/tags` | Ollama model list (all installed models) | **tested** |

## Model containers (direct, Wi-Fi/LAN only, no auth)

Port from `GET /api/v1/models/running`; changes on every load. Blocked over USB.

| model | call |
|---|---|
| Qwen3-TTS (Base) | `POST :port/v1/synthesize {"text", "config": {"ref_audio", "ref_text", "language", "instruct"}}` → `audio_data` (24 kHz WAV) |
| Foundation-1 | `POST :port/v1/generate/wav {"prompt", "config": {seconds_total, steps, cfg_scale, seed}}` → WAV; has `/openapi.json` |
| SongGeneration-v2 | `POST :port/v1/generate {"config": {caption, lyrics, generation_type: mixed\|bgm\|vocal\|separate, seconds_total, seed, steps, temperature, top_k, top_p}}` → `audio_data` (48 kHz); only `config` at the top level |
| Hunyuan3D-2.1 | `POST :port/v1/tasks` (multipart, as `/v1/3d/tasks`); `GET :port/v1/tasks/{id}` |
| GLM-OCR | `POST :port/v1/chat/completions` with the image and the text `Text Recognition:` |

## Knowledge base / vault (`/kb`)

Full list (54 endpoints, from the device's own spec): [api/kb.md](api/kb.md). The vault service calls
itself "SimpleMem Knowledge Base Service" 2.0.0.

**Before you start:** files are only indexed while an **embedding model** is loaded (e.g.
`Qwen/Qwen3-Embedding-0.6B`, ~1% NPU), and a normal search (and the nightly summaries) want a **chat
model** loaded too; without one TiinyOS shows "model not loaded" alerts and searches stall.
`fast_mode: true` skips the chat model. The docs send `Host: kb.tiiny.local`; it isn't needed on the IP.
Same `Authorization: Bearer <API key>` as everything else.

### Files: upload → index → search → rename → download → delete (**tested** 2026-10-25)

| step | call | body | real response (trimmed) |
|---|---|---|---|
| upload | `POST /kb/upload` | multipart `file` (optional `finalize_now`, `source_id`, `timestamp`) | `{"success": true, "source_id": "file-8cb17251b7f4", "filename": "…", "chunks": 1, "vectorized": false, "finalized": false}` (0.1 s) |
| list | `GET /kb/sources?limit=500` | — | `{"count", "total", "limit", "offset", "items": [{"source_id", "filename", "source_type", "chunk_count", "finalized", "vectorize_status", "dream_status", …}]}` |
| index | `POST /kb/finalize` | `{"source_id": "file-…"}` | `{"success": true, "generated_entries": 1}` (0.9 s) |
| search | `POST /kb/retrieve` | `{"question": "…", "fast_mode": false, "include_memory_context": false}` | `{"success", "query", "count", "results": [{"entry_id", "lossless_restatement": "Source file: …\n\n<text>", "keywords", …}]}` (26 s with no chat model loaded) |
| rename | `PATCH /kb/sources/{source_id}` | `{"filename": "new name.txt"}` | `{"success": true, "item": {…}}` |
| download | `GET /kb/files/{source_id}` | — | the original file, byte for byte |
| delete | `POST /kb/delete-memory` | `{"source_id": "file-…"}` | `{"success": true, "deleted_entries": 1, "removed_chunks": 1, "deleted_source": true}`; gone from search too |

`include_memory_context` defaults to **true**: the search then also pulls in your personal memory
(profile, facts). Set it to false to search only files. `GET /kb/retrieve?question=…` works too, and
`POST /kb/query` takes the same body.

```bash
KEY=your-api-key; TIINY=http://192.168.1.50
curl -H "Authorization: Bearer $KEY" -F "file=@notes.txt" $TIINY/kb/upload
curl -H "Authorization: Bearer $KEY" "$TIINY/kb/sources?limit=500"
curl -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" -d '{"source_id":"file-…"}' $TIINY/kb/finalize
curl -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" -d '{"question":"…","include_memory_context":false}' $TIINY/kb/retrieve
```

### Conversations, daily summaries and memory (**spec**; read-only parts checked)

TiinyOS chats are stored in the vault as conversations and summarised ("dreamed") into memory entries
by a background worker, nightly 23:00–05:00 by default. All of it can be read and managed:

| method | path | what |
|---|---|---|
| POST | `/kb/upload_conversation` | add conversation text: `text`*, `time`* (`/stats` for counts) |
| POST | `/kb/add` | add a single memory: `speaker`*, `content`*, `timestamp`, `source_id`, `auto_finalize` |
| POST | `/kb/finalize_conversation` | summarise pending conversation text now |
| GET | `/kb/sources/conversations`, `/kb/sources/files` | conversation sources / file sources only |
| GET / DELETE | `/kb/conversation_days`, `/kb/conversation_days/{day}` | stored conversations per day |
| GET / PUT | `/kb/memory/schedule` | the nightly window (`nightly_start_hour`* …); GET also shows the summary worker's state (**tested**) |
| GET / PUT | `/kb/memory/model` | which model writes the summaries (`model_id`); GET → `{"model_id": "", "automatic_summary_enabled": true, "running_models": []}` (**tested**) |
| POST | `/kb/memory/process` | run memory processing now |
| GET / POST / PATCH / DELETE | `/kb/memory/facts`, `/kb/memory/facts/{fact_id}` | extracted facts (`fact_type`, `subject`, `predicate`, `object`, …), filterable (`status`, `fact_type`, `subject`, `q`) |
| GET / POST | `/kb/memory/proposals`, `/{id}/approve`, `/{id}/reject` | facts waiting for approval |
| POST | `/kb/context` | build the memory context the assistant uses: `query`*, `include_user_context`, `include_facts`, `include_vector_memories`, `max_tokens` |
| GET / PATCH | `/kb/user-context`, `/kb/user-context/profile/{field}`, `/kb/user-context/versions?limit=1` | your profile (occupation, preferences, more_about_you) |

**How the memory pipeline behaves** (read from the spec and the live data shapes, 2026-10-25; not
stress-tested):
- **Writers:** anything with the API key — files (`/kb/upload`), single memories with a free-form
  `speaker` (`/kb/add`), conversation text (`/kb/upload_conversation`, which TiinyOS chats feed as
  "conversation days"). There's no per-agent permission model in the API.
- **"Dreaming":** every source has `dream_status`, `dream_attempt_count`, `dream_deferred_until`;
  conversation days have `has_summary`, `summary_memory_worthy`, `summary_consumed`,
  `summary_refinement_status` (`queued`/`success`/`skipped`). A `conversation_summary` worker runs in the
  nightly window (default 23:00–05:00, `/kb/memory/schedule`) with the model from `/kb/memory/model`
  (empty `model_id` = automatic summaries off) and extracts **facts** (`subject`/`predicate`/`object`,
  `confidence`, `evidence`, `last_seen_at`, `created_by`, e.g. `curation_auto_approve`). Riskier ones
  become **proposals** (`risk_level`) waiting for approve/reject.
- **Eviction:** none found — no TTL, size cap or retention setting. Facts are replaced by newer facts
  (`supersedes_fact_id`, `status` other than `active`) rather than aged out; removal is explicit
  (`/kb/delete-memory`, `DELETE /kb/conversation_days/{day}`, `DELETE /kb/memory/facts/{id}`).
- **Memory scope (added in the 2026-10 TiinyOS update, kb 59 endpoints):** `GET/PUT /kb/memory/scope`, body
  `{"scope": "all" | "tiinyos"}`. The spec describes it as the *summary scope*: `tiinyos` "summarizes TiinyOS
  client chats only", `all` "summarizes every LLM output". The GET also reports `supported_scopes` and
  `origin_tagging` (true here).
- **Every chat sent to the gateway is captured** (tested 2026-10-27, device services 0.1.36), not only TiinyOS's own:
  on an idle device the day's `/kb/conversation_days/{day}` entry didn't change for 9 minutes; one chat sent
  straight to the gateway's `/v1/chat/completions` changed its `content_updated_at` 14 s later. Captures are batched
  (whole minutes). The API doesn't return the text (`journal_text` stays empty while queued; `/kb/files/{source_id}`
  404s for the day file) and TiinyOS doesn't show it. Without a summary model nothing becomes memory, but the days
  stay `queued`. Switching the scope to `tiinyos` doesn't stop the capture. `DELETE /kb/conversation_days/{day}`
  removes a day. Details: [tiiny-api-notes `docs/PRIVACY.md`](https://github.com/thezolon/tiiny-api-notes/blob/main/docs/PRIVACY.md).

## On-device assistant: connectors and MCP

From Tiiny's docs (**docs**), not tested here.

| method | path | what |
|---|---|---|
| GET | `/v1/{gmail\|outlook\|calendar\|outlook/calendar}/status`, `/auth-url` | OAuth email/calendar connectors |
| DELETE | `/v1/{connector}/connection` | disconnect |
| POST | `/v1/telegram/check-with-token`, `/v1/discord/check-with-token`, `/v2/x/check-with-token` | verify bot/API tokens |
| PATCH | `/v1/config/{telegram\|discord\|email\|x\|whatsapp}`, `/v1/config/channels/{…}` | save connector settings |
| POST | `/v1/email/imap/verify` | check IMAP settings |
| POST / GET | `/v1/whatsapp/qr`, `/v1/whatsapp/bridge/status`, `/bridge/logout` | WhatsApp bridge |
| GET / POST / DELETE | `/v1/mcp/servers`, `/v1/mcp/servers/import-json`, `/v1/mcp/servers/{id}`, `/{id}/check` | custom MCP servers |
| GET / POST | `/v1/mcp/tools`, `/v1/mcp/tools/call` | MCP tools (**app**) |
| GET | `/api/v1/chat/history/`, `/sessions`; DELETE `/{session_id}` | TiinyOS chat history (**spec**) |

## Account and pairing

`/api/v1/account/*` (auth, mail codes, passwords, recovery keys, device name, feedback),
`/api/v1/connect` (pair a client), `/api/v1/auth/init`, `/api/v1/auth/login`: see
[api/gateway.md](api/gateway.md) (**spec**) and the TiinyOS docs. The studio doesn't use these.

## Apps

`GET /api/v1/apps`, `/api/v1/apps/icons`; `GET /api/v1/webapp/`, `/running`, `POST /start`, `/stop` (**spec**);
`/api/services`, `/api/services/{id}`, `/download/{id}/status`, `/updates/check`, `/resource-usage/ws`
(**docs**/**app**): the app farm.

## Known quirks (TiinyOS 0.1.35)

- One request at a time; images are one per request (`n` > 1 fails).
- Port 80 returns 502 until the TiinyOS app has connected since boot.
- Over USB only port 80 (and 3588) is open: no model ports, so no GLM-OCR and no SongGeneration direct
  route (separate tracks, exact-length songs). The USB /30 is derived from the serial and can land in
  Docker's `172.17.x.x`.
- Hunyuan3D `background` stage fails (BriefNet missing; still on 0.1.37); SongGeneration style reference fails.
- **Song models** (tested 2026-10-30, 0.1.38): `/v1/music/info?model=` gives `service_name` (`SongGenerationTTMService`,
  `AceStepTTMService`, `YuE2TTMService`, `FoundationTTMService`), `default_config` (steps, temperature, top_k, top_p),
  per-task `defaults`, and `model_info.model` (`codec_llm_duration` = longest song in seconds: AceStep 480,
  SongGeneration 270, YuE2 0; `max_decoder_steps`: AceStep 20). The `config_schema` groups (`common`, `ace_step`, `yue2`)
  aren't model-specific: SongGeneration lists the `ace_step` group too. AceStep and YuE2 take only the plain
  `/v1/music/generate` ("… preview generation is not implemented"; YuE2: a 15 s request gave 23 s in 93 s). AceStep's own
  settings go **nested** in `config.ace_step` (`bpm` number, `key_scale` "G major", `time_signature` "4",
  `vocal_language`, `negative_prompt`); at the top of `config` they're refused ("Extra inputs are not permitted").
- **Voice models** (tested 2026-10-30, 0.1.38, `POST /v1/audio/speech`): a built-in speaker goes in `voice` (Qwen3-TTS
  CustomVoice: aiden, dylan, eric, ono_anna, ryan, serena, sohee, uncle_fu, vivian; an unknown name: `Unsupported
  speaker`), a voice description in `instruct` (VoiceDesign; required), a clone in `ref_audio` + `ref_text` (Base);
  `language` works for all. CustomVoice reports `support_instruct` but rejects `instruct` ("voice_design is not
  supported"). Supertonic-3 fails through the gateway whatever it's sent ("The upstream TTS model failed"), though its
  container answers directly over Wi-Fi when called by its own name `Supertonic-3`; it also refuses `speed` other than
  1.0 despite `support_speed`. MOSS-TTS-Nano speaks with `voice: default`; cloning fails (the gateway demands
  `ref_text`, the container refuses it).
- **Thinking** (tested 2026-10-30, 0.1.38): follow the model's `thinking` object, not its name. `toggleable` models
  (Qwen3.5/3.6) switch with `chat_template_kwargs.enable_thinking` (off: no reasoning, ~10x faster); `reasoning_effort`
  changes nothing for them. gpt-oss (`toggleable: false`, levels low/medium/high) reasons whatever it's sent and no longer
  rejects `enable_thinking`; neither `reasoning_effort` nor `chat_template_kwargs.reasoning_effort` changed its reasoning
  reliably. `supported: false` models (Qwen3-4B-Instruct) never reason.
- **3D models differ; read `/v1/3d/capabilities`** (tested 2026-10-29, TiinyOS 0.1.37). Its `inputs` name what each
  stage takes: Hunyuan3D-2.1 `simplify: [mesh]`, `texture: [image, mesh]`, `export: [material_artifact, mesh_glb]`;
  TRELLIS.2-4B `simplify: [shape_artifact]`, `texture: [shape_artifact]`,
  `export: [geometry_artifact, material_artifact, mesh_glb, shape_artifact]` (required: material, shape). The task
  form's fields are `image, mesh, material, file, shape_artifact, geometry_artifact`, so `material_artifact` goes in
  `material` and `mesh_glb` in `mesh`. A stage's output artifact of type `X` is the next stage's `X_artifact`.
  TRELLIS also lists `stage_specs` (each stage's params and controls; it rejects others) and returns its artifacts
  **by id, not inline**: fetch them from `GET /v1/3d/tasks/{task}/artifacts/{artifact_id}` (shape ≈ 40 MB zip,
  geometry/material ≈ 20 MB zips, the final GLB ≈ 7 MB). Its shape and simplify stages also offer a `*_preview` GLB.
  Timings (mug, draft/fast): shape 200 s, simplify 17 s, texture 98 s, export 43 s. TRELLIS's `background` stage
  fails like Hunyuan's (BriefNet missing).
  `output_precisions`: Hunyuan3D gives `octree_resolution` (192/288/384); TRELLIS.2 gives `pipeline_type` 512/512/1024 with
  `effective` (its draft is `standard`). Both list `inference_qualities` and `asset_qualities` (fast/balance/quality)
  and accept `asset_quality` (tested 2026-10-30, 0.1.38). FLUX.2-klein-4B-Background-Remove, as a cut-out, returns RGB on
  white (no alpha): TRELLIS.2 builds the subject from it, Hunyuan3D builds the white square.
  With two 3D models loaded, `/v1/3d/*` without a model goes to either one: add `?model=<id>` (URL-encoded) to every
  call (capabilities, tasks, polling, cancel, artifacts). Chained by reference, `input_refs` is a JSON form field:
  `{"shape_artifact": {"task_id": "…", "artifact_id": "…"}}` (a bare id is refused).
- Hunyuan3D `shape` reads the alpha channel and treats every pixel that isn't fully transparent as the subject: a
  cut-out whose background has faint alpha (1-2, typical of an image model's output) comes out as a flat card, and
  an opaque picture as a slab. The studio sets alpha < 32 to 0 before sending (tested 2026-10-29).
- Plain `/v1/music/generate` fails for SongGeneration ("Session not found"); `/resume` ignores `seconds_total`.
