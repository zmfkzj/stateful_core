use serde::{Deserialize, Serialize};

pub const CURRENT_SESSION_SCOPE_SOURCE_REF: &str = "CurrentSessionScope";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    Brief,
    Detailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurrentItemKind {
    Session,
    Intent,
    Lease,
    WaitQueue,
    Reservation,
    Finalization,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CurrentSeverity {
    Block,
    Warn,
    Info,
}

impl CurrentSeverity {
    fn as_str(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Warn => "warn",
            Self::Info => "info",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CurrentFreshness {
    Live,
    Stale,
    Expired,
    Finalized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurrentEvidenceKind {
    DeclaredIntent,
    LeaseOnly,
    WaitQueue,
    Reservation,
    ObservedWrite,
    VerifiedDiff,
}

impl CurrentEvidenceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::DeclaredIntent => "declared_intent",
            Self::LeaseOnly => "lease_only",
            Self::WaitQueue => "wait_queue",
            Self::Reservation => "reservation",
            Self::ObservedWrite => "observed_write",
            Self::VerifiedDiff => "verified_diff",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentItem {
    pub kind: CurrentItemKind,
    pub severity: CurrentSeverity,
    pub freshness: CurrentFreshness,
    pub resource: String,
    pub purpose: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_kind: Option<CurrentEvidenceKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_refs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_seconds: Option<i64>,
}

impl CurrentItem {
    pub fn new(
        kind: CurrentItemKind,
        severity: CurrentSeverity,
        freshness: CurrentFreshness,
        resource: impl Into<String>,
        purpose: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            severity,
            freshness,
            resource: resource.into(),
            purpose: purpose.into(),
            summary: summary.into(),
            next_action: None,
            evidence: None,
            evidence_kind: None,
            session_id: None,
            workspace_id: None,
            source_refs: Vec::new(),
            observed_at: None,
            expires_at: None,
            age_seconds: None,
        }
    }

    pub fn with_next_action(mut self, next_action: impl Into<String>) -> Self {
        self.next_action = Some(next_action.into());
        self
    }

    pub fn with_evidence(mut self, evidence: impl Into<String>) -> Self {
        self.evidence = Some(evidence.into());
        self
    }

    pub fn with_evidence_kind(mut self, evidence_kind: CurrentEvidenceKind) -> Self {
        self.evidence_kind = Some(evidence_kind);
        self
    }

    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_workspace(mut self, workspace_id: impl Into<String>) -> Self {
        self.workspace_id = Some(workspace_id.into());
        self
    }

    pub fn with_source_ref(mut self, source_ref: impl Into<String>) -> Self {
        self.source_refs.push(source_ref.into());
        self
    }

    pub fn with_observed_at(mut self, observed_at: impl Into<String>) -> Self {
        self.observed_at = Some(observed_at.into());
        self
    }

    pub fn with_expires_at(mut self, expires_at: Option<String>) -> Self {
        self.expires_at = expires_at;
        self
    }

    pub fn with_age_seconds(mut self, age_seconds: Option<i64>) -> Self {
        self.age_seconds = age_seconds;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextPackage {
    status: ContextStatus,
    items: Vec<CurrentItem>,
}

impl ContextPackage {
    pub fn empty() -> Self {
        Self {
            status: ContextStatus::Ok,
            items: Vec::new(),
        }
    }

    pub fn from_items(items: Vec<CurrentItem>) -> Self {
        let status = if items.iter().any(|item| {
            item.freshness == CurrentFreshness::Live && item.severity == CurrentSeverity::Block
        }) {
            ContextStatus::Blocked
        } else {
            ContextStatus::Ok
        };
        Self { status, items }
    }

    pub fn items(&self) -> &[CurrentItem] {
        &self.items
    }

    pub fn blocked_human_write(path: impl Into<String>) -> Self {
        let path = path.into();
        Self::from_items(vec![
            CurrentItem::new(
                CurrentItemKind::Lease,
                CurrentSeverity::Block,
                CurrentFreshness::Live,
                path.clone(),
                "Reconcile a human write before resuming agent edits.",
                format!("{path} has an unreconciled human write."),
            )
            .with_next_action(format!(
                "Reread {path}, summarize the human change, choose adopt/reapply/ask_user/abandon, then call state.reconcile.ack."
            ))
            .with_evidence("HumanWriteObserved affects active agent work.")
            .with_evidence_kind(CurrentEvidenceKind::ObservedWrite),
        ])
    }

    pub fn with_warning(mut self, resource: impl Into<String>, summary: impl Into<String>) -> Self {
        self.items.push(CurrentItem::new(
            CurrentItemKind::Intent,
            CurrentSeverity::Warn,
            CurrentFreshness::Live,
            resource,
            "Coordinate with related planned work.",
            summary,
        ));
        self.status = package_status(&self.items);
        self
    }

    pub fn with_nearby_activity(
        mut self,
        resource: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        self.items.push(CurrentItem::new(
            CurrentItemKind::Session,
            CurrentSeverity::Info,
            CurrentFreshness::Live,
            resource,
            "Understand nearby active work before editing.",
            summary,
        ));
        self.status = package_status(&self.items);
        self
    }

    pub fn with_stale_activity(
        mut self,
        resource: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        self.items.push(CurrentItem::new(
            CurrentItemKind::Finalization,
            CurrentSeverity::Info,
            CurrentFreshness::Expired,
            resource,
            "Preserve handoff context from previous work.",
            summary,
        ));
        self.status = package_status(&self.items);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ContextStatus {
    Ok,
    Blocked,
}

pub fn render_prompt_text(package: &ContextPackage, mode: RenderMode) -> String {
    let mut output = String::new();
    let max_total = match mode {
        RenderMode::Brief => 8,
        RenderMode::Detailed => 20,
    };
    let mut rendered = 0usize;

    let active_scope = package
        .items
        .iter()
        .filter(|item| {
            item.freshness == CurrentFreshness::Live && is_current_session_scope_item(item)
        })
        .collect::<Vec<_>>();
    render_section(
        &mut output,
        "Your Active Scope",
        &active_scope,
        mode,
        max_total,
        &mut rendered,
    );

    let blocking = package
        .items
        .iter()
        .filter(|item| {
            item.freshness == CurrentFreshness::Live
                && item.severity == CurrentSeverity::Block
                && !is_current_session_scope_item(item)
        })
        .collect::<Vec<_>>();
    render_section(
        &mut output,
        "Blocking",
        &blocking,
        mode,
        max_total,
        &mut rendered,
    );

    if matches!(package.status, ContextStatus::Blocked) {
        render_required_next_action(&mut output, &blocking);
    }

    let warnings = package
        .items
        .iter()
        .filter(|item| {
            item.freshness == CurrentFreshness::Live
                && item.severity == CurrentSeverity::Warn
                && !is_current_session_scope_item(item)
        })
        .collect::<Vec<_>>();
    render_section(
        &mut output,
        "Warnings",
        &warnings,
        mode,
        max_total,
        &mut rendered,
    );

    let nearby = package
        .items
        .iter()
        .filter(|item| {
            item.freshness == CurrentFreshness::Live
                && item.severity == CurrentSeverity::Info
                && !is_current_session_scope_item(item)
        })
        .collect::<Vec<_>>();
    render_section(
        &mut output,
        "Nearby Activity",
        &nearby,
        mode,
        max_total,
        &mut rendered,
    );

    let stale_limit = match mode {
        RenderMode::Brief => 3,
        RenderMode::Detailed => 10,
    };
    let stale = package
        .items
        .iter()
        .filter(|item| {
            item.freshness != CurrentFreshness::Live && !is_current_session_scope_item(item)
        })
        .take(stale_limit)
        .collect::<Vec<_>>();
    render_section(
        &mut output,
        "Stale/Expired",
        &stale,
        mode,
        max_total,
        &mut rendered,
    );

    output
}

fn is_current_session_scope_item(item: &CurrentItem) -> bool {
    item.source_refs
        .iter()
        .any(|source_ref| source_ref == CURRENT_SESSION_SCOPE_SOURCE_REF)
}

fn package_status(items: &[CurrentItem]) -> ContextStatus {
    if items.iter().any(|item| {
        item.freshness == CurrentFreshness::Live && item.severity == CurrentSeverity::Block
    }) {
        ContextStatus::Blocked
    } else {
        ContextStatus::Ok
    }
}

fn render_required_next_action(output: &mut String, items: &[&CurrentItem]) {
    let mut next_actions = Vec::new();
    for item in items {
        let Some(next_action) = item.next_action.as_deref() else {
            continue;
        };
        let next_action = trim_trailing_period(next_action);
        if !next_actions.contains(&next_action) {
            next_actions.push(next_action);
        }
    }
    if next_actions.is_empty() {
        return;
    }

    if !output.is_empty() {
        output.push('\n');
    }
    output.push_str("Required Next Action\n");
    for next_action in next_actions {
        output.push_str(&format!("- {next_action}\n"));
    }
}

fn render_section(
    output: &mut String,
    title: &str,
    items: &[&CurrentItem],
    mode: RenderMode,
    max_total: usize,
    rendered: &mut usize,
) {
    if items.is_empty() || *rendered >= max_total {
        return;
    }

    if !output.is_empty() {
        output.push('\n');
    }
    output.push_str(title);
    output.push('\n');
    for item in items {
        if *rendered >= max_total {
            break;
        }
        *rendered += 1;
        output.push_str(&format!(
            "- [{}] {}: {}.\n",
            item.severity.as_str(),
            item.resource,
            trim_trailing_period(&item.summary)
        ));
        output.push_str(&format!(
            "  purpose: {}\n",
            trim_trailing_period(&item.purpose)
        ));
        if item.severity != CurrentSeverity::Info {
            if let Some(next_action) = &item.next_action {
                output.push_str(&format!("  next: {}\n", trim_trailing_period(next_action)));
            }
        }
        if let Some(evidence_kind) = item.evidence_kind {
            output.push_str(&format!("  evidence kind: {}\n", evidence_kind.as_str()));
        }
        if matches!(mode, RenderMode::Detailed) {
            if let Some(evidence) = &item.evidence {
                output.push_str(&format!("  evidence: {}\n", trim_trailing_period(evidence)));
            }
        }
    }
}

fn trim_trailing_period(value: &str) -> &str {
    value.strip_suffix('.').unwrap_or(value)
}
