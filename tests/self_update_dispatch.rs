use std::error::Error;
use std::process::Command;

#[test]
fn version_is_claimed_by_the_binary_entrypoint() -> Result<(), Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_ll-desktop-cli"))
        .arg("--version")
        .output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert_eq!(
        stdout.trim(),
        concat!("ll-desktop-cli ", env!("CARGO_PKG_VERSION"))
    );
    return Ok(());
}

#[test]
fn self_update_help_bypasses_product_parser() -> Result<(), Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_ll-desktop-cli"))
        .args(["self-update", "--help"])
        .output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("Usage: ll-desktop-cli self-update [VERSION] [OPTIONS]"));
    return Ok(());
}
