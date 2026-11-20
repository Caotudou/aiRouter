//! Local usage history. Records contain timestamps and counters only.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

pub const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 24 * HOUR_MS;
const MAX_HOURS: u64 = 7 * 24;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Tokens {
    input: u64,
    output: u64,
    cached_input: u64,
    reasoning_output: u64,
    total: u64,
}

impl Tokens {
    pub(crate) fn from_total(value: &Value) -> Option<Self> {
        Some(Self {
            input: value["inputTokens"].as_u64()?,
            output: value["outputTokens"].as_u64()?,
            cached_input: value["cachedInputTokens"].as_u64().unwrap_or(0),
            reasoning_output: value["reasoningOutputTokens"].as_u64().unwrap_or(0),
            total: value["totalTokens"].as_u64()?,
        })
    }

    pub(crate) fn take_increment(&mut self, total: Self) -> Option<Self> {
        if total.total <= self.total {
            return None;
        }
        let next = Self {
            input: total.input.saturating_sub(self.input),
            output: total.output.saturating_sub(self.output),
            cached_input: total.cached_input.saturating_sub(self.cached_input),
            reasoning_output: total.reasoning_output.saturating_sub(self.reasoning_output),
            total: total.total - self.total,
        };
        *self = total;
        Some(next)
    }

    fn add(&mut self, incr: Self) {
        self.input = self.input.saturating_add(incr.input);
        self.output = self.output.saturating_add(incr.output);
        self.cached_input = self.cached_input.saturating_add(incr.cached_input);
        self.reasoning_output = self.reasoning_output.saturating_add(incr.reasoning_output);
        self.total = self.total.saturating_add(incr.total);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct QuotaReading {
    at: u64,
    used_percent: f64,
    resets_at: Option<u64>,
}

#[derive(Deserialize, Serialize)]
struct Record {
    at: u64,
    #[serde(flatten)]
    data: Event,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Event {
    Request,
    Tokens { tokens: Tokens },
    Quota {
        short: Option<QuotaReading>,
        weekly: Option<QuotaReading>,
    },
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageHour {
    start: u64,
    requests: u64,
    tokens: Tokens,
    short_quota: Option<QuotaReading>,
    weekly_quota: Option<QuotaReading>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    hours: Vec<UsageHour>,
    incomplete: bool,
}

pub struct UsageStore {
    directory: PathBuf,
    write_failed: Mutex<bool>,
}

impl UsageStore {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            write_failed: Mutex::new(false),
        }
    }

    pub(crate) async fn record_request(self: &Arc<Self>) {
        self.record(now_ms(), Event::Request).await;
    }

    pub(crate) async fn record_tokens(self: &Arc<Self>, tokens: Tokens) {
        self.record(now_ms(), Event::Tokens { tokens }).await;
    }

    #[cfg(feature = "desktop")]
    pub(crate) async fn record_quota(self: &Arc<Self>, quota: &Value) {
        let at = now_ms();
        let limits = quota["rateLimitsByLimitId"]["codex"]
            .as_object()
            .map_or(&quota["rateLimits"], |_| &quota["rateLimitsByLimitId"]["codex"]);
        let reading = |minutes| {
            ["primary", "secondary"].into_iter().find_map(|key| {
                let window = &limits[key];
                if window["windowDurationMins"].as_u64() != Some(minutes) {
                    return None;
                }
                let used_percent = window["usedPercent"].as_f64().filter(|v| *v >= 0.0)?;
                Some(QuotaReading {
                    at,
                    used_percent,
                    resets_at: window["resetsAt"].as_u64(),
                })
            })
        };
        let short = reading(300);
        let weekly = reading(10080);
        if short.is_some() || weekly.is_some() {
            self.record(at, Event::Quota { short, weekly }).await;
        }
    }

    async fn record(self: &Arc<Self>, at: u64, data: Event) {
        let store = self.clone();
        let record = Record { at, data };
        // Keep the append running if an HTTP client disconnects during the write.
        let _ = tokio::task::spawn_blocking(move || store.append(record)).await;
    }

    fn append(&self, record: Record) {
        let Ok(mut write_failed) = self.write_failed.lock() else {
            return;
        };
        let result = (|| -> io::Result<()> {
            fs::create_dir_all(&self.directory)?;
            let mut file = OpenOptions::new()
                .append(true)
                .create(true)
                .open(self.day_path(record.at / DAY_MS))?;
            // A subsequent entry stays readable after a truncated write.
            let line = format!("\n{}\n", serde_json::to_string(&record)?);
            file.write_all(line.as_bytes())
        })();
        if let Err(error) = result {
            *write_failed = true;
            eprintln!("AI Router usage write failed: {error}");
        }
    }

    fn day_path(&self, day: u64) -> PathBuf {
        self.directory.join(format!("{day}.jsonl"))
    }

    pub fn report(&self, start: u64, end: u64) -> Result<UsageReport, String> {
        let span = end.checked_sub(start).filter(|span| {
            *span > 0 && *span <= MAX_HOURS * HOUR_MS && *span % HOUR_MS == 0
        });
        let Some(span) = span else {
            return Err("请选择七天以内的整小时时段。".into());
        };
        let write_failed = self.write_failed.lock().map_err(|_| "无法读取用量记录。")?;
        let mut report = UsageReport {
            hours: (0..span / HOUR_MS)
                .map(|hour| UsageHour { start: start + hour * HOUR_MS, ..UsageHour::default() })
                .collect(),
            incomplete: *write_failed,
        };
        for day in start / DAY_MS..=(end - 1) / DAY_MS {
            let file = match File::open(self.day_path(day)) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => return Err("无法读取本机用量记录。".into()),
            };
            for line in BufReader::new(file).lines() {
                let line = line.map_err(|_| "无法读取本机用量记录。")?;
                if line.is_empty() {
                    continue;
                }
                let Ok(record) = serde_json::from_str::<Record>(&line) else {
                    report.incomplete = true;
                    continue;
                };
                if !(start..end).contains(&record.at) {
                    continue;
                }
                let hour = &mut report.hours[((record.at - start) / HOUR_MS) as usize];
                match record.data {
                    Event::Request => hour.requests = hour.requests.saturating_add(1),
                    Event::Tokens { tokens } => hour.tokens.add(tokens),
                    Event::Quota { short, weekly } => {
                        keep_latest(&mut hour.short_quota, short);
                        keep_latest(&mut hour.weekly_quota, weekly);
                    }
                }
            }
        }
        Ok(report)
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn keep_latest(bucket: &mut Option<QuotaReading>, candidate: Option<QuotaReading>) {
    if let Some(candidate) = candidate {
        if bucket.is_none_or(|current| current.at <= candidate.at) {
            *bucket = Some(candidate);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_use_the_selected_hour_boundaries_and_keep_latest_quota() {
        let directory = std::env::temp_dir().join(format!("ai-router-usage-{}", uuid::Uuid::new_v4()));
        let store = UsageStore::new(directory.clone());
        // The local hour can start halfway through a UTC hour.
        let start = DAY_MS - HOUR_MS / 2;
        let boundary = start + HOUR_MS;
        for at in [start - 1, start, DAY_MS, boundary, start + 2 * HOUR_MS] {
            store.append(Record { at, data: Event::Request });
        }
        store.append(Record {
            at: boundary - 1,
            data: Event::Tokens { tokens: Tokens { input: 20, total: 20, ..Tokens::default() } },
        });
        for (at, used_percent) in [(boundary - 1, 12.0), (boundary - 2, 10.0)] {
            store.append(Record {
                at,
                data: Event::Quota {
                    short: Some(QuotaReading { at, used_percent, resets_at: Some(12345) }),
                    weekly: None,
                },
            });
        }
        let report = UsageStore::new(directory.clone()).report(start, start + 2 * HOUR_MS).unwrap();
        assert_eq!(report.hours[0].requests, 2);
        assert_eq!(report.hours[0].tokens.total, 20);
        assert_eq!(report.hours[0].short_quota.unwrap().used_percent, 12.0);
        assert_eq!(report.hours[1].requests, 1);
        assert!(report.hours[1].short_quota.is_none());
        assert!(!report.incomplete);
        assert!(store.report(start, start + (MAX_HOURS + 1) * HOUR_MS).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(feature = "desktop")]
    #[tokio::test]
    async fn quota_sampler_stores_only_codex_window_numbers() {
        let directory = std::env::temp_dir().join(format!("ai-router-quota-{}", uuid::Uuid::new_v4()));
        let store = Arc::new(UsageStore::new(directory.clone()));
        let now = now_ms();
        store.record_quota(&serde_json::json!({
            "accountId":"do-not-store-account",
            "rateLimitsByLimitId":{"codex":{
                "primary":{"windowDurationMins":300,"usedPercent":22.5,"resetsAt":12345},
                "secondary":{"windowDurationMins":10080,"usedPercent":0,"resetsAt":45678}
            }},
            "rateLimits":{"primary":{"windowDurationMins":300,"usedPercent":42}}
        })).await;
        let start = now - now % HOUR_MS;
        let report = store.report(start, start + 2 * HOUR_MS).unwrap();
        let sample = report.hours.iter().find(|hour| hour.short_quota.is_some()).unwrap();
        assert_eq!(sample.short_quota.unwrap().used_percent, 22.5);
        assert_eq!(sample.weekly_quota.unwrap().used_percent, 0.0);
        for file in fs::read_dir(&directory).unwrap() {
            assert!(!fs::read_to_string(file.unwrap().path()).unwrap().contains("do-not-store-account"));
        }
        fs::remove_dir_all(directory).unwrap();
    }
}
