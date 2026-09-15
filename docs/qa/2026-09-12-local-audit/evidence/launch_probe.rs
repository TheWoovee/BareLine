use std::path::Path;
use std::os::windows::fs::OpenOptionsExt;
fn main() {
 let root = std::env::current_dir().unwrap().join("docs/qa/2026-09-12-local-audit/evidence/launch-probe-fixture");
 let source = root.join("roaming"); let destination=root.join("local");
 std::fs::create_dir_all(&source).unwrap();
 std::fs::write(source.join("settings.toml"), "sentinel=1").unwrap();
 let held=std::fs::OpenOptions::new().read(true).share_mode(0).open(source.join("settings.toml")).unwrap();
 migrate_from_roaming(Some(&source), &destination);
 println!("migration_first_local_exists={}; settings_in_local={}; settings_in_roaming={}",destination.exists(),destination.join("settings.toml").exists(),source.join("settings.toml").exists());
 drop(held);
 migrate_from_roaming(Some(&source), &destination);
 println!("migration_retry_settings_in_local={}; settings_in_roaming={}",destination.join("settings.toml").exists(),source.join("settings.toml").exists());
 let sentinel=root.join("outside-temp/cache-4294967294-1/keep.txt");
 println!("junction_sentinel_before={}",sentinel.exists());
 let removed=sweep_temp_caches(&root.join("temp"), &|_|false);
 println!("sweep_removed={removed}; junction_sentinel_after={}",sentinel.exists());
}
fn migrate_from_roaming(roaming: Option<&Path>, local: &Path) {
    let Some(roaming) = roaming else { return };
    if roaming == local || !roaming.is_dir() || local.exists() {
        return;
    }
    if std::fs::create_dir_all(local).is_err() {
        return;
    }
    for name in ["settings.toml", "session.json", "recovery", "macros", "extensions"] {
        let from = roaming.join(name);
        let to = local.join(name);
        if from.exists() && !to.exists() {
            let _ = std::fs::rename(&from, &to);
        }
    }
}

/// Private caches are named `<kind>-<pid>-<serial>` inside a `Bareline-*`
/// directory under the temporary folder; a crash leaves them behind.
fn cache_owner_pid(name: &str) -> Option<u32> {
    let mut parts = name.rsplitn(3, '-');
    parts.next()?;
    let pid = parts.next()?;
    parts.next()?;
    pid.parse().ok()
}

/// Delete temporary caches left by processes that are no longer running.
fn sweep_temp_caches(temp: &Path, alive: &dyn Fn(u32) -> bool) -> usize {
    let Ok(roots) = std::fs::read_dir(temp) else {
        return 0;
    };
    let mut removed = 0;
    for root in roots.flatten() {
        if !root.file_name().to_string_lossy().starts_with("Bareline-") || !root.path().is_dir() {
            continue;
        }
        let Ok(caches) = std::fs::read_dir(root.path()) else {
            continue;
        };
        for cache in caches.flatten() {
            let name = cache.file_name();
            let Some(pid) = cache_owner_pid(&name.to_string_lossy()) else {
                continue;
            };
            if alive(pid) {
                continue;
            }
            if std::fs::remove_dir_all(cache.path()).is_ok() {
                removed += 1;
            }
        }
        let _ = std::fs::remove_dir(root.path());
    }
    removed
}


