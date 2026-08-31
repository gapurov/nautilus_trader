// -------------------------------------------------------------------------------------------------
//  Copyright (C) 2015-2026 Nautech Systems Pty Ltd. All rights reserved.
//  https://nautechsystems.io
//
//  Licensed under the GNU Lesser General Public License Version 3.0 (the "License");
//  You may not use this file except in compliance with the License.
//  You may obtain a copy of the License at https://www.gnu.org/licenses/lgpl-3.0.en.html
//
//  Unless required by applicable law or agreed to in writing, software
//  distributed under the License is distributed on an "AS IS" BASIS,
//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//  See the License for the specific language governing permissions and
//  limitations under the License.
// -------------------------------------------------------------------------------------------------

//! Account-wide Dragonfly admission, response reconciliation, and reconnect coordination.

use std::{
    fmt::Debug,
    sync::{
        Arc, PoisonError, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use nautilus_common::live::get_runtime;
use nautilus_core::UUID4;
use redis::{Cmd, ErrorKind, FromRedisValue, ServerErrorKind, aio::ConnectionManager};
use thiserror::Error;

const ROLLING_WINDOW_MS: u64 = 60_000;
const CONCURRENCY_POLL_INTERVAL: Duration = Duration::from_millis(50);

const BOOTSTRAP_SCRIPT: &str = include_str!("../resources/bootstrap.lua");
const ADMIT_SCRIPT: &str = include_str!("../resources/admit.lua");
const RENEW_SCRIPT: &str = include_str!("../resources/renew.lua");
const RELEASE_SCRIPT: &str = include_str!("../resources/release.lua");
const RECONCILE_SCRIPT: &str = include_str!("../resources/reconcile.lua");
const RECONNECT_SCRIPT: &str = include_str!("../resources/reconnect.lua");

/// Dragonfly coordination failure.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum CoordinationError {
    #[error("Dragonfly coordination is unavailable")]
    Unavailable,
    #[error("Dragonfly coordination state reset")]
    StateReset,
    #[error("configured coordination server is not Dragonfly")]
    NotDragonfly,
}

/// Reason an otherwise valid request was not admitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionDeniedKind {
    GlobalBlock,
    MinuteLimit,
    ConcurrencyLimit,
    DailyBudget,
}

/// An atomic admission result.
#[derive(Debug)]
pub enum AdmissionDecision {
    Admitted(AdmissionLease),
    Denied {
        kind: AdmissionDeniedKind,
        retry_after: Duration,
    },
}

/// Response-derived account limit observations.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResponseObservation {
    pub retry_after_until_ms: Option<i64>,
    pub minute_request_counter: Option<i64>,
    pub requests_per_minute_remaining: Option<i64>,
    pub requests_per_minute_reset_ms: Option<i64>,
    pub success: bool,
    pub concurrent_limit_exceeded: bool,
}

#[derive(Clone, Debug)]
struct GateKeys {
    sentinel: String,
    starts: String,
    leases: String,
    daily: String,
    state: String,
}

impl GateKeys {
    fn new(scope_hash: &str) -> Self {
        let prefix = format!("nautilus:uw:{{{scope_hash}}}");
        Self {
            sentinel: format!("{prefix}:epoch"),
            starts: format!("{prefix}:starts"),
            leases: format!("{prefix}:leases"),
            daily: format!("{prefix}:daily"),
            state: format!("{prefix}:state"),
        }
    }
}

struct LoadedScript {
    code: &'static str,
    hash: RwLock<String>,
}

impl Debug for LoadedScript {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(LoadedScript))
            .field("hash", &self.hash)
            .finish_non_exhaustive()
    }
}

impl LoadedScript {
    fn new(code: &'static str) -> Self {
        Self {
            code,
            hash: RwLock::new(String::new()),
        }
    }

    async fn load(&self, connection: &mut ConnectionManager) -> redis::RedisResult<()> {
        let hash: String = redis::cmd("SCRIPT")
            .arg("LOAD")
            .arg(self.code)
            .query_async(connection)
            .await?;
        *self.hash.write().unwrap_or_else(PoisonError::into_inner) = hash;
        Ok(())
    }

    async fn invoke<T, F>(
        &self,
        connection: &mut ConnectionManager,
        configure: F,
    ) -> redis::RedisResult<T>
    where
        T: FromRedisValue,
        F: Fn(&mut Cmd),
    {
        let command = self.command(&configure);
        match command.query_async(connection).await {
            Ok(value) => Ok(value),
            Err(e) if e.kind() == ErrorKind::Server(ServerErrorKind::NoScript) => {
                self.load(connection).await?;
                self.command(&configure).query_async(connection).await
            }
            Err(e) => Err(e),
        }
    }

    fn command<F>(&self, configure: &F) -> Cmd
    where
        F: Fn(&mut Cmd),
    {
        let hash = self
            .hash
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut command = redis::cmd("EVALSHA");
        command.arg(hash);
        configure(&mut command);
        command
    }
}

#[derive(Debug)]
struct Scripts {
    bootstrap: LoadedScript,
    admit: LoadedScript,
    renew: LoadedScript,
    release: LoadedScript,
    reconcile: LoadedScript,
    reconnect: LoadedScript,
}

impl Scripts {
    fn new() -> Self {
        Self {
            bootstrap: LoadedScript::new(BOOTSTRAP_SCRIPT),
            admit: LoadedScript::new(ADMIT_SCRIPT),
            renew: LoadedScript::new(RENEW_SCRIPT),
            release: LoadedScript::new(RELEASE_SCRIPT),
            reconcile: LoadedScript::new(RECONCILE_SCRIPT),
            reconnect: LoadedScript::new(RECONNECT_SCRIPT),
        }
    }

    async fn load_all(&self, connection: &mut ConnectionManager) -> redis::RedisResult<()> {
        self.bootstrap.load(connection).await?;
        self.admit.load(connection).await?;
        self.renew.load(connection).await?;
        self.release.load(connection).await?;
        self.reconcile.load(connection).await?;
        self.reconnect.load(connection).await?;
        Ok(())
    }
}

struct GateInner {
    connection: ConnectionManager,
    keys: GateKeys,
    scripts: Scripts,
    epoch: String,
    requests_per_minute: u32,
    concurrent_requests: u32,
    daily_request_limit: u32,
    lease_ttl_ms: u64,
    reconnect_interval_ms: u64,
    healthy: AtomicBool,
}

impl Debug for GateInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(GateInner))
            .field("keys", &self.keys)
            .field("epoch", &self.epoch)
            .field("requests_per_minute", &self.requests_per_minute)
            .field("concurrent_requests", &self.concurrent_requests)
            .field("daily_request_limit", &self.daily_request_limit)
            .field("lease_ttl_ms", &self.lease_ttl_ms)
            .field("reconnect_interval_ms", &self.reconnect_interval_ms)
            .field("healthy", &self.healthy)
            .finish_non_exhaustive()
    }
}

/// Cloneable account-wide coordination gate backed only by Dragonfly.
#[derive(Clone, Debug)]
pub struct DragonflyGate {
    inner: Arc<GateInner>,
}

impl DragonflyGate {
    /// Connects to Dragonfly, verifies server identity, loads scripts, and pins the state epoch.
    ///
    /// # Errors
    ///
    /// Returns an error if the URL is invalid, Dragonfly is unavailable, the server is not
    /// Dragonfly, a script cannot load, or bootstrap state cannot be established.
    pub async fn connect(
        dragonfly_url: &str,
        scope_hash: &str,
        requests_per_minute: u32,
        concurrent_requests: u32,
        daily_request_limit: u32,
        lease_ttl: Duration,
        reconnect_interval: Duration,
    ) -> Result<Self, CoordinationError> {
        let client =
            redis::Client::open(dragonfly_url).map_err(|_| CoordinationError::Unavailable)?;
        let mut connection = client
            .get_connection_manager()
            .await
            .map_err(|_| CoordinationError::Unavailable)?;
        let server_info: String = redis::cmd("INFO")
            .arg("SERVER")
            .query_async(&mut connection)
            .await
            .map_err(|_| CoordinationError::Unavailable)?;
        if !server_info.to_ascii_lowercase().contains("dragonfly") {
            return Err(CoordinationError::NotDragonfly);
        }

        let scripts = Scripts::new();
        scripts
            .load_all(&mut connection)
            .await
            .map_err(|_| CoordinationError::Unavailable)?;
        let keys = GateKeys::new(scope_hash);
        let proposed_epoch = UUID4::new().to_string();
        let (epoch, _created): (String, i64) = scripts
            .bootstrap
            .invoke(&mut connection, |command| {
                command.arg(1).arg(&keys.sentinel).arg(&proposed_epoch);
            })
            .await
            .map_err(|_| CoordinationError::Unavailable)?;

        Ok(Self {
            inner: Arc::new(GateInner {
                connection,
                keys,
                scripts,
                epoch,
                requests_per_minute,
                concurrent_requests,
                daily_request_limit,
                lease_ttl_ms: duration_millis(lease_ttl),
                reconnect_interval_ms: duration_millis(reconnect_interval),
                healthy: AtomicBool::new(true),
            }),
        })
    }

    /// Returns whether the last coordination operation succeeded.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.inner.healthy.load(Ordering::Acquire)
    }

    /// Waits for account-wide availability and atomically admits one HTTP attempt.
    ///
    /// Returns a denial only when the UTC daily request budget is exhausted.
    ///
    /// # Errors
    ///
    /// Returns an error when Dragonfly is unavailable or the pinned coordination epoch changed.
    pub async fn admit_http(&self) -> Result<AdmissionDecision, CoordinationError> {
        loop {
            match self.try_admit_http().await? {
                AdmissionDecision::Denied {
                    kind: AdmissionDeniedKind::ConcurrencyLimit,
                    retry_after,
                } => tokio::time::sleep(retry_after.min(CONCURRENCY_POLL_INTERVAL)).await,
                AdmissionDecision::Denied {
                    kind: AdmissionDeniedKind::GlobalBlock | AdmissionDeniedKind::MinuteLimit,
                    retry_after,
                } => tokio::time::sleep(retry_after).await,
                decision => return Ok(decision),
            }
        }
    }

    async fn try_admit_http(&self) -> Result<AdmissionDecision, CoordinationError> {
        let lease_id = UUID4::new().to_string();
        let mut connection = self.inner.connection.clone();
        let result: (i64, i64, i64, i64, i64, i64) = self
            .inner
            .scripts
            .admit
            .invoke(&mut connection, |command| {
                command
                    .arg(5)
                    .arg(&self.inner.keys.sentinel)
                    .arg(&self.inner.keys.starts)
                    .arg(&self.inner.keys.leases)
                    .arg(&self.inner.keys.daily)
                    .arg(&self.inner.keys.state)
                    .arg(&self.inner.epoch)
                    .arg(&lease_id)
                    .arg(self.inner.requests_per_minute)
                    .arg(self.inner.concurrent_requests)
                    .arg(self.inner.daily_request_limit)
                    .arg(self.inner.lease_ttl_ms)
                    .arg(ROLLING_WINDOW_MS);
            })
            .await
            .map_err(|_| self.unavailable())?;
        self.inner.healthy.store(true, Ordering::Release);

        match result.0 {
            0 => Ok(AdmissionDecision::Admitted(AdmissionLease {
                gate: self.clone(),
                lease_id,
                admitted_at_ms: result.2,
                released: Arc::new(AtomicBool::new(false)),
            })),
            1 => Ok(denied(AdmissionDeniedKind::GlobalBlock, result.1)),
            2 => Ok(denied(AdmissionDeniedKind::MinuteLimit, result.1)),
            3 => Ok(denied(AdmissionDeniedKind::ConcurrencyLimit, result.1)),
            4 => Ok(denied(AdmissionDeniedKind::DailyBudget, result.1)),
            5 => Err(self.state_reset()),
            _ => Err(self.unavailable()),
        }
    }

    /// Atomically reconciles provider rate and concurrency observations.
    ///
    /// # Errors
    ///
    /// Returns an error when Dragonfly is unavailable or the pinned coordination epoch changed.
    pub async fn reconcile_response(
        &self,
        observation: ResponseObservation,
    ) -> Result<(), CoordinationError> {
        let mut connection = self.inner.connection.clone();
        let result: (i64, i64) = self
            .inner
            .scripts
            .reconcile
            .invoke(&mut connection, |command| {
                command
                    .arg(3)
                    .arg(&self.inner.keys.sentinel)
                    .arg(&self.inner.keys.leases)
                    .arg(&self.inner.keys.state)
                    .arg(&self.inner.epoch)
                    .arg(observation.retry_after_until_ms.unwrap_or(0))
                    .arg(observation.minute_request_counter.unwrap_or(-1))
                    .arg(observation.requests_per_minute_remaining.unwrap_or(-1))
                    .arg(observation.requests_per_minute_reset_ms.unwrap_or(0))
                    .arg(i32::from(observation.success))
                    .arg(i32::from(observation.concurrent_limit_exceeded))
                    .arg(self.inner.concurrent_requests)
                    .arg(ROLLING_WINDOW_MS);
            })
            .await
            .map_err(|_| self.unavailable())?;

        if result.0 == 0 {
            return Err(self.state_reset());
        }
        self.inner.healthy.store(true, Ordering::Release);
        Ok(())
    }

    /// Atomically admits a WebSocket connection or reconnect start.
    ///
    /// # Errors
    ///
    /// Returns an error when Dragonfly is unavailable or the pinned coordination epoch changed.
    pub async fn admit_reconnect(&self) -> Result<Option<Duration>, CoordinationError> {
        let mut connection = self.inner.connection.clone();
        let result: (i64, i64, i64) = self
            .inner
            .scripts
            .reconnect
            .invoke(&mut connection, |command| {
                command
                    .arg(2)
                    .arg(&self.inner.keys.sentinel)
                    .arg(&self.inner.keys.state)
                    .arg(&self.inner.epoch)
                    .arg(self.inner.reconnect_interval_ms);
            })
            .await
            .map_err(|_| self.unavailable())?;
        self.inner.healthy.store(true, Ordering::Release);

        match result.0 {
            0 => Ok(None),
            1 => Ok(Some(Duration::from_millis(nonnegative_u64(result.1)))),
            2 => Err(self.state_reset()),
            _ => Err(self.unavailable()),
        }
    }

    async fn renew_lease(&self, lease_id: &str) -> Result<(), CoordinationError> {
        let mut connection = self.inner.connection.clone();
        let renewed: i64 = self
            .inner
            .scripts
            .renew
            .invoke(&mut connection, |command| {
                command
                    .arg(2)
                    .arg(&self.inner.keys.sentinel)
                    .arg(&self.inner.keys.leases)
                    .arg(&self.inner.epoch)
                    .arg(lease_id)
                    .arg(self.inner.lease_ttl_ms);
            })
            .await
            .map_err(|_| self.unavailable())?;

        match renewed {
            1 => {
                self.inner.healthy.store(true, Ordering::Release);
                Ok(())
            }
            0 => Err(self.unavailable()),
            _ => Err(self.state_reset()),
        }
    }

    async fn release_lease(&self, lease_id: &str) -> Result<(), CoordinationError> {
        let mut connection = self.inner.connection.clone();
        let released: i64 = self
            .inner
            .scripts
            .release
            .invoke(&mut connection, |command| {
                command
                    .arg(2)
                    .arg(&self.inner.keys.sentinel)
                    .arg(&self.inner.keys.leases)
                    .arg(&self.inner.epoch)
                    .arg(lease_id);
            })
            .await
            .map_err(|_| self.unavailable())?;

        if released < 0 {
            return Err(self.state_reset());
        }
        self.inner.healthy.store(true, Ordering::Release);
        Ok(())
    }

    fn unavailable(&self) -> CoordinationError {
        self.inner.healthy.store(false, Ordering::Release);
        CoordinationError::Unavailable
    }

    fn state_reset(&self) -> CoordinationError {
        self.inner.healthy.store(false, Ordering::Release);
        CoordinationError::StateReset
    }
}

/// Account-wide concurrency lease for one HTTP attempt.
pub struct AdmissionLease {
    gate: DragonflyGate,
    lease_id: String,
    admitted_at_ms: i64,
    released: Arc<AtomicBool>,
}

impl Debug for AdmissionLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(AdmissionLease))
            .field("lease_id", &self.lease_id)
            .field("admitted_at_ms", &self.admitted_at_ms)
            .field("released", &self.released)
            .finish()
    }
}

impl AdmissionLease {
    /// Returns Dragonfly's admission timestamp in Unix milliseconds.
    #[must_use]
    pub const fn admitted_at_ms(&self) -> i64 {
        self.admitted_at_ms
    }

    /// Returns the interval for renewing this lease while transport is active.
    #[must_use]
    pub fn renewal_interval(&self) -> Duration {
        Duration::from_millis((self.gate.inner.lease_ttl_ms / 3).max(1))
    }

    /// Extends this active lease from Dragonfly's current time.
    ///
    /// # Errors
    ///
    /// Returns an error if the lease was released, lost, or coordination failed.
    pub async fn renew(&self) -> Result<(), CoordinationError> {
        if self.released.load(Ordering::Acquire) {
            return Err(CoordinationError::Unavailable);
        }
        self.gate.renew_lease(&self.lease_id).await
    }

    /// Releases account-wide concurrency immediately.
    ///
    /// # Errors
    ///
    /// Returns an error if Dragonfly is unavailable or its coordination state reset.
    pub async fn release(&self) -> Result<(), CoordinationError> {
        if self.released.swap(true, Ordering::AcqRel) {
            return Ok(());
        }

        match self.gate.release_lease(&self.lease_id).await {
            Ok(()) => Ok(()),
            Err(e) => {
                self.released.store(false, Ordering::Release);
                Err(e)
            }
        }
    }
}

impl Drop for AdmissionLease {
    fn drop(&mut self) {
        if self.released.swap(true, Ordering::AcqRel) {
            return;
        }
        let gate = self.gate.clone();
        let lease_id = self.lease_id.clone();
        get_runtime().spawn(async move {
            if gate.release_lease(&lease_id).await.is_err() {
                log::warn!("Failed to release Unusual Whales Dragonfly admission lease");
            }
        });
    }
}

fn denied(kind: AdmissionDeniedKind, wait_ms: i64) -> AdmissionDecision {
    AdmissionDecision::Denied {
        kind,
        retry_after: Duration::from_millis(nonnegative_u64(wait_ms).max(1)),
    }
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn nonnegative_u64(value: i64) -> u64 {
    u64::try_from(value.max(0)).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use nautilus_core::UUID4;
    use regex::Regex;
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn all_coordination_keys_share_full_scope_hash_tag() {
        let scope_hash = "0123456789abcdef";
        let keys = GateKeys::new(scope_hash);
        let expected = format!("{{{scope_hash}}}");

        for key in [
            keys.sentinel,
            keys.starts,
            keys.leases,
            keys.daily,
            keys.state,
        ] {
            assert!(key.contains(&expected));
        }
    }

    #[rstest]
    fn production_scripts_do_not_disable_atomicity_or_allow_undeclared_keys() {
        for script in [
            BOOTSTRAP_SCRIPT,
            ADMIT_SCRIPT,
            RELEASE_SCRIPT,
            RECONCILE_SCRIPT,
            RECONNECT_SCRIPT,
        ] {
            assert!(!script.contains("disable-atomicity"));
            assert!(!script.contains("allow-undeclared-keys"));
        }
    }

    #[rstest]
    fn every_lua_key_access_is_declared() {
        let scripts = [
            (BOOTSTRAP_SCRIPT, 1_usize),
            (ADMIT_SCRIPT, 5),
            (RELEASE_SCRIPT, 2),
            (RECONCILE_SCRIPT, 3),
            (RECONNECT_SCRIPT, 2),
        ];
        let key_pattern = Regex::new(r"KEYS\[(\d+)\]").unwrap();
        for (script, declared) in scripts {
            let keys: HashSet<usize> = key_pattern
                .captures_iter(script)
                .map(|capture| capture[1].parse::<usize>().unwrap())
                .collect();
            assert!(keys.iter().all(|key| *key <= declared));
            assert_eq!(keys.len(), declared);
        }
    }

    async fn integration_gate(
        requests_per_minute: u32,
        concurrent_requests: u32,
        daily_request_limit: u32,
        lease_ttl: Duration,
    ) -> (DragonflyGate, String) {
        let url = std::env::var("UNUSUAL_WHALES_DRAGONFLY_TEST_URL")
            .expect("UNUSUAL_WHALES_DRAGONFLY_TEST_URL is required");
        let scope_hash = blake3::hash(UUID4::new().as_str().as_bytes())
            .to_hex()
            .to_string();
        let gate = DragonflyGate::connect(
            &url,
            &scope_hash,
            requests_per_minute,
            concurrent_requests,
            daily_request_limit,
            lease_ttl,
            Duration::from_millis(50),
        )
        .await
        .expect("UNUSUAL_WHALES_DRAGONFLY_TEST_URL must point to Dragonfly");
        (gate, url)
    }

    async fn admitted(gate: &DragonflyGate) -> AdmissionLease {
        match gate.admit_http().await.unwrap() {
            AdmissionDecision::Admitted(lease) => lease,
            decision => panic!("expected admission, was {decision:?}"),
        }
    }

    #[tokio::test]
    #[ignore = "requires UNUSUAL_WHALES_DRAGONFLY_TEST_URL"]
    async fn dragonfly_concurrent_admission_is_atomic() {
        let (gate, _) = integration_gate(10, 1, 10, Duration::from_secs(1)).await;
        let (first, second) = tokio::join!(gate.try_admit_http(), gate.try_admit_http());
        let mut lease = None;
        let mut denied = 0;

        for decision in [first.unwrap(), second.unwrap()] {
            match decision {
                AdmissionDecision::Admitted(admission_lease) => {
                    assert!(lease.replace(admission_lease).is_none());
                }
                AdmissionDecision::Denied {
                    kind: AdmissionDeniedKind::ConcurrencyLimit,
                    ..
                } => denied += 1,
                decision => panic!("unexpected admission decision: {decision:?}"),
            }
        }
        assert_eq!(denied, 1);
        lease.unwrap().release().await.unwrap();
        admitted(&gate).await.release().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires UNUSUAL_WHALES_DRAGONFLY_TEST_URL"]
    async fn fourth_caller_waits_until_one_of_three_leases_releases() {
        let (gate, _) = integration_gate(10, 3, 10, Duration::from_secs(1)).await;
        let first = admitted(&gate).await;
        let second = admitted(&gate).await;
        let third = admitted(&gate).await;
        let waiting_gate = gate.clone();
        let fourth = tokio::spawn(async move { waiting_gate.admit_http().await });

        tokio::time::sleep(Duration::from_millis(75)).await;
        assert!(!fourth.is_finished());
        first.release().await.unwrap();

        let fourth = tokio::time::timeout(Duration::from_millis(500), fourth)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let fourth = match fourth {
            AdmissionDecision::Admitted(lease) => lease,
            decision => panic!("expected admission after release, was {decision:?}"),
        };
        fourth.release().await.unwrap();
        second.release().await.unwrap();
        third.release().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires UNUSUAL_WHALES_DRAGONFLY_TEST_URL"]
    async fn renewed_lease_remains_active_beyond_its_original_expiry() {
        let (gate, _) = integration_gate(10, 1, 10, Duration::from_secs(1)).await;
        let lease = admitted(&gate).await;
        tokio::time::sleep(Duration::from_millis(600)).await;
        lease.renew().await.unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;

        assert!(matches!(
            gate.try_admit_http().await.unwrap(),
            AdmissionDecision::Denied {
                kind: AdmissionDeniedKind::ConcurrencyLimit,
                ..
            }
        ));
        lease.release().await.unwrap();
        admitted(&gate).await.release().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires UNUSUAL_WHALES_DRAGONFLY_TEST_URL"]
    async fn dragonfly_rolling_minute_and_daily_budgets_are_enforced() {
        let (minute_gate, _) = integration_gate(1, 1, 10, Duration::from_secs(1)).await;
        admitted(&minute_gate).await.release().await.unwrap();
        assert!(matches!(
            minute_gate.try_admit_http().await.unwrap(),
            AdmissionDecision::Denied {
                kind: AdmissionDeniedKind::MinuteLimit,
                ..
            }
        ));

        let (daily_gate, _) = integration_gate(10, 1, 1, Duration::from_secs(1)).await;
        admitted(&daily_gate).await.release().await.unwrap();
        assert!(matches!(
            daily_gate.try_admit_http().await.unwrap(),
            AdmissionDecision::Denied {
                kind: AdmissionDeniedKind::DailyBudget,
                ..
            }
        ));
    }

    #[tokio::test]
    #[ignore = "requires UNUSUAL_WHALES_DRAGONFLY_TEST_URL"]
    async fn denied_claim_changes_no_usage_state() {
        let (gate, _) = integration_gate(2, 1, 2, Duration::from_secs(1)).await;
        let first = admitted(&gate).await;
        assert!(matches!(
            gate.try_admit_http().await.unwrap(),
            AdmissionDecision::Denied {
                kind: AdmissionDeniedKind::ConcurrencyLimit,
                ..
            }
        ));
        first.release().await.unwrap();
        admitted(&gate).await.release().await.unwrap();
        assert!(matches!(
            gate.try_admit_http().await.unwrap(),
            AdmissionDecision::Denied {
                kind: AdmissionDeniedKind::MinuteLimit,
                ..
            }
        ));
    }

    #[tokio::test]
    #[ignore = "requires UNUSUAL_WHALES_DRAGONFLY_TEST_URL"]
    async fn expired_lease_recovers_after_crash() {
        let (gate, _) = integration_gate(10, 1, 10, Duration::from_millis(20)).await;
        let lease = admitted(&gate).await;
        let _crashed_process_lease = std::mem::ManuallyDrop::new(lease);
        tokio::time::sleep(Duration::from_millis(30)).await;
        admitted(&gate).await.release().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires UNUSUAL_WHALES_DRAGONFLY_TEST_URL"]
    async fn response_limits_only_tighten_until_reset() {
        let (gate, _) = integration_gate(10, 1, 10, Duration::from_secs(1)).await;
        let first = admitted(&gate).await;
        let reset = first.admitted_at_ms() + 60_000;
        let observation = ResponseObservation {
            minute_request_counter: Some(1),
            requests_per_minute_remaining: Some(4),
            requests_per_minute_reset_ms: Some(reset),
            success: true,
            ..Default::default()
        };
        gate.reconcile_response(observation).await.unwrap();
        gate.reconcile_response(observation).await.unwrap();
        first.release().await.unwrap();
        for _ in 0..4 {
            admitted(&gate).await.release().await.unwrap();
        }
        gate.reconcile_response(ResponseObservation {
            minute_request_counter: Some(1),
            requests_per_minute_remaining: Some(9),
            requests_per_minute_reset_ms: Some(reset),
            success: true,
            ..Default::default()
        })
        .await
        .unwrap();
        assert!(matches!(
            gate.try_admit_http().await.unwrap(),
            AdmissionDecision::Denied {
                kind: AdmissionDeniedKind::MinuteLimit,
                ..
            }
        ));
    }

    #[tokio::test]
    #[ignore = "requires UNUSUAL_WHALES_DRAGONFLY_TEST_URL"]
    async fn successful_zero_remaining_and_429_block_new_starts() {
        let (zero_gate, _) = integration_gate(10, 1, 10, Duration::from_secs(1)).await;
        let lease = admitted(&zero_gate).await;
        zero_gate
            .reconcile_response(ResponseObservation {
                requests_per_minute_remaining: Some(0),
                requests_per_minute_reset_ms: Some(lease.admitted_at_ms() + 5_000),
                success: true,
                ..Default::default()
            })
            .await
            .unwrap();
        lease.release().await.unwrap();
        assert!(matches!(
            zero_gate.try_admit_http().await.unwrap(),
            AdmissionDecision::Denied {
                kind: AdmissionDeniedKind::GlobalBlock,
                ..
            }
        ));

        let (rate_gate, _) = integration_gate(10, 1, 10, Duration::from_secs(1)).await;
        let lease = admitted(&rate_gate).await;
        rate_gate
            .reconcile_response(ResponseObservation {
                retry_after_until_ms: Some(lease.admitted_at_ms() + 5_000),
                ..Default::default()
            })
            .await
            .unwrap();
        lease.release().await.unwrap();
        assert!(matches!(
            rate_gate.try_admit_http().await.unwrap(),
            AdmissionDecision::Denied {
                kind: AdmissionDeniedKind::GlobalBlock,
                ..
            }
        ));
    }

    #[tokio::test]
    #[ignore = "requires UNUSUAL_WHALES_DRAGONFLY_TEST_URL"]
    async fn impossible_stored_minute_reset_is_cleared() {
        let (gate, url) = integration_gate(10, 1, 10, Duration::from_secs(1)).await;
        let lease = admitted(&gate).await;
        let impossible_reset = lease.admitted_at_ms() + 60_000_000;
        let client = redis::Client::open(url).unwrap();
        let mut connection = client.get_connection_manager().await.unwrap();
        redis::cmd("HSET")
            .arg(&gate.inner.keys.state)
            .arg("observed_minute_limit")
            .arg(1)
            .arg("observed_minute_used")
            .arg(1)
            .arg("observed_minute_reset_ms")
            .arg(impossible_reset)
            .arg("blocked_until_ms")
            .arg(impossible_reset)
            .query_async::<()>(&mut connection)
            .await
            .unwrap();
        lease.release().await.unwrap();

        let decision = tokio::time::timeout(Duration::from_millis(500), gate.admit_http())
            .await
            .unwrap()
            .unwrap();
        let lease = match decision {
            AdmissionDecision::Admitted(lease) => lease,
            decision => panic!("expected admission after reset repair, was {decision:?}"),
        };
        lease.release().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires UNUSUAL_WHALES_DRAGONFLY_TEST_URL"]
    async fn noscript_recovers_and_state_reset_fails_closed() {
        let (gate, url) = integration_gate(10, 1, 10, Duration::from_secs(1)).await;
        let client = redis::Client::open(url).unwrap();
        let mut connection = client.get_connection_manager().await.unwrap();
        redis::cmd("SCRIPT")
            .arg("FLUSH")
            .query_async::<()>(&mut connection)
            .await
            .unwrap();
        admitted(&gate).await.release().await.unwrap();

        redis::cmd("DEL")
            .arg(&gate.inner.keys.sentinel)
            .query_async::<()>(&mut connection)
            .await
            .unwrap();
        assert_eq!(
            gate.try_admit_http().await.unwrap_err(),
            CoordinationError::StateReset
        );
    }

    #[tokio::test]
    async fn unavailable_coordination_fails_closed() {
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            DragonflyGate::connect(
                "redis://127.0.0.1:9/",
                "0000000000000000000000000000000000000000000000000000000000000000",
                1,
                1,
                1,
                Duration::from_secs(1),
                Duration::from_secs(1),
            ),
        )
        .await;
        assert!(result.is_err() || result.unwrap().is_err());
    }
}
