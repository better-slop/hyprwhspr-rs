//! Exercise ConfigManager publication without touching process-wide XDG paths.

use super::*;

struct Fixture {
    root: PathBuf,
    manager: ConfigManager,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "hyprwhspr-config-reload-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let config_path = root.join("config.jsonc");
        let config = Config {
            max_copy_text_bytes: 1,
            ..Config::default()
        };
        ConfigManager::write_config_file(&config_path, &config).unwrap();
        // Match ConfigManager::load: no receiver remains during synchronous
        // model initialization, before app/control subscriptions are installed.
        let (change_tx, _) = watch::channel(config.clone());
        Self {
            root,
            manager: ConfigManager {
                inner: Arc::new(ConfigManagerInner {
                    config: RwLock::new(config),
                    config_path,
                    change_tx,
                    watcher_active: AtomicBool::new(false),
                }),
            },
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn save_keeps_latest_config_for_future_subscribers() {
    let fixture = Fixture::new();
    fixture
        .manager
        .inner
        .config
        .write()
        .unwrap()
        .max_copy_text_bytes = 128;
    assert_eq!(fixture.manager.inner.change_tx.receiver_count(), 0);
    fixture.manager.save().unwrap();
    assert_eq!(
        fixture.manager.subscribe().borrow().max_copy_text_bytes,
        128
    );
}

#[tokio::test(flavor = "current_thread")]
async fn file_reload_keeps_latest_config_before_any_subscriber_exists() {
    let fixture = Fixture::new();
    fixture.manager.start_watching();
    // Let the real watcher capture its initial file state before this edit.
    tokio::task::yield_now().await;
    let mut updated = fixture.manager.get();
    updated.max_copy_text_bytes = 128;
    ConfigManager::write_config_file(&fixture.manager.inner.config_path, &updated).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.manager.get().max_copy_text_bytes != 128 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("real watcher must apply edited config");
    assert_eq!(fixture.manager.inner.change_tx.receiver_count(), 0);
    assert_eq!(
        fixture.manager.subscribe().borrow().max_copy_text_bytes,
        128
    );
}

#[tokio::test(flavor = "current_thread")]
async fn file_edit_before_watch_task_first_poll_is_not_missed() {
    let fixture = Fixture::new();
    fixture.manager.start_watching();
    // No yield: the spawned task has not captured or polled anything yet.
    let mut updated = fixture.manager.get();
    updated.max_copy_text_bytes = 128;
    ConfigManager::write_config_file(&fixture.manager.inner.config_path, &updated).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.manager.get().max_copy_text_bytes != 128 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("watcher must notice an edit immediately after start_watching");
    assert_eq!(
        fixture.manager.subscribe().borrow().max_copy_text_bytes,
        128
    );
}
