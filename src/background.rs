// Full-deck video background layer (local fork addition).
//
// Loads pre-sliced per-key frames from `~/.config/opendeck-akp05/background/`
// (produced by render_frames.py: f%05d_r%d_c%d.jpg + meta.json). When present,
// key images from OpenDeck are cached here instead of being written to the
// device directly; a per-device frame clock (device.rs) composites each cached
// icon over the current video tile and paints the whole key grid in sync.
//
// Compositing uses luminance keying: OpenDeck flattens transparent icons onto
// black, so near-black pixels are treated as transparent (video shows through)
// with a smooth ramp between `key_lo` and `key_hi` (max-channel value, 0-255).
// Positions listed in meta.json `opaque` are painted as-is, no keying.
//
// No background directory -> `active()` is false and the driver behaves stock.
use image::{RgbImage, imageops};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::LazyLock;
use tokio::sync::RwLock;

use crate::mappings::{COL_COUNT, ROW_COUNT};

const TILE: u32 = 112; // native key resolution (protocol v3)

fn default_key_lo() -> u8 {
    24
}
fn default_key_hi() -> u8 {
    64
}

#[derive(Deserialize)]
struct Meta {
    fps: f32,
    rows: usize,
    cols: usize,
    count: usize,
    #[serde(default)]
    source: Option<String>,
    #[serde(default = "default_key_lo")]
    key_lo: u8,
    #[serde(default = "default_key_hi")]
    key_hi: u8,
    #[serde(default)]
    opaque: Vec<u8>,
}

pub struct Background {
    frames: Vec<HashMap<u8, RgbImage>>, // frame -> OpenDeck key position -> tile
    pub interval_ms: u64,
    key_lo: u8,
    key_hi: u8,
    opaque: Vec<u8>,
}

pub static BACKGROUND: LazyLock<Option<Background>> = LazyLock::new(load);

/// Last key image OpenDeck sent, per device id, in OpenDeck position space.
pub static ICONS: LazyLock<RwLock<HashMap<String, HashMap<u8, RgbImage>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn active() -> bool {
    BACKGROUND.is_some()
}

fn load() -> Option<Background> {
    let dir = dirs::config_dir()?.join("opendeck-akp05").join("background");
    let meta: Meta = match std::fs::read_to_string(dir.join("meta.json")) {
        Ok(s) => match serde_json::from_str(&s) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("background: bad meta.json, background disabled: {e}");
                return None;
            }
        },
        Err(_) => return None, // no background configured
    };

    let rows = meta.rows.min(ROW_COUNT);
    let cols = meta.cols.min(COL_COUNT);
    let mut frames = Vec::with_capacity(meta.count);
    for i in 0..meta.count {
        let mut tiles = HashMap::new();
        for r in 0..rows {
            for c in 0..cols {
                let path = dir.join(format!("f{:05}_r{}_c{}.jpg", i, r, c));
                let img = match image::open(&path) {
                    Ok(img) => img,
                    Err(e) => {
                        log::warn!("background: failed to load {path:?}, background disabled: {e}");
                        return None;
                    }
                };
                let mut rgb = img.to_rgb8();
                if rgb.width() != TILE || rgb.height() != TILE {
                    rgb = imageops::resize(&rgb, TILE, TILE, imageops::FilterType::Triangle);
                }
                tiles.insert((r * COL_COUNT + c) as u8, rgb);
            }
        }
        frames.push(tiles);
    }
    if frames.is_empty() {
        return None;
    }

    let fps = if meta.fps > 0.0 { meta.fps } else { 5.0 };
    log::info!(
        "background: loaded {} frames ({}x{} keys @ {} fps, source={})",
        frames.len(),
        rows,
        cols,
        fps,
        meta.source.as_deref().unwrap_or("?")
    );
    Some(Background {
        frames,
        interval_ms: (1000.0 / fps) as u64,
        key_lo: meta.key_lo,
        key_hi: meta.key_hi,
        opaque: meta.opaque,
    })
}

impl Background {
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Image for one key at one frame: pure video tile, or the cached icon
    /// luminance-keyed over it. None if the background doesn't cover this key.
    pub fn compose(&self, frame: usize, pos: u8, icon: Option<&RgbImage>) -> Option<RgbImage> {
        let tile = self.frames.get(frame)?.get(&pos)?;
        let icon = match icon {
            None => return Some(tile.clone()),
            Some(icon) => icon,
        };
        if self.opaque.contains(&pos) {
            return Some(icon.clone());
        }

        let (lo, hi) = (self.key_lo as u16, self.key_hi as u16);
        let mut out = tile.clone();
        for (o, i) in out.pixels_mut().zip(icon.pixels()) {
            let v = i.0[0].max(i.0[1]).max(i.0[2]) as u16;
            let a = if v <= lo {
                0
            } else if v >= hi {
                255
            } else {
                (v - lo) * 255 / (hi - lo)
            };
            for ch in 0..3 {
                o.0[ch] = ((i.0[ch] as u16 * a + o.0[ch] as u16 * (255 - a)) / 255) as u8;
            }
        }
        Some(out)
    }
}

/// Store the icon OpenDeck sent for a key (112x112 RGB) in the cache; the
/// frame clock paints it on its next tick.
pub async fn cache_icon(device: String, position: u8, icon: Option<RgbImage>) {
    let mut icons = ICONS.write().await;
    match icon {
        Some(mut rgb) => {
            if rgb.width() != TILE || rgb.height() != TILE {
                rgb = imageops::resize(&rgb, TILE, TILE, imageops::FilterType::Triangle);
            }
            icons.entry(device).or_default().insert(position, rgb);
        }
        None => {
            if let Some(m) = icons.get_mut(&device) {
                m.remove(&position);
            }
        }
    }
}

pub async fn clear_device(device: &str) {
    ICONS.write().await.remove(device);
}
