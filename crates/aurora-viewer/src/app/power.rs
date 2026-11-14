//! The computer's sleep and screen as the app drives them (power.rs): the
//! wishes given to the system each frame, the session a sleep logs out,
//! the note of the login screen after it, and `AURORA_DEMO_SLEEP`.

use super::{App, Screen};
use std::time::Duration;

/// `AURORA_DEMO_SLEEP=<frame>[,<seconds>]`: at that frame the computer
/// "goes to sleep" (the suspend notice, the logout path), the whole viewer
/// stands still for that long (16 s by default, like the sleep that showed
/// the false stall notice), then "wakes up".
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct SleepDemo {
    at: u64,
    seconds: f32,
    done: bool,
}

impl SleepDemo {
    pub(super) fn from_env() -> Option<SleepDemo> {
        std::env::var("AURORA_DEMO_SLEEP").ok().and_then(|v| SleepDemo::parse(&v))
    }

    fn parse(value: &str) -> Option<SleepDemo> {
        let mut parts = value.split(',');
        let at = parts.next()?.trim().parse().ok()?;
        let seconds = parts.next().map_or(Some(16.0), |s| s.trim().parse().ok())?;
        (seconds >= 0.0).then_some(SleepDemo { at, seconds, done: false })
    }
}

/// The note of the login screen after a sleep's logout.
fn sleep_note() -> String {
    aurora_i18n::tr!("app-slept-logged-out").to_string()
}

impl App {
    /// Each frame, before it takes the window.
    pub(super) fn power_frame(&mut self) {
        let logged_in = self.in_world();
        if logged_in != self.power_logged_in {
            self.power_logged_in = logged_in;
            // the session a sleep logs out (the demo's too: its offline
            // session only logs the command, the path is the same)
            crate::power::session(logged_in.then(|| self.net.logout_handle()));
        }
        crate::power::apply(crate::power::wishes(
            self.settings.keep_screen_on,
            self.settings.keep_awake,
            logged_in,
        ));
        self.demo_sleep_frame();
        // on the grid the region answers the logout (LogoutReply, or the
        // session's own 6 s) and `NetEvent::LoggedOut` shows the note; the
        // demo has no region: its session ends here
        if crate::power::take_resumed() && self.demo && logged_in && crate::power::take_sleep_logout() {
            self.session_ended();
            self.back_to_login(Some(sleep_note()));
        }
    }

    /// The note for the login screen when the session was logged out by a
    /// sleep (true once), for the end of the session however it comes.
    pub(super) fn sleep_logout_note(&self) -> Option<String> {
        crate::power::take_sleep_logout().then(sleep_note)
    }

    fn demo_sleep_frame(&mut self) {
        let Some(demo) = &mut self.demo_sleep else {
            return;
        };
        if demo.done || self.frame_count < demo.at || !matches!(self.screen, Screen::World) {
            return;
        }
        demo.done = true;
        let seconds = demo.seconds;
        log::info!(
            "AURORA_DEMO_SLEEP: the computer goes to sleep for {seconds} s at frame {}",
            self.frame_count
        );
        crate::power::suspending();
        let pause = Duration::from_secs_f32(seconds);
        crate::crash::expect_pause(pause);
        std::thread::sleep(pause);
        crate::power::resuming();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_takes_a_frame_and_an_optional_length() {
        assert_eq!(
            SleepDemo::parse("600"),
            Some(SleepDemo {
                at: 600,
                seconds: 16.0,
                done: false
            })
        );
        assert_eq!(
            SleepDemo::parse("600, 3.5"),
            Some(SleepDemo {
                at: 600,
                seconds: 3.5,
                done: false
            })
        );
        assert_eq!(SleepDemo::parse(""), None);
        assert_eq!(SleepDemo::parse("soon"), None);
        assert_eq!(SleepDemo::parse("600,-1"), None);
    }
}
