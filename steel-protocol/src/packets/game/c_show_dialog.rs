use std::io::{Result, Write};

use simdnbt::owned::NbtTag;
use steel_macros::ClientPacket;
use steel_registry::packets::play::C_SHOW_DIALOG;
use steel_utils::codec::VarInt;
use steel_utils::serial::WriteTo;

/// Vanilla `ClientboundShowDialogPacket` in play: a `Holder<Dialog>`, either a
/// dialog of the synced registry or one defined inline.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_SHOW_DIALOG)]
pub struct CShowDialog {
    pub dialog: DialogHolder,
}

/// Vanilla `ByteBufCodecs.holder` for a dialog.
#[derive(Clone, Debug)]
pub enum DialogHolder {
    /// A dialog of the registry, by its network id.
    Reference(i32),
    /// A dialog defined inline, as its encoded NBT.
    Direct(NbtTag),
}

impl CShowDialog {
    /// Shows a dialog defined inline.
    #[must_use]
    pub const fn new(dialog: NbtTag) -> Self {
        Self {
            dialog: DialogHolder::Direct(dialog),
        }
    }

    /// Shows the registry dialog with network id `id`.
    #[must_use]
    pub const fn reference(id: i32) -> Self {
        Self {
            dialog: DialogHolder::Reference(id),
        }
    }
}

impl WriteTo for CShowDialog {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        match &self.dialog {
            // Vanilla writes the registry id plus one; zero marks a direct holder.
            DialogHolder::Reference(id) => VarInt(id + 1).write(writer),
            DialogHolder::Direct(dialog) => {
                VarInt(0).write(writer)?;
                WriteTo::write(dialog, writer)
            }
        }
    }
}
