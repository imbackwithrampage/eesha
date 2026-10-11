// The island's own Win32: its window styles, the cursor, session windows and
// keyboard layouts. Everything a feature may also need (file layout, clock,
// process and window lists) lives in notch-core and is re-exported here, so
// the app's modules keep one set of names.

pub mod keyhook;
mod windows;
pub use self::windows::*;

pub use notch_core::win::{
    config_dir, ensure_private_dir, find_on_path, home_dir, local_dir, local_time, no_console, open_url,
    picture_dirs, reveal_folder, user_sid as current_user_sid,
};
