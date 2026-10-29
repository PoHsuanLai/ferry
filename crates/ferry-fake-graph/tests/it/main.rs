//! The crate's integration tests: one executable, one module per topic.

mod graph_issued;

#[test]
fn every_module_is_declared() {
    test_kit::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
