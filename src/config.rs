//! The server's `enshrouded_server.json`, changed by a JSON merge patch
//! (RFC 7396) from the environment.
//!
//! The server fills in every key the file lacks and writes it back on
//! start, so a patch names only what the host wants to set, and a key the
//! game adds later needs no change here.

use std::path::Path;

use serde_json::{Map, Value};

/// Merges `changes`, a JSON object, into the file at `path`, created as `{}`
/// when it does not exist yet.
///
/// # Errors
///
/// A patch that is not a JSON object, a file that is not one, or a file
/// that cannot be read or written.
pub(crate) fn apply(path: &Path, changes: &str) -> Result<(), String> {
    let wanted: Value = serde_json::from_str(changes)
        .map_err(|error| format!("ENSHWICK_SERVER_CONFIG is not JSON: {error}"))?;
    if !wanted.is_object() {
        return Err("ENSHWICK_SERVER_CONFIG must be a JSON object".to_owned());
    }
    let mut config = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|error| format!("{}: not JSON: {error}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    if !config.is_object() {
        return Err(format!("{}: not a JSON object", path.display()));
    }
    merge(&mut config, &wanted);
    let text = serde_json::to_string_pretty(&config).map_err(|error| error.to_string())?;
    std::fs::write(path, text + "\n").map_err(|error| format!("{}: {error}", path.display()))
}

/// RFC 7396: an object merges key by key, `null` removes a key, anything
/// else replaces the target.
fn merge(target: &mut Value, patch: &Value) {
    let Value::Object(changes) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(Map::new());
    }
    let Value::Object(fields) = target else {
        return;
    };
    for (key, change) in changes {
        if change.is_null() {
            fields.remove(key);
        } else {
            merge(fields.entry(key.clone()).or_insert(Value::Null), change);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_follows_rfc_7396() {
        // The examples of RFC 7396, appendix A, that use objects.
        let cases = [
            (json!({"a": "b"}), json!({"a": "c"}), json!({"a": "c"})),
            (
                json!({"a": "b"}),
                json!({"b": "c"}),
                json!({"a": "b", "b": "c"}),
            ),
            (json!({"a": "b"}), json!({"a": null}), json!({})),
            (
                json!({"a": "b", "b": "c"}),
                json!({"a": null}),
                json!({"b": "c"}),
            ),
            (json!({"a": ["b"]}), json!({"a": "c"}), json!({"a": "c"})),
            (json!({"a": "c"}), json!({"a": ["b"]}), json!({"a": ["b"]})),
            (
                json!({"a": {"b": "c"}}),
                json!({"a": {"b": "d", "c": null}}),
                json!({"a": {"b": "d"}}),
            ),
            (
                json!({"a": [{"b": "c"}]}),
                json!({"a": [1]}),
                json!({"a": [1]}),
            ),
            (
                json!({"e": null}),
                json!({"a": 1}),
                json!({"e": null, "a": 1}),
            ),
            (
                json!([1, 2]),
                json!({"a": "b", "c": null}),
                json!({"a": "b"}),
            ),
            (
                json!({}),
                json!({"a": {"bb": {"ccc": null}}}),
                json!({"a": {"bb": {}}}),
            ),
        ];
        for (mut target, patch, expected) in cases {
            merge(&mut target, &patch);
            assert_eq!(target, expected, "patch {patch}");
        }
    }

    #[test]
    fn apply_keeps_what_the_patch_does_not_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("enshrouded_server.json");
        std::fs::write(
            &path,
            r#"{"name": "old", "slotCount": 16, "gameSettings": {"a": 1}}"#,
        )
        .unwrap();
        apply(&path, r#"{"name": "new", "gameSettings": {"b": 2}}"#).unwrap();
        let config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            config,
            json!({"name": "new", "slotCount": 16, "gameSettings": {"a": 1, "b": 2}})
        );
    }

    #[test]
    fn apply_creates_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("enshrouded_server.json");
        apply(&path, r#"{"name": "first"}"#).unwrap();
        let config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(config, json!({"name": "first"}));
    }

    #[test]
    fn apply_refuses_a_patch_that_is_not_an_object() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("enshrouded_server.json");
        assert!(apply(&path, "[1]").is_err());
        assert!(apply(&path, "{").is_err());
        assert!(!path.exists());
    }
}
