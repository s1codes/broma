// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Windows single-instance startup and activation handoff.

use std::ffi::c_void;
use std::io;
use std::ptr;
use std::thread;

use crate::daemon_bus::{DaemonEvent, DaemonSender};

const MUTEX_NAME: &str = "Local\\io.github.s1codes.broma.SingleInstance";
const EVENT_NAME: &str = "Local\\io.github.s1codes.broma.Activate";

const ERROR_ALREADY_EXISTS: u32 = 183;
const EVENT_MODIFY_STATE: u32 = 0x0002;
const SYNCHRONIZE: u32 = 0x0010_0000;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_FAILED: u32 = 0xFFFF_FFFF;
const INFINITE: u32 = 0xFFFF_FFFF;
const ACTIVATION_RETRY_COUNT: usize = 100;
const ACTIVATION_RETRY_DELAY_MS: u32 = 10;
const WINDOW_RETRY_COUNT: usize = 600;
const SW_RESTORE: i32 = 9;

type Handle = *mut c_void;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateMutexW(attributes: *const c_void, initial_owner: i32, name: *const u16) -> Handle;
    fn CreateEventW(
        attributes: *const c_void,
        manual_reset: i32,
        initial_state: i32,
        name: *const u16,
    ) -> Handle;
    fn OpenEventW(access: u32, inherit_handle: i32, name: *const u16) -> Handle;
    fn SetEvent(event: Handle) -> i32;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn CloseHandle(handle: Handle) -> i32;
    fn GetLastError() -> u32;
    fn SetLastError(error: u32);
    fn Sleep(milliseconds: u32);
}

#[link(name = "user32")]
unsafe extern "system" {
    fn FindWindowW(class_name: *const u16, window_name: *const u16) -> Handle;
    fn ShowWindow(window: Handle, command: i32) -> i32;
    fn SetForegroundWindow(window: Handle) -> i32;
}

/// Holds the named kernel objects for the lifetime of the primary process.
pub struct SingleInstanceGuard {
    mutex: Handle,
    event: Handle,
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        // These handles were created successfully by `acquire` and remain owned
        // by this guard until shutdown.
        unsafe {
            let _ = CloseHandle(self.event);
            let _ = CloseHandle(self.mutex);
        }
    }
}

/// Acquires the process-wide guard, or notifies the already-running process.
///
/// `Ok(None)` means another instance owns the mutex and has been asked to show
/// its window. The bounded retry only handles the tiny startup interval between
/// creating the mutex and its activation event; no idle polling is used.
pub fn acquire() -> io::Result<Option<SingleInstanceGuard>> {
    let mutex_name = wide(MUTEX_NAME);
    // Reset the thread-local last-error value so a successful newly-created
    // mutex cannot be mistaken for an existing one due to an earlier API call.
    unsafe { SetLastError(0) };
    // SAFETY: The name is NUL-terminated UTF-16 and a null security descriptor
    // requests the current user's default security attributes.
    let mutex = unsafe { CreateMutexW(ptr::null(), 0, mutex_name.as_ptr()) };
    if mutex.is_null() {
        return Err(io::Error::last_os_error());
    }

    // No Win32 call may be made between `CreateMutexW` and this check because
    // `GetLastError` reports whether the named mutex already existed.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // SAFETY: `mutex` is a valid handle returned by CreateMutexW.
        unsafe {
            let _ = CloseHandle(mutex);
        }
        request_existing_instance_activation();
        return Ok(None);
    }

    let event_name = wide(EVENT_NAME);
    // Auto-reset event coalesces simultaneous launch requests and wakes exactly
    // one blocking listener in the primary process.
    // SAFETY: The name is NUL-terminated UTF-16 and the event arguments are valid.
    let event = unsafe { CreateEventW(ptr::null(), 0, 0, event_name.as_ptr()) };
    if event.is_null() {
        let error = io::Error::last_os_error();
        // SAFETY: `mutex` is a valid handle returned by CreateMutexW.
        unsafe {
            let _ = CloseHandle(mutex);
        }
        return Err(error);
    }

    Ok(Some(SingleInstanceGuard { mutex, event }))
}

/// Starts one blocking listener that forwards relaunches into iced's event bus.
pub fn start_listener(bus: DaemonSender) {
    static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }

    let name = wide(EVENT_NAME);
    // SAFETY: The event exists for the lifetime of the primary process and its
    // name is NUL-terminated UTF-16.
    let event = unsafe { OpenEventW(SYNCHRONIZE, 0, name.as_ptr()) };
    if event.is_null() {
        return;
    }

    let event_handle = event as usize;
    let spawn = thread::Builder::new()
        .name("broma-single-instance".to_string())
        .spawn(move || {
            let event = event_handle as Handle;
            loop {
                // SAFETY: `event` is a valid event handle owned by this thread.
                match unsafe { WaitForSingleObject(event, INFINITE) } {
                    WAIT_OBJECT_0 => bus.send(DaemonEvent::ExternalLaunch),
                    WAIT_FAILED => break,
                    _ => break,
                }
            }

            // SAFETY: `event` is a valid handle returned by OpenEventW.
            unsafe {
                let _ = CloseHandle(event);
            }
        });

    if spawn.is_err() {
        // SAFETY: If spawning failed, the closure did not take ownership of the
        // event handle.
        unsafe {
            let _ = CloseHandle(event);
        }
    }
}

fn request_existing_instance_activation() {
    let name = wide(EVENT_NAME);
    for attempt in 0..ACTIVATION_RETRY_COUNT {
        // SAFETY: `name` is NUL-terminated UTF-16 and access requests only the
        // right needed to signal the existing auto-reset event.
        let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
        if !event.is_null() {
            // SAFETY: `event` is a valid handle returned by OpenEventW.
            unsafe {
                let _ = SetEvent(event);
                let _ = CloseHandle(event);
            }
            activate_existing_window();
            return;
        }
        if attempt + 1 < ACTIVATION_RETRY_COUNT {
            // The primary creates the event immediately after its mutex. This
            // short bounded delay bridges only that startup race.
            // SAFETY: Sleep accepts any finite duration and has no pointer args.
            unsafe { Sleep(ACTIVATION_RETRY_DELAY_MS) };
        }
    }
}

/// The launching process has foreground permission, so it performs the actual
/// restore/focus after asking the primary process to open its window. This also
/// restores a window minimized to the taskbar, which iced's `gain_focus` alone
/// deliberately does not do.
fn activate_existing_window() {
    let title = wide(crate::APP_TITLE);
    for attempt in 0..WINDOW_RETRY_COUNT {
        // SAFETY: The class-name pointer is null by design; `title` is
        // NUL-terminated UTF-16.
        let window = unsafe { FindWindowW(ptr::null(), title.as_ptr()) };
        if !window.is_null() {
            // SAFETY: `window` is the HWND returned by FindWindowW.
            unsafe {
                let _ = ShowWindow(window, SW_RESTORE);
                let _ = SetForegroundWindow(window);
            }
            return;
        }
        if attempt + 1 < WINDOW_RETRY_COUNT {
            // SAFETY: Sleep accepts any finite duration and has no pointer args.
            unsafe { Sleep(ACTIVATION_RETRY_DELAY_MS) };
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
