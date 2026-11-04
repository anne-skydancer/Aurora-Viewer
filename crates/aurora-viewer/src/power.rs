//! The computer's sleep and its screen, as the viewer wants them.
//!
//! Three things, each behind its own rule ([`wishes`]):
//!
//! - **The screen stays on while Aurora is in front** (setting
//!   `keep_screen_on`, off by default). Port of LLWindowWin32's
//!   `WM_SYSCOMMAND` (indra/llwindow/llwindowwin32.cpp, originally LGPL
//!   2.1): the window procedure eats `SC_SCREENSAVE` and `SC_MONITORPOWER`,
//!   which Windows sends to the foreground window before it starts the
//!   screen saver or turns the screen off. So it only acts while the window
//!   is in front, and it never keeps the computer from sleeping: the
//!   Legacy Viewer does the same, always and with no setting (a viewer left
//!   open behind should not keep a screen on unless asked: the justified
//!   difference). The idle timers of `SetThreadExecutionState` are not used
//!   for it: Windows counts every thread holding one, display or system,
//!   among what keeps the computer awake.
//! - **The computer stays awake while logged in** (setting `keep_awake`,
//!   off by default; Aurora-only, the Legacy Viewer has nothing like it).
//!   Windows: `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)`
//!   while logged in to a region, cleared on the login screen and at exit.
//!   It only holds the idle timer: a sleep the user asks for (Start menu,
//!   lid, power button) still happens, through the logout below. The
//!   state belongs to the calling thread: always the main thread, which
//!   lives as long as the viewer ([`apply`]).
//! - **A sleep while logged in logs out** (`WM_POWERBROADCAST`
//!   `PBT_APMSUSPEND`): the TODO of `LLViewerWindow::handleSuspendRequest`
//!   (indra/newview/llviewerwindow.cpp), which only sends the viewer
//!   statistics and lets the region drop the session a minute later. Here
//!   the session is asked for its normal logout (the same
//!   `NetCommand::Logout` as the user's: the last statistics report, then
//!   the LogoutRequest, `LLAppViewer::requestQuit`) and the suspend is held
//!   until the request has left, [`SUSPEND_HOLD`] at most, as
//!   LLWindowWin32 holds it `ms_sleep(1000)`; never for the LogoutReply.
//!   The crash session is marked `slept` meanwhile (crash/mod.rs), so a
//!   computer that never wakes leaves nothing to report. On resume the app
//!   finds the login screen with a note ([`take_sleep_logout`]).
//!
//! The clocks of the GPU stall notice and of the window watchdog do not
//! count the sleep either, on every system: they see it as a gap between
//! two of their own passes (app/gpu_stall.rs, crash/watchdog.rs). The
//! suspend and resume notices themselves are Windows' only for now.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// How long the system's suspend is held for the LogoutRequest to leave.
/// Windows gives about two seconds to answer `PBT_APMSUSPEND`.
pub const SUSPEND_HOLD: Duration = Duration::from_secs(1);

/// What the viewer asks of the system.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Wishes {
    /// The screen saver and the screen's power-off are kept off while the
    /// window is in front.
    pub screen_on: bool,
    /// The computer does not go to sleep from inactivity.
    pub awake: bool,
}

/// The rule: the screen follows its setting alone (Windows only sends the
/// screen saver notices to the foreground window), the computer is kept
/// awake only while logged in.
pub fn wishes(keep_screen_on: bool, keep_awake: bool, logged_in: bool) -> Wishes {
    Wishes {
        screen_on: keep_screen_on,
        awake: keep_awake && logged_in,
    }
}

/// `screen_on` of the wishes last applied, read by the window procedure.
static SCREEN_ON: AtomicBool = AtomicBool::new(false);
/// The session whose logout a sleep asks for, while logged in.
static LOGOUT: Mutex<Option<aurora_net::LogoutHandle>> = Mutex::new(None);
/// Resume notices so far; the app takes each one once.
static RESUMES: AtomicU64 = AtomicU64::new(0);
/// A sleep asked the session for its logout, not told to the user yet.
static SLEEP_LOGOUT: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// The wishes last given to the system, on the main thread.
    static APPLIED: std::cell::Cell<Option<Wishes>> = const { std::cell::Cell::new(None) };
    /// Resume notices the app has seen.
    static RESUMES_SEEN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Gives the system the wishes, when they changed. From the main thread
/// only: the execution state belongs to the thread that sets it.
pub fn apply(wishes: Wishes) {
    if APPLIED.with(|a| a.replace(Some(wishes))) == Some(wishes) {
        return;
    }
    SCREEN_ON.store(wishes.screen_on, Ordering::Release);
    log::info!(
        "power: screen kept on while in front {}, computer kept awake {}",
        if wishes.screen_on { "on" } else { "off" },
        if wishes.awake { "on" } else { "off" }
    );
    platform::execution_state(wishes.awake);
}

/// The session a sleep logs out, None on the login screen.
pub fn session(handle: Option<aurora_net::LogoutHandle>) {
    if let Ok(mut logout) = LOGOUT.lock() {
        *logout = handle;
    }
}

/// At exit, on every path: nothing asked of the system any more.
pub fn release() {
    apply(Wishes::default());
    session(None);
}

/// The system is about to sleep: the logout, with the suspend held until
/// the request has left ([`SUSPEND_HOLD`] at most). Called by the window
/// procedure on `PBT_APMSUSPEND`, and by `AURORA_DEMO_SLEEP`.
pub fn suspending() {
    log::info!("power: the computer is going to sleep");
    crate::crash::slept();
    let handle = LOGOUT.lock().ok().and_then(|logout| logout.clone());
    let Some(handle) = handle else {
        log::info!("power: not logged in, nothing to log out");
        return;
    };
    let before = handle.request();
    SLEEP_LOGOUT.store(true, Ordering::Release);
    log::info!("power: logout asked of the session, the suspend held for the LogoutRequest to leave");
    if handle.wait_sent(before, SUSPEND_HOLD) {
        log::info!("power: the LogoutRequest left before the sleep");
    } else {
        log::warn!(
            "power: the LogoutRequest was not confirmed sent within {} ms, sleeping anyway",
            SUSPEND_HOLD.as_millis()
        );
    }
}

/// The system is back from its sleep.
pub fn resuming() {
    log::info!("power: the computer woke up");
    crate::crash::woke();
    RESUMES.fetch_add(1, Ordering::AcqRel);
}

/// Once per frame on the main thread: a resume happened since the last call.
pub fn take_resumed() -> bool {
    let now = RESUMES.load(Ordering::Acquire);
    RESUMES_SEEN.with(|seen| seen.replace(now)) != now
}

/// The session was logged out by a sleep: true once, for the note on the
/// login screen.
pub fn take_sleep_logout() -> bool {
    SLEEP_LOGOUT.swap(false, Ordering::AcqRel)
}

/// Hears the system's notices on the main window: called for every main
/// window (at start and after a GPU recovery), on the thread that created
/// it, after `window_hold::install`.
pub fn install(window: &winit::window::Window) {
    platform::install(window);
}

#[cfg(windows)]
mod platform {
    use std::sync::atomic::{AtomicIsize, Ordering};

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::SC_SCREENSAVE;
    use windows_sys::Win32::System::Power::{ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, GWLP_WNDPROC, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND, SC_MONITORPOWER, SetWindowLongPtrW,
        WM_POWERBROADCAST, WM_SYSCOMMAND, WNDPROC,
    };
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    /// Window procedure in place before ours (window_hold's, the same for
    /// every window of the viewer).
    static PREVIOUS: AtomicIsize = AtomicIsize::new(0);

    /// `ES_SYSTEM_REQUIRED` held, or every requirement cleared: each call
    /// replaces the thread's whole state.
    pub fn execution_state(awake: bool) {
        let flags = ES_CONTINUOUS | if awake { ES_SYSTEM_REQUIRED } else { 0 };
        // SAFETY: a flag word, no memory involved.
        let previous = unsafe { SetThreadExecutionState(flags) };
        if previous == 0 {
            log::warn!("power: the system refused the execution state {flags:#x}");
        }
    }

    pub fn install(window: &winit::window::Window) {
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };
        let hwnd = handle.hwnd.get();
        // SAFETY: `hwnd` is a live window of this thread, and `hook` has the
        // signature of a window procedure. The previous procedure is stored
        // before any message can reach `hook`: the messages of this window
        // are only delivered on this thread.
        unsafe {
            let hook: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = hook;
            let previous = SetWindowLongPtrW(hwnd as HWND, GWLP_WNDPROC, hook as usize as isize);
            if previous == 0 {
                log::warn!("power: the window procedure could not be replaced, no sleep or screen saver notices");
                return;
            }
            let kept = PREVIOUS.load(Ordering::Acquire);
            if kept != 0 && kept != previous {
                SetWindowLongPtrW(hwnd as HWND, GWLP_WNDPROC, previous);
                log::warn!("power: a window with another procedure, left without the notices");
                return;
            }
            PREVIOUS.store(previous, Ordering::Release);
        }
    }

    unsafe extern "system" fn hook(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            // the low four bits of a system command are the system's
            WM_SYSCOMMAND if matches!(wparam as u32 & 0xFFF0, SC_SCREENSAVE | SC_MONITORPOWER) => {
                if super::SCREEN_ON.load(Ordering::Acquire) {
                    log::debug!(
                        "power: {} kept off while the window is in front",
                        if wparam as u32 & 0xFFF0 == SC_SCREENSAVE {
                            "the screen saver"
                        } else {
                            "the screen's power-off"
                        }
                    );
                    // eaten, as LLWindowWin32 does: the screen saver does
                    // not start, the screen stays on
                    return 0;
                }
            }
            WM_POWERBROADCAST => match wparam as u32 {
                PBT_APMSUSPEND => {
                    super::suspending();
                    return 1;
                }
                PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => {
                    super::resuming();
                    return 1;
                }
                _ => {}
            },
            _ => {}
        }
        // SAFETY: `PREVIOUS` holds the value `SetWindowLongPtrW` returned for
        // this window: a window procedure (or a handle to one, which
        // `CallWindowProcW` accepts), never 0 once the hook is installed;
        // the arguments are those Windows gave for this message.
        unsafe {
            let previous: WNDPROC = std::mem::transmute::<isize, WNDPROC>(PREVIOUS.load(Ordering::Acquire));
            CallWindowProcW(previous, hwnd, msg, wparam, lparam)
        }
    }
}

#[cfg(not(windows))]
mod platform {
    /// Nothing yet: macOS would hold an IOPMAssertion
    /// (`kIOPMAssertionTypePreventUserIdleSystemSleep`), Linux a logind
    /// `Inhibit("idle")` over D-Bus (TASKS.md).
    pub fn execution_state(_awake: bool) {}

    /// Nothing yet: macOS would listen to NSWorkspace's will-sleep and
    /// did-wake notifications, Linux to logind's `PrepareForSleep` with a
    /// delay inhibitor (TASKS.md).
    pub fn install(_window: &winit::window::Window) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_computer_is_kept_awake_only_while_logged_in_and_asked() {
        assert!(!wishes(false, false, false).awake);
        assert!(!wishes(false, true, false).awake, "setting on, on the login screen");
        assert!(!wishes(false, false, true).awake, "logged in, setting off");
        assert!(wishes(false, true, true).awake);
    }

    #[test]
    fn the_screen_follows_its_setting_alone_and_the_two_are_independent() {
        assert!(
            wishes(true, false, false).screen_on,
            "the login screen too: Windows only tells the window in front"
        );
        assert!(!wishes(true, false, true).awake);
        assert!(!wishes(false, true, true).screen_on);
    }

    #[test]
    fn a_resume_is_taken_once_and_a_sleep_logout_too() {
        let _ = take_resumed();
        assert!(!take_resumed());
        resuming();
        assert!(take_resumed());
        assert!(!take_resumed());
        // (one test for the flag: the tests run side by side)
        assert!(!take_sleep_logout());
        SLEEP_LOGOUT.store(true, Ordering::Release);
        assert!(take_sleep_logout());
        assert!(!take_sleep_logout());
        // without a session, a sleep logs nothing out
        session(None);
        suspending();
        assert!(!take_sleep_logout());
    }
}
