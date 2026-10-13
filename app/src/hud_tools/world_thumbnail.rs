//! Keeps each local world's menu thumbnail current, as vanilla saves a world icon: while the
//! player is in the world with no menu open, a frame is captured now and then, cropped to the
//! view around the crosshair and stored beside the world.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bevy::{
    image::Image,
    prelude::{App, IntoScheduleConfigs, Last, Res, ResMut, Resource, Time, Update},
    time::Real,
};
use client_shell::{local_worlds::LocalWorlds, menu::MenuRuntime};
use launcher::local_worlds::world::THUMBNAIL_SIZE;

use super::screenshot::{FrameCapture, FrameCaptureSet};

/// The first capture waits for the terrain around the player to load.
const FIRST_CAPTURE: Duration = Duration::from_secs(10);
/// Later captures follow the player around the world at this pace.
const RECAPTURE: Duration = Duration::from_secs(60);
/// The share of the frame's width the thumbnail keeps around its centre, leaving out the
/// hotbar, the held item and the status bars at the frame's bottom and sides.
const CROP_WIDTH: f32 = 0.7;
const JPEG_QUALITY: u8 = 80;

/// Which world is being captured, when its next capture is due, and finished thumbnails
/// waiting to be stored.
#[derive(Resource, Default)]
struct WorldThumbnailCapture {
    world: Option<String>,
    next_at: Option<Duration>,
    encoded: Arc<Mutex<Vec<(String, Vec<u8>)>>>,
}

pub(super) fn configure(app: &mut App) {
    app.init_resource::<WorldThumbnailCapture>()
        .add_systems(Update, store_world_thumbnails)
        .add_systems(Last, capture_world_thumbnail.before(FrameCaptureSet));
}

/// Stores the thumbnails captures have finished.
fn store_world_thumbnails(state: Res<WorldThumbnailCapture>, worlds: Option<ResMut<LocalWorlds>>) {
    let finished =
        std::mem::take(&mut *state.encoded.lock().unwrap_or_else(PoisonError::into_inner));
    if let Some(mut worlds) = worlds {
        for (id, image) in finished {
            worlds.save_thumbnail(&id, image);
        }
    }
}

/// Requests this frame's pixels for the open world's thumbnail once a capture is due.
fn capture_world_thumbnail(
    time: Res<Time<Real>>,
    menu: Option<Res<MenuRuntime>>,
    mut state: ResMut<WorldThumbnailCapture>,
    mut capture: ResMut<FrameCapture>,
) {
    let now = time.elapsed();
    let playing = menu
        .as_deref()
        .and_then(MenuRuntime::playing_local_world)
        .map(str::to_owned);
    if playing != state.world {
        state.next_at = playing.as_ref().map(|_| now + FIRST_CAPTURE);
        state.world = playing;
    }
    let (Some(id), Some(due)) = (state.world.clone(), state.next_at) else {
        return;
    };
    // A menu over the world, such as the pause screen, would be captured with it.
    if now < due || menu.is_some_and(|menu| menu.is_visible()) {
        return;
    }
    state.next_at = Some(now + RECAPTURE);
    let encoded = Arc::clone(&state.encoded);
    capture.request(move |frame| {
        if let Some(image) = encode_thumbnail(frame) {
            encoded
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((id, image));
        }
    });
}

/// The frame cropped to a 16:9 view around its centre, a little above it, shrunk to
/// [`THUMBNAIL_SIZE`] and encoded as JPEG.
fn encode_thumbnail(frame: &Image) -> Option<Vec<u8>> {
    let pixels = frame.clone().try_into_dynamic().ok()?.to_rgb8();
    let (width, height) = pixels.dimensions();
    let crop_width = ((width as f32 * CROP_WIDTH) as u32).max(1);
    let crop_height = (crop_width * 9 / 16).clamp(1, height);
    let view = image::imageops::crop_imm(
        &pixels,
        (width - crop_width) / 2,
        (height - crop_height) / 3,
        crop_width,
        crop_height,
    )
    .to_image();
    let thumbnail = image::imageops::thumbnail(&view, THUMBNAIL_SIZE[0], THUMBNAIL_SIZE[1]);
    let mut encoded = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, JPEG_QUALITY)
        .encode_image(&thumbnail)
        .ok()?;
    Some(encoded)
}
