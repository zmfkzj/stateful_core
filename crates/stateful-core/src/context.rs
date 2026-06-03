#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    Brief,
    Detailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextPackage {
    status: ContextStatus,
    items: Vec<ContextItem>,
}

impl ContextPackage {
    pub fn empty() -> Self {
        Self {
            status: ContextStatus::Ok,
            items: Vec::new(),
        }
    }

    pub fn blocked_human_write(path: impl Into<String>) -> Self {
        let path = path.into();
        Self {
            status: ContextStatus::Blocked,
            items: vec![ContextItem {
                severity: ContextSeverity::Block,
                resource: path.clone(),
                summary: format!("{path} has an unreconciled human write."),
                next_action: Some(format!(
                    "Reread {path}, summarize the human change, choose adopt/reapply/ask_user/abandon, then call state.reconcile.ack."
                )),
                evidence: Some("HumanWriteObserved affects active agent work.".to_string()),
            }],
        }
    }

    pub fn with_warning(mut self, resource: impl Into<String>, summary: impl Into<String>) -> Self {
        self.items.push(ContextItem {
            severity: ContextSeverity::Warning,
            resource: resource.into(),
            summary: summary.into(),
            next_action: None,
            evidence: None,
        });
        self
    }

    pub fn with_nearby_activity(
        mut self,
        resource: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        self.items.push(ContextItem {
            severity: ContextSeverity::Nearby,
            resource: resource.into(),
            summary: summary.into(),
            next_action: None,
            evidence: None,
        });
        self
    }

    pub fn with_stale_activity(
        mut self,
        resource: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        self.items.push(ContextItem {
            severity: ContextSeverity::Stale,
            resource: resource.into(),
            summary: summary.into(),
            next_action: None,
            evidence: None,
        });
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextStatus {
    Ok,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ContextItem {
    severity: ContextSeverity,
    resource: String,
    summary: String,
    next_action: Option<String>,
    evidence: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextSeverity {
    Block,
    Warning,
    Nearby,
    Stale,
}

impl ContextSeverity {
    fn as_str(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Warning => "warning",
            Self::Nearby => "nearby",
            Self::Stale => "stale",
        }
    }
}

pub fn render_prompt_text(package: &ContextPackage, mode: RenderMode) -> String {
    let mut output = String::new();

    render_section(
        &mut output,
        "Blocking",
        ContextSeverity::Block,
        &package.items,
        mode,
    );
    render_section(
        &mut output,
        "Warnings",
        ContextSeverity::Warning,
        &package.items,
        mode,
    );
    render_section(
        &mut output,
        "Nearby Activity",
        ContextSeverity::Nearby,
        &package.items,
        mode,
    );
    render_section(
        &mut output,
        "Stale/Expired",
        ContextSeverity::Stale,
        &package.items,
        mode,
    );

    if matches!(package.status, ContextStatus::Blocked) {
        output.push_str("\nRequired Next Action\n");
        for item in &package.items {
            if let Some(next_action) = &item.next_action {
                output.push_str(&format!("- {}\n", trim_trailing_period(next_action)));
            }
        }
    }

    output
}

fn render_section(
    output: &mut String,
    title: &str,
    severity: ContextSeverity,
    items: &[ContextItem],
    mode: RenderMode,
) {
    let section_items = items
        .iter()
        .filter(|item| item.severity == severity)
        .collect::<Vec<_>>();
    if section_items.is_empty() {
        return;
    }

    if !output.is_empty() {
        output.push('\n');
    }
    output.push_str(title);
    output.push('\n');
    for item in section_items {
        output.push_str(&format!(
            "- [{}] {}: {}.\n",
            item.severity.as_str(),
            item.resource,
            trim_trailing_period(&item.summary)
        ));
        if let Some(next_action) = &item.next_action {
            output.push_str(&format!("  next: {}\n", trim_trailing_period(next_action)));
        }
        if matches!(mode, RenderMode::Detailed)
            && let Some(evidence) = &item.evidence
        {
            output.push_str(&format!("  evidence: {}\n", trim_trailing_period(evidence)));
        }
    }
}

fn trim_trailing_period(value: &str) -> &str {
    value.strip_suffix('.').unwrap_or(value)
}
