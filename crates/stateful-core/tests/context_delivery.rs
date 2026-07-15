use stateful_core::{
    ContextDelta, ContextPackage, CurrentFreshness, CurrentItem, CurrentItemKind, CurrentSeverity,
    RenderMode,
};

#[test]
fn changed_delta_carries_items_and_rendered_prompt() {
    let package = ContextPackage::from_items(vec![CurrentItem::new(
        CurrentItemKind::Reservation,
        CurrentSeverity::Warn,
        CurrentFreshness::Live,
        "src/lib.rs",
        "Coordinate before editing.",
        "Another agent reserved src/lib.rs.",
    )]);

    let delta = ContextDelta::changed(0, 1, "delivery-1", package, RenderMode::Brief);

    assert_eq!(delta.from_version, 0);
    assert_eq!(delta.workspace_version, 1);
    assert!(delta.changed);
    assert!(!delta.reset_required);
    assert_eq!(delta.delivery_id.as_deref(), Some("delivery-1"));
    assert_eq!(delta.items.len(), 1);
    assert!(delta.prompt_text.contains("src/lib.rs"));
}
