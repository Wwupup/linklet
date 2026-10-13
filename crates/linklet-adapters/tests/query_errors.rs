//! Filesystem failures retain OS categories, not guesses from translated messages.
use linklet_core::{transfer::Destination, wire};
#[test]
fn absent_file_is_a_typed_failure_not_an_empty_query() {
    let base = std::env::temp_dir();
    let paths = Destination::new(base.to_str().expect("temp path")).expect("paths");
    let name = format!("linklet-absent-{}-never-created", std::process::id());
    let listing = linklet_adapters::list_directory(&paths, &name);
    let search = linklet_adapters::grep(
        &paths,
        &name,
        &linklet_core::search::Pattern::new("x"),
        linklet_core::search::Limit::default(),
        linklet_core::search::Direction::First,
    );
    assert_eq!(
        wire::ls_listing_to_json(&listing)["problem"]["code"],
        "not_found"
    );
    assert_eq!(
        wire::search_to_json(&search)["problem"]["code"],
        "not_found"
    );
}
