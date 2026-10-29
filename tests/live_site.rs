#[test]
#[ignore = "live network test against AnimePahe"]
fn fixed_application_handles_animepahe() {
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_animepahe-dl"))
        .args(["--url", "https://animepahe.ru/", "--http-only"])
        .output()
        .expect("Could not start the fixed application");
    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    print!("{stdout}");
    eprint!("{stderr}");
    if result.status.success() {
        assert!(stdout.contains("No browser started."));
    } else {
        assert!(
            stderr.contains("CAPTCHA detected; --http-only prevented"),
            "The application could not fetch/classify the live response: {stderr}"
        );
        println!(
            "The live response requires manual verification; the fixed application detected it."
        );
    }
}

#[test]
#[ignore = "live manual CAPTCHA test against AnimePahe; opens WebKit when challenged"]
fn manual_challenge_flow_on_animepahe() {
    println!("Starting the fixed application; manual verification may be required in its window.");
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_animepahe-dl"))
        .args(["--url", "https://animepahe.ru/"])
        .status()
        .expect("Could not start the fixed application");
    assert!(
        status.success(),
        "The application did not complete the live manual flow successfully"
    );
}
