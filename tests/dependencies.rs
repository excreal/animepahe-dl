use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn missing_chrome_impersonation_stops_before_any_requests() {
    let root =
        gtk::glib::mkdtemp(std::env::temp_dir().join("animepahe-dependencies-XXXXXX")).unwrap();
    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    let script = "#!/bin/sh\ncase \"$*\" in *--list-impersonate-targets*) exit 1 ;; esac\nexit 0\n";
    for name in ["yt-dlp", "python3", "ffmpeg", "ffprobe"] {
        let file = bin.join(name);
        fs::write(&file, script).unwrap();
        fs::set_permissions(file, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let curl = bin.join("curl");
    fs::write(
        &curl,
        "#!/bin/sh\nif [ \"$1\" = --version ]; then exit 0; fi\ntouch request-started\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(curl, fs::Permissions::from_mode(0o700)).unwrap();
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let result = Command::new(env!("CARGO_BIN_EXE_animepahe-dl"))
        .current_dir(&root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("PATH", std::env::join_paths(paths).unwrap())
        .args(["-s", "fixture", "-e", "1"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("yt-dlp Chrome impersonation"));
    assert!(!root.join("request-started").exists());
    fs::remove_dir_all(root).unwrap();
}
