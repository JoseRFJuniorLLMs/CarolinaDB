//! S2/S3 differential tests: the native tree against `BTreeMap<key, Vec<version>>` (SPEC-002 §144–§145).
//! The campaign bodies live in `carolina_storage::campaign` and are shared with `carolina-qualify`.

use carolina_storage::campaign::{btree_differential, btree_hot_key};

#[test]
fn random_differential_small_pool() {
    btree_differential(1, 6000, 16, 400).unwrap();
}

#[test]
fn random_differential_many_versions_per_key() {
    btree_differential(2, 5000, 64, 40).unwrap();
}

#[test]
fn random_differential_wide_keyspace() {
    btree_differential(3, 8000, 128, 5000).unwrap();
}

#[test]
fn many_versions_of_one_key_span_leaves() {
    btree_hot_key(3000).unwrap();
}
