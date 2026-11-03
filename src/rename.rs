//! Lyssna was called SpotiFlex. On the first start after the rename, this
//! moves what SpotiFlex kept under its name to Lyssna's: settings, caches,
//! the interface's storage, the sign-in startup entry and, on Windows, the
//! install itself. The interface's storage also moves from earlier app
//! identifiers to the current one. Saved credentials move when first read (see
//! `credential`).
use std::{
    fs, io,
    path::{Path, PathBuf},
};

const FORMER_NAME: &str = "SpotiFlex";
const NAME: &str = "Lyssna";
/// Identifiers the app had before, oldest first.
const FORMER_IDENTIFIERS: [&str; 2] = ["xyz.clypdat.spotiflex", "xyz.clypdat.lyssna"];
const IDENTIFIER: &str = "app.lyssna.desktop";

/// Move SpotiFlex's folders to Lyssna's. Runs before anything reads them.
pub fn migrate() {
    for (from, to) in folders() {
        if let Err(error) = move_folder(&from, &to) {
            log::warn!(
                "Cannot move {} to {}: {error}",
                from.display(),
                to.display()
            );
        }
    }
    #[cfg(windows)]
    if let Err(error) = crate::autostart::migrate() {
        log::warn!("Cannot move the startup entry: {error:#}");
    }
}

/// Each SpotiFlex folder and where Lyssna keeps it.
fn folders() -> Vec<(PathBuf, PathBuf)> {
    let mut pairs = Vec::new();
    if let (Some(former), Some(current)) = (
        directories::ProjectDirs::from("", "", FORMER_NAME),
        directories::ProjectDirs::from("", "", NAME),
    ) {
        pairs.extend(
            [
                (former.config_dir(), current.config_dir()),
                (former.data_dir(), current.data_dir()),
                (former.data_local_dir(), current.data_local_dir()),
                (former.cache_dir(), current.cache_dir()),
            ]
            .map(|(from, to)| (from.to_path_buf(), to.to_path_buf())),
        );
    }
    // The interface's storage and window placement, named by the app's
    // identifier.
    if let Some(base) = directories::BaseDirs::new() {
        for dir in [
            base.config_dir(),
            base.data_dir(),
            base.data_local_dir(),
            base.cache_dir(),
        ] {
            for former in FORMER_IDENTIFIERS {
                pairs.push((dir.join(former), dir.join(IDENTIFIER)));
            }
        }
        // WebKit names its storage by the bundle identifier, which an app
        // updated in place keeps until it is installed again.
        #[cfg(target_os = "macos")]
        if crate::mac_bundle::identifier().as_deref() == Some(IDENTIFIER) {
            let webkit = base.home_dir().join("Library/WebKit");
            for former in FORMER_IDENTIFIERS {
                pairs.push((webkit.join(former), webkit.join(IDENTIFIER)));
            }
        }
    }
    let mut seen = Vec::new();
    pairs.retain(|(from, _)| {
        let new = !seen.contains(from);
        seen.push(from.clone());
        new
    });
    pairs
}

/// Move a folder to Lyssna's name. When Lyssna already has one (a build
/// can create it first), what it lacks moves in and nothing of its own is
/// replaced. Whatever is left of SpotiFlex's then goes: Lyssna never reads
/// it. A parent left empty is removed.
fn move_folder(from: &Path, to: &Path) -> io::Result<()> {
    if !from.is_dir() {
        return Ok(());
    }
    if to.exists() {
        merge_folder(from, to)?;
    } else {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        move_entry(from, to)?;
    }
    if let Some(parent) = from.parent()
        && parent.file_name().is_some_and(|name| name == FORMER_NAME)
    {
        let _ = fs::remove_dir(parent);
    }
    Ok(())
}

fn merge_folder(from: &Path, to: &Path) -> io::Result<()> {
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if !target.exists() {
            move_entry(&entry.path(), &target)?;
        } else if entry.file_type()?.is_dir() && target.is_dir() {
            merge_folder(&entry.path(), &target)?;
        } else {
            // Lyssna's own copy wins.
            remove_entry(&entry.path());
        }
    }
    let _ = fs::remove_dir(from);
    Ok(())
}

/// Rename, retrying briefly: a scanner or indexer can hold a file for a
/// moment just after SpotiFlex closed. Otherwise copy (another volume, or a
/// file held open) and remove what copied.
fn move_entry(from: &Path, to: &Path) -> io::Result<()> {
    for wait in [0, 100, 250, 500] {
        std::thread::sleep(std::time::Duration::from_millis(wait));
        if fs::rename(from, to).is_ok() {
            return Ok(());
        }
    }
    if from.is_dir() {
        copy_folder(from, to)?;
    } else {
        fs::copy(from, to)?;
    }
    remove_entry(from);
    Ok(())
}

fn copy_folder(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_folder(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn remove_entry(path: &Path) {
    let _ = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
}

/// Where `build.ps1` installs Lyssna, and where it installed SpotiFlex.
#[cfg(windows)]
fn installs() -> Option<(PathBuf, PathBuf)> {
    let programs = directories::BaseDirs::new()?
        .data_local_dir()
        .join("Programs");
    Some((
        programs.join(FORMER_NAME).join("spotiflex.exe"),
        programs.join(NAME).join("lyssna.exe"),
    ))
}

/// An install updated in place still runs as SpotiFlex's executable. Copy it
/// to Lyssna's install and start that copy, which removes the old one once
/// this copy has exited. True when this copy should exit now.
#[cfg(windows)]
pub fn relocate() -> bool {
    let Some((former, current)) = installs() else {
        return false;
    };
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    if !same_file(&exe, &former) {
        return false;
    }
    let copied = current
        .parent()
        .is_some_and(|dir| fs::create_dir_all(dir).is_ok())
        && fs::copy(&exe, &current).is_ok();
    if !copied {
        return false;
    }
    let mut command = std::process::Command::new(&current);
    command
        .arg(crate::updater::AFTER_EXIT_ARG)
        .arg(std::process::id().to_string())
        .args(std::env::args().skip(1).filter(|argument| {
            // The previous copy's own wait is already over.
            argument != crate::updater::AFTER_EXIT_ARG && argument.parse::<u32>().is_err()
        }));
    command.spawn().is_ok()
}

/// Remove SpotiFlex's install once Lyssna runs from its own.
#[cfg(windows)]
pub fn remove_former_install() {
    let Some((former, current)) = installs() else {
        return;
    };
    if std::env::current_exe().is_ok_and(|exe| same_file(&exe, &current))
        && let Some(dir) = former.parent()
        && dir.is_dir()
    {
        // Only SpotiFlex's own files: the executable and its staging copies.
        if fs::remove_file(&former).is_ok() || !former.exists() {
            let _ = fs::remove_dir_all(dir);
        }
    }
}

#[cfg(windows)]
fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_move_and_merge_without_replacing_lyssnas() {
        let root = std::env::temp_dir().join(format!("lyssna-rename-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let from = root.join(FORMER_NAME).join("config");
        let to = root.join(NAME).join("config");
        fs::create_dir_all(from.join("resume")).unwrap();
        fs::write(from.join("settings.json"), "{}").unwrap();
        fs::write(from.join("resume/a.json"), "1").unwrap();
        move_folder(&from, &to).unwrap();
        assert_eq!(fs::read_to_string(to.join("settings.json")).unwrap(), "{}");
        assert_eq!(fs::read_to_string(to.join("resume/a.json")).unwrap(), "1");
        assert!(
            !root.join(FORMER_NAME).exists(),
            "empty old parent is removed"
        );
        // Into a folder Lyssna already has: what it lacks moves in, and
        // nothing of its own is replaced.
        fs::create_dir_all(from.join("build-signing")).unwrap();
        fs::write(from.join("settings.json"), "old").unwrap();
        fs::write(from.join("build-signing/identity.json"), "id").unwrap();
        move_folder(&from, &to).unwrap();
        assert_eq!(fs::read_to_string(to.join("settings.json")).unwrap(), "{}");
        assert_eq!(
            fs::read_to_string(to.join("build-signing/identity.json")).unwrap(),
            "id"
        );
        // A copy left by an earlier start is cleared, not walked again.
        assert!(
            !root.join(FORMER_NAME).exists(),
            "old duplicates are removed"
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
