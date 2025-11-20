use std::path::PathBuf;

use code_core::global_usage_tracker::{scan_global_usage, GlobalUsageScanOptions};

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::thread_spawner;

pub(super) fn start_global_usage_refresh(
    app_event_tx: AppEventSender,
    code_home: PathBuf,
) {
    let fallback_tx = app_event_tx.clone();
    if thread_spawner::spawn_lightweight("global-usage", move || {
        let mut options = GlobalUsageScanOptions::new(code_home.clone());
        if let Ok(parallelism) = std::thread::available_parallelism() {
            options = options.with_max_workers(parallelism.get().max(1));
        }
        match scan_global_usage(options) {
            Ok(snapshot) => {
                app_event_tx.send(AppEvent::GlobalUsageSnapshotReady { snapshot });
            }
            Err(err) => {
                let message = format!("Failed to compute global usage: {}", err);
                app_event_tx.send(AppEvent::GlobalUsageSnapshotFailed { message });
            }
        }
    })
    .is_none()
    {
        fallback_tx.send(AppEvent::GlobalUsageSnapshotFailed {
            message: "Failed to start global usage task: worker limit reached".to_string(),
        });
    }
}
