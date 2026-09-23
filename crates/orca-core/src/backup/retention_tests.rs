use super::*;

const DAY: u64 = 86_400;
const NOW: u64 = 1_790_000_000;

fn days_ago(n: u64) -> u64 {
    NOW - n * DAY
}

#[test]
fn a_streak_of_failed_nights_never_deletes_the_last_good_backups() {
    // #204: 20 nightly snapshots, the newest 15 days old (15 failed nights
    // produced nothing), retention 14 days. Age-only retention deleted all
    // of them; the floor keeps the newest 7.
    let items: Vec<(u32, u64)> = (0..20).map(|i| (i, days_ago(15 + u64::from(i)))).collect();
    let pruned = to_prune(&items, NOW, 14, 7);
    assert_eq!(pruned.len(), 13);
    for kept in 0..7 {
        assert!(!pruned.contains(&kept), "snapshot {kept} must be kept");
    }
}

#[test]
fn within_retention_nothing_is_deleted_and_the_floor_is_not_a_cap() {
    let items: Vec<(u32, u64)> = (0..30).map(|i| (i, days_ago(u64::from(i)))).collect();
    // 30 daily snapshots, retention 60: all young, all kept (keep_min is a
    // floor, not a maximum).
    assert!(to_prune(&items, NOW, 60, 7).is_empty());
    // Retention 14: days 15..29 go, except that the floor is already met.
    let pruned = to_prune(&items, NOW, 14, 7);
    assert_eq!(pruned, (15..30).collect::<Vec<u32>>());
}

#[test]
fn keep_min_zero_is_pure_age_retention() {
    let items = vec![("old", days_ago(40)), ("new", days_ago(1))];
    assert_eq!(to_prune(&items, NOW, 30, 0), vec!["old"]);
}

#[test]
fn s3_keys_are_grouped_and_dated_from_their_names() {
    assert_eq!(
        s3_group("master/2026-09-23/secrets_20260923T030000Z.json.age"),
        Some(("master/secrets".into(), 1_790_132_400))
    );
    assert_eq!(
        s3_group("agents/ubuntu-16gb-fsn1-1/2026-09-23/orca-gitea-db-data.tar.gz"),
        Some((
            "agents/ubuntu-16gb-fsn1-1/orca-gitea-db-data.tar.gz".into(),
            1_790_121_600
        ))
    );
    // Anything outside the layout orca writes is never touched.
    for foreign in [
        "manual-dump.sql",
        "master/2026-09-23/notes.txt",
        "agents/host/not-a-date/x.tar.gz",
        "other/2026-09-23/secrets_20260923T030000Z.json",
    ] {
        assert_eq!(s3_group(foreign), None, "{foreign}");
    }
}

#[test]
fn s3_plan_prunes_per_artifact_and_leaves_foreign_keys_alone() {
    let mut keys = vec!["manual-dump.sql".to_string()];
    // 10 nights of two artifacts, oldest first; the plan runs "now" = +40 days.
    for d in 0..10u32 {
        let date = format!("2026-08-{:02}", d + 1);
        let ts = format!("202608{:02}T030000Z", d + 1);
        keys.push(format!("master/{date}/secrets_{ts}.json.age"));
        keys.push(format!("agents/host/{date}/orca-db-data.tar.gz"));
    }
    let now = s3_group("master/2026-09-20/x_20260920T030000Z.json")
        .unwrap()
        .1;
    let plan = s3_prune_plan(&keys, now, 30, 3);
    // All 10 of each are > 30 days old; the newest 3 of each are kept.
    assert_eq!(plan.len(), 14);
    assert!(!plan.contains(&"manual-dump.sql".to_string()));
    assert!(
        !plan.iter().any(|k| k.contains("2026-08-10")),
        "newest kept"
    );
    assert!(
        plan.iter().any(|k| k.contains("2026-08-01")),
        "oldest pruned"
    );
}
