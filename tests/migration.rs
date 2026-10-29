use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root =
            gtk::glib::mkdtemp(std::env::temp_dir().join("animepahe-migration-XXXXXX")).unwrap();
        fs::create_dir_all(root.join("config/animepahe-dl")).unwrap();
        fs::create_dir(root.join("import")).unwrap();
        fs::write(root.join("import/events.jsonl"), "").unwrap();
        Self(root)
    }

    fn run(&self) {
        let result = Command::new(env!("CARGO_BIN_EXE_animepahe-dl"))
            .current_dir(&self.0)
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .args(["--import-session", "import", "--import-only"])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn updating_toml_preserves_legacy_files_that_were_not_loaded() {
    let fixture = Fixture::new();
    let app = fixture.0.join("config/animepahe-dl");
    fs::write(app.join("config.toml"), "quality = '720'\n").unwrap();
    fs::write(app.join("config.json"), r#"{"quality":"1080"}"#).unwrap();
    fs::write(fixture.0.join("config.json"), r#"{"unrelated":true}"#).unwrap();
    fixture.run();
    assert_eq!(
        fs::read_to_string(fixture.0.join("config.json")).unwrap(),
        r#"{"unrelated":true}"#
    );
    assert_eq!(
        fs::read_to_string(app.join("config.json")).unwrap(),
        r#"{"quality":"1080"}"#
    );
    let config: toml::Value = fs::read_to_string(app.join("config.toml"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(config["quality"].as_str(), Some("720"));
}

#[test]
fn first_run_ignores_unrelated_config_and_session_files() {
    for json in [r#"{"unrelated":true}"#, "not JSON"] {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("config.json"), json).unwrap();
        fs::create_dir(fixture.0.join(".session")).unwrap();
        fs::write(fixture.0.join(".session/unrelated"), "keep").unwrap();
        fixture.run();
        assert_eq!(
            fs::read_to_string(fixture.0.join("config.json")).unwrap(),
            json
        );
        assert_eq!(
            fs::read_to_string(fixture.0.join(".session/unrelated")).unwrap(),
            "keep"
        );
        assert!(
            !fs::read_to_string(fixture.0.join("config/animepahe-dl/config.toml"))
                .unwrap()
                .contains("unrelated")
        );
    }
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("config.json")).unwrap();
    fixture.run();
    assert!(fixture.0.join("config.json").is_dir());

    let fixture = Fixture::new();
    let _socket = std::os::unix::net::UnixListener::bind(fixture.0.join("config.json")).unwrap();
    fixture.run();
    assert!(fs::symlink_metadata(fixture.0.join("config.json")).is_ok());
}

#[test]
fn unreadable_project_config_is_migrated_after_path_is_repaired() {
    let fixture = Fixture::new();
    let source = fixture.0.join("config.json");
    // A symlink loop fails to read even when permission checks are bypassed as root.
    std::os::unix::fs::symlink("config.json", &source).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_animepahe-dl"))
        .current_dir(&fixture.0)
        .env("XDG_CONFIG_HOME", fixture.0.join("config"))
        .args(["--import-session", "import", "--import-only"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!fixture.0.join("config/animepahe-dl/config.toml").exists());
    assert!(fs::symlink_metadata(&source).unwrap().is_symlink());
    fs::remove_file(&source).unwrap();
    fs::write(
        &source,
        r#"{"animepahe_host":"https://example.test","quality":"480"}"#,
    )
    .unwrap();
    fixture.run();
    let config: toml::Value =
        fs::read_to_string(fixture.0.join("config/animepahe-dl/config.toml"))
            .unwrap()
            .parse()
            .unwrap();
    assert_eq!(config["quality"].as_str(), Some("480"));
    assert!(!source.exists());
}

#[test]
fn recognized_project_settings_migrate_without_overwriting_backups() {
    let fixture = Fixture::new();
    let legacy =
        r#"{"animepahe_host":"https://example.test","audio_language":"eng","ua":"old","cf":"old"}"#;
    fs::write(fixture.0.join("config.json"), legacy).unwrap();
    fs::set_permissions(
        fixture.0.join("config.json"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let original = fs::File::open(fixture.0.join("config.json")).unwrap();
    fs::write(fixture.0.join("config.json.migrated"), "old backup").unwrap();
    fs::create_dir(fixture.0.join("config.json.migrated.1")).unwrap();
    fs::create_dir(fixture.0.join(".session")).unwrap();
    fs::write(
        fixture.0.join(".session/cookies.txt"),
        "# Netscape HTTP Cookie File\n",
    )
    .unwrap();
    fs::write(fixture.0.join(".session/user-agent.txt"), "legacy-agent").unwrap();
    fixture.run();
    assert!(!fixture.0.join("config.json").exists());
    assert_eq!(
        fs::read_to_string(fixture.0.join("config.json.migrated.2")).unwrap(),
        legacy
    );
    assert_eq!(
        fs::read_to_string(fixture.0.join("config.json.migrated")).unwrap(),
        "old backup"
    );
    assert!(fixture.0.join("config.json.migrated.1").is_dir());
    assert_eq!(
        fs::metadata(fixture.0.join("config.json.migrated.2"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        original.metadata().unwrap().permissions().mode() & 0o777,
        0o644
    );
    let app = fixture.0.join("config/animepahe-dl");
    let config: toml::Value = fs::read_to_string(app.join("config.toml"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(config["audio_language"].as_str(), Some("eng"));
    assert_eq!(
        config["animepahe_host"].as_str(),
        Some("https://example.test")
    );
    assert!(config.get("ua").is_none() && config.get("cf").is_none());
    assert_eq!(
        fs::read_to_string(app.join("session/user-agent.txt")).unwrap(),
        "legacy-agent"
    );
    assert!(!fixture.0.join(".session").exists());
}

#[test]
fn app_legacy_config_takes_precedence_and_only_its_source_is_backed_up() {
    let fixture = Fixture::new();
    let app = fixture.0.join("config/animepahe-dl");
    fs::write(app.join("config.json"), r#"{"quality":"480"}"#).unwrap();
    fs::write(
        fixture.0.join("config.json"),
        r#"{"animepahe_host":"https://other.test"}"#,
    )
    .unwrap();
    fixture.run();
    assert!(!app.join("config.json").exists());
    assert_eq!(
        fs::read_to_string(app.join("config.json.migrated")).unwrap(),
        r#"{"quality":"480"}"#
    );
    assert!(fixture.0.join("config.json").exists());
    let config: toml::Value = fs::read_to_string(app.join("config.toml"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(config["quality"].as_str(), Some("480"));
}

#[test]
fn partial_configs_can_be_migrated_explicitly_from_the_app_config_directory() {
    for (key, value) in [
        ("quality", serde_json::json!("480")),
        (
            "output_directory",
            serde_json::json!("/tmp/custom-downloads"),
        ),
        ("concurrent_fragments", serde_json::json!(16)),
    ] {
        let fixture = Fixture::new();
        let legacy = serde_json::json!({key: value}).to_string();
        fs::write(fixture.0.join("config/animepahe-dl/config.json"), &legacy).unwrap();
        fixture.run();
        let config: toml::Value =
            fs::read_to_string(fixture.0.join("config/animepahe-dl/config.toml"))
                .unwrap()
                .parse()
                .unwrap();
        assert_eq!(serde_json::to_value(&config[key]).unwrap(), value);
        assert_eq!(
            fs::read_to_string(fixture.0.join("config/animepahe-dl/config.json.migrated")).unwrap(),
            legacy
        );
    }
}

#[test]
fn partial_legacy_session_keeps_cookies_and_creates_missing_user_agent() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join(".session")).unwrap();
    fs::write(
        fixture.0.join(".session/cookies.txt"),
        "# Netscape HTTP Cookie File\nexample.test\tFALSE\t/\tFALSE\t0\tsaved\tkeep\n",
    )
    .unwrap();
    fixture.run();
    let session = fixture.0.join("config/animepahe-dl/session");
    assert!(fs::read_to_string(session.join("cookies.txt"))
        .unwrap()
        .contains("saved\tkeep"));
    assert!(!fs::read_to_string(session.join("user-agent.txt"))
        .unwrap()
        .is_empty());
    assert!(!fixture.0.join(".session").exists());
}

#[test]
fn shared_setting_names_do_not_identify_an_application_config() {
    for legacy in [
        r#"{"quality":"high"}"#,
        r#"{"quality":"480"}"#,
        r#"{"output_directory":"/tmp/other-project"}"#,
        r#"{"concurrent_fragments":16}"#,
        r#"{"ua":"other-client","cf":"other-token"}"#,
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("config.json"), legacy).unwrap();
        fixture.run();
        assert_eq!(
            fs::read_to_string(fixture.0.join("config.json")).unwrap(),
            legacy
        );
        assert!(!fixture.0.join("config.json.migrated").exists());
        let config: toml::Value =
            fs::read_to_string(fixture.0.join("config/animepahe-dl/config.toml"))
                .unwrap()
                .parse()
                .unwrap();
        assert_eq!(config["quality"].as_str(), Some("best"));
        assert_eq!(config["output_directory"].as_str(), Some("."));
        assert_eq!(config["concurrent_fragments"].as_integer(), Some(8));
    }
}
