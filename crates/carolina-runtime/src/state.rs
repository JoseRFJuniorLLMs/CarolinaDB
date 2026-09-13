//! Mapping between the interpreter's logical state and storage rows.
//!
//! One row = one storage version under `Namespace::User || RecordId || primary key` (SPEC-002 §11).
//! The value is the canonical `row.v1` object carrying the primary key and the typed fields, so a
//! row can be decoded without schema-driven key parsing. A CompiledBatch carries the final per-key
//! effect of one invocation: the difference between the pre-state and the accepted post-state.

use std::collections::BTreeMap;

use carolina_core::canon::{parse_canonical_int, CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::ids::{FieldId, RecordId};
use carolina_core::keycodec::{prefix_upper_bound, KeyWriter, Namespace};
use carolina_core::limits::Limits;
use carolina_lang::interp::State;
use carolina_lang::types::Value;
use carolina_storage::batch::{LogicalKey, SemanticMeta, StorageMutation};
use carolina_storage::kernel::{DurableStorageKernel, LocalSnapshot};

/// Storage key prefix of every row of `record`.
pub fn record_prefix(record: RecordId) -> Vec<u8> {
    KeyWriter::new()
        .namespace(Namespace::User)
        .u64(record.0)
        .finish()
}

/// Storage key of the row `record[key]`.
pub fn row_key(record: RecordId, key: &Value) -> CoreResult<LogicalKey> {
    let w = KeyWriter::new().namespace(Namespace::User).u64(record.0);
    Ok(LogicalKey(key.write_key(w)?.finish()))
}

/// Canonical `row.v1` value bytes.
pub fn encode_row(key: &Value, row: &BTreeMap<FieldId, Value>) -> Vec<u8> {
    let mut fields = BTreeMap::new();
    for (fid, v) in row {
        fields.insert(fid.0.to_string(), v.to_canon());
    }
    CanonValue::obj()
        .f("fields", CanonValue::Object(fields))
        .fc("key", key)
        .fstr("kind", "row.v1")
        .build()
        .encode()
}

pub fn decode_row(bytes: &[u8]) -> CoreResult<(Value, BTreeMap<FieldId, Value>)> {
    let v = CanonValue::decode(bytes, &Limits::v1())?;
    v.expect_fields(&["fields", "key", "kind"])?;
    if v.field("kind")?.as_str()? != "row.v1" {
        return Err(CoreError::new(ErrorCode::UnsupportedFormat, "row kind"));
    }
    let key = Value::from_canon(v.field("key")?)?;
    let mut row = BTreeMap::new();
    for (fid, fv) in v.field("fields")?.as_object()? {
        row.insert(
            FieldId(parse_canonical_int(fid)? as u64),
            Value::from_canon(fv)?,
        );
    }
    Ok((key, row))
}

/// Load every row of the given records visible at `snap` into an interpreter state.
pub fn load_records(
    kernel: &mut dyn DurableStorageKernel,
    snap: LocalSnapshot,
    records: &[RecordId],
) -> CoreResult<State> {
    let mut st = State::default();
    for rid in records {
        let prefix = record_prefix(*rid);
        let end = prefix_upper_bound(&prefix);
        let rows = kernel.scan(&prefix, end.as_deref(), snap, usize::MAX)?;
        for (_k, bytes) in rows {
            let (key, row) = decode_row(&bytes)?;
            st.insert(*rid, key, row);
        }
        // make the table exist even when empty so the interpreter sees a known record
        st.rows.entry(*rid).or_default();
    }
    Ok(st)
}

/// Final per-key effect of an accepted invocation over `records`, in key order.
pub fn diff_states(
    records: &[RecordId],
    pre: &State,
    post: &State,
) -> CoreResult<Vec<StorageMutation>> {
    let mut out: BTreeMap<LogicalKey, StorageMutation> = BTreeMap::new();
    let empty = BTreeMap::new();
    for rid in records {
        let before = pre.rows.get(rid).unwrap_or(&empty);
        let after = post.rows.get(rid).unwrap_or(&empty);
        for (key, row) in after {
            if before.get(key) != Some(row) {
                let lk = row_key(*rid, key)?;
                out.insert(
                    lk.clone(),
                    StorageMutation::Put {
                        key: lk,
                        value: encode_row(key, row),
                        semantic_meta: SemanticMeta::default(),
                    },
                );
            }
        }
        for key in before.keys() {
            if !after.contains_key(key) {
                let lk = row_key(*rid, key)?;
                out.insert(
                    lk.clone(),
                    StorageMutation::Delete {
                        key: lk,
                        semantic_meta: SemanticMeta::default(),
                    },
                );
            }
        }
    }
    Ok(out.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_roundtrip() {
        let mut row = BTreeMap::new();
        row.insert(FieldId(1), Value::Uuid([7u8; 16]));
        row.insert(FieldId(2), Value::I64(-5));
        let key = Value::Uuid([7u8; 16]);
        let bytes = encode_row(&key, &row);
        let (k2, r2) = decode_row(&bytes).unwrap();
        assert_eq!(k2, key);
        assert_eq!(r2, row);
        let lk = row_key(RecordId(3), &key).unwrap();
        assert!(lk.0.starts_with(&record_prefix(RecordId(3))));
    }
}
