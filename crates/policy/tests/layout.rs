use git_forest_policy::repo_root;

#[test]
fn repo_root_holds_the_product_sources() {
    let lib = repo_root().join("src/lib.rs");
    assert!(
        lib.is_file(),
        "expected the product library at {}",
        lib.display()
    );
}
