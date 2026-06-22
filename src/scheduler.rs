use tokio::sync::oneshot;
use tokio::time::{interval, Duration};
use crate::app_event::AppEvent;

pub struct WallpaperScheduler {
    tx: std::sync::mpsc::Sender<AppEvent>,
    cancel_tx: Option<oneshot::Sender<()>>,
}

impl WallpaperScheduler {
    pub fn new(tx: std::sync::mpsc::Sender<AppEvent>) -> Self {
        Self { tx, cancel_tx: None }
    }

    /// Start the periodic auto-switch timer.
    /// Cancels any existing timer before starting the new one.
    pub fn start(&mut self, interval_secs: u64) {
        self.stop();

        if interval_secs == 0 {
            return;
        }

        let (cancel_tx, mut cancel_rx) = oneshot::channel::<()>();
        self.cancel_tx = Some(cancel_tx);
        let tx = self.tx.clone();

        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(interval_secs));
            // Skip the first immediate tick so we don't trigger switch immediately on start
            ticker.tick().await;

            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        let _ = tx.send(AppEvent::Next);
                    }
                    _ = &mut cancel_rx => {
                        break;
                    }
                }
            }
        });
    }

    /// Stop the current auto-switch timer.
    pub fn stop(&mut self) {
        if let Some(cancel) = self.cancel_tx.take() {
            let _ = cancel.send(());
        }
    }
}
