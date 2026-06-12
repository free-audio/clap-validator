#[test]
fn validate_clack_effect() {
    validate("clack-effect", TestType::ValidateOk);
}

#[test]
fn validate_clack_synth() {
    validate("clack-synth", TestType::ValidateOk);
}

#[test]
fn fuzz_clack_effect() {
    validate("clack-effect", TestType::Fuzz);
}

#[test]
fn fuzz_clack_synth() {
    validate("clack-synth", TestType::Fuzz);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestType {
    ValidateOk,
    ValidateFail,
    Fuzz,
}

/// Runs the validator on the specified package and checks that it behaves as expected.
fn validate(package: &str, test: TestType) {
    use std::fs::{copy, create_dir_all, write};
    use std::process::{Command, Stdio};

    let output = Command::new("cargo")
        .args(["build", "--package", package])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .output()
        .unwrap();

    assert!(output.status.success(), "Cargo build failed for package '{}'", package);

    let dylib_path = if cfg!(target_os = "windows") {
        format!("target/debug/{}.dll", package.replace('-', "_"))
    } else if cfg!(target_os = "macos") {
        format!("target/debug/lib{}.dylib", package.replace('-', "_"))
    } else if cfg!(target_os = "linux") {
        format!("target/debug/lib{}.so", package.replace('-', "_"))
    } else {
        panic!("Unsupported operating system");
    };

    let plugin_path = if cfg!(target_os = "macos") {
        let target_out = format!("target/debug/{}.clap", package);

        create_dir_all(format!("{}/Contents/MacOS", target_out)).unwrap();
        copy(&dylib_path, format!("{}/Contents/MacOS/{}", target_out, package)).unwrap();
        write(format!("{}/Contents/PkgInfo", target_out), "BNDL????").unwrap();
        write(
            format!("{}/Contents/Info.plist", target_out),
            format!(
                r#"
            <?xml version="1.0" encoding="UTF-8"?>
            <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
            <plist version="1.0">
            <dict>
                <key>CFBundleName</key>
                <string>{package}</string>
                <key>CFBundleExecutable</key>
                <string>{package}</string>
                <key>CFBundleIdentifier</key>
                <string>com.example.{package}</string>
                <key>CFBundleVersion</key>
                <string>1.0</string>
            </dict>
            </plist>
        "#
            ),
        )
        .unwrap();

        target_out
    } else {
        dylib_path
    };

    match test {
        TestType::ValidateOk | TestType::ValidateFail => {
            let output = Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "clap-validator",
                    "--",
                    "validate",
                    "--only-failed",
                    &plugin_path,
                ])
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .output()
                .unwrap();

            if test == TestType::ValidateFail {
                assert!(
                    !output.status.success(),
                    "Validation unexpectedly succeeded for '{}'",
                    package
                );
            } else {
                assert!(output.status.success(), "Validation failed for '{}'", package);
            }
        }

        TestType::Fuzz => {
            let output = Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "clap-validator",
                    "--",
                    "fuzz",
                    "-d",
                    "10s",
                    &plugin_path,
                ])
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .output()
                .unwrap();

            assert!(output.status.success(), "Fuzzing failed for '{}'", package);
        }
    }
}
