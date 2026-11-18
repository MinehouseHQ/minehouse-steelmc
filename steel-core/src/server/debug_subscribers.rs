//! Vanilla `ServerDebugSubscribers` and the dedicated server's tick time chart
//! (`RemoteSampleLogger` for `RemoteDebugSampleType.TICK_TIME`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use rustc_hash::FxHashMap;
use steel_protocol::packet_traits::ClientPacket;
use steel_protocol::packets::game::{
    CDebugSample, DEDICATED_SERVER_TICK_TIME, RemoteDebugSampleType,
};
use steel_utils::locks::SyncMutex;

use super::Server;
use crate::player::Player;

/// Vanilla `TpsDebugDimensions`, the order of a tick time sample.
#[derive(Clone, Copy)]
enum TpsDebugDimension {
    /// The whole tick, from one logged sample to the next.
    FullTick = 0,
    /// The tick's own work, vanilla `tickServer`.
    TickServerMethod = 1,
    /// Work between ticks: finishing queued packets.
    ScheduledTasks = 2,
    /// Sleeping until the next tick.
    Idle = 3,
}

/// Players per debug subscription, rebuilt every tick.
#[derive(Default)]
pub(crate) struct DebugSubscribers {
    enabled: SyncMutex<FxHashMap<i32, Vec<Arc<Player>>>>,
}

/// Vanilla `DedicatedServer.tickTimeLogger` with `MinecraftServer`'s timing
/// hooks. It only samples while some operator watches the chart.
///
/// Steel sleeps at the start of a loop iteration, so a sample is sent once the
/// next iteration has slept and finished its queued packets: the same
/// tick, wait, log order as vanilla's `tickServer`, `waitUntilNextTick`,
/// `logFullTickTime`.
pub(super) struct TickTimeLogger {
    enabled: bool,
    sample: [i64; 4],
    last_full_tick: Instant,
    between_ticks_start: Instant,
    tick_work_start: Instant,
    idle: Duration,
}

impl TickTimeLogger {
    pub(super) fn new() -> Self {
        let now = Instant::now();
        Self {
            enabled: false,
            sample: [0; 4],
            last_full_tick: now,
            between_ticks_start: now,
            tick_work_start: now,
            idle: Duration::ZERO,
        }
    }

    /// Vanilla `waitForTasks` adds parked time to `idleTimeNanos`.
    pub(super) fn slept(&mut self, idle: Duration) {
        if self.enabled {
            self.idle += idle;
        }
    }

    /// The tick's queued packets are done: vanilla `finishMeasuringTaskExecutionTime`
    /// then `logFullTickTime`. The tick's own work starts now.
    pub(super) fn packets_drained(&mut self, server: &Server) {
        let now = Instant::now();
        if self.enabled {
            let between_ticks = now.saturating_duration_since(self.between_ticks_start);
            self.log_partial(
                TpsDebugDimension::ScheduledTasks,
                between_ticks.saturating_sub(self.idle),
            );
            self.log_partial(TpsDebugDimension::Idle, self.idle);
            self.log_full(server, now.saturating_duration_since(self.last_full_tick));
        }
        self.last_full_tick = now;
        self.tick_work_start = now;
    }

    /// Vanilla `logTickMethodTime` at the end of `tickServer`.
    pub(super) fn tick_work_finished(&mut self) {
        if self.enabled {
            let work = self.tick_work_start.elapsed();
            self.log_partial(TpsDebugDimension::TickServerMethod, work);
        }
    }

    /// Vanilla `startMeasuringTaskExecutionTime` and `endMetricsRecordingTick`,
    /// which decides whether the next tick is sampled.
    pub(super) fn tick_ended(&mut self, server: &Server) {
        self.between_ticks_start = Instant::now();
        self.idle = Duration::ZERO;
        self.enabled = server.has_debug_subscriber(DEDICATED_SERVER_TICK_TIME);
    }

    /// Vanilla `AbstractSampleLogger.logPartialSample`.
    fn log_partial(&mut self, dimension: TpsDebugDimension, time: Duration) {
        self.sample[dimension as usize] = nanos(time);
    }

    /// Vanilla `logSample` of the full tick: sends the sample and resets it.
    fn log_full(&mut self, server: &Server, full_tick: Duration) {
        self.sample[TpsDebugDimension::FullTick as usize] = nanos(full_tick);
        server.broadcast_to_debug_subscribers(
            DEDICATED_SERVER_TICK_TIME,
            CDebugSample {
                sample: self.sample.to_vec(),
                sample_type: RemoteDebugSampleType::TickTime,
            },
        );
        self.sample = [0; 4];
    }
}

fn nanos(time: Duration) -> i64 {
    i64::try_from(time.as_nanos()).unwrap_or(i64::MAX)
}

impl Server {
    /// Vanilla `ServerDebugSubscribers.tick`.
    pub(super) fn tick_debug_subscribers(&self) {
        let mut enabled: FxHashMap<i32, Vec<Arc<Player>>> = FxHashMap::default();
        self.online_players.iter_players(|_, player| {
            for subscription in player.debug_subscriptions() {
                enabled
                    .entry(subscription)
                    .or_default()
                    .push(Arc::clone(player));
            }
            true
        });
        *self.debug_subscribers.enabled.lock() = enabled;
    }

    /// Vanilla `ServerDebugSubscribers.hasAnySubscriberFor`.
    #[must_use]
    pub(crate) fn has_debug_subscriber(&self, subscription: i32) -> bool {
        self.debug_subscribers
            .enabled
            .lock()
            .get(&subscription)
            .is_some_and(|players| !players.is_empty())
    }

    /// Vanilla `ServerDebugSubscribers.broadcastToAll`.
    pub(crate) fn broadcast_to_debug_subscribers<P: ClientPacket + Clone>(
        &self,
        subscription: i32,
        packet: P,
    ) {
        let players = self
            .debug_subscribers
            .enabled
            .lock()
            .get(&subscription)
            .cloned()
            .unwrap_or_default();
        for player in players {
            player.send_packet(packet.clone());
        }
    }
}
