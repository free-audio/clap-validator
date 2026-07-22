use anyhow::{Context, Result};
use std::collections::HashMap;

#[derive(Debug, Default, serde::Deserialize)]
pub struct Config {
    pub test: HashMap<String, bool>,
}

impl Config {
    pub fn from_current() -> Result<Self> {
        // use env var if set
        if let Ok(path) = std::env::var("CLAP_VALIDATOR_CONFIG") {
            return Self::from_file(&path).context(path);
        }

        // scan up and look for clap-validator.toml
        let mut current_dir = std::env::current_dir()?.canonicalize()?;
        loop {
            let config_path = current_dir.join("clap-validator.toml");
            if config_path.exists() {
                return Self::from_file(&config_path);
            }

            if !current_dir.pop() {
                break;
            }
        }

        Ok(Self::default())
    }

    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
        basic_toml::from_str(&contents).with_context(|| path.display().to_string())
    }

    pub fn is_test_enabled(&self, test_name: &str) -> bool {
        self.test.get(test_name).copied().unwrap_or(true)
    }
}
