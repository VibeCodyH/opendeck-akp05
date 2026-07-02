#!/usr/bin/env python3
# Slice a video, GIF, or still image into per-key tiles for the deck video
# background layer of the opendeck-akp05 driver.
# Treats the deck face as one continuous canvas — keys grid + bezel gaps, plus
# the touchscreen strip band below (4 tiles of 176x112 above the knobs) —
# cover-fits the source onto it, then crops each surface's rectangle out of
# every frame.
# Requires: Pillow; ffmpeg on PATH for video/GIF sources.
# Usage: python3 render_frames.py SOURCE [--fps 5] [--gap 0.28] [--out DIR]
import argparse, json, shutil, subprocess, tempfile
from pathlib import Path
from PIL import Image

STILL_EXT = {'.png', '.jpg', '.jpeg', '.bmp', '.webp'}

p = argparse.ArgumentParser()
p.add_argument('source', help='video, GIF, or still image')
p.add_argument('--fps', type=float, default=5)
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
SW = round(STRIP_TILES * 176 * T / 112)   # strip band width in canvas units
SH = round(112 * T / 112)                 # strip band height
WK = a.cols * T + (a.cols - 1) * G        # key grid width
HK = a.rows * T + (a.rows - 1) * G        # key grid height
W = max(WK, SW) if a.strip else WK
H = HK + (G + SH if a.strip else 0)
XK = (W - WK) // 2                        # key grid x-offset (centered)
XS = (W - SW) // 2                        # strip x-offset (centered)
YS = HK + G                               # strip y-offset

src = Path(a.source)
tmp = None
if src.suffix.lower() in STILL_EXT:
    # still image: cover-fit + center-crop, single frame
    img = Image.open(src).convert('RGB')
    scale = max(W / img.width, H / img.height)
    img = img.resize((round(img.width * scale), round(img.height * scale)), Image.LANCZOS)
    x, y = (img.width - W) // 2, (img.height - H) // 2
    frame_imgs = [img.crop((x, y, x + W, y + H))]
    fps = 1.0
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

for i, img in enumerate(frame_imgs):
    for r in range(a.rows):
        for c in range(a.cols):
            x, y = XK + c * (T + G), r * (T + G)
            img.crop((x, y, x + T, y + T)).save(out / f'f{i:05d}_r{r}_c{c}.jpg', quality=82)
    if a.strip:
        tw = SW // STRIP_TILES
        for s in range(STRIP_TILES):
            x = XS + s * tw
            tile = img.crop((x, YS, x + tw, YS + SH))
            if tile.size != (176, 112):
                tile = tile.resize((176, 112), Image.LANCZOS)
            tile.save(out / f'f{i:05d}_s{s}.jpg', quality=82)
if tmp:
    shutil.rmtree(tmp)

(out / 'meta.json').write_text(json.dumps({
    'fps': fps, 'rows': a.rows, 'cols': a.cols,
    'count': len(frame_imgs), 'strip': STRIP_TILES if a.strip else 0,
    'source': src.name,
    'key_lo': a.key_lo, 'key_hi': a.key_hi,
    'opaque': [int(x) for x in a.opaque.split(',') if x.strip()],
}))
print(f'{len(frame_imgs)} frames x {a.rows * a.cols} keys'
      f'{" + 4 strip tiles" if a.strip else ""} -> {out}')
