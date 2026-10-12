//! Minimal Unix FFI bindings and POSIX constants.
//!
//! Replaces external `libc` crate for atomic traversal and symlink boundary checks.

#![cfg(unix)]
#![allow(non_upper_case_globals, non_camel_case_types)]

pub use std::ffi::{c_char, c_int};
#[cfg(target_os = "linux")]
pub use std::ffi::c_long;

pub const O_RDONLY: c_int = 0;

#[cfg(target_os = "linux")]
pub const O_NONBLOCK: c_int = 0o4000;
#[cfg(not(target_os = "linux"))]
pub const O_NONBLOCK: c_int = 0x0004;

#[cfg(target_os = "linux")]
pub const O_DIRECTORY: c_int = 0o200000;
#[cfg(not(target_os = "linux"))]
pub const O_DIRECTORY: c_int = 0x100000;

#[cfg(target_os = "linux")]
pub const O_NOFOLLOW: c_int = 0o400000;
#[cfg(not(target_os = "linux"))]
pub const O_NOFOLLOW: c_int = 0x00000100;

#[cfg(target_os = "linux")]
pub const O_CLOEXEC: c_int = 0o2000000;
#[cfg(not(target_os = "linux"))]
pub const O_CLOEXEC: c_int = 0x1000000;

pub const ENXIO: c_int = 6;

#[cfg(target_os = "linux")]
pub const ELOOP: c_int = 40;
#[cfg(not(target_os = "linux"))]
pub const ELOOP: c_int = 62;

#[cfg(target_os = "linux")]
pub const EPERM: c_int = 1;
#[cfg(target_os = "linux")]
pub const EINVAL: c_int = 22;
#[cfg(target_os = "linux")]
pub const ENOSYS: c_int = 38;
#[cfg(target_os = "linux")]
pub const EOPNOTSUPP: c_int = 95;
#[cfg(target_os = "linux")]
pub const SYS_openat2: c_long = 437;

#[cfg(target_os = "linux")]
unsafe extern "C" {
    pub fn syscall(number: c_long, ...) -> c_long;
}

unsafe extern "C" {
    pub fn openat(dirfd: c_int, pathname: *const c_char, flags: c_int) -> c_int;
}

#[cfg(test)]
#[cfg(target_os = "linux")]
pub type mode_t = u32;
#[cfg(test)]
#[cfg(not(target_os = "linux"))]
pub type mode_t = u16;

#[cfg(test)]
unsafe extern "C" {
    pub fn mkfifo(pathname: *const c_char, mode: mode_t) -> c_int;
}
