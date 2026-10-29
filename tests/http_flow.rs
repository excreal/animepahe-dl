use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    time::Duration,
};

#[test]
fn cli_fetches_and_reuses_session_without_a_browser() {
    let root = std::env::temp_dir().join(format!("webkit-cli-test-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = vec![];
        for index in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut data = vec![0; 16384];
            let size = stream.read(&mut data).unwrap();
            requests.push(String::from_utf8_lossy(&data[..size]).to_string());
            let (status, headers, body) = if index == 2 {
                (
                    "403 Forbidden",
                    "cf-mitigated: challenge\r\n",
                    "<title>Just a moment...</title>",
                )
            } else {
                (
                    "200 OK",
                    "Set-Cookie: remembered=yes; Path=/; HttpOnly\r\n",
                    "<title>File details fixture</title><p>Saved without a browser</p>",
                )
            };
            write!(stream,"HTTP/1.1 {status}\r\n{headers}Content-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
        requests
    });
    for index in 0..3 {
        let mut command = Command::new(env!("CARGO_BIN_EXE_animepahe-dl"));
        command
            .current_dir(&root)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .args(["--url", &format!("http://{address}/"), "--capture"]);
        if index == 2 {
            command.arg("--http-only");
        }
        let result = command.output().unwrap();
        let stdout = String::from_utf8_lossy(&result.stdout);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert_eq!(result.status.success(), index != 2, "{stdout}\n{stderr}");
        if index != 2 {
            assert!(stdout.contains("No browser started."), "{stdout}");
        } else {
            assert!(stderr.contains("CAPTCHA detected; --http-only"), "{stderr}");
        }
        assert!(!stderr.contains("GTK"), "{stderr}");
        std::thread::sleep(Duration::from_millis(2));
    }
    let requests = server.join().unwrap();
    assert!(!requests[0].contains("remembered=yes"));
    assert!(requests[1].contains("remembered=yes"));
    assert!(requests[2].contains("remembered=yes"));
    let agents: Vec<_> = requests
        .iter()
        .map(|r| r.lines().find(|l| l.starts_with("User-Agent:")).unwrap())
        .collect();
    assert!(agents.iter().all(|a| *a == agents[0]));
    assert!(root
        .join("config/animepahe-dl/session/user-agent.txt")
        .exists());
    assert_eq!(
        fs::read_dir(root.join("config/animepahe-dl/captures"))
            .unwrap()
            .count(),
        3
    );
    fs::remove_dir_all(root).unwrap();
}
