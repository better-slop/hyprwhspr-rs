use crate::resource_usage::{ResourceDelta, ResourceSnapshot};
use anyhow::Result;
use std::future::Future;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(crate) struct TipResourceTimeline {
    case_start: ResourceSnapshot,
    case_start_at: Instant,
    samples: Vec<TipPhaseSample>,
}

#[derive(Debug, Clone)]
pub(crate) struct TipPhaseSample {
    pub(crate) name: &'static str,
    pub(crate) wall_duration: Duration,
    pub(crate) resource_delta: ResourceDelta,
    pub(crate) bytes_in: Option<usize>,
    pub(crate) bytes_out: Option<usize>,
}

impl TipResourceTimeline {
    pub(crate) fn new() -> Self {
        Self {
            case_start: ResourceSnapshot::capture(),
            case_start_at: Instant::now(),
            samples: Vec::new(),
        }
    }

    pub(crate) fn measure<T>(
        &mut self,
        name: &'static str,
        bytes_in: Option<usize>,
        bytes_out: Option<usize>,
        operation: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let started_at = Instant::now();
        let start = ResourceSnapshot::capture();
        let result = operation();
        self.push_sample(name, started_at.elapsed(), start, bytes_in, bytes_out);
        result
    }

    pub(crate) async fn measure_async<T, Fut>(
        &mut self,
        name: &'static str,
        bytes_in: Option<usize>,
        bytes_out: Option<usize>,
        operation: impl FnOnce() -> Fut,
    ) -> Result<T>
    where
        Fut: Future<Output = Result<T>>,
    {
        let started_at = Instant::now();
        let start = ResourceSnapshot::capture();
        let result = operation().await;
        self.push_sample(name, started_at.elapsed(), start, bytes_in, bytes_out);
        result
    }

    pub(crate) fn total_delta(&self) -> ResourceDelta {
        self.case_start
            .delta(ResourceSnapshot::capture(), self.case_start_at.elapsed())
    }

    pub(crate) fn elapsed(&self) -> Duration {
        self.case_start_at.elapsed()
    }

    pub(crate) fn samples(&self) -> &[TipPhaseSample] {
        &self.samples
    }

    pub(crate) fn set_latest_bytes_out(&mut self, name: &'static str, bytes_out: usize) {
        if let Some(sample) = self
            .samples
            .iter_mut()
            .rev()
            .find(|sample| sample.name == name)
        {
            sample.bytes_out = Some(bytes_out);
        }
    }

    fn push_sample(
        &mut self,
        name: &'static str,
        wall_duration: Duration,
        start: ResourceSnapshot,
        bytes_in: Option<usize>,
        bytes_out: Option<usize>,
    ) {
        let end = ResourceSnapshot::capture();
        self.samples.push(TipPhaseSample {
            name,
            wall_duration,
            resource_delta: start.delta(end, wall_duration),
            bytes_in,
            bytes_out,
        });
    }
}
