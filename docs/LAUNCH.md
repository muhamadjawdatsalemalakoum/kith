# Launch kit — Product Hunt

Everything you need to launch Kith. The product is built, tested, committed, and
documented; this is the turnkey checklist + copy so the launch is a few clicks plus
your accounts. (Achieving #1 is up to the launch — this gives you the best shot.)

---

## 0. Pre-launch checklist (do in order)

- [ ] **Capture screenshots** (the one thing only you can do from the running app) —
      see the shot list in §4. Save them to `www/screenshots/` and the PH gallery.
- [ ] **Code-sign** the builds — add the Apple/Windows secrets and uncomment the `env:`
      block in `.github/workflows/release.yml` (see [RELEASING.md](RELEASING.md)).
      Unsigned builds trigger SmartScreen/Gatekeeper warnings that tank installs.
- [ ] **Cut a release** — `git tag v0.1.0 && git push origin v0.1.0`, then review the
      draft release and **Publish** it (installers for Win/macOS/Linux attach
      automatically).
- [ ] **Deploy the landing page** — repo Settings → Pages → Source = "GitHub Actions",
      then run the "Deploy landing page" workflow. Confirm
      `https://muhamadjawdatsalemalakoum.github.io/kith/` loads.
- [ ] **Go public** — flip the repo to Public. Set the description + topics (below).
- [ ] **Smoke test** the published installer on a clean machine + link two devices.
- [ ] **Schedule the PH post** for **12:01 AM PT** (Product Hunt's day boundary; early
      gives a full day to accrue upvotes).

### Repo description + topics (Settings → About)
> Serverless, end-to-end-encrypted sync for your own devices — memory, tabs, and files. No account, no cloud. MCP-native.

Topics: `p2p` `end-to-end-encryption` `local-first` `crdt` `iroh` `mcp` `rust` `tauri`
`privacy` `file-transfer`

---

## 1. The listing

- **Name:** Kith
- **Tagline (≤60 chars) — pick one:**
  - `Your circle of devices, in sync — no account, no cloud`
  - `Private, serverless sync for your own devices`
  - `End-to-end-encrypted memory, tabs & files across your devices`
- **Topics:** Productivity, Privacy, Open Source, Developer Tools, Artificial Intelligence
- **Links:** Website (the Pages URL) · GitHub (repo) · Download (releases)

### Description (the "what is it")
> Kith keeps your own devices in sync — your notes, your saved tabs, and your files —
> directly device-to-device, end-to-end encrypted. There's no account to create and no
> server that can read your data, because there isn't one: devices link with a short
> code, and everything syncs over an encrypted peer-to-peer mesh. It's also MCP-native,
> so your AI assistant (Claude, Cursor) can use the very same memory, tabs, and files —
> locally. Free, open source, for Windows, macOS, and Linux.

---

## 2. Maker's first comment (post this immediately after launch)

> Hey Product Hunt 👋
>
> I built Kith because "sync across my devices" always meant handing my data to
> someone's cloud. Kith doesn't have a cloud. Your devices link with a short code and
> talk **directly**, end-to-end encrypted — there's literally no server that can read
> your notes, tabs, or files.
>
> Three things I'm proud of:
> - **No account, ever.** Pairing is a one-time code (SPAKE2), not a login.
> - **It's a platform, not one app.** Memory, tabs, and file transfer all run on one
>   tiny encrypted engine — and they're **MCP-native**, so your AI can read and add to
>   your own data, locally.
> - **Honestly serverless.** Built on iroh (QUIC) + Automerge (CRDTs). Both sync *and*
>   file transfer are gated by your shared key — a non-member can't fetch a byte.
>
> It's **alpha** and not yet independently audited — I'd genuinely love your eyes on it
> (it's fully open source, MIT/Apache-2.0). Happy to answer anything!

---

## 3. Reply templates (for the comment thread)

- **"Is it really serverless?"** Discovery uses the public mainline DHT and a relay is
  used only as a fallback when a direct connection can't be made — and the relay only
  ever forwards ciphertext it can't read. No data-bearing server we run.
- **"How is this different from Syncthing / Obsidian Sync / etc.?"** No account, and
  it's a small platform (memory + tabs + files) that's MCP-native for AI — not just file
  folders. CRDT state means conflict-free edits, not file-conflict copies.
- **"What about the AI angle?"** Run `kith serve` (it's the same app binary) and point
  Claude Desktop / Cursor at it; your assistant gets `memory.*`, `tabs.*`, `files.*`
  tools over MCP, operating on your local data with no server in between.
- **"Is my data safe?"** End-to-end encrypted in transit (QUIC/TLS 1.3) and encrypted
  at rest; access is gated by a shared group key. It's **alpha** and not independently
  audited yet — see SECURITY.md; don't trust it with irreplaceable data just yet.
- **"Windows/Mac/Linux?"** All three. Download links are on the site/releases.

---

## 4. Gallery shot list (capture from the running app — `cargo run -p kith`)

Order matters; the first image is the thumbnail. Aim for 1280×800, dark theme.

1. **Hero/brand** — `www/og.png` (already in the repo) or the Memory view full-window.
2. **Memory** — the notes list populated, composer visible.
3. **Files** — a transfer mid-download showing the progress bar + "direct" badge.
4. **Devices** — the "Link a device" modal with the one-time code.
5. **Agents** — the MCP setup screen (config + the tool list).
6. **About** — brand + "no account, no cloud" framing.

> Tip: the in-app empty/onboarding states and the brand assets in `assets/brand/` are
> also good source material.

---

## 5. Distribution (your channels — optional but high-leverage)

- Post from your own **LinkedIn** (https://www.linkedin.com/in/akoum/) and any dev
  communities you're in, linking the PH page (not the repo) on launch morning.
- A short demo GIF (link two devices → a note appears on both) outperforms static
  shots — record one if you can.

---

*The build is done and green. This kit + the human steps in §0 are the launch.*
