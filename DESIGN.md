---
audience: contributors, agents
stability: stable
last-reviewed: 2026-06-23
---
# phux Design System

**TL;DR.** phux should feel like a precise terminal instrument: quiet, dense
when useful, and unmistakably wire-native. The signature is electric lime used
as a restrained signal color across dark technical surfaces.

## 1. Atmosphere & Identity

phux is a command surface for people and agents sharing the same live terminal
object. It should feel sharper than tmux, calmer than a dashboard, and more
material than a raw protocol spec. The visual signature is the "wire object":
thin terminal-grid geometry with a single lime path showing that panes, agents,
and clients are holding the same object rather than copying a screen.

## 2. Color

### Palette

| Role | Token | Light | Dark | Usage |
|------|-------|-------|------|-------|
| Surface/primary | `--surface-primary` | `#f8fafc` | `#090b0f` | Documentation page background, outer terminal field |
| Surface/secondary | `--surface-secondary` | `#eef2f7` | `#11141b` | Terminal panes, README demo field |
| Surface/elevated | `--surface-elevated` | `#ffffff` | `#171b23` | Modals, prompt overlays, callouts |
| Text/primary | `--text-primary` | `#0f172a` | `#f4f7fb` | Headlines, status titles, foreground text |
| Text/secondary | `--text-secondary` | `#475569` | `#9aa4b2` | Body copy, inactive pane labels |
| Text/tertiary | `--text-tertiary` | `#64748b` | `#697386` | Muted hints, disabled controls |
| Border/default | `--border-default` | `#cbd5e1` | `#343a46` | Pane dividers, modal borders |
| Border/subtle | `--border-subtle` | `#e2e8f0` | `#242936` | Secondary separators |
| Accent/primary | `--accent-primary` | `#65a30d` | `#bef264` | Default accent, active wire, modal titles |
| Accent/secondary | `--accent-secondary` | `#15803d` | `#86efac` | Key chords, secondary active states |
| Status/error | `--status-error` | `#dc2626` | `#f87171` | Errors, destructive messages |
| Status/warning | `--status-warning` | `#ca8a04` | `#fde047` | Warnings, section headers |

### Rules

- Lime is a signal, not wallpaper. Use it for active objects, command focus,
  agent events, and the README wordmark path.
- Prefer off-black technical surfaces over pure black.
- Keep screenshots and demo assets legible when downscaled to README width.

## 3. Typography

### Scale

| Level | Size | Weight | Line Height | Tracking | Usage |
|-------|------|--------|-------------|----------|-------|
| Display | 48px | 700 | 1.05 | 0 | Wordmark, large launch visuals |
| H1 | 36px | 700 | 1.15 | 0 | Page title |
| H2 | 28px | 650 | 1.25 | 0 | Section headers |
| H3 | 20px | 650 | 1.35 | 0 | Panel titles |
| Body | 16px | 400 | 1.6 | 0 | Documentation prose |
| Body/sm | 14px | 400 | 1.5 | 0 | Captions, status text |
| Mono/sm | 13px | 500 | 1.45 | 0 | Commands, pane labels, JSON |

### Font Stack

- Primary: system sans-serif (`ui-sans-serif`, `system-ui`, `-apple-system`)
- Mono: system monospace (`ui-monospace`, `SFMono-Regular`, `Menlo`, `monospace`)

### Rules

- Terminal and protocol surfaces may lean mono-heavy; prose should stay calm
  and readable.
- Letter spacing is zero unless a real terminal glyph grid requires otherwise.

## 4. Spacing & Layout

### Base Unit

All spacing derives from a base of 4px.

| Token | Value | Usage |
|-------|-------|-------|
| `--space-1` | 4px | Icon-to-label, hairline offsets |
| `--space-2` | 8px | Compact terminal chrome |
| `--space-3` | 12px | Status groups, modal inner gaps |
| `--space-4` | 16px | Default panel padding |
| `--space-6` | 24px | README asset padding |
| `--space-8` | 32px | Section grouping |
| `--space-12` | 48px | Major front-door rhythm |

### Grid

- Max content width: 1120px for docs and launch assets.
- Terminal surfaces use stable cell grids; avoid layouts that resize around
  dynamic command text.

### Rules

- Use full-width bands or single composed surfaces for launch visuals.
- Do not nest decorative cards inside other cards.

## 5. Components

### Terminal Demo Surface

- **Structure**: dark terminal frame, single status strip, pane grid, command
  transcript, and one accent wire/path.
- **Variants**: static README image, animated GIF, TUI smoke capture.
- **Spacing**: `--space-4` inside the frame, `--space-2` around pane chrome.
- **States**: active pane has lime title/path; inactive panes use secondary
  text and default borders.
- **Accessibility**: alt text must describe the product behavior, not the
  decoration.

### Wordmark

- **Structure**: mono wordmark plus one wire-object mark.
- **Variants**: SVG source, PNG export for surfaces that do not render SVG.
- **Spacing**: clear space at least the height of the mark's inner node.
- **Accessibility**: `alt="phux"` when used as a brand mark.

## 6. Motion & Interaction

### Timing

| Type | Duration | Easing | Usage |
|------|----------|--------|-------|
| Micro | 120ms | ease-out | Button or focus state |
| Standard | 240ms | ease-in-out | Overlay open/close |
| Demo beat | 800-1400ms | linear or ease-in-out | README GIF command/event reveal |

### Rules

- Animate opacity and transform only in browser-facing assets.
- Terminal demo animation should be readable first, kinetic second.
- Respect reduced-motion in future web surfaces.

## 7. Depth & Surface

### Strategy

Use tonal shift plus 1px borders. Shadows are reserved for modal overlays and
should be subtle enough to disappear in a terminal screenshot.

| Type | Value | Usage |
|------|-------|-------|
| Default border | `1px solid var(--border-default)` | Panes, demo frame |
| Subtle border | `1px solid var(--border-subtle)` | Internal separators |
| Overlay shadow | `0 16px 48px rgba(0,0,0,0.28)` | Help/prompt overlays |
