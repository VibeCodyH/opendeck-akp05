# VSD N4 Pro (5548:1023) background protocol notes

Reverse-engineered 2026-07-03 while making the touchscreen strip seamless
(disassembly of the vendor SDK's `libtransport.so` + on-device probing).
Everything below was verified on real hardware unless marked otherwise.

## Command framing

HID output reports, `0x00` report id + 1024-byte packets (protocol v3).
Commands start with the `CRT` prefix; payload data packets are raw (no prefix).

```
prefix:  43 52 54 00 00                          "CRT"
BAT:     42 41 54 <size:4 BE> <key>              per-key JPEG image (existing path)
LOG:     4C 4F 47 <size:4 BE> 01                 persistent full-face background (JPEG)
STP:     53 54 50                                 commit/refresh
CLE:     43 4C 45 00 00 00 <key|FF>               clear key (FF = all)
```

## The face (LOG write)

- One 800x480 JPEG, **pre-rotated 180°** (firmware displays it rotated back).
- Sent as: LOG header packet, then the JPEG bytes in 1024-byte chunks, then STP.
- Paints the live screen immediately AND persists in flash: it survives power
  cycles and becomes the resting display after the stock boot animation.
- **It is a flash write**: the firmware blocks ~2s and *drops* commands that
  arrive during the write (observed: BAT images sent right after a LOG never
  painted). Sleep ~2.5s after LOG before sending anything else.
- Never write it per frame — flash wear. Only on background change, guarded by
  a content hash (`~/.config/opendeck-akp05/background/.face-logged`).
- The trailing `01` is a mode/slot flag, not a frame index: writing "frame 1"
  then "frame 2" just replaces the image (no boot slideshow).

## Face geometry (measured with on-device ruler patterns)

- Full face: 800x480. Key windows are direct crops; the top key row starts at
  face row 0.
- Touchscreen strip: rows ~368..480, all 800 columns, one continuous LCD.
- The four touch-zone windows reachable via BAT (protocol keys 1–4) sit at
  x = 0, 208, 416, 624, each 176 wide — so three ~32px slivers between windows
  are **unreachable by per-key writes**. Only the flashed face lights them.
  (This is why per-key strip tiling alone renders as "four boxes".)

## Dead ends (documented so nobody re-digs)

- `BGPIC` (`Transport::setBackgroundFrameStream` — positioned live background
  JPEG with x/y/w/h + FBlayer) and `BGCLE`: present in the vendor
  `libtransport.so` for this device family, but **no-ops on this firmware
  revision** (never painted in any variant: full-face, partial rect, layer 0/1,
  after BGCLE, freshly rebooted). The vendor's smooth Windows video backgrounds
  presumably rely on it on other firmware.
- The fluid ~30fps boot animation is a separate boot-only firmware asset; its
  upload mechanism is unknown (candidate follow-up: `QUCMD` / `DELED` / `DC`
  opcodes seen in the transport rodata).
- The older `Python-Linux-SDK` `libtransport.so` (`setBackgroundImgDualDevice`)
  uses the same LOG opcode with 1024-byte chunks — that's what led here.
