use chrono::{DateTime, Utc};
use opentelemetry::metrics::{Counter, Histogram, Meter, MeterProvider, ObservableGauge};
use opentelemetry::trace::TracerProvider;
use opentelemetry::KeyValue;
use opentelemetry_otlp::{MetricExporter, SpanExporter};
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::{SdkTracer, SdkTracerProvider};
use opentelemetry_sdk::Resource;
use std::sync::{Arc, RwLock, RwLockReadGuard};
use tracing::Subscriber;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::registry::LookupSpan;
use utopia_core::AppResult;
use utopia_store::jobs::QueueDepthRow;
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct QueueSnapshot(Arc<RwLock<Vec<QueueDepthRow>>>);

impl QueueSnapshot {
    pub fn replace(&self, rows: Vec<QueueDepthRow>) {
        *self.0.write().unwrap() = rows;
    }

    fn rows(&self) -> RwLockReadGuard<'_, Vec<QueueDepthRow>> {
        self.0.read().unwrap()
    }

    pub async fn refresh(&self, pool: &sqlx::PgPool) -> AppResult<()> {
        self.replace(utopia_store::jobs::queue_depth_by_base(pool).await?);
        Ok(())
    }
}

pub fn observe_queue_size(meter: &Meter, snapshot: &QueueSnapshot) -> ObservableGauge<i64> {
    let snapshot = snapshot.clone();
    meter
        .i64_observable_gauge("utopia.jobs.queue.size")
        .with_callback(move |observer| {
            for row in snapshot.rows().iter() {
                observer.observe(row.count, &queue_attributes(row));
            }
        })
        .build()
}

pub fn observe_oldest_wait(
    meter: &Meter,
    snapshot: &QueueSnapshot,
    now: impl Fn() -> DateTime<Utc> + Send + Sync + 'static,
) -> ObservableGauge<f64> {
    let snapshot = snapshot.clone();
    meter
        .f64_observable_gauge("utopia.jobs.queue.oldest_wait")
        .with_unit("s")
        .with_callback(move |observer| {
            for row in snapshot.rows().iter().filter(|row| row.status == "queued") {
                let wait = (now() - row.oldest).num_milliseconds() as f64 / 1000.0;
                observer.observe(wait, &queue_attributes(row));
            }
        })
        .build()
}

pub fn otlp_endpoint(env: impl Fn(&str) -> Option<String>) -> Option<String> {
    env("OTEL_EXPORTER_OTLP_ENDPOINT").filter(|endpoint| !endpoint.trim().is_empty())
}

const DEFAULT_SERVICE_NAME: &str = "utopia-server";

pub struct Telemetry {
    meter_provider: SdkMeterProvider,
    tracer_provider: SdkTracerProvider,
}

impl Telemetry {
    pub fn meter(&self) -> Meter {
        self.meter_provider.meter(DEFAULT_SERVICE_NAME)
    }
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        if let Err(e) = self.meter_provider.shutdown() {
            tracing::warn!(error = %e, "metrics provider shutdown failed");
        }
        if let Err(e) = self.tracer_provider.shutdown() {
            tracing::warn!(error = %e, "tracer provider shutdown failed");
        }
    }
}

/// Exports over OTLP/gRPC when `OTEL_EXPORTER_OTLP_ENDPOINT` is set; the other
/// `OTEL_*` variables (interval, headers, resource attributes) are read by the SDK.
pub fn init<S>() -> anyhow::Result<Option<(Telemetry, OpenTelemetryLayer<S, SdkTracer>)>>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    if otlp_endpoint(|name| std::env::var(name).ok()).is_none() {
        return Ok(None);
    }
    let mut resource = Resource::builder();
    if std::env::var_os("OTEL_SERVICE_NAME").is_none() {
        resource = resource.with_service_name(DEFAULT_SERVICE_NAME);
    }
    let resource = resource.build();

    let meter_provider = SdkMeterProvider::builder()
        .with_resource(resource.clone())
        .with_reader(
            PeriodicReader::builder(MetricExporter::builder().with_tonic().build()?).build(),
        )
        .build();
    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(SpanExporter::builder().with_tonic().build()?)
        .build();
    let layer =
        tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer(DEFAULT_SERVICE_NAME));
    Ok(Some((
        Telemetry {
            meter_provider,
            tracer_provider,
        },
        layer,
    )))
}

#[derive(Clone)]
pub struct ModelCallMetrics {
    duration: Histogram<f64>,
    tokens: Counter<u64>,
}

impl ModelCallMetrics {
    pub fn new(meter: &Meter) -> Self {
        Self {
            duration: meter
                .f64_histogram("utopia.llm.call.duration")
                .with_unit("s")
                .build(),
            tokens: meter.u64_counter("utopia.llm.tokens").build(),
        }
    }
}

pub struct ModelCall<'a> {
    pub model: &'a str,
    pub task: &'a str,
    pub kb: Uuid,
    pub duration: std::time::Duration,
    pub outcome: &'a str,
    pub usage: Option<utopia_llm::Usage>,
}

pub fn record_model_call(metrics: &ModelCallMetrics, call: &ModelCall) {
    let attributes = vec![
        KeyValue::new("model", call.model.to_string()),
        KeyValue::new("task", call.task.to_string()),
        KeyValue::new("kb", call.kb.to_string()),
        KeyValue::new("outcome", call.outcome.to_string()),
    ];
    metrics
        .duration
        .record(call.duration.as_secs_f64(), &attributes);
    let Some(usage) = call.usage else {
        return;
    };
    for (kind, count) in [
        ("prompt", usage.prompt_tokens),
        ("completion", usage.completion_tokens),
    ] {
        let mut labelled = attributes.clone();
        labelled.push(KeyValue::new("kind", kind));
        metrics.tokens.add(count, &labelled);
    }
}

pub fn observe_model_calls(metrics: ModelCallMetrics, kb: Uuid) -> utopia_llm::CallObserver {
    Arc::new(move |record| {
        record_model_call(
            &metrics,
            &ModelCall {
                model: &record.model,
                task: &record.task,
                kb,
                duration: record.duration,
                outcome: record.outcome,
                usage: record.usage,
            },
        )
    })
}

fn queue_attributes(row: &QueueDepthRow) -> [KeyValue; 3] {
    let base = row
        .kb_id
        .map_or_else(|| "none".to_string(), |id| id.to_string());
    [
        KeyValue::new("kb", base),
        KeyValue::new("lane", row.lane.to_string()),
        KeyValue::new("status", row.status.clone()),
    ]
}

#[cfg(test)]
mod tests {
    use super::{
        observe_model_calls, observe_oldest_wait, observe_queue_size, otlp_endpoint,
        record_model_call, ModelCall, ModelCallMetrics, QueueSnapshot,
    };
    use chrono::{Duration, Utc};
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, GaugeDataPoint, MetricData};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
    use std::collections::{BTreeMap, BTreeSet};
    use std::time::Duration as StdDuration;
    use utopia_llm::Usage;
    use utopia_store::jobs::QueueDepthRow;
    use uuid::Uuid;

    #[test]
    fn no_endpoint_leaves_export_off() {
        assert_eq!(otlp_endpoint(|_| None), None);
    }

    #[test]
    fn a_blank_endpoint_leaves_export_off() {
        assert_eq!(otlp_endpoint(|_| Some("  ".to_string())), None);
    }

    #[test]
    fn an_endpoint_turns_export_on() {
        let lookup = |name: &str| {
            (name == "OTEL_EXPORTER_OTLP_ENDPOINT").then(|| "http://collector:4317".to_string())
        };
        assert_eq!(
            otlp_endpoint(lookup),
            Some("http://collector:4317".to_string())
        );
    }

    #[test]
    fn two_queue_rows_export_two_points_labelled_by_base_lane_and_status() {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        let snapshot = QueueSnapshot::default();
        let _gauge = observe_queue_size(&provider.meter("utopia"), &snapshot);
        let kb_id = Uuid::new_v4();
        snapshot.replace(vec![
            queue_row(Some(kb_id), 1, "queued", 3),
            queue_row(None, 2, "running", 5),
        ]);

        provider.force_flush().unwrap();

        let expected = BTreeSet::from([
            (kb_id.to_string(), "1".to_string(), "queued".to_string(), 3),
            (
                "none".to_string(),
                "2".to_string(),
                "running".to_string(),
                5,
            ),
        ]);
        assert_eq!(exported_points(&exporter), expected);
    }

    fn queue_row(kb_id: Option<Uuid>, lane: i16, status: &str, count: i64) -> QueueDepthRow {
        QueueDepthRow {
            kb_id,
            lane,
            status: status.into(),
            count,
            oldest: Utc::now(),
        }
    }

    type ExportedPoint = (String, String, String, i64);

    fn labels<T>(point: &GaugeDataPoint<T>) -> (String, String, String) {
        let attr = |key: &str| {
            point
                .attributes()
                .find(|kv| kv.key.as_str() == key)
                .map(|kv| kv.value.to_string())
                .unwrap()
        };
        (attr("kb"), attr("lane"), attr("status"))
    }

    fn exported_points(exporter: &InMemoryMetricExporter) -> BTreeSet<ExportedPoint> {
        exporter
            .get_finished_metrics()
            .unwrap()
            .iter()
            .flat_map(|resource| resource.scope_metrics())
            .flat_map(|scope| scope.metrics())
            .filter_map(|metric| match metric.data() {
                AggregatedMetrics::I64(MetricData::Gauge(gauge)) => Some(gauge),
                _ => None,
            })
            .flat_map(|gauge| gauge.data_points())
            .map(|point| {
                let (kb, lane, status) = labels(point);
                (kb, lane, status, point.value())
            })
            .collect()
    }

    #[test]
    fn a_job_queued_ninety_seconds_ago_reports_a_ninety_second_wait() {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        let snapshot = QueueSnapshot::default();
        let now = Utc::now();
        let _gauge = observe_oldest_wait(&provider.meter("utopia"), &snapshot, move || now);
        let kb_id = Uuid::new_v4();
        snapshot.replace(vec![
            QueueDepthRow {
                oldest: now - Duration::seconds(90),
                ..queue_row(Some(kb_id), 0, "queued", 3)
            },
            QueueDepthRow {
                oldest: now - Duration::seconds(600),
                ..queue_row(Some(kb_id), 0, "running", 1)
            },
        ]);

        provider.force_flush().unwrap();

        let expected = vec![(
            kb_id.to_string(),
            "0".to_string(),
            "queued".to_string(),
            90.0,
        )];
        assert_eq!(exported_wait_points(&exporter), expected);
    }

    fn exported_wait_points(
        exporter: &InMemoryMetricExporter,
    ) -> Vec<(String, String, String, f64)> {
        exporter
            .get_finished_metrics()
            .unwrap()
            .iter()
            .flat_map(|resource| resource.scope_metrics())
            .flat_map(|scope| scope.metrics())
            .filter_map(|metric| match metric.data() {
                AggregatedMetrics::F64(MetricData::Gauge(gauge)) => Some(gauge),
                _ => None,
            })
            .flat_map(|gauge| gauge.data_points())
            .map(|point| {
                let (kb, lane, status) = labels(point);
                (kb, lane, status, point.value())
            })
            .collect()
    }

    #[test]
    fn a_base_whose_queue_emptied_drops_out_of_the_next_export() {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        let snapshot = QueueSnapshot::default();
        let _gauge = observe_queue_size(&provider.meter("utopia"), &snapshot);
        let emptied_kb = Uuid::new_v4();
        let busy_kb = Uuid::new_v4();
        snapshot.replace(vec![
            queue_row(Some(emptied_kb), 1, "queued", 3),
            queue_row(Some(busy_kb), 2, "running", 5),
        ]);
        provider.force_flush().unwrap();
        exporter.reset();

        snapshot.replace(vec![queue_row(Some(busy_kb), 2, "running", 5)]);
        provider.force_flush().unwrap();

        let expected = BTreeSet::from([(
            busy_kb.to_string(),
            "2".to_string(),
            "running".to_string(),
            5,
        )]);
        assert_eq!(exported_points(&exporter), expected);
    }

    #[tokio::test]
    async fn a_refresh_exports_the_queued_job_of_a_new_base() -> anyhow::Result<()> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(());
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        let (org, ws, kb_id) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'queue-refresh-test')")
            .bind(org)
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'queue-refresh-test')",
        )
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'queue-refresh-test')",
        )
        .bind(kb_id)
        .bind(ws)
        .execute(&pool)
        .await?;

        let run = async {
            sqlx::query("INSERT INTO jobs (kind, payload) VALUES ('noop', $1)")
                .bind(serde_json::json!({ "kb_id": kb_id }))
                .execute(&pool)
                .await?;
            let exporter = InMemoryMetricExporter::default();
            let provider = SdkMeterProvider::builder()
                .with_periodic_exporter(exporter.clone())
                .build();
            let snapshot = QueueSnapshot::default();
            let _gauge = observe_queue_size(&provider.meter("utopia"), &snapshot);

            snapshot.refresh(&pool).await?;
            provider.force_flush()?;

            let own_points: BTreeSet<_> = exported_points(&exporter)
                .into_iter()
                .filter(|(kb, ..)| *kb == kb_id.to_string())
                .collect();
            let expected =
                BTreeSet::from([(kb_id.to_string(), "0".to_string(), "queued".to_string(), 1)]);
            assert_eq!(own_points, expected);
            Ok::<_, anyhow::Error>(())
        }
        .await;

        let _ = sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id' = $1")
            .bind(kb_id.to_string())
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM organizations WHERE id = $1")
            .bind(org)
            .execute(&pool)
            .await;
        run
    }

    type Labels = BTreeMap<String, String>;

    fn labels_of<'a>(attributes: impl Iterator<Item = &'a opentelemetry::KeyValue>) -> Labels {
        attributes
            .map(|kv| (kv.key.to_string(), kv.value.to_string()))
            .collect()
    }

    type Durations = Vec<(Labels, u64, f64)>;
    type Tokens = BTreeSet<(Labels, u64)>;

    fn exported_model_call(exporter: &InMemoryMetricExporter) -> (Durations, Tokens) {
        let mut durations = Vec::new();
        let mut tokens = BTreeSet::new();
        for metric in exporter
            .get_finished_metrics()
            .unwrap()
            .iter()
            .flat_map(|resource| resource.scope_metrics())
            .flat_map(|scope| scope.metrics())
        {
            match (metric.name(), metric.data()) {
                ("utopia.llm.call.duration", AggregatedMetrics::F64(MetricData::Histogram(h))) => {
                    durations.extend(
                        h.data_points().map(|point| {
                            (labels_of(point.attributes()), point.count(), point.sum())
                        }),
                    );
                }
                ("utopia.llm.tokens", AggregatedMetrics::U64(MetricData::Sum(sum))) => {
                    tokens.extend(
                        sum.data_points()
                            .map(|point| (labels_of(point.attributes()), point.value())),
                    );
                }
                _ => {}
            }
        }
        (durations, tokens)
    }

    #[test]
    fn a_successful_call_records_its_time_and_both_token_counts() {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        let metrics = ModelCallMetrics::new(&provider.meter("utopia"));
        let kb_id = Uuid::new_v4();

        record_model_call(
            &metrics,
            &ModelCall {
                model: "gpt-x",
                task: "extraction",
                kb: kb_id,
                duration: StdDuration::from_millis(1500),
                outcome: "ok",
                usage: Some(Usage {
                    prompt_tokens: 120,
                    completion_tokens: 30,
                }),
            },
        );
        provider.force_flush().unwrap();

        let call = |extra: Option<(&str, &str)>| {
            let mut labels = Labels::from([
                ("model".to_string(), "gpt-x".to_string()),
                ("task".to_string(), "extraction".to_string()),
                ("kb".to_string(), kb_id.to_string()),
                ("outcome".to_string(), "ok".to_string()),
            ]);
            labels.extend(extra.map(|(key, value)| (key.to_string(), value.to_string())));
            labels
        };
        let (durations, tokens) = exported_model_call(&exporter);

        assert_eq!(durations, vec![(call(None), 1, 1.5)]);
        assert_eq!(
            tokens,
            BTreeSet::from([
                (call(Some(("kind", "prompt"))), 120),
                (call(Some(("kind", "completion"))), 30),
            ])
        );
    }

    #[test]
    fn an_observer_for_a_base_records_the_reported_call_under_that_base() {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        let metrics = ModelCallMetrics::new(&provider.meter("utopia"));
        let kb_id = Uuid::new_v4();
        let observer = observe_model_calls(metrics, kb_id);

        observer(&utopia_llm::CallRecord {
            model: "gpt-x".to_string(),
            task: "extract".to_string(),
            duration: StdDuration::from_secs(2),
            outcome: "ok",
            usage: Some(Usage {
                prompt_tokens: 11,
                completion_tokens: 7,
            }),
        });
        provider.force_flush().unwrap();

        let call = |extra: Option<(&str, &str)>| {
            let mut labels = Labels::from([
                ("model".to_string(), "gpt-x".to_string()),
                ("task".to_string(), "extract".to_string()),
                ("kb".to_string(), kb_id.to_string()),
                ("outcome".to_string(), "ok".to_string()),
            ]);
            labels.extend(extra.map(|(key, value)| (key.to_string(), value.to_string())));
            labels
        };
        let (durations, tokens) = exported_model_call(&exporter);

        assert_eq!(
            (durations, tokens),
            (
                vec![(call(None), 1, 2.0)],
                BTreeSet::from([
                    (call(Some(("kind", "prompt"))), 11),
                    (call(Some(("kind", "completion"))), 7),
                ])
            )
        );
    }
}
