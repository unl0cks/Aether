use crate::backends::DesktopUiBackend;
use crate::custom_event::RuffleEvent;
use crate::gui::movie::{MovieView, MovieViewRenderer};
use crate::gui::theme::ThemeController;
use crate::gui::{MENU_HEIGHT, RuffleGui};
use crate::player::{LaunchOptions, PlayerController};
use crate::preferences::GlobalPreferences;
use anyhow::anyhow;
use egui::{Context, FontData, FontDefinitions, ViewportId};
use fontdb::{Database, Family, Query, Source};
use ruffle_core::events::{ImeCursorArea, ImePurpose};
use ruffle_core::{Player, PlayerEvent};
use ruffle_frontend_utils::content::ContentDescriptor;
use ruffle_render_wgpu::backend::{
    WgpuRenderBackend, create_wgpu_instance, request_adapter_and_device,
};
use ruffle_render_wgpu::descriptors::Descriptors;
use ruffle_render_wgpu::target::desired_maximum_frame_latency;
use ruffle_render_wgpu::utils::{format_list, get_backend_names};
use std::any::Any;
use std::fs::File;
use std::path::Path;
use std::sync::{Arc, MutexGuard};
use std::time::{Duration, Instant};
use url::Url;
use wgpu::SurfaceError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GuiRenderOutcome {
    Presented,
    SurfaceUnavailable,
    /// The graphics device is unusable and Aether should shut down.
    DeviceUnusable,
}

/// How many consecutive surface-acquisition failures to tolerate before giving up.
/// At 60 fps this is roughly two seconds of dropped frames, which is far longer
/// than any genuinely transient stall.
const MAX_CONSECUTIVE_SURFACE_FAILURES: u32 = 120;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SurfaceErrorHandling {
    Reconfigure,
    SkipFrame,
    Fatal(SurfaceFailureCause),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SurfaceFailureCause {
    OutOfMemory,
    DeviceFaulted,
    TooManyConsecutiveFailures,
}

impl SurfaceFailureCause {
    fn summary(self) -> &'static str {
        match self {
            SurfaceFailureCause::OutOfMemory => "the graphics device ran out of memory",
            SurfaceFailureCause::DeviceFaulted => "the graphics device reported a fatal fault",
            SurfaceFailureCause::TooManyConsecutiveFailures => {
                "the graphics device stopped producing frames"
            }
        }
    }
}

/// Decide what to do about a failure to acquire the next surface texture.
///
/// `device_faulted` reflects wgpu's own device-loss and uncaptured-error channels.
/// Once either of those has fired, continuing to render is not merely useless: the
/// next invalidated resource we touch takes the process down through a wgpu panic
/// that no error handler can intercept.
fn surface_error_handling(
    error: &SurfaceError,
    consecutive_failures: u32,
    device_faulted: bool,
) -> SurfaceErrorHandling {
    if matches!(error, SurfaceError::OutOfMemory) {
        return SurfaceErrorHandling::Fatal(SurfaceFailureCause::OutOfMemory);
    }
    if device_faulted {
        return SurfaceErrorHandling::Fatal(SurfaceFailureCause::DeviceFaulted);
    }
    if consecutive_failures >= MAX_CONSECUTIVE_SURFACE_FAILURES {
        return SurfaceErrorHandling::Fatal(SurfaceFailureCause::TooManyConsecutiveFailures);
    }
    match error {
        SurfaceError::Lost | SurfaceError::Outdated => SurfaceErrorHandling::Reconfigure,
        _ => SurfaceErrorHandling::SkipFrame,
    }
}
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::EventLoopProxy;
use winit::keyboard::{Key, NamedKey};
use winit::window::{ImePurpose as WinitImePurpose, Theme, Window};

use super::dialogs::export_bundle_dialog::ExportBundleDialogConfiguration;
use super::{DialogDescriptor, FilePicker};

/// Integration layer connecting wgpu+winit to egui.
/// Whether a size the window reported is worth rebuilding the swapchain for.
///
/// A zero dimension means the window was minimised, and configuring a swapchain to it is an error.
/// A size we are already at is what a window move reports on Windows, and rebuilding for it is the
/// most expensive way to do nothing.
fn resize_is_worth_applying(current: PhysicalSize<u32>, reported: PhysicalSize<u32>) -> bool {
    reported.width > 0 && reported.height > 0 && reported != current
}

pub struct GuiController {
    descriptors: Arc<Descriptors>,
    egui_winit: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    gui: RuffleGui,
    window: Arc<Window>,
    last_update: Instant,
    repaint_after: Duration,
    surface: wgpu::Surface<'static>,
    surface_format: wgpu::TextureFormat,
    movie_view_renderer: Arc<MovieViewRenderer>,
    // Note that `window.get_inner_size` can change at any point on x11, even between two lines of code.
    // Use this instead.
    size: PhysicalSize<u32>,
    /// The most recent size the window has reported, if it has not been applied yet.
    ///
    /// Winit reports one of these per frame for the whole of a resize drag, and on Windows
    /// reports them during a plain window move as well. Reconfiguring the swapchain for each
    /// one is both wasted work and, at the rate a drag produces them, enough to lose the
    /// device outright: a 3080 faulted after seven seconds of it. So the reports are
    /// collapsed here and the last one is applied once, on the next frame that renders.
    pending_size: Option<PhysicalSize<u32>>,
    /// If this is set, we should not render the main menu.
    no_gui: bool,
    theme_controller: ThemeController,
    /// Surface acquisition failures since the last successfully acquired frame.
    consecutive_surface_failures: u32,
    /// Whether we have already told the user the device is gone.
    device_fault_reported: bool,
    /// A rolling record of what the GPU was holding, sampled while the renderer is still healthy.
    /// See `aether_gpu_timeline` for why this cannot be sampled at the moment of failure.
    gpu_timeline: ruffle_render_wgpu::aether_gpu_timeline::GpuTimeline,
    /// Optional full history on disk, one JSON object per sample.
    gpu_timeline_file: Option<std::io::BufWriter<std::fs::File>>,
}

impl GuiController {
    pub fn new(
        window: Arc<Window>,
        event_loop: EventLoopProxy<RuffleEvent>,
        preferences: GlobalPreferences,
        font_database: &Database,
        initial_movie_url: Option<Url>,
        no_gui: bool,
    ) -> anyhow::Result<Self> {
        let (instance, backend) = select_wgpu_backend(preferences.graphics_backends().into())?;
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::from_window(window.as_ref())?)
        }?;
        let (adapter, device, queue) = futures::executor::block_on(request_adapter_and_device(
            backend,
            &instance,
            Some(&surface),
            preferences.graphics_power_preference().into(),
        ))
        .map_err(|e| anyhow!(e.to_string()))?;
        let adapter_info = adapter.get_info();
        tracing::info!(
            "Using graphics API {} on {} (type: {:?})",
            adapter_info.backend.to_str(),
            adapter_info.name,
            adapter_info.device_type
        );
        let preferred_formats = [
            // by egui
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Bgra8Unorm,
        ];
        let supported_formats = surface.get_capabilities(&adapter).formats;
        let surface_format = preferred_formats
            .iter()
            .find(|format| supported_formats.contains(format))
            .copied()
            .unwrap_or_else(|| {
                supported_formats
                    .first()
                    .copied()
                    .expect("At least one format should be supported")
            });
        tracing::info!("Using surface format {:?}", surface_format);
        let size = window.inner_size();
        surface.configure(
            &device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: surface_format,
                width: size.width,
                height: size.height,
                present_mode: Default::default(),
                desired_maximum_frame_latency: desired_maximum_frame_latency(&adapter_info),
                alpha_mode: Default::default(),
                view_formats: Default::default(),
            },
        );
        let descriptors = Descriptors::new(instance, adapter, device, queue);
        let egui_ctx = Context::default();

        let theme_controller = futures::executor::block_on(ThemeController::new(
            window.clone(),
            preferences.clone(),
            egui_ctx.clone(),
        ));
        let mut egui_winit = egui_winit::State::new(
            egui_ctx,
            ViewportId::ROOT,
            window.as_ref(),
            None,
            None,
            None,
        );
        egui_winit.set_max_texture_side(descriptors.limits.max_texture_dimension_2d as usize);

        let movie_view_renderer = Arc::new(MovieViewRenderer::new(
            &descriptors.device,
            surface_format,
            window.fullscreen().is_none() && !no_gui,
            size.height,
            window.scale_factor(),
        ));
        let egui_renderer = egui_wgpu::Renderer::new(
            &descriptors.device,
            surface_format,
            egui_wgpu::RendererOptions {
                msaa_samples: 1,
                depth_stencil_format: None,
                dithering: false,
                predictable_texture_filtering: false,
            },
        );
        let descriptors = Arc::new(descriptors);
        let gui = RuffleGui::new(
            Arc::downgrade(&window),
            event_loop,
            initial_movie_url.map(|url| ContentDescriptor {
                url,
                root_content_path: None,
            }),
            LaunchOptions::from(&preferences),
            preferences.clone(),
        );
        let system_fonts = load_system_fonts(font_database, preferences.language());
        egui_winit.egui_ctx().set_fonts(system_fonts);

        egui_extras::install_image_loaders(egui_winit.egui_ctx());

        // Opened before the renderer starts so the very first samples are captured. A path that
        // cannot be opened is reported and then ignored: the in-memory ring still reaches the
        // crash report, which is the part that matters.
        let gpu_timeline_file =
            preferences
                .cli
                .aether_gpu_timeline_file
                .as_ref()
                .and_then(|path| match std::fs::File::create(path) {
                    Ok(file) => Some(std::io::BufWriter::new(file)),
                    Err(error) => {
                        tracing::warn!("Could not open {}: {error}", path.display());
                        None
                    }
                });

        Ok(Self {
            descriptors,
            egui_winit,
            egui_renderer,
            gui,
            window,
            last_update: Instant::now(),
            repaint_after: Duration::ZERO,
            surface,
            surface_format,
            movie_view_renderer,
            size,
            pending_size: None,
            no_gui,
            theme_controller,
            consecutive_surface_failures: 0,
            device_fault_reported: false,
            gpu_timeline: Default::default(),
            gpu_timeline_file,
        })
    }

    pub fn set_theme(&self, theme: Theme) {
        self.theme_controller.set_theme(theme);
    }

    pub fn descriptors(&self) -> &Arc<Descriptors> {
        &self.descriptors
    }

    pub fn file_picker(&self) -> FilePicker {
        self.gui.dialogs.file_picker()
    }

    pub fn window(&self) -> &Arc<Window> {
        &self.window
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if resize_is_worth_applying(self.size, size) {
            self.size = size;
            self.reconfigure_surface();
        }
    }

    /// Apply the last size the window reported, and return it if anything changed.
    ///
    /// Called once per rendered frame rather than once per event, so a drag costs one swapchain
    /// reconfiguration per frame instead of one per report. A report for the size already in use
    /// costs nothing at all, which is the case a window move produces.
    pub fn apply_pending_resize(&mut self) -> Option<PhysicalSize<u32>> {
        let size = self.pending_size.take()?;
        if !resize_is_worth_applying(self.size, size) {
            return None;
        }
        self.size = size;
        self.reconfigure_surface();
        Some(size)
    }

    pub fn reconfigure_surface(&self) {
        self.surface.configure(
            &self.descriptors.device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: self.surface_format,
                width: self.size.width,
                height: self.size.height,
                present_mode: Default::default(),
                desired_maximum_frame_latency: desired_maximum_frame_latency(
                    &self.descriptors.adapter.get_info(),
                ),
                alpha_mode: Default::default(),
                view_formats: Default::default(),
            },
        );
        self.movie_view_renderer.update_resolution(
            &self.descriptors,
            self.window.fullscreen().is_none() && !self.no_gui,
            self.size.height,
            self.window.scale_factor(),
        );
    }

    #[must_use]
    pub fn handle_event(&mut self, event: &WindowEvent) -> bool {
        if let WindowEvent::Resized(size) = &event {
            self.pending_size = Some(*size);
        }

        if let WindowEvent::ThemeChanged(theme) = &event {
            self.set_theme(*theme);
        }

        if matches!(
            &event,
            WindowEvent::KeyboardInput {
                event: winit::event::KeyEvent {
                    logical_key: Key::Named(NamedKey::Tab),
                    ..
                },
                ..
            }
        ) {
            // Prevent egui from consuming the Tab key.
            return false;
        }

        let response = self.egui_winit.on_window_event(&self.window, event);
        if response.repaint {
            self.window.request_redraw();
        }
        response.consumed
    }

    pub fn close_movie(&mut self, player: &mut PlayerController) {
        player.destroy();
        self.gui.on_player_destroyed();
    }

    pub fn create_movie(
        &mut self,
        player: &mut PlayerController,
        opt: LaunchOptions,
        content_descriptor: ContentDescriptor,
    ) {
        tracing::info!("Opening {}", content_descriptor.describe());

        self.close_movie(player);
        let movie_view = MovieView::new(
            self.movie_view_renderer.clone(),
            &self.descriptors.device,
            self.size.width,
            self.size.height,
        );
        player.create(&opt, &content_descriptor, movie_view);
        self.gui.on_player_created(
            opt,
            content_descriptor,
            player
                .get()
                .expect("Player must exist after being created."),
        );
    }

    /// Take a GPU sample if a second has passed, and append it to the history file if one is open.
    ///
    /// Cheap enough to call every frame: it checks a clock, and only on the second does it read
    /// wgpu's counters, which are plain atomics.
    fn sample_gpu_timeline(&mut self) {
        let Some(sample) = self.gpu_timeline.maybe_sample(&self.descriptors.device) else {
            return;
        };

        if let Some(file) = &mut self.gpu_timeline_file {
            use std::io::Write as _;
            // A failed write must not take the session down; the in-memory ring is still intact
            // and the crash report is what actually matters.
            if let Err(error) = writeln!(file, "{}", sample.to_json_line()) {
                tracing::warn!("Could not write the GPU timeline: {error}");
                self.gpu_timeline_file = None;
            } else {
                // Flushed per sample rather than on drop. This exists to survive a crash, and a
                // buffer that is still in memory when the process dies records nothing.
                let _ = file.flush();
            }
        }
    }

    /// Report the fault once, then tell the caller Aether has to shut down.
    fn report_device_fault(&mut self) -> GuiRenderOutcome {
        if !self.device_fault_reported {
            self.device_fault_reported = true;
            let fault = self.descriptors.device_status.fault();
            match &fault {
                Some(fault) => tracing::error!(
                    "{}. Aether has to close. Details: {}",
                    fault.kind.summary(),
                    fault.detail
                ),
                None => {
                    tracing::error!("The graphics device stopped responding. Aether has to close.")
                }
            }

            // A lost device unwinds cleanly and exits, so the panic hook never sees it. This is the
            // only point where the fault and the renderer's own diagnostics are both still in hand.
            if crate::crash_report::is_armed() {
                let detail = match &fault {
                    Some(fault) => format!("{}: {}", fault.kind.summary(), fault.detail),
                    None => "the graphics device stopped responding".to_string(),
                };
                // Always reported, metrics build or not. Device loss is the failure that ends real
                // sessions, it is reported from ordinary release builds, and without these counts
                // a report cannot say whether a fix changed anything on the affected machine.
                // Mutable only when the texture census below is compiled in.
                #[cfg_attr(not(feature = "metrics"), allow(unused_mut))]
                let mut sections = vec![
                    // Kept, but read it knowing what it is: wgpu has already torn the device down
                    // by the time this runs, so it describes the aftermath. The timeline below is
                    // the one that describes the cause.
                    crate::crash_report::Section::new(
                        "GPU resources after device loss",
                        ruffle_render_wgpu::device_resource_report(&self.descriptors.device),
                    ),
                    crate::crash_report::Section::new(
                        "GPU timeline before device loss",
                        self.gpu_timeline.report(),
                    ),
                ];
                #[cfg(feature = "metrics")]
                sections.push(crate::crash_report::Section::new(
                    "Renderer texture census",
                    ruffle_render_wgpu::aether_metrics::texture_census_report(24).join("\n"),
                ));

                if let Some(path) =
                    crate::crash_report::write("graphics device lost", &detail, &sections)
                {
                    tracing::error!("Crash report written to {}", path.display());
                }
            }
        }
        GuiRenderOutcome::DeviceUnusable
    }

    pub fn height_offset(&self) -> f64 {
        if self.window.fullscreen().is_some() || self.no_gui {
            0.0
        } else {
            MENU_HEIGHT as f64 * self.window.scale_factor()
        }
    }

    pub fn window_to_movie_position(&self, position: PhysicalPosition<f64>) -> (f64, f64) {
        let x = position.x;
        let y = position.y - self.height_offset();
        (x, y)
    }

    pub fn movie_to_window_position(&self, x: f64, y: f64) -> PhysicalPosition<f64> {
        let y = y + self.height_offset();
        PhysicalPosition::new(x, y)
    }

    pub fn render(&mut self, mut player: Option<MutexGuard<Player>>) -> GuiRenderOutcome {
        // Sampled before the fault check, not after it. Once the device is faulted the counters
        // describe the teardown rather than the session, which is the trap the previous census
        // fell into.
        self.sample_gpu_timeline();

        // A fault reported through wgpu's device-loss or uncaptured-error channels
        // means every resource we hold may already be invalid. Touching one of them
        // panics from inside wgpu, so stop before we get there.
        if self.descriptors.device_status.is_faulted() {
            return self.report_device_fault();
        }

        let surface_texture = match self.surface.get_current_texture() {
            Ok(surface_texture) => surface_texture,
            Err(error) => {
                self.consecutive_surface_failures =
                    self.consecutive_surface_failures.saturating_add(1);
                return match surface_error_handling(
                    &error,
                    self.consecutive_surface_failures,
                    self.descriptors.device_status.is_faulted(),
                ) {
                    SurfaceErrorHandling::Reconfigure => {
                        // Surface loss and format/size changes require a new swap chain.
                        tracing::warn!("Surface became unavailable: {error:?}, reconfiguring");
                        self.reconfigure_surface();
                        GuiRenderOutcome::SurfaceUnavailable
                    }
                    SurfaceErrorHandling::SkipFrame => {
                        // Timeouts and generic acquisition failures can be transient under GPU load.
                        // Dropping one presentation lets the next frame retry without killing Aether.
                        tracing::warn!("Surface became unavailable: {error:?}, skipping a frame");
                        GuiRenderOutcome::SurfaceUnavailable
                    }
                    SurfaceErrorHandling::Fatal(cause) => {
                        tracing::error!(
                            "Giving up on the graphics device because {}: {error:?}",
                            cause.summary()
                        );
                        self.report_device_fault()
                    }
                };
            }
        };
        self.consecutive_surface_failures = 0;

        let raw_input = self.egui_winit.take_egui_input(&self.window);
        let show_menu = self.window.fullscreen().is_none() && !self.no_gui;
        let mut full_output = self.egui_winit.egui_ctx().run(raw_input, |context| {
            self.gui.update(
                context,
                show_menu,
                player.as_deref_mut(),
                if show_menu {
                    MENU_HEIGHT as f64 * self.window.scale_factor()
                } else {
                    0.0
                },
            );
        });
        self.repaint_after = full_output
            .viewport_output
            .get(&ViewportId::ROOT)
            .expect("Root viewport must exist")
            .repaint_delay;

        // If we're not in a UI, tell egui which cursor we prefer to use instead
        if !self.egui_winit.egui_ctx().wants_pointer_input()
            && let Some(player) = player.as_deref()
        {
            full_output.platform_output.cursor_icon =
                <dyn Any>::downcast_ref::<DesktopUiBackend>(player.ui())
                    .unwrap_or_else(|| panic!("UI Backend should be DesktopUiBackend"))
                    .cursor();
        }
        self.egui_winit
            .handle_platform_output(&self.window, full_output.platform_output);

        let clipped_primitives = self
            .egui_winit
            .egui_ctx()
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        let scale_factor = self.window.scale_factor() as f32;
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.size.width, self.size.height],
            pixels_per_point: scale_factor,
        };

        let mut encoder =
            self.descriptors
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("egui encoder"),
                });

        for (id, image_delta) in &full_output.textures_delta.set {
            self.egui_renderer.update_texture(
                &self.descriptors.device,
                &self.descriptors.queue,
                *id,
                image_delta,
            );
        }

        let mut command_buffers = self.egui_renderer.update_buffers(
            &self.descriptors.device,
            &self.descriptors.queue,
            &mut encoder,
            &clipped_primitives,
            &screen_descriptor,
        );

        let movie_view = if let Some(player) = player.as_deref_mut() {
            let renderer =
                <dyn Any>::downcast_ref::<WgpuRenderBackend<MovieView>>(player.renderer_mut())
                    .expect("Renderer must be correct type");
            Some(renderer.target())
        } else {
            None
        };

        {
            let surface_view = surface_texture.texture.create_view(&Default::default());

            let mut render_pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &surface_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    label: Some("egui_render"),
                    ..Default::default()
                })
                .forget_lifetime();

            if let Some(movie_view) = movie_view {
                movie_view.render(&self.movie_view_renderer, &mut render_pass);
            }

            self.egui_renderer
                .render(&mut render_pass, &clipped_primitives, &screen_descriptor);
        }

        for id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }

        command_buffers.push(encoder.finish());
        self.descriptors.queue.submit(command_buffers);

        // If the device faulted while we were building this frame, there is no safe way
        // to dispose of the acquired surface texture: `Surface::present` and the
        // `discard` that `SurfaceTexture::drop` performs both begin with a device
        // validity check, and both route failure through wgpu's `handle_error_fatal`,
        // which panics unconditionally. Leaking the texture skips `drop` and lets us
        // shut down and report the fault instead of aborting the process. The leak is
        // bounded by the exit that follows.
        if self.descriptors.device_status.is_faulted() {
            std::mem::forget(surface_texture);
            return self.report_device_fault();
        }

        self.window.pre_present_notify();
        surface_texture.present();
        GuiRenderOutcome::Presented
    }

    pub fn show_context_menu(
        &mut self,
        menu: Vec<ruffle_core::ContextMenuItem>,
        close_event: PlayerEvent,
    ) {
        self.gui.show_context_menu(menu, close_event);
    }

    pub fn is_context_menu_visible(&self) -> bool {
        self.gui.is_context_menu_visible()
    }

    pub fn needs_render(&self) -> bool {
        Instant::now().duration_since(self.last_update) >= self.repaint_after
    }

    pub fn show_open_dialog(&mut self) {
        self.gui.dialogs.open_file_advanced()
    }

    pub fn open_dialog(&mut self, dialog_event: DialogDescriptor) {
        self.gui.dialogs.open_dialog(dialog_event);
    }

    pub fn set_ime_allowed(&self, allowed: bool) {
        self.window.set_ime_allowed(allowed);
    }

    pub fn set_ime_purpose(&self, purpose: ImePurpose) {
        self.window.set_ime_purpose(match purpose {
            ImePurpose::Standard => WinitImePurpose::Normal,
            ImePurpose::Password => WinitImePurpose::Password,
        });
    }

    pub fn set_ime_cursor_area(&self, cursor_area: ImeCursorArea) {
        self.window.set_ime_cursor_area(
            self.movie_to_window_position(cursor_area.x, cursor_area.y),
            PhysicalSize::new(cursor_area.width, cursor_area.height),
        );
    }

    pub fn export_bundle(&mut self) {
        let Some(content_descriptor) = self.gui.dialogs.saved_content_descriptor() else {
            return;
        };

        let launch_options = self.gui.dialogs.saved_launch_options();
        let player_options = launch_options.player.clone();
        self.gui
            .dialogs
            .open_dialog(DialogDescriptor::ExportBundle(Box::new(
                ExportBundleDialogConfiguration::new(content_descriptor, player_options),
            )));
        self.gui.on_player_destroyed();
    }
}

fn select_wgpu_backend(
    preferred_backends: wgpu::Backends,
) -> anyhow::Result<(wgpu::Instance, wgpu::Backends)> {
    for backend in preferred_backends.iter() {
        if let Some(instance) = try_wgpu_backend(backend) {
            tracing::info!(
                "Using preferred backend {}",
                format_list(&get_backend_names(backend), "and")
            );
            return Ok((instance, backend));
        }
    }

    tracing::warn!(
        "Preferred backend(s) of {} not available; falling back to any",
        format_list(&get_backend_names(preferred_backends), "or")
    );

    for backend in wgpu::Backends::all() - preferred_backends {
        if let Some(instance) = try_wgpu_backend(backend) {
            tracing::info!(
                "Using fallback backend {}",
                format_list(&get_backend_names(backend), "and")
            );
            return Ok((instance, backend));
        }
    }

    Err(anyhow!(
        "No compatible graphics backends of any kind were available"
    ))
}

fn try_wgpu_backend(backend: wgpu::Backends) -> Option<wgpu::Instance> {
    let instance = create_wgpu_instance(backend, wgpu::BackendOptions::default());
    if instance.enumerate_adapters(backend).is_empty() {
        None
    } else {
        Some(instance)
    }
}

// Load fallback fonts
fn load_system_fonts(
    font_database: &Database,
    locale: unic_langid::LanguageIdentifier,
) -> egui::FontDefinitions {
    let mut fd: FontDefinitions = egui::FontDefinitions::default();

    let lang = locale.language.as_str();
    let is_ja = lang == "ja";
    let is_ko = lang == "ko";
    let is_zh = lang == "zh";
    let is_sc = is_zh && locale.to_string().as_str() == "zh-CN";
    let is_tc = is_zh && !is_sc;

    let mut queries: PrioritizedQueries = Vec::new();

    // The main font
    queries.push((1, vec![Family::SansSerif]));

    // Pan-CJK fonts
    queries.push((
        2,
        vec![
            Family::Name("Noto Sans CJK"),     // Open font
            Family::Name("Source Han Sans"),   // Open font, same as Noto Sans CJK
            Family::Name("WenQuanYi Zen Hei"), // Open font
            Family::Name("Arial Unicode MS"),  // MacOS
        ],
    ));

    // Korean
    queries.push((
        3 + if is_ko { 0 } else { 1 },
        vec![
            Family::Name("Noto Sans CJK KR"), // Open font
            Family::Name("Malgun Gothic"),    // Windows
        ],
    ));

    // Japanese
    queries.push((
        3 + if is_ja { 0 } else { 1 },
        vec![
            Family::Name("Noto Sans CJK JP"), // Open font
            Family::Name("MS UI Gothic"),     // Windows
        ],
    ));

    // Chinese Simplified
    queries.push((
        3 + if is_sc { 0 } else { 1 },
        vec![
            Family::Name("Noto Sans CJK SC"), // Open font
            Family::Name("Microsoft YaHei"),  // Windows
        ],
    ));

    // Chinese Traditional
    queries.push((
        3 + if is_tc { 0 } else { 1 },
        vec![
            Family::Name("Noto Sans CJK TC"),   // Open font
            Family::Name("Microsoft JhengHei"), // Windows
        ],
    ));

    // Hebrew
    queries.push((
        4,
        vec![
            Family::Name("Noto Sans Hebrew"), // Open font
            Family::Name("Tahoma"),           // Windows
        ],
    ));

    // Arabic
    queries.push((
        5,
        vec![
            Family::Name("Noto Sans Arabic"), // Open font
            Family::Name("Tahoma"),           // Windows
        ],
    ));

    // Thai
    queries.push((
        6,
        vec![
            Family::Name("Noto Sans Thai"), // Open font
            Family::Name("Tahoma"),         // Windows
        ],
    ));

    register_family(
        font_database,
        &mut fd,
        egui::FontFamily::Proportional,
        queries,
    );

    fd
}

type FamilyQuery<'a> = Vec<Family<'a>>;
type PrioritizedQueries<'a> = Vec<(usize, FamilyQuery<'a>)>;

fn register_family(
    font_database: &Database,
    fd: &mut FontDefinitions,
    family: egui::FontFamily,
    mut queries: PrioritizedQueries<'_>,
) {
    queries.sort_by_key(|(priority, _)| *priority);
    for (_, query) in queries {
        register_family_font(font_database, fd, family.clone(), &query);
    }
}

fn register_family_font(
    font_database: &Database,
    fd: &mut FontDefinitions,
    family: egui::FontFamily,
    query: &FamilyQuery<'_>,
) {
    let (name, fontdata) = match load_system_font(font_database, query) {
        Ok((name, fontdata)) => (name, fontdata),
        Err(e) => {
            tracing::warn!("Failed to register {query:?} as {family}: {e}");
            return;
        }
    };

    tracing::debug!("Registering font {name} as {family}");

    fd.font_data.insert(name.clone(), fontdata.into());
    fd.families.entry(family).or_default().push(name);
}

fn load_system_font(
    font_database: &Database,
    families: &Vec<Family<'_>>,
) -> anyhow::Result<(String, FontData)> {
    let system_unicode_fonts = Query {
        families,
        ..Query::default()
    };

    let id = font_database
        .query(&system_unicode_fonts)
        .ok_or(anyhow!("no unicode fonts found!"))?;
    let (name, src, index) = font_database
        .face(id)
        .map(|f| (f.post_script_name.clone(), f.source.clone(), f.index))
        .expect("id not found in font database");

    let mut fontdata = match src {
        Source::File(path) | Source::SharedFile(path, _) => {
            let data = mmap_system_font(&path)?;

            // egui accepts only static data, so we have to leak mmapped fonts.
            // This is acceptable, as we're doing it only once.
            let data = Box::leak(Box::new(data));

            egui::FontData::from_static(data)
        }
        Source::Binary(bin) => {
            let data = bin.as_ref().as_ref().to_vec();
            egui::FontData::from_owned(data)
        }
    };
    fontdata.index = index;

    Ok((name, fontdata))
}

fn mmap_system_font(path: &Path) -> anyhow::Result<memmap2::Mmap> {
    let file = File::open(path).map_err(|e| anyhow!("Couldn't open font file at {path:?}: {e}"))?;

    // SAFETY: We have to assume that the font file won't change.
    // This assumption is realistic, as we're using system fonts only.
    let mmap = unsafe { memmap2::Mmap::map(&file) };

    let mmap = mmap.map_err(|e| anyhow!("Failed to mmap font file at {path:?}: {e}"))?;
    Ok(mmap)
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_CONSECUTIVE_SURFACE_FAILURES, PhysicalSize, SurfaceErrorHandling, SurfaceFailureCause,
        resize_is_worth_applying, surface_error_handling,
    };
    use wgpu::SurfaceError;

    #[test]
    fn generic_surface_error_skips_only_the_current_frame() {
        assert_eq!(
            surface_error_handling(&SurfaceError::Other, 0, false),
            SurfaceErrorHandling::SkipFrame
        );
    }

    #[test]
    fn timeout_skips_only_the_current_frame() {
        assert_eq!(
            surface_error_handling(&SurfaceError::Timeout, 0, false),
            SurfaceErrorHandling::SkipFrame
        );
    }

    /// The case that lost a graphics device. Dragging a window by the title bar reports its
    /// unchanged size once per frame, and each report was reconfiguring the swapchain.
    #[test]
    fn a_report_for_the_size_we_already_have_is_not_worth_applying() {
        let current = PhysicalSize::new(1280, 720);
        assert!(!resize_is_worth_applying(current, current));
    }

    #[test]
    fn a_real_resize_is_worth_applying() {
        let current = PhysicalSize::new(1280, 720);
        assert!(resize_is_worth_applying(
            current,
            PhysicalSize::new(1281, 720)
        ));
        assert!(resize_is_worth_applying(
            current,
            PhysicalSize::new(1280, 721)
        ));
    }

    /// Minimising reports a zero dimension, and a swapchain cannot be configured to one.
    #[test]
    fn a_minimised_window_is_not_worth_applying() {
        let current = PhysicalSize::new(1280, 720);
        assert!(!resize_is_worth_applying(
            current,
            PhysicalSize::new(0, 720)
        ));
        assert!(!resize_is_worth_applying(
            current,
            PhysicalSize::new(1280, 0)
        ));
        assert!(!resize_is_worth_applying(current, PhysicalSize::new(0, 0)));
    }

    #[test]
    fn lost_and_outdated_surfaces_are_reconfigured() {
        assert_eq!(
            surface_error_handling(&SurfaceError::Lost, 0, false),
            SurfaceErrorHandling::Reconfigure
        );
        assert_eq!(
            surface_error_handling(&SurfaceError::Outdated, 0, false),
            SurfaceErrorHandling::Reconfigure
        );
    }

    #[test]
    fn out_of_memory_is_always_fatal() {
        assert_eq!(
            surface_error_handling(&SurfaceError::OutOfMemory, 0, false),
            SurfaceErrorHandling::Fatal(SurfaceFailureCause::OutOfMemory)
        );
    }

    #[test]
    fn a_faulted_device_makes_any_surface_error_fatal() {
        // Once the device is gone, retrying only walks us into an unrecoverable
        // panic inside wgpu on the next resource we touch.
        assert_eq!(
            surface_error_handling(&SurfaceError::Other, 0, true),
            SurfaceErrorHandling::Fatal(SurfaceFailureCause::DeviceFaulted)
        );
        assert_eq!(
            surface_error_handling(&SurfaceError::Lost, 0, true),
            SurfaceErrorHandling::Fatal(SurfaceFailureCause::DeviceFaulted)
        );
    }

    #[test]
    fn transient_failures_stop_being_treated_as_transient_eventually() {
        assert_eq!(
            surface_error_handling(
                &SurfaceError::Other,
                MAX_CONSECUTIVE_SURFACE_FAILURES - 1,
                false
            ),
            SurfaceErrorHandling::SkipFrame
        );
        assert_eq!(
            surface_error_handling(
                &SurfaceError::Other,
                MAX_CONSECUTIVE_SURFACE_FAILURES,
                false
            ),
            SurfaceErrorHandling::Fatal(SurfaceFailureCause::TooManyConsecutiveFailures)
        );
    }
}
