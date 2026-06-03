---
audience: contributors
stability: stable
last-reviewed: 2026-06-03
---

# Recording the README demo

**TL;DR.** The README has a demo-shaped hole at the top and it's the most
important pixel on the page. This is the storyboard for filling it: two short
beats — *modern terminal content survives a detach/reattach*, then *the same
thing driven headless* — recorded as a GIF and dropped at
`docs/assets/demo.gif`. Use a real screen-recorder for the first beat
(asciinema players don't render kitty graphics, which is the whole point);
asciinema is fine for the second.

---

## The two beats

Keep it under ~15 seconds. The pitch is "look how little is going on, and yet."

**Beat 1 — it survives (≈8s).** In a graphics-capable terminal (Ghostty,
kitty, WezTerm), run `phux`, paint something a lesser multiplexer would mangle,
detach, reattach. The viewer should see the fancy content come back *intact*.

```sh
phux                                 # attach
# in the pane: paint a truecolor gradient + an inline image
bash docs/assets/payload.sh          # (see "the payload" below) — or any kitty +kitten icat IMAGE
# detach:
Ctrl-A d
# reattach — the gradient and the image are still there, pixel-for-pixel:
phux
```

**Beat 2 — an agent could've done that (≈6s).** Drop to a second terminal and
drive the *same* session without a TTY. This is the line that makes a skimmer
stop.

```sh
phux run . "cargo test --quiet"      # runs in the live pane, prints the exit code
phux watch --json . | jq .           # live events scrolling by; Ctrl-C to cut
```

Cut on the JSON events scrolling. No narration, no captions. Let the
juxtaposition do it.

## The payload

For Beat 1 you want something visibly *modern* so the "survives reattach" claim
is legible. A 256-step truecolor gradient is the cheapest unmistakable one:

```sh
# truecolor sweep — every column a distinct 24-bit color
awk 'BEGIN{for(i=0;i<256;i++)printf "\033[48;2;%d;%d;%dm ",i,(128+i)%256,255-i;print "\033[0m"}'
```

For the real flex, add an inline image with your terminal's image kitten
(`kitty +kitten icat logo.png`, or Ghostty/WezTerm's equivalent). The image is
what tmux literally cannot carry across a reattach; the gradient is what it
carries badly.

Park whatever you settle on at `docs/assets/payload.sh` so the recording is
reproducible and the next person doesn't reinvent it.

## Tools

- **GIF (Beat 1, required):** any screen-recorder that captures real pixels —
  [`vhs`](https://github.com/charmbracelet/vhs) if you want it scripted and
  deterministic, or a plain screen-capture-to-GIF. Kitty-graphics frames must
  survive into the GIF, so this can't be a terminal-only recorder.
- **asciinema (Beat 2, optional):** fine for the headless half if you'd rather
  splice. [`agg`](https://github.com/asciinema/agg) renders a `.cast` to GIF.
- Target **≤ 2 MB** so it loads before the reader scrolls past it. Trim
  generously; the demo is a hook, not a tutorial.

## Wiring it in

Drop the finished file at `docs/assets/demo.gif`, then replace the
`<!-- DEMO GOES HERE … -->` comment at the top of the root `README.md` with a
standard markdown image: `!` then `[alt text]` then `(docs/assets/demo.gif)`,
where the alt text is something like *phux: modern terminal content surviving a
detach/reattach, then driven headless*.

(The literal image syntax isn't shown inline here on purpose — the docs CI
resolves every relative link, and the GIF doesn't exist until you record it.)

That's the page going from half-built to built.
