use std::io::{Result, Write};

use simdnbt::owned::NbtTag;
use steel_macros::ClientPacket;
use steel_registry::packets::play::C_SHOW_DIALOG;
use steel_utils::codec::VarInt;
use steel_utils::serial::WriteTo;

/// Shows a dialog defined inline. In play vanilla sends a `Holder<Dialog>`: registry id + 1,
/// or 0 followed by the dialog itself, which is what this packet always writes.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_SHOW_DIALOG)]
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
        VarInt(0).write(writer)?;
        WriteTo::write(&self.dialog, writer)
    }
}
