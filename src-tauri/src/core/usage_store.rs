use std::{collections::BTreeMap, path::Path};

use chrono::{DateTime, Duration, Local, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::core::day_boundary::local_day_bounds;
use crate::core::dedupe::compute_dedupe_key;
use crate::core::observation::{
    validate_observation, NormalizedObservation, SourceRecordIdConfidence,
};
use crate::core::pricing::{
    combine_pricing_status, estimate_model_cost, estimate_model_cost_breakdown, CostPeriodSummary,
    ModelTokenUsage, WidgetCostSummary,
};
use crate::core::profile::{
    intensity_for_cost, model_label, period_bounds, source_label, PeriodProfileSummary,
    ProfileCostDrivers, ProfileDayBucket, ProfilePeakDay, ProfilePeriod, ProfileSummary,
    RankedProfileBreakdown, YearProfileSummary,
};
use crate::core::usage_series::{
    average_tokens_per_bucket, bucket_index_for, empty_usage_buckets, WidgetUsageSeries,
    WIDGET_USAGE_ACTIVE_THRESHOLD_MINUTES, WIDGET_USAGE_WINDOW_MINUTES,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOutcome {
    Inserted,
    Duplicate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackingWindow {
    pub id: i64,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

pub const DEFAULT_RETENTION_DAYS: i64 = 365;
pub const DEFAULT_RETENTION_MIN_INTERVAL_HOURS: i64 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub observation_retention_days: i64,
    pub min_interval_hours: i64,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            observation_retention_days: DEFAULT_RETENTION_DAYS,
            min_interval_hours: DEFAULT_RETENTION_MIN_INTERVAL_HOURS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionSkipReason {
    RecentlySucceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionOutcome {
    pub ran: bool,
    pub cutoff: DateTime<Utc>,
    pub deleted_observations: usize,
    pub skipped_reason: Option<RetentionSkipReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionDiagnostics {
    pub policy_days: i64,
    pub min_interval_hours: i64,
    pub last_success_at: Option<String>,
    pub last_deleted_observations: Option<usize>,
    pub last_failure_at: Option<String>,
    pub last_error_kind: Option<String>,
}

pub struct UsageStore {
    conn: Connection,
}

#[derive(Debug, Clone)]
struct ProfileObservationRow {
    source: String,
    model: Option<String>,
    input_tokens: i64,
    output_tokens: i64,
    cached_input_tokens: i64,
    cache_creation_input_tokens: i64,
    reasoning_output_tokens: i64,
    total_tokens: i64,
    observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct PricedProfileObservationRow {
    source: String,
    model: Option<String>,
    estimated_cost: f64,
    total_tokens: i64,
    drivers: crate::core::pricing::PricedCostDrivers,
    observed_at: DateTime<Utc>,
}

impl UsageStore {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(
            r#"
            create table if not exists token_observations (
              id integer primary key autoincrement,
              tracking_window_id integer,
              source text not null,
              adapter_version text not null,
              source_record_id text not null,
              source_record_id_confidence text not null,
              session_id text,
              turn_id text,
              turn_boundary_id text,
              source_path text,
              line_no integer,
              byte_offset integer,
              input_tokens integer not null,
              output_tokens integer not null,
              cached_input_tokens integer not null,
              cache_creation_input_tokens integer not null,
              reasoning_output_tokens integer not null,
              total_tokens integer not null,
              cumulative_total_tokens integer,
              model text,
              cwd text,
              observed_at text not null,
              created_at text not null default (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
              token_payload_hash text not null,
              dedupe_key text not null
            );
            create unique index if not exists idx_token_observations_dedupe_key
              on token_observations(dedupe_key);
            create index if not exists idx_token_observations_observed_at
              on token_observations(observed_at);
            create index if not exists idx_token_observations_source_session
              on token_observations(source, session_id);
            create index if not exists idx_token_observations_source_turn_boundary
              on token_observations(source, turn_boundary_id);
            create table if not exists tracking_windows (
              id integer primary key autoincrement,
              started_at text not null,
              ended_at text
            );
            create table if not exists file_baselines (
              source_path text primary key,
              byte_offset integer not null,
              updated_at text not null default (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            );
            create table if not exists retention_state (
              key text primary key,
              value text not null
            );
            "#,
        )?;
        self.ensure_column("token_observations", "tracking_window_id", "integer")?;
        self.conn.execute(
            "create index if not exists idx_token_observations_tracking_window_id on token_observations(tracking_window_id)",
            [],
        )?;
        self.conn.execute(
            "create index if not exists idx_token_observations_tracked_observed_at on token_observations(observed_at) where tracking_window_id is not null",
            [],
        )?;
        Ok(())
    }

    fn ensure_column(&self, table: &str, column: &str, definition: &str) -> anyhow::Result<()> {
        let mut stmt = self.conn.prepare(&format!("pragma table_info({table})"))?;
        let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
        for existing in columns {
            if existing? == column {
                return Ok(());
            }
        }
        self.conn.execute(
            &format!("alter table {table} add column {column} {definition}"),
            [],
        )?;
        Ok(())
    }

    fn retention_state_value(&self, key: &str) -> anyhow::Result<Option<String>> {
        self.conn
            .query_row(
                "select value from retention_state where key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn retention_diagnostics(
        &self,
        policy: RetentionPolicy,
    ) -> anyhow::Result<RetentionDiagnostics> {
        let last_deleted_observations = self
            .retention_state_value("last_deleted_observations")?
            .and_then(|value| value.parse::<usize>().ok());
        Ok(RetentionDiagnostics {
            policy_days: policy.observation_retention_days,
            min_interval_hours: policy.min_interval_hours,
            last_success_at: self.retention_state_value("last_success_at")?,
            last_deleted_observations,
            last_failure_at: self.retention_state_value("last_failure_at")?,
            last_error_kind: self.retention_state_value("last_error_kind")?,
        })
    }

    pub fn record_retention_failure(
        &self,
        now: DateTime<Utc>,
        error_kind: &str,
    ) -> anyhow::Result<()> {
        self.conn.execute(
            r#"
            insert into retention_state (key, value)
            values ('last_failure_at', ?1)
            on conflict(key) do update set value = excluded.value
            "#,
            params![now.to_rfc3339()],
        )?;
        self.conn.execute(
            r#"
            insert into retention_state (key, value)
            values ('last_error_kind', ?1)
            on conflict(key) do update set value = excluded.value
            "#,
            params![error_kind],
        )?;
        Ok(())
    }

    pub fn run_retention_if_due(
        &mut self,
        now: DateTime<Utc>,
        policy: RetentionPolicy,
    ) -> anyhow::Result<RetentionOutcome> {
        let cutoff = now - Duration::days(policy.observation_retention_days);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let last_success_at = tx
            .query_row(
                "select value from retention_state where key = 'last_success_at'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(last_success_at) = last_success_at {
            let last_success_at =
                DateTime::parse_from_rfc3339(&last_success_at)?.with_timezone(&Utc);
            if now < last_success_at + Duration::hours(policy.min_interval_hours) {
                return Ok(RetentionOutcome {
                    ran: false,
                    cutoff,
                    deleted_observations: 0,
                    skipped_reason: Some(RetentionSkipReason::RecentlySucceeded),
                });
            }
        }

        let deleted_observations = tx.execute(
            "delete from token_observations where observed_at < ?1",
            params![cutoff.to_rfc3339()],
        )?;
        tx.execute(
            r#"
            insert into retention_state (key, value)
            values ('last_success_at', ?1)
            on conflict(key) do update set value = excluded.value
            "#,
            params![now.to_rfc3339()],
        )?;
        tx.execute(
            r#"
            insert into retention_state (key, value)
            values ('last_deleted_observations', ?1)
            on conflict(key) do update set value = excluded.value
            "#,
            params![deleted_observations.to_string()],
        )?;
        tx.execute(
            "delete from retention_state where key in ('last_failure_at', 'last_error_kind')",
            [],
        )?;
        tx.commit()?;

        Ok(RetentionOutcome {
            ran: true,
            cutoff,
            deleted_observations,
            skipped_reason: None,
        })
    }

    pub fn insert_untracked_observation_for_test(
        &self,
        observation: &NormalizedObservation,
    ) -> anyhow::Result<InsertOutcome> {
        self.insert_observation_inner(observation, None)
    }

    pub fn insert_observation_for_tracking_window(
        &self,
        observation: &NormalizedObservation,
        tracking_window_id: i64,
    ) -> anyhow::Result<InsertOutcome> {
        self.insert_observation_inner(observation, Some(tracking_window_id))
    }

    fn insert_observation_inner(
        &self,
        observation: &NormalizedObservation,
        tracking_window_id: Option<i64>,
    ) -> anyhow::Result<InsertOutcome> {
        validate_observation(observation)?;
        if let Some(tracking_window_id) = tracking_window_id {
            self.validate_tracking_window(tracking_window_id, observation.observed_at)?;
        }
        let dedupe_key = compute_dedupe_key(observation);
        let confidence = match observation.source_record_id_confidence {
            SourceRecordIdConfidence::Exact => "exact",
            SourceRecordIdConfidence::Fallback => "fallback",
        };
        let inserted = self.conn.execute(
            r#"
            insert or ignore into token_observations (
              tracking_window_id, source, adapter_version, source_record_id, source_record_id_confidence,
              session_id, turn_id, turn_boundary_id, source_path, line_no, byte_offset,
              input_tokens, output_tokens, cached_input_tokens, cache_creation_input_tokens,
              reasoning_output_tokens, total_tokens, cumulative_total_tokens, model, cwd,
              observed_at, token_payload_hash, dedupe_key
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)
            "#,
            params![
                tracking_window_id,
                observation.source,
                observation.adapter_version,
                observation.source_record_id,
                confidence,
                observation.session_id,
                observation.turn_id,
                observation.turn_boundary_id,
                observation.source_path,
                observation.line_no,
                observation.byte_offset,
                observation.input_tokens,
                observation.output_tokens,
                observation.cached_input_tokens,
                observation.cache_creation_input_tokens,
                observation.reasoning_output_tokens,
                observation.total_tokens,
                observation.cumulative_total_tokens,
                observation.model,
                observation.cwd,
                observation.observed_at.to_rfc3339(),
                observation.token_payload_hash,
                dedupe_key,
            ],
        )?;
        if inserted == 0 {
            Ok(InsertOutcome::Duplicate)
        } else {
            Ok(InsertOutcome::Inserted)
        }
    }

    fn validate_tracking_window(
        &self,
        tracking_window_id: i64,
        observed_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let window = self
            .conn
            .query_row(
                "select started_at, ended_at from tracking_windows where id = ?1",
                params![tracking_window_id],
                |row| {
                    let started_at: String = row.get(0)?;
                    let ended_at: Option<String> = row.get(1)?;
                    Ok((started_at, ended_at))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("tracking window not found: {tracking_window_id}"))?;
        let started_at = DateTime::parse_from_rfc3339(&window.0)?.with_timezone(&Utc);
        let ended_at = window
            .1
            .map(|value| {
                DateTime::parse_from_rfc3339(&value).map(|parsed| parsed.with_timezone(&Utc))
            })
            .transpose()?;
        if observed_at < started_at || ended_at.is_some_and(|ended_at| observed_at >= ended_at) {
            anyhow::bail!("observation outside tracking window: {tracking_window_id}");
        }
        Ok(())
    }

    pub fn today_total(&self, now: DateTime<Local>) -> anyhow::Result<i64> {
        let (start, next) = local_day_bounds(now);
        let total = self.conn.query_row(
            r#"
            select coalesce(sum(total_tokens), 0)
            from token_observations
            where tracking_window_id is not null
              and observed_at >= ?1 and observed_at < ?2
            "#,
            params![
                start.with_timezone(&Utc).to_rfc3339(),
                next.with_timezone(&Utc).to_rfc3339(),
            ],
            |row| row.get(0),
        )?;
        Ok(total)
    }

    pub fn latest_turn_delta(&self) -> anyhow::Result<i64> {
        let latest: Option<(String, Option<String>, Option<String>, Option<String>, i64)> = self
            .conn
            .query_row(
                r#"
                select source, session_id, turn_boundary_id, turn_id, total_tokens
                from token_observations
                where tracking_window_id is not null
                order by observed_at desc, id desc
                limit 1
                "#,
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((source, session_id, turn_boundary_id, turn_id, latest_total)) = latest else {
            return Ok(0);
        };
        let boundary = turn_boundary_id.or(turn_id);
        let Some(boundary) = boundary else {
            return Ok(latest_total);
        };
        let total = self.conn.query_row(
            r#"
            select coalesce(sum(total_tokens), 0)
            from token_observations
            where source = ?1
              and coalesce(session_id, '') = coalesce(?2, '')
              and coalesce(turn_boundary_id, turn_id, '') = ?3
              and tracking_window_id is not null
            "#,
            params![source, session_id, boundary],
            |row| row.get(0),
        )?;
        Ok(total)
    }

    pub fn state_revision(&self) -> anyhow::Result<i64> {
        self.conn
            .query_row(
                r#"
                select coalesce(max(id), 0)
                from token_observations
                where tracking_window_id is not null
                "#,
                [],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn last_observed_at(&self) -> anyhow::Result<Option<DateTime<Utc>>> {
        let value: Option<String> = self.conn.query_row(
            r#"
            select max(observed_at)
            from token_observations
            where tracking_window_id is not null
            "#,
            [],
            |row| row.get(0),
        )?;
        value
            .map(|timestamp| {
                DateTime::parse_from_rfc3339(&timestamp)
                    .map(|parsed| parsed.with_timezone(&Utc))
                    .map_err(Into::into)
            })
            .transpose()
    }

    fn tracked_observation_count_between(
        &self,
        start_at: DateTime<Utc>,
        end_at: DateTime<Utc>,
    ) -> anyhow::Result<i64> {
        Ok(self.conn.query_row(
            r#"
            select count(*)
            from token_observations
            where tracking_window_id is not null
              and observed_at >= ?1
              and observed_at < ?2
            "#,
            params![start_at.to_rfc3339(), end_at.to_rfc3339()],
            |row| row.get(0),
        )?)
    }

    fn usage_buckets_for_window(
        &self,
        window_end: DateTime<Utc>,
    ) -> anyhow::Result<Vec<crate::core::usage_series::WidgetUsageBucket>> {
        let window_start = window_end - chrono::Duration::minutes(WIDGET_USAGE_WINDOW_MINUTES);
        let mut buckets = empty_usage_buckets(window_end);
        let mut stmt = self.conn.prepare(
            r#"
            select observed_at, total_tokens
            from token_observations
            where tracking_window_id is not null
              and observed_at >= ?1
              and observed_at < ?2
            order by observed_at asc
            "#,
        )?;
        let rows = stmt.query_map(
            params![window_start.to_rfc3339(), window_end.to_rfc3339()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )?;

        for row in rows {
            let (observed_at, total_tokens) = row?;
            let observed_at = DateTime::parse_from_rfc3339(&observed_at)?.with_timezone(&Utc);
            if let Some(index) = bucket_index_for(observed_at, window_end) {
                if let Some(bucket) = buckets.get_mut(index) {
                    bucket.total_tokens += total_tokens;
                }
            }
        }

        Ok(buckets)
    }

    pub fn usage_series_at(&self, now: DateTime<Utc>) -> anyhow::Result<WidgetUsageSeries> {
        let window_start = now - chrono::Duration::minutes(WIDGET_USAGE_WINDOW_MINUTES);
        let buckets = self.usage_buckets_for_window(now)?;
        let previous_day_buckets =
            self.usage_buckets_for_window(now - chrono::Duration::hours(24))?;

        let latest_bucket_start = buckets
            .last()
            .map(|bucket| bucket.start_at)
            .unwrap_or(window_start);
        let latest_bucket_tokens = buckets
            .last()
            .map(|bucket| bucket.total_tokens)
            .unwrap_or(0);
        let latest_bucket_observation_count =
            self.tracked_observation_count_between(latest_bucket_start, now)?;
        let last_observed_at = self.last_observed_at()?;
        let latest_bucket_active = last_observed_at
            .map(|observed_at| {
                latest_bucket_observation_count > 0
                    && observed_at >= latest_bucket_start
                    && observed_at < now
                    && now - observed_at
                        <= chrono::Duration::minutes(WIDGET_USAGE_ACTIVE_THRESHOLD_MINUTES)
            })
            .unwrap_or(false);

        Ok(WidgetUsageSeries {
            window_minutes: WIDGET_USAGE_WINDOW_MINUTES,
            bucket_minutes: crate::core::usage_series::WIDGET_USAGE_BUCKET_MINUTES,
            generated_at: now,
            state_revision: self.state_revision()?,
            average_tokens_per_bucket: average_tokens_per_bucket(&buckets),
            latest_bucket_tokens,
            latest_bucket_active,
            buckets,
            previous_day_buckets,
        })
    }

    pub fn cost_summary_between(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> anyhow::Result<CostPeriodSummary> {
        let mut stmt = self.conn.prepare(
            r#"
            select coalesce(model, '') as model,
                   coalesce(sum(input_tokens), 0),
                   coalesce(sum(output_tokens), 0),
                   coalesce(sum(cached_input_tokens), 0),
                   coalesce(sum(cache_creation_input_tokens), 0),
                   coalesce(sum(reasoning_output_tokens), 0),
                   coalesce(sum(total_tokens), 0)
            from token_observations
            where tracking_window_id is not null
              and observed_at >= ?1
              and observed_at < ?2
            group by coalesce(model, '')
            "#,
        )?;
        let rows = stmt.query_map(params![start.to_rfc3339(), end.to_rfc3339()], |row| {
            Ok(ModelTokenUsage {
                model: match row.get::<_, String>(0)?.as_str() {
                    "" => None,
                    value => Some(value.to_string()),
                },
                input_tokens: row.get(1)?,
                output_tokens: row.get(2)?,
                cached_input_tokens: row.get(3)?,
                cache_creation_input_tokens: row.get(4)?,
                reasoning_output_tokens: row.get(5)?,
                total_tokens: row.get(6)?,
            })
        })?;

        let mut estimated_cost = 0.0;
        let mut total_tokens = 0;
        let mut statuses = Vec::new();
        for row in rows {
            let cost = estimate_model_cost(&row?);
            estimated_cost += cost.estimated_cost;
            total_tokens += cost.total_tokens;
            statuses.push(cost.pricing_status);
        }

        Ok(CostPeriodSummary {
            estimated_cost,
            total_tokens,
            pricing_status: combine_pricing_status(&statuses),
        })
    }

    pub fn widget_cost_summary_at(
        &self,
        now_utc: DateTime<Utc>,
        now_local: DateTime<Local>,
    ) -> anyhow::Result<WidgetCostSummary> {
        let (today_start, _) = local_day_bounds(now_local);
        let today = self.cost_summary_between(today_start.with_timezone(&Utc), now_utc)?;
        let seven_days = self.cost_summary_between(now_utc - chrono::Duration::days(7), now_utc)?;
        Ok(WidgetCostSummary {
            generated_at: now_utc,
            currency: "CNY".to_string(),
            today,
            seven_days,
        })
    }

    fn profile_rows_between(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> anyhow::Result<Vec<ProfileObservationRow>> {
        let mut stmt = self.conn.prepare(
            r#"
            select source, model, input_tokens, output_tokens, cached_input_tokens,
                   cache_creation_input_tokens, reasoning_output_tokens, total_tokens, observed_at
            from token_observations
            where tracking_window_id is not null
              and observed_at >= ?1
              and observed_at < ?2
            order by observed_at asc, id asc
            "#,
        )?;
        let rows = stmt.query_map(params![start.to_rfc3339(), end.to_rfc3339()], |row| {
            let observed_at: String = row.get(8)?;
            Ok(ProfileObservationRow {
                source: row.get(0)?,
                model: row.get(1)?,
                input_tokens: row.get(2)?,
                output_tokens: row.get(3)?,
                cached_input_tokens: row.get(4)?,
                cache_creation_input_tokens: row.get(5)?,
                reasoning_output_tokens: row.get(6)?,
                total_tokens: row.get(7)?,
                observed_at: DateTime::parse_from_rfc3339(&observed_at)
                    .map(|parsed| parsed.with_timezone(&Utc))
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            8,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn row_usage(row: &ProfileObservationRow) -> ModelTokenUsage {
        ModelTokenUsage {
            model: row.model.clone(),
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cached_input_tokens: row.cached_input_tokens,
            cache_creation_input_tokens: row.cache_creation_input_tokens,
            reasoning_output_tokens: row.reasoning_output_tokens,
            total_tokens: row.total_tokens,
        }
    }

    fn priced_rows(rows: &[ProfileObservationRow]) -> Vec<PricedProfileObservationRow> {
        rows.iter()
            .map(|row| {
                let priced = estimate_model_cost_breakdown(&Self::row_usage(row));
                PricedProfileObservationRow {
                    source: row.source.clone(),
                    model: row.model.clone(),
                    estimated_cost: priced.estimated_cost,
                    total_tokens: priced.total_tokens,
                    drivers: priced.drivers,
                    observed_at: row.observed_at,
                }
            })
            .collect()
    }

    fn ranked_breakdown(
        rows: &[PricedProfileObservationRow],
        key_for: impl Fn(&PricedProfileObservationRow) -> (String, String),
    ) -> Vec<RankedProfileBreakdown> {
        let mut grouped: BTreeMap<String, (String, f64, i64)> = BTreeMap::new();
        for row in rows {
            let (key, label) = key_for(row);
            let entry = grouped.entry(key).or_insert((label, 0.0, 0));
            entry.1 += row.estimated_cost;
            entry.2 += row.total_tokens;
        }
        let total_token_sum: i64 = grouped.values().map(|(_, _, tokens)| *tokens).sum();
        let mut ranked = grouped
            .into_iter()
            .map(
                |(key, (label, estimated_cost, total_tokens))| RankedProfileBreakdown {
                    key,
                    label,
                    estimated_cost,
                    total_tokens,
                    share: if total_token_sum > 0 {
                        total_tokens as f64 / total_token_sum as f64
                    } else {
                        0.0
                    },
                },
            )
            .collect::<Vec<_>>();
        ranked.sort_by(|a, b| {
            b.total_tokens
                .cmp(&a.total_tokens)
                .then_with(|| a.label.cmp(&b.label))
        });
        if ranked.len() > 10 {
            let omitted = ranked.split_off(9);
            let other_estimated_cost: f64 = omitted.iter().map(|row| row.estimated_cost).sum();
            let other_total_tokens: i64 = omitted.iter().map(|row| row.total_tokens).sum();
            ranked.push(RankedProfileBreakdown {
                key: "other".to_string(),
                label: "Other".to_string(),
                estimated_cost: other_estimated_cost,
                total_tokens: other_total_tokens,
                share: if total_token_sum > 0 {
                    other_total_tokens as f64 / total_token_sum as f64
                } else {
                    0.0
                },
            });
        }
        ranked
    }

    fn cost_drivers(rows: &[PricedProfileObservationRow]) -> ProfileCostDrivers {
        let mut drivers = ProfileCostDrivers::default();
        let mut input_tokens = 0;
        for row in rows {
            drivers.input_cost += row.drivers.input_cost;
            drivers.output_cost += row.drivers.output_cost;
            drivers.reasoning_output_cost += row.drivers.reasoning_output_cost;
            drivers.cache_creation_input_cost += row.drivers.cache_creation_input_cost;
            drivers.cached_input_cost += row.drivers.cached_input_cost;
            drivers.unattributed_cost += row.drivers.unattributed_cost;
            drivers.cached_input_tokens += row.drivers.cached_input_tokens;
            input_tokens += row.drivers.input_tokens;
        }
        drivers.cache_read_ratio = if input_tokens > 0 {
            (drivers.cached_input_tokens as f64 / input_tokens as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        drivers
    }

    fn year_profile_from_rows(
        rows: &[PricedProfileObservationRow],
        now_local: DateTime<Local>,
    ) -> YearProfileSummary {
        let today = now_local.date_naive();
        let first_day = today - chrono::Days::new(364);
        let mut days = (0..365)
            .map(|offset| {
                let local_date = first_day + chrono::Days::new(offset);
                (local_date, (0.0_f64, 0_i64))
            })
            .collect::<BTreeMap<_, _>>();

        for row in rows {
            let local_date = row.observed_at.with_timezone(&Local).date_naive();
            if let Some((estimated_cost, total_tokens)) = days.get_mut(&local_date) {
                *estimated_cost += row.estimated_cost;
                *total_tokens += row.total_tokens;
            }
        }

        let max_cost = days
            .values()
            .map(|(estimated_cost, _)| *estimated_cost)
            .fold(0.0_f64, f64::max);
        let estimated_cost: f64 = days.values().map(|(cost, _)| *cost).sum();
        let total_tokens: i64 = days.values().map(|(_, tokens)| *tokens).sum();
        let active_days = days
            .values()
            .filter(|(cost, tokens)| *cost > 0.0 || *tokens > 0)
            .count();
        let peak_day = days
            .iter()
            .max_by(|(_, (left_cost, _)), (_, (right_cost, _))| {
                left_cost
                    .partial_cmp(right_cost)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .and_then(|(local_date, (cost, tokens))| {
                if *cost > 0.0 || *tokens > 0 {
                    Some(ProfilePeakDay {
                        local_date: *local_date,
                        estimated_cost: *cost,
                        total_tokens: *tokens,
                    })
                } else {
                    None
                }
            });
        let buckets = days
            .into_iter()
            .map(
                |(local_date, (estimated_cost, total_tokens))| ProfileDayBucket {
                    local_date,
                    estimated_cost,
                    total_tokens,
                    intensity: intensity_for_cost(estimated_cost, max_cost),
                },
            )
            .collect();

        YearProfileSummary {
            days: buckets,
            estimated_cost,
            total_tokens,
            active_days,
            average_active_day_cost: if active_days > 0 {
                estimated_cost / active_days as f64
            } else {
                0.0
            },
            peak_day,
        }
    }

    pub fn profile_summary_at(
        &self,
        period: ProfilePeriod,
        now_utc: DateTime<Utc>,
        now_local: DateTime<Local>,
    ) -> anyhow::Result<ProfileSummary> {
        let first_year_day = now_local.date_naive() - chrono::Days::new(364);
        let year_start = first_year_day
            .and_hms_opt(0, 0, 0)
            .and_then(|start| start.and_local_timezone(Local).single())
            .ok_or_else(|| anyhow::anyhow!("invalid local year profile start: {first_year_day}"))?;
        let year_rows =
            Self::priced_rows(&self.profile_rows_between(year_start.with_timezone(&Utc), now_utc)?);
        let (period_start, period_end) = period_bounds(period, now_utc, now_local)?;
        let period_rows = Self::priced_rows(&self.profile_rows_between(period_start, period_end)?);
        let period_estimated_cost: f64 = period_rows.iter().map(|row| row.estimated_cost).sum();
        let period_total_tokens: i64 = period_rows.iter().map(|row| row.total_tokens).sum();

        Ok(ProfileSummary {
            generated_at: now_utc,
            currency: "CNY".to_string(),
            year_profile: Self::year_profile_from_rows(&year_rows, now_local),
            selected_period: PeriodProfileSummary {
                period,
                started_at: period_start,
                ended_at: period_end,
                estimated_cost: period_estimated_cost,
                total_tokens: period_total_tokens,
                model_breakdown: Self::ranked_breakdown(&period_rows, |row| {
                    let label = model_label(row.model.as_deref());
                    let key = if label == "Unknown" {
                        "unknown".to_string()
                    } else {
                        label.to_ascii_lowercase()
                    };
                    (key, label)
                }),
                source_breakdown: Self::ranked_breakdown(&period_rows, |row| {
                    let label = source_label(&row.source);
                    let key = if label == "Unknown" {
                        "unknown".to_string()
                    } else {
                        row.source.trim().to_ascii_lowercase()
                    };
                    (key, label)
                }),
                cost_drivers: Self::cost_drivers(&period_rows),
            },
        })
    }

    pub fn open_tracking_window(&self, started_at: DateTime<Utc>) -> anyhow::Result<i64> {
        self.conn.execute(
            "insert into tracking_windows (started_at) values (?1)",
            params![started_at.to_rfc3339()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn close_tracking_window(&self, ended_at: DateTime<Utc>) -> anyhow::Result<()> {
        self.conn.execute(
            "update tracking_windows set ended_at = ?1 where ended_at is null",
            params![ended_at.to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn active_tracking_windows(&self) -> anyhow::Result<Vec<TrackingWindow>> {
        let mut stmt = self.conn.prepare(
            "select id, started_at, ended_at from tracking_windows where ended_at is null order by id",
        )?;
        let rows = stmt.query_map([], |row| {
            let started_at: String = row.get(1)?;
            let ended_at: Option<String> = row.get(2)?;
            Ok(TrackingWindow {
                id: row.get(0)?,
                started_at: DateTime::parse_from_rfc3339(&started_at)
                    .unwrap()
                    .with_timezone(&Utc),
                ended_at: ended_at.map(|value| {
                    DateTime::parse_from_rfc3339(&value)
                        .unwrap()
                        .with_timezone(&Utc)
                }),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn tracking_windows_for_ingest(&self) -> anyhow::Result<Vec<TrackingWindow>> {
        let mut stmt = self
            .conn
            .prepare("select id, started_at, ended_at from tracking_windows order by id")?;
        let rows = stmt.query_map([], |row| {
            let started_at: String = row.get(1)?;
            let ended_at: Option<String> = row.get(2)?;
            Ok(TrackingWindow {
                id: row.get(0)?,
                started_at: DateTime::parse_from_rfc3339(&started_at)
                    .unwrap()
                    .with_timezone(&Utc),
                ended_at: ended_at.map(|value| {
                    DateTime::parse_from_rfc3339(&value)
                        .unwrap()
                        .with_timezone(&Utc)
                }),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn recent_observation_metadata(
        &self,
        limit: i64,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let mut stmt = self.conn.prepare(
            r#"
            select tracking_window_id, source, adapter_version, source_record_id_confidence, session_id, turn_id,
                   turn_boundary_id, source_path, line_no, byte_offset, total_tokens,
                   cumulative_total_tokens, model, cwd, observed_at, created_at
            from token_observations
            order by observed_at desc, id desc
            limit ?1
            "#,
        )?;
        let rows = stmt.query_map([limit], |row| {
            Ok(serde_json::json!({
                "tracking_window_id": row.get::<_, Option<i64>>(0)?,
                "source": row.get::<_, String>(1)?,
                "adapter_version": row.get::<_, String>(2)?,
                "source_record_id_confidence": row.get::<_, String>(3)?,
                "session_id": row.get::<_, Option<String>>(4)?,
                "turn_id": row.get::<_, Option<String>>(5)?,
                "turn_boundary_id": row.get::<_, Option<String>>(6)?,
                "source_path": row.get::<_, Option<String>>(7)?,
                "line_no": row.get::<_, Option<i64>>(8)?,
                "byte_offset": row.get::<_, Option<i64>>(9)?,
                "total_tokens": row.get::<_, i64>(10)?,
                "cumulative_total_tokens": row.get::<_, Option<i64>>(11)?,
                "model": row.get::<_, Option<String>>(12)?,
                "cwd": row.get::<_, Option<String>>(13)?,
                "observed_at": row.get::<_, String>(14)?,
                "created_at": row.get::<_, String>(15)?,
            }))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn set_file_baseline(&self, source_path: &str, byte_offset: i64) -> anyhow::Result<()> {
        self.conn.execute(
            r#"
            insert into file_baselines (source_path, byte_offset)
            values (?1, ?2)
            on conflict(source_path) do update set byte_offset = excluded.byte_offset,
              updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            "#,
            params![source_path, byte_offset],
        )?;
        Ok(())
    }

    pub fn file_baseline(&self, source_path: &str) -> anyhow::Result<Option<i64>> {
        let value = self
            .conn
            .query_row(
                "select byte_offset from file_baselines where source_path = ?1",
                params![source_path],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value)
    }
}
