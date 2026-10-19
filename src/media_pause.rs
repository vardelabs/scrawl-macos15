//! Pauses Music and Spotify while a recording runs, so the narration isn't recorded over a
//! song, and resumes exactly the players it paused when the recording finishes or is cancelled.
//! Switching what is recorded keeps them paused. Through AppleScript: macOS asks once per
//! player whether Scrawl may control it, and a player that isn't running is never launched.
//! Off with `"pauseMusic": false` in `config.json`.

use std::process::Command;
use std::sync::Mutex;

/// Scripting names of the players this Scrawl paused and has not resumed yet.
static PAUSED: Mutex<Vec<String>> = Mutex::new(Vec::new());

const PLAYERS: [(&str, &str); 2] = [("music", "com.apple.Music"), ("spotify", "com.spotify.client")];

fn pause_script() -> String {
    let mut script = String::from("set paused to {}\n");
    for (name, bundle) in PLAYERS {
        script.push_str(&format!(
            "if application id \"{bundle}\" is running then\n\
             tell application id \"{bundle}\"\n\
             if player state is playing then\n\
             pause\n\
             set end of paused to \"{name}\"\n\
             end if\n\
             end tell\n\
             end if\n"
        ));
    }
    script.push_str("set AppleScript's text item delimiters to \",\"\nreturn paused as text\n");
    script
}

fn resume_script(names: &[String]) -> String {
    PLAYERS
        .iter()
        .filter(|(name, _)| names.iter().any(|paused| paused == name))
        .map(|(_, bundle)| {
            format!(
                "if application id \"{bundle}\" is running then\ntell application id \"{bundle}\" to play\nend if\n"
            )
        })
        .collect()
}

/// The player names an `osascript` run printed, comma separated.
fn parse_paused(stdout: &str) -> Vec<String> {
    stdout
        .trim()
        .split(',')
        .map(str::trim)
        .filter(|name| PLAYERS.iter().any(|(known, _)| known == name))
        .map(str::to_owned)
        .collect()
}

fn osascript(script: &str) -> Option<String> {
    let output = Command::new("/usr/bin/osascript").args(["-e", script]).output().ok()?;
    if !output.status.success() {
        crate::capture::scrawl_log("Pause music: osascript failed (not allowed to control the player?)".to_owned());
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// At a recording's start, off the main thread: pauses whatever is playing.
pub fn pause() {
    if !crate::handoff::pause_music() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("scrawl-pause-music".into())
        .spawn(|| {
            let Some(stdout) = osascript(&pause_script()) else {
                return;
            };
            let names = parse_paused(&stdout);
            if names.is_empty() {
                return;
            }
            crate::capture::scrawl_log(format!("Pause music: paused {}", names.join(", ")));
            let mut paused = PAUSED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            for name in names {
                if !paused.contains(&name) {
                    paused.push(name);
                }
            }
        });
}

/// When a recording finishes or is cancelled: plays again what `pause` paused.
pub fn resume() {
    let names = std::mem::take(&mut *PAUSED.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    if names.is_empty() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("scrawl-resume-music".into())
        .spawn(move || {
            if osascript(&resume_script(&names)).is_some() {
                crate::capture::scrawl_log(format!("Pause music: resumed {}", names.join(", ")));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_players_come_back_from_the_script() {
        assert_eq!(parse_paused("music, spotify\n"), vec!["music", "spotify"]);
        assert_eq!(parse_paused("\n"), Vec::<String>::new());
        assert_eq!(parse_paused("music,itunes"), vec!["music"]);
    }

    #[test]
    fn resume_plays_only_the_players_that_were_paused() {
        let script = resume_script(&["spotify".to_owned()]);
        assert!(script.contains("com.spotify.client"));
        assert!(!script.contains("com.apple.Music"));
        assert!(resume_script(&[]).is_empty());
    }

    #[test]
    fn the_pause_script_never_launches_a_player() {
        let script = pause_script();
        for (_, bundle) in PLAYERS {
            let guard = format!("if application id \"{bundle}\" is running then");
            let tell = format!("tell application id \"{bundle}\"");
            assert!(script.find(&guard).unwrap() < script.find(&tell).unwrap());
        }
    }
}
