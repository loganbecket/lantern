//! The three things Lantern does to files: trash, copy to Downloads, move.
//!
//! Every operation goes through GIO so it behaves like the file manager:
//! trash is the system trash, copies preserve nothing surprising, and moves
//! across devices fall back to copy-and-delete.

use gtk::{gio, glib};
use gtk::prelude::*;

/// What happened to each file, in the order given.
pub type Outcome = Vec<(gio::File, Result<gio::File, String>)>;

/// Move files to the trash. Never a hard delete.
pub async fn trash(files: Vec<gio::File>) -> Outcome {
    let mut outcome = Vec::with_capacity(files.len());
    for file in files {
        let result = file.trash_future(glib::Priority::DEFAULT).await;
        outcome.push((file.clone(), result.map(|_| file).map_err(|e| e.to_string())));
    }
    outcome
}

/// Copy files into the Downloads folder, never overwriting anything there.
pub async fn download(files: Vec<gio::File>) -> Outcome {
    let downloads = glib::user_special_dir(glib::UserDirectory::Downloads)
        .unwrap_or_else(|| glib::home_dir().join("Downloads"));
    let downloads = gio::File::for_path(downloads);

    let mut outcome = Vec::with_capacity(files.len());
    for file in files {
        let name = file.basename().unwrap_or_default();
        let target = unique_child(&downloads, &name.to_string_lossy());
        let (copy, _progress) = file.copy_future(&target, gio::FileCopyFlags::NONE, glib::Priority::DEFAULT);
        outcome.push((file, copy.await.map(|_| target).map_err(|e| e.to_string())));
    }
    outcome
}

/// Move files into `folder`, keeping their names. A file that would land on
/// an existing one is left alone and reported.
pub async fn move_into(files: Vec<gio::File>, folder: gio::File) -> Outcome {
    let mut outcome = Vec::with_capacity(files.len());
    for file in files {
        let name = file.basename().unwrap_or_default();
        let target = folder.child(&name);
        outcome.push((file.clone(), move_to(&file, target).await));
    }
    outcome
}

/// Move or rename one file to exactly `target`.
pub async fn move_to(file: &gio::File, target: gio::File) -> Result<gio::File, String> {
    if file.equal(&target) {
        return Ok(target);
    }
    if target.query_exists(gio::Cancellable::NONE) {
        return Err(format!("{} already exists", target.basename().unwrap_or_default().to_string_lossy()));
    }
    let (mv, _progress) = file.move_future(&target, gio::FileCopyFlags::NONE, glib::Priority::DEFAULT);
    mv.await.map(|_| target).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> gio::File {
        let dir = std::env::temp_dir().join(format!("lantern-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        gio::File::for_path(dir)
    }

    fn touch(dir: &gio::File, name: &str) -> gio::File {
        let file = dir.child(name);
        std::fs::write(file.path().unwrap(), b"x").unwrap();
        file
    }

    fn run<T>(future: impl std::future::Future<Output = T>) -> T {
        glib::MainContext::default().block_on(future)
    }

    #[test]
    fn unique_names_never_collide() {
        let dir = scratch("unique");
        assert_eq!(unique_child(&dir, "a.jpg").basename().unwrap().to_str(), Some("a.jpg"));
        touch(&dir, "a.jpg");
        assert_eq!(unique_child(&dir, "a.jpg").basename().unwrap().to_str(), Some("a (2).jpg"));
        touch(&dir, "a (2).jpg");
        assert_eq!(unique_child(&dir, "a.jpg").basename().unwrap().to_str(), Some("a (3).jpg"));
        touch(&dir, "noext");
        assert_eq!(unique_child(&dir, "noext").basename().unwrap().to_str(), Some("noext (2)"));
    }

    #[test]
    fn rename_and_move_refuse_to_overwrite() {
        let dir = scratch("move");
        let a = touch(&dir, "a.jpg");
        let b = touch(&dir, "b.jpg");

        // Rename in place.
        let renamed = run(move_to(&a, dir.child("c.jpg"))).unwrap();
        assert!(renamed.query_exists(gio::Cancellable::NONE));
        assert!(!a.query_exists(gio::Cancellable::NONE));

        // Onto an existing file: refused, both untouched.
        let err = run(move_to(&b, dir.child("c.jpg"))).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
        assert!(b.query_exists(gio::Cancellable::NONE));

        // Moving to itself is a no-op success.
        assert!(run(move_to(&b, b.clone())).is_ok());

        // Into another folder, keeping names.
        let other = scratch("move-dest");
        let outcome = run(move_into(vec![renamed.clone(), b.clone()], other.clone()));
        assert!(outcome.iter().all(|(_, r)| r.is_ok()));
        assert!(other.child("c.jpg").query_exists(gio::Cancellable::NONE));
        assert!(other.child("b.jpg").query_exists(gio::Cancellable::NONE));
        assert!(!renamed.query_exists(gio::Cancellable::NONE));
    }
}

/// `name`, or `name (2)`, `name (3)`, ... whichever doesn't exist in `dir`.
fn unique_child(dir: &gio::File, name: &str) -> gio::File {
    let candidate = dir.child(name);
    if !candidate.query_exists(gio::Cancellable::NONE) {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| dir.child(format!("{stem} ({n}){ext}")))
        .find(|f| !f.query_exists(gio::Cancellable::NONE))
        .unwrap()
}
