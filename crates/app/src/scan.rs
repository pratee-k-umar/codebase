//! Background scanning.
//!
//! Analysis is CPU-bound and can take seconds on a large repository, so it runs
//! on a worker thread and reports back over a channel. The UI polls that
//! channel once per frame.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use codebase_intel_core::{analyze_with, AnalysisResult, Config};

/// A message from the scanning thread.
pub enum ScanUpdate {
    /// Human-readable stage description.
    Progress(String),
    Finished(Box<ScanOutcome>),
    Failed(String),
}

pub struct ScanOutcome {
    pub root: PathBuf,
    pub result: AnalysisResult,
    pub elapsed: Duration,
}

/// Start analyzing `root` in the background.
pub fn spawn(root: PathBuf, config: Config) -> Receiver<ScanUpdate> {
    let (sender, receiver) = mpsc::channel();
    let progress_sender = sender.clone();

    thread::spawn(move || {
        let started = Instant::now();
        let mut progress = |message: &str| {
            let _ = progress_sender.send(ScanUpdate::Progress(message.to_string()));
        };

        let outcome = analyze_with(&root, config.clone(), &mut progress);

        let message = match outcome {
            Ok(result) => ScanUpdate::Finished(Box::new(ScanOutcome {
                root,
                result,
                elapsed: started.elapsed(),
            })),
            Err(err) => ScanUpdate::Failed(err.to_string()),
        };

        let _ = sender.send(message);
    });

    receiver
}

/// Format a duration the way a person would read it.
pub fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs_f64();
    if seconds < 1.0 {
        format!("{} ms", duration.as_millis())
    } else if seconds < 60.0 {
        format!("{seconds:.1} s")
    } else {
        let minutes = (seconds / 60.0).floor();
        let rest = seconds - minutes * 60.0;
        format!("{minutes:.0}m {rest:.0}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(Duration::from_millis(250)), "250 ms");
        assert_eq!(format_duration(Duration::from_millis(1500)), "1.5 s");
        assert_eq!(format_duration(Duration::from_secs(75)), "1m 15s");
    }
}

