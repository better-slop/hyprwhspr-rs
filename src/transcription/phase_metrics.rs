use std::fs;
use std::future::Future;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct BackendPhaseMetric {
    pub name: &'static str,
    pub wall_duration: Duration,
    pub resource_delta: BackendResourceDelta,
    pub bytes_in: Option<usize>,
    pub bytes_out: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
pub struct BackendResourceDelta {
    pub self_user_cpu: Duration,
    pub self_system_cpu: Duration,
    pub child_user_cpu: Duration,
    pub child_system_cpu: Duration,
    pub total_cpu: Duration,
    pub cpu_percent: Option<f64>,
    pub rss_delta_kb: Option<i64>,
    pub high_water_rss_kb: Option<u64>,
    pub max_rss_kb: Option<u64>,
}

impl BackendPhaseMetric {
    pub(crate) fn set_bytes_out(&mut self, bytes_out: usize) {
        self.bytes_out = Some(bytes_out);
    }
}

pub(crate) struct BackendPhaseProbe;

impl BackendPhaseProbe {
    pub(crate) fn measure<T, E>(
        name: &'static str,
        bytes_in: Option<usize>,
        operation: impl FnOnce() -> Result<T, E>,
    ) -> (Result<T, E>, BackendPhaseMetric) {
        let started_at = Instant::now();
        let start = ResourceSnapshot::capture();
        let result = operation();
        let wall_duration = started_at.elapsed();
        let end = ResourceSnapshot::capture();

        (
            result,
            BackendPhaseMetric {
                name,
                wall_duration,
                resource_delta: start.delta(end, wall_duration),
                bytes_in,
                bytes_out: None,
            },
        )
    }

    pub(crate) async fn measure_async<T, E, Fut>(
        name: &'static str,
        bytes_in: Option<usize>,
        operation: impl FnOnce() -> Fut,
    ) -> (Result<T, E>, BackendPhaseMetric)
    where
        Fut: Future<Output = Result<T, E>>,
    {
        let started_at = Instant::now();
        let start = ResourceSnapshot::capture();
        let result = operation().await;
        let wall_duration = started_at.elapsed();
        let end = ResourceSnapshot::capture();

        (
            result,
            BackendPhaseMetric {
                name,
                wall_duration,
                resource_delta: start.delta(end, wall_duration),
                bytes_in,
                bytes_out: None,
            },
        )
    }
}

#[derive(Debug, Clone, Copy)]
struct ResourceSnapshot {
    self_user_cpu: Duration,
    self_system_cpu: Duration,
    child_user_cpu: Duration,
    child_system_cpu: Duration,
    rss_kb: Option<u64>,
    high_water_rss_kb: Option<u64>,
    max_rss_kb: Option<u64>,
}

impl ResourceSnapshot {
    fn capture() -> Self {
        let self_usage = usage(libc::RUSAGE_SELF);
        let child_usage = usage(libc::RUSAGE_CHILDREN);
        let status = read_proc_status();

        Self {
            self_user_cpu: self_usage.user_cpu,
            self_system_cpu: self_usage.system_cpu,
            child_user_cpu: child_usage.user_cpu,
            child_system_cpu: child_usage.system_cpu,
            rss_kb: status.rss_kb,
            high_water_rss_kb: status.high_water_rss_kb,
            max_rss_kb: self_usage.max_rss_kb.max(child_usage.max_rss_kb),
        }
    }

    fn delta(self, end: Self, wall_duration: Duration) -> BackendResourceDelta {
        let self_user_cpu = end.self_user_cpu.saturating_sub(self.self_user_cpu);
        let self_system_cpu = end.self_system_cpu.saturating_sub(self.self_system_cpu);
        let child_user_cpu = end.child_user_cpu.saturating_sub(self.child_user_cpu);
        let child_system_cpu = end.child_system_cpu.saturating_sub(self.child_system_cpu);
        let total_cpu = self_user_cpu
            .saturating_add(self_system_cpu)
            .saturating_add(child_user_cpu)
            .saturating_add(child_system_cpu);
        let cpu_percent = if wall_duration.is_zero() {
            None
        } else {
            Some((total_cpu.as_secs_f64() / wall_duration.as_secs_f64()) * 100.0)
        };

        BackendResourceDelta {
            self_user_cpu,
            self_system_cpu,
            child_user_cpu,
            child_system_cpu,
            total_cpu,
            cpu_percent,
            rss_delta_kb: self
                .rss_kb
                .zip(end.rss_kb)
                .map(|(start, end)| end as i64 - start as i64),
            high_water_rss_kb: end.high_water_rss_kb,
            max_rss_kb: end.max_rss_kb,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Usage {
    user_cpu: Duration,
    system_cpu: Duration,
    max_rss_kb: Option<u64>,
}

fn usage(who: libc::c_int) -> Usage {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    let ok = unsafe { libc::getrusage(who, usage.as_mut_ptr()) == 0 };
    if !ok {
        return Usage {
            user_cpu: Duration::ZERO,
            system_cpu: Duration::ZERO,
            max_rss_kb: None,
        };
    }

    let usage = unsafe { usage.assume_init() };
    Usage {
        user_cpu: timeval_duration(usage.ru_utime),
        system_cpu: timeval_duration(usage.ru_stime),
        max_rss_kb: u64::try_from(usage.ru_maxrss).ok(),
    }
}

fn timeval_duration(value: libc::timeval) -> Duration {
    let secs = u64::try_from(value.tv_sec).unwrap_or_default();
    let micros = u32::try_from(value.tv_usec).unwrap_or_default();
    Duration::new(secs, micros.saturating_mul(1_000))
}

#[derive(Debug, Clone, Copy, Default)]
struct ProcStatus {
    rss_kb: Option<u64>,
    high_water_rss_kb: Option<u64>,
}

fn read_proc_status() -> ProcStatus {
    let Ok(content) = fs::read_to_string("/proc/self/status") else {
        return ProcStatus::default();
    };

    let mut status = ProcStatus::default();
    for line in content.lines() {
        if let Some(value) = line.strip_prefix("VmRSS:") {
            status.rss_kb = parse_status_kb(value);
        } else if let Some(value) = line.strip_prefix("VmHWM:") {
            status.high_water_rss_kb = parse_status_kb(value);
        }
    }

    status
}

fn parse_status_kb(value: &str) -> Option<u64> {
    value.split_whitespace().next()?.parse().ok()
}
