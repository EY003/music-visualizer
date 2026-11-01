use eframe::egui;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSession,
};

#[derive(Clone, Copy, PartialEq)]
enum DynamicColorMode {
    Off,
    Cover,
}

struct Settings {
    bar_count: usize,
    bar_width: f32,
    bar_gap: f32,
    sensitivity: f32,
    smoothing: f32,
    visualizer_height: f32,
    bg_opacity: f32,
    dynamic_mode: DynamicColorMode,
    show_settings: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bar_count: 64,
            bar_width: 8.0,
            bar_gap: 2.0,
            sensitivity: 40.0,
            smoothing: 0.25,
            visualizer_height: 180.0,
            bg_opacity: 0.0, // Fully transparent by default
            dynamic_mode: DynamicColorMode::Cover,
            show_settings: false,
        }
    }
}

struct TrackInfo {
    title: String,
    artist: String,
    cover_texture: Option<egui::TextureHandle>,
    dominant_color: egui::Color32,
}

struct VisualizerApp {
    settings: Settings,
    audio_fft_data: Arc<Mutex<Vec<f32>>>,
    smooth_bars: Vec<f32>,
    track_info: Arc<Mutex<TrackInfo>>,
}

impl VisualizerApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let audio_fft_data = Arc::new(Mutex::new(vec![0.0; 64]));
        let track_info = Arc::new(Mutex::new(TrackInfo {
            title: "Waiting for MusicBee...".into(),
            artist: "Play a song to begin".into(),
            cover_texture: None,
            dominant_color: egui::Color32::from_rgb(100, 180, 255),
        }));

        // Spawn background thread for real Windows SMTC Media & Cover Art monitoring
        let track_info_clone = Arc::clone(&track_info);
        let ctx_clone = cc.egui_ctx.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                loop {
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    if let Ok(manager) = GlobalSystemMediaTransportControlsSessionManager::RequestAsync() {
                        if let Ok(session) = manager.and_then(|m| m.GetCurrentSession()) {
                            if let Ok(props) = session.TryGetMediaPropertiesAsync() {
                                if let Ok(p) = props {
                                    let title = p.Title().unwrap_or_default().to_string_lossy();
                                    let artist = p.Artist().unwrap_or_default().to_string_lossy();
                                    
                                    let mut locked = track_info_clone.lock().unwrap();
                                    if locked.title != title || locked.artist != artist {
                                        locked.title = if title.is_empty() { "Unknown Title".into() } else { title };
                                        locked.artist = if artist.is_empty() { "Unknown Artist".into() } else { artist };
                                        
                                        // Attempt to grab thumbnail cover art
                                        if let Ok(ref stream_ref) = p.Thumbnail() {
                                            if let Ok(stream) = rt.block_on(async { stream_ref.OpenReadAsync() }) {
                                                if let Ok(readable_stream) = stream {
                                                    let size = readable_stream.Size().unwrap_or(0) as usize;
                                                    if size > 0 {
                                                        if let Ok(reader) = windows::Storage::Streams::DataReader::CreateDataReader(&readable_stream) {
                                                            let _ = rt.block_on(async { reader.LoadAsync(size as u32) });
                                                            let mut buf = vec![0u8; size];
                                                            if reader.ReadBytes(&mut buf).is_ok() {
                                                                if let Ok(img) = image::load_from_memory(&buf) {
                                                                    let rgba = img.to_rgba8();
                                                                    let w = rgba.width() as usize;
                                                                    let h = rgba.height() as usize;
                                                                    let pixels = rgba.into_raw();
                                                                    
                                                                    // Extract dominant color using color-thief
                                                                    if let Ok(color) = color_thief::color_thief(&pixels, color_thief::ColorFormat::Rgba, 10, color_thief::ColorSpace::Rgb) {
                                                                        locked.dominant_color = egui::Color32::from_rgb(color.r, color.g, color.b);
                                                                    }
                                                                    
                                                                    let image_data = egui::ColorImage::from_rgba_unmultiplied([w, h], &pixels);
                                                                    let texture = ctx_clone.load_texture("cover_art", image_data, egui::TextureOptions::default());
                                                                    locked.cover_texture = Some(texture);
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    ctx_clone.request_repaint();
                }
            });
        });

        // Spawn background audio capture thread via CPAL loopback + FFT
        let fft_data_clone = Arc::clone(&audio_fft_data);
        std::thread::spawn(move || {
            run_audio_capture(fft_data_clone);
        });

        Self {
            settings: Settings::default(),
            audio_fft_data,
            smooth_bars: vec![0.0; 64],
            track_info,
        }
    }
}

fn run_audio_capture(fft_data: Arc<Mutex<Vec<f32>>>) {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let device = match host.default_output_device() {
        Some(d) => d,
        None => return,
    };
    let config = match device.default_output_config() {
        Ok(c) => c,
        Err(_) => return,
    };

    let stream = device.build_input_stream(
        &config.into(),
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            let mut planner = rustfft::FftPlanner::new();
            let fft_size = 512;
            let fft = planner.plan_fft_forward(fft_size);
            
            let mut buffer: Vec<rustfft::num_complex::Complex<f32>> = data.iter()
                .take(fft_size)
                .map(|&v| rustfft::num_complex::Complex { re: v, im: 0.0 })
                .collect();
            
            buffer.resize(fft_size, rustfft::num_complex::Complex { re: 0.0, im: 0.0 });
            fft.process(&mut buffer);

            let mut bars = vec![0.0; 128];
            let magnitude: Vec<f32> = buffer.iter().map(|c| c.norm()).collect();
            let chunk_size = (magnitude.len() / 2).max(1) / bars.len();

            for (i, bar) in bars.iter_mut().enumerate() {
                let start = i * chunk_size;
                let end = (start + chunk_size).min(magnitude.len() / 2);
                if start < end {
                    let sum: f32 = magnitude[start..end].iter().sum();
                    *bar = (sum / (end - start) as f32).clamp(0.0, 1.0);
                }
            }
            if let Ok(mut lock) = fft_data.lock() {
                *lock = bars;
            }
        },
        |err| eprintln!("Audio capture error: {}", err),
        None,
    );

    if let Ok(s) = stream {
        let _ = s.play();
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }
}

impl eframe::App for VisualizerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint(); // Keep 60 FPS continuous rendering for smooth visualizer

        // Window background fill opacity control
        let mut style = (*ctx.style()).clone();
        style.visuals.window_fill = egui::Color32::from_black_alpha((self.settings.bg_opacity * 255.0) as u8);
        ctx.set_style(style);

        // Top-Right Movable Song Info & Cover Art Panel
        egio_top_right_panel(ctx, &self.track_info, &mut self.settings);

        // Resize smooth bars array if bar count setting changes
        if self.smooth_bars.len() != self.settings.bar_count {
            self.smooth_bars.resize(self.settings.bar_count, 0.0);
        }

        // Fetch raw audio FFT data
        let raw_bars = self.audio_fft_data.lock().unwrap().clone();
        let track = self.track_info.lock().unwrap();
        let bar_color = match self.settings.dynamic_mode {
            DynamicColorMode::Cover => track.dominant_color,
            DynamicColorMode::Off => egui::Color32::from_rgb(130, 200, 255),
        };

        // Bottom Visualizer Bars Render Panel (Movable / Positionable)
        egui::Window::new("Visualizer Bars")
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .frame(egui::Frame::none().fill(egui::Color32::from_black_alpha((self.settings.bg_opacity * 255.0) as u8)))
            .show(ctx, |ui| {
                let total_width = self.settings.bar_count as f32 * (self.settings.bar_width + self.settings.bar_gap);
                let (rect, _response) = ui.allocate_exact_size(
                    egui::vec2(total_width, self.settings.visualizer_height + 20.0),
                    egui::Sense::drag(),
                );
                let painter = ui.painter();
                let base_y = rect.bottom() - 10.0;

                for i in 0..self.settings.bar_count {
                    let raw_val = if i < raw_bars.len() { raw_bars[i] } else { 0.0 };
                    let target_height = raw_val * self.settings.visualizer_height * (self.settings.sensitivity / 20.0);
                    
                    // Smooth interpolation (lerp) for 60 FPS fluid decay
                    self.smooth_bars[i] += (target_height - self.smooth_bars[i]) * self.settings.smoothing;
                    let height = self.smooth_bars[i].max(2.0);

                    let x = rect.left() + i as f32 * (self.settings.bar_width + self.settings.bar_gap);
                    let bar_rect = egui::Rect::from_min_size(
                        egui::pos2(x, base_y - height),
                        egui::vec2(self.settings.bar_width, height),
                    );

                    painter.rect_filled(bar_rect, 1.5, bar_color);
                }
            });

        // Settings Menu Modal
        if self.settings.show_settings {
            egui::Window::new("Visualizer Settings ⚙")
                .open(&mut self.settings.show_settings)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.heading("Visualizer Customization");
                    ui.add(egui::Slider::new(&mut self.settings.bar_count, 16..=128).text("Bar Count"));
                    ui.add(egui::Slider::new(&mut self.settings.bar_width, 2.0..=20.0).text("Bar Width"));
                    ui.add(egui::Slider::new(&mut self.settings.bar_gap, 0.0..=8.0).text("Bar Gap"));
                    ui.add(egui::Slider::new(&mut self.settings.sensitivity, 10.0..=100.0).text("Sensitivity"));
                    ui.add(egui::Slider::new(&mut self.settings.smoothing, 0.05..=1.0).text("Smoothness / Decay"));
                    ui.add(egui::Slider::new(&mut self.settings.visualizer_height, 50.0..=400.0).text("Visualizer Height"));
                    ui.add(egui::Slider::new(&mut self.settings.bg_opacity, 0.0..=1.0).text("Background Opacity"));
                    
                    ui.separator();
                    ui.label("Dynamic Colors");
                    ui.radio_value(&mut self.settings.dynamic_mode, DynamicColorMode::Off, "Static Color");
                    ui.radio_value(&mut self.settings.dynamic_mode, DynamicColorMode::Cover, "Match Cover Art Accent");
                });
        }
    }
}

fn egio_top_right_panel(ctx: &egui::Context, track_info: &Arc<Mutex<TrackInfo>>, settings: &mut Settings) {
    egui::Window::new("Song Info Panel")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .frame(egui::Frame::none().fill(egui::Color32::TRANSPARENT))
        .show(ctx, |ui| {
            let track = track_info.lock().unwrap();
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(&track.title).size(16.0).strong().color(egui::Color32::WHITE));
                    ui.label(egui::RichText::new(&track.artist).size(13.0).color(egui::Color32::LIGHT_GRAY));
                });
                if let Some(tex) = &track.cover_texture {
                    ui.image((tex.id(), egui::vec2(65.0, 65.0)));
                } else {
                    let (_, rect) = ui.allocate_space(egui::vec2(65.0, 65.0));
                    ui.painter().rect_filled(rect, 4.0, egui::Color32::from_black_alpha(100));
                }
            });

            if ui.small_button("⚙ Settings").clicked() {
                settings.show_settings = !settings.show_settings;
            }
        });
}

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_transparent(true)
            .with_decorations(false)
            .with_inner_size([1920.0, 1080.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Music Visualizer Rust",
        options,
        Box::new(|cc| Ok(Box::new(VisualizerApp::new(cc)))),
    )
}
