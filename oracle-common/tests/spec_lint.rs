//! Schema-lint test: walks every `spec/<profile>/<v>/schema/*.schema.json` next
//! to a sibling `spec/<profile>/<v>/examples/*.json` and asserts each example
//! validates against the matching schema.
//!
//! Naming convention: example files start with `sla.` or `delivery.` so we know
//! which schema to validate them against. This lets us cover Tasks 13.8 / 14.8 /
//! 15.10 in one place, runs as part of the workspace `cargo test`, and avoids
//! a separate CI job.

use std::{fs, path::PathBuf};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn every_example_validates_against_its_schema() {
    let root = workspace_root();

    // (crate, profile-dir-name)
    let crates: &[(&str, &str)] = &[
        ("oracle-api-quality", "api-quality-v1"),
        ("oracle-onchain-transfer", "onchain-transfer-v1"),
        ("oracle-file-delivery", "file-delivery-attestation-v1"),
    ];

    let mut total_validated = 0usize;
    let mut crates_seen = 0usize;

    for (krate, profile) in crates {
        let spec_dir = root.join(krate).join("spec").join(profile);
        if !spec_dir.exists() {
            // Crates that haven't shipped a normative spec yet are skipped — this
            // keeps the test green while specs are being authored.
            continue;
        }
        crates_seen += 1;
        let schema_dir = spec_dir.join("schema");
        let examples_dir = spec_dir.join("examples");
        if !examples_dir.exists() {
            continue;
        }

        let sla_schema = schema_dir.join("sla-document.schema.json");
        let delivery_schema = schema_dir.join("delivery-evidence.schema.json");

        let sla_validator = sla_schema
            .exists()
            .then(|| compile_schema(&sla_schema).expect("sla schema compiles"));
        let delivery_validator = delivery_schema
            .exists()
            .then(|| compile_schema(&delivery_schema).expect("delivery schema compiles"));

        for entry in fs::read_dir(&examples_dir).expect("read examples dir") {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let raw = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read example {}: {e}", path.display()));
            let value: serde_json::Value = serde_json::from_str(&raw)
                .unwrap_or_else(|e| panic!("parse example {}: {e}", path.display()));

            let validator = if name.starts_with("sla.") {
                sla_validator.as_ref()
            } else if name.starts_with("delivery.") {
                delivery_validator.as_ref()
            } else {
                None
            };
            let Some(v) = validator else {
                // Examples that don't follow the sla.* / delivery.* naming convention
                // get a soft pass (e.g. fixtures consumed by a custom test harness).
                continue;
            };
            let errs: Vec<String> = v
                .iter_errors(&value)
                .map(|e| format!("{e} at {}", e.instance_path))
                .collect();
            assert!(
                errs.is_empty(),
                "example {} failed schema validation: {}",
                path.display(),
                errs.join("; ")
            );
            total_validated += 1;
        }
    }

    assert!(crates_seen > 0, "expected at least one crate spec");
    assert!(
        total_validated > 0,
        "expected at least one example to validate"
    );
    eprintln!("schema-lint: validated {total_validated} examples across {crates_seen} crate(s)");
}

fn compile_schema(path: &std::path::Path) -> Result<jsonschema::Validator, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", path.display()))?;
    jsonschema::validator_for(&value).map_err(|e| format!("compile {}: {e}", path.display()))
}
