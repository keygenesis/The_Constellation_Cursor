//! A Universal Constellation System Cursor
//!
//! LD_PRELOAD library that piledrives DRM compositor cursor operations and renders
//! its own vector cursor directly to hardware.
//!
//! # Usage
//!
//! ```bash
//! LD_PRELOAD=/path/to/libdrm_constellation_cursor.so hyprland
//! ```
//!
//! # Debug mode
//!
//! ```bash
//! CONSTELLATION_CURSOR_DEBUG=1 LD_PRELOAD=... YourCompositor
//! ```
//!
//! # Features
//!
//! - `constellation` Enable full Constellation vector rendering (requires the constellation engine)
//!
//! Without the `constellation` feature, renders traditional simple vectors for your cursor.
//! With the feature enabled, uses full Constellation rendering for advanced vector cursors.
//!
//! # Sexy Hardware Cursor Plane Benefits
//!
//! - Always on top of everything (spaking directly to display hardware)
//! - Input passthrough (doesn't capture naughty clicks)
//! - Resolution independent, as one woul expect (vector-based)
//! - Should Work with any Wayland compositor (More hardware dependant)
//!
//! # Multi-Cursor Support Architecture (Needs to be signaled manualy for now)
//!
//! Currently, this library renders a single cursor shape unless you tell it to change.
//! Full multi-cursor support (arrow/pointer/I-beam/wait/etc.) would require detecting
//! which cursor shape the compositor intends to display, which I might tackle at a later date.
//!
//! ## The Challenge
//!
//! When a compositor sets a cursor, it typically:
//! 1. Creates a wl_buffer from cursor theme (xcursor)
//! 2. Calls `wl_pointer.set_cursor()` with that buffer
//! 3. The compositor maps this to a DRM framebuffer
//! 4. We intercept at the DRM level via `drmModeAtomicAddProperty(FB_ID)`
//!
//! At the DRM level, we only see framebuffer IDs, not cursor semantic types.
//! The compositor's intent (pointer vs I-beam vs wait) seems to be lost by the time we intercept.
//!
//! ## Some Workarounds
//!
//! **1. Manual Signaling (Current approach)**
//! - Signal the cursor directly with commands like `echo "wait" > /tmp/constellation_cursor_type`
//! - And refresh it by using `touch /tmp/constellation_cursor_refresh`
//! - this also works with scale `echo "5.2" > /tmp/constellation_cursor_scale`
//!
//! **2. Wayland Protocol Interception**
//! - intercept `wl_pointer.set_cursor()` at the libwayland level
//! - Track the cursor shape name/type from the cursor theme
//! - Use this to inform which vector cursor to render
//! - But this would require additional LD_PRELOAD hooks for libwayland-client
//!
//! **3. X Cursor Theme Parsing**
//! - Read xcursor files directly to understand shape → buffer mapping
//! - Track which xcursor shape was loaded for which buffer handle
//! - Requires parsing XDG cursor theme directories, which might be easier
//!
//! **4. Compositor-Specific Integration**
//! - Work with compositor developers to expose cursor type via environment/IPC
//! - Each compositor could set e.g. `CURSOR_TYPE=pointer` before atomic commits
//! - Most intrusive but most reliable
//!
//!
//! ## Current Implementation
//!
//! For now, we render a single arrow cursor unless instructed otherwise. Users can customize by:
//! - Using `cursor_designer.html` to create new cursor shapes
//! - Editing `render_arrow_cursor()` with their preferred design
//! - Rebuilding the library
//!
//! ## Application-Controlled Cursor Types
//!
//! As mentioned above Applications can control the cursor shape via:
//!
//! **Environment variable:**
//! ```bash
//! CONSTELLATION_CURSOR_TYPE=pointer LD_PRELOAD=... YourCompositor
//! ```
//!
//! **Runtime file (for dynamic switching):**
//! ```bash
//! echo "text" > /tmp/constellation_cursor_type
//! ```
//!
//! Available types: `default`, `pointer`, `text`, `crosshair`, `wait`, `grab`, `not-allowed`
//!
//! This enables applications to signal cursor changes without compositor integration.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

const VERSION: &str = env!("CARGO_PKG_VERSION");

static DEBUG: AtomicBool = AtomicBool::new(false);
static DEBUG_CHECKED: AtomicBool = AtomicBool::new(false);
static VERSION_PRINTED: AtomicBool = AtomicBool::new(false);

fn print_version_info() {
    if VERSION_PRINTED.swap(true, Ordering::SeqCst) {
        return;
    }
    eprintln!();
    eprintln!("  The Constellation Cursor v{}", VERSION);
    eprintln!("  ─────────────────────────────────────");
    eprintln!("  Intercepted DRM calls:");
    eprintln!("    ioctl                     MODE_CURSOR, MODE_CURSOR2");
    eprintln!("    drmModeSetCursor          legacy cursor set");
    eprintln!("    drmModeSetCursor2         legacy cursor set v2");
    eprintln!("    drmModeMoveCursor         cursor position update");
    eprintln!("    drmModeGetPlane           cursor plane detection");
    eprintln!("    drmModeAtomicAddProperty  FB_ID replacement");
    eprintln!();
    eprintln!("  Environment variables:");
    eprintln!("    CONSTELLATION_CURSOR_DEBUG=1  verbose logging");
    eprintln!("    CONSTELLATION_CURSOR_INFO=1   show this info");
    eprintln!("    CONSTELLATION_CURSOR_FADE=1   fade out when hiding");
    eprintln!();
}

fn debug_enabled() -> bool {
    if !DEBUG_CHECKED.load(Ordering::Relaxed) {
        let debug = std::env::var("CONSTELLATION_CURSOR_DEBUG").is_ok();
        let info = std::env::var("CONSTELLATION_CURSOR_INFO").is_ok();

        if debug || info {
            print_version_info();
        }

        DEBUG.store(debug, Ordering::Relaxed);
        DEBUG_CHECKED.store(true, Ordering::Relaxed);
    }
    DEBUG.load(Ordering::Relaxed)
}

// The Constellation cursor has a configuration in .config

/// Load config from ~/.config/constellation_cursor/cursor.conf
/// Config format is simple key=value pairs:
///   fade_enabled=true
///   fade_speed=30
///   frost_intensity=100
///   hotspot_smoothing=true
///   hotspot_threshold=5
fn load_config() {
    if CONFIG_LOADED.load(Ordering::Relaxed) {
        return;
    }
    CONFIG_LOADED.store(true, Ordering::Relaxed);

    let config_path = if let Ok(home) = std::env::var("HOME") {
        format!("{}/.config/constellation_cursor/cursor.conf", home)
    } else {
        return; // No HOME, can't find config
    };

    let contents = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => {
            // Config doesn't exist, so we create the default one
            let default_config = r#"# Constellation Cursor Config
# Edit this file to customize cursor behavior
#
# Changes are detected automatically when you save this file.
# To manually refresh use: touch /tmp/constellation_cursor_refresh

# Cursor size multiplier (default 1.5)
cursor_scale=1.5

# Outline thickness override (0 = use cursor default, 0.5-5.0 for custom)
# outline_thickness=0

# Enable fade-out effect when cursor hides (runs in background thread)
# (Buggy)
fade_enabled=false

# Enable fade-in effect when cursor appears
# (Buggy)
fade_in_enabled=false

# Fade speed (1-255, higher = faster fade)
fade_speed=30

# Frosted glass intensity (0-100)
# (Doesn't look great at the moment)
frost_intensity=0

# Smooth hotspot transitions between cursor types
# For positional syncing, is likely to cause issues if not needed
hotspot_smoothing=false

# Threshold for hotspot change detection (pixels)
hotspot_threshold=0

# --- Config Hot-Reload Settings ---
# The cursor library can automatically detect when this file changes.
# Set to false to disable automatic reloading (saves a tiny bit of CPU).
# To re-enable: edit this file to set config_polling=true, then run:
#   touch /tmp/constellation_cursor_refresh
config_polling=true

# How often to check for config changes (number of cursor moves between checks)
# Lower = more responsive, Higher = less CPU. Default: 50
config_poll_interval=50
"#;
            let config_dir = format!(
                "{}/.config/constellation_cursor",
                std::env::var("HOME").unwrap_or_default()
            );
            let _ = std::fs::create_dir_all(&config_dir);
            let _ = std::fs::write(&config_path, default_config);
            return;
        }
    };

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            match key {
                "fade_enabled" => {
                    let enabled = value == "true" || value == "1";
                    CONFIG_FADE_ENABLED.store(enabled, Ordering::Relaxed);
                    CURSOR_FADE_ENABLED.store(enabled, Ordering::Relaxed);
                }
                "fade_in_enabled" => {
                    let enabled = value == "true" || value == "1";
                    CONFIG_FADE_IN_ENABLED.store(enabled, Ordering::Relaxed);
                }
                "fade_speed" => {
                    if let Ok(speed) = value.parse::<u32>() {
                        CONFIG_FADE_SPEED.store(speed.clamp(1, 255), Ordering::Relaxed);
                    }
                }
                "frost_intensity" => {
                    if let Ok(intensity) = value.parse::<u32>() {
                        CONFIG_FROST_INTENSITY.store(intensity.clamp(0, 100), Ordering::Relaxed);
                    }
                }
                "hotspot_smoothing" => {
                    let enabled = value == "true" || value == "1";
                    CONFIG_HOTSPOT_SMOOTHING.store(enabled, Ordering::Relaxed);
                }
                "hotspot_threshold" => {
                    if let Ok(threshold) = value.parse::<i32>() {
                        CONFIG_HOTSPOT_THRESHOLD.store(threshold.clamp(0, 50), Ordering::Relaxed);
                    }
                }
                "cursor_scale" => {
                    if let Ok(scale) = value.parse::<f32>() {
                        // Store as integer * 100 for atomic storage
                        let scale_int = (scale.clamp(0.5, 10.0) * 100.0) as u32;
                        CONFIG_CURSOR_SCALE.store(scale_int, Ordering::Relaxed);
                    }
                }
                "outline_thickness" => {
                    if let Ok(thickness) = value.parse::<f32>() {
                        // Store as integer * 10 for atomic storage (0 = use default)
                        let thickness_int = (thickness.clamp(0.0, 5.0) * 10.0) as u32;
                        CONFIG_OUTLINE_THICKNESS.store(thickness_int, Ordering::Relaxed);
                    }
                }
                "config_polling" => {
                    let enabled = value == "true" || value == "1";
                    CONFIG_POLLING_ENABLED.store(enabled, Ordering::Relaxed);
                }
                "config_poll_interval" => {
                    if let Ok(interval) = value.parse::<u32>() {
                        CONFIG_POLL_INTERVAL.store(interval.clamp(1, 1000), Ordering::Relaxed);
                    }
                }
                _ => {} // Unknown key, ignore
            }
        }
    }

    // Store the config file's mtime for hot-reload detection
    if let Ok(home) = std::env::var("HOME") {
        let config_path = format!("{}/.config/constellation_cursor/cursor.conf", home);
        if let Ok(metadata) = std::fs::metadata(&config_path) {
            if let Ok(modified) = metadata.modified() {
                if let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) {
                    CONFIG_LAST_MTIME.store(duration.as_secs(), Ordering::Relaxed);
                }
            }
        }
    }
}

unsafe fn check_config_changed() -> bool {
    if !CONFIG_POLLING_ENABLED.load(Ordering::Relaxed) {
        return false;
    }

    let interval = CONFIG_POLL_INTERVAL.load(Ordering::Relaxed).max(1);
    let counter = CONFIG_CHECK_COUNTER.fetch_add(1, Ordering::Relaxed);
    if counter % interval != 0 {
        return false;
    }

    let config_path = if let Ok(home) = std::env::var("HOME") {
        format!("{}/.config/constellation_cursor/cursor.conf", home)
    } else {
        return false;
    };

    let current_mtime = if let Ok(metadata) = std::fs::metadata(&config_path) {
        if let Ok(modified) = metadata.modified() {
            if let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) {
                duration.as_secs()
            } else {
                return false;
            }
        } else {
            return false;
        }
    } else {
        return false;
    };

    let last_mtime = CONFIG_LAST_MTIME.load(Ordering::Relaxed);
    if current_mtime > last_mtime && last_mtime > 0 {
        if DEBUG.load(Ordering::Relaxed) {
            eprintln!("[constellation-cursor] Config file changed, reloading...");
        }
        CONFIG_LOADED.store(false, Ordering::Relaxed);
        CURSOR_FADE_CHECKED.store(false, Ordering::Relaxed);
        load_config();

        if INITIALIZED.load(Ordering::SeqCst) && !CURSOR_BUFFER.is_null() {
            render_cursor();
        }
        return true;
    }

    false
}

/// Check if cursor fade effect is enabled
/// Internally praying doesn't look like sphincter ejecta
fn cursor_fade_enabled() -> bool {
    // Load config if not already done
    load_config();

    if !CURSOR_FADE_CHECKED.load(Ordering::Relaxed) {
        // Environment variable takes priority over config
        let fade = std::env::var("CONSTELLATION_CURSOR_FADE").is_ok()
            || CONFIG_FADE_ENABLED.load(Ordering::Relaxed);
        CURSOR_FADE_ENABLED.store(fade, Ordering::Relaxed);
        CURSOR_FADE_CHECKED.store(true, Ordering::Relaxed);
    }
    CURSOR_FADE_ENABLED.load(Ordering::Relaxed)
}

macro_rules! debug_print {
    ($($arg:tt)*) => {
        if debug_enabled() {
            eprintln!("[constellation-cursor] {}", format!($($arg)*));
        }
    };
}

// DRM ioctl codes
const DRM_IOCTL_MODE_CURSOR: libc::c_ulong = 0xC01C64A3;
const DRM_IOCTL_MODE_CURSOR2: libc::c_ulong = 0xC03064BB;
const DRM_IOCTL_MODE_CREATE_DUMB: libc::c_ulong = 0xC02064B2;
const DRM_IOCTL_MODE_MAP_DUMB: libc::c_ulong = 0xC01064B3;
const DRM_IOCTL_MODE_DESTROY_DUMB: libc::c_ulong = 0xC00464B4;
const DRM_IOCTL_MODE_ADDFB2: libc::c_ulong = 0xC04064B8;

const DRM_PLANE_TYPE_CURSOR: u64 = 2;

// global state fort the cursor buffer
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static CURSOR_HANDLE: AtomicU32 = AtomicU32::new(0);
// framebuffer ID for atomic
static CURSOR_FB_ID: AtomicU32 = AtomicU32::new(0);
static CURSOR_FD: AtomicI32 = AtomicI32::new(-1);
// Match what I hope is typical compositor cursor size
static CURSOR_WIDTH: AtomicU32 = AtomicU32::new(256);
static CURSOR_HEIGHT: AtomicU32 = AtomicU32::new(256);

// Track current cursor type for the runtime switching
static CURRENT_CURSOR_TYPE: AtomicU32 = AtomicU32::new(0);

// Hotspot offset, when cursor geometry extends into "negative" space,
// we offset the render and adjust the hotspot so clicks still register correctly
static CURSOR_HOTSPOT_X: AtomicI32 = AtomicI32::new(0);
static CURSOR_HOTSPOT_Y: AtomicI32 = AtomicI32::new(0);

// applied hotspot, what we actually sent to DRM (for silky smoothing)
static APPLIED_HOTSPOT_X: AtomicI32 = AtomicI32::new(0);
static APPLIED_HOTSPOT_Y: AtomicI32 = AtomicI32::new(0);
static HOTSPOT_INITIALIZED: AtomicBool = AtomicBool::new(false);

// Cursor fade state (Still looks... well, it escapes mothers love)
static CURSOR_FADING_OUT: AtomicBool = AtomicBool::new(false);
static CURSOR_FADING_IN: AtomicBool = AtomicBool::new(false);
static CURSOR_FADE_ALPHA: AtomicU32 = AtomicU32::new(255);
static CURSOR_HIDDEN: AtomicBool = AtomicBool::new(false);
static CURSOR_VISIBLE: AtomicBool = AtomicBool::new(true);
static CURSOR_FADE_ENABLED: AtomicBool = AtomicBool::new(false);
static CURSOR_FADE_CHECKED: AtomicBool = AtomicBool::new(false);
static FADE_THREAD_RUNNING: AtomicBool = AtomicBool::new(false);

// config loaded from ~/.config/constellation_cursor/cursor.conf
static CONFIG_LOADED: AtomicBool = AtomicBool::new(false);
static CONFIG_FADE_ENABLED: AtomicBool = AtomicBool::new(false);
static CONFIG_FADE_IN_ENABLED: AtomicBool = AtomicBool::new(false);
static CONFIG_FADE_SPEED: AtomicU32 = AtomicU32::new(30);
static CONFIG_FROST_INTENSITY: AtomicU32 = AtomicU32::new(100);
static CONFIG_HOTSPOT_SMOOTHING: AtomicBool = AtomicBool::new(true);
static CONFIG_HOTSPOT_THRESHOLD: AtomicI32 = AtomicI32::new(5);
static CONFIG_CURSOR_SCALE: AtomicU32 = AtomicU32::new(150);
static CONFIG_OUTLINE_THICKNESS: AtomicU32 = AtomicU32::new(0);
static CONFIG_LAST_MTIME: AtomicU64 = AtomicU64::new(0);
static CONFIG_CHECK_COUNTER: AtomicU32 = AtomicU32::new(0);
static CONFIG_POLLING_ENABLED: AtomicBool = AtomicBool::new(true);
static CONFIG_POLL_INTERVAL: AtomicU32 = AtomicU32::new(50);

// Cursor screen position
static CURSOR_SCREEN_X: AtomicI32 = AtomicI32::new(0);
static CURSOR_SCREEN_Y: AtomicI32 = AtomicI32::new(0);

// Primary framebuffer info
static PRIMARY_FB_ID: AtomicU32 = AtomicU32::new(0);
static PRIMARY_FB_WIDTH: AtomicU32 = AtomicU32::new(0);
static PRIMARY_FB_HEIGHT: AtomicU32 = AtomicU32::new(0);
static PRIMARY_FB_STRIDE: AtomicU32 = AtomicU32::new(0);
static mut PRIMARY_FB_BUFFER: *mut u32 = std::ptr::null_mut();

// mmap'd
static mut CURSOR_BUFFER: *mut u32 = std::ptr::null_mut();

// Property IDs for cursor planes, tracking these sneaky bastards
static mut CURSOR_FB_PROP_IDS: [u32; 8] = [0; 8];
static mut CURSOR_SRC_W_PROP_IDS: [u32; 8] = [0; 8];
static mut CURSOR_SRC_H_PROP_IDS: [u32; 8] = [0; 8];
static mut CURSOR_CRTC_W_PROP_IDS: [u32; 8] = [0; 8];
static mut CURSOR_CRTC_H_PROP_IDS: [u32; 8] = [0; 8];

// The actual display size for our cursor (content is ~32x48, use 64x64 for compatibility)
const CURSOR_DISPLAY_SIZE: u32 = 64;

static mut REAL_IOCTL: Option<unsafe extern "C" fn(i32, libc::c_ulong, ...) -> i32> = None;

#[repr(C)]
#[derive(Default)]
struct DrmModeCreateDumb {
    height: u32,
    width: u32,
    bpp: u32,
    flags: u32,
    handle: u32,
    pitch: u32,
    size: u64,
}

#[repr(C)]
#[derive(Default)]
struct DrmModeMapDumb {
    handle: u32,
    pad: u32,
    offset: u64,
}

#[repr(C)]
struct DrmModeCursor2 {
    flags: u32,
    crtc_id: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    handle: u32,
    hot_x: i32,
    hot_y: i32,
}

#[repr(C)]
#[derive(Default)]
struct DrmModeFB2 {
    fb_id: u32,
    width: u32,
    height: u32,
    pixel_format: u32,
    flags: u32,
    handles: [u32; 4],
    pitches: [u32; 4],
    offsets: [u32; 4],
    modifier: [u64; 4],
}

// DRM format codes
// 'A' 'R' '2' '4' in little-endian
const DRM_FORMAT_ARGB8888: u32 = 0x34325241;

// flags
const DRM_MODE_CURSOR_BO: u32 = 0x01;
const DRM_MODE_CURSOR_MOVE: u32 = 0x02;

unsafe fn init_real_functions() {
    if REAL_IOCTL.is_none() {
        let sym = libc::dlsym(libc::RTLD_NEXT, b"ioctl\0".as_ptr() as *const i8);
        if !sym.is_null() {
            REAL_IOCTL = Some(std::mem::transmute(sym));
        }
    }
}

unsafe fn real_ioctl(fd: i32, request: libc::c_ulong, arg: *mut c_void) -> i32 {
    init_real_functions();
    if let Some(func) = REAL_IOCTL {
        func(fd, request, arg)
    } else {
        -1
    }
}

/// Create the poor excuse for a constellation cursor buffer on the DRM device
unsafe fn create_cursor_buffer(fd: i32, width: u32, height: u32) -> bool {
    let mut create = DrmModeCreateDumb {
        width,
        height,
        bpp: 32,
        ..Default::default()
    };

    let ret = real_ioctl(
        fd,
        DRM_IOCTL_MODE_CREATE_DUMB,
        &mut create as *mut _ as *mut c_void,
    );
    if ret < 0 {
        return false;
    }

    let mut map = DrmModeMapDumb {
        handle: create.handle,
        ..Default::default()
    };

    let ret = real_ioctl(
        fd,
        DRM_IOCTL_MODE_MAP_DUMB,
        &mut map as *mut _ as *mut c_void,
    );
    if ret < 0 {
        return false;
    }

    // mmap it
    let ptr = libc::mmap(
        std::ptr::null_mut(),
        create.size as usize,
        libc::PROT_READ | libc::PROT_WRITE,
        libc::MAP_SHARED,
        fd,
        map.offset as i64,
    );

    if ptr == libc::MAP_FAILED {
        return false;
    }

    let mut fb = DrmModeFB2 {
        width,
        height,
        pixel_format: DRM_FORMAT_ARGB8888,
        handles: [create.handle, 0, 0, 0],
        pitches: [create.pitch, 0, 0, 0],
        offsets: [0, 0, 0, 0],
        ..Default::default()
    };

    let ret = real_ioctl(fd, DRM_IOCTL_MODE_ADDFB2, &mut fb as *mut _ as *mut c_void);
    if ret < 0 {
        return false;
    }
    CURSOR_FB_ID.store(fb.fb_id, Ordering::SeqCst);

    CURSOR_BUFFER = ptr as *mut u32;
    CURSOR_HANDLE.store(create.handle, Ordering::SeqCst);
    CURSOR_FD.store(fd, Ordering::SeqCst);
    CURSOR_WIDTH.store(width, Ordering::SeqCst);
    CURSOR_HEIGHT.store(height, Ordering::SeqCst);
    INITIALIZED.store(true, Ordering::SeqCst);

    render_cursor();

    true
}

// =============================================================================
// Constellation-based cursor rendering (For when I actually finish it)
// =============================================================================

#[cfg(feature = "constellation")]
/// Render cursor using Constellation super cool vector graphics library
unsafe fn render_cursor() {
    if CURSOR_BUFFER.is_null() {
        return;
    }

    let width = CURSOR_WIDTH.load(Ordering::SeqCst) as usize;
    let height = CURSOR_HEIGHT.load(Ordering::SeqCst) as usize;

    for i in 0..(width * height) {
        *CURSOR_BUFFER.add(i) = 0x00000000;
    }

    // Use Constellation's vector rendering
    // TODO: When Constellation is integrated, use VectorGlyph/VectorPath here
    // For now, use cursor type detection with standard polygon rendering
    match get_cursor_type() {
        CursorType::Default => render_arrow_cursor(width),
        CursorType::Pointer => render_pointer_cursor(width),
        CursorType::Text => render_text_cursor(width),
        CursorType::Crosshair => render_crosshair_cursor(width),
        CursorType::Wait => render_wait_cursor(width),
        CursorType::Grab => render_grab_cursor(width),
        CursorType::NotAllowed => render_not_allowed_cursor(width),
        CursorType::Custom => render_custom_cursor(width),
    }
}

// =============================================================================
// Standalone cursor rendering (default, plain, old and kind)
// =============================================================================

/// Cursor types that can be selected via CONSTELLATION_CURSOR_TYPE env var
/// or /tmp/constellation_cursor_type file
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum CursorType {
    Default = 0,
    Pointer = 1,
    Text = 2,
    Crosshair = 3,
    Wait = 4,
    Grab = 5,
    NotAllowed = 6,
    Custom = 7,
}

impl CursorType {
    fn as_u32(self) -> u32 {
        self as u32
    }
}

/// Get the current cursor type from environment or file
/// Applications can change cursor by:
/// 1. Setting CONSTELLATION_CURSOR_TYPE=pointer (etc)
/// 2. Writing to /tmp/constellation_cursor_type
fn get_cursor_type() -> CursorType {
    if std::path::Path::new("/tmp/constellation_cursor_custom").exists() {
        return CursorType::Custom;
    }

    if let Ok(cursor_type) = std::env::var("CONSTELLATION_CURSOR_TYPE") {
        return match cursor_type.to_lowercase().as_str() {
            "pointer" | "hand" => CursorType::Pointer,
            "text" | "ibeam" | "i-beam" => CursorType::Text,
            "crosshair" | "cross" => CursorType::Crosshair,
            "wait" | "loading" | "busy" => CursorType::Wait,
            "grab" | "grabbing" => CursorType::Grab,
            "not-allowed" | "no" | "forbidden" => CursorType::NotAllowed,
            "custom" => CursorType::Custom,
            _ => CursorType::Default,
        };
    }

    if let Ok(contents) = std::fs::read_to_string("/tmp/constellation_cursor_type") {
        return match contents.trim().to_lowercase().as_str() {
            "pointer" | "hand" => CursorType::Pointer,
            "text" | "ibeam" | "i-beam" => CursorType::Text,
            "crosshair" | "cross" => CursorType::Crosshair,
            "wait" | "loading" | "busy" => CursorType::Wait,
            "grab" | "grabbing" => CursorType::Grab,
            "not-allowed" | "no" | "forbidden" => CursorType::NotAllowed,
            "custom" => CursorType::Custom,
            _ => CursorType::Default,
        };
    }

    CursorType::Default
}

/// Get cursor scale from environment or file
/// Default is 1.5, can be overridden via:
/// - CONSTELLATION_CURSOR_SCALE=2.0
/// - echo "2.0" > /tmp/constellation_cursor_scale
fn get_cursor_scale() -> f32 {
    load_config();

    if let Ok(scale_str) = std::env::var("CONSTELLATION_CURSOR_SCALE") {
        if let Ok(scale) = scale_str.parse::<f32>() {
            if scale > 0.0 && scale <= 10.0 {
                return scale;
            }
        }
    }

    if let Ok(contents) = std::fs::read_to_string("/tmp/constellation_cursor_scale") {
        if let Ok(scale) = contents.trim().parse::<f32>() {
            if scale > 0.0 && scale <= 10.0 {
                return scale;
            }
        }
    }

    let config_scale = CONFIG_CURSOR_SCALE.load(Ordering::Relaxed) as f32 / 100.0;
    if config_scale >= 0.5 && config_scale <= 10.0 {
        return config_scale;
    }

    1.5 // Default scale
}

/// Check if a refresh has been requested via /tmp/constellation_cursor_refresh
/// Apps can trigger a cursor refresh by:
///   touch /tmp/constellation_cursor_refresh
/// Or set the type and refresh in one command:
///   echo "pointer" > /tmp/constellation_cursor_type && touch /tmp/constellation_cursor_refresh
unsafe fn check_cursor_refresh() {
    const REFRESH_PATH: &str = "/tmp/constellation_cursor_refresh";

    if std::path::Path::new(REFRESH_PATH).exists() {
        let _ = std::fs::remove_file(REFRESH_PATH);

        if !INITIALIZED.load(Ordering::SeqCst) || CURSOR_BUFFER.is_null() {
            return;
        }

        CONFIG_LOADED.store(false, Ordering::Relaxed);
        CURSOR_FADE_CHECKED.store(false, Ordering::Relaxed);
        load_config();

        let new_type = get_cursor_type();
        debug_print!("Cursor refresh requested, type: {:?}", new_type.as_u32());
        CURRENT_CURSOR_TYPE.store(new_type.as_u32(), Ordering::SeqCst);
        render_cursor();
    }
}

#[cfg(not(feature = "constellation"))]
unsafe fn render_cursor() {
    if CURSOR_BUFFER.is_null() {
        return;
    }

    let width = CURSOR_WIDTH.load(Ordering::SeqCst) as usize;
    let height = CURSOR_HEIGHT.load(Ordering::SeqCst) as usize;

    for i in 0..(width * height) {
        *CURSOR_BUFFER.add(i) = 0x00000000;
    }

    match get_cursor_type() {
        CursorType::Default => render_arrow_cursor(width),
        CursorType::Pointer => render_pointer_cursor(width),
        CursorType::Text => render_text_cursor(width),
        CursorType::Crosshair => render_crosshair_cursor(width),
        CursorType::Wait => render_wait_cursor(width),
        CursorType::Grab => render_grab_cursor(width),
        CursorType::NotAllowed => render_not_allowed_cursor(width),
        CursorType::Custom => render_custom_cursor(width),
    }
}

// =============================================================================
// Cursor shape renderers
// =============================================================================

/// Transform points with scale and rotation, adjusting bounds so all geometry
/// is in positive space. Returns (transformed_points, hotspot_offset).
///
/// The first point is the logical hotspot. After transformation:
/// 1. Scale all points around the hotspot
/// 2. Apply rotation around the hotspot
/// 3. Calculate bounding box
/// 4. Offset all points so min_x and min_y are 0
/// 5. Return the hotspot offset so DRM cursor positioning works correctly
fn transform_points(
    points: &[(f32, f32)],
    scale: f32,
    rotation_deg: f32,
) -> (Vec<(f32, f32)>, (i32, i32)) {
    if points.is_empty() {
        return (Vec::new(), (0, 0));
    }

    // First point is the hotspot
    let (hx, hy) = points[0];
    let rotation_rad = rotation_deg * std::f32::consts::PI / 180.0;
    let cos_r = rotation_rad.cos();
    let sin_r = rotation_rad.sin();

    // transform all points: offset to hotspot origin, scale, rotate
    let transformed: Vec<(f32, f32)> = points
        .iter()
        .map(|(x, y)| {
            // Offset so hotspot is at origin
            let dx = (x - hx) * scale;
            let dy = (y - hy) * scale;
            // Apply rotation around origin (which is, you guessed it, the hotspot)
            let rx = dx * cos_r - dy * sin_r;
            let ry = dx * sin_r + dy * cos_r;
            (rx, ry)
        })
        .collect();

    // find bounding box
    let min_x = transformed.iter().map(|p| p.0).fold(f32::MAX, f32::min);
    let min_y = transformed.iter().map(|p| p.1).fold(f32::MAX, f32::min);

    // Offset all points so minimum is at (0, 0)
    // this ensures all geometry is in positive space
    let adjusted: Vec<(f32, f32)> = transformed
        .iter()
        .map(|(x, y)| (x - min_x, y - min_y))
        .collect();

    // The hotspot offset is how much we moved the origin
    // This is what we need to tell DRM so clicks register at the right spot
    let hotspot_x = (-min_x).round() as i32;
    let hotspot_y = (-min_y).round() as i32;

    (adjusted, (hotspot_x, hotspot_y))
}

/// Simple scale without rotation (hopeful legacy compatibility)
fn scale_points_around_hotspot(points: &[(f32, f32)], scale: f32) -> Vec<(f32, f32)> {
    let (adjusted, (hx, hy)) = transform_points(points, scale, 0.0);
    CURSOR_HOTSPOT_X.store(hx, Ordering::SeqCst);
    CURSOR_HOTSPOT_Y.store(hy, Ordering::SeqCst);
    adjusted
}

/// Default embedded cursor design, Constellation arrow cursor (not finished)
/// Simple, elegant arrow with dark fill and subtle gray outline
const EMBEDDED_CURSOR_JSON: &str = r##"{"type":"default","layers":[{"points":[{"x":0,"y":0},{"x":3,"y":18},{"x":10,"y":17.5},{"x":12.5,"y":15.5},{"x":14.5,"y":10}],"fill":"#011023","fillAlpha":91,"outline":"#948f8f","outlineWidth":2,"outlineAlpha":100,"shadow":"#000000","shadowAlpha":33,"shadowOffset":3,"blur":0,"blurOutline":false,"passthroughTo":-1}],"settings":{"scale":1.5}}"##;

/// Default arrow cursor, renders the embedded multi-layer design
unsafe fn render_arrow_cursor(stride: usize) {
    render_custom_cursor_v2(stride, EMBEDDED_CURSOR_JSON);
}

/// Pointer/hand cursor (the result of a copy/paste)
unsafe fn render_pointer_cursor(stride: usize) {
    let scale = get_cursor_scale();
    let points: [(f32, f32); 7] = [
        (0.0, 0.0),
        (0.0, 16.0),
        (4.0, 12.0),
        (6.0, 18.0),
        (9.0, 17.0),
        (7.0, 11.0),
        (12.0, 11.0),
    ];
    let scaled = scale_points_around_hotspot(&points, scale);
    draw_filled_polygon(stride, &scaled, 1.0, 1.0, 0x80000000);
    draw_filled_polygon(stride, &scaled, 0.0, 0.0, 0xFFFFFFFF);
    draw_polygon_outline(stride, &scaled, 0.0, 0.0, 0xFF000000);
}

/// Text/I-beam cursor (if we squint)
unsafe fn render_text_cursor(stride: usize) {
    let scale = get_cursor_scale();
    let points: [(f32, f32); 14] = [
        (2.0, 0.0),
        (5.0, 0.0),
        (5.0, 1.0),
        (7.0, 3.0),
        (7.0, 17.0),
        (5.0, 19.0),
        (5.0, 20.0),
        (2.0, 20.0),
        (2.0, 19.0),
        (5.0, 19.0),
        (6.0, 18.0),
        (6.0, 2.0),
        (5.0, 1.0),
        (2.0, 1.0),
    ];
    let scaled = scale_points_around_hotspot(&points, scale);
    draw_filled_polygon(stride, &scaled, 1.0, 1.0, 0x80000000);
    draw_filled_polygon(stride, &scaled, 0.0, 0.0, 0xFFFFFFFF);
    draw_polygon_outline(stride, &scaled, 0.0, 0.0, 0xFF000000);
}

/// Crosshair cursor (for precision selection, off-center, for that extra precision)
unsafe fn render_crosshair_cursor(stride: usize) {
    let scale = get_cursor_scale() * 0.8; // smaller for crosshair
    let points: [(f32, f32); 20] = [
        (8.0, 0.0),
        (8.0, 6.0),
        (6.0, 6.0),
        (6.0, 8.0),
        (0.0, 8.0),
        (0.0, 10.0),
        (6.0, 10.0),
        (6.0, 12.0),
        (8.0, 12.0),
        (8.0, 18.0),
        (10.0, 18.0),
        (10.0, 12.0),
        (12.0, 12.0),
        (12.0, 10.0),
        (18.0, 10.0),
        (18.0, 8.0),
        (12.0, 8.0),
        (12.0, 6.0),
        (10.0, 6.0),
        (10.0, 0.0),
    ];
    let scaled = scale_points_around_hotspot(&points, scale);
    draw_filled_polygon(stride, &scaled, 1.0, 1.0, 0x80000000);
    draw_filled_polygon(stride, &scaled, 0.0, 0.0, 0xFFFFFFFF);
    draw_polygon_outline(stride, &scaled, 0.0, 0.0, 0xFF000000);
}

/// Wait/loading cursor (pretend hourglass shape)
unsafe fn render_wait_cursor(stride: usize) {
    let scale = get_cursor_scale();
    let points: [(f32, f32); 8] = [
        (0.0, 0.0),
        (12.0, 0.0),
        (12.0, 3.0),
        (6.0, 9.0),
        (12.0, 15.0),
        (12.0, 18.0),
        (0.0, 18.0),
        (0.0, 15.0),
    ];
    let scaled = scale_points_around_hotspot(&points, scale);
    draw_filled_polygon(stride, &scaled, 1.0, 1.0, 0x80000000);
    draw_filled_polygon(stride, &scaled, 0.0, 0.0, 0xFFFFFFFF);
    draw_polygon_outline(stride, &scaled, 0.0, 0.0, 0xFF000000);
}

/// Grab/hand cursor (the result of my unending potential for graphical design)
unsafe fn render_grab_cursor(stride: usize) {
    let scale = get_cursor_scale() * 0.87;
    let points: [(f32, f32); 20] = [
        (6.0, 0.0),
        (6.0, 8.0),
        (8.0, 8.0),
        (8.0, 3.0),
        (10.0, 3.0),
        (10.0, 8.0),
        (12.0, 8.0),
        (12.0, 5.0),
        (14.0, 5.0),
        (14.0, 8.0),
        (16.0, 8.0),
        (16.0, 7.0),
        (18.0, 7.0),
        (18.0, 16.0),
        (12.0, 20.0),
        (4.0, 20.0),
        (0.0, 16.0),
        (0.0, 12.0),
        (4.0, 12.0),
        (4.0, 0.0),
    ];
    let scaled = scale_points_around_hotspot(&points, scale);
    draw_filled_polygon(stride, &scaled, 1.0, 1.0, 0x80000000);
    draw_filled_polygon(stride, &scaled, 0.0, 0.0, 0xFFFFFFFF);
    draw_polygon_outline(stride, &scaled, 0.0, 0.0, 0xFF000000);
}

/// Not-allowed cursor (circle with slash)
unsafe fn render_not_allowed_cursor(stride: usize) {
    let scale = get_cursor_scale();
    let radius = 9.0;

    let mut points: Vec<(f32, f32)> = Vec::new();

    let offset = radius;
    for i in 0..16 {
        let angle = (i as f32) * std::f32::consts::PI * 2.0 / 16.0;
        points.push((offset + radius * angle.cos(), offset + radius * angle.sin()));
    }

    points.insert(0, (offset, offset));

    let scaled = scale_points_around_hotspot(&points, scale);

    let circle_points: Vec<(f32, f32)> = scaled[1..].to_vec();
    draw_polygon_outline(stride, &circle_points, 1.0, 1.0, 0x80000000);
    draw_polygon_outline(stride, &circle_points, 0.0, 0.0, 0xFFFF0000);

    let slash: [(f32, f32); 5] = [
        (offset, offset),
        (offset - 6.0, offset - 6.0),
        (offset - 5.0, offset - 7.0),
        (offset + 7.0, offset + 5.0),
        (offset + 6.0, offset + 6.0),
    ];
    let scaled_slash = scale_points_around_hotspot(&slash, scale);
    let slash_points: Vec<(f32, f32)> = scaled_slash[1..].to_vec();
    draw_filled_polygon(stride, &slash_points, 0.0, 0.0, 0xFFFF0000);
}

/// Custom cursor loaded from /tmp/constellation_cursor_custom
/// File format is simple JSON:
///
/// Custom cursor format v2 (multi-layer):
/// {
///   "version": 2,
///   "scale": 1.5,
///   "rotation": 0.0,
///   "layers": [
///     {
///       "name": "Layer 1",
///       "points": [{"x": 0, "y": 0}, {"x": 1, "y": 2, "type": "curve", "cx1": ..., "cy1": ..., "cx2": ..., "cy2": ...}],
///       "fill": "#AARRGGBB",
///       "outline": "#AARRGGBB",
///       "outlineWidth": 1.0,
///       "shadow": "#AARRGGBB",
///       "shadowOffset": 1.0
///     }
///   ]
/// }
unsafe fn render_custom_cursor(stride: usize) {
    const CUSTOM_PATH: &str = "/tmp/constellation_cursor_custom";

    let content = match std::fs::read_to_string(CUSTOM_PATH) {
        Ok(c) => c,
        Err(_) => {
            render_arrow_cursor(stride);
            return;
        }
    };

    // Check for version 2 (multi-layer) format
    let version = parse_float(&content, "version").unwrap_or(1.0) as i32;

    if version >= 2 {
        render_custom_cursor_v2(stride, &content);
    } else {
        render_custom_cursor_v1(stride, &content);
    }
}

/// Render v1 format (single layer, backwards compatible for my own work, will be removed later)
unsafe fn render_custom_cursor_v1(stride: usize, content: &str) {
    let points = parse_custom_points(content);
    let fill_color = parse_color(content, "fill").unwrap_or(0xFFFFFFFF);
    let outline_color = parse_color(content, "outline").unwrap_or(0xFF000000);
    let shadow_color = parse_color(content, "shadow").unwrap_or(0x80000000);
    let custom_scale = parse_float(content, "scale").unwrap_or(1.5);
    let rotation = parse_float(content, "rotation").unwrap_or(0.0);
    let shadow_offset = parse_float(content, "shadowOffset").unwrap_or(1.0);

    if points.is_empty() {
        render_arrow_cursor(stride);
        return;
    }

    let (scaled, (hx, hy)) = transform_points(&points, custom_scale, rotation);
    CURSOR_HOTSPOT_X.store(hx, Ordering::SeqCst);
    CURSOR_HOTSPOT_Y.store(hy, Ordering::SeqCst);

    if shadow_offset > 0.0 {
        draw_filled_polygon(stride, &scaled, shadow_offset, shadow_offset, shadow_color);
    }
    draw_filled_polygon(stride, &scaled, 0.0, 0.0, fill_color);
    draw_polygon_outline(stride, &scaled, 0.0, 0.0, outline_color);

    debug_print!(
        "Rendered custom cursor v1 with {} points, rotation: {}°, hotspot: ({}, {})",
        points.len(),
        rotation,
        hx,
        hy
    );
}

/// Render v2 format (multi-layer)
unsafe fn render_custom_cursor_v2(stride: usize, content: &str) {
    let json_scale = parse_float(content, "scale").unwrap_or(1.5);
    let runtime_scale = get_cursor_scale();
    let custom_scale = json_scale * runtime_scale / 1.5;
    let rotation = parse_float(content, "rotation").unwrap_or(0.0);

    let layers = parse_layers(content);

    if layers.is_empty() {
        let scale = get_cursor_scale();
        let points: [(f32, f32); 7] = [
            (0.0, 0.0),
            (0.0, 18.0),
            (4.5, 14.0),
            (7.5, 21.0),
            (10.5, 19.5),
            (7.5, 12.0),
            (13.0, 12.0),
        ];
        let scaled = scale_points_around_hotspot(&points, scale);
        draw_filled_polygon(stride, &scaled, 1.0, 1.0, 0x80000000);
        draw_filled_polygon(stride, &scaled, 0.0, 0.0, 0xFFFFFFFF);
        draw_polygon_outline(stride, &scaled, 0.0, 0.0, 0xFF000000);
        return;
    }

    let mut all_points: Vec<(f32, f32)> = Vec::new();
    for layer in &layers {
        all_points.extend(layer.points.iter().cloned());
    }

    let (_, (hx, hy)) = transform_points(&all_points, custom_scale, rotation);
    CURSOR_HOTSPOT_X.store(hx, Ordering::SeqCst);
    CURSOR_HOTSPOT_Y.store(hy, Ordering::SeqCst);

    for (i, layer) in layers.iter().enumerate() {
        if layer.points.len() < 3 {
            continue;
        }

        let (scaled, _) = transform_points(&layer.points, custom_scale, rotation);

        let is_passthrough = layer.passthrough_to >= 0;
        if is_passthrough {
            debug_print!(
                "Layer {} is passthrough (target: {}) with blur: {}",
                i,
                layer.passthrough_to,
                layer.blur
            );

            if layer.blur != 0.0 {
                let frost_mult = CONFIG_FROST_INTENSITY.load(Ordering::Relaxed) as f32 / 100.0;
                let adjusted_blur = layer.blur * frost_mult;
                draw_frosted_glass(stride, &scaled, 0.0, 0.0, layer.fill_color, adjusted_blur);
            } else {
                let alpha = ((layer.fill_color >> 24) & 0xFF) as f32 / 255.0;
                let reduced_alpha = (alpha * 0.5 * 255.0) as u32;
                let tint_color = (reduced_alpha << 24) | (layer.fill_color & 0x00FFFFFF);
                draw_filled_polygon(stride, &scaled, 0.0, 0.0, tint_color);
            }

            if layer.outline_width > 0.0 && (layer.outline_color >> 24) > 0 {
                if layer.blur != 0.0 && layer.blur_outline {
                    draw_polygon_outline_spiral_blur(
                        stride,
                        &scaled,
                        0.0,
                        0.0,
                        layer.outline_color,
                        layer.blur,
                    );
                } else {
                    draw_polygon_outline(stride, &scaled, 0.0, 0.0, layer.outline_color);
                }
            }
            continue;
        }

        if layer.shadow_offset > 0.0 && (layer.shadow_color >> 24) > 0 {
            if layer.blur != 0.0 {
                draw_filled_polygon_spiral_blur(
                    stride,
                    &scaled,
                    layer.shadow_offset,
                    layer.shadow_offset,
                    layer.shadow_color,
                    layer.blur,
                );
            } else {
                draw_filled_polygon(
                    stride,
                    &scaled,
                    layer.shadow_offset,
                    layer.shadow_offset,
                    layer.shadow_color,
                );
            }
        }

        if (layer.fill_color >> 24) > 0 {
            if layer.blur != 0.0 {
                draw_filled_polygon_spiral_blur(
                    stride,
                    &scaled,
                    0.0,
                    0.0,
                    layer.fill_color,
                    layer.blur,
                );
            } else {
                draw_filled_polygon(stride, &scaled, 0.0, 0.0, layer.fill_color);
            }
        }
        // Blur did not work as I wanted, So a lot of this will be refactored
        if layer.outline_width > 0.0 && (layer.outline_color >> 24) > 0 {
            if layer.blur != 0.0 && layer.blur_outline {
                draw_polygon_outline_spiral_blur(
                    stride,
                    &scaled,
                    0.0,
                    0.0,
                    layer.outline_color,
                    layer.blur,
                );
            } else {
                draw_polygon_outline(stride, &scaled, 0.0, 0.0, layer.outline_color);
            }
        }

        debug_print!(
            "Rendered layer {} with {} points, blur: {}",
            i,
            layer.points.len(),
            layer.blur
        );
    }

    debug_print!(
        "Rendered custom cursor v2 with {} layers, rotation: {}°, hotspot: ({}, {})",
        layers.len(),
        rotation,
        hx,
        hy
    );
}

struct CursorLayer {
    points: Vec<(f32, f32)>,
    fill_color: u32,
    outline_color: u32,
    outline_width: f32,
    shadow_color: u32,
    shadow_offset: f32,
    blur: f32,
    blur_outline: bool,
    passthrough_to: i32,
}

fn parse_layers(content: &str) -> Vec<CursorLayer> {
    let mut layers = Vec::new();

    if let Some(layers_start) = content.find("\"layers\"") {
        if let Some(arr_start) = content[layers_start..].find('[') {
            let arr_content = &content[layers_start + arr_start..];

            let mut depth = 0;
            let mut in_layer = false;
            let mut layer_start = 0;

            for (i, c) in arr_content.chars().enumerate() {
                match c {
                    '[' if depth == 0 => depth = 1,
                    ']' if depth == 1 => break,
                    '{' => {
                        if depth == 1 {
                            in_layer = true;
                            layer_start = i;
                        }
                        depth += 1;
                    }
                    '}' => {
                        depth -= 1;
                        if depth == 1 && in_layer {
                            let layer_str = &arr_content[layer_start..=i];
                            if let Some(layer) = parse_single_layer(layer_str) {
                                layers.push(layer);
                            }
                            in_layer = false;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    layers
}

fn parse_single_layer(layer_str: &str) -> Option<CursorLayer> {
    let points = parse_layer_points(layer_str);

    if points.is_empty() {
        return None;
    }

    let fill_color_base = parse_color(layer_str, "fill").unwrap_or(0xFFFFFFFF);
    let outline_color_base = parse_color(layer_str, "outline").unwrap_or(0xFF000000);
    let shadow_color = parse_color(layer_str, "shadow").unwrap_or(0x80000000);

    let fill_alpha = parse_float(layer_str, "fillAlpha").unwrap_or(100.0);
    let outline_alpha = parse_float(layer_str, "outlineAlpha").unwrap_or(100.0);

    let fill_alpha_byte = ((fill_alpha / 100.0 * 255.0) as u32).min(255);
    let outline_alpha_byte = ((outline_alpha / 100.0 * 255.0) as u32).min(255);
    let fill_color = (fill_alpha_byte << 24) | (fill_color_base & 0x00FFFFFF);
    let outline_color = (outline_alpha_byte << 24) | (outline_color_base & 0x00FFFFFF);

    let outline_width = parse_float(layer_str, "outlineWidth").unwrap_or(1.0);
    let shadow_offset = parse_float(layer_str, "shadowOffset").unwrap_or(1.0);
    let blur = parse_float(layer_str, "blur").unwrap_or(0.0);
    let blur_outline = parse_bool(layer_str, "blurOutline").unwrap_or(false);

    let passthrough_to = if let Some(pt) = parse_int(layer_str, "passthroughTo") {
        pt
    } else if parse_bool(layer_str, "passthrough").unwrap_or(false) {
        0 // Legacy: passthrough=true means punch through to layer 0
    } else {
        -1 // Default: no passthrough
    };

    Some(CursorLayer {
        points,
        fill_color,
        outline_color,
        outline_width,
        shadow_color,
        shadow_offset,
        blur,
        blur_outline,
        passthrough_to,
    })
}

fn parse_bool(content: &str, key: &str) -> Option<bool> {
    let search_key = format!("\"{}\"", key);
    if let Some(key_pos) = content.find(&search_key) {
        let after_key = &content[key_pos + search_key.len()..];
        let trimmed = after_key.trim_start().strip_prefix(':')?.trim_start();
        if trimmed.starts_with("true") {
            return Some(true);
        } else if trimmed.starts_with("false") {
            return Some(false);
        }
    }
    None
}

fn parse_int(content: &str, key: &str) -> Option<i32> {
    let search_key = format!("\"{}\"", key);
    if let Some(key_pos) = content.find(&search_key) {
        let after_key = &content[key_pos + search_key.len()..];
        let trimmed = after_key.trim_start().strip_prefix(':')?.trim_start();

        let mut num_str = String::new();
        for c in trimmed.chars() {
            if c == '-' || c.is_ascii_digit() {
                num_str.push(c);
            } else {
                break;
            }
        }

        if !num_str.is_empty() {
            return num_str.parse().ok();
        }
    }
    None
}

/// Parse points array from layer
fn parse_layer_points(layer_str: &str) -> Vec<(f32, f32)> {
    let mut points = Vec::new();

    if let Some(points_start) = layer_str.find("\"points\"") {
        if let Some(arr_start) = layer_str[points_start..].find('[') {
            let arr_content = &layer_str[points_start + arr_start..];

            let mut depth = 0;
            let mut arr_end = 0;
            for (i, c) in arr_content.chars().enumerate() {
                match c {
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            arr_end = i;
                            break;
                        }
                    }
                    _ => {}
                }
            }

            if arr_end > 0 {
                let points_arr = &arr_content[1..arr_end];

                // Parse each point object {x: ..., y: ..., type?: "curve", cx1?: ..., ...}
                let mut obj_depth = 0;
                let mut obj_start = 0;

                for (i, c) in points_arr.chars().enumerate() {
                    match c {
                        '{' => {
                            if obj_depth == 0 {
                                obj_start = i;
                            }
                            obj_depth += 1;
                        }
                        '}' => {
                            obj_depth -= 1;
                            if obj_depth == 0 {
                                let point_str = &points_arr[obj_start..=i];

                                let x = parse_float(point_str, "x");
                                let y = parse_float(point_str, "y");

                                if let (Some(px), Some(py)) = (x, y) {
                                    let is_curve = point_str.contains("\"type\"")
                                        && point_str.contains("curve");

                                    if is_curve {
                                        let cx1 = parse_float(point_str, "cx1").unwrap_or(px);
                                        let cy1 = parse_float(point_str, "cy1").unwrap_or(py);
                                        let cx2 = parse_float(point_str, "cx2").unwrap_or(px);
                                        let cy2 = parse_float(point_str, "cy2").unwrap_or(py);

                                        if let Some(&(prev_x, prev_y)) = points.last() {
                                            for t in 1..=8 {
                                                let t = t as f32 / 8.0;
                                                let mt = 1.0 - t;
                                                let bx = mt * mt * mt * prev_x
                                                    + 3.0 * mt * mt * t * cx1
                                                    + 3.0 * mt * t * t * cx2
                                                    + t * t * t * px;
                                                let by = mt * mt * mt * prev_y
                                                    + 3.0 * mt * mt * t * cy1
                                                    + 3.0 * mt * t * t * cy2
                                                    + t * t * t * py;
                                                points.push((bx, by));
                                            }
                                        } else {
                                            points.push((px, py));
                                        }
                                    } else {
                                        points.push((px, py));
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    points
}

/// Parse points array from JSON-like format: "points": [[x, y], [x, y], ...]
fn parse_custom_points(content: &str) -> Vec<(f32, f32)> {
    let mut points = Vec::new();

    if let Some(start) = content.find("\"points\"") {
        if let Some(arr_start) = content[start..].find('[') {
            let arr_content = &content[start + arr_start..];
            let mut depth = 0;
            let mut arr_end = 0;
            for (i, c) in arr_content.chars().enumerate() {
                match c {
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            arr_end = i;
                            break;
                        }
                    }
                    _ => {}
                }
            }

            if arr_end > 0 {
                let arr_str = &arr_content[1..arr_end];

                let mut i = 0;
                let chars: Vec<char> = arr_str.chars().collect();
                while i < chars.len() {
                    if chars[i] == '[' {
                        let mut j = i + 1;
                        while j < chars.len() && chars[j] != ']' {
                            j += 1;
                        }
                        if j < chars.len() {
                            let point_str: String = chars[i + 1..j].iter().collect();
                            let coords: Vec<&str> = point_str.split(',').collect();
                            if coords.len() >= 2 {
                                if let (Ok(x), Ok(y)) = (
                                    coords[0].trim().parse::<f32>(),
                                    coords[1].trim().parse::<f32>(),
                                ) {
                                    points.push((x, y));
                                }
                            }
                        }
                        i = j + 1;
                    } else {
                        i += 1;
                    }
                }
            }
        }
    }

    points
}

/// Parse a color value like "fill": "#RRGGBB" or "fill": "#AARRGGBB"
fn parse_color(content: &str, key: &str) -> Option<u32> {
    let pattern = format!("\"{}\"", key);
    if let Some(start) = content.find(&pattern) {
        if let Some(hash_pos) = content[start..].find('#') {
            let color_start = start + hash_pos + 1;
            let hex_chars: String = content[color_start..]
                .chars()
                .take_while(|c| c.is_ascii_hexdigit())
                .collect();

            if hex_chars.len() >= 6 {
                let (alpha, rgb) = if hex_chars.len() >= 8 {
                    let a = u8::from_str_radix(&hex_chars[0..2], 16).ok()?;
                    let rgb = u32::from_str_radix(&hex_chars[2..8], 16).ok()?;
                    (a as u32, rgb)
                } else {
                    let rgb = u32::from_str_radix(&hex_chars[0..6], 16).ok()?;
                    (0xFF, rgb)
                };
                return Some((alpha << 24) | rgb);
            }
        }
    }
    None
}

/// Parse a float value like "scale": 1.5
fn parse_float(content: &str, key: &str) -> Option<f32> {
    let pattern = format!("\"{}\"", key);
    if let Some(start) = content.find(&pattern) {
        if let Some(colon) = content[start..].find(':') {
            let value_start = start + colon + 1;
            let value_str: String = content[value_start..]
                .chars()
                .skip_while(|c| c.is_whitespace())
                .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
                .collect();
            return value_str.parse().ok();
        }
    }
    None
}

/// Scanline fill'n
unsafe fn draw_filled_polygon(stride: usize, points: &[(f32, f32)], ox: f32, oy: f32, color: u32) {
    if points.is_empty() {
        return;
    }

    let height = CURSOR_HEIGHT.load(Ordering::SeqCst) as i32;

    let min_y = points.iter().map(|(_, y)| *y + oy).fold(f32::MAX, f32::min) as i32;
    let max_y = points.iter().map(|(_, y)| *y + oy).fold(f32::MIN, f32::max) as i32;

    let min_y = min_y.max(0);
    let max_y = max_y.min(height - 1);

    // Scanline fill'ning
    for y in min_y..=max_y {
        let mut intersections = Vec::new();
        let yf = y as f32 + 0.5;

        for i in 0..points.len() {
            let (x1, y1) = (points[i].0 + ox, points[i].1 + oy);
            let (x2, y2) = (
                points[(i + 1) % points.len()].0 + ox,
                points[(i + 1) % points.len()].1 + oy,
            );

            if (y1 <= yf && y2 > yf) || (y2 <= yf && y1 > yf) {
                let x = x1 + (yf - y1) / (y2 - y1) * (x2 - x1);
                intersections.push(x);
            }
        }

        intersections.sort_by(|a, b| a.partial_cmp(b).unwrap());

        for chunk in intersections.chunks(2) {
            if chunk.len() == 2 {
                let x_start = chunk[0].max(0.0) as i32;
                let x_end = chunk[1].min(stride as f32 - 1.0) as i32;
                for x in x_start..=x_end {
                    if x >= 0 && (x as usize) < stride {
                        let idx = y as usize * stride + x as usize;
                        *CURSOR_BUFFER.add(idx) = blend_pixel(*CURSOR_BUFFER.add(idx), color);
                    }
                }
            }
        }
    }
}

unsafe fn draw_polygon_outline(stride: usize, points: &[(f32, f32)], ox: f32, oy: f32, color: u32) {
    draw_polygon_outline_thickness(stride, points, ox, oy, color, 0.0);
}

/// Draw polygon outline with configurable thickness
/// thickness parameter: 0.0 = use config or default (1.0), else use specified value
unsafe fn draw_polygon_outline_thickness(
    stride: usize,
    points: &[(f32, f32)],
    ox: f32,
    oy: f32,
    color: u32,
    thickness: f32,
) {
    // Get thickness from config if not specified
    let config_thickness = CONFIG_OUTLINE_THICKNESS.load(Ordering::Relaxed) as f32 / 10.0;
    let actual_thickness = if thickness > 0.0 {
        thickness
    } else if config_thickness > 0.0 {
        config_thickness
    } else {
        1.0 // default
    };

    let base_alpha = ((color >> 24) & 0xFF) as u32;
    let rgb = color & 0x00FFFFFF;

    // For thickness > 1, draw multiple concentric outlines
    let passes = (actual_thickness.ceil() as i32).max(1);

    for pass in 0..passes {
        let offset = pass as f32 * 0.5;
        let pass_alpha = if pass == 0 {
            base_alpha
        } else {
            (base_alpha * (100 - pass as u32 * 25) / 100).max(30)
        };
        let pass_color = (pass_alpha << 24) | rgb;

        for i in 0..points.len() {
            let (x1, y1) = (points[i].0 + ox, points[i].1 + oy);
            let (x2, y2) = (
                points[(i + 1) % points.len()].0 + ox,
                points[(i + 1) % points.len()].1 + oy,
            );

            if offset > 0.0 {
                let dx = x2 - x1;
                let dy = y2 - y1;
                let len = (dx * dx + dy * dy).sqrt().max(0.001);
                let nx = -dy / len * offset;
                let ny = dx / len * offset;
                draw_line_aa(stride, x1 + nx, y1 + ny, x2 + nx, y2 + ny, pass_color);
            } else {
                draw_line_aa(stride, x1, y1, x2, y2, pass_color);
            }
        }
    }

    let glow_offset = actual_thickness * 0.4;
    let glow_alpha = (base_alpha * 35 / 100).min(255);
    let glow_color = (glow_alpha << 24) | rgb;
    for i in 0..points.len() {
        let (x1, y1) = (points[i].0 + ox, points[i].1 + oy);
        let (x2, y2) = (
            points[(i + 1) % points.len()].0 + ox,
            points[(i + 1) % points.len()].1 + oy,
        );
        let dx = x2 - x1;
        let dy = y2 - y1;
        let len = (dx * dx + dy * dy).sqrt().max(0.001);
        let nx = -dy / len * (actual_thickness * 0.5 + glow_offset);
        let ny = dx / len * (actual_thickness * 0.5 + glow_offset);
        draw_line_aa(stride, x1 + nx, y1 + ny, x2 + nx, y2 + ny, glow_color);
    }
}

/// Frosted outline, draws outline with noise-varied alpha
/// for what was supposed to be a textured look...
unsafe fn draw_polygon_outline_spiral_blur(
    stride: usize,
    points: &[(f32, f32)],
    ox: f32,
    oy: f32,
    color: u32,
    blur_intensity: f32,
) {
    if points.is_empty() || blur_intensity == 0.0 {
        draw_polygon_outline(stride, points, ox, oy, color);
        return;
    }

    let height = CURSOR_HEIGHT.load(Ordering::SeqCst) as usize;

    let base_alpha = ((color >> 24) & 0xFF) as f32;
    let base_r = ((color >> 16) & 0xFF) as u32;
    let base_g = ((color >> 8) & 0xFF) as u32;
    let base_b = (color & 0xFF) as u32;
    let noise_strength = (blur_intensity * 6.0).min(60.0);

    for i in 0..points.len() {
        let (x1, y1) = (points[i].0 + ox, points[i].1 + oy);
        let (x2, y2) = (
            points[(i + 1) % points.len()].0 + ox,
            points[(i + 1) % points.len()].1 + oy,
        );

        let dx = (x2 - x1).abs();
        let dy = (y2 - y1).abs();
        let steps = (dx.max(dy) as i32).max(1);

        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            let x = (x1 + t * (x2 - x1)) as i32;
            let y = (y1 + t * (y2 - y1)) as i32;

            if x >= 0 && y >= 0 && (x as usize) < stride && (y as usize) < height {
                let hash = ((x as u32)
                    .wrapping_mul(374761393)
                    .wrapping_add((y as u32).wrapping_mul(668265263)))
                    ^ ((x as u32).wrapping_add(y as u32).wrapping_mul(1274126177));
                let noise = ((hash % 1000) as f32 / 500.0) - 1.0;
                let alpha_var = noise * noise_strength;
                let final_alpha = (base_alpha + alpha_var).clamp(30.0, 255.0) as u32;

                let color_noise = ((hash >> 10) % 16) as i32 - 8;
                let final_r = (base_r as i32 + color_noise).clamp(0, 255) as u32;
                let final_g = (base_g as i32 + color_noise).clamp(0, 255) as u32;
                let final_b = (base_b as i32 + color_noise).clamp(0, 255) as u32;

                let frosted_color =
                    (final_alpha << 24) | (final_r << 16) | (final_g << 8) | final_b;

                let idx = y as usize * stride + x as usize;
                let existing = *CURSOR_BUFFER.add(idx);
                *CURSOR_BUFFER.add(idx) = blend_pixel(existing, frosted_color);
            }
        }
    }
}

/// I'm Sorry
unsafe fn draw_filled_polygon_spiral_blur(
    stride: usize,
    points: &[(f32, f32)],
    ox: f32,
    oy: f32,
    color: u32,
    blur_intensity: f32,
) {
    if points.is_empty() || blur_intensity == 0.0 {
        draw_filled_polygon(stride, points, ox, oy, color);
        return;
    }

    let frost_mult = CONFIG_FROST_INTENSITY.load(Ordering::Relaxed) as f32 / 100.0;
    let adjusted_blur = blur_intensity * frost_mult;

    if adjusted_blur == 0.0 {
        draw_filled_polygon(stride, points, ox, oy, color);
        return;
    }

    draw_frosted_glass(stride, points, ox, oy, color, adjusted_blur);
}

unsafe fn draw_frosted_glass(
    stride: usize,
    points: &[(f32, f32)],
    ox: f32,
    oy: f32,
    tint_color: u32,
    blur_intensity: f32,
) {
    if points.is_empty() {
        return;
    }

    let base_alpha = ((tint_color >> 24) & 0xFF) as f32;
    let tint_r = ((tint_color >> 16) & 0xFF) as f32;
    let tint_g = ((tint_color >> 8) & 0xFF) as f32;
    let tint_b = (tint_color & 0xFF) as f32;

    // Smaller cells = finer grain
    let cell_size = (2.5 - blur_intensity * 0.15).max(1.2);
    let alpha_variation_max = (blur_intensity * 25.0).min(100.0);
    let color_variation_max = (blur_intensity * 10.0).min(50.0);

    let height = CURSOR_HEIGHT.load(Ordering::SeqCst) as i32;

    let min_y = points.iter().map(|(_, y)| *y + oy).fold(f32::MAX, f32::min) as i32;
    let max_y = points.iter().map(|(_, y)| *y + oy).fold(f32::MIN, f32::max) as i32;

    let min_y = min_y.max(0);
    let max_y = max_y.min(height - 1);

    for y in min_y..=max_y {
        let mut intersections = Vec::new();
        let yf = y as f32 + 0.5;

        for i in 0..points.len() {
            let (x1, y1) = (points[i].0 + ox, points[i].1 + oy);
            let (x2, y2) = (
                points[(i + 1) % points.len()].0 + ox,
                points[(i + 1) % points.len()].1 + oy,
            );

            if (y1 <= yf && y2 > yf) || (y2 <= yf && y1 > yf) {
                let x = x1 + (yf - y1) / (y2 - y1) * (x2 - x1);
                intersections.push(x);
            }
        }

        intersections.sort_by(|a, b| a.partial_cmp(b).unwrap());

        for chunk in intersections.chunks(2) {
            if chunk.len() == 2 {
                let x_start = chunk[0].max(0.0) as i32;
                let x_end = chunk[1].min(stride as f32 - 1.0) as i32;

                for x in x_start..=x_end {
                    if x >= 0 && (x as usize) < stride {
                        let idx = y as usize * stride + x as usize;

                        let cell_x = (x as f32 / cell_size) as i32;
                        let cell_y = (y as f32 / cell_size) as i32;

                        let hash = ((cell_x as u32)
                            .wrapping_mul(374761393)
                            .wrapping_add((cell_y as u32).wrapping_mul(668265263)))
                            ^ ((cell_x as u32)
                                .wrapping_add(cell_y as u32)
                                .wrapping_mul(1274126177));

                        let noise1 = ((hash % 1000) as f32 / 500.0) - 1.0;
                        let hash2 = hash.wrapping_mul(16807);
                        let noise2 = ((hash2 % 1000) as f32 / 500.0) - 1.0;
                        let noise = noise1 * 0.7 + noise2 * 0.3;

                        let alpha_variation = noise * alpha_variation_max;
                        let final_alpha = (base_alpha + alpha_variation).clamp(15.0, 240.0) as u32;

                        let color_shift = noise * color_variation_max;
                        let final_r = (tint_r + color_shift).clamp(0.0, 255.0) as u32;
                        let final_g = (tint_g + color_shift).clamp(0.0, 255.0) as u32;
                        let final_b = (tint_b + color_shift * 0.5).clamp(0.0, 255.0) as u32;

                        let frosted_color =
                            (final_alpha << 24) | (final_r << 16) | (final_g << 8) | final_b;

                        let existing = *CURSOR_BUFFER.add(idx);
                        let existing_alpha = (existing >> 24) & 0xFF;

                        if existing_alpha > 0 {
                            let blend = 0.5;
                            let ex_r = ((existing >> 16) & 0xFF) as f32;
                            let ex_g = ((existing >> 8) & 0xFF) as f32;
                            let ex_b = (existing & 0xFF) as f32;

                            let blended_r =
                                ((ex_r * (1.0 - blend) + final_r as f32 * blend) as u32).min(255);
                            let blended_g =
                                ((ex_g * (1.0 - blend) + final_g as f32 * blend) as u32).min(255);
                            let blended_b =
                                ((ex_b * (1.0 - blend) + final_b as f32 * blend) as u32).min(255);
                            let blended_a =
                                ((existing_alpha as f32 + final_alpha as f32) / 2.0) as u32;

                            *CURSOR_BUFFER.add(idx) = (blended_a << 24)
                                | (blended_r << 16)
                                | (blended_g << 8)
                                | blended_b;
                        } else {
                            *CURSOR_BUFFER.add(idx) = frosted_color;
                        }
                    }
                }
            }
        }
    }
}

/// Some smart dude math (Bresenham's line algorithm)
unsafe fn draw_line(_stride: usize, _x0: i32, _y0: i32, _x1: i32, _y1: i32, _color: u32) {
    // Deprecated: will refactor, use draw_line_aa for anti-aliased lines
    draw_line_aa(
        _stride, _x0 as f32, _y0 as f32, _x1 as f32, _y1 as f32, _color,
    );
}

/// Anti-aliased line drawing using more smart dude math (Xiaolin Wu's algorithm)
/// Produces smooth lines by blending pixels at fractional positions
unsafe fn draw_line_aa(
    stride: usize,
    mut x0: f32,
    mut y0: f32,
    mut x1: f32,
    mut y1: f32,
    color: u32,
) {
    let steep = (y1 - y0).abs() > (x1 - x0).abs();

    if steep {
        std::mem::swap(&mut x0, &mut y0);
        std::mem::swap(&mut x1, &mut y1);
    }
    if x0 > x1 {
        std::mem::swap(&mut x0, &mut x1);
        std::mem::swap(&mut y0, &mut y1);
    }

    let dx = x1 - x0;
    let dy = y1 - y0;
    let gradient = if dx < 0.0001 { 1.0 } else { dy / dx };

    let xend = x0.round();
    let yend = y0 + gradient * (xend - x0);
    let xgap = 1.0 - (x0 + 0.5).fract();
    let xpxl1 = xend as i32;
    let ypxl1 = yend.floor() as i32;

    if steep {
        plot_aa(stride, ypxl1, xpxl1, color, (1.0 - yend.fract()) * xgap);
        plot_aa(stride, ypxl1 + 1, xpxl1, color, yend.fract() * xgap);
    } else {
        plot_aa(stride, xpxl1, ypxl1, color, (1.0 - yend.fract()) * xgap);
        plot_aa(stride, xpxl1, ypxl1 + 1, color, yend.fract() * xgap);
    }

    let mut intery = yend + gradient;

    let xend = x1.round();
    let yend = y1 + gradient * (xend - x1);
    let xgap = (x1 + 0.5).fract();
    let xpxl2 = xend as i32;
    let ypxl2 = yend.floor() as i32;

    if steep {
        plot_aa(stride, ypxl2, xpxl2, color, (1.0 - yend.fract()) * xgap);
        plot_aa(stride, ypxl2 + 1, xpxl2, color, yend.fract() * xgap);
    } else {
        plot_aa(stride, xpxl2, ypxl2, color, (1.0 - yend.fract()) * xgap);
        plot_aa(stride, xpxl2, ypxl2 + 1, color, yend.fract() * xgap);
    }

    for x in (xpxl1 + 1)..xpxl2 {
        if steep {
            plot_aa(
                stride,
                intery.floor() as i32,
                x,
                color,
                1.0 - intery.fract(),
            );
            plot_aa(stride, intery.floor() as i32 + 1, x, color, intery.fract());
        } else {
            plot_aa(
                stride,
                x,
                intery.floor() as i32,
                color,
                1.0 - intery.fract(),
            );
            plot_aa(stride, x, intery.floor() as i32 + 1, color, intery.fract());
        }
        intery += gradient;
    }
}

#[inline]
unsafe fn plot_aa(stride: usize, x: i32, y: i32, color: u32, brightness: f32) {
    let height = CURSOR_HEIGHT.load(Ordering::SeqCst) as usize;
    if x < 0 || y < 0 || (x as usize) >= stride || (y as usize) >= height || brightness <= 0.0 {
        return;
    }

    let idx = y as usize * stride + x as usize;
    let base_alpha = ((color >> 24) & 0xFF) as f32;
    let aa_alpha = (base_alpha * brightness.clamp(0.0, 1.0)) as u32;

    if aa_alpha == 0 {
        return;
    }

    let aa_color = (aa_alpha << 24) | (color & 0x00FFFFFF);
    let existing = *CURSOR_BUFFER.add(idx);
    *CURSOR_BUFFER.add(idx) = blend_pixel(existing, aa_color);
}

fn blend_pixel(dst: u32, src: u32) -> u32 {
    let sa = (src >> 24) & 0xFF;
    if sa == 0 {
        return dst;
    }
    if sa == 255 {
        return src;
    }

    let da = (dst >> 24) & 0xFF;
    let sr = (src >> 16) & 0xFF;
    let sg = (src >> 8) & 0xFF;
    let sb = src & 0xFF;
    let dr = (dst >> 16) & 0xFF;
    let dg = (dst >> 8) & 0xFF;
    let db = dst & 0xFF;

    let inv_sa = 255 - sa;
    let out_a = sa + (da * inv_sa) / 255;
    let out_r = (sr * sa + dr * inv_sa) / 255;
    let out_g = (sg * sa + dg * inv_sa) / 255;
    let out_b = (sb * sa + db * inv_sa) / 255;

    (out_a << 24) | (out_r << 16) | (out_g << 8) | out_b
}

#[no_mangle]
pub unsafe extern "C" fn ioctl(fd: i32, request: libc::c_ulong, arg: *mut c_void) -> i32 {
    init_real_functions();

    // Capture DRM fd from any DRM ioctl (they all start with 0x64 = 'd') //noice
    if (request >> 8) & 0xFF == 0x64 {
        if CURSOR_FD.load(Ordering::SeqCst) < 0 {
            CURSOR_FD.store(fd, Ordering::SeqCst);
            debug_print!("Captured DRM fd: {}", fd);
        }
    }

    // Handle legacy cursor operations (hopefully)
    if request == DRM_IOCTL_MODE_CURSOR || request == DRM_IOCTL_MODE_CURSOR2 {
        debug_print!("Legacy cursor ioctl: 0x{:x}", request);
        if !INITIALIZED.load(Ordering::SeqCst) {
            if !create_cursor_buffer(fd, 256, 256) {
                debug_print!("Failed to create cursor buffer!");
                return 0;
            }
            debug_print!(
                "Created cursor buffer, FB_ID={}",
                CURSOR_FB_ID.load(Ordering::SeqCst)
            );
        }

        // For cursor operations, use OUR proper buffer instead
        let cursor = arg as *mut DrmModeCursor2;
        if !cursor.is_null() {
            let flags = (*cursor).flags;

            if flags & DRM_MODE_CURSOR_BO != 0 {
                // If compositor wants to hide cursor (handle = 0), allow it through
                if (*cursor).handle == 0 {
                    debug_print!("Compositor hiding cursor (handle=0), passing through");
                    return real_ioctl(fd, request, arg);
                }

                (*cursor).handle = CURSOR_HANDLE.load(Ordering::SeqCst);
                // Use display size, not buffer size (hardware may not support large cursors)
                (*cursor).width = CURSOR_DISPLAY_SIZE;
                (*cursor).height = CURSOR_DISPLAY_SIZE;
            }

            return real_ioctl(fd, request, arg);
        }
        return 0;
    }

    real_ioctl(fd, request, arg)
}

#[no_mangle]
pub unsafe extern "C" fn drmModeSetCursor(
    fd: i32,
    crtc_id: u32,
    bo_handle: u32,
    _width: u32,
    _height: u32,
) -> i32 {
    // If compositor wants to hide cursor (handle = 0), allow it through
    if bo_handle == 0 {
        debug_print!("drmModeSetCursor: hiding cursor (handle=0)");
        let cursor = DrmModeCursor2 {
            flags: DRM_MODE_CURSOR_BO,
            crtc_id,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            handle: 0,
            hot_x: 0,
            hot_y: 0,
        };
        return real_ioctl(
            fd,
            DRM_IOCTL_MODE_CURSOR2,
            &cursor as *const _ as *mut c_void,
        );
    }

    if !INITIALIZED.load(Ordering::SeqCst) {
        if !create_cursor_buffer(fd, 256, 256) {
            return 0;
        }
    }

    let cursor = DrmModeCursor2 {
        flags: DRM_MODE_CURSOR_BO,
        crtc_id,
        x: 0,
        y: 0,
        // Use display size, not buffer size (hardware may not support large cursors)
        width: CURSOR_DISPLAY_SIZE,
        height: CURSOR_DISPLAY_SIZE,
        handle: CURSOR_HANDLE.load(Ordering::SeqCst),
        hot_x: CURSOR_HOTSPOT_X.load(Ordering::SeqCst),
        hot_y: CURSOR_HOTSPOT_Y.load(Ordering::SeqCst),
    };

    real_ioctl(
        fd,
        DRM_IOCTL_MODE_CURSOR2,
        &cursor as *const _ as *mut c_void,
    )
}

/// FaceSmack drmModeSetCursor2
#[no_mangle]
pub unsafe extern "C" fn drmModeSetCursor2(
    fd: i32,
    crtc_id: u32,
    bo_handle: u32,
    _width: u32,
    _height: u32,
    hot_x: i32,
    hot_y: i32,
) -> i32 {
    // If compositor wants to hide cursor (handle = 0)
    if bo_handle == 0 {
        debug_print!("drmModeSetCursor2: hiding cursor (handle=0)");
        CURSOR_VISIBLE.store(false, Ordering::SeqCst);

        // If fade is enabled, start the poor excuse of fading instead of instant hide
        if cursor_fade_enabled() {
            CURSOR_FADING_OUT.store(true, Ordering::SeqCst);
            // But suprise, Don't actually hide yet, the fade will happen in drmModeMoveCursor
            return 0;
        }

        let cursor = DrmModeCursor2 {
            flags: DRM_MODE_CURSOR_BO,
            crtc_id,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            handle: 0,
            hot_x: 0,
            hot_y: 0,
        };
        return real_ioctl(
            fd,
            DRM_IOCTL_MODE_CURSOR2,
            &cursor as *const _ as *mut c_void,
        );
    }

    CURSOR_VISIBLE.store(true, Ordering::SeqCst);
    CURSOR_FADING_OUT.store(false, Ordering::SeqCst);
    CURSOR_FADE_ALPHA.store(255, Ordering::SeqCst);

    if !INITIALIZED.load(Ordering::SeqCst) {
        if !create_cursor_buffer(fd, 256, 256) {
            return 0;
        }
    }

    let new_hot_x = hot_x + CURSOR_HOTSPOT_X.load(Ordering::SeqCst);
    let new_hot_y = hot_y + CURSOR_HOTSPOT_Y.load(Ordering::SeqCst);

    let final_hot_x;
    let final_hot_y;

    if !HOTSPOT_INITIALIZED.load(Ordering::SeqCst) {
        final_hot_x = new_hot_x;
        final_hot_y = new_hot_y;
        APPLIED_HOTSPOT_X.store(final_hot_x, Ordering::SeqCst);
        APPLIED_HOTSPOT_Y.store(final_hot_y, Ordering::SeqCst);
        HOTSPOT_INITIALIZED.store(true, Ordering::SeqCst);
        debug_print!("Hotspot initialized: ({},{})", final_hot_x, final_hot_y);
    } else {
        load_config();

        let current_hot_x = APPLIED_HOTSPOT_X.load(Ordering::SeqCst);
        let current_hot_y = APPLIED_HOTSPOT_Y.load(Ordering::SeqCst);

        if CONFIG_HOTSPOT_SMOOTHING.load(Ordering::Relaxed) {
            let threshold = CONFIG_HOTSPOT_THRESHOLD.load(Ordering::Relaxed);
            let dx = (new_hot_x - current_hot_x).abs();
            let dy = (new_hot_y - current_hot_y).abs();

            if dx > threshold || dy > threshold {
                final_hot_x = current_hot_x + (new_hot_x - current_hot_x) / 3;
                final_hot_y = current_hot_y + (new_hot_y - current_hot_y) / 3;
                APPLIED_HOTSPOT_X.store(final_hot_x, Ordering::SeqCst);
                APPLIED_HOTSPOT_Y.store(final_hot_y, Ordering::SeqCst);
                debug_print!(
                    "Hotspot smoothed: ({},{}) -> ({},{})",
                    current_hot_x,
                    current_hot_y,
                    final_hot_x,
                    final_hot_y
                );
            } else {
                final_hot_x = current_hot_x;
                final_hot_y = current_hot_y;
            }
        } else {
            final_hot_x = new_hot_x;
            final_hot_y = new_hot_y;
            APPLIED_HOTSPOT_X.store(final_hot_x, Ordering::SeqCst);
            APPLIED_HOTSPOT_Y.store(final_hot_y, Ordering::SeqCst);
        }
    }

    let cursor = DrmModeCursor2 {
        flags: DRM_MODE_CURSOR_BO,
        crtc_id,
        x: 0,
        y: 0,
        // Use display size, not buffer size (again, hardware may not support large cursors)
        width: CURSOR_DISPLAY_SIZE,
        height: CURSOR_DISPLAY_SIZE,
        handle: CURSOR_HANDLE.load(Ordering::SeqCst),
        hot_x: final_hot_x,
        hot_y: final_hot_y,
    };

    real_ioctl(
        fd,
        DRM_IOCTL_MODE_CURSOR2,
        &cursor as *const _ as *mut c_void,
    )
}

/// Apply uniform alpha fade to cursor buffer
/// This does not work as intended yet.
/// All non-zero pixels should get scaled to target_alpha proportionally
/// This should ensures outline and fill fade together perceptually
/// Outline still does its own thing, It will be fixed later
unsafe fn apply_cursor_fade(target_alpha: f32) {
    if CURSOR_BUFFER.is_null() {
        return;
    }

    let width = CURSOR_WIDTH.load(Ordering::SeqCst) as usize;
    let height = CURSOR_HEIGHT.load(Ordering::SeqCst) as usize;
    let target = target_alpha.clamp(0.0, 255.0);

    for i in 0..(width * height) {
        let pixel = *CURSOR_BUFFER.add(i);
        let orig_a = ((pixel >> 24) & 0xFF) as f32;

        if orig_a > 0.0 {
            let new_a = ((orig_a / 255.0) * target) as u32;
            *CURSOR_BUFFER.add(i) = (new_a << 24) | (pixel & 0x00FFFFFF);
        }
    }
}

/// Spawn a thread to handle our fade-out animation
fn spawn_fade_out_thread() {
    if FADE_THREAD_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }

    thread::spawn(move || {
        let fade_speed = CONFIG_FADE_SPEED.load(Ordering::Relaxed) as f32;
        let frame_time = Duration::from_millis(16); // ~60fps
        let step = fade_speed.max(5.0);

        let mut alpha = 255.0_f32;

        while alpha > 0.0 {
            if !CURSOR_FADING_OUT.load(Ordering::SeqCst) {
                break;
            }

            alpha = (alpha - step).max(0.0);

            unsafe {
                if !CURSOR_BUFFER.is_null() {
                    render_cursor();
                    apply_cursor_fade(alpha);
                }
            }

            thread::sleep(frame_time);
        }

        if CURSOR_FADING_OUT.load(Ordering::SeqCst) {
            CURSOR_FADING_OUT.store(false, Ordering::SeqCst);
            CURSOR_FADE_ALPHA.store(0, Ordering::SeqCst);

            unsafe {
                if !CURSOR_BUFFER.is_null() {
                    let width = CURSOR_WIDTH.load(Ordering::SeqCst) as usize;
                    let height = CURSOR_HEIGHT.load(Ordering::SeqCst) as usize;
                    for i in 0..(width * height) {
                        *CURSOR_BUFFER.add(i) = 0x00000000;
                    }
                }
            }
        }

        FADE_THREAD_RUNNING.store(false, Ordering::SeqCst);
    });
}

fn spawn_fade_in_thread() {
    if FADE_THREAD_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }

    thread::spawn(move || {
        let fade_speed = CONFIG_FADE_SPEED.load(Ordering::Relaxed) as f32;
        let frame_time = Duration::from_millis(16); // set to a standard ~60fps
        let step = fade_speed.max(5.0);

        let mut alpha = 0.0_f32;

        while alpha < 255.0 {
            if !CURSOR_FADING_IN.load(Ordering::SeqCst) {
                break;
            }

            alpha = (alpha + step).min(255.0);

            unsafe {
                if !CURSOR_BUFFER.is_null() {
                    render_cursor();
                    apply_cursor_fade(alpha);
                }
            }

            thread::sleep(frame_time);
        }

        if CURSOR_FADING_IN.load(Ordering::SeqCst) {
            CURSOR_FADING_IN.store(false, Ordering::SeqCst);
            CURSOR_FADE_ALPHA.store(255, Ordering::SeqCst);

            unsafe {
                if !CURSOR_BUFFER.is_null() {
                    render_cursor();
                }
            }
        }

        FADE_THREAD_RUNNING.store(false, Ordering::SeqCst);
    });
}

#[no_mangle]
pub unsafe extern "C" fn drmModeMoveCursor(fd: i32, crtc_id: u32, x: i32, y: i32) -> i32 {
    CURSOR_SCREEN_X.store(x, Ordering::SeqCst);
    CURSOR_SCREEN_Y.store(y, Ordering::SeqCst);

    check_config_changed();

    if CURSOR_FADING_OUT.load(Ordering::SeqCst) {
        let current_alpha = CURSOR_FADE_ALPHA.load(Ordering::SeqCst);

        if current_alpha > 0 {
            let fade_speed = CONFIG_FADE_SPEED.load(Ordering::Relaxed);
            let new_alpha = current_alpha.saturating_sub(fade_speed);
            CURSOR_FADE_ALPHA.store(new_alpha, Ordering::SeqCst);

            render_cursor();
            let fade_mult = new_alpha as f32 / 255.0;
            apply_cursor_fade(fade_mult);

            if new_alpha == 0 {
                CURSOR_FADING_OUT.store(false, Ordering::SeqCst);
                let cursor = DrmModeCursor2 {
                    flags: DRM_MODE_CURSOR_BO,
                    crtc_id,
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                    handle: 0,
                    hot_x: 0,
                    hot_y: 0,
                };
                return real_ioctl(
                    fd,
                    DRM_IOCTL_MODE_CURSOR2,
                    &cursor as *const _ as *mut c_void,
                );
            }

            // Buffer was modified and need to re-set the cursor BO, not just mov
            // DRM_MODE_CURSOR_BO | DRM_MODE_CURSOR_MOVE combined sets and moves
            let cursor = DrmModeCursor2 {
                flags: DRM_MODE_CURSOR_BO | DRM_MODE_CURSOR_MOVE,
                crtc_id,
                x,
                y,
                width: CURSOR_DISPLAY_SIZE,
                height: CURSOR_DISPLAY_SIZE,
                handle: CURSOR_HANDLE.load(Ordering::SeqCst),
                hot_x: APPLIED_HOTSPOT_X.load(Ordering::SeqCst),
                hot_y: APPLIED_HOTSPOT_Y.load(Ordering::SeqCst),
            };
            return real_ioctl(
                fd,
                DRM_IOCTL_MODE_CURSOR2,
                &cursor as *const _ as *mut c_void,
            );
        }
    }

    let cursor = DrmModeCursor2 {
        flags: DRM_MODE_CURSOR_MOVE,
        crtc_id,
        x,
        y,
        width: 0,
        height: 0,
        handle: 0,
        hot_x: 0,
        hot_y: 0,
    };

    real_ioctl(
        fd,
        DRM_IOCTL_MODE_CURSOR2,
        &cursor as *const _ as *mut c_void,
    )
}

// track planes and filter their updates
static mut CURSOR_PLANE_IDS: [u32; 8] = [0; 8];
static mut NUM_CURSOR_PLANES: usize = 0;

// Real function pointers for atomic stuff, I promise
static mut REAL_GET_PLANE: Option<unsafe extern "C" fn(i32, u32) -> *mut DrmModePlane> = None;
static mut REAL_GET_OBJECT_PROPERTIES: Option<
    unsafe extern "C" fn(i32, u32, u32) -> *mut DrmModeObjectProperties,
> = None;
static mut REAL_GET_PROPERTY: Option<unsafe extern "C" fn(i32, u32) -> *mut DrmModePropertyRes> =
    None;
static mut REAL_FREE_PLANE: Option<unsafe extern "C" fn(*mut DrmModePlane)> = None;
static mut REAL_FREE_OBJECT_PROPERTIES: Option<unsafe extern "C" fn(*mut DrmModeObjectProperties)> =
    None;
static mut REAL_FREE_PROPERTY: Option<unsafe extern "C" fn(*mut DrmModePropertyRes)> = None;
static mut REAL_ATOMIC_ADD: Option<unsafe extern "C" fn(*mut c_void, u32, u32, u64) -> i32> = None;

const DRM_MODE_OBJECT_PLANE: u32 = 0xeeeeeeee;

#[repr(C)]
struct DrmModePlane {
    count_formats: u32,
    formats: *mut u32,
    plane_id: u32,
    crtc_id: u32,
    fb_id: u32,
    crtc_x: u32,
    crtc_y: u32,
    x: u32,
    y: u32,
    possible_crtcs: u32,
    gamma_size: u32,
}

#[repr(C)]
struct DrmModeObjectProperties {
    count_props: u32,
    props: *mut u32,
    prop_values: *mut u64,
}

#[repr(C)]
struct DrmModePropertyRes {
    prop_id: u32,
    flags: u32,
    name: [i8; 32],
    count_values: u32,
    values: *mut u64,
    count_enums: u32,
    enums: *mut c_void,
    count_blobs: u32,
    blob_ids: *mut u32,
}

unsafe fn init_plane_functions() {
    if REAL_GET_PLANE.is_none() {
        let sym = libc::dlsym(libc::RTLD_NEXT, b"drmModeGetPlane\0".as_ptr() as *const i8);
        if !sym.is_null() {
            REAL_GET_PLANE = Some(std::mem::transmute(sym));
        }
    }
    if REAL_GET_OBJECT_PROPERTIES.is_none() {
        let sym = libc::dlsym(
            libc::RTLD_NEXT,
            b"drmModeObjectGetProperties\0".as_ptr() as *const i8,
        );
        if !sym.is_null() {
            REAL_GET_OBJECT_PROPERTIES = Some(std::mem::transmute(sym));
        }
    }
    if REAL_GET_PROPERTY.is_none() {
        let sym = libc::dlsym(
            libc::RTLD_NEXT,
            b"drmModeGetProperty\0".as_ptr() as *const i8,
        );
        if !sym.is_null() {
            REAL_GET_PROPERTY = Some(std::mem::transmute(sym));
        }
    }
    if REAL_FREE_PLANE.is_none() {
        let sym = libc::dlsym(libc::RTLD_NEXT, b"drmModeFreePlane\0".as_ptr() as *const i8);
        if !sym.is_null() {
            REAL_FREE_PLANE = Some(std::mem::transmute(sym));
        }
    }
    if REAL_FREE_OBJECT_PROPERTIES.is_none() {
        let sym = libc::dlsym(
            libc::RTLD_NEXT,
            b"drmModeFreeObjectProperties\0".as_ptr() as *const i8,
        );
        if !sym.is_null() {
            REAL_FREE_OBJECT_PROPERTIES = Some(std::mem::transmute(sym));
        }
    }
    if REAL_FREE_PROPERTY.is_none() {
        let sym = libc::dlsym(
            libc::RTLD_NEXT,
            b"drmModeFreeProperty\0".as_ptr() as *const i8,
        );
        if !sym.is_null() {
            REAL_FREE_PROPERTY = Some(std::mem::transmute(sym));
        }
    }
    if REAL_ATOMIC_ADD.is_none() {
        let sym = libc::dlsym(
            libc::RTLD_NEXT,
            b"drmModeAtomicAddProperty\0".as_ptr() as *const i8,
        );
        if !sym.is_null() {
            REAL_ATOMIC_ADD = Some(std::mem::transmute(sym));
        }
    }
}

unsafe fn is_cursor_plane(plane_id: u32) -> bool {
    for i in 0..NUM_CURSOR_PLANES {
        if CURSOR_PLANE_IDS[i] == plane_id {
            return true;
        }
    }
    false
}

unsafe fn register_cursor_plane(plane_id: u32) -> usize {
    for i in 0..NUM_CURSOR_PLANES {
        if CURSOR_PLANE_IDS[i] == plane_id {
            return i;
        }
    }
    if NUM_CURSOR_PLANES < 8 {
        let idx = NUM_CURSOR_PLANES;
        CURSOR_PLANE_IDS[idx] = plane_id;
        NUM_CURSOR_PLANES += 1;
        return idx;
    }
    8
}

unsafe fn get_cursor_plane_index(plane_id: u32) -> Option<usize> {
    for i in 0..NUM_CURSOR_PLANES {
        if CURSOR_PLANE_IDS[i] == plane_id {
            return Some(i);
        }
    }
    None
}

#[no_mangle]
pub unsafe extern "C" fn drmModeGetPlane(fd: i32, plane_id: u32) -> *mut DrmModePlane {
    init_plane_functions();

    if CURSOR_FD.load(Ordering::SeqCst) < 0 {
        CURSOR_FD.store(fd, Ordering::SeqCst);
    }

    let plane = match REAL_GET_PLANE {
        Some(func) => func(fd, plane_id),
        None => return std::ptr::null_mut(),
    };

    if plane.is_null() {
        return plane;
    }

    if let Some(get_props) = REAL_GET_OBJECT_PROPERTIES {
        let props = get_props(fd, plane_id, DRM_MODE_OBJECT_PLANE);
        if !props.is_null() {
            let count = (*props).count_props as usize;
            let mut is_cursor = false;
            let mut fb_id_prop = 0u32;
            let mut src_w_prop = 0u32;
            let mut src_h_prop = 0u32;
            let mut crtc_w_prop = 0u32;
            let mut crtc_h_prop = 0u32;

            for i in 0..count {
                let prop_id = *(*props).props.add(i);
                let prop_value = *(*props).prop_values.add(i);

                if let Some(get_prop) = REAL_GET_PROPERTY {
                    let prop = get_prop(fd, prop_id);
                    if !prop.is_null() {
                        let name_ptr = (*prop).name.as_ptr();

                        // Check if property name is "type"
                        if libc::strcmp(name_ptr, b"type\0".as_ptr() as *const i8) == 0 {
                            if prop_value == DRM_PLANE_TYPE_CURSOR {
                                is_cursor = true;
                            }
                        }

                        if libc::strcmp(name_ptr, b"FB_ID\0".as_ptr() as *const i8) == 0 {
                            fb_id_prop = prop_id;
                        }
                        if libc::strcmp(name_ptr, b"SRC_W\0".as_ptr() as *const i8) == 0 {
                            src_w_prop = prop_id;
                        }
                        if libc::strcmp(name_ptr, b"SRC_H\0".as_ptr() as *const i8) == 0 {
                            src_h_prop = prop_id;
                        }
                        if libc::strcmp(name_ptr, b"CRTC_W\0".as_ptr() as *const i8) == 0 {
                            crtc_w_prop = prop_id;
                        }
                        if libc::strcmp(name_ptr, b"CRTC_H\0".as_ptr() as *const i8) == 0 {
                            crtc_h_prop = prop_id;
                        }

                        if let Some(free_prop) = REAL_FREE_PROPERTY {
                            free_prop(prop);
                        }
                    }
                }
            }

            if is_cursor {
                let idx = register_cursor_plane(plane_id);
                if idx < 8 {
                    if fb_id_prop != 0 {
                        CURSOR_FB_PROP_IDS[idx] = fb_id_prop;
                    }
                    if src_w_prop != 0 {
                        CURSOR_SRC_W_PROP_IDS[idx] = src_w_prop;
                    }
                    if src_h_prop != 0 {
                        CURSOR_SRC_H_PROP_IDS[idx] = src_h_prop;
                    }
                    if crtc_w_prop != 0 {
                        CURSOR_CRTC_W_PROP_IDS[idx] = crtc_w_prop;
                    }
                    if crtc_h_prop != 0 {
                        CURSOR_CRTC_H_PROP_IDS[idx] = crtc_h_prop;
                    }
                }
            }

            if let Some(free_props) = REAL_FREE_OBJECT_PROPERTIES {
                free_props(props);
            }
        }
    }

    plane
}

/// Try
unsafe fn try_detect_cursor_plane(object_id: u32) -> bool {
    let fd = CURSOR_FD.load(Ordering::SeqCst);
    if fd < 0 {
        return false;
    }

    // Hmmm, Already known cursor plane?
    if get_cursor_plane_index(object_id).is_some() {
        return true;
    }

    // let's see if it's a cursor plane
    if let Some(get_props) = REAL_GET_OBJECT_PROPERTIES {
        let props = get_props(fd, object_id, DRM_MODE_OBJECT_PLANE);
        if !props.is_null() {
            let count = (*props).count_props as usize;
            let mut is_cursor = false;
            let mut fb_id_prop = 0u32;
            let mut src_w_prop = 0u32;
            let mut src_h_prop = 0u32;
            let mut crtc_w_prop = 0u32;
            let mut crtc_h_prop = 0u32;

            for i in 0..count {
                let prop_id = *(*props).props.add(i);
                let prop_value = *(*props).prop_values.add(i);

                if let Some(get_prop) = REAL_GET_PROPERTY {
                    let prop = get_prop(fd, prop_id);
                    if !prop.is_null() {
                        let name_ptr = (*prop).name.as_ptr();

                        if libc::strcmp(name_ptr, b"type\0".as_ptr() as *const i8) == 0 {
                            if prop_value == DRM_PLANE_TYPE_CURSOR {
                                is_cursor = true;
                            }
                        }

                        if libc::strcmp(name_ptr, b"FB_ID\0".as_ptr() as *const i8) == 0 {
                            fb_id_prop = prop_id;
                        }
                        if libc::strcmp(name_ptr, b"SRC_W\0".as_ptr() as *const i8) == 0 {
                            src_w_prop = prop_id;
                        }
                        if libc::strcmp(name_ptr, b"SRC_H\0".as_ptr() as *const i8) == 0 {
                            src_h_prop = prop_id;
                        }
                        if libc::strcmp(name_ptr, b"CRTC_W\0".as_ptr() as *const i8) == 0 {
                            crtc_w_prop = prop_id;
                        }
                        if libc::strcmp(name_ptr, b"CRTC_H\0".as_ptr() as *const i8) == 0 {
                            crtc_h_prop = prop_id;
                        }

                        if let Some(free_prop) = REAL_FREE_PROPERTY {
                            free_prop(prop);
                        }
                    }
                }
            }

            if is_cursor {
                debug_print!(
                    "Detected cursor plane {} with FB_ID prop {}",
                    object_id,
                    fb_id_prop
                );
                let idx = register_cursor_plane(object_id);
                if idx < 8 {
                    if fb_id_prop != 0 {
                        CURSOR_FB_PROP_IDS[idx] = fb_id_prop;
                    }
                    if src_w_prop != 0 {
                        CURSOR_SRC_W_PROP_IDS[idx] = src_w_prop;
                    }
                    if src_h_prop != 0 {
                        CURSOR_SRC_H_PROP_IDS[idx] = src_h_prop;
                    }
                    if crtc_w_prop != 0 {
                        CURSOR_CRTC_W_PROP_IDS[idx] = crtc_w_prop;
                    }
                    if crtc_h_prop != 0 {
                        CURSOR_CRTC_H_PROP_IDS[idx] = crtc_h_prop;
                    }
                }
            }

            if let Some(free_props) = REAL_FREE_OBJECT_PROPERTIES {
                free_props(props);
            }

            return is_cursor;
        }
    }

    false
}

#[no_mangle]
pub unsafe extern "C" fn drmModeAtomicAddProperty(
    req: *mut c_void,
    object_id: u32,
    property_id: u32,
    value: u64,
) -> i32 {
    init_plane_functions();

    check_cursor_refresh();

    check_config_changed();

    let is_cursor =
        get_cursor_plane_index(object_id).is_some() || try_detect_cursor_plane(object_id);

    if is_cursor {
        debug_print!(
            "Cursor plane {} property {} = {}",
            object_id,
            property_id,
            value
        );

        if !INITIALIZED.load(Ordering::SeqCst) {
            let fd = CURSOR_FD.load(Ordering::SeqCst);
            if fd >= 0 {
                debug_print!("Creating cursor buffer on fd {}", fd);
                if create_cursor_buffer(fd, 256, 256) {
                    debug_print!(
                        "Cursor buffer created, FB_ID={}",
                        CURSOR_FB_ID.load(Ordering::SeqCst)
                    );
                } else {
                    debug_print!("Failed to create cursor buffer!");
                }
            } else {
                debug_print!("No DRM fd captured yet!");
            }
        }

        if let Some(idx) = get_cursor_plane_index(object_id) {
            let fb_prop_id = CURSOR_FB_PROP_IDS[idx];
            let src_w_prop_id = CURSOR_SRC_W_PROP_IDS[idx];
            let src_h_prop_id = CURSOR_SRC_H_PROP_IDS[idx];
            let crtc_w_prop_id = CURSOR_CRTC_W_PROP_IDS[idx];
            let crtc_h_prop_id = CURSOR_CRTC_H_PROP_IDS[idx];

            if fb_prop_id != 0 && property_id == fb_prop_id {
                // If compositor wants to hide cursor (FB_ID = 0)
                if value == 0 {
                    CURSOR_FADING_IN.store(false, Ordering::SeqCst);
                    CURSOR_VISIBLE.store(false, Ordering::SeqCst);

                    if cursor_fade_enabled() && !CURSOR_FADING_OUT.load(Ordering::SeqCst) {
                        CURSOR_FADING_OUT.store(true, Ordering::SeqCst);
                        spawn_fade_out_thread();

                        // Tell compositor "ok" but keep showing our cursor for the fade effect
                        let our_fb = CURSOR_FB_ID.load(Ordering::SeqCst);
                        if our_fb != 0 {
                            if let Some(func) = REAL_ATOMIC_ADD {
                                return func(req, object_id, property_id, our_fb as u64);
                            }
                        }
                    }

                    if let Some(func) = REAL_ATOMIC_ADD {
                        return func(req, object_id, property_id, 0);
                    }
                }

                CURSOR_FADING_OUT.store(false, Ordering::SeqCst);
                CURSOR_VISIBLE.store(true, Ordering::SeqCst);

                if CONFIG_FADE_IN_ENABLED.load(Ordering::Relaxed)
                    && !CURSOR_FADING_IN.load(Ordering::SeqCst)
                    && CURSOR_FADE_ALPHA.load(Ordering::SeqCst) < 255
                {
                    CURSOR_FADING_IN.store(true, Ordering::SeqCst);
                    CURSOR_FADE_ALPHA.store(10, Ordering::SeqCst);
                    spawn_fade_in_thread();
                } else {
                    CURSOR_FADE_ALPHA.store(255, Ordering::SeqCst);
                }

                let our_fb = CURSOR_FB_ID.load(Ordering::SeqCst);
                if our_fb != 0 {
                    debug_print!("Replacing FB_ID {} with our FB_ID {}", value, our_fb);
                    if let Some(func) = REAL_ATOMIC_ADD {
                        return func(req, object_id, property_id, our_fb as u64);
                    }
                } else {
                    debug_print!("FB_ID property matched but our FB_ID is 0!");
                }
            }

            if src_w_prop_id != 0 && property_id == src_w_prop_id {
                let our_src_w = (CURSOR_DISPLAY_SIZE as u64) << 16;
                debug_print!("Overriding SRC_W {} with {}", value, our_src_w);
                if let Some(func) = REAL_ATOMIC_ADD {
                    return func(req, object_id, property_id, our_src_w);
                }
            }

            if src_h_prop_id != 0 && property_id == src_h_prop_id {
                let our_src_h = (CURSOR_DISPLAY_SIZE as u64) << 16;
                debug_print!("Overriding SRC_H {} with {}", value, our_src_h);
                if let Some(func) = REAL_ATOMIC_ADD {
                    return func(req, object_id, property_id, our_src_h);
                }
            }

            if crtc_w_prop_id != 0 && property_id == crtc_w_prop_id {
                debug_print!("Overriding CRTC_W {} with {}", value, CURSOR_DISPLAY_SIZE);
                if let Some(func) = REAL_ATOMIC_ADD {
                    return func(req, object_id, property_id, CURSOR_DISPLAY_SIZE as u64);
                }
            }

            if crtc_h_prop_id != 0 && property_id == crtc_h_prop_id {
                debug_print!("Overriding CRTC_H {} with {}", value, CURSOR_DISPLAY_SIZE);
                if let Some(func) = REAL_ATOMIC_ADD {
                    return func(req, object_id, property_id, CURSOR_DISPLAY_SIZE as u64);
                }
            }
        }

        if let Some(func) = REAL_ATOMIC_ADD {
            return func(req, object_id, property_id, value);
        }
        return -1;
    }

    match REAL_ATOMIC_ADD {
        Some(func) => func(req, object_id, property_id, value),
        None => -1,
    }
}
