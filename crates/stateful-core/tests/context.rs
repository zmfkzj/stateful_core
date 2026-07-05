use stateful_core::{
    AGENT_CONTEXT_SCOPE_SOURCE_REF, ContextPackage, CurrentEvidenceKind, CurrentFreshness,
    CurrentItem, CurrentItemKind, CurrentSeverity, ReconciliationDecision, RenderMode,
    render_prompt_text,
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
        .with_stale_activity("src/old_auth.ts", "Expired auth refactor reservation");

    let text = render_prompt_text(&package, RenderMode::Detailed);

    assert!(text.contains("Warnings"));
    assert!(text.contains("Nearby Activity"));
    assert!(text.contains("Stale/Expired"));
    assert!(text.contains("src/session.ts"));
    assert!(text.contains("src/auth/mod.ts"));
    assert!(text.contains("src/old_auth.ts"));
}

#[test]
fn brief_context_starts_with_summary_for_mixed_live_items() {
    let package = ContextPackage::from_items(vec![
        CurrentItem::new(
            CurrentItemKind::Claim,
            CurrentSeverity::Block,
            CurrentFreshness::Live,
            "src/auth.ts",
            "Fix auth validation behavior requested by the user.",
            "Session s1 has an active write claim.",
        )
        .with_next_action("Wait for s1 to release the claim."),
        CurrentItem::new(
            CurrentItemKind::Reservation,
            CurrentSeverity::Info,
            CurrentFreshness::Live,
            "src/session.ts",
            "Warm cache cleanup.",
            "Session s2 declared a reservation.",
        ),
    ]);

    let text = render_prompt_text(&package, RenderMode::Brief);

    assert!(text.starts_with("Stateful summary:"));
    assert!(text.contains("1 blocking"));
    assert!(text.contains("1 info"));
    assert!(text.contains("Blocking"));
    assert!(text.contains("Required Next Action"));
}

#[test]
fn brief_context_info_reservation_omits_purpose_after_summary() {
    let package = ContextPackage::from_items(vec![CurrentItem::new(
        CurrentItemKind::Reservation,
        CurrentSeverity::Info,
        CurrentFreshness::Live,
        "src/cache.ts",
        "Warm cache cleanup.",
        "Session s2 declared a reservation.",
    )]);

    let text = render_prompt_text(&package, RenderMode::Brief);

    assert!(text.starts_with("Stateful summary:"));
    assert!(text.contains("Nearby Activity"));
    assert!(!text.contains("purpose: Warm cache cleanup"));
}

#[test]
fn detailed_context_info_reservation_keeps_purpose() {
    let package = ContextPackage::from_items(vec![CurrentItem::new(
        CurrentItemKind::Reservation,
        CurrentSeverity::Info,
        CurrentFreshness::Live,
        "src/cache.ts",
        "Warm cache cleanup.",
        "Session s2 declared a reservation.",
    )]);

    let text = render_prompt_text(&package, RenderMode::Detailed);

    assert!(text.contains("Nearby Activity"));
    assert!(text.contains("purpose: Warm cache cleanup"));
}

#[test]
fn structured_items_render_purpose_and_required_actions() {
    let package = ContextPackage::from_items(vec![
        CurrentItem::new(
            CurrentItemKind::Claim,
            CurrentSeverity::Block,
            CurrentFreshness::Live,
            "src/auth.ts",
            "Fix auth validation behavior requested by the user.",
            "Session s1 has an active write claim.",
        )
        .with_next_action("Wait for s1 to release the claim."),
        CurrentItem::new(
            CurrentItemKind::ClaimableReservation,
            CurrentSeverity::Info,
            CurrentFreshness::Live,
            "src/session.ts",
            "Resume queued session cleanup after rereading.",
            "Session s2 has a claimable reservation.",
        )
        .with_next_action("Reread src/session.ts before continuing."),
    ]);

    let text = render_prompt_text(&package, RenderMode::Brief);

    assert!(text.contains("Blocking"));
    assert!(text.contains("Required Next Action"));
    assert!(text.contains("purpose: Fix auth validation behavior requested by the user"));
    assert!(text.contains("next: Wait for s1 to release the claim"));
    assert!(text.contains("Nearby Activity"));
    assert!(!text.contains("purpose: Resume queued session cleanup after rereading"));
    assert!(!text.contains("next: Reread src/session.ts before continuing"));
}

#[test]
fn active_scope_info_item_shows_next_action() {
    let package = ContextPackage::from_items(vec![CurrentItem::new(
        CurrentItemKind::Reservation,
        CurrentSeverity::Info,
        CurrentFreshness::Live,
        "src/auth.ts",
        "Fix auth validation.",
        "This session declared reservation for src/auth.ts.",
    )
    .with_next_action("Keep an exact same-reservation file claim active.")
    .with_agent("s1")
    .with_source_ref(AGENT_CONTEXT_SCOPE_SOURCE_REF)]);

    let text = render_prompt_text(&package, RenderMode::Brief);

    assert!(text.contains("Your Active Scope"));
    assert!(text.contains("next: Keep an exact same-reservation file claim active"));
}

#[test]
fn block_item_shows_evidence_in_brief() {
    let package = ContextPackage::from_items(vec![CurrentItem::new(
        CurrentItemKind::WaitQueue,
        CurrentSeverity::Block,
        CurrentFreshness::Live,
        "src/auth.ts",
        "Queue requested write after blocker clears.",
        "Agent s2 is queued for write_file on src/auth.ts.",
    )
    .with_evidence("Blocked by agent s1; wait_id wait-1.")]);

    let text = render_prompt_text(&package, RenderMode::Brief);

    assert!(text.contains("evidence: Blocked by agent s1; wait_id wait-1"));
}

#[test]
fn required_next_action_deduplicates_repeated_blocking_actions() {
    let repeated = "Wait for the claim to release, or coordinate with session-a.";
    let package = ContextPackage::from_items(vec![
        CurrentItem::new(
            CurrentItemKind::Claim,
            CurrentSeverity::Block,
            CurrentFreshness::Live,
            "src/auth.ts",
            "Fix auth validation behavior.",
            "session-a has an active write claim on src/auth.ts.",
        )
        .with_next_action(repeated),
        CurrentItem::new(
            CurrentItemKind::Claim,
            CurrentSeverity::Block,
            CurrentFreshness::Live,
            "src/session.ts",
            "Fix auth validation behavior.",
            "session-a has an active write claim on src/session.ts.",
        )
        .with_next_action(repeated),
        CurrentItem::new(
            CurrentItemKind::ClaimableReservation,
            CurrentSeverity::Block,
            CurrentFreshness::Live,
            "src/cache.ts",
            "Resume queued cache update.",
            "A reservation is ready for src/cache.ts.",
        )
        .with_next_action("Reread src/cache.ts before continuing."),
    ]);

    let text = render_prompt_text(&package, RenderMode::Brief);
    let required_section = text
        .split("Required Next Action\n")
        .nth(1)
        .expect("required next action section should render");

    assert_eq!(
        required_section
            .matches("- Wait for the claim to release, or coordinate with session-a")
            .count(),
        1
    );
    assert!(required_section.contains("- Reread src/cache.ts before continuing"));
}

#[test]
fn brief_context_renders_evidence_kind_without_evidence_text() {
    let package = ContextPackage::from_items(vec![
        CurrentItem::new(
            CurrentItemKind::Reservation,
            CurrentSeverity::Info,
            CurrentFreshness::Live,
            "src/auth.ts",
            "Fix auth validation behavior.",
            "Session s1 declared reservation for src/auth.ts.",
        )
        .with_evidence("ReservationDeclared event from session s1.")
        .with_evidence_kind(CurrentEvidenceKind::DeclaredReservation),
    ]);

    let text = render_prompt_text(&package, RenderMode::Brief);

    assert!(text.starts_with("Stateful summary:"));
    assert!(text.contains("evidence kind: declared_reservation"));
    assert!(!text.contains("purpose: Fix auth validation behavior"));
    assert!(!text.contains("evidence: ReservationDeclared event"));
}

#[test]
fn detailed_context_renders_evidence_text() {
    let package = ContextPackage::from_items(vec![
        CurrentItem::new(
            CurrentItemKind::Reservation,
            CurrentSeverity::Info,
            CurrentFreshness::Live,
            "src/auth.ts",
            "Fix auth validation behavior.",
            "Session s1 declared reservation for src/auth.ts.",
        )
        .with_evidence("ReservationDeclared event from session s1.")
        .with_evidence_kind(CurrentEvidenceKind::DeclaredReservation),
    ]);

    let text = render_prompt_text(&package, RenderMode::Detailed);

    assert!(text.contains("evidence kind: declared_reservation"));
    assert!(text.contains("evidence: ReservationDeclared event from session s1"));
}

#[test]
fn only_adopt_and_reapply_clear_human_write_blocks() {
    assert!(ReconciliationDecision::Adopt.clears_human_write_block());
    assert!(ReconciliationDecision::Reapply.clears_human_write_block());
    assert!(!ReconciliationDecision::AskUser.clears_human_write_block());
    assert!(!ReconciliationDecision::Abandon.clears_human_write_block());
}
