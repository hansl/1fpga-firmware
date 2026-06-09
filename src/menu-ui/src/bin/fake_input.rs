//! `fake_input` — inject synthetic keyboard navigation events through
//! `uinput`, so the menu UI can be driven hands-free while capturing video
//! (e.g. on the JetKVM). Creates a virtual keyboard exposing the nav keys
//! and loops a key sequence.
//!
//! The menu maps standard evdev keycodes to intents (see
//! `menu-ui/src/input/router.rs`): arrows → navigate_*, Enter → confirm,
//! Esc → back. This tool taps those keys on a timer.
//!
//! IMPORTANT: start this BEFORE `menu_ui`. `menu_ui` enumerates
//! `/dev/input/event*` once at startup and does not hot-plug, so the
//! virtual device must already exist when it launches.
//!
//! Examples:
//!   fake_input                       # loop right,left every 250 ms
//!   fake_input --keys rrrrllll       # scroll right 4, left 4, repeat
//!   fake_input --keys rl --interval-ms 400 --count 50

// This small tool only needs clap + evdev from the menu-ui crate's
// dependency set; the workspace `unused_crate_dependencies` lint is a warn,
// so the other deps just produce (harmless) unused warnings for this bin.

use std::thread::sleep;
use std::time::Duration;

use clap::Parser;
use evdev::uinput::VirtualDeviceBuilder;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Flags {
    /// Key sequence to loop. Each char taps one key:
    /// l/r/u/d = arrows, c = confirm (Enter), b = back (Esc).
    #[clap(long, default_value = "rl")]
    keys: String,

    /// Milliseconds between the start of each tap.
    #[clap(long, default_value_t = 250)]
    interval_ms: u64,

    /// Key-hold duration per tap, in milliseconds.
    #[clap(long, default_value_t = 40)]
    hold_ms: u64,

    /// Number of times to loop the whole sequence (0 = forever).
    #[clap(long, default_value_t = 0)]
    count: u64,
}

fn key_for(c: char) -> Option<KeyCode> {
    match c {
        'l' => Some(KeyCode::KEY_LEFT),
        'r' => Some(KeyCode::KEY_RIGHT),
        'u' => Some(KeyCode::KEY_UP),
        'd' => Some(KeyCode::KEY_DOWN),
        'c' => Some(KeyCode::KEY_ENTER),
        'b' => Some(KeyCode::KEY_ESC),
        _ => None,
    }
}

fn main() -> std::io::Result<()> {
    let flags = Flags::parse();

    let seq: Vec<KeyCode> = flags.keys.chars().filter_map(key_for).collect();
    if seq.is_empty() {
        eprintln!("fake_input: no valid keys in --keys '{}' (use l/r/u/d/c/b)", flags.keys);
        std::process::exit(1);
    }

    let mut keys = AttributeSet::<KeyCode>::new();
    for k in [
        KeyCode::KEY_LEFT,
        KeyCode::KEY_RIGHT,
        KeyCode::KEY_UP,
        KeyCode::KEY_DOWN,
        KeyCode::KEY_ENTER,
        KeyCode::KEY_ESC,
    ] {
        keys.insert(k);
    }

    let mut device = VirtualDeviceBuilder::new()?
        .name("fake-input menu nav")
        .with_keys(&keys)?
        .build()?;

    eprintln!(
        "fake_input: virtual keyboard created. Start menu_ui NOW \
         (it scans input devices once at startup)."
    );
    eprintln!(
        "fake_input: looping keys=\"{}\" every {} ms (hold {} ms){}",
        flags.keys,
        flags.interval_ms,
        flags.hold_ms,
        if flags.count == 0 {
            ", forever (Ctrl+C to stop)".to_string()
        } else {
            format!(", {} loops", flags.count)
        },
    );

    // Give udev + menu_ui a moment to enumerate the new device before the
    // first key, so the opening taps aren't dropped.
    sleep(Duration::from_millis(1500));

    let gap = Duration::from_millis(flags.interval_ms.saturating_sub(flags.hold_ms));
    let hold = Duration::from_millis(flags.hold_ms);

    let mut loops = 0u64;
    loop {
        for &k in &seq {
            device.emit(&[InputEvent::new(EventType::KEY.0, k.code(), 1)])?;
            sleep(hold);
            device.emit(&[InputEvent::new(EventType::KEY.0, k.code(), 0)])?;
            sleep(gap);
        }
        loops += 1;
        if flags.count != 0 && loops >= flags.count {
            break;
        }
    }
    Ok(())
}
