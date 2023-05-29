//! parsing nix.conf

use anyhow::Context;
use std::collections::{hash_map::Entry, HashMap};

/// A Key-value representation of nix.conf
pub type NixConfig = HashMap<String, String>;

/// Parse the current nix config by running nix show-config
///
/// Concatenates together the extra-* options
pub async fn get_nix_config() -> anyhow::Result<NixConfig> {
    let mut cmd = tokio::process::Command::new("nix");
    cmd.args([
        "--extra-experimental-features",
        "nix-command",
        "show-config",
    ]);
    let output = cmd.output().await.context("running nix show-config")?;
    anyhow::ensure!(
        output.status.success(),
        "nix show-config failed: {:?} {} {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let out = String::from_utf8(output.stdout).context("nix show-config returned non utf8 data")?;
    parse_nix_config(&out)
}

fn parse_nix_config(text: &str) -> anyhow::Result<NixConfig> {
    let mut result = NixConfig::new();
    for line in text.split("\n") {
        if let Some(cut) = line.find("=") {
            let key = &line[..cut].trim();
            let value = &line[(cut + 1)..].trim();
            if key.starts_with("extra-") {
                let key = &key[6..];
                let entry = result.entry(key.to_string());
                entry
                    .and_modify(|before| {
                        before.push_str(" ");
                        before.push_str(value);
                    })
                    .or_insert_with(|| value.to_string());
            } else {
                let entry = result.entry(key.to_string());
                match entry {
                    Entry::Occupied(_) => {
                        anyhow::bail!("several values for nix config entry {}", key)
                    }
                    Entry::Vacant(e) => e.insert(value.to_string()),
                };
            }
        }
    }
    Ok(result)
}

#[test]
fn nix_config() {
    let config = r#"
                                foo = bar
                                # comment
                                baz = complex"#;
    let expected = maplit::hashmap! { "foo".to_string() => "bar".to_string(), "baz".to_string() => "complex".to_string() };
    assert_eq!(parse_nix_config(config).unwrap(), expected);
}

#[test]
fn nix_config_extra_empty() {
    let config = r#"extra-experimental-features = nix-command"#;
    let expected =
        maplit::hashmap! { "experimental-features".to_string() => "nix-command".to_string() };
    assert_eq!(parse_nix_config(config).unwrap(), expected);
}

#[test]
fn nix_config_extra() {
    let config = r#"
        experimental-features = flakes
        extra-experimental-features = nix-command"#;
    let expected = maplit::hashmap! { "experimental-features".to_string() => "flakes nix-command".to_string() };
    assert_eq!(parse_nix_config(config).unwrap(), expected);
}
