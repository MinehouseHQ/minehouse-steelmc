//! Clientbound debug sample, vanilla `ClientboundDebugSamplePacket`.

use std::io::{Result, Write};

use steel_macros::ClientPacket;
use steel_registry::packets::play::C_DEBUG_SAMPLE;
use steel_utils::codec::VarInt;
use steel_utils::serial::WriteTo;

/// Vanilla `RemoteDebugSampleType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteDebugSampleType {
    /// The dedicated server's tick time chart in the client's debug screen.
    TickTime,
}

/// One sample of a remote debug chart, sent to players subscribed to it.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_DEBUG_SAMPLE)]
pub struct CDebugSample {
    /// The sample's values; for tick time, vanilla `TpsDebugDimensions` in nanoseconds.
    pub sample: Vec<i64>,
    /// Which chart the sample belongs to.
    pub sample_type: RemoteDebugSampleType,
}

impl WriteTo for CDebugSample {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        let length = i32::try_from(self.sample.len()).unwrap_or(i32::MAX);
        VarInt(length).write(writer)?;
        for value in &self.sample {
            value.write(writer)?;
        }
        let ordinal = match self.sample_type {
            RemoteDebugSampleType::TickTime => 0,
        };
        VarInt(ordinal).write(writer)
    }
}
