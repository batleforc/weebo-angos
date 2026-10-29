//! A status is written with a JSON merge patch (RFC 7396): objects merge,
//! so a key the status held is only removed when the patch sends it as
//! `null`.

use serde_json::{Map, Value};

/// Sends `null` for every key `previous` holds that `next` lacks, at every
/// depth where both are objects (an array or a scalar is replaced whole).
/// `previous` is the status the patch applies to.
pub fn null_removed(previous: &Map<String, Value>, next: &mut Map<String, Value>) {
    for (key, old) in previous {
        match next.get_mut(key) {
            None => {
                next.insert(key.clone(), Value::Null);
            }
            Some(Value::Object(new)) => {
                if let Value::Object(old) = old {
                    null_removed(old, new);
                }
            }
            Some(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn object(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(map) => map,
            _ => panic!("an object"),
        }
    }

    #[test]
    fn a_removed_key_is_nulled_at_any_depth() {
        let previous = object(json!({"a": {"x": 1, "y": {"z": 2}}, "b": [1]}));
        let mut next = object(json!({"a": {"y": {}}, "b": []}));
        null_removed(&previous, &mut next);
        assert_eq!(
            Value::Object(next),
            json!({"a": {"x": null, "y": {"z": null}}, "b": []})
        );
    }

    #[test]
    fn an_unchanged_object_gets_no_null() {
        let previous = object(json!({"a": {"x": 1}}));
        let mut next = previous.clone();
        null_removed(&previous, &mut next);
        assert_eq!(next, previous);
    }
}
