use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use data_url::DataUrl;
use image::{DynamicImage, load_from_memory_with_format};
use mirajazz::{device::Device, error::MirajazzError, state::DeviceStateUpdate};
use openaction::{OUTBOUND_EVENT_MANAGER, SetImageEvent};
use tokio::time::interval;
use tokio_util::sync::CancellationToken;

use crate::{
    DEVICES, TOKENS, background, led_config,
    mappings::{
        COL_COUNT, CandidateDevice, DEVICE_TYPE, ENCODER_COUNT, KEY_COUNT, Kind, ROW_COUNT,
    },
};

/// Initializes a device and listens for events
pub async fn device_task(candidate: CandidateDevice, token: CancellationToken) {
    log::info!("Running device task for {:?}", candidate);

    // Wrap in a closure so we can use `?` operator
    let device = async {
        let device = connect(&candidate).await?;

        device.set_brightness(50).await?;
        device.clear_all_button_images().await?;
        device.flush().await?;

        let led = led_config::load();
        log::info!("Applying LED config: {:?}", led);
        if let Some(led_config::LedMode::Static { colors }) = led.mode {
            device.set_led_brightness(led.brightness).await?;
            device.set_led_colors(&colors).await?;
        }

        Ok(device)
    }
    .await;

    let device: Device = match device {
        Ok(device) => device,
        Err(err) => {
            handle_error(&candidate.id, err).await;

            log::error!(
                "Had error during device init, finishing device task: {:?}",
                candidate
            );

            return;
        }
    };

    log::info!("Registering device {}", candidate.id);
    if let Some(outbound) = OUTBOUND_EVENT_MANAGER.lock().await.as_mut() {
        outbound
            .register_device(
                candidate.id.clone(),
                candidate.kind.human_name(),
                ROW_COUNT as u8,
                COL_COUNT as u8,
                ENCODER_COUNT as u8,
                DEVICE_TYPE,
            )
            .await
            .unwrap();
    }

    DEVICES.write().await.insert(candidate.id.clone(), device);

    tokio::select! {
        _ = device_events_task(&candidate) => {},
        _ = keepalive_task(&candidate) => {},
        _ = background_task(&candidate) => {},
        _ = token.cancelled() => {}
    };

    log::info!("Shutting down device {:?}", candidate);

    if let Some(device) = DEVICES.read().await.get(&candidate.id) {
        device.shutdown().await.ok();
    }

    log::info!("Device task finished for {:?}", candidate);
}

/// Handles errors, returning true if should continue, returning false if an error is fatal
pub async fn handle_error(id: &String, err: MirajazzError) -> bool {
    log::error!("Device {} error: {}", id, err);

    // Some errors are not critical and can be ignored without sending disconnected event
    if matches!(err, MirajazzError::ImageError(_) | MirajazzError::BadData) {
        return true;
    }

    log::info!("Deregistering device {}", id);
    if let Some(outbound) = OUTBOUND_EVENT_MANAGER.lock().await.as_mut() {
        outbound.deregister_device(id.clone()).await.unwrap();
    }

    log::info!("Cancelling tasks for device {}", id);
    if let Some(token) = TOKENS.read().await.get(id) {
        token.cancel();
    }

    log::info!("Removing device {} from the list", id);
    DEVICES.write().await.remove(id);

    log::info!("Finished clean-up for {}", id);

    false
}

pub async fn connect(candidate: &CandidateDevice) -> Result<Device, MirajazzError> {
    let result = Device::connect(
        &candidate.dev,
        candidate.kind.protocol_version(),
        KEY_COUNT,
        ENCODER_COUNT,
    )
    .await;

    match result {
        Ok(device) => {
            Ok(device
                .with_supports_both_encoder_states(candidate.kind.supports_both_encoder_states()))
        }
        Err(e) => {
            log::error!("Error while connecting to device: {e}");

            Err(e)
        }
    }
}

/// Handles events from device to OpenDeck
async fn device_events_task(candidate: &CandidateDevice) -> Result<(), MirajazzError> {
    log::info!("Connecting to {} for incoming events", candidate.id);

    let devices_lock = DEVICES.read().await;
    let reader = match devices_lock.get(&candidate.id) {
        Some(device) => device.get_reader(crate::inputs::process_input),
        None => return Ok(()),
    };
    drop(devices_lock);

    log::info!("Connected to {} for incoming events", candidate.id);

    log::info!("Reader is ready for {}", candidate.id);

    loop {
        log::debug!("Reading updates...");

        let updates = match reader.read(None).await {
            Ok(updates) => updates,
            Err(e) => {
                if !handle_error(&candidate.id, e).await {
                    break;
                }

                continue;
            }
        };

        if inputs_suppressed() && !updates.is_empty() {
            log::info!(
                "Dropping {} input event(s) read during the face-flash window",
                updates.len()
            );
            continue;
        }

        for update in updates {
            log::debug!("New update: {:#?}", update);

            let id = candidate.id.clone();

            if let Some(outbound) = OUTBOUND_EVENT_MANAGER.lock().await.as_mut() {
                match update {
                    DeviceStateUpdate::ButtonDown(key) => outbound.key_down(id, key).await.unwrap(),
                    DeviceStateUpdate::ButtonUp(key) => outbound.key_up(id, key).await.unwrap(),
                    DeviceStateUpdate::EncoderDown(encoder) => {
                        outbound.encoder_down(id, encoder).await.unwrap();
                    }
                    DeviceStateUpdate::EncoderUp(encoder) => {
                        outbound.encoder_up(id, encoder).await.unwrap();
                    }
                    DeviceStateUpdate::EncoderTwist(encoder, val) => {
                        outbound
                            .encoder_change(id, encoder, val as i16)
                            .await
                            .unwrap();
                    }
                }
            }
        }
    }

    Ok(())
}

/// Sends periodic keepalives to the device to maintain connection
async fn keepalive_task(candidate: &CandidateDevice) -> Result<(), MirajazzError> {
    let mut interval = interval(Duration::from_secs(10));

    loop {
        interval.tick().await;

        log::debug!("Sending keepalive to {}", candidate.id);

        let devices_lock = DEVICES.read().await;
        let device = match devices_lock.get(&candidate.id) {
            Some(device) => device,
            None => return Ok(()),
        };

        if let Err(e) = device.keep_alive().await {
            drop(devices_lock);
            if !handle_error(&candidate.id, e).await {
                break;
            }
        }
    }

    Ok(())
}

/// Inputs read before this deadline (ms since UNIX_EPOCH) are dropped. The
/// face flash makes the device emit ACK/status reports that parse as phantom
/// presses (observed: encoder 0 taps — "mute" — on every background switch),
/// so the reader ignores everything from flash start until shortly after.
static INPUT_SUPPRESS_UNTIL: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn suppress_inputs_for_ms(ms: u64) {
    INPUT_SUPPRESS_UNTIL.store(now_ms() + ms, Ordering::Relaxed);
}

fn inputs_suppressed() -> bool {
    now_ms() < INPUT_SUPPRESS_UNTIL.load(Ordering::Relaxed)
}

/// Flash the full-face 800x480 background (the persistent "LOG" write, the
/// same one the vendor tool uses for its device background). It lights the
/// entire touchscreen strip as one seamless band — including the ~32px gaps
/// between the four per-key zone windows that `set_button_image` can never
/// reach — and survives power cycles (it doubles as the boot image).
///
/// This is a firmware FLASH write: it blocks the device for a couple of
/// seconds and commands sent meanwhile are dropped (verified on-device), so
/// callers must sleep ~2s afterwards before sending images. Never call this
/// per frame — flash wear. Only on background *change*.
pub async fn write_face(device: &Device, jpeg: &[u8]) -> Result<(), MirajazzError> {
    // Cover the whole flash, then trim to a short tail once it's done — the
    // device keeps emitting phantom-parsing reports for a moment after STP.
    suppress_inputs_for_ms(30_000);
    let result = write_face_inner(device, jpeg).await;
    suppress_inputs_for_ms(3_000);
    result
}

async fn write_face_inner(device: &Device, jpeg: &[u8]) -> Result<(), MirajazzError> {
    let n = jpeg.len();
    let mut hdr = vec![
        0x00, 0x43, 0x52, 0x54, 0x00, 0x00, // report id + "CRT" prefix
        0x4C, 0x4F, 0x47, // "LOG"
        (n >> 24) as u8,
        (n >> 16) as u8,
        (n >> 8) as u8,
        n as u8,
        0x01,
    ];
    device.write_extended_data(&mut hdr).await?;
    // Payload rides in packet-size chunks (1024 on the protocol-v3 devices
    // this plugin drives), 0x00 report id in front of each.
    for chunk in jpeg.chunks(1024) {
        let mut buf = Vec::with_capacity(1025);
        buf.push(0x00);
        buf.extend_from_slice(chunk);
        buf.resize(1025, 0);
        device.write_data(&buf).await?;
    }
    let mut stp = vec![0x00, 0x43, 0x52, 0x54, 0x00, 0x00, 0x53, 0x54, 0x50];
    device.write_extended_data(&mut stp).await
}

/// A plain black face, flashed when the background is removed so the strip
/// (and boot screen) go dark instead of showing a stale wallpaper.
pub fn black_face_jpeg() -> Vec<u8> {
    let img = image::RgbImage::new(800, 480);
    let mut buf = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85);
    enc.encode(img.as_raw(), 800, 480, image::ExtendedColorType::Rgb8)
        .ok();
    buf
}

fn map_position(mut position: u8, is_encoder: bool) -> Result<u8, MirajazzError> {
    if is_encoder {
        position += 10;
    }
    let position = match position {
        0 => 10,
        1 => 11,
        2 => 12,
        3 => 13,
        4 => 14,
        5 => 5,
        6 => 6,
        7 => 7,
        8 => 8,
        9 => 9,
        10 => 0,
        11 => 1,
        12 => 2,
        13 => 3,
        _ => {
            log::error!("Invalid key position");
            return Err(MirajazzError::BadData);
        }
    };
    Ok(position)
}

/// Repaints the key grid (and touchscreen strip, when the background covers
/// it) in sync with the background frame clock: every tick, each surface gets
/// its video tile with the cached icon (if any) luminance-keyed on top.
/// Idles when no background is configured; picks up hot-reloads.
async fn background_task(candidate: &CandidateDevice) -> Result<(), MirajazzError> {
    let key_format = candidate.kind.image_format();
    let strip_format = candidate.kind.touch_image_format();
    let mut frame = 0usize;
    // (frame, icon generation, background identity) of the last paint — when a
    // still background (frame_count 1) and the icons are both unchanged,
    // skip the tick entirely instead of spamming identical images over USB.
    let mut painted: Option<(usize, u64, usize)> = None;
    // Background identity whose face was last handled (flashed or skipped).
    let mut faced: Option<usize> = None;

    loop {
        let Some(bg) = background::current().await else {
            tokio::time::sleep(Duration::from_millis(500)).await;
            continue;
        };
        tokio::time::sleep(Duration::from_millis(bg.interval_ms.max(50))).await;
        if frame >= bg.frame_count() {
            frame = 0; // background was swapped for a shorter one
        }
        let bg_id = std::sync::Arc::as_ptr(&bg) as usize;

        // New background with a face: flash it (skipped when the marker says
        // identical content is already on the device), then let OpenDeck
        // resend icons — the flash write eats anything sent during it.
        if bg.face_jpeg.is_some() && faced != Some(bg_id) {
            faced = Some(bg_id);
            if let Some(jpeg) = &bg.face_jpeg {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                std::hash::Hasher::write(&mut hasher, jpeg);
                let hash = format!("{:016x}", std::hash::Hasher::finish(&hasher));
                let marker = background::face_marker_path();
                let logged = marker
                    .as_ref()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .unwrap_or_default();
                if logged.trim() != hash {
                    log::info!("background: flashing face ({} bytes) to {}", jpeg.len(), candidate.id);
                    {
                        let devices_lock = DEVICES.read().await;
                        let Some(device) = devices_lock.get(&candidate.id) else {
                            return Ok(());
                        };
                        if let Err(e) = write_face(device, jpeg).await {
                            log::warn!("background: face flash failed: {e}");
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(2500)).await;
                    if let Some(p) = marker {
                        std::fs::write(p, &hash).ok();
                    }
                    if let Some(outbound) = OUTBOUND_EVENT_MANAGER.lock().await.as_mut() {
                        outbound.rerender_images(candidate.id.clone()).await.ok();
                    }
                }
            }
        }

        let state = (frame, background::icon_generation(), bg_id);
        if painted == Some(state) {
            continue;
        }

        // Compose everything first so the icon cache isn't locked during USB IO
        let composed: Vec<(u8, bool, image::RgbImage)> = {
            let icons = background::ICONS.read().await;
            let device_icons = icons.get(&candidate.id);
            let mut v = Vec::new();
            for pos in 0..(ROW_COUNT * COL_COUNT) as u8 {
                let icon = device_icons.and_then(|m| m.get(&pos));
                if let Some(img) = bg.compose(frame, pos, icon) {
                    v.push((pos, false, img));
                }
            }
            for s in 0..ENCODER_COUNT as u8 {
                let slot = background::STRIP_SLOT + s;
                let icon = device_icons.and_then(|m| m.get(&slot));
                if let Some(img) = bg.compose(frame, slot, icon) {
                    v.push((s, true, img));
                }
            }
            v
        };

        let devices_lock = DEVICES.read().await;
        let Some(device) = devices_lock.get(&candidate.id) else {
            return Ok(());
        };

        let result: Result<(), MirajazzError> = async {
            for (pos, is_encoder, img) in composed {
                let format = if is_encoder { &strip_format } else { &key_format };
                device
                    .set_button_image(
                        map_position(pos, is_encoder)?,
                        format.clone(),
                        DynamicImage::ImageRgb8(img),
                    )
                    .await?;
            }
            device.flush().await
        }
        .await;

        drop(devices_lock);

        if let Err(e) = result {
            if !handle_error(&candidate.id, e).await {
                break;
            }
        } else {
            painted = Some(state);
        }

        frame = (frame + 1) % bg.frame_count();
    }

    Ok(())
}

/// Handles different combinations of "set image" event, including clearing the specific buttons and whole device
pub async fn handle_set_image(device: &Device, evt: SetImageEvent) -> Result<(), MirajazzError> {
    let is_encoder = evt.controller.as_deref() == Some("Encoder");

    // Background layer: the icon cache is kept current on every event (even
    // while inactive, so a hot-loaded background composites immediately).
    // When the background covers this surface, skip the direct write — the
    // frame clock (background_task) owns those writes.
    let covered = match background::current().await {
        Some(bg) => !is_encoder || bg.has_strip(),
        None => false,
    };
    let slot_base = if is_encoder { background::STRIP_SLOT } else { 0 };

    match (evt.position, evt.image) {
        (Some(position), Some(image)) => {
            log::debug!("Setting image for button {}", position);

            // OpenDeck sends image as a data url, so parse it using a library
            let url = DataUrl::process(image.as_str()).unwrap(); // Isn't expected to fail, so unwrap it is
            let (body, _fragment) = url.decode_to_vec().unwrap(); // Same here

            // Allow only image/jpeg mime for now
            if url.mime_type().subtype != "jpeg" {
                log::error!("Incorrect mime type: {}", url.mime_type());

                return Ok(()); // Not a fatal error, enough to just log it
            }

            let image = load_from_memory_with_format(body.as_slice(), image::ImageFormat::Jpeg)?;

            background::cache_icon(evt.device, slot_base + position, Some(image.to_rgb8())).await;
            if covered {
                return Ok(());
            }

            device
                .set_button_image(
                    map_position(position, is_encoder)?,
                    if is_encoder {
                        Kind::from_vid_pid(device.vid, device.pid)
                            .unwrap()
                            .touch_image_format()
                    } else {
                        Kind::from_vid_pid(device.vid, device.pid)
                            .unwrap()
                            .image_format()
                    },
                    image,
                )
                .await?;
            device.flush().await?;
        }
        (Some(position), None) => {
            background::cache_icon(evt.device, slot_base + position, None).await;
            if covered {
                return Ok(());
            }
            let position = map_position(position, is_encoder)?;
            device.clear_button_image(position).await?;
            device.flush().await?;
        }
        (None, None) => {
            background::clear_device(&evt.device).await;
            if covered {
                return Ok(());
            }
            device.clear_all_button_images().await?;
            device.flush().await?;
        }
        _ => {}
    }

    Ok(())
}
