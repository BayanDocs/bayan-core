//! The engine configuration ([engine protocol specification][spec] §3.3).
//!
//! The host hands the engine all of its configuration when it creates it, as a JSON object; the engine reads no environment variables (architecture §12). Every field is optional. Unknown fields at the top level are ignored, so a newer shell can configure an older engine, but an unknown or misspelt field inside `test` makes creation fail, so a test can never silently run the normal path.
//!
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md

use std::fmt;

use serde::Deserialize;

use crate::limits::MAX_MESSAGE_BYTES;

/// How an engine is configured.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Config {
    /// Whether `diag.panic` makes the engine panic (`test.allow_panic`), for tests of the error path (spec §12).
    pub allow_panic: bool,
}

/// Why a configuration was refused. The reason is a fixed description, never part of the configuration itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigError {
    reason: &'static str,
}

impl ConfigError {
    /// The reason, for logs and error messages.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        self.reason
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.reason)
    }
}

impl std::error::Error for ConfigError {}

#[derive(Deserialize)]
struct ConfigFile {
    #[serde(default)]
    test: Option<TestSwitches>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TestSwitches {
    #[serde(default)]
    allow_panic: bool,
}

impl Config {
    /// Reads a configuration from JSON. Empty input means the defaults.
    ///
    /// # Errors
    ///
    /// Fails if the input is larger than a message may be, is not a JSON object, has a value of the wrong type, or has an unknown field inside `test`.
    pub fn from_json(json: &[u8]) -> Result<Self, ConfigError> {
        if json.is_empty() {
            return Ok(Self::default());
        }
        if json.len() > MAX_MESSAGE_BYTES {
            return Err(ConfigError {
                reason: "the configuration is larger than 16 MiB",
            });
        }
        let value: serde_json::Value = serde_json::from_slice(json).map_err(|_| ConfigError {
            reason: "the configuration is not valid JSON",
        })?;
        if !value.is_object() {
            return Err(ConfigError {
                reason: "the configuration is not a JSON object",
            });
        }
        let file: ConfigFile = serde_json::from_value(value).map_err(|_| ConfigError {
            reason: "the configuration has an unknown test switch or a value of the wrong type",
        })?;
        Ok(Self {
            allow_panic: file.test.is_some_and(|test| test.allow_panic),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_and_an_empty_object_mean_the_defaults() {
        assert_eq!(Config::from_json(b""), Ok(Config::default()));
        assert_eq!(Config::from_json(b"{}"), Ok(Config::default()));
        assert_eq!(Config::from_json(b"{\"test\":null}"), Ok(Config::default()));
    }

    #[test]
    fn reads_the_test_switch() {
        let config = Config::from_json(br#"{"test": {"allow_panic": true}}"#).unwrap();
        assert!(config.allow_panic);
    }

    #[test]
    fn ignores_unknown_top_level_fields() {
        let config = Config::from_json(br#"{"later": {"anything": 1}}"#).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn refuses_unknown_test_switches_and_wrong_types() {
        for bad in [
            &br#"{"test": {"allow_panics": true}}"#[..],
            br#"{"test": {"allow_panic": "yes"}}"#,
            br#"{"test": true}"#,
            b"[]",
            b"null",
            b"{",
        ] {
            assert!(
                Config::from_json(bad).is_err(),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }
}
