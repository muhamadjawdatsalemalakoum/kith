# Linking devices

Kith has no accounts. You connect two computers with a one-time **link code** — and from
then on they keep each other in sync automatically.

## How to link

On the computer you want to link **from**:

1. Open **Devices → Link a device**.
2. It shows a one-time code (and keeps a "waiting…" status).

On the **other** computer (also running Kith):

3. Open **Devices → Enter a code**.
4. Paste the code and click **Link devices**.

Within a few seconds both computers show each other under **Devices**, and your memory,
tabs, and files start syncing. Keep both apps open until the link completes.

## What actually happens

- The code is a short shared secret. Both sides run **SPAKE2** from it to derive an
  identical ephemeral key — without ever sending the code or key over the wire.
- A mutual, constant-time **key-confirmation** step proves both derived the same key
  (this is what defeats a wrong code or a man-in-the-middle). A wrong code yields
  different keys, the confirmation fails, and **nothing is handed over**.
- The host then sends the **group key** (the credential for your circle), encrypted under
  the confirmed ephemeral key. The joiner adopts it and restarts its engine to join.
- Pairing is **mutual**: the two devices exchange identities so each can reach the other —
  not just the joiner reaching the host.

The host answers exactly **one** pairing attempt per code, then disarms.

## After linking

- Both devices appear under **Devices**, where you can **rename** or **unlink** them.
- Linking is one-time; the devices reconnect on their own afterward.

> Note: in a **Personal** space, **Unlink** stops this device from syncing with that one;
> it does not by itself rotate the shared key, so use **Reset & re-key** if you want a
> removed device fully locked out. A **Team** space has real revocation — removing a device
> rotates an Admin-signed epoch key so it can't follow future changes. See [SECURITY.md](../SECURITY.md).
