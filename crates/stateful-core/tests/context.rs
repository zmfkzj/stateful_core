use stateful_core::{
    ContextPackage, CurrentEvidenceKind, CurrentFreshness, CurrentItem, CurrentItemKind,
    CurrentSeverity, ReconciliationDecision, RenderMode, render_prompt_text,
};

#[test]
fn empty_context_renders_no_prompt_text() {
    let package = ContextPackage::empty();

    let text = render_prompt_text(&package, RenderMode::Brief);

    assert_eq!(text, "");
}

#[test]
fn blocked_context_renders_required_next_action() {
    let package = ContextPackage::blocked_human_write("src/auth.ts");

    let text = render_prompt_text(&package, RenderMode::Detailed);

    assert!(text.contains("Blocking"));
    assert!(text.contains("Required Next Action"));
    assert!(text.contains("src/auth.ts"));
    assert!(text.contains("purpose: Reconcile a human write"));
    assert!(text.contains("state.reconcile.ack"));
}

#[test]
fn detailed_context_renders_warning_nearby_and_stale_sections() {
    let package = ContextPackage::blocked_human_write("src/auth.ts")
        .with_warning("src/session.ts", "Another session plans a related edit")
        .with_nearby_activity("src/auth/mod.ts", "Agent s2 is inspecting nearby auth code")
        .with_stale_activity("src/old_auth.ts", "Expired auth refactor intent");

    let text = render_prompt_text(&package, RenderMode::Detailed);

    assert!(text.contains("Warnings"));
    assert!(text.contains("Nearby Activity"));
    assert!(text.contains("Stale/Expired"));
    assert!(text.contains("src/session.ts"));
    assert!(text.contains("src/auth/mod.ts"));
    assert!(text.contains("src/old_auth.ts"));
}

#[test]
fn structured_items_render_purpose_and_required_actions() {
    let package = ContextPackage::from_items(vec![
        CurrentItem::new(
            CurrentItemKind::Lease,
            CurrentSeverity::Block,
            CurrentFreshness::Live,
            "src/auth.ts",
            "Fix auth validation behavior requested by the user.",
            "Session s1 has an active write lease.",
        )
        .with_next_action("Wait for s1 to release the lease."),
        CurrentItem::new(
            CurrentItemKind::Reservation,
            CurrentSeverity::Info,
            CurrentFreshness::Live,
            "src/session.ts",
            "Resume queued session cleanup after rereading.",
            "Session s2 has a claimable reservation.",
        ),
    ]);

    let text = render_prompt_text(&package, RenderMode::Brief);

    assert!(text.contains("Blocking"));
    assert!(text.contains("Required Next Action"));
    assert!(text.contains("purpose: Fix auth validation behavior requested by the user"));
    assert!(text.contains("Nearby Activity"));
    assert!(text.contains("purpose: Resume queued session cleanup after rereading"));
}

#[test]
fn detailed_context_renders_evidence_kind() {
    let package = ContextPackage::from_items(vec![
        CurrentItem::new(
            CurrentItemKind::Intent,
            CurrentSeverity::Info,
            CurrentFreshness::Live,
            "src/auth.ts",
            "Fix auth validation behavior.",
            "Session s1 declared intent for src/auth.ts.",
        )
        .with_evidence_kind(CurrentEvidenceKind::DeclaredIntent),
    ]);

    let text = render_prompt_text(&package, RenderMode::Detailed);

    assert!(text.contains("evidence kind: declared_intent"));
}

#[test]
fn only_adopt_and_reapply_clear_human_write_blocks() {
    assert!(ReconciliationDecision::Adopt.clears_human_write_block());
    assert!(ReconciliationDecision::Reapply.clears_human_write_block());
    assert!(!ReconciliationDecision::AskUser.clears_human_write_block());
    assert!(!ReconciliationDecision::Abandon.clears_human_write_block());
}
