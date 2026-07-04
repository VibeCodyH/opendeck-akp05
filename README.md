![Plugin Icon](assets/icon.png)

# OpenDeck Ajazz AKP05 / Mirabox N4 Plugin — video background fork

A fork of [ambiso/opendeck-akp05](https://github.com/ambiso/opendeck-akp05) that adds
**full-deck video, GIF, and image backgrounds** — the feature the Windows vendor
software (VSD Craft / Mirabox) has and nothing on Linux did.

## The background layer

- One looping video (or GIF, or still image) plays across **all 10 LCD keys and the
  touchscreen strip** as a single continuous picture, bezel gaps accounted for.
- The touchscreen strip renders as **one seamless bar**, like the vendor software:
  the driver flashes the wallpaper as the device's persistent background (the same
  mechanism VSD Craft uses), which lights the strip edge-to-edge — including the
  slivers between its four touch zones that per-key writes can't reach. Bonus: the
  device shows your wallpaper at power-on, before the host even connects.
- Your key icons stay **on top** of the video and your buttons keep working exactly
  as before — icons are composited over the background with luminance keying
  (near-black pixels become transparent, so icons rendered on black float over the
  video the way the vendor software does it).
- Backgrounds **hot-reload**: change the video and the deck follows in ~2 seconds,
  no OpenDeck restart.
- On KDE Plasma, the deck can **follow your live video wallpaper** automatically —
  and on multi-monitor setups, **pin which monitor** it follows.
- No background configured → the driver behaves exactly like stock.

### Setup

1. Install this fork's build of the plugin (see Installation below — either install
   the release from this repo, or replace the `opendeck-akp05-linux` binary inside an
   existing install of the upstream plugin and restart OpenDeck).
2. Get the `tools/` directory from this repo. You'll need `python3` with
   [Pillow](https://pypi.org/project/pillow/) (`pip install pillow`), and `ffmpeg` on
   PATH for video/GIF sources.

### Usage

```sh
deck-bg set ~/Videos/some-loop.mp4     # use a video (or .gif / .png / .jpg / .webp)
deck-bg off                            # background off, plain icons restored
deck-bg status                         # what's currently deployed
deck-bg test                           # labelled diagnostic pattern (tile order,
                                       #   orientation, strip alignment)
```

Follow your KDE Plasma live video wallpaper (e.g. the
[Smart Video Wallpaper reborn](https://github.com/luisbocanegra/plasma-smart-video-wallpaper-reborn)
plugin) — whenever your desktop wallpaper changes, the deck follows within ~30s:

```sh
# one-time: install the follow daemon (adjust the repo path inside the unit first)
cp tools/deck-bg-sync.service ~/.config/systemd/user/
systemctl --user daemon-reload && systemctl --user enable --now deck-bg-sync.service

deck-bg follow                         # follow the primary monitor's wallpaper
deck-bg follow --monitor DP-3          # …or pin one monitor by connector name
deck-bg follow --monitor 1             # …or by Plasma screen index
```

With multiple monitors each screen can run a different wallpaper; `--monitor` pins
which one the deck mirrors (default: primary). Re-running `deck-bg follow` without
`--monitor` resets the pin to the primary monitor.

### Tuning

`deck-bg set` passes extra options through to the renderer:

```sh
deck-bg set video.mp4 --fps 5            # fewer frames/sec (default 10; lower it
                                         #   if the deck stutters)
deck-bg set video.mp4 --gap 0.35         # bezel gap between keys, fraction of key size
                                         #   (raise/lower until the picture lines up)
deck-bg set video.mp4 --opaque 4,9       # slots drawn as-is, no keying
                                         #   (0-4 top key row, 5-9 bottom, 10-13 strip)
deck-bg set video.mp4 --key-lo 40 --key-hi 80   # keying thresholds (0-255 luminance):
                                         #   below lo = video, above hi = icon. Raise
                                         #   them if icons leave faint boxes on the
                                         #   strip; lower if dark icon detail vanishes
deck-bg set video.mp4 --no-strip         # keys only, leave the touchscreen strip alone
deck-bg set video.mp4 --seconds 10       # only use the first N seconds
```

Renders are cached in `~/.cache/deck-bg/renders/` (a few MB per video), so switching
back to a background you've used before is instant.

### How it works

The renderer slices each frame of the source into per-key JPEG tiles laid out on a
virtual canvas of the deck's face (keys + bezel gaps + strip band) and writes them to
`~/.config/opendeck-akp05/background/`. The driver watches that directory's
`meta.json`; when it changes, the frames hot-reload. From then on the driver caches
every key image OpenDeck sends instead of writing it straight to the device, and a
frame clock composites icon-over-tile for the whole deck in sync each tick. Still
images paint once and go quiet instead of re-sending identical frames over USB.

The touchscreen strip is one continuous LCD, but the firmware only exposes four
176x112 windows of it (at x = 0/208/416/624 of an 800x480 face; rows 368-480) to
per-key writes — the ~32px slivers between windows are unreachable that way, which
is why naive strip tiling shows "four boxes". The fix, reverse-engineered from the
vendor SDK's `libtransport.so`: the renderer also emits the full 800x480 face
(`f_face.jpg`, frame 0, pre-rotated 180°), and the driver flashes it to the device
as the persistent background (`CRT LOG` write). The flashed face lights the slivers;
the four windows animate over it with zone tiles cropped in register. The flash
happens **only when the background changes** (a content-hash marker suppresses
repeats — it's a real flash write and takes ~2s, during which the firmware drops
incoming commands), and `deck-bg off` flashes black to clear it. Side effects: your
wallpaper doubles as the power-on image, replacing the stock VSD boot screen, and
per-frame strip animation pauses in the slivers (they hold frame 0).

The background layer is developed and tested on Linux; the rest of the driver is
unchanged from upstream.

---

# Upstream documentation

An unofficial plugin for Mirabox N4-family devices

## OpenDeck version

Requires OpenDeck 2.5.0 or newer

## Supported devices

- Mirabox N4E (6603:1007)
- Mirabox N4 (6602:1001)
- Mirabox N4 Pro E (5548:1021)
- Mirabox N4 Pro (5548:1008)
- Ajazz AKP05E (0300:3004)
- Ajazz AKP05E Pro (0300:3013)
- Ajazz AKP05 (0300:3006)
- VSDInside N4 Pro (5548:1023)
- Mars Gaming MSD-Pro (0B00:1003)
- Soomfon CN003 (1500:3002)
- Redragon SS552 (0200:3001)

## Platform support

- Linux: Guaranteed, if stuff breaks - I'll probably catch it before public release
- Mac: Zero effort, no tests before release, if stuff breaks - too bad, it's up to you to contribute fixes
- Windows: Zero effort, no tests before release, if stuff breaks - too bad, it's up to you to contribute fixes

## Installation

1. Download an archive from [releases](../../releases)
2. In OpenDeck: Plugins -> Install from file
3. Download [udev rules](./40-opendeck-akp05.rules) and install them by copying into `/etc/udev/rules.d/` and running `sudo udevadm control --reload-rules`
4. Unplug and plug again the device, restart OpenDeck

## Knob LED configuration

By default no LED commands are sent, so the device keeps its own built-in effect.

To configure the knob LEDs, create `~/.config/opendeck-akp05/leds.toml`.
(Windows: `%APPDATA%\opendeck-akp05\leds.toml`, macOS: `~/Library/Application Support/opendeck-akp05/leds.toml`)

All LEDs the same color:

```toml
brightness = 100 # 0-100

[mode.Static]
colors = [[255, 0, 128]] # RGB
```

Each LED a different color:

```toml
brightness = 100 # 0-100

[mode.Static]
colors = [
    [255, 0,   0  ], # RGB knob 1
    [0,   255, 0  ], # RGB knob 2
    [0,   0,   255], # RGB knob 3
    [255, 255, 0  ], # RGB knob 4
]
```

When OpenDeck is being terminated, a disconnect signal is sent to the device, which results in a hardcoded red for all knobs.

## Adding new devices

Read [this wiki page](https://github.com/4ndv/opendeck-akp03/wiki/Adding-support-for-new-devices) for more information.

## Building

### Prerequisites

You'll need:

- A Linux OS of some sort
- Rust 1.87 and up with `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-gnu` targets installed
- gcc with Windows support
- Docker
- [just](https://just.systems)

On Arch Linux:

```sh
sudo pacman -S just mingw-w64-gcc mingw-w64-binutils
```

Adding rust targets:

```sh
rustup target add x86_64-pc-windows-gnu
rustup target add x86_64-unknown-linux-gnu
```

### Preparing environment

```sh
$ just prepare
```

This will build docker image for macOS crosscompilation

### Building a release package

```sh
$ just package
```

## Acknowledgments

The video background layer was added in this fork; everything else is the work of
[ambiso](https://github.com/ambiso/opendeck-akp05) and the upstream contributors.

This plugin is heavily based on work by contributors of [elgato-streamdeck](https://github.com/streamduck-org/elgato-streamdeck) crate

Further inspiration was taken from these sister repos:
- https://github.com/naerschhersch/opendeck-akp05
- https://github.com/GrauBlitz/opendeck-akp05
- https://github.com/maillota/opendeck-akp05

The icon was yoinked from https://github.com/naerschhersch/opendeck-akp05/
