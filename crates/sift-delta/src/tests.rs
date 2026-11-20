use super::*;
use std::path::Path;

fn write(root: &Path, rel: &str, content: &[u8]) {
    let path = local(root, rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// Bytes that compress poorly, so a patch has to find them in the base.
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// Two releases: a big program that changes a little, a file that stays, one
/// that goes, one that comes, and (on Unix) a link and an executable bit.
fn releases(dir: &Path) -> (PathBuf, PathBuf) {
    let (old, new) = (dir.join("old"), dir.join("new"));
    let program = noise(3 << 20, 7);
    let mut changed = program.clone();
    changed[1000..1010].copy_from_slice(b"0123456789");
    changed.extend_from_slice(b"more at the end");
    write(&old, "bin/app", &program);
    write(&new, "bin/app", &changed);
    write(&old, "lib/same.dat", b"unchanged");
    write(&new, "lib/same.dat", b"unchanged");
    write(&old, "gone.txt", b"only in the old one");
    write(&new, "res/new.txt", b"only in the new one");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for root in [&old, &new] {
            fs::set_permissions(local(root, "bin/app"), fs::Permissions::from_mode(0o755)).unwrap();
            std::os::unix::fs::symlink("lib/same.dat", root.join("link")).unwrap();
        }
    }
    (old, new)
}

/// Every file, link and mode under `root`, file contents by hash so a mismatch prints briefly.
fn listing(root: &Path) -> Vec<(String, String, u32)> {
    raw_listing(root).into_iter().map(|(path, content, mode)| (path, sha256_hex(&content), mode)).collect()
}

fn raw_listing(root: &Path) -> Vec<(String, Vec<u8>, u32)> {
    let mut all = Vec::new();
    fn visit(root: &Path, dir: &Path, all: &mut Vec<(String, Vec<u8>, u32)>) {
        let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
        entries.sort();
        for path in entries {
            let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            let meta = path.symlink_metadata().unwrap();
            #[cfg(unix)]
            let mode = std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o777;
            #[cfg(not(unix))]
            let mode = 0;
            if meta.file_type().is_symlink() {
                all.push((rel, fs::read_link(&path).unwrap().to_string_lossy().into_owned().into_bytes(), 0));
            } else if meta.is_dir() {
                all.push((rel, Vec::new(), mode));
                visit(root, &path, all);
            } else {
                all.push((rel, fs::read(&path).unwrap(), mode));
            }
        }
    }
    visit(root, root, &mut all);
    all
}

#[test]
fn build_makes_the_new_release() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = releases(dir.path());
    let bytes = make(&old, &new, "1.0.0", "1.1.0").unwrap();
    assert!(bytes.len() < 100_000, "a 3 MB program changed in two places gave {} bytes", bytes.len());
    let delta = Delta::parse(&bytes).unwrap();
    assert_eq!((delta.manifest.version.as_str(), delta.manifest.from.as_str()), ("1.1.0", "1.0.0"));
    assert_eq!(delta.manifest.removed, ["gone.txt"]);
    let out = dir.path().join("out");
    delta.build(&old, &out).unwrap();
    assert_eq!(listing(&out), listing(&new));
}

#[test]
fn stage_and_commit_change_only_what_differs() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = releases(dir.path());
    let delta = Delta::parse(&make(&old, &new, "1.0.0", "1.1.0").unwrap()).unwrap();
    // Files the installer made that no release lists stay.
    write(&old, "uninstall.exe", b"installer's own");
    write(&new, "uninstall.exe", b"installer's own");
    let stage = old.join(".update");
    delta.build(&old, &stage).unwrap();
    Staged::prepare(&old, &stage, &delta.manifest, &delta.manifest.removed).unwrap();
    // Nothing installed is touched before the commit, which can come from a later launch.
    assert_eq!(fs::read(old.join("gone.txt")).unwrap(), b"only in the old one");
    let staged = Staged::load(&old, &stage).unwrap();
    assert_eq!(staged.version, "1.1.0");
    assert_eq!(staged.removed, ["gone.txt"]);
    assert!(!staged.changed.iter().any(|p| p == "lib/same.dat"), "{:?}", staged.changed);
    staged.commit().unwrap();
    assert!(!stage.exists());
    assert!(old.join(format!("bin/app{OLD_SUFFIX}")).exists());
    sweep(&old);
    assert_eq!(listing(&old), listing(&new));
}

/// Deltas follow one another: each is built on what the last one made, and
/// the installed release then takes in what differs from the end result.
#[test]
fn deltas_chain() {
    let dir = tempfile::tempdir().unwrap();
    let (v1, v2) = releases(dir.path());
    let v3 = dir.path().join("v3");
    for (path, content, _) in raw_listing(&v2) {
        let at = local(&v2, &path);
        if at.symlink_metadata().unwrap().file_type().is_file() {
            write(&v3, &path, &content);
        }
    }
    let mut program = fs::read(v3.join("bin/app")).unwrap();
    program[2000..2004].copy_from_slice(b"v3v3");
    write(&v3, "bin/app", &program);
    // One file goes in v2 and comes back in v3, one goes in v3.
    write(&v3, "gone.txt", b"back again");
    fs::remove_file(v3.join("lib/same.dat")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(v3.join("bin/app"), fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("res/new.txt", v3.join("link")).unwrap();
    }
    let d12 = Delta::parse(&make(&v1, &v2, "1", "2").unwrap()).unwrap();
    let d23 = Delta::parse(&make(&v2, &v3, "2", "3").unwrap()).unwrap();
    let (t2, t3) = (dir.path().join("t2"), dir.path().join("t3"));
    d12.build(&v1, &t2).unwrap();
    d23.build(&t2, &t3).unwrap();
    assert_eq!(listing(&t3), listing(&v3));
    let removed: Vec<String> = d12.manifest.removed.iter().chain(&d23.manifest.removed).cloned().collect();
    let staged = Staged::prepare(&v1, &t3, &d23.manifest, &removed).unwrap();
    assert_eq!(staged.removed, ["lib/same.dat"]);
    staged.commit().unwrap();
    sweep(&v1);
    assert_eq!(listing(&v1), listing(&v3));
}
#[test]
fn a_wrong_base_is_refused_before_anything_changes() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = releases(dir.path());
    let delta = Delta::parse(&make(&old, &new, "1.0.0", "1.1.0").unwrap()).unwrap();
    write(&old, "bin/app", b"some other build");
    let err = delta.build(&old, &dir.path().join("out")).unwrap_err();
    assert!(err.to_string().contains("bin/app"), "{err}");
    // An unchanged file that was altered fails too.
    write(&old, "bin/app", &noise(3 << 20, 7));
    write(&old, "lib/same.dat", b"tampered");
    let err = delta.build(&old, &dir.path().join("out2")).unwrap_err();
    assert!(err.to_string().contains("lib/same.dat"), "{err}");
}
#[test]
fn a_damaged_delta_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = releases(dir.path());
    let mut bytes = make(&old, &new, "1.0.0", "1.1.0").unwrap();
    // Flip a byte in the middle of the program's patch.
    let (at, len) = tar::Archive::new(bytes.as_slice())
        .entries()
        .unwrap()
        .map(Result::unwrap)
        .find(|e| e.path().unwrap().to_string_lossy() == "data/0")
        .map(|e| (e.raw_file_position() as usize, e.size() as usize))
        .unwrap();
    bytes[at + len / 2] ^= 0x55;
    let delta = Delta::parse(&bytes).unwrap();
    assert!(delta.build(&old, &dir.path().join("out")).is_err());
}

#[test]
fn paths_leaving_the_folder_are_refused() {
    for path in ["../evil", "/etc/passwd", "a/../../b", "a\\b", "", "a//b", "./a"] {
        assert!(checked(path).is_err(), "{path:?} passed");
    }
    assert!(checked("Contents/MacOS/sift").is_ok());
    let manifest = Manifest {
        version: "2".into(),
        from: "1".into(),
        entries: vec![Entry::Symlink { path: "x".into(), target: "/etc".into() }],
        removed: vec![],
    };
    let mut tar = tar::Builder::new(Vec::new());
    let json = serde_json::to_vec(&manifest).unwrap();
    let mut header = tar::Header::new_gnu();
    header.set_size(json.len() as u64);
    header.set_cksum();
    tar.append_data(&mut header, MANIFEST, json.as_slice()).unwrap();
    assert!(Delta::parse(&tar.into_inner().unwrap()).is_err());
}

#[test]
fn a_failed_commit_puts_everything_back() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = releases(dir.path());
    let delta = Delta::parse(&make(&old, &new, "1.0.0", "1.1.0").unwrap()).unwrap();
    let before = listing(&old);
    let stage = dir.path().join("stage");
    delta.build(&old, &stage).unwrap();
    let staged = Staged::prepare(&old, &stage, &delta.manifest, &delta.manifest.removed).unwrap();
    // The last staged file goes missing, so the commit fails partway.
    let last = staged.changed.last().unwrap().clone();
    fs::remove_file(local(&stage, &last)).unwrap();
    assert!(staged.commit().is_err());
    sweep(&old);
    // Folders made for the new files may stay behind, empty.
    let files = |list: Vec<(String, String, u32)>| list.into_iter().filter(|(p, ..)| !local(&old, p).is_dir()).collect::<Vec<_>>();
    assert_eq!(files(listing(&old)), files(before));
}

#[test]
fn swap_exchanges_folders() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a"), dir.path().join("b"));
    write(&a, "f", b"a");
    write(&b, "f", b"b");
    swap(&a, &b).unwrap();
    assert_eq!(fs::read(a.join("f")).unwrap(), b"b");
    assert_eq!(fs::read(b.join("f")).unwrap(), b"a");
}

/// Windows lets a running program be renamed but not replaced: committing
/// over one that is running works, and its old copy goes once it has exited.
#[cfg(windows)]
#[test]
fn commit_replaces_a_running_program() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = (dir.path().join("old"), dir.path().join("new"));
    let system = std::env::var("SystemRoot").unwrap();
    let ping = fs::read(Path::new(&system).join("System32").join("PING.EXE")).unwrap();
    write(&old, "app.exe", &ping);
    let mut changed = ping.clone();
    changed.extend_from_slice(b"appended");
    write(&new, "app.exe", &changed);
    let delta = Delta::parse(&make(&old, &new, "1", "2").unwrap()).unwrap();
    let mut running = std::process::Command::new(old.join("app.exe")).args(["-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
    assert!(fs::remove_file(old.join("app.exe")).is_err(), "a running program should not be deletable");
    let stage = old.join(".update");
    delta.build(&old, &stage).unwrap();
    Staged::prepare(&old, &stage, &delta.manifest, &delta.manifest.removed).unwrap().commit().unwrap();
    assert_eq!(fs::read(old.join("app.exe")).unwrap(), changed);
    running.kill().unwrap();
    running.wait().unwrap();
    sweep(&old);
    assert!(!old.join(format!("app.exe{OLD_SUFFIX}")).exists());
}
