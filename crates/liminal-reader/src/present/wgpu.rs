use std::sync::Arc;

use lege_gpu::presentation::{
    GpuCompositor, ImageKey, ImageQuad, ImageSource, MAX_IMAGE_EXTENT, PresentationConfig,
    PresentationError, Rect, Sampling,
};
use winit::window::Window;

use crate::geometry::{RectF, RectI};
use crate::scene::{FrameScene, ImageSampling, SceneCommand, tile_image_words};

use super::{PixelFormat, PresentError, Presenter, PresenterBackend, PresenterStats, ScrollReuse};

/// How many times a lost surface is re-adopted before it counts as a real loss.
/// A window being dragged can outrun a single retry.
const LOST_RETRIES: usize = 8;

#[allow(missing_debug_implementations)]
pub struct WgpuPresenter {
    window: Arc<Window>,
    compositor: GpuCompositor,
    /// The size the swapchain is configured for. `GpuCompositor::resize`
    /// reconfigures the surface unconditionally, so it is only called when
    /// this differs from the frame being presented.
    size: (u32, u32),
}

impl WgpuPresenter {
    pub fn new(window: Arc<Window>, width: u32, height: u32) -> Result<Self, PresentError> {
        let compositor = GpuCompositor::new(
            window.clone(),
            width.max(1),
            height.max(1),
            PresentationConfig::default(),
        )
        .map_err(|error| PresentError::Backend(error.to_string()))?;
        Ok(Self {
            window,
            compositor,
            size: (width.max(1), height.max(1)),
        })
    }
}

impl Presenter for WgpuPresenter {
    fn resize(&mut self, width: u32, height: u32) -> Result<(), PresentError> {
        if self.size == (width, height) {
            return Ok(());
        }
        self.compositor
            .resize(width, height)
            .map_err(|error| PresentError::Backend(error.to_string()))?;
        self.size = (width, height);
        Ok(())
    }

    fn present(
        &mut self,
        scene: &FrameScene,
        _damage: &[RectI],
        _scroll_reuse: Option<ScrollReuse>,
    ) -> Result<(), PresentError> {
        // Before `begin_frame`: the compositor clips pushes to its own size.
        self.resize(scene.width, scene.height)?;
        self.draw(scene)?;
        let mut outcome = self.compositor.present();
        // The window changed size after the last `Resized` we handled, so the
        // swapchain is out of date, and reconfiguring it at the size we last
        // knew cannot cure that. Adopt the window's real size each time.
        for _ in 0..LOST_RETRIES {
            if !matches!(outcome, Err(PresentationError::SurfaceLost)) {
                break;
            }
            self.adopt_window_size()?;
            if self.size != (scene.width, scene.height) {
                // This frame is for a size the window no longer has, and the
                // `Resized` that says so is already queued: skip it rather
                // than draw stale content or give up on the GPU.
                return Ok(());
            }
            // Sizes agree. Draw again; a loss that survives every attempt is
            // real and reaches the caller's software fallback.
            self.draw(scene)?;
            outcome = self.compositor.present();
        }
        outcome
            .map(|_| ())
            .map_err(|error| PresentError::Backend(error.to_string()))
    }

    fn format(&self) -> PixelFormat {
        PixelFormat::Xrgb8888
    }

    fn backend(&self) -> PresenterBackend {
        PresenterBackend::Gpu
    }

    fn stats(&self) -> PresenterStats {
        let stats = self.compositor.stats();
        PresenterStats {
            adapter: Some(self.compositor.adapter_name().to_owned()),
            backend: Some(self.compositor.backend_name().to_owned()),
            frames_presented: stats.frames_presented,
            frames_skipped: stats.frames_skipped,
            atlas_uploads: stats.atlas_uploads,
            atlas_upload_bytes: stats.atlas_upload_bytes,
            atlas_resident_images: stats.atlas_resident_images,
            atlas_bytes: stats.atlas_bytes,
            draw_calls: stats.draw_calls,
            vertices: stats.vertices,
        }
    }
}

impl WgpuPresenter {
    /// Reconfigure the swapchain for the size the window has right now.
    fn adopt_window_size(&mut self) -> Result<(), PresentError> {
        let size = self.window.inner_size();
        let (width, height) = (size.width.max(1), size.height.max(1));
        self.compositor
            .resize(width, height)
            .map_err(|error| PresentError::Backend(error.to_string()))?;
        self.size = (width, height);
        Ok(())
    }

    /// Record every scene command into the compositor for the next present.
    fn draw(&mut self, scene: &FrameScene) -> Result<(), PresentError> {
        self.compositor.begin_frame(scene.clear_color);
        for command in &scene.commands {
            match command {
                SceneCommand::Solid { rect, clip, color } => {
                    self.compositor
                        .push_solid(gpu_rect_i(*rect), gpu_rect_i(*clip), *color);
                }
                SceneCommand::AlphaSolid { rect, clip, color } => {
                    self.compositor
                        .push_solid_argb(gpu_rect_i(*rect), gpu_rect_i(*clip), *color);
                }
                SceneCommand::Image {
                    tile,
                    destination,
                    clip,
                    sampling,
                } => {
                    push_image_split(
                        &mut self.compositor,
                        tile_image_words(tile),
                        tile.generation,
                        &tile.pixels,
                        *destination,
                        *clip,
                        *sampling,
                    )?;
                }
                SceneCommand::Surface {
                    surface,
                    destination,
                    clip,
                    sampling,
                } => {
                    push_image_split(
                        &mut self.compositor,
                        surface.key,
                        surface.revision,
                        &surface.pixels,
                        *destination,
                        *clip,
                        *sampling,
                    )?;
                }
            }
        }
        Ok(())
    }
}

/// Push one logical image as however many atlas-sized quads it needs. The
/// compositor's atlas stores at most `MAX_IMAGE_EXTENT`² per image, so wide
/// chrome surfaces (for example a wide toolbar group) are sliced into a grid
/// of sub-quads that share the source buffer through its stride. Each slice
/// gets a salted key so the atlas treats it as its own resident image.
fn push_image_split(
    compositor: &mut GpuCompositor,
    key: [u64; 4],
    revision: u64,
    pixels: &crate::paint::PixelSurface,
    destination: RectF,
    clip: RectI,
    sampling: ImageSampling,
) -> Result<(), PresentError> {
    let sampling = match sampling {
        ImageSampling::Nearest => Sampling::Nearest,
        ImageSampling::Linear => Sampling::Linear,
    };
    if pixels.width <= MAX_IMAGE_EXTENT && pixels.height <= MAX_IMAGE_EXTENT {
        return compositor
            .push_image(ImageQuad {
                source: ImageSource {
                    key: ImageKey(key),
                    revision,
                    width: pixels.width,
                    height: pixels.height,
                    stride_pixels: pixels.stride,
                    pixels: &pixels.pixels,
                },
                destination: gpu_rect(destination),
                clip: gpu_rect_i(clip),
                sampling,
            })
            .map_err(|error| PresentError::Backend(error.to_string()));
    }
    let scale_x = destination.width / f64::from(pixels.width);
    let scale_y = destination.height / f64::from(pixels.height);
    let mut y = 0u32;
    let mut row = 0u64;
    while y < pixels.height {
        let slice_height = (pixels.height - y).min(MAX_IMAGE_EXTENT);
        let mut x = 0u32;
        let mut column = 0u64;
        while x < pixels.width {
            let slice_width = (pixels.width - x).min(MAX_IMAGE_EXTENT);
            let offset = y as usize * pixels.stride + x as usize;
            let salted = [
                key[0],
                key[1],
                key[2],
                key[3] ^ (0x534c_4943_0000_0000 | (column << 16) | row),
            ];
            compositor
                .push_image(ImageQuad {
                    source: ImageSource {
                        key: ImageKey(salted),
                        revision,
                        width: slice_width,
                        height: slice_height,
                        stride_pixels: pixels.stride,
                        pixels: &pixels.pixels[offset..],
                    },
                    destination: gpu_rect(RectF {
                        x: destination.x + f64::from(x) * scale_x,
                        y: destination.y + f64::from(y) * scale_y,
                        width: f64::from(slice_width) * scale_x,
                        height: f64::from(slice_height) * scale_y,
                    }),
                    clip: gpu_rect_i(clip),
                    sampling,
                })
                .map_err(|error| PresentError::Backend(error.to_string()))?;
            x += slice_width;
            column += 1;
        }
        y += slice_height;
        row += 1;
    }
    Ok(())
}

fn gpu_rect(rect: RectF) -> Rect {
    Rect {
        x: rect.x as f32,
        y: rect.y as f32,
        width: rect.width as f32,
        height: rect.height as f32,
    }
}

fn gpu_rect_i(rect: RectI) -> Rect {
    Rect {
        x: rect.x as f32,
        y: rect.y as f32,
        width: rect.width as f32,
        height: rect.height as f32,
    }
}
