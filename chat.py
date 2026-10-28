"""Chat with the loaded model, which can also use the studio: create or edit images, voice text, make
loops and songs. When a tool needs a model that doesn't fit next to the chat model, the chat model is
unloaded, the tool's model loaded, the work done, and the chat model loaded back before the reply goes on.

Runs as a "chat" job in the shared device queue (it holds the device lock for the whole turn, so the
tools run inline). The reply streams into job["partial"], which the page polls."""
import base64
import json
import re
import time
import urllib.error
import urllib.parse
import urllib.request

import media
import safety
from media import JobError

MAX_TOOL_ROUNDS = 4
LOAD_TIMEOUT = 420  # seconds; the biggest models take ~2 minutes to load

SYSTEM = ("You are the assistant in Tiiny Studio, a local media studio running on the user's own Tiiny AI device. "
          "Be direct and helpful. Use Markdown when it helps. When the user wants something made (an image, an edit "
          "of an image, speech, a music loop, a song), call the matching tool instead of describing it; write rich, "
          "specific prompts for it. Nothing is made unless you call a tool: never claim you made or changed something "
          "without calling the tool in this turn. The result appears in the chat and in the studio's gallery, so after the "
          "tool returns just tell the user briefly what you made. Making things can take a minute or more because models "
          "may be swapped on the device.")

TOOL_SPECS = {
    "create_image": {"description": "Create a new image from a detailed text-to-image prompt.",
                     "parameters": {"type": "object", "properties": {
                         "prompt": {"type": "string", "description": "Detailed visual description: subject, setting, style, lighting."},
                         "shape": {"type": "string", "enum": ["square", "portrait", "landscape", "wide"], "description": "Aspect ratio."}},
                         "required": ["prompt"]}},
    "edit_image": {"description": "Edit the most recent image in the conversation (attached by the user or made earlier) "
                                  "with a plain-language instruction.",
                   "parameters": {"type": "object", "properties": {
                       "instruction": {"type": "string", "description": "What to change, and what to keep the same."}},
                       "required": ["instruction"]}},
    "speak": {"description": "Read text aloud in the user's saved voice and return the audio.",
              "parameters": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}},
    "make_loop": {"description": "Make a short instrument loop for music production.",
                  "parameters": {"type": "object", "properties": {
                      "tags": {"type": "string", "description": "Comma-separated tags: one instrument first, then character, phrase and effect tags."},
                      "bpm": {"type": "integer", "minimum": 100, "maximum": 150}},
                      "required": ["tags"]}},
    "make_song": {"description": "Make a short song preview (about 8 seconds) that the user can continue to a full song in the Audio workspace.",
                  "parameters": {"type": "object", "properties": {
                      "caption": {"type": "string", "description": "Genre, mood, instruments, voice."},
                      "lyrics": {"type": "string", "description": "Lyrics with [verse] and [chorus] tags on their own lines. Empty for an instrumental."}},
                      "required": ["caption"]}},
}
SHAPES = {"square": "1024x1024", "portrait": "768x1024", "landscape": "1024x768", "wide": "1024x576"}


class Sub(dict):
    """A tool's job record: progress messages show up on the chat job while the tool runs."""
    def __init__(self, parent, **kw):
        super().__init__(kw)
        self.parent = parent

    def __setitem__(self, k, v):
        super().__setitem__(k, v)
        if k in ("message", "progress"):
            self.parent["tool_" + k] = v

    def update(self, *a, **kw):
        for k, v in dict(*a, **kw).items():
            self[k] = v


# ---------- model juggling ----------
def running_ids(m):
    code, run = m.device("GET", "/api/v1/models/running", None, 15)
    return [r.get("model_id") for r in (run.get("instances") or {}).get("running", [])] if code == 200 else []


def wait_loaded(m, job, mid):
    t0 = time.time()
    while time.time() - t0 < LOAD_TIMEOUT:
        if job["cancel"]:
            raise JobError("Cancelled")
        if mid in running_ids(m):
            return
        time.sleep(3)
    raise JobError(f"{short(mid)} didn't finish loading")


def short(mid):
    return (mid or "").split("/")[-1]


def make_room(m, job, target):
    """Load target, unloading whatever the device says must go. Returns the models it unloaded."""
    if target in running_ids(m):
        return []
    unloaded = []
    for _ in range(4):  # one unload may not be enough
        code, plan = m.device("GET", f"/api/v1/models/{urllib.parse.quote(target, safe='')}/unload_candidate", None, 30)
        if code != 200:
            raise JobError(m.err(plan))
        if not plan.get("unload_required"):
            break
        for victim in plan.get("model_id") or []:
            job["message"] = f"Unloading {short(victim)} to make room"
            code, res = m.device("POST", f"/api/v1/models/{urllib.parse.quote(victim, safe='')}/stop", None, 120)
            if code != 200:
                raise JobError(m.err(res))
            unloaded.append(victim)
    job["message"] = f"Loading {short(target)}"
    code, res = m.device("POST", f"/api/v1/models/{urllib.parse.quote(target, safe='')}/launch", {"model_id": target}, 120)
    if code != 200:
        raise JobError(m.err(res))
    wait_loaded(m, job, target)
    return unloaded


def put_back(m, job, target, unloaded):
    """After a tool: if the chat model (and others) had to make room, unload the tool's model and reload them."""
    if not unloaded:
        return  # everything fitted: leave the tool's model loaded for next time
    job["message"] = f"Unloading {short(target)}"
    m.device("POST", f"/api/v1/models/{urllib.parse.quote(target, safe='')}/stop", None, 120)
    for mid in unloaded:
        job["message"] = f"Loading {short(mid)} back"
        code, res = m.device("POST", f"/api/v1/models/{urllib.parse.quote(mid, safe='')}/launch", {"model_id": mid}, 120)
        if code != 200:
            raise JobError(f"Couldn't load {short(mid)} back: {m.err(res)}")
    for mid in unloaded:
        wait_loaded(m, job, mid)


# ---------- tools ----------
def last_image(messages):
    for msg in reversed(messages):
        content = msg.get("content")
        if isinstance(content, list):
            for part in reversed(content):
                if part.get("type") == "image_url":
                    return part["image_url"]["url"]
    return None


def run_tool(m, job, req, name, args, messages):
    """Run one tool inline (we already hold the device lock); returns (gallery entry, text for the model)."""
    models, prefs = req.get("tool_models") or {}, req.get("prefs") or {}
    if name == "create_image":
        kind, model = "image", models.get("image")
        sub_req = {"model": model, "prompt": args.get("prompt", ""), "size": SHAPES.get(args.get("shape"), prefs.get("size") or "1024x1024")}
    elif name == "edit_image":
        # newest first: made earlier in this turn, then the latest in the conversation (the page sends it,
        # since images the assistant made aren't part of the messages), then the user's attachments
        src = job.get("turn_image") or req.get("last_image") or last_image(messages)
        if not src:
            return None, "There's no image in the conversation to edit. Ask the user to attach one."
        if src.startswith("/images/"):
            src = file_uri(m, src.split("/")[-1])
        kind, model = "image", models.get("edit")
        sub_req = {"model": model, "prompt": args.get("instruction", ""), "images": [src]}
    elif name == "speak":
        voice = req.get("voice")
        if not voice:
            return None, "No saved voice to speak with. Tell the user to save a voice in the Audio workspace first."
        kind, model = "speech", models.get("speech")
        sub_req = {"model": model, "text": args.get("text", ""), "ref_audio": voice["uri"], "ref_text": voice["text"], "voice_name": voice.get("name")}
    elif name == "make_loop":
        kind, model = "loop", models.get("loop")
        bpm = int(args.get("bpm") or 120)
        sub_req = {"model": model, "prompt": f"{args.get('tags', '')}, 8 bars, {bpm} BPM", "seconds_total": round(8 * 4 * 60 / bpm, 2), "bpm": bpm, "bars": 8}
    elif name == "make_song":
        kind, model = "song", models.get("song")
        lyrics = (args.get("lyrics") or "").strip()
        sub_req = {"model": model, "caption": args.get("caption", ""), "lyrics": lyrics, "generation_type": "mixed" if lyrics else "bgm", "preview": True}
    else:
        return None, f"Unknown tool {name}"
    if not model:
        return None, f"No model for {name.replace('_', ' ')} is installed on the device."
    job["tool"] = {"name": name, "model": model}
    unloaded = make_room(m, job, model)
    try:
        sub = Sub(job, id=media.uuid.uuid4().hex[:12], kind=kind, model=model, cancel=False, message="", progress=0, started=time.time())
        job["message"] = {"image": "Making the image", "speech": "Speaking", "loop": "Making the loop", "song": "Making the song"}[kind]
        if job["cancel"]:
            raise JobError("Cancelled")
        entry = media.KINDS[kind](m, sub, sub_req)
        entry.update(id=sub["id"], created=int(time.time()), kind=entry.get("kind", kind), seconds=round(time.time() - sub["started"], 1), via="chat")
        m.add_history(entry)
        if kind == "image" and entry.get("outputs"):
            job["turn_image"] = "/images/" + entry["outputs"][0]
    finally:
        job.pop("tool_message", None)  # so "Loading … back" shows instead of the tool's last step
        job.pop("tool_progress", None)
        put_back(m, job, model, unloaded)
    what = {"image": "image", "speech": "audio", "loop": "loop", "song": "song preview"}[kind]
    return entry, f"Done: the {what} is shown to the user and saved in the gallery (made with {short(model)})."


def file_uri(m, name):
    if not re.fullmatch(r"[\w.-]+", name) or not (m.dir / name).is_file():
        raise JobError("That image isn't in the gallery any more")
    ext = name.rsplit(".", 1)[-1].lower()
    return f"data:image/{'jpeg' if ext == 'jpg' else ext};base64," + base64.b64encode((m.dir / name).read_bytes()).decode()


# ---------- the turn ----------
def stream_completion(m, job, body):
    """POST a streaming chat completion; fill job['partial'] as it arrives. Returns (content, reasoning, tool_calls, finish)."""
    req = urllib.request.Request(f"{m.settings.url}/v1/chat/completions", json.dumps(body).encode(), method="POST")
    req.add_header("Content-Type", "application/json")
    for k, v in m.gateway_headers().items():
        req.add_header(k, v)
    content, reasoning, calls, finish = [], [], {}, None
    try:
        with urllib.request.urlopen(req, timeout=900) as r:
            for raw in r:
                if job["cancel"]:
                    break
                line = raw.decode("utf-8", "replace").strip()
                if not line.startswith("data:") or line == "data: [DONE]":
                    continue
                d = json.loads(line[5:])
                ch = (d.get("choices") or [{}])[0]
                delta = ch.get("delta") or {}
                if delta.get("reasoning_content"):
                    reasoning.append(delta["reasoning_content"])
                if delta.get("content"):
                    content.append(delta["content"])
                for tc in delta.get("tool_calls") or []:
                    slot = calls.setdefault(tc.get("index", 0), {"id": "", "name": "", "args": ""})
                    slot["id"] = tc.get("id") or slot["id"]
                    fn = tc.get("function") or {}
                    slot["name"] += fn.get("name") or ""
                    slot["args"] += fn.get("arguments") or ""
                finish = ch.get("finish_reason") or finish
                job["partial"]["content"] = job["partial"]["done"] + "".join(content)
                job["partial"]["reasoning"] = job["partial"]["done_reasoning"] + "".join(reasoning)
    except urllib.error.HTTPError as e:
        try:
            raise JobError(m.err(json.loads(e.read())))
        except ValueError:
            raise JobError(f"The device answered {e.code}")
    except (urllib.error.URLError, TimeoutError, OSError) as e:
        raise JobError(f"Device unreachable: {e}")
    return "".join(content), "".join(reasoning), [calls[i] for i in sorted(calls)], finish


def chat_model_id(m, wanted):
    """The real id behind 'default' (the device's alias for the loaded model), so it can be reloaded after a swap."""
    code, res = m.device("GET", "/v1/models", None, 15)
    for item in (res.get("data") or []) if code == 200 else []:
        if item.get("id") == wanted or wanted in (item.get("aliases") or []):
            return item["id"]
    if wanted and wanted != "default":
        return wanted
    raise JobError("No chat model is loaded. Load one from the Models panel.")


def run_chat(m, job, req):
    messages = [msg for msg in (req.get("messages") or []) if isinstance(msg, dict)]
    if not messages:
        raise JobError("Nothing to send")
    # attached documents are untrusted: delimited, screened, and flagged to the user (never silently dropped)
    rule = " " + safety.RULE if safety.has_documents(messages) else ""
    messages, warnings = safety.guard_messages(messages)
    if warnings:
        job["warnings"] = warnings
    model = chat_model_id(m, req.get("model") or "default")
    job["model"] = model
    tools = [{"type": "function", "function": {"name": n, **spec}} for n, spec in TOOL_SPECS.items() if n in (req.get("tools") or [])]
    convo = [{"role": "system", "content": SYSTEM + rule + (" " + req["system"].strip() if (req.get("system") or "").strip() else "")}] + messages
    job["partial"] = {"content": "", "reasoning": "", "done": "", "done_reasoning": ""}
    artifacts, usage_rounds = [], 0
    for usage_rounds in range(MAX_TOOL_ROUNDS + 1):
        body = {"model": model, "messages": convo, "stream": True, "max_tokens": int(req.get("max_tokens") or 4096)}
        if "gpt-oss" in model:
            body["reasoning_effort"] = "medium" if req.get("thinking") else "low"
        else:
            body["chat_template_kwargs"] = {"enable_thinking": bool(req.get("thinking"))}
        if tools and usage_rounds < MAX_TOOL_ROUNDS:
            body["tools"] = tools
        job["message"] = "Thinking" if req.get("thinking") else "Replying"
        content, reasoning, calls, finish = stream_completion(m, job, body)
        if job["cancel"]:
            raise JobError("Stopped")
        if not calls:
            break
        convo.append({"role": "assistant", "content": content or "",
                      "tool_calls": [{"id": c["id"] or f"call_{i}", "type": "function", "function": {"name": c["name"], "arguments": c["args"] or "{}"}}
                                     for i, c in enumerate(calls)]})
        for i, c in enumerate(calls):
            try:
                args = json.loads(c["args"] or "{}")
            except ValueError:
                args = {}
            try:
                entry, result = run_tool(m, job, req, c["name"], args, messages)
            except JobError as e:
                if job["cancel"]:
                    raise
                entry, result = None, f"That didn't work: {e}"
            if entry:
                artifacts.append({"kind": entry["kind"], "id": entry["id"], "outputs": entry.get("outputs", []),
                                  "prompt": args.get("prompt") or args.get("instruction") or args.get("text") or args.get("tags") or args.get("caption") or "",
                                  "duration": entry.get("duration"), "tool": c["name"]})
                job["artifacts"] = artifacts
            convo.append({"role": "tool", "tool_call_id": c["id"] or f"call_{i}", "content": result})
        # keep what was said before the tool call on screen while the reply goes on
        sep = "\n\n" if content.strip() else ""
        job["partial"]["done"] = job["partial"]["content"] + sep
        job["partial"]["done_reasoning"] = job["partial"]["reasoning"]
        job.pop("tool", None)
    text = job["partial"]["content"].strip()
    entry = {"kind": "chat", "params": {"model": model}, "sources": [], "outputs": [], "text": text,
             "reasoning": job["partial"]["reasoning"].strip(), "artifacts": artifacts, "finish": finish}
    if warnings:
        entry["warnings"] = warnings
    return entry


media.KINDS["chat"] = run_chat
