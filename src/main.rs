use eframe::egui;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq)]
enum DynamicColorMode {
    Off,
    Cover,
    Windows,
}

struct Settings {
    bar_width: f32,
    bar_gap: f32,
    sensitivity: f32,
    fft_size: usize,
    visualizer_height: f32,
    dynamic_mode: DynamicColorMode,
    click_through: bool,
    show_settings: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bar_width: 8.0,
            bar_gap: 2.0,
            sensitivity: 35.0,
            fft_size: 1024,
            visualizer_height: 180.0,
            dynamic_mode: DynamicColorMode::Cover,
            click_through: false,
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
    track_info: Arc<Mutex<TrackInfo>>,
}

impl VisualizerApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let audio_fft_data = Arc::new(Mutex::new(vec![0.0; 64]));
        let track_info = Arc::new(Mutex::new(TrackInfo {
            title: "Waiting for music...".into(),
            artist: "MusicBee / SMTC".into(),
            cover_texture: None,
            dominant_color: egui::Color32::from_rgb(100, 180, 255),
        }));

        // Spawn background thread for Windows SMTC Media Session monitoring
        let track_info_clone = Arc::clone(&track_info);
        let ctx_clone = cc.egui_ctx.clone();
        std::thread::spawn(move || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                loop {
                    // Poll Windows Media Session Manager for current track & cover art
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    // Placeholder for actual Windows SMTC binding logic
                    ctx_clone.request_repaint();
                }
            });
        });

        // Spawn background audio capture thread via CPAL loopback
        let fft_data_clone = Arc::clone(&audio_fft_data);
        std::thread::spawn(move || {
            run_audio_capture(fft_data_clone);
        });

        Self {
            settings: Settings::default(),
            audio_fft_data,
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
            let mut bars = vec![0.0; 64];
            let chunk_size = data.len().max(1) / 64;
            for (i, bar) in bars.iter_mut().enumerate() {
                let start = i * chunk_size;
                let end = (start + chunk_size).min(data.len());
                if start < end {
                    let sum: f32 = data[start..end].iter().map(|v| v.abs()).sum();
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
        // Transparent background styling
        let mut style = (*ctx.style()).clone();
        style.visuals.window_fill = egui::Color32::from_black_alpha(150);
        ctx.set_style(style);

        // Top-Right Album Art & Song Info Widget
        egio_top_right_panel(ctx, &self.track_info, &mut self.settings);

        // Bottom Visualizer Bars
        let bars = self.audio_fft_data.lock().unwrap().clone();
        let track = self.track_info.lock().unwrap();
        
        egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| {
            let rect = ui.max_rect();
            let painter = ui.painter();
            
            let bar_width = self.settings.bar_width;
            let bar_gap = self.settings.bar_gap;
            let total_bars = bars.len();
            let total_width = total_bars as f32 * (bar_width + bar_gap);
            let start_x = rect.right() - total_width - 40.0;
            let base_y = rect.bottom() - 20.0;

            for (i, &val) in bars.iter().enumerate() {
                let x = start_x + i as f32 * (bar_width + bar_gap);
                let height = val * self.settings.visualizer_height * (self.settings.sensitivity / 30.0);
                let bar_rect = egui::Rect::from_min_size(
                    egui::pos2(x, base_y - height),
                    egui::vec2(bar_width, height),
                );
                
                painter.rect_filled(
                    bar_rect,
                    2.0,
                    track.dominant_color,
                );
            }
        });

        // Settings Menu Modal if toggled
        if self.settings.show_settings {
            egui::Window::new("Visualizer Settings")
                .open(&mut self.settings.show_settings)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.heading("General & Visualizer Setup");
                    ui.add(egui::Slider::new(&mut self.settings.bar_width, 2.0..=20.0).text("Bar width"));
                    ui.add(egui::Slider::new(&mut self.settings.bar_gap, 0.0..=10.0).text("Bar gap"));
                    ui.add(egui::Slider::new(&mut self.settings.sensitivity, 10.0..=100.0).text("Sensitivity"));
                    ui.add(egui::Slider::new(&mut self.settings.visualizer_height, 50.0..=400.0).text("Visualizer height"));
                    
                    ui.separator();
                    ui.label("Dynamic Colors");
                    ui.radio_value(&mut self.settings.dynamic_mode, DynamicColorMode::Off, "Off");
                    ui.radio_value(&mut self.settings.dynamic_mode, DynamicColorMode::Cover, "Cover Art Accent");
                    ui.radio_value(&mut self.settings.dynamic_mode, DynamicColorMode::Windows, "Windows Theme Accent");

                    ui.separator();
                    let mut click_through_val = self.settings.click_through;
                    if ui.checkbox(&mut click_through_val, "Click-Through Desktop Mode").changed() {
                        self.settings.click_through = click_through_val;
                        // Win32 API window style toggle for click-through can be hooked here
                    }
                });
        }
    }
}

fn egio_top_right_panel(ctx: &egui::Context, track_info: &Arc<Mutex<TrackInfo>>, settings: &mut Settings) {
    egio_window_builder(ctx, "song_info_window", egui::Align2::RIGHT_TOP, |ui| {
        let track = track_info.lock().unwrap();
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(&track.title).size(18.0).strong().color(egui::Color32::WHITE));
                ui.label(egui::RichText::new(&track.artist).size(14.0).color(egui::Color32::LIGHT_GRAY));
            });
            if let Some(tex) = &track.cover_texture {
                ui.image((tex.id(), egui::vec2(70.0, 70.0)));
            } else {
                let (_, rect) = ui.allocate_space(egui::vec2(70.0, 70.0));
                ui.painter().rect_filled(rect, 4.0, egui::Color32::DARK_GRAY);
            }
        });
        
        if ui.small_button("⚙ Settings").clicked() {
            settings.show_settings = !settings.show_settings;
        }
    });
}

fn egio_window_builder(ctx: &egui::Context, id: &str, align: egui::Align2, add_contents: impl FnOnce(&mut egui::Ui)) {
    let screen_rect = ctx.screen_rect();
    let pos = match align {
        egui::Align2::RIGHT_TOP => egui::pos2(screen_rect.right() - 250.0, 20.0),
        _ => egui::pos2(20.0, 20.0),
    };
    egui::Window::new(id)
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .fixed_pos(pos)
        .frame(egui::Frame::none().fill(egui::Color32::TRANSPARENT))
        .show(ctx, add_contents);
}

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_transparent(true)
            .with_decorations(false)
            .with_always_on_bottom(true)
            .with_inner_size([1920.0, 1080.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Music Visualizer Rust",
        options,
        Box::new(|cc| Ok(Box::new(VisualizerApp::new(cc)))),
    )
}
