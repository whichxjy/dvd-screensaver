//! DVD Screensaver with Musical Logo Animation
//!
//! This application creates an animated screensaver featuring:
//! - A classic DVD logo that bounces around the screen and changes color on collision
//! - A musical logo that moves in sync with a 4-bar rhythm pattern at 136 BPM
//!
//! Supports both real-time display and video export modes.

use nannou::image::{self, DynamicImage, GenericImageView, RgbaImage};
use nannou::prelude::*;
use nannou::rand::{thread_rng, Rng};
use std::env;
use std::process::{exit, Command, Stdio};
use std::io::Write;
use std::sync::mpsc;
use std::thread;

// Logo scale factors
const DVD_LOGO_SCALE: f32 = 3.0;
const MUSICAL_LOGO_SCALE: f32 = 2.3;

// Video output settings
const VIDEO_WIDTH: u32 = 540;
const VIDEO_HEIGHT: u32 = 960;
const VIDEO_ASPECT_RATIO: f32 = 9.0 / 16.0;
const FPS: u32 = 60;
const DURATION_SECONDS: u32 = 30;

// Musical timing constants
const BPM: f32 = 136.0;
const BEATS_PER_MEASURE: f32 = 8.0;
const MEASURES_PER_CYCLE: f32 = 4.0;

// Color rotation range for DVD logo (in degrees)
const COLOR_HUE_MIN: i32 = 120;
const COLOR_HUE_MAX: i32 = 240;

/// Type of logo being rendered
#[derive(Clone, Copy, PartialEq)]
enum LogoType {
    Dvd,
    Musical,
}

/// State machine for musical logo movement pattern
#[derive(Clone, Copy, PartialEq)]
enum MusicalState {
    Waiting,
    MovingToBottom,
    WaitingAtBottom,
    MovingToRight,
    WaitingAtRight,
    MovingToTop,
    WaitingAtTop,
    MovingToLeft,
    WaitingAtLeft,
}

/// Logo entity containing image data and movement properties
struct Logo {
    image: DynamicImage,
    texture: Option<wgpu::Texture>,
    rect: Rect,
    velocity: Vec2,
    logo_type: LogoType,

    // Musical logo specific properties
    musical_state: MusicalState,
    start_pos: Vec2,
    target_pos: Vec2,
    movement_progress: f32,
    hit_positions: [Vec2; 4], // Left, Bottom, Right, Top positions

    // Performance optimization: pre-converted RGBA image
    cached_rgba_image: Option<RgbaImage>,
}

/// Application model containing all state
struct Model {
    logos: Vec<Logo>,
    start_time: f32,
    beat_duration: f32,

    // Background rendering
    background_texture: Option<wgpu::Texture>,
    background_rgba: Option<RgbaImage>,

    // Video recording state
    recording: bool,
    frame_count: u32,
    max_frames: u32,

    // Multi-threaded video encoding
    frame_sender: Option<mpsc::Sender<Vec<u8>>>,
    encoder_handle: Option<thread::JoinHandle<()>>,
}

fn main() {
    let args: Vec<String> = env::args().collect();

    // Check for video mode flag
    if args.len() > 1 {
        match args[1].as_str() {
            "--video" | "-v" => println!("Starting in video generation mode..."),
            "/c" | "/p" => {
                println!("Configuration menu and preview mode are not implemented");
                exit(0);
            }
            _ => {}
        }
    }

    nannou::app(model).update(update).run();
}

/// Apply random hue rotation to create color variation
fn change_color(image: &DynamicImage) -> DynamicImage {
    let hue_shift = thread_rng().gen_range(COLOR_HUE_MIN..COLOR_HUE_MAX);
    image.huerotate(hue_shift)
}

/// Create a DVD logo with classic bouncing behavior
fn create_dvd_logo(
    app: Option<&App>,
    win_rect: &Rect,
    initial_pos: Vec2,
    velocity: Vec2,
) -> Logo {
    let img_data = include_bytes!("../assets/dvd_logo.png");

    // Scale logo to appropriate size
    let target_width = (win_rect.w() / DVD_LOGO_SCALE) as u32;
    let target_height = (win_rect.h() / DVD_LOGO_SCALE) as u32;

    let image = change_color(&image::load_from_memory(img_data)
        .expect("Failed to load DVD logo")
        .thumbnail(target_width, target_height));

    let (width, height) = image.dimensions();
    let rect = Rect::from_x_y_w_h(
        initial_pos.x,
        initial_pos.y,
        width as f32,
        height as f32,
    );

    // Pre-convert to RGBA for faster rendering
    let cached_rgba_image = Some(image.to_rgba8());

    // Create GPU texture only in display mode
    let texture = app.map(|a| wgpu::Texture::from_image(a, &image));

    Logo {
        image,
        texture,
        rect,
        velocity,
        logo_type: LogoType::Dvd,
        musical_state: MusicalState::Waiting,
        start_pos: Vec2::ZERO,
        target_pos: Vec2::ZERO,
        movement_progress: 0.0,
        hit_positions: [Vec2::ZERO; 4],
        cached_rgba_image,
    }
}

/// Generate randomized hit positions for musical logo movement
///
/// The positions are:
/// - [0]: Left edge (fixed X, slightly randomized Y)
/// - [1]: Bottom edge (randomized X, fixed Y)
/// - [2]: Right edge (fixed X, randomized Y)
/// - [3]: Top edge (randomized X, fixed Y)
fn generate_hit_positions(win: &Rect, logo_size: (u32, u32)) -> [Vec2; 4] {
    let mut rng = thread_rng();
    let logo_w = logo_size.0 as f32;
    let logo_h = logo_size.1 as f32;

    // Calculate randomization ranges (30% of available space)
    let vertical_range = win.h() - logo_h;
    let horizontal_range = win.w() - logo_w;
    let vertical_offset_range = vertical_range * 0.3;
    let horizontal_offset_range = horizontal_range * 0.3;

    [
        // Left position
        Vec2::new(
            win.left() + logo_w / 2.0,
            win.top() - win.h() / 4.0
        ),
        // Bottom position
        Vec2::new(
            rng.gen_range(-horizontal_offset_range..horizontal_offset_range),
            win.bottom() + logo_h / 2.0
        ),
        // Right position
        Vec2::new(
            win.right() - logo_w / 2.0,
            rng.gen_range(-vertical_offset_range..vertical_offset_range)
        ),
        // Top position
        Vec2::new(
            rng.gen_range(-horizontal_offset_range..horizontal_offset_range),
            win.top() - logo_h / 2.0
        ),
    ]
}

/// Create a musical logo that moves in rhythm
fn create_musical_logo(app: Option<&App>, win_rect: &Rect) -> Logo {
    let img_data = include_bytes!("../assets/stylophone.png");

    // Scale logo to appropriate size
    let target_width = (win_rect.w() / MUSICAL_LOGO_SCALE) as u32;
    let target_height = (win_rect.h() / MUSICAL_LOGO_SCALE) as u32;

    let image = image::load_from_memory(img_data)
        .expect("Failed to load musical logo")
        .thumbnail(target_width, target_height);

    let logo_size = image.dimensions();
    let hit_positions = generate_hit_positions(win_rect, logo_size);
    let initial_pos = hit_positions[0];

    let rect = Rect::from_x_y_w_h(
        initial_pos.x,
        initial_pos.y,
        logo_size.0 as f32,
        logo_size.1 as f32,
    );

    let cached_rgba_image = Some(image.to_rgba8());

    // Create GPU texture only in display mode
    let texture = app.map(|a| wgpu::Texture::from_image(a, &image));

    Logo {
        image,
        texture,
        rect,
        velocity: Vec2::ZERO,
        logo_type: LogoType::Musical,
        musical_state: MusicalState::Waiting,
        start_pos: initial_pos,
        target_pos: initial_pos,
        movement_progress: 0.0,
        hit_positions,
        cached_rgba_image,
    }
}

/// Crop an image to match the target aspect ratio
fn crop_to_aspect_ratio(image: &DynamicImage, target_aspect: f32) -> DynamicImage {
    let (img_width, img_height) = image.dimensions();
    let img_aspect = img_width as f32 / img_height as f32;

    // Return original if aspect ratio already matches
    if (img_aspect - target_aspect).abs() < 0.001 {
        return image.clone();
    }

    // Calculate new dimensions maintaining aspect ratio
    let (new_width, new_height) = if img_aspect > target_aspect {
        let new_width = (img_height as f32 * target_aspect) as u32;
        (new_width, img_height)
    } else {
        let new_height = (img_width as f32 / target_aspect) as u32;
        (img_width, new_height)
    };

    // Center crop
    let x = (img_width - new_width) / 2;
    let y = (img_height - new_height) / 2;

    image.crop_imm(x, y, new_width, new_height)
}

/// Load and prepare background image
fn load_background(app: Option<&App>) -> (Option<wgpu::Texture>, Option<RgbaImage>) {
    match std::fs::read("assets/background.png") {
        Ok(img_data) => {
            match image::load_from_memory(&img_data) {
                Ok(img) => {
                    // Crop and resize to match video dimensions
                    let cropped = crop_to_aspect_ratio(&img, VIDEO_ASPECT_RATIO);
                    let resized = cropped.resize_exact(
                        VIDEO_WIDTH,
                        VIDEO_HEIGHT,
                        image::imageops::FilterType::Triangle
                    );
                    let rgba_image = resized.to_rgba8();

                    // Create GPU texture only in display mode
                    let texture = app.map(|a| {
                        let bg_dynamic = DynamicImage::ImageRgba8(rgba_image.clone());
                        wgpu::Texture::from_image(a, &bg_dynamic)
                    });

                    (texture, Some(rgba_image))
                },
                Err(e) => {
                    eprintln!("Failed to decode background image: {}", e);
                    (None, None)
                }
            }
        },
        Err(e) => {
            eprintln!("Failed to load background image: {}", e);
            (None, None)
        }
    }
}

/// Start FFmpeg encoder in a separate thread for video generation
fn start_ffmpeg_encoder() -> (mpsc::Sender<Vec<u8>>, thread::JoinHandle<()>) {
    let (sender, receiver) = mpsc::channel::<Vec<u8>>();

    let handle = thread::spawn(move || {
        let output_path = "output_video.mp4";

        let mut child = Command::new("ffmpeg")
            .args([
                "-y",
                "-f", "rawvideo",
                "-vcodec", "rawvideo",
                "-s", &format!("{}x{}", VIDEO_WIDTH, VIDEO_HEIGHT),
                "-pix_fmt", "rgba",
                "-r", &FPS.to_string(),
                "-i", "-",
                "-c:v", "libx264",
                "-pix_fmt", "yuv420p",
                "-preset", "ultrafast",
                "-crf", "23",
                output_path,
            ])
            .stdin(Stdio::piped())
            .spawn()
            .expect("Failed to start FFmpeg. Please ensure FFmpeg is installed.");

        let mut stdin = child.stdin.take().expect("Failed to get stdin handle");

        // Process frames from the channel
        while let Ok(frame_data) = receiver.recv() {
            if stdin.write_all(&frame_data).is_err() {
                eprintln!("Failed to write frame to FFmpeg");
                break;
            }
        }

        drop(stdin);
        let _ = child.wait();
        println!("Video encoding complete! Saved to: {}", output_path);
    });

    (sender, handle)
}

/// Initialize the application model
fn model(app: &App) -> Model {
    let args: Vec<String> = env::args().collect();
    let is_video_mode = args.iter().any(|arg| arg == "--video" || arg == "-v");

    // Create window
    let primary_window_id = app
        .new_window()
        .view(view)
        .size(VIDEO_WIDTH, VIDEO_HEIGHT)
        .title("DVD Screensaver - Mobile Format (9:16)")
        .build()
        .unwrap();

    // Hide cursor in display mode
    if !is_video_mode {
        app.window(primary_window_id)
            .unwrap()
            .set_cursor_visible(false);
    }

    let win_rect = Rect::from_w_h(VIDEO_WIDTH as f32, VIDEO_HEIGHT as f32);

    // Initialize logos
    let app_ref = if is_video_mode { None } else { Some(app) };
    let logos = vec![
        create_dvd_logo(
            app_ref,
            &win_rect,
            Vec2::new(-100.0, 200.0),
            Vec2::new(180.0, 120.0),
        ),
        create_musical_logo(app_ref, &win_rect),
    ];

    // Calculate beat duration from BPM (eighth notes)
    let beat_duration = 60.0 / BPM / 2.0;

    // Load background
    let (background_texture, background_rgba) = load_background(app_ref);

    // Initialize video encoder if in video mode
    let (frame_sender, encoder_handle) = if is_video_mode {
        let (sender, handle) = start_ffmpeg_encoder();
        (Some(sender), Some(handle))
    } else {
        (None, None)
    };

    Model {
        logos,
        start_time: 0.0,
        beat_duration,
        background_texture,
        background_rgba,
        recording: is_video_mode,
        frame_count: 0,
        max_frames: FPS * DURATION_SECONDS,
        frame_sender,
        encoder_handle,
    }
}

/// Create a rectangle at the specified position
fn rect_at_position(rect: &Rect, pos: Vec2) -> Rect {
    Rect::from_x_y_w_h(pos.x, pos.y, rect.w(), rect.h())
}

/// Calculate current beat position within the 32-beat cycle
fn get_current_beat_in_cycle(elapsed_time: f32, beat_duration: f32) -> f32 {
    let total_cycle_duration = beat_duration * BEATS_PER_MEASURE * MEASURES_PER_CYCLE;
    let cycle_time = elapsed_time % total_cycle_duration;
    cycle_time / beat_duration
}

/// Update musical logo position based on rhythm pattern
///
/// Movement pattern (A=beat 3, B=beat 6, C=beat 5, D=beat 6):
/// - Measure 1: Wait -> Move to bottom (A->B)
/// - Measure 2: Wait -> Move to right (C->D)
/// - Measure 3: Wait -> Move to top (A->B)
/// - Measure 4: Wait -> Move to left (C->D)
fn update_musical_logo(
    logo: &mut Logo,
    elapsed_time: f32,
    beat_duration: f32,
    win: &Rect,
) {
    let current_beat = get_current_beat_in_cycle(elapsed_time, beat_duration);

    // Extract position shortcuts
    let [left_pos, bottom_pos, right_pos, top_pos] = logo.hit_positions;

    let measure = (current_beat / BEATS_PER_MEASURE) as i32 % 4;
    let beat_in_measure = current_beat % BEATS_PER_MEASURE;

    match measure {
        0 => handle_first_measure(logo, beat_in_measure, beat_duration, left_pos, bottom_pos),
        1 => handle_second_measure(logo, beat_in_measure, beat_duration, bottom_pos, right_pos),
        2 => handle_third_measure(logo, beat_in_measure, beat_duration, right_pos, top_pos),
        3 => handle_fourth_measure(logo, beat_in_measure, beat_duration, top_pos, left_pos),
        _ => {}
    }

    // Reset cycle when returning to start
    if measure == 0 && beat_in_measure < 1.0 && logo.musical_state == MusicalState::WaitingAtLeft {
        logo.musical_state = MusicalState::Waiting;
        logo.hit_positions = generate_hit_positions(win, logo.image.dimensions());
        let new_left_pos = logo.hit_positions[0];
        logo.rect = rect_at_position(&logo.rect, new_left_pos);
    }
}

/// Handle first measure movement (left -> bottom)
fn handle_first_measure(
    logo: &mut Logo,
    beat_in_measure: f32,
    beat_duration: f32,
    start_pos: Vec2,
    end_pos: Vec2,
) {
    match logo.musical_state {
        MusicalState::Waiting => {
            if (2.0..2.1).contains(&beat_in_measure) {
                logo.musical_state = MusicalState::MovingToBottom;
                logo.start_pos = start_pos;
                logo.target_pos = end_pos;
                logo.movement_progress = 0.0;
            }
        }
        MusicalState::MovingToBottom => {
            if beat_in_measure >= 5.0 {
                logo.musical_state = MusicalState::WaitingAtBottom;
                logo.rect = rect_at_position(&logo.rect, end_pos);
            } else {
                update_movement(logo, beat_in_measure - 2.0, 3.0, beat_duration);
            }
        }
        _ => {}
    }
}

/// Handle second measure movement (bottom -> right)
fn handle_second_measure(
    logo: &mut Logo,
    beat_in_measure: f32,
    beat_duration: f32,
    start_pos: Vec2,
    end_pos: Vec2,
) {
    match logo.musical_state {
        MusicalState::WaitingAtBottom => {
            if (4.0..4.1).contains(&beat_in_measure) {
                logo.musical_state = MusicalState::MovingToRight;
                logo.start_pos = start_pos;
                logo.target_pos = end_pos;
                logo.movement_progress = 0.0;
            }
        }
        MusicalState::MovingToRight => {
            if beat_in_measure >= 5.0 {
                logo.musical_state = MusicalState::WaitingAtRight;
                logo.rect = rect_at_position(&logo.rect, end_pos);
            } else {
                update_movement(logo, beat_in_measure - 4.0, 1.0, beat_duration);
            }
        }
        _ => {}
    }
}

/// Handle third measure movement (right -> top)
fn handle_third_measure(
    logo: &mut Logo,
    beat_in_measure: f32,
    beat_duration: f32,
    start_pos: Vec2,
    end_pos: Vec2,
) {
    match logo.musical_state {
        MusicalState::WaitingAtRight => {
            if (2.0..2.1).contains(&beat_in_measure) {
                logo.musical_state = MusicalState::MovingToTop;
                logo.start_pos = start_pos;
                logo.target_pos = end_pos;
                logo.movement_progress = 0.0;
            }
        }
        MusicalState::MovingToTop => {
            if beat_in_measure >= 5.0 {
                logo.musical_state = MusicalState::WaitingAtTop;
                logo.rect = rect_at_position(&logo.rect, end_pos);
            } else {
                update_movement(logo, beat_in_measure - 2.0, 3.0, beat_duration);
            }
        }
        _ => {}
    }
}

/// Handle fourth measure movement (top -> left)
fn handle_fourth_measure(
    logo: &mut Logo,
    beat_in_measure: f32,
    beat_duration: f32,
    start_pos: Vec2,
    end_pos: Vec2,
) {
    match logo.musical_state {
        MusicalState::WaitingAtTop => {
            if (4.0..4.1).contains(&beat_in_measure) {
                logo.musical_state = MusicalState::MovingToLeft;
                logo.start_pos = start_pos;
                logo.target_pos = end_pos;
                logo.movement_progress = 0.0;
            }
        }
        MusicalState::MovingToLeft => {
            if beat_in_measure >= 5.0 {
                logo.musical_state = MusicalState::WaitingAtLeft;
                logo.rect = rect_at_position(&logo.rect, end_pos);
            } else {
                update_movement(logo, beat_in_measure - 4.0, 1.0, beat_duration);
            }
        }
        _ => {}
    }
}

/// Update logo position during movement
fn update_movement(
    logo: &mut Logo,
    elapsed_beats: f32,
    duration_beats: f32,
    beat_duration: f32,
) {
    let movement_duration = duration_beats * beat_duration;
    let movement_elapsed = elapsed_beats * beat_duration;
    logo.movement_progress = (movement_elapsed / movement_duration).min(1.0);

    let current_pos = logo.start_pos.lerp(logo.target_pos, logo.movement_progress);
    logo.rect = rect_at_position(&logo.rect, current_pos);
}

/// Update DVD logo physics
fn update_dvd_logo(
    app: &App,
    logo: &mut Logo,
    win: &Rect,
    delta_time: f32,
    recording: bool,
) {
    let new_x = logo.rect.x() + logo.velocity.x * delta_time;
    let new_y = logo.rect.y() + logo.velocity.y * delta_time;

    let half_width = logo.rect.w() / 2.0;
    let half_height = logo.rect.h() / 2.0;

    // Check horizontal boundaries
    let constrained_x = if new_x - half_width <= win.left() {
        logo.velocity.x = logo.velocity.x.abs();
        recolor_logo(app, logo, recording);
        win.left() + half_width
    } else if new_x + half_width >= win.right() {
        logo.velocity.x = -logo.velocity.x.abs();
        recolor_logo(app, logo, recording);
        win.right() - half_width
    } else {
        new_x
    };

    // Check vertical boundaries
    let constrained_y = if new_y - half_height <= win.bottom() {
        logo.velocity.y = logo.velocity.y.abs();
        recolor_logo(app, logo, recording);
        win.bottom() + half_height
    } else if new_y + half_height >= win.top() {
        logo.velocity.y = -logo.velocity.y.abs();
        recolor_logo(app, logo, recording);
        win.top() - half_height
    } else {
        new_y
    };

    logo.rect = Rect::from_x_y_w_h(
        constrained_x,
        constrained_y,
        logo.rect.w(),
        logo.rect.h(),
    );
}

/// Apply color change to logo on collision
fn recolor_logo(app: &App, logo: &mut Logo, recording: bool) {
    logo.image = change_color(&logo.image);
    logo.cached_rgba_image = Some(logo.image.to_rgba8());
    if !recording {
        logo.texture = Some(wgpu::Texture::from_image(app, &logo.image));
    }
}

/// Main update function
fn update(app: &App, model: &mut Model, _update: Update) {
    // Check if video generation is complete
    if model.recording && model.frame_count >= model.max_frames {
        // Signal encoder thread and wait for completion
        drop(model.frame_sender.take());
        if let Some(handle) = model.encoder_handle.take() {
            let _ = handle.join();
        }
        app.quit();
        return;
    }

    // Initialize start time on first frame
    if model.start_time == 0.0 {
        model.start_time = if model.recording {
            -0.001 // Ensures elapsed_time starts at 0 in video mode
        } else {
            app.time
        };
    }

    // Get window dimensions
    let win = if model.recording {
        Rect::from_w_h(VIDEO_WIDTH as f32, VIDEO_HEIGHT as f32)
    } else {
        app.window_rect()
    };

    // Calculate time values
    let delta_time = if model.recording {
        1.0 / FPS as f32
    } else {
        app.duration.since_prev_update.secs() as f32
    };

    let current_time = if model.recording {
        model.frame_count as f32 / FPS as f32
    } else {
        app.time
    };

    let elapsed_time = current_time - model.start_time;

    // Update all logos
    for logo in &mut model.logos {
        match logo.logo_type {
            LogoType::Dvd => update_dvd_logo(app, logo, &win, delta_time, model.recording),
            LogoType::Musical => update_musical_logo(logo, elapsed_time, model.beat_duration, &win),
        }
    }

    // Handle video recording
    if model.recording {
        let buffer = render_frame_to_buffer(model, VIDEO_WIDTH, VIDEO_HEIGHT);
        if let Some(ref sender) = model.frame_sender {
            let _ = sender.send(buffer);
        }

        model.frame_count += 1;

        // Progress indicator
        if model.frame_count.is_multiple_of(FPS) {
            println!("Rendered {} seconds of video...", model.frame_count / FPS);
        }
    }
}

/// Software renderer for video generation
fn render_frame_to_buffer(model: &Model, width: u32, height: u32) -> Vec<u8> {
    // Start with background or blank canvas
    let mut img_buffer = model.background_rgba.clone()
        .unwrap_or_else(|| RgbaImage::new(width, height));

    // Composite logos onto background
    for logo in &model.logos {
        if let Some(ref rgba_img) = logo.cached_rgba_image {
            // Convert from centered coordinates to top-left origin
            let logo_x = ((logo.rect.x() + width as f32 / 2.0) - logo.rect.w() / 2.0) as i32;
            let logo_y = ((height as f32 / 2.0 - logo.rect.y()) - logo.rect.h() / 2.0) as i32;

            // Only draw if within bounds
            if logo_x >= 0 && logo_y >= 0 {
                image::imageops::overlay(&mut img_buffer, rgba_img, logo_x as u32, logo_y as u32);
            }
        }
    }

    img_buffer.into_raw()
}

/// GPU rendering function for display mode
fn view(app: &App, model: &Model, frame: Frame) {
    // Skip GPU rendering in video mode
    if model.recording {
        return;
    }

    let draw = app.draw();

    // Draw background
    if let Some(ref bg_texture) = model.background_texture {
        draw.texture(bg_texture)
            .xy(Vec2::ZERO)
            .wh(app.window_rect().wh());
    } else {
        frame.clear(BLACK);
    }

    // Draw logos
    for logo in &model.logos {
        if let Some(ref texture) = logo.texture {
            draw.texture(&texture)
                .xy(logo.rect.xy())
                .wh(logo.rect.wh());
        }
    }

    draw.to_frame(app, &frame).unwrap();
}
