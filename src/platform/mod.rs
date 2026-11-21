//! Everything that differs between operating systems / desktops, behind one small
//! API that each platform module implements:
//!
//! - `Placement`, `cursor()`, `placement()`, `restore_cursor()`: where the dial goes
//! - `launch()`: run a dial item
//! - `keep_rules_applied()` / `apply_rules()`: compositor window rules, if any
//! - `theme_palette()`, `default_font()`, `default_items()`: platform-flavoured defaults
//! - `running_as_root()`

#[cfg(target_os = "linux")]
mod hypr;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::*;
