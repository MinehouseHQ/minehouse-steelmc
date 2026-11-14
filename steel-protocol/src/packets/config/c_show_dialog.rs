use std::io::{Result, Write};

use simdnbt::owned::NbtTag;
use steel_macros::ClientPacket;
use steel_registry::packets::config::C_SHOW_DIALOG;
use steel_utils::serial::WriteTo;

/// Shows a dialog defined inline. In the configuration phase the client has no dialog
/// registry yet, so vanilla sends the dialog itself (`CONTEXT_FREE_STREAM_CODEC`).
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Config = C_SHOW_DIALOG)]
pub struct CShowDialog {
    pub dialog: NbtTag,
}

impl CShowDialog {
    #[must_use]
    pub const fn new(dialog: NbtTag) -> Self {
        Self { dialog }
    }
}

impl WriteTo for CShowDialog {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        WriteTo::write(&self.dialog, writer)
    }
}
