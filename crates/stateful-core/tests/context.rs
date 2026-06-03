use stateful_core::{ContextPackage, ReconciliationDecision, RenderMode, render_prompt_text};

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
fn only_adopt_and_reapply_clear_human_write_blocks() {
    assert!(ReconciliationDecision::Adopt.clears_human_write_block());
    assert!(ReconciliationDecision::Reapply.clears_human_write_block());
    assert!(!ReconciliationDecision::AskUser.clears_human_write_block());
    assert!(!ReconciliationDecision::Abandon.clears_human_write_block());
}
