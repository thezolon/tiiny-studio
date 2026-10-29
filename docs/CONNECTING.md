# Connecting Tiiny Studio to your Tiiny

Tiiny Studio needs two things: your Tiiny's **network address** and its **API key**. You enter both
once in the studio's **Settings** (gear icon, top right), and **Test connection** checks them.

## 1. Find the API key

1. Open **TiinyOS** on your computer.
2. Click your **avatar** in the lower-left corner, then open **Settings → API Key**.
3. Next to **API Key** (shown masked, like `0b86••••••••`), click the **copy** icon.
4. Paste it into the **API key** field in Tiiny Studio.

> The same page lists **API Base URLs** such as `http://openai.api.tiiny/v1`. Don't use those in
> Tiiny Studio: `*.api.tiiny` names only resolve on a computer running TiinyOS (they point at a local
> proxy, `127.0.0.1`), and not inside Docker. TiinyOS's own *Use in Docker* note says the same: use
> the Tiiny's IP address instead.

## 2. Find the network address

### Option A: Wi-Fi or wired network (recommended; every feature works)

1. Put the Tiiny on your network. Wi-Fi is **off by default**: avatar → **Settings → Wi-Fi**, turn it
   on, pick your network and enter the password. For a wired network, plug a USB-C Ethernet adapter
   into any port **except** "To PC".
2. Read its IP address in TiinyOS: avatar → **Settings → Networks**. You can also check your
   router's list of connected devices. If you have several Tiinys, give each one a device name in
   TiinyOS so they're easy to tell apart there.
3. In Tiiny Studio, enter it as the device address, e.g. `http://192.168.1.50` (`http://` is added
   for you if you leave it out).

Tip: give the Tiiny a **DHCP reservation** in your router so its address doesn't change.

### Option B: the USB cable ("To PC" port)

The cable creates a tiny private network with just two addresses: your computer and the Tiiny.

- **Windows:** run `ipconfig` and find the adapter with subnet mask `255.255.255.252`. If your PC is
  `172.20.31.110`, the Tiiny is the other address, `172.20.31.109`.
- **macOS / Linux:** `ifconfig` or `ip addr`; look for the interface with a `/30` address.

Two limits apply over USB:

1. **Songs are limited.** Over the cable the Tiiny only exposes its gateway (port 80). Images, chat,
   transcription, voice cloning, 3D and music loops all go through it and work. Songs work through the
   gateway's preview route: shorter lengths are the *opening* of a full arrangement (they can end
   mid-phrase), 4:00 or more renders a complete song (~15 min), and "Separate tracks" returns only the
   mix. Over Wi-Fi every length is a complete song and separate tracks work.
   GLM-OCR doesn't work over the cable either (the gateway can't route it); PP-OCRv6 does.
2. **Docker may not reach it** on Windows or macOS. The USB address is somewhere in
   `172.16.x.x`–`172.31.x.x` (picked from the device's serial). If yours is `172.17.x.x`, that's also
   Docker's default internal network, so traffic from containers never reaches the cable. Either run
   the studio without Docker (see the README: `python server.py`, standard library only), or move
   Docker's default network: Docker Desktop → Settings → Docker Engine → add
   `"bip": "10.200.0.1/24"` and restart Docker (verified: after this, the studio in Docker reaches
   the Tiiny over USB). That second fix affects every container on the machine.

## Several ways to reach it (home, hotspot, USB)

Add each address as its own device under **Settings → + Add device** (the key is the same one from
TiinyOS). Put the one you prefer at the top with **↑**. With *Switch automatically* on, the studio
checks every 20 seconds and uses the first device in the list that answers, so moving from home
Wi-Fi to your phone's hotspot needs no changes. The top bar shows which one is in use, and a
message appears when it switches.

## Troubleshooting

**Keep TiinyOS open.** After the Tiiny powers on, its API answers `502 Bad Gateway` until the TiinyOS
desktop app connects, so open TiinyOS once after starting the Tiiny, or turn on TiinyOS's
**Auto Start** setting so it starts with your computer.

| Test connection says | What to do |
|---|---|
| *Unexpected response (HTTP 502)* | The Tiiny's services aren't running yet: open the TiinyOS app (or enable its Auto Start) and wait a few seconds. |
| *The device rejected the API key* | Copy the key again from Settings → API Key; make sure nothing extra was pasted. |
| *Device unreachable* / *timed out* | Check the address, that the Tiiny is on, and that your computer is on the same network. With Docker on the USB link, see Option B. |
| *Connected. 0 model(s) loaded* | It works; load a model from the studio's **Models** panel (`M`). |

When you change the device address, the studio asks for the key again. A saved key is only ever
sent to the device it was saved for.

## Powering the Tiiny off without TiinyOS

The studio's **Settings → Power off** does this for you. From any terminal (Linux, macOS, or
Windows with `curl`), it's one request with your API key:

```
curl -X POST -H "Authorization: Bearer YOUR_API_KEY" http://TIINY_ADDRESS/api/v1/sys/shutdown/
```

Keep the slash at the end. The reply is `{"status":"ok","message":"System shutdown will be initiated in
seconds via host API"}` and the device switches off a few seconds later. Press its power button to turn
it back on.

**On TiinyOS 0.1.36 this doesn't work yet:** the reply is the same, but the Tiiny stays on (a file is missing on the
device; Tiiny will fix it in a minor update). Until then, switch it off from the TiinyOS app or with its power button.

## What your Tiiny keeps

The studio saves your results only on your computer, but the Tiiny keeps its own copy of the chats its models
are sent. Since TiinyOS 0.1.36 (the 2026-10 update):

- **The vault stores every chat that goes through the gateway as full text**: system prompt, message and reply,
  from any app that uses the device's API (this studio's chats and chat helpers, Hermes, Cline, scripts) as well
  as TiinyOS itself. They're grouped into one "conversation day" per day for the vault's optional daily summary.
- **The memory scope setting** (`tiinyos` or `all`) only changes what's summarized; API chats are stored either way.
- **Without a summary model** set in TiinyOS nothing becomes memory, but the days are kept (status `queued`).
  TiinyOS doesn't show them.

You can delete the vault's copy of a day through the API. It removes the whole day, including that day's
TiinyOS chats in the vault (they stay in TiinyOS's own chat history), and can't be undone:

```bash
curl -H "Authorization: Bearer YOUR_KEY" "http://192.168.1.50/kb/conversation_days?limit=500"
curl -X DELETE -H "Authorization: Bearer YOUR_KEY" "http://192.168.1.50/kb/conversation_days/2026-10-26"
```

How this was found and what else the device keeps: [tiiny-api-notes](https://github.com/thezolon/tiiny-api-notes)
(`docs/PRIVACY.md`). This has been raised with Tiiny. If you want to be sure nothing leaves your network, block
the Tiiny's internet access in your router and allow it only for updates and model downloads.
