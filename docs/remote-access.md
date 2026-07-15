---
audience: humans, contributors
stability: evolving
last-reviewed: 2026-07-15
---

# Remote access over an overlay network

**TL;DR.** Reach a self-hosted phux server from another network by putting both
ends on a WireGuard-class overlay, minting credentials with phux pair, and
attaching to the overlay address over QUIC or TLS WebSocket. Covers Tailscale,
Headscale, and raw WireGuard end to end, plus troubleshooting for routing,
auth, and fingerprint failures.

---

## Why an overlay

phux already ships everything a remote attach needs except reachability: wss://
(TLS 1.3) and QUIC transports, `phux pair` to mint a bearer token plus a
certificate fingerprint, and a non-loopback bind that engages TLS and token
auth automatically
([ADR-0031](../ADR/0031-remote-consumer-auth-and-encryption.md)). What remains
is purely packet reachability — a self-hosted server behind NAT or CGNAT has no
inbound-reachable address. The sanctioned answer is a WireGuard-class overlay
network ([ADR-0037](../ADR/0037-overlay-network-reachability.md)): an L3
substrate that hands the client a routable address (a `100.x` IP or a MagicDNS
`*.ts.net` name) which phux dials exactly like a LAN address, with zero new
code. Cert pinning is on the fingerprint, not the hostname, so overlay DNS
names work unchanged. phux is overlay-agnostic, and the fully-OSS
Headscale/WireGuard path is first-class, not a downgrade. Hosted relays,
rendezvous servers, and hole-punching are deliberately out of scope. The trust
model and environment knobs live in
[operations.md](./operations.md#connecting-from-another-network-overlay-reachability);
this page owns the step-by-step task.

## Common steps: pair, then listen

Every path below shares the same server-side setup, done once. Pair before
starting the listener: the server loads the token store at startup, so adding
or deleting a token takes effect after a restart. `phux pair` never contacts a
running server, and it provisions the self-signed certificate if none exists
yet, so the fingerprint it prints is the one the server will present.

```sh
# On the server host, before starting the listener:
phux pair
```

Its output looks like this (the overlay-address block appears only when a
tailnet or CGNAT-routed address is detected on the host):

```
Pairing token (a secret — give it to the device once):
  <64-hex token>

Server certificate SHA-256 (verify on the device to defeat MITM):
  <64-hex fingerprint>

Overlay network addresses (dial one of these from the device):
  100.x.y.z

Token written to <state-dir>/remote-tokens
```

Record the token and the fingerprint; every `phux attach` below uses both. The
fingerprint is SHA-256, 64 hex digits, optionally colon-separated. Then start
the listener on a non-loopback bind — TLS and token auth engage automatically:

```sh
phux server --listen 0.0.0.0:8787      # TLS WebSocket (= PHUX_WS_ADDR)
# or, for QUIC:
phux server --quic 0.0.0.0:8788        # (= PHUX_QUIC_ADDR)
```

Prefer QUIC where UDP is open — it handles roaming and connection migration
better. Use `--ws wss://` when UDP is blocked by a network or firewall.

## Path A: Tailscale

[Tailscale](https://tailscale.com) is the frictionless on-ramp.

1. Install Tailscale on both the server host and the client device.
2. Run `tailscale up` on each.
3. Confirm both peers appear in `tailscale status`.
4. Find the server's address: `tailscale status` prints both the `100.x.y.z`
   IP and the MagicDNS name (like `myhost.tailnet-name.ts.net`).

Then dial from the client:

```sh
# QUIC (preferred when UDP is open):
phux attach --quic myhost.tailnet-name.ts.net:8788 --token HEX --cert-fingerprint FP

# TLS WebSocket fallback (when UDP is blocked):
phux attach --ws wss://myhost.tailnet-name.ts.net:8787 --token HEX --cert-fingerprint FP
```

Routable hosts require `--cert-fingerprint` (only loopback trusts the dev
cert). The pin is fingerprint-based, so the MagicDNS name and the `100.x` IP
are interchangeable — no re-pairing when you switch between them. The honest
tradeoff: trust extends to Tailscale's coordination plane, mitigated by phux's
own TLS + token riding on top.

## Path B: Headscale

[Headscale](https://github.com/juanfont/headscale) is a self-hostable,
fully-OSS control plane for the same data plane, for operators who will not
depend on a third-party coordinator. The client tooling is identical.

1. Run a Headscale server.
2. Create a user and a preauth key:
   `headscale users create NAME`, then
   `headscale preauthkeys create --user NAME`.
3. Join each node:
   `tailscale up --login-server https://headscale.example.com --authkey KEY`.
4. Verify both peers with `tailscale status`.

Dial exactly as in Path A, using the Headscale-assigned `100.x` address (or
its DNS name if configured):

```sh
phux attach --quic 100.64.0.2:8788 --token HEX --cert-fingerprint FP
# or
phux attach --ws wss://100.64.0.2:8787 --token HEX --cert-fingerprint FP
```

## Path C: Raw WireGuard

A hand-rolled [WireGuard](https://www.wireguard.com) overlay works the same
way — all three paths look identical to phux, which only ever sees an IP.

1. Generate a keypair on both ends:
   `wg genkey | tee privatekey | wg pubkey > publickey`.
2. Write a minimal `/etc/wireguard/wg0.conf` on each end. Server side:

   ```ini
   [Interface]
   Address = 10.8.0.1/24
   ListenPort = 51820
   PrivateKey = <server privatekey>

   [Peer]
   PublicKey = <client publickey>
   AllowedIPs = 10.8.0.2/32
   ```

   Client side (the `Endpoint` goes on whichever side can see the other's
   public address):

   ```ini
   [Interface]
   Address = 10.8.0.2/24
   PrivateKey = <client privatekey>

   [Peer]
   PublicKey = <server publickey>
   AllowedIPs = 10.8.0.1/32
   Endpoint = server.example.com:51820
   PersistentKeepalive = 25
   ```

3. Bring the tunnel up on both ends: `wg-quick up wg0`.
4. Verify a recent handshake with `wg show`.

Dial the peer's tunnel address:

```sh
phux attach --quic 10.8.0.1:8788 --token HEX --cert-fingerprint FP
# or
phux attach --ws wss://10.8.0.1:8787 --token HEX --cert-fingerprint FP
```

With raw WireGuard there is no MagicDNS; use the tunnel IP or your own DNS.

## Troubleshooting

Failures fall into three classes, and the symptom tells you which one you have.

- **No route / connection timed out / connection refused.** An overlay
  problem, not a phux problem. Check `tailscale status` (both peers listed and
  not `offline`) and `tailscale ping <host>` on Tailscale/Headscale, or `wg
  show` for a recent handshake on raw WireGuard. Confirm the server binds an
  address the overlay routes (`0.0.0.0:8787` or the overlay IP itself) and
  that no host firewall drops the port. QUIC needs UDP end to end — if QUIC
  times out but wss:// works, UDP is blocked; stay on `--ws`.
- **Auth failure** (HTTP 401 / unauthorized on the WebSocket upgrade; QUIC
  token rejection). The link is fine; the bearer token is missing, mistyped,
  or not yet loaded. Mint one with `phux pair` and remember the server reads
  the token store only at startup — restart the listener after pairing. The
  401 is returned before any phux frame is read, so a 401 proves reachability.
- **Fingerprint mismatch.** The certificate the server presented does not
  match `--cert-fingerprint`. Either the pinned value is stale (the server
  state dir was recreated, regenerating `remote-cert.pem`), an operator
  certificate was substituted via `PHUX_WS_TLS_CERT`/`PHUX_WS_TLS_KEY`, or you
  are dialing the wrong host. Re-run `phux pair` on the server host — it
  re-prints the persisted certificate's fingerprint without contacting the
  running server — and compare. Do not "fix" a mismatch by dropping the flag:
  the pin is what closes the trust-on-first-use MITM window.
- **MagicDNS name does not resolve.** MagicDNS may be disabled on the tailnet,
  or the client OS resolver is not wired up; fall back to the `100.x` IP from
  `tailscale status`. The pin is on the fingerprint, not the hostname, so
  switching between name and IP needs no re-pairing.

Overlay links are higher-latency than a LAN; remote consumers get better
behavior by requesting state-sync output — see
[operations.md](./operations.md#output-mode-for-remote-consumers).

## Scope and alternatives

`ssh HOST phux stdio-bridge` remains a valid manual path where SSH is already
the trust boundary — no token or pin is involved on that transport. Hosted
relays, rendezvous servers, STUN/TURN, and reverse tunnels are deliberately
out of scope for the self-host repo; see
[ADR-0037](../ADR/0037-overlay-network-reachability.md). For the full attach
and pair CLI surface, see [the reference TUI](./consumers/tui.md).
