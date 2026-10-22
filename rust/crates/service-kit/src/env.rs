//! Configuration from an explicit env file (MIGRATION_PLAN.md M25, X7). A service reads only the
//! file it is given with `--env-file`; nothing is loaded from the working directory. As with
//! Node's `--env-file`, a variable set in the process environment wins over the file.

use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct Env {
    values: HashMap<String, String>,
}

#[derive(Debug, thiserror::Error)]
pub enum EnvError {
    #[error("{path}: {message}")]
    File { path: String, message: String },
    #[error("{name}: {message}")]
    Value { name: String, message: String },
}

impl Env {
    /// The file's variables (if a file is given), overridden by the process environment.
    pub fn load(file: Option<&Path>) -> Result<Self, EnvError> {
        let mut values = HashMap::new();
        if let Some(path) = file {
            let file_error = |message: String| EnvError::File {
                path: path.display().to_string(),
                message,
            };
            for item in dotenvy::from_path_iter(path).map_err(|e| file_error(e.to_string()))? {
                let (name, value) = item.map_err(|e| file_error(e.to_string()))?;
                values.insert(name, value);
            }
        }
        values.extend(std::env::vars());
        Ok(Self { values })
    }

    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self {
            values: pairs.into_iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect(),
        }
    }

    /// Every variable, as set.
    pub fn vars(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// The value as set, also when it is empty.
    pub fn raw(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// The value, or `None` when it is unset or empty (the Node code treats both alike).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str).filter(|v| !v.is_empty())
    }

    /// Parses a set value; `None` when unset or empty.
    pub fn parse<T>(&self, name: &str) -> Result<Option<T>, EnvError>
    where
        T: std::str::FromStr,
        T::Err: std::fmt::Display,
    {
        self.get(name)
            .map(|text| {
                text.trim().parse().map_err(|e: T::Err| EnvError::Value {
                    name: name.to_owned(),
                    message: e.to_string(),
                })
            })
            .transpose()
    }

    /// A switch: `1` is on, `0` is off, unset is `default`; anything else is an error.
    pub fn flag(&self, name: &str, default: bool) -> Result<bool, EnvError> {
        match self.get(name) {
            None => Ok(default),
            Some("1") => Ok(true),
            Some("0") => Ok(false),
            Some(other) => Err(EnvError::Value {
                name: name.to_owned(),
                message: format!("{other:?} is not 0 or 1"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_parse_or_fail_with_the_variable_name() {
        let env = Env::from_pairs([("PORT", "61000"), ("BAD", "x"), ("EMPTY", ""), ("ON", "1")]);
        assert_eq!(env.parse::<u16>("PORT").unwrap(), Some(61000));
        assert_eq!(env.parse::<u16>("EMPTY").unwrap(), None);
        assert!(env.parse::<u16>("BAD").unwrap_err().to_string().starts_with("BAD:"));
        assert!(env.flag("ON", false).unwrap());
        assert!(!env.flag("MISSING", false).unwrap());
        assert!(env.flag("BAD", false).is_err());
    }
}
