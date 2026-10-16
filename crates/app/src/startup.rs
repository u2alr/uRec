//! How the window is drawn, and what happens when the graphics driver can't do it.
//!
//! uRec draws with wgpu (Vulkan or Direct3D 12). On some PCs that fails: an old or broken
//! graphics driver, a missing Vulkan runtime, a remote or virtual display. So:
//!
//! - if the window can't be created, uRec retries at once with OpenGL (`Renderer::Glow`);
//! - if a start never got as far as drawing its first frame (the driver crashed the process),
//!   the next start uses OpenGL, and keeps using it;
//! - if nothing works, a message says so and where the log is, instead of closing silently.
//!
//! `UREC_RENDERER=wgpu` or `glow` overrides all of this (tests and troubleshooting).

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renderer {
    Wgpu,
    Glow,
}

impl Renderer {
    pub fn name(self) -> &'static str {
        match self {
            Renderer::Wgpu => "wgpu",
            Renderer::Glow => "glow",
        }
    }

    pub fn other(self) -> Renderer {
        match self {
            Renderer::Wgpu => Renderer::Glow,
            Renderer::Glow => Renderer::Wgpu,
        }
    }
}

/// Saved choice: "glow" = always OpenGL.
fn pref_file() -> PathBuf {
    crate::settings::local_dir().join("renderer.txt")
}

/// Exists from just before the window is created until its first frame is drawn; holds the
/// renderer that was being tried.
fn trial_file() -> PathBuf {
    crate::settings::local_dir().join("renderer-trial.txt")
}

fn read(path: PathBuf) -> String {
    std::fs::read_to_string(path).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Which renderer to try first.
pub fn choose() -> Renderer {
    match std::env::var("UREC_RENDERER").as_deref() {
        Ok("glow") => return Renderer::Glow,
        Ok("wgpu") => return Renderer::Wgpu,
        _ => {}
    }
    decide(&read(pref_file()), &read(trial_file()), |r| {
        match r {
            Renderer::Glow => {
                crate::controller::log("startup: the last start never drew a window; using OpenGL from now on");
                let _ = std::fs::create_dir_all(crate::settings::local_dir());
                let _ = std::fs::write(pref_file(), "glow");
            }
            Renderer::Wgpu => {
                crate::controller::log("startup: OpenGL didn't work last time either; trying the default again");
                let _ = std::fs::remove_file(pref_file());
            }
        };
    })
}

/// The decision itself, from the saved choice and the leftover trial marker (pure, for tests).
/// `changed` is told when the saved choice should be rewritten.
fn decide(pref: &str, trial: &str, changed: impl FnOnce(Renderer)) -> Renderer {
    match (pref, trial) {
        // A start with OpenGL never drew a frame: give the default another go.
        (_, "glow") => {
            changed(Renderer::Wgpu);
            Renderer::Wgpu
        }
        // A start with the default never drew a frame: use OpenGL from now on.
        (_, "wgpu") => {
            changed(Renderer::Glow);
            Renderer::Glow
        }
        ("glow", _) => Renderer::Glow,
        _ => Renderer::Wgpu,
    }
}

/// Call right before creating the window.
pub fn begin_trial(r: Renderer) {
    let _ = std::fs::create_dir_all(crate::settings::local_dir());
    let _ = std::fs::write(trial_file(), r.name());
}

/// Call when the first frame has been drawn: this renderer works.
pub fn first_frame_drawn() {
    let _ = std::fs::remove_file(trial_file());
}

/// The default renderer failed with an error and OpenGL worked: remember that.
pub fn remember_opengl() {
    let _ = std::fs::create_dir_all(crate::settings::local_dir());
    let _ = std::fs::write(pref_file(), "glow");
}

/// Tell the user the window couldn't be opened (never in CI, where nobody could click it away).
pub fn show_failure(error: &str) {
    controller_log_failure(error);
    if std::env::var_os("CI").is_some() || std::env::var_os("UREC_NO_DIALOG").is_some() {
        return;
    }
    let log = crate::settings::local_dir().join("logs").join("urec.log");
    urec_win::error_box(
        "uRec couldn't open its window",
        &format!(
            "uRec started, but the window couldn't be drawn. This is usually an out-of-date graphics driver.\n\n\
             Updating the driver from NVIDIA, AMD or Intel usually fixes it. If it still doesn't open, send the log:\n{}\n\n\
             Details: {error}",
            log.display()
        ),
    );
}

fn controller_log_failure(error: &str) {
    crate::controller::log(&format!("startup: no renderer worked: {error}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(pref: &str, trial: &str) -> (Renderer, Option<Renderer>) {
        let mut changed = None;
        let r = decide(pref, trial, |c| changed = Some(c));
        (r, changed)
    }

    #[test]
    fn default_is_wgpu_and_a_saved_choice_sticks() {
        assert_eq!(pick("", ""), (Renderer::Wgpu, None));
        assert_eq!(pick("glow", ""), (Renderer::Glow, None));
        assert_eq!(pick("something else", ""), (Renderer::Wgpu, None));
    }

    #[test]
    fn a_start_that_never_drew_switches_renderer() {
        // The default crashed the process: OpenGL, and keep it.
        assert_eq!(pick("", "wgpu"), (Renderer::Glow, Some(Renderer::Glow)));
        // OpenGL never drew either: the default again, and forget the saved choice.
        assert_eq!(pick("glow", "glow"), (Renderer::Wgpu, Some(Renderer::Wgpu)));
        assert_eq!(Renderer::Wgpu.other(), Renderer::Glow);
        assert_eq!(Renderer::Glow.other(), Renderer::Wgpu);
    }
}
