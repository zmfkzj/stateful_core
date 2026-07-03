use std::collections::BTreeMap;

use stateful_bench::{
    CollisionMetrics, FunctionalOutcome, HarnessTaskOutcome, PairClass, PairEligibility,
    PairManifestEntry, SweBenchInstance, classify_pair, composite_score, extract_touched_files,
    generate_fallback_preflight_from_instances, prepare_pairs_from_instances, stratified_sample,
};

#[test]
fn unified_diff_touched_file_extraction_handles_add_modify_delete_and_rename() {
    let files = extract_touched_files(
        r#"diff --git a/src/lib.py b/src/lib.py
--- a/src/lib.py
+++ b/src/lib.py
@@ -1 +1 @@
-old
+new
diff --git a/new.py b/new.py
--- /dev/null
+++ b/new.py
@@ -0,0 +1 @@
+new
diff --git a/deleted.py b/deleted.py
--- a/deleted.py
+++ /dev/null
@@ -1 +0,0 @@
-old
diff --git a/old_name.py b/new_name.py
similarity index 92%
rename from old_name.py
rename to new_name.py
"#,
    );

    assert_eq!(
        files.into_iter().collect::<Vec<_>>(),
        vec![
            "deleted.py".to_string(),
            "new.py".to_string(),
            "new_name.py".to_string(),
            "old_name.py".to_string(),
            "src/lib.py".to_string(),
        ]
    );
}

#[test]
fn pair_classification_distinguishes_overlap_directory_and_disjoint() {
    let exact = classify_pair(
        &extract_touched_files("diff --git a/pkg/a.py b/pkg/a.py\n"),
        &extract_touched_files("diff --git a/pkg/a.py b/pkg/a.py\n"),
    );
    let same_dir = classify_pair(
        &extract_touched_files("diff --git a/pkg/a.py b/pkg/a.py\n"),
        &extract_touched_files("diff --git a/pkg/b.py b/pkg/b.py\n"),
    );
    let disjoint = classify_pair(
        &extract_touched_files("diff --git a/pkg/a.py b/pkg/a.py\n"),
        &extract_touched_files("diff --git a/other/b.py b/other/b.py\n"),
    );

    assert_eq!(exact, PairClass::ExactFileOverlap);
    assert_eq!(same_dir, PairClass::SameDirectory);
    assert_eq!(disjoint, PairClass::SameRepoDisjoint);
}

#[test]
fn prepare_pairs_generates_all_same_repo_same_base_pairs_with_class_counts() {
    let instances = vec![
        instance(
            "repo__project-1",
            "repo/project",
            "base1",
            "1.0",
            "pkg/a.py",
        ),
        instance(
            "repo__project-2",
            "repo/project",
            "base1",
            "1.0",
            "pkg/a.py",
        ),
        instance(
            "repo__project-3",
            "repo/project",
            "base1",
            "1.0",
            "pkg/b.py",
        ),
        instance(
            "repo__project-4",
            "repo/project",
            "base2",
            "1.0",
            "pkg/c.py",
        ),
    ];

    let prepared = prepare_pairs_from_instances(&instances, None);

    assert_eq!(prepared.pairs.len(), 3);
    assert_eq!(
        prepared.class_counts,
        BTreeMap::from([
            (PairClass::ExactFileOverlap, 1),
            (PairClass::SameDirectory, 2),
        ])
    );
    assert!(prepared.pairs.iter().all(|pair| {
        pair.repo == "repo/project" && pair.eligibility == PairEligibility::SameBaseCommit
    }));
}

#[test]
fn fallback_preflight_generation_emits_same_repo_version_cross_base_candidates() {
    let mut first = instance(
        "repo__project-1",
        "repo/project",
        "base1",
        "1.0",
        "pkg/a.py",
    );
    first.test_patch = "diff --git a/tests/test_a.py b/tests/test_a.py\n".to_string();
    let mut second = instance(
        "repo__project-2",
        "repo/project",
        "base2",
        "1.0",
        "pkg/b.py",
    );
    second.test_patch = "diff --git a/tests/test_b.py b/tests/test_b.py\n".to_string();
    let mut same_base = instance(
        "repo__project-3",
        "repo/project",
        "base1",
        "1.0",
        "pkg/c.py",
    );
    same_base.test_patch = "diff --git a/tests/test_c.py b/tests/test_c.py\n".to_string();
    let mut other_version = instance(
        "repo__project-4",
        "repo/project",
        "base3",
        "2.0",
        "pkg/d.py",
    );
    other_version.test_patch = "diff --git a/tests/test_d.py b/tests/test_d.py\n".to_string();

    let preflight = generate_fallback_preflight_from_instances(
        &[first, second, same_base, other_version],
        true,
    );

    assert_eq!(preflight.len(), 2);
    assert!(
        preflight
            .iter()
            .all(|record| { record.test_patch_clean_apply && record.baseline_metadata_available })
    );
    assert!(preflight.iter().any(|record| {
        record.task_a == "repo__project-1" && record.task_b == "repo__project-2"
    }));
    assert!(preflight.iter().any(|record| {
        record.task_a == "repo__project-2" && record.task_b == "repo__project-3"
    }));
}

#[test]
fn stratified_sample_is_reproducible_and_balances_present_classes() {
    let pairs = vec![
        pair("exact-1", PairClass::ExactFileOverlap),
        pair("exact-2", PairClass::ExactFileOverlap),
        pair("dir-1", PairClass::SameDirectory),
        pair("dir-2", PairClass::SameDirectory),
        pair("repo-1", PairClass::SameRepoDisjoint),
        pair("repo-2", PairClass::SameRepoDisjoint),
    ];

    let first = stratified_sample(&pairs, 3, 42);
    let second = stratified_sample(&pairs, 3, 42);

    assert_eq!(first, second);
    assert_eq!(first.len(), 3);
    assert!(
        first
            .iter()
            .any(|pair| pair.class == PairClass::ExactFileOverlap)
    );
    assert!(
        first
            .iter()
            .any(|pair| pair.class == PairClass::SameDirectory)
    );
    assert!(
        first
            .iter()
            .any(|pair| pair.class == PairClass::SameRepoDisjoint)
    );
}

#[test]
fn composite_score_combines_functional_collision_safety_and_cost() {
    let score = composite_score(
        &FunctionalOutcome {
            task_a: HarnessTaskOutcome::Passed,
            task_b: HarnessTaskOutcome::Failed,
        },
        &CollisionMetrics {
            uncoordinated_same_file_collisions: 1,
            coordinated_blocks: 2,
            lost_edit_events: 0,
            denied_writes: 1,
            scope_mismatches: 1,
            stale_intents: 0,
            timeouts: 0,
            long_idle_periods: 0,
            authorization_warnings: 0,
            warned_writes_applied: 0,
            wait_events: 0,
        },
    )
    .expect("non-setup outcome should be scored");

    assert_eq!(score.functional_pair_score, 0.5);
    assert_eq!(score.collision_safety_score, 0.7);
    assert_eq!(score.coordination_cost_score, 0.8);
    assert_eq!(score.composite_coordination_score, 0.62);
}

fn instance(
    instance_id: &str,
    repo: &str,
    base_commit: &str,
    version: &str,
    source_file: &str,
) -> SweBenchInstance {
    SweBenchInstance {
        instance_id: instance_id.to_string(),
        repo: repo.to_string(),
        base_commit: base_commit.to_string(),
        problem_statement: "Fix it".to_string(),
        version: Some(version.to_string()),
        patch: format!("diff --git a/{source_file} b/{source_file}\n"),
        test_patch: String::new(),
        fail_to_pass: vec!["tests/test_fix.py::test_fix".to_string()],
        pass_to_pass: vec!["tests/test_existing.py::test_existing".to_string()],
        difficulty: None,
    }
}

fn pair(pair_id: &str, class: PairClass) -> PairManifestEntry {
    let task_a = instance(
        &format!("{pair_id}-a"),
        "repo/project",
        "base",
        "1.0",
        "a.py",
    );
    let task_b = instance(
        &format!("{pair_id}-b"),
        "repo/project",
        "base",
        "1.0",
        "b.py",
    );

    PairManifestEntry {
        pair_id: pair_id.to_string(),
        repo: "repo/project".to_string(),
        base_commit: Some("base".to_string()),
        version: Some("1.0".to_string()),
        eligibility: PairEligibility::SameBaseCommit,
        class,
        task_a_files: vec!["a.py".to_string()],
        task_b_files: vec!["b.py".to_string()],
        task_a,
        task_b,
    }
}
