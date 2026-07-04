#!/usr/bin/env python3
# Slice a video, GIF, or still image into per-key tiles for the deck video
# background layer of the opendeck-akp05 driver.
# Treats the deck face as one continuous canvas — keys grid + bezel gaps, plus
# the touchscreen strip band below — cover-fits the source onto it, then crops
# each key's rectangle out of every frame.
#
# The touchscreen strip is one continuous LCD exposed by the firmware as a
# persistent full-face background (800x480, the "LOG" write) plus 4 per-key
# windows (176x112 at x = 0/208/416/624, rows 368..480). The renderer
# therefore also emits:
#   f_face.jpg      frame-0 face, pre-rotated 180, for the driver to flash as
#                   the device background — it lights the strip (and the 32px
#                   gaps between the 4 windows) seamlessly
#   f_face_s0..3.jpg  static zone tiles cropped from the face band, so icon
#                   composites over a strip zone blend into the background
# Keys stay animated per frame; the strip band is static (frame 0) — the
# firmware has no flash-safe per-frame full-face write.
# Requires: Pillow; ffmpeg on PATH for video/GIF sources.
# Usage: python3 render_frames.py SOURCE [--fps 5] [--gap 0.28] [--out DIR]
import argparse, json, shutil, subprocess, tempfile
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

STILL_EXT = {'.png', '.jpg', '.jpeg', '.bmp', '.webp'}

p = argparse.ArgumentParser()
p.add_argument('source', nargs='?', help='video, GIF, or still image')
p.add_argument('--test', action='store_true',
               help='ignore source; render a labelled diagnostic pattern '
                    '(key tiles show r,c; strip shows a left→right gradient '
                    'with S0..S3 + an arrow) to verify tile order/orientation')
p.add_argument('--test-solid', action='store_true',
               help='with --test: fill the whole strip band solid white '
                    '(diagnoses whether the black gaps are per-tile masking, '
                    'a positioning gap, or render content)')
p.add_argument('--fps', type=float, default=10)
p.add_argument('--seconds', type=float, default=0, help='0 = whole video')
p.add_argument('--rows', type=int, default=2)
p.add_argument('--cols', type=int, default=5)
p.add_argument('--gap', type=float, default=0.28,
               help='bezel gap between keys, as a fraction of key size')
p.add_argument('--tile', type=int, default=112)
p.add_argument('--no-strip', dest='strip', action='store_false',
               help='skip the touchscreen strip band')
p.add_argument('--out', default=str(Path(__file__).resolve().parent / 'frames'))
p.add_argument('--key-lo', type=int, default=24,
               help='driver luminance keying: below this = fully video')
p.add_argument('--key-hi', type=int, default=64,
               help='driver luminance keying: above this = fully icon')
p.add_argument('--opaque', default='',
               help='comma-separated slots to draw icon as-is, no keying '
                    '(0-4 top key row, 5-9 bottom, 10-13 strip)')
a = p.parse_args()

T = a.tile
G = int(round(T * a.gap))
STRIP_TILES = 4
# Device face geometry (VSD N4 Pro, measured on-device): 800x480 framebuffer,
# strip band at rows 368..480 full width, zone windows 176 wide at 208 pitch.
FACE_W, FACE_H = 800, 480
BAND_Y, BAND_H = 368, 112
BAND_BLEED = 6                            # draw the band a few px early to
                                          # absorb panel-row slop at its top
ZONE_X, ZONE_W = (0, 208, 416, 624), 176
SW = round(FACE_W * T / 112)              # strip band width in canvas units
SH = round((BAND_H + BAND_BLEED) * T / 112)   # strip band height (with bleed)
WK = a.cols * T + (a.cols - 1) * G        # key grid width
HK = a.rows * T + (a.rows - 1) * G        # key grid height
W = max(WK, SW) if a.strip else WK
H = HK + (G + SH if a.strip else 0)
XK = (W - WK) // 2                        # key grid x-offset (centered)
XS = (W - SW) // 2                        # strip x-offset (centered)
YS = HK + G                               # strip y-offset

def _font(size):
    for name in ('DejaVuSans-Bold.ttf', 'DejaVuSans.ttf'):
        try:
            return ImageFont.truetype(name, size)
        except OSError:
            continue
    return ImageFont.load_default()


def test_canvas():
    """A single labelled frame the same size as a real render. Keys show
    'r,c' on a checker; the strip band shows a continuous left→right hue
    ramp with 'S0'..'S3' markers and a '>>>' arrow, so on-device you can
    read off tile order (left→right?), orientation (upright? mirrored?),
    and whether the four strip tiles join seamlessly."""
    img = Image.new('RGB', (W, H), (0, 0, 0))
    d = ImageDraw.Draw(img)
    kf, sf = _font(max(20, T // 3)), _font(max(18, SH // 3))
    for r in range(a.rows):
        for c in range(a.cols):
            x, y = XK + c * (T + G), r * (T + G)
            shade = 60 if (r + c) % 2 else 110
            d.rectangle((x, y, x + T - 1, y + T - 1), fill=(shade, shade, shade))
            d.text((x + 8, y + 6), '^', font=kf, fill=(255, 220, 0))
            d.text((x + 8, y + T // 2), f'{r},{c}', font=kf, fill=(255, 255, 255))
    if a.strip and a.test_solid:
        d.rectangle((XS, YS, XS + SW - 1, YS + SH - 1), fill=(255, 255, 255))
        return img
    if a.strip:
        for px in range(SW):
            hue = int(px * 255 / max(1, SW - 1))
            d.line((XS + px, YS, XS + px, YS + SH - 1),
                   fill=(hue, 90, 255 - hue))
        tw = SW // STRIP_TILES
        for s in range(STRIP_TILES):
            cx = XS + s * tw
            # small corner label only — NO divider line, so the ramp itself
            # reveals whether the four tiles join seamlessly on the hardware
            d.text((cx + 4, YS + 2), f'S{s}', font=sf, fill=(255, 255, 255))
    return img


src = None if a.test else Path(a.source) if a.source else None
if a.test:
    frame_imgs = [test_canvas()]
    fps = 1.0
    tmp = None
elif src is None:
    p.error('a source file is required (or pass --test)')
elif src.suffix.lower() in STILL_EXT:
    # still image: cover-fit + center-crop, single frame
    img = Image.open(src).convert('RGB')
    scale = max(W / img.width, H / img.height)
    img = img.resize((round(img.width * scale), round(img.height * scale)), Image.LANCZOS)
    x, y = (img.width - W) // 2, (img.height - H) // 2
    frame_imgs = [img.crop((x, y, x + W, y + H))]
    fps = 1.0
    tmp = None
else:
    tmp = Path(tempfile.mkdtemp(prefix='deck-bg-'))
    cmd = ['ffmpeg', '-v', 'error', '-i', str(src)]
    if a.seconds:
        cmd += ['-t', str(a.seconds)]
    cmd += ['-vf', f'fps={a.fps},scale={W}:{H}:force_original_aspect_ratio=increase,crop={W}:{H}',
            str(tmp / 'f%05d.png')]
    subprocess.run(cmd, check=True)
    frame_imgs = [Image.open(f).convert('RGB') for f in sorted(tmp.glob('f*.png'))]
    fps = a.fps

out = Path(a.out)
if out.exists():
    shutil.rmtree(out)
out.mkdir(parents=True)

def face_band(img):
    """The canvas strip band mapped to face-pixel coordinates (800 wide,
    band height + bleed)."""
    return img.crop((XS, YS, XS + SW, YS + SH)) \
              .resize((FACE_W, BAND_H + BAND_BLEED), Image.LANCZOS)


def build_face(img):
    """Map the canvas onto the 800x480 device face: strip band at its
    measured rows (with upward bleed), key region scaled above it."""
    face = Image.new('RGB', (FACE_W, FACE_H), (0, 0, 0))
    keys = img.crop((0, 0, W, YS)).resize((FACE_W, BAND_Y - BAND_BLEED), Image.LANCZOS)
    face.paste(keys, (0, 0))
    face.paste(face_band(img), (0, BAND_Y - BAND_BLEED))
    return face


for i, img in enumerate(frame_imgs):
    for r in range(a.rows):
        for c in range(a.cols):
            x, y = XK + c * (T + G), r * (T + G)
            img.crop((x, y, x + T, y + T)).save(out / f'f{i:05d}_r{r}_c{c}.jpg', quality=82)
    if a.strip:
        # Zone tiles cropped at the measured window offsets so they animate
        # in register with the flashed face filling the inter-zone gaps.
        band = face_band(img)
        for s in range(STRIP_TILES):
            band.crop((ZONE_X[s], BAND_BLEED,
                       ZONE_X[s] + ZONE_W, BAND_BLEED + BAND_H)) \
                .save(out / f'f{i:05d}_s{s}.jpg', quality=82)

if a.strip:
    # Frame 0 as the persistent device background: it lights the 32px gaps
    # between zone windows (which stay static — flashing per frame would wear
    # out the device flash) and doubles as the boot image.
    build_face(frame_imgs[0]).rotate(180).save(out / 'f_face.jpg', quality=88)
if tmp:
    shutil.rmtree(tmp)

(out / 'meta.json').write_text(json.dumps({
    'fps': fps, 'rows': a.rows, 'cols': a.cols,
    'count': len(frame_imgs), 'strip': STRIP_TILES if a.strip else 0,
    'face': 'f_face.jpg' if a.strip else None,
    'source': '__test__' if a.test else src.name,
    'key_lo': a.key_lo, 'key_hi': a.key_hi,
    'opaque': [int(x) for x in a.opaque.split(',') if x.strip()],
}))
print(f'{len(frame_imgs)} frames x {a.rows * a.cols} keys'
      f'{" + face + 4 strip zone tiles" if a.strip else ""} -> {out}')
