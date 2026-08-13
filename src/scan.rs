//! Directory scan: every markdown file under a root, recursively, respecting
//! `.gitignore` and skipping `.git`/`node_modules` (§1, §7). Replaces the
//! macOS FileScanner/GitignoreParser with the `ignore` crate — the same
//! walker ripgrep uses.
//!
//! Results stream in batches over an async-channel so the Navigator shows
//! first rows while the walk is still running (§9.1: first rows ≤100 ms,
//! complete ≤1 s at 10k files).

use std::path::{Path, PathBuf};

const BATCH: usize = 128;

/// Walk `root` synchronously, invoking `on_batch` with groups of markdown
/// file paths (absolute). The channel wrapper below runs this on a thread;
/// tests call it directly.
pub fn scan_into(root: &Path, mut on_batch: impl FnMut(Vec<PathBuf>)) {
    let walker = ignore::WalkBuilder::new(root)
        // .gitignore applies even outside a git repository — the default
        // (true) silently ignores it there (§7).
        .require_git(false)
        .follow_links(false)
        .filter_entry(|entry| {
            let name = entry.file_name();
            name != ".git" && name != "node_modules"
        })
        .build();

    let mut batch = Vec::with_capacity(BATCH);
    for entry in walker.flatten() {
        let is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);
        if !is_file {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if !crate::langmap::is_markdown(&name) {
            continue;
        }
        batch.push(entry.into_path());
        if batch.len() >= BATCH {
            on_batch(std::mem::take(&mut batch));
            batch.reserve(BATCH);
        }
    }
    if !batch.is_empty() {
        on_batch(batch);
    }
}

/// Spawn the walk on a worker thread; markdown paths arrive in batches.
/// The channel closes when the walk completes.
pub fn scan_markdown(root: &Path) -> async_channel::Receiver<Vec<PathBuf>> {
    let (tx, rx) = async_channel::unbounded();
    let root = root.to_path_buf();
    std::thread::spawn(move || {
        scan_into(&root, |batch| {
            let _ = tx.send_blocking(batch);
        });
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;

    fn scan_all(root: &Path) -> BTreeSet<String> {
        let mut all = BTreeSet::new();
        scan_into(root, |batch| {
            for p in batch {
                all.insert(
                    p.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .to_string(),
                );
            }
        });
        all
    }

    fn tempdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("moremaid-scan-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn finds_markdown_recursively_and_skips_junk() {
        let root = tempdir("basic");
        fs::write(root.join("a.md"), "# a").unwrap();
        fs::write(root.join("b.markdown"), "# b").unwrap();
        fs::write(root.join("notes.txt"), "not markdown").unwrap();
        fs::create_dir_all(root.join("sub/deep")).unwrap();
        fs::write(root.join("sub/deep/c.md"), "# c").unwrap();
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::write(root.join("node_modules/pkg/readme.md"), "skip").unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/x.md"), "skip").unwrap();

        let got = scan_all(&root);
        let expected: BTreeSet<String> =
            ["a.md", "b.markdown", "sub/deep/c.md"].iter().map(|s| s.to_string()).collect();
        assert_eq!(got, expected);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn gitignore_respected_outside_a_git_repo() {
        // No .git directory here — require_git(false) is what makes this pass.
        let root = tempdir("gitignore");
        fs::write(root.join(".gitignore"), "ignored.md\nbuild/\n").unwrap();
        fs::write(root.join("kept.md"), "# kept").unwrap();
        fs::write(root.join("ignored.md"), "# ignored").unwrap();
        fs::create_dir_all(root.join("build")).unwrap();
        fs::write(root.join("build/gen.md"), "# generated").unwrap();

        let got = scan_all(&root);
        let expected: BTreeSet<String> = ["kept.md".to_string()].into_iter().collect();
        assert_eq!(got, expected);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn hidden_files_skipped() {
        let root = tempdir("hidden");
        fs::write(root.join("seen.md"), "# seen").unwrap();
        fs::create_dir_all(root.join(".cache")).unwrap();
        fs::write(root.join(".cache/h.md"), "# hidden").unwrap();
        fs::write(root.join(".hidden.md"), "# hidden file").unwrap();

        let got = scan_all(&root);
        let expected: BTreeSet<String> = ["seen.md".to_string()].into_iter().collect();
        assert_eq!(got, expected);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn broken_symlinks_treated_as_missing() {
        let root = tempdir("symlink");
        fs::write(root.join("real.md"), "# real").unwrap();
        std::os::unix::fs::symlink(root.join("gone.md"), root.join("link.md")).unwrap();

        let got = scan_all(&root);
        let expected: BTreeSet<String> = ["real.md".to_string()].into_iter().collect();
        assert_eq!(got, expected);
        let _ = fs::remove_dir_all(&root);
    }
}
