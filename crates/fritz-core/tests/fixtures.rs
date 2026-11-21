//! The fixtures in the repository hold no personal values.

use std::path::Path;

use fritz_core::anonymize;

#[test]
fn fixtures_are_clean() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // Private patterns are only known locally; CI checks without them.
    let private =
        anonymize::read_private_patterns(&root.join(".private-patterns")).unwrap_or_default();
    let findings = anonymize::check(&root.join("crates/fritz-sim/fixtures"), &private).unwrap();
    let report: Vec<String> = findings.iter().map(ToString::to_string).collect();
    assert_eq!(report, Vec::<String>::new());
}
