use serde_json::Value;

use super::schema_types::{
    Check, Node, SchemaMismatch, enumeration, equal, error, exact_fields, field, integer, mismatch,
    object, string, text_field,
};

#[derive(Debug)]
pub(crate) struct SchemaValidator {
    schema: Value,
}

impl SchemaValidator {
    pub(crate) fn new(schema_json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(schema_json).map(|schema| Self { schema })
    }

    pub(crate) fn command(&self, value: &Value) -> Check {
        let operation = text_field(value, "operation", "command")?;
        let path = format!("commands.{operation}");
        let specification = self.pointer(&format!("/commands/{operation}"), &path)?;
        self.object_with_fixed(
            Node {
                value,
                specification,
                path: &path,
            },
            &[
                ("version", Value::from(1)),
                ("operation", Value::from(operation)),
            ],
        )
    }

    pub(crate) fn response(&self, name: &str, value: &Value) -> Check {
        let path = format!("responses.{name}");
        let specification = self.pointer(&format!("/responses/{name}"), &path)?;
        self.node(Node {
            value,
            specification,
            path: &path,
        })
    }

    pub(crate) fn event(&self, value: &Value) -> Check {
        let event_type = text_field(value, "type", "event")?;
        let path = format!("events.{event_type}");
        let changed = self.pointer(&format!("/events/{event_type}/fields/changed"), &path)?;
        let revision = self.pointer("/types/revision", &path)?;
        let object = object(value, &path)?;
        exact_fields(object, &["type", "revision", "changed"], &path)?;
        equal(object.get("type"), Some(&Value::from(event_type)), &path)?;
        self.node(Node {
            value: field(object, "revision", &path)?,
            specification: revision,
            path: &format!("{path}.revision"),
        })?;
        self.node(Node {
            value: field(object, "changed", &path)?,
            specification: changed,
            path: &format!("{path}.changed"),
        })
    }

    fn node(&self, node: Node<'_>) -> Check {
        if let Some(reference) = node.specification.get("ref").and_then(Value::as_str) {
            let target = self.pointer(&format!("/types/{reference}"), node.path)?;
            return self.node(Node {
                specification: target,
                ..node
            });
        }
        if let Some(nullable) = node.specification.get("nullable") {
            return if node.value.is_null() {
                Ok(())
            } else {
                self.node(Node {
                    specification: nullable,
                    ..node
                })
            };
        }
        if let Some(literal) = node.specification.get("literal") {
            return equal(Some(node.value), Some(literal), node.path);
        }
        if node.specification.get("one_of").and_then(Value::as_str) == Some("results") {
            return self.result(node.value, node.path);
        }
        match node.specification.get("type").and_then(Value::as_str) {
            Some("array") => self.array(node),
            Some("boolean") if node.value.is_boolean() => Ok(()),
            Some("boolean") => mismatch(node.path, "expected boolean"),
            Some("enum") => enumeration(node),
            Some("integer") => integer(node),
            Some("object") => self.object(node),
            Some("string") => string(node),
            None if node.specification.get("fields").is_some() => self.object(node),
            other => mismatch(node.path, &format!("unsupported schema node {other:?}")),
        }
    }

    fn result(&self, value: &Value, path: &str) -> Check {
        let object = object(value, path)?;
        exact_fields(object, &["type", "payload"], path)?;
        let result_type = text_field(value, "type", path)?;
        let payload = self.pointer(&format!("/results/{result_type}/payload"), path)?;
        self.node(Node {
            value: field(object, "payload", path)?,
            specification: payload,
            path: &format!("{path}.{result_type}.payload"),
        })
    }

    fn object(&self, node: Node<'_>) -> Check {
        self.object_with_fixed(node, &[])
    }

    fn object_with_fixed(&self, node: Node<'_>, fixed: &[(&str, Value)]) -> Check {
        let object = object(node.value, node.path)?;
        let fields = node
            .specification
            .get("fields")
            .and_then(Value::as_object)
            .ok_or_else(|| error(node.path, "object fields must exist"))?;
        let required = node
            .specification
            .get("required")
            .and_then(Value::as_array)
            .ok_or_else(|| error(node.path, "required fields must exist"))?;
        let mut names = required
            .iter()
            .map(|name| {
                name.as_str()
                    .ok_or_else(|| error(node.path, "required field must be text"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        names.extend(fixed.iter().map(|(name, _)| *name));
        exact_fields(object, &names, node.path)?;
        for (name, expected) in fixed {
            equal(
                object.get(*name),
                Some(expected),
                &format!("{}.{}", node.path, name),
            )?;
        }
        for (name, specification) in fields {
            self.node(Node {
                value: field(object, name, node.path)?,
                specification,
                path: &format!("{}.{}", node.path, name),
            })?;
        }
        Ok(())
    }

    fn array(&self, node: Node<'_>) -> Check {
        let items = node
            .value
            .as_array()
            .ok_or_else(|| error(node.path, "expected array"))?;
        if let Some(maximum) = node.specification.get("max_items").and_then(Value::as_u64) {
            let maximum =
                usize::try_from(maximum).map_err(|_| error(node.path, "invalid bound"))?;
            if items.len() > maximum {
                return mismatch(node.path, "array exceeds bound");
            }
        }
        let specification = node
            .specification
            .get("items")
            .ok_or_else(|| error(node.path, "array items must exist"))?;
        for (index, value) in items.iter().enumerate() {
            self.node(Node {
                value,
                specification,
                path: &format!("{}[{index}]", node.path),
            })?;
        }
        Ok(())
    }

    fn pointer<'a>(&'a self, pointer: &str, path: &str) -> Result<&'a Value, SchemaMismatch> {
        self.schema
            .pointer(pointer)
            .ok_or_else(|| error(path, "schema path must exist"))
    }
}
