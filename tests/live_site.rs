#[test]
#[ignore = "live network test; invokes the fixed application against idrama.dramafren.org"]
fn fixed_application_handles_another_site() {
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_animepahe-dl"))
        .args(["--url", "https://idrama.dramafren.org/", "--http-only"])
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
#[ignore = "live manual CAPTCHA test; opens the fixed application's WebKit window when challenged"]
fn manual_challenge_flow_on_another_site() {
    println!("Starting the fixed application; manual verification may be required in its window.");
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_animepahe-dl"))
        .args(["--url", "https://idrama.dramafren.org/"])
        .status()
        .expect("Could not start the fixed application");
    assert!(
        status.success(),
        "The application did not complete the live manual flow successfully"
    );
}
