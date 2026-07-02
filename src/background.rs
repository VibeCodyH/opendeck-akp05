// Full-deck video background layer (local fork addition).
//
// Loads pre-sliced per-key frames from `~/.config/opendeck-akp05/background/`
// (produced by render_frames.py: f%05d_r%d_c%d.jpg for keys, f%05d_s%d.jpg for
// the touchscreen strip, + meta.json). When present, key/strip images from
// OpenDeck are cached here instead of being written to the device directly;
// a per-device frame clock (device.rs) composites each cached icon over the
// current video tile and paints the whole deck in sync.
//
// Compositing uses luminance keying: OpenDeck flattens transparent icons onto
// black, so near-black pixels are treated as transparent (video shows through)
// with a smooth ramp between `key_lo` and `key_hi` (max-channel value, 0-255).
// Positions listed in meta.json `opaque` are painted as-is, no keying.
//
// The background hot-reloads: watch_task polls meta.json's mtime every 2s and
// swaps the frames in on change (then asks OpenDeck to resend all images).
// No background directory -> inactive, and the driver behaves stock.
use image::{RgbImage, imageops};
use openaction::OUTBOUND_EVENT_MANAGER;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::mappings::{COL_COUNT, ROW_COUNT};

const TILE: u32 = 112; // native key resolution (protocol v3)
const STRIP_W: u32 = 176; // native strip tile resolution, per encoder
const STRIP_H: u32 = 112;
/// Cache slot offset for strip icons (mirrors map_position's encoder offset)
pub const STRIP_SLOT: u8 = 10;

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
    strip: usize, // number of strip tiles per frame (0 = no strip band)
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
    // frame -> slot -> tile; slots: 0-9 = keys (row*COL_COUNT+col), STRIP_SLOT+i = strip
    frames: Vec<HashMap<u8, RgbImage>>,
    strip: usize,
    pub interval_ms: u64,
    key_lo: u8,
    key_hi: u8,
    opaque: Vec<u8>,
}

static BACKGROUND: LazyLock<RwLock<Option<Arc<Background>>>> = LazyLock::new(|| RwLock::new(None));

/// Last image OpenDeck sent per device, by slot (0-9 keys, STRIP_SLOT+i strip).
pub static ICONS: LazyLock<RwLock<HashMap<String, HashMap<u8, RgbImage>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub async fn current() -> Option<Arc<Background>> {
    BACKGROUND.read().await.clone()
}

fn meta_path() -> Option<std::path::PathBuf> {
    Some(
        dirs::config_dir()?
            .join("opendeck-akp05")
            .join("background")
            .join("meta.json"),
    )
}

/// Polls meta.json and (re)loads the background when it appears or changes.
pub async fn watch_task(token: CancellationToken) {
    let mut last: Option<SystemTime> = None;
    loop {
        if token.is_cancelled() {
            return;
        }
        let mtime = meta_path().and_then(|p| std::fs::metadata(p).ok()?.modified().ok());
        if mtime != last {
            last = mtime;
            let bg = tokio::task::spawn_blocking(load).await.ok().flatten();
            let loaded = bg.is_some();
            *BACKGROUND.write().await = bg.map(Arc::new);
            if loaded {
                // Ask OpenDeck to resend every image so icon caches repopulate
                if let Some(outbound) = OUTBOUND_EVENT_MANAGER.lock().await.as_mut() {
                    for id in crate::DEVICES.read().await.keys() {
                        outbound.rerender_images(id.clone()).await.ok();
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

fn load() -> Option<Background> {
    let dir = meta_path()?.parent()?.to_path_buf();
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
    let strip = meta.strip.min(4);
    let mut frames = Vec::with_capacity(meta.count);
    for i in 0..meta.count {
        let mut tiles = HashMap::new();
        for r in 0..rows {
            for c in 0..cols {
                let path = dir.join(format!("f{:05}_r{}_c{}.jpg", i, r, c));
                tiles.insert(
                    (r * COL_COUNT + c) as u8,
                    load_tile(&path, TILE, TILE)?,
                );
            }
        }
        for s in 0..strip {
            let path = dir.join(format!("f{:05}_s{}.jpg", i, s));
            tiles.insert(STRIP_SLOT + s as u8, load_tile(&path, STRIP_W, STRIP_H)?);
        }
        frames.push(tiles);
    }
    if frames.is_empty() {
        return None;
    }

    let fps = if meta.fps > 0.0 { meta.fps } else { 5.0 };
    log::info!(
        "background: loaded {} frames ({}x{} keys, {} strip tiles @ {} fps, source={})",
        frames.len(),
        rows,
        cols,
        strip,
        fps,
        meta.source.as_deref().unwrap_or("?")
    );
    Some(Background {
        frames,
        strip,
        interval_ms: (1000.0 / fps) as u64,
        key_lo: meta.key_lo,
        key_hi: meta.key_hi,
        opaque: meta.opaque,
    })
}

fn load_tile(path: &std::path::Path, w: u32, h: u32) -> Option<RgbImage> {
    let img = match image::open(path) {
        Ok(img) => img,
        Err(e) => {
            log::warn!("background: failed to load {path:?}, background disabled: {e}");
            return None;
        }
    };
    let mut rgb = img.to_rgb8();
    if rgb.width() != w || rgb.height() != h {
        rgb = imageops::resize(&rgb, w, h, imageops::FilterType::Triangle);
    }
    Some(rgb)
}

impl Background {
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn has_strip(&self) -> bool {
        self.strip > 0
    }

    /// Image for one slot at one frame: pure video tile, or the cached icon
    /// luminance-keyed over it. None if the background doesn't cover the slot.
    pub fn compose(&self, frame: usize, slot: u8, icon: Option<&RgbImage>) -> Option<RgbImage> {
        let tile = self.frames.get(frame)?.get(&slot)?;
        let icon = match icon {
            None => return Some(tile.clone()),
            Some(icon) => icon,
        };
        if self.opaque.contains(&slot) {
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

/// Store the image OpenDeck sent for a key/strip slot, sized to the slot's
/// native resolution. Cached even when the background is inactive, so a
/// hot-loaded background composites immediately.
pub async fn cache_icon(device: String, slot: u8, icon: Option<RgbImage>) {
    let mut icons = ICONS.write().await;
    match icon {
        Some(mut rgb) => {
            let (w, h) = if slot >= STRIP_SLOT {
                (STRIP_W, STRIP_H)
            } else {
                (TILE, TILE)
            };
            if rgb.width() != w || rgb.height() != h {
                rgb = imageops::resize(&rgb, w, h, imageops::FilterType::Triangle);
            }
            icons.entry(device).or_default().insert(slot, rgb);
        }
        None => {
            if let Some(m) = icons.get_mut(&device) {
                m.remove(&slot);
            }
        }
    }
}

pub async fn clear_device(device: &str) {
    ICONS.write().await.remove(device);
}
